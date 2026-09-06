//! `generate_pdf__*` step dispatch — render a document and persist it.
//!
//! Mirrors [`crate::email`]'s `email_send__*` dispatch: the caller
//! threads a [`DocumentPayload`] through the signal `value`, and the
//! worker (the `workflows-service` `NotationService` in prod, the
//! in-process [`crate::DispatchingRuntime`] in dev/tests) renders the
//! PDF and persists it via [`cloud::StorageService`] when a transition
//! lands on a `generate_pdf__*` state.
//!
//! Why thread the payload instead of reloading template + answers from
//! the database here: it keeps one data path (the same one EmailSend
//! uses) and keeps this crate free of the intake-side substitution
//! logic. The caller (`portal::retainer_walk`) does the templating
//! (template body + answers → Typst source); this step only renders
//! that source and stores the bytes.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Everything the worker needs to produce and persist one document.
/// Carried (JSON, internally tagged on `kind`) as the `value` of the
/// signal that lands on a `generate_pdf__*` state. Two production
/// modes share one dispatch:
///
/// - [`DocumentPayload::Typst`] — render fresh Typst source to a PDF
///   (the retainer and other generated documents).
/// - [`DocumentPayload::Acroform`] — fill an existing fillable
///   government form (AcroForm) fetched from storage with field values
///   (Nevada SoS articles, IRS 990, …). Output is
///   **attorney-review-ready, never auto-filed** — the workflow spec
///   parks it at `lawyer_review` before any filing step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DocumentPayload {
    /// Render Typst `typst_source` to a PDF and persist it at
    /// `storage_key`. `typst_source` is the final document source with
    /// every `{{placeholder}}` already resolved by the caller — not the
    /// markdown template body, not the HTML preview.
    Typst {
        storage_key: String,
        typst_source: String,
    },
    /// Fetch the blank form at `blank_form_key`, fill its AcroForm
    /// `/Fields` from `fields` (name → value), **flatten** the result to
    /// static page content, and persist it at `storage_key`. The workflow
    /// spec reaches this fill only after `lawyer_review`
    /// ([`crate::lawyer_review_precedes_submission`]), so flattening here
    /// freezes exactly what an attorney approved — no downstream viewer
    /// can re-edit a value before it reaches a government office.
    Acroform {
        storage_key: String,
        blank_form_key: String,
        fields: std::collections::BTreeMap<String, String>,
    },
}

