//! Project-document HTTP surface:
//!
//! - `POST /app/projects/{project_code}/documents/upload` — multipart upload
//!   that pipes bytes through [`store::documents::ingest_bytes`]. The
//!   picker is `multiple`, so one submission may carry a batch of
//!   files; each becomes its own document.
//! - `GET /app/projects/{project_code}/documents/:doc_id` — per-document
//!   detail page showing full provenance.
//! - `GET /app/projects/{project_code}/documents/:doc_id/download` — issues
//!   a 302 to a short-lived signed URL on the storage backend, or
//!   streams bytes through the app on backends that can't sign
//!   (`FsStorage` in local dev).
//!
//! # Authorization model
//!
//! Three layers gate every request before bytes leave the building:
//!
//! 1. **Admin sub-router** — CSRF, session, and embedded Rego policy are
//!    already enforced by middleware before any handler in this
//!    module runs. An unauthenticated request never reaches us.
//! 2. **Cross-project + visibility guard** — every handler resolves
//!    the document via [`load_doc_for_project`] and 404s if
//!    `document.project_id` doesn't match the `:id` segment of the
//!    URL. A user who guesses or steals a `doc_id` from another
//!    project can't tunnel it through their own project's URL. Under
//!    the client lens it also 404s an
//!    `internal` asset, so a client can't fetch internal work product
//!    on their own matter by guessing a `doc_id` — matching the gate
//!    the filename list and ZIP export already apply (#782).
//! 3. **Signed-URL handoff** — only after layers 1+2 pass do we ask
//!    the storage backend for a signed URL. The URL itself carries
//!    an HMAC of the canonical request signed by the GCS service
//!    account's private key; GCS rejects any request to the bucket
//!    that doesn't carry a valid signature. Bytes never proxy
//!    through this pod in production — the browser fetches direct
//!    from GCS, the app's role is to *decide* whether to issue the
//!    URL in the first place.
//!
//! Production uses GCS V4 signing; local dev uses `FsStorage` which
//! has no signing concept, so the handler falls back to streaming
//! bytes through the app. Same Rust code path, two backends.

use std::time::Duration;