/// Errors from rendering / filling or persisting a document step.
#[derive(Debug, thiserror::Error)]
pub enum DocumentError {
    #[error("pdf: {0}")]
    Pdf(#[from] pdf::PdfError),
    #[error("storage: {0}")]
    Storage(#[from] cloud::StorageError),
    #[error("database: {0}")]
    Db(String),
    #[error("ingest: {0}")]
    Ingest(#[from] store::documents::IngestError),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("notation {0} not found")]
    NotationNotFound(uuid::Uuid),
    #[error("template {0} not found")]
    TemplateNotFound(uuid::Uuid),
    #[error("template: {0}")]
    Template(#[from] store::templates::TemplateError),
    #[error("notation store: {0}")]
    Notation(#[from] store::notations::NotationError),
    #[error(
        "template {0} has no declared kind; a generate_pdf step cannot classify its asset \
         (declare `kind:` in the template's frontmatter and re-save it)"
    )]
    MissingKind(uuid::Uuid),
}

impl From<String> for DocumentError {
    fn from(message: String) -> Self {
        Self::Db(message)
    }
}

/// The reference recorded on the `generate_pdf` transition's
/// `notation_events.payload` — where the rendered PDF landed in
/// content-addressed object storage, plus the `assets` row that files it
/// into the matter. Identifiers only (asset id, sha, key, size): the
/// document *content* stays in object storage, never the journal — the
/// same no-content-in-the-journal rule `workflow_payload` holds to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedPdfRef {
    /// The `assets` row id for the persisted PDF.
    pub asset_id: uuid::Uuid,
    pub sha256: String,
    /// Project-scoped content-addressed storage key of the asset.
    pub storage_key: String,
    pub byte_size: i64,
}

/// Produce the document, persist it, and file it into the matter as a
/// content-addressed document `assets` row. The single side effect of a
/// `generate_pdf` step; callers wrap it in `ctx.run` (worker) or call it
/// inline (`DispatchingRuntime`) so it is journaled / idempotent on
/// replay. Idempotent by construction: the same payload writes the same
/// bytes to the same object key, and `ingest_bytes` dedups storage by
/// SHA-256.
///
/// Returns the JSON [`GeneratedPdfRef`] the caller records on the
/// `generate_pdf` transition's `notation_events.payload` — so the audit
/// trail links straight to the persisted intermediary PDF. When `db` is
/// `None` (the render-only unit path) the asset write and the payload are
/// skipped and this returns `None`; the object-storage PDF the signature
/// step reads back is written either way.
///
/// The persisted asset's `kind` is the notation's pinned template's
/// declared `kind:` — **not** a slug derived from the `generate_pdf__*`
/// state name. A template reaches this point only after S103/S104 have
/// validated its `kind:` (or the notation opened before that gate
/// existed), so a `None` here surfaces as [`DocumentError::MissingKind`]
/// rather than silently defaulting to `"generated"`. That classification
/// is resolved *before* the PDF is written to object storage, so a
/// `MissingKind` (terminal, never resolves on replay) rejects the
/// dispatch without leaving an orphaned object behind.
pub async fn dispatch_generate_pdf(
    storage: &Arc<dyn cloud::StorageService>,
    surreal: Option<&store::surreal::SurrealDb>,
    notation_id: uuid::Uuid,
    payload: &DocumentPayload,
) -> Result<Option<String>, DocumentError> {
    let (bytes, storage_key) = match payload {
        DocumentPayload::Typst {
            storage_key,
            typst_source,
        } => (pdf::render(typst_source)?, storage_key.as_str()),
        DocumentPayload::Acroform {
            storage_key,
            blank_form_key,
            fields,
        } => {
            let blank = storage.get(blank_form_key).await?.bytes;
            let filled = pdf::fill_acroform(&blank, fields)?;
            // Flatten to static content before persisting: this fill sits
            // past lawyer_review, so nothing downstream may re-edit the
            // approved values on their way to a government office.
            (pdf::flatten(&filled)?, storage_key.as_str())
        }
    };
    // Resolve the notation's project and its pinned template's declared
    // kind BEFORE persisting any bytes. A legacy template with no declared
    // kind surfaces as [`DocumentError::MissingKind`], a terminal failure
    // that can never succeed on replay; resolving it ahead of the write
    // means a rejected dispatch never leaves an orphaned PDF in object
    // storage with no matching `assets` row or audit-trail entry. The
    // render-only unit path (no handle) files no asset, so it skips this
    // classification and just persists the bytes below. The notation and
    // the template it pinned are both SurrealDB-resident, so only the
    // Surreal handle is needed here.
    let classified = match surreal {
        Some(surreal) => Some(notation_project_and_template_kind(surreal, notation_id).await?),
        None => None,
    };

    storage.put(storage_key, &bytes, "application/pdf").await?;

    // Without the store handles (the render-only unit path) there is
    // nothing to file the asset into; the object-storage PDF above is all
    // the signature step needs to read back.
    let (Some(surreal), Some((project_id, kind))) = (surreal, classified) else {
        return Ok(None);
    };

    // File the rendered bytes as a content-addressed document `assets`
    // row — the same lane the inbound `document_intake` step writes
    // through.
    let filename = storage_key.rsplit('/').next().unwrap_or("document.pdf");
    // The bytes live at two keys: the Project-scoped content-addressed key the
    // asset points at, and `storage_key` (the notation key the attest /
    // signature steps and the portal read back). Record that second location
    // on the asset row — in the same insert — so every asset row for these
    // bytes carries it and a governed expunge deletes every copy (#470).
    let ingested = store::documents::ingest_bytes(
        surreal,
        storage,
        &store::documents::IngestArgs {
            project_id,
            source: store::documents::source::GENERATED,
            filename,
            kind: &kind,
            content_type: "application/pdf",
            description: None,
            secondary_storage_key: Some(storage_key),
            // `doc_kind` spans both client-facing artifacts (retainer,
            // government filings) and internal ones (`review_memo`); the
            // per-kind classification is the asset-lane identity work filed
            // alongside #782, so this stays internal until that lands. The
            // client already reads these back via the notation
            // download/`exists()` path, not this listing.
            visibility: store::documents::visibility::INTERNAL,
        },
        &bytes,
    )
    .await?;

    let pdf_ref = GeneratedPdfRef {
        asset_id: ingested.asset_id,
        storage_key: ingested.storage_key,
        sha256: ingested.sha256_hex,
        byte_size: ingested.byte_size,
    };
    Ok(Some(serde_json::to_string(&pdf_ref)?))
}

/// Resolve a notation to the project it belongs to and its pinned
/// template's declared `kind`. `ingest_bytes` is project-scoped, but step
/// dispatch only carries the notation id, so this fetches the notation
/// once and follows `template_id` to read the template's classification.
async fn notation_project_and_template_kind(
    surreal: &store::surreal::SurrealDb,
    notation_id: uuid::Uuid,
) -> Result<(uuid::Uuid, String), DocumentError> {
    let notation = store::notations::find_by_id(surreal, notation_id)
        .await?
        .ok_or(DocumentError::NotationNotFound(notation_id))?;
    let template = store::templates::find_by_id(surreal, notation.template_id)
        .await?
        .ok_or(DocumentError::TemplateNotFound(notation.template_id))?;
    let kind = template
        .kind
        .ok_or(DocumentError::MissingKind(template.id))?;
    Ok((notation.project_id, kind))
}

#[cfg(test)]
mod tests {
    use super::{dispatch_generate_pdf, DocumentPayload};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    async fn fs_storage() -> Arc<dyn cloud::StorageService> {
        Arc::new(
            cloud::FsStorage::new(std::env::temp_dir().join("navigator-document-dispatch-test"))
                .await
                .expect("temp FsStorage"),
        )
    }

    #[tokio::test]
    async fn typst_dispatch_renders_and_persists_a_pdf_at_the_key() {
        let storage = fs_storage().await;
        let payload = DocumentPayload::Typst {
            storage_key: "notations/doc-test/retainer.pdf".into(),
            typst_source: "Hello, retainer.".into(),
        };
        // No store handles here: the render-only path persists the
        // object-storage PDF and skips the `asset` file (returns `None`).
        let recorded = dispatch_generate_pdf(&storage, None, uuid::Uuid::from_u128(1), &payload)
            .await
            .expect("dispatch succeeds");
        assert!(recorded.is_none(), "no journal payload without the store");

        let stored = storage
            .get("notations/doc-test/retainer.pdf")
            .await
            .expect("object persisted");
        assert_eq!(stored.content_type, "application/pdf");
        // A real PDF starts with the `%PDF` magic bytes.
        assert!(
            stored.bytes.starts_with(b"%PDF"),
            "expected PDF magic bytes, got {:?}",
            stored.bytes.get(..8)
        );
    }

    #[tokio::test]
    async fn acroform_dispatch_fills_flattens_and_persists_a_form() {
        let storage = fs_storage().await;
        // Stage a blank fillable form in storage, then dispatch a fill.
        let blank = pdf::blank_acroform(&["entity_name"]);
        storage
            .put("forms/nv_articles.pdf", &blank, "application/pdf")
            .await
            .unwrap();

        let mut fields = BTreeMap::new();
        fields.insert("entity_name".to_string(), "Neon Law LLC".to_string());
        let payload = DocumentPayload::Acroform {
            storage_key: "notations/acro-test/nv_articles.pdf".into(),
            blank_form_key: "forms/nv_articles.pdf".into(),
            fields,
        };
        dispatch_generate_pdf(&storage, None, uuid::Uuid::from_u128(2), &payload)
            .await
            .expect("acroform dispatch succeeds");

        let stored = storage
            .get("notations/acro-test/nv_articles.pdf")
            .await
            .expect("filled form persisted");
        // The persisted packet is flattened: no interactive fields remain,
        // yet the filled value is still readable as static page content.
        assert!(
            pdf::field_names(&stored.bytes).expect("parses").is_empty(),
            "the filed packet must carry no re-editable form fields"
        );
        assert!(
            pdf::page_text(&stored.bytes)
                .expect("extract text")
                .contains("Neon Law LLC"),
            "the reviewed value must survive as static content"
        );
    }

    #[tokio::test]
    async fn payload_is_internally_tagged_on_kind() {
        // Pin the wire shape so web and the worker stay in sync.
        let typst = serde_json::to_value(DocumentPayload::Typst {
            storage_key: "k".into(),
            typst_source: "s".into(),
        })
        .unwrap();
        assert_eq!(typst["kind"], "typst");
        let acro = serde_json::to_value(DocumentPayload::Acroform {
            storage_key: "k".into(),
            blank_form_key: "b".into(),
            fields: BTreeMap::new(),
        })
        .unwrap();
        assert_eq!(acro["kind"], "acroform");
    }
}