use axum::body::Body;
use axum::extract::{Extension, Multipart, Path as AxumPath, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use store::documents::{source, IngestArgs};
use tower_cookies::Cookies;
use uuid::Uuid;

use crate::admin::AdminState;
use crate::session::SessionData;

/// Signed-URL validity window for project documents.
///
/// A signed URL is the *only* credential the browser presents to
/// GCS — the URL contains an HMAC-SHA256 signature over the bucket,
/// object key, expiry, and HTTP method, signed by the service
/// account's RSA private key. GCS verifies the signature on every
/// request and rejects unsigned hits to the bucket. That means the
/// TTL is the URL's full security lifetime: anyone who obtains the
/// URL (legitimate user, screenshot, browser history, Slack paste,
/// dev-tools spectator on a shared screen) can fetch the bytes
/// until expiry, with no further auth check.
///
/// One hour is the product call. Trade-off:
///
/// - Shorter (e.g. 5 min, what retainer PDFs use in
///   [`crate::documents`]) tightens the leak window but breaks the
///   "lawyer opens the page, gets pulled into a call, comes back,
///   clicks Download" flow — they'd hit a 403 and have to refresh.
/// - Longer (24h, the user's stated upper bound) survives same-day
///   share-via-Slack but means a URL caught in someone else's
///   browser history is usable for the rest of the day.
/// - One hour fits a typical work session: long enough for normal
///   interruptions, short enough that a leak goes stale before the
///   next coffee break.
///
/// GCS V4 caps signed-URL TTL at 7 days; we're well inside the
/// bound. Bump cautiously: every hour added is another hour a leaked
/// URL stays live.
const SIGNED_URL_TTL: Duration = Duration::from_hours(1);

/// Most files one submission may carry.
///
/// The batch is held in memory until the whole multipart body has been
/// read — `kind` and `description` post *after* the files, so a file
/// can't be filed the moment it arrives. That buffer is what makes a
/// ceiling necessary: without one, an authenticated lawyer session could
/// post an unbounded number of parts and exhaust the process, taking
/// other requests down with it. Fifty is far above a realistic
/// discovery batch and far below anything that threatens the pod.
const MAX_BATCH_FILES: usize = 50;

/// Most bytes one submission may carry across every file in it.
///
/// The per-file ceiling is the same number: one 500 MB file and fifty
/// 10 MB files cost the same memory, so the limit that matters is the
/// total. Sized for scanned-PDF discovery batches, which is the heaviest
/// legitimate traffic this route sees.
///
/// `pub(crate)` so `admin.rs` can raise Axum's own default ~2 MB body
/// limit to match on this route — without that layer, Axum rejects a
/// multi-megabyte PDF long before this constant's check ever runs.
pub(crate) const MAX_BATCH_BYTES: usize = 500 * 1024 * 1024;

/// `POST /app/projects/{project_code}/documents/upload`.
pub async fn upload(
    State(state): State<AdminState>,
    AxumPath(project_code): AxumPath<String>,
    cookies: Cookies,
    session: Option<Extension<SessionData>>,
    mut multipart: Multipart,
) -> Response {
    let Some(Extension(session_data)) = session else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(project_id) = store::projects::id_for_code(&state.surreal, &project_code).await else {
        return StatusCode::NOT_FOUND.into_response();
    };

    // A matter-surface write: the matter's gate (a participation row of every
    // tier) plus the lawyer-tier check, which that gate does not make — a client
    // on their own matter reaches the page but must not file to it.
    if !session_data.role.is_lawyer_tier()
        || !store::access::can_see_project(
            &state.surreal,
            session_data.person_id,
            session_data.role,
            project_id,
        )
        .await
        .unwrap_or(false)
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    // Bail early if the project doesn't exist, so a bad id is a friendly 404
    // rather than a later failure mid-ingest.
    let Ok(Some(_)) = store::projects::find_by_id(&state.surreal, project_id).await else {
        return StatusCode::NOT_FOUND.into_response();
    };

    // The multipart CSRF check the middleware can't do without buffering
    // the upload: the form renders `_csrf` first, so verify it before
    // reading the file field. Bearer callers carry no cookie and are
    // exempt, matching `require_csrf`.
    if let Err(status) =
        crate::csrf::require_multipart_csrf(&cookies, &session_data, &mut multipart).await
    {
        return status.into_response();
    }

    let batch = match read_upload_batch(&mut multipart).await {
        Ok(batch) => batch,
        Err(BatchError::Malformed) => return StatusCode::BAD_REQUEST.into_response(),
        Err(BatchError::TooLarge) => {
            tracing::warn!(%project_id, "project document upload exceeded the batch ceiling");
            return StatusCode::PAYLOAD_TOO_LARGE.into_response();
        }
    };
    if batch.files.is_empty() {
        return Redirect::to(
            &crate::dioxus_app::project_show_path(&state.surreal, project_id).await,
        )
        .into_response();
    }
    let UploadBatch {
        files: uploads,
        kind,
        description,
        visibility,
    } = batch;
    let description_trimmed = description.as_deref();

    // File the upload as the lawyer/admin who uploaded it, so the matter
    // repo's `git log` attributes it to them.
    let (author_name, author_email) = uploader_identity(&state.surreal, Some(session_data)).await;

    // Ingest sequentially: each file is its own `assets` row and its own
    // commit in the matter repo, so a batch of five reads in `git log` the
    // same way five single uploads would. A failure part-way through
    // leaves the already-filed documents in place rather than rolling the
    // batch back — the lawyer sees what landed and can re-send the
    // rest, which beats silently discarding good uploads.
    //
    // That only works because re-sending is safe: `already_filed` skips a
    // file this project already holds under the same name and content
    // hash, so retrying the whole batch tops up the missing documents
    // instead of duplicating the ones that landed the first time.
    for upload in &uploads {
        if let Err(status) = file_one(
            &state,
            project_id,
            upload,
            &kind,
            description_trimmed,
            visibility,
            (&author_name, &author_email),
        )
        .await
        {
            return status.into_response();
        }
    }

    Redirect::to(&crate::dioxus_app::project_show_path(&state.surreal, project_id).await)
        .into_response()
}

/// File one document from a batch, skipping it when the matter already
/// holds it. `Err(status)` is the response the caller must return.
async fn file_one(
    state: &AdminState,
    project_id: Uuid,
    upload: &UploadedFile,
    kind: &str,
    description: Option<&str>,
    visibility: &str,
    author: (&str, &str),
) -> Result<(), StatusCode> {
    let file_name = upload
        .file_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(|| format!("upload-{project_id}"), str::to_string);
    let content_type = upload
        .content_type
        .as_deref()
        .unwrap_or("application/octet-stream");

    match already_filed(&state.surreal, project_id, &file_name, &upload.bytes).await {
        Ok(Some(_existing)) => {
            tracing::info!(
                %project_id,
                filename = %file_name,
                "skipping a project document this matter already holds"
            );
            // The bytes and filename are unchanged, so re-ingesting would
            // only duplicate the row — but a lawyer re-submitting the same
            // file under a new visibility choice must not have that choice
            // silently dropped. A matter can hold several duplicate rows for
            // one file (uploads filed before dedup existed, concurrent
            // submissions, or other ingest paths), and the dedup lookup
            // returns only one of them, so sync EVERY matching row — updating
            // just one would leave a duplicate at its old visibility and its
            // filename/bytes reachable through the client list and ZIP export
            // (#782 follow-up: Greptile P1).
            if let Err(e) = sync_visibility(
                &state.surreal,
                project_id,
                &file_name,
                &upload.bytes,
                visibility,
            )
            .await
            {
                tracing::error!(
                    error = %e, %project_id, filename = %file_name,
                    "failed to sync visibility on a re-upload of an existing document"
                );
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
            return Ok(());
        }
        Ok(None) => {}
        Err(e) => {
            tracing::error!(error = %e, %project_id, filename = %file_name, "dedup lookup failed");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    let args = IngestArgs {
        project_id,
        source: source::UPLOAD,
        filename: &file_name,
        kind,
        content_type,
        description,
        secondary_storage_key: None,
        visibility,
    };
    let (author_name, author_email) = author;
    let author = repos::Author {
        name: author_name,
        email: author_email,
    };

    crate::matter_documents::record_document(
        &state.surreal,
        &state.storage,
        author,
        &args,
        &upload.bytes,
    )
    .await
    .map_err(|e| {
        tracing::error!(
            project_id = %project_id,
            filename = %file_name,
            error = %e,
            "project document upload failed"
        );
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(())
}

/// Why a multipart upload body could not be turned into an
/// [`UploadBatch`].
enum BatchError {
    /// The multipart stream itself was unreadable.
    Malformed,
    /// The batch broke [`MAX_BATCH_FILES`] or [`MAX_BATCH_BYTES`].
    TooLarge,
}

/// The existing document `assets` row, when this project already holds one
/// with the same filename and the same bytes.
///
/// This is what makes re-sending a partially-failed batch safe. Ingest
/// dedupes the stored *object* by content hash but still writes a fresh
/// `assets` row every call, so without this check a retry would leave
/// the matter showing two rows for one file. Filename is part of the key
/// because the same bytes filed under two names are two documents.
async fn already_filed(
    surreal: &store::surreal::SurrealDb,
    project_id: Uuid,
    filename: &str,
    bytes: &[u8],
) -> Result<Option<store::assets::Asset>, store::assets::AssetError> {
    let sha_hex = store::documents::sha256_hex(bytes);
    store::assets::find_filed_copy(surreal, project_id, filename, &sha_hex).await
}

/// Sync `visibility` across *every* row this matter holds for a re-uploaded
/// document (same filename, same bytes), used when a lawyer re-uploads a
/// document the matter already holds but picks a different visibility. The
/// dedup skip must not drop that explicit choice, and because a matter can
/// hold several duplicate rows for one file (uploads filed before dedup
/// existed, concurrent submissions, or other ingest paths), updating only
/// the one row the dedup lookup happened to return would leave a duplicate
/// at its old visibility — a re-upload as `internal` could otherwise leave
/// a duplicate still client-visible and its bytes reachable through the
/// client list and ZIP export. The `visibility != target` filter touches
/// only the divergent duplicates, so this is a no-op when they already match.
async fn sync_visibility(
    surreal: &store::surreal::SurrealDb,
    project_id: Uuid,
    filename: &str,
    bytes: &[u8],
    visibility: &str,
) -> Result<(), store::assets::AssetError> {
    let sha_hex = store::documents::sha256_hex(bytes);
    store::assets::sync_visibility(surreal, project_id, filename, &sha_hex, visibility).await?;
    Ok(())
}

/// One file part pulled off a multipart upload, held until the whole
/// batch has been read off the wire.
struct UploadedFile {
    file_name: Option<String>,
    content_type: Option<String>,
    bytes: Vec<u8>,
}

/// Everything one submission of the upload form carries: the files the
/// picker selected plus the batch-level metadata that applies to all of
/// them.
struct UploadBatch {
    files: Vec<UploadedFile>,
    /// A `rules::kind::Kind` valid for `Lane::Asset`. Already narrowed to
    /// `unclassified` when the form left it blank or sent anything else —
    /// the honest value wins over a malformed submission, exactly as the
    /// safe tier does for `visibility` below.
    kind: String,
    /// `None` when the form left it blank, so it isn't stored as `""`.
    description: Option<String>,
    /// `store::documents::visibility::INTERNAL` or `::CLIENT`. Already
    /// defaulted to `INTERNAL` when the form left it blank or sent anything
    /// else — the safe tier wins over a malformed submission.
    visibility: &'static str,
}

/// Drain the multipart body into an [`UploadBatch`].
///
/// The picker is `multiple`, so the browser posts one `file` part per
/// selected file, all under the same field name — this collects every
/// one rather than taking the first. `kind` and `description` are
/// single-valued and apply to the whole batch.
///
/// [`BatchError::Malformed`] means the multipart stream was unreadable
/// (a `400`); [`BatchError::TooLarge`] means it broke a batch ceiling
/// (a `413`).
async fn read_upload_batch(multipart: &mut Multipart) -> Result<UploadBatch, BatchError> {
    let mut files: Vec<UploadedFile> = Vec::new();
    let mut kind: Option<String> = None;
    let mut description: Option<String> = None;
    let mut visibility: Option<String> = None;
    let mut total_bytes: usize = 0;

    loop {
        let next = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(_) => return Err(BatchError::Malformed),
        };
        let name = next.name().map(String::from);
        match name.as_deref() {
            Some("file") => {
                let file_name = next.file_name().map(String::from);
                let content_type = next.content_type().map(String::from);
                let bytes = match next.bytes().await {
                    Ok(b) => b.to_vec(),
                    Err(_) => return Err(BatchError::Malformed),
                };
                // A picker with nothing selected still posts one part —
                // no filename, zero bytes. That is "nothing selected".
                // A *named* part is a real selection and is kept even when
                // it is empty, so a zero-byte file the lawyer chose
                // is filed rather than vanishing from the batch without a
                // word.
                let named = file_name
                    .as_deref()
                    .map(str::trim)
                    .is_some_and(|n| !n.is_empty());
                if !named && bytes.is_empty() {
                    continue;
                }

                total_bytes = total_bytes.saturating_add(bytes.len());
                if files.len() >= MAX_BATCH_FILES || total_bytes > MAX_BATCH_BYTES {
                    return Err(BatchError::TooLarge);
                }

                files.push(UploadedFile {
                    file_name,
                    content_type,
                    bytes,
                });
            }
            Some("kind") => kind = next.text().await.ok(),
            Some("description") => description = next.text().await.ok(),
            Some("visibility") => visibility = next.text().await.ok(),
            _ => {}
        }
    }

    Ok(UploadBatch {
        files,
        // `store::documents::ingest_bytes` refuses a kind outside the asset
        // lane, and refusing is right for a boundary whose job is
        // classification. But a lawyer's document must not be lost to a
        // malformed POST, so the door narrows an unrecognized value to
        // `unclassified` here instead: the file lands, honestly labelled as
        // unclassified, and the matter's lifecycle warnings are untouched.
        // The form itself only offers lane-valid values, so this is the
        // hand-crafted-submission path, not the ordinary one.
        kind: kind
            .as_deref()
            .map(str::trim)
            .filter(|k| {
                rules::kind::Kind::parse(k).is_some_and(|k| k.valid_for(rules::kind::Lane::Asset))
            })
            .map_or_else(
                || rules::kind::Kind::Unclassified.as_str().to_string(),
                str::to_string,
            ),
        description: description
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        visibility: match visibility.as_deref().map(str::trim) {
            Some(store::documents::visibility::CLIENT) => store::documents::visibility::CLIENT,
            _ => store::documents::visibility::INTERNAL,
        },
    })
}

/// Resolve the uploader's `(name, email)` for git authorship from their
/// session. Prefers the linked `persons` row (faithful name + email);
/// falls back to the session email, then to a neutral placeholder so a
/// commit is never blocked on a missing identity.
pub(crate) async fn uploader_identity(
    surreal: &store::surreal::SurrealDb,
    session: Option<SessionData>,
) -> (String, String) {
    if let Some(session) = session {
        if let Some(pid) = session.person_id {
            if let Ok(Some(p)) = store::persons::find_by_id(surreal, pid).await {
                return (p.name, p.email);
            }
        }
        if let Some(email) = session.email {
            return (email.clone(), email);
        }
    }
    (
        "Neon Law Navigator lawyer".to_string(),
        "lawyer@localhost".to_string(),
    )
}

/// `GET /app/projects/{project_code}/documents/:doc_id/download`. Resolves
/// the document, blocks cross-project leakage, then either 302s to
/// a signed URL or streams bytes through the app.
pub async fn download(
    State(state): State<AdminState>,
    AxumPath((project_code, doc_id)): AxumPath<(String, Uuid)>,
    session: Option<Extension<SessionData>>,
) -> Response {
    // A code naming no matter is the same 404 a caller off the matter gets. A
    // store *failure* is not a miss, though: it stays a 500, so an outage does
    // not read to every client as "your document is gone".
    let project_id = match store::projects::find_by_code(&state.surreal, &project_code).await {
        Ok(Some(project)) => project.id,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            tracing::error!(error = %error, %project_code, "project document download: matter lookup failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    match can_read_project_document(&state, session.as_deref(), project_id).await {
        Ok(true) => {}
        Ok(false) => {
            tracing::info!(%project_id, %doc_id, "project document download denied by access policy");
            return StatusCode::NOT_FOUND.into_response();
        }
        Err(e) => {
            tracing::error!(error = %e, %project_id, %doc_id, "project document access check failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    // The client lens still never resolves an `internal` asset (#782); the
    // tier is what selects it now that one path serves both sides.
    let lens = store::access::ProjectLens::for_role(
        session
            .as_deref()
            .map_or(store::persons::Role::Client, |s| s.role),
    );
    let doc = match load_doc_for_project(&state, project_id, doc_id, lens).await {
        Ok(Some(asset)) => asset,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(
                error = %e,
                %project_id,
                %doc_id,
                "db error loading project document for download"
            );
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let filename = doc.filename.as_deref().unwrap_or("document");

    match state
        .storage
        .signed_url(&doc.storage_key, SIGNED_URL_TTL)
        .await
    {
        Ok(url) => Redirect::temporary(&url).into_response(),
        Err(cloud::StorageError::Unsupported(_)) => {
            stream_through(state, &doc.storage_key, &doc.content_type, filename).await
        }
        Err(cloud::StorageError::NotFound(_)) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(
                error = %e,
                %project_id,
                %doc_id,
                storage_key = %doc.storage_key,
                "signed_url failed for project document"
            );
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `Ok(false)` is a refusal; `Err` is a store failure.
///
/// These are deliberately different answers. Collapsing a failed access query
/// into "not a participant" reports a pool or query outage as a missing
/// document, which is the masking `project_document_*_500s_on_database_error`
/// exists to prevent — and it matters more now that the gate itself reads the
/// participation ledger, so an outage breaks the gate before the lookup.
async fn can_read_project_document(
    state: &AdminState,
    session: Option<&SessionData>,
    project_id: Uuid,
) -> Result<bool, String> {
    let Some(session) = session else {
        return Ok(false);
    };
    store::access::can_see_project(&state.surreal, session.person_id, session.role, project_id)
        .await
}

/// Look up the document `assets` row by id and reject if it doesn't
/// belong to the project_id from the URL — this is the cross-project
/// leakage guard. A bare content asset (no `project_id`) is never a
/// project document, so it fails the guard too.
///
/// The client lens additionally never resolves an `internal` asset: the
/// filename list and ZIP export are already gated on `visibility`, and
/// this closes the by-UUID detail/download path so a client can't fetch
/// internal work product (`review_memo`, `unclassified` lawyer/email
/// uploads) on their own matter by guessing or replaying a `doc_id`
/// (#782). The lawyer lens is unfiltered — lawyers see every asset.
///
/// `Ok(None)` is a genuine 404 (absent, cross-project, or internal under
/// the client lens); `Err` is a database failure the caller must surface
/// as a 500 rather than mask as not-found.
async fn load_doc_for_project(
    state: &AdminState,
    project_id: Uuid,
    doc_id: Uuid,
    lens: store::access::ProjectLens,
) -> Result<Option<store::assets::Asset>, store::assets::AssetError> {
    let Some(doc) = store::assets::find_by_id(&state.surreal, doc_id).await? else {
        tracing::info!(%project_id, %doc_id, "project document row not found");
        return Ok(None);
    };
    if doc.project_id != Some(project_id) {
        tracing::warn!(
            %project_id,
            %doc_id,
            doc_project_id = ?doc.project_id,
            "project document requested under the wrong project (cross-project guard)"
        );
        return Ok(None);
    }
    if lens == store::access::ProjectLens::Client
        && doc.visibility != store::documents::visibility::CLIENT
    {
        tracing::warn!(
            %project_id,
            %doc_id,
            visibility = %doc.visibility,
            "internal project document requested through the client lens (visibility guard)"
        );
        return Ok(None);
    }
    Ok(Some(doc))
}

/// Stream bytes through the app — fallback when the storage backend
/// has no signed-URL concept (`FsStorage` in local dev). Sets a
/// `Content-Disposition: attachment` so the browser downloads with
/// the original filename rather than the content-addressed
/// `blobs/<sha>` key.
async fn stream_through(
    state: AdminState,
    key: &str,
    content_type: &str,
    filename: &str,
) -> Response {
    match state.storage.get(key).await {
        Ok(obj) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type)
            .header(
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            )
            .body(Body::from(obj.bytes))
            .map_or_else(
                |e| {
                    tracing::error!(error = %e, "build streaming response");
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                },
                IntoResponse::into_response,
            ),
        Err(cloud::StorageError::NotFound(_)) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(error = %e, key, "storage get failed for project document");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
