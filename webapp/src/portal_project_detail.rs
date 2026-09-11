//! The client portal matter-detail page (`/app/projects/{code}`) as a Dioxus
//! component (#641 Phase 3, projects cluster) — the single-matter client view.
//!
//! The successor to the `portal::projects::detail` render. Every caller
//! sees the matter through the client lens: a lawyer/admin user who also holds
//! client-side matters gets the same view a client does, and a caller without
//! client-side scope gets `404` (never `403` — the matter does not exist from
//! their perspective). The page gathers, server-side of the render:
//!
//! - the matter name and status;
//! - the invoice from the local Xero mirror (never Xero live);
//! - the matter's notations (retainer, etc.) with a download link per PDF that
//!   exists in the object store (`store::notations` keys, probed through the
//!   injected storage handle) — except the signed copy, which additionally
//!   requires a completed `store::signatures` record, so an object at that
//!   key never reads as executed on its own;
//! - the client-readable review drafts;
//! - the matter's documents.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::people::ViewerRole;
use crate::portal_project_list::PersonId;

/// The pending client-intake notation resolved by the portal request layer.
/// The resolver lives in `portal`, which owns the workflow dependency; this
/// wasm-safe extension carries only the link target into the Dioxus server fn.
#[derive(Clone, Default)]
pub struct PendingClientIntake(pub Option<String>);

/// One of the matter's invoices, read from the local Xero mirror. The Xero
/// invoice id is deliberately not carried — only client-facing fields.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct InvoiceView {
    /// The invoice-level reference (`Matter <project_id>`).
    pub reference: String,
    /// Formatted total, e.g. `$3,333.00`.
    pub amount: String,
    /// Provider status mirror (`AUTHORISED`, `PAID`, …).
    pub status: String,
    /// `true` once reconcile has seen the invoice paid in full.
    pub paid: bool,
    /// The date Xero raised the invoice, `YYYY-MM-DD`.
    pub issued_on: String,
}

/// One of the matter's notations (e.g. the retainer), in plain words, with the
/// download links keyed off which of its three PDFs exist in storage.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // One independent readiness flag per document/action.
pub struct NotationRow {
    pub id: String,
    pub title: String,
    /// Client-friendly status, e.g. "Signed" / "Awaiting your signature".
    pub status: String,
    pub rendered_ready: bool,
    /// Whether a completed signature record — provider id and `signed_at`
    /// — backs this notation's document. Deliberately not "does an object
    /// exist at the signed-document storage key": that would let any
    /// upload landing at that key read as executed. See
    /// [`notation_status_label`].
    pub signed_ready: bool,
    pub certificate_ready: bool,
    /// True only for a live envelope (signature
    /// [`store::signatures::SignatureState::Requested`]) whose bound signer
    /// (`notation.person_id`) is the current session's person — never for a
    /// declined, voided, expired, or already-completed envelope, and never
    /// for a notation addressed to a different participant on the same
    /// matter.
    ///
    /// Also false for `emailed` delivery, whose recipient carries no
    /// `clientUserId`: the sign route has no embedded session to mint for
    /// one and answers `409`, so the action would lead nowhere.
    pub signable: bool,
}

/// One attorney-advanced draft the client may read and comment on.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ReviewDocRow {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub status: String,
}

/// The rendered matter-detail view — every field wasm-safe (plain scalars; no
/// `store`/`SeaORM`/`cloud` type crosses to the client build).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ProjectDetailView {
    pub id: String,
    /// The Project code, which keys both the matter page and client portal.
    pub code: String,
    pub name: String,
    pub status: String,
    /// Every invoice mirrored for this matter, newest first — a matter may
    /// carry more than one over time (ENG-588). Empty until Xero raises one.
    pub invoices: Vec<InvoiceView>,
    pub notations: Vec<NotationRow>,
    pub documents: Vec<String>,
    pub review_docs: Vec<ReviewDocRow>,
    /// The matter's collaboration resources, filtered to a client's audience:
    /// the shared Slack channel, the shared Notion page, and the portal. The
    /// firm's three private resources are never built for this view, so no
    /// firm-only URL reaches a client's markup or hydration payload.
    #[serde(default)]
    pub resources: crate::project_resources::ProjectResourcesView,
    pub csrf_token: String,
    pub role: ViewerRole,
    /// The original firm actor when this client lens is a matter preview. The
    /// page renders its exit banner from this server-injected state rather than
    /// inferring anything from the effective client session.
    #[serde(default)]
    pub viewing_as_dri: Option<crate::components::ClientDriView>,
    /// The deploy's brand mark for the navbar. `None` when the mounted brand
    /// configures none.
    #[serde(default)]
    pub logo: Option<crate::components::AppLogo>,
    /// The resolved brand's tokens stylesheet href, so the page wears
    /// its own palette rather than the firm's on a non-default host.
    #[serde(default)]
    pub tokens_href: String,
    /// The one client-facing intake continuation for this matter, when the
    /// current client-facing workflow still needs an answer.
    #[serde(default)]
    pub pending_intake: Option<String>,
}

/// Client-friendly status for a notation, derived from its workflow state,
/// its signature evidence, and which PDFs have materialized — never the raw
/// docket state. Mirrors the `notation_status_label`.
///
/// `signed_ready` here is signature evidence
/// ([`store::signatures::completed_for_notation`]), not object presence — a
/// declined or voided envelope, or one still outstanding, both leave it
/// `false`, so neither renders "Signed". `signature_state` is what tells
/// those two apart: a live envelope reads
/// [`store::signatures::SignatureState::Requested`], while a dead one reads
/// `Declined`, `Voided`, or `Expired` — none of those may read as "Ready for
/// signature" or "Awaiting your signature", both of which offer a signing
/// action that no longer exists (ENG-558/ENG-560).
///
/// `awaiting_countersignature` is always `false` from every caller today:
/// the `signature` row is envelope-scoped (one `state` for every recipient
/// on the sequential two-party envelope), and `DocuSign`'s per-recipient
/// completion event is presently ignored by `esignature_webhook`, so
/// nothing distinguishes a client who has not started signing from one who
/// has already signed and is waiting on the firm's countersignature — both
/// read as `Requested`. The parameter exists so this function's copy is
/// correct and covered now; wiring a caller that can pass `true` needs
/// per-signer completion capture, a later lane.
#[cfg(feature = "server")]
fn notation_status_label(
    state: &str,
    signature_state: Option<store::signatures::SignatureState>,
    signed_ready: bool,
    rendered_ready: bool,
    awaiting_countersignature: bool,
) -> &'static str {
    use store::signatures::SignatureState;
    if signed_ready {
        "Signed"
    } else if matches!(
        signature_state,
        Some(SignatureState::Declined | SignatureState::Voided | SignatureState::Expired)
    ) {
        "Signing was declined. Contact the firm to continue."
    } else if awaiting_countersignature {
        "You have signed. Awaiting the firm's countersignature."
    } else if state.starts_with("sent_for_signature") {
        "Awaiting your signature"
    } else if rendered_ready {
        "Ready for signature"
    } else {
        "In preparation"
    }
}

/// Format integer cents as a US dollar amount with thousands separators, e.g.
/// `333_300` → `"$3,333.00"`. Mirrors the `format_usd`; money never flows
/// through a float.
#[cfg(feature = "server")]
fn format_usd(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let abs = cents.unsigned_abs();
    let digits = (abs / 100).to_string();
    let mut grouped = String::new();
    for (i, ch) in digits.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    let grouped: String = grouped.chars().rev().collect();
    format!("${sign}{grouped}.{:02}", abs % 100)
}

/// Fetch one matter's client-lens detail for the current request. Refuses a
/// caller without client-side scope with a `404` (the matter does not exist for
/// them), then gathers the invoice, notations (with per-PDF storage probes),
/// review drafts, and documents.
#[server]
#[cfg_attr(feature = "server", allow(clippy::too_many_lines))]
pub async fn get_project_detail() -> Result<ProjectDetailView, ServerFnError> {
    use std::sync::Arc;

    let axum::extract::Path(code) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Path<String>, _>()
            .await?;
    let PersonId(person_id) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<PersonId>, _>()
            .await
            .map(|axum::Extension(id)| id)
            .unwrap_or_default();
    let role = dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<ViewerRole>, _>()
        .await
        .map(|axum::Extension(role)| role)
        .unwrap_or_default();
    // The navbar renders on the 404 body too, so the mark is resolved before the
    // first early return rather than only on the happy path.
    let logo = crate::app_chrome::app_logo_from_context().await;
    let tokens_href = crate::app_chrome::app_tokens_href_from_context().await;
    let csrf_token = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::csrf::CsrfToken>,
        _,
    >()
    .await
    .map(|axum::Extension(token)| token.0)
    .unwrap_or_default();
    let crate::components::ViewingAsDri(viewing_as_dri) =
        dioxus_fullstack_core::FullstackContext::extract::<
            axum::Extension<crate::components::ViewingAsDri>,
            _,
        >()
        .await
        .map(|axum::Extension(viewing_as_dri)| viewing_as_dri)
        .unwrap_or_default();
    let pending_intake = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<PendingClientIntake>,
        _,
    >()
    .await
    .map(|axum::Extension(intake)| intake.0)
    .unwrap_or_default();
    let person_id = person_id.and_then(|raw| raw.parse::<uuid::Uuid>().ok());

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let storage = consume_context::<Arc<dyn cloud::StorageService>>();
    let Some(project) = store::projects::find_by_code(&surreal, &code)
        .await
        .map_err(server_error)?
    else {
        return Ok(not_found(uuid::Uuid::nil(), role, logo, csrf_token));
    };
    let id = project.id;

    // Row-visibility runs before the row load, so an unauthorised caller never
    // even pulls the matter name into the response — the same 404 a missing id
    // would produce (never 403). A read error is a 500.
    let visible = store::projects::can_access_as_client_in_surreal(&surreal, person_id, id)
        .await
        .map_err(server_error)?;
    if !visible {
        return Ok(not_found(id, role, logo, csrf_token));
    }
    // Queue only for a real client session. A firm member's read-only
    // client-DRI view renders the same page but must not look like client
    // activity in the firm's channel. The one-way Restate call is best-effort
    // for the page: a Slack outage must not turn an authorized portal read into
    // a failed client request.
    if role == ViewerRole::Client && viewing_as_dri.is_none() {
        tokio::spawn(queue_client_project_view(id));
    }
    // Notations, each with which of its three PDFs exist in storage.
    let notation_rows = notation_rows(&surreal, storage.as_ref(), id, person_id).await?;

    // Client-readable drafts (only those an attorney has advanced past `draft`).
    let review_docs = store::review_documents::client_visible_for_project(&surreal, id)
        .await
        .map_err(server_error)?;
    let review_rows = review_docs
        .iter()
        .map(|d| ReviewDocRow {
            id: d.id.to_string(),
            title: d.title.clone(),
            kind: d.kind.clone(),
            status: d.status.clone(),
        })
        .collect();

    // Documents (read-only list of filenames) — gated to the assets a
    // lawyer has explicitly marked client-visible. Internal work product
    // (`review_memo`, `unclassified` lawyer/email uploads) never reaches
    // this list (#782).
    let documents = store::assets::for_project(&surreal, id)
        .await
        .map_err(server_error)?
        .into_iter()
        .filter(|d| d.visibility == store::documents::visibility::CLIENT)
        .map(|d| d.filename.unwrap_or_default())
        .collect();

    // Every invoice from the local mirror, newest first; only client-facing
    // fields reach the client (never the Xero invoice id).
    let invoices = store::xero_invoices::for_projects(&surreal, &[id])
        .await
        .map_err(server_error)?
        .into_iter()
        .map(|r| InvoiceView {
            reference: r.reference,
            amount: format_usd(r.amount_cents),
            paid: r.amount_cents > 0 && r.amount_paid_cents >= r.amount_cents,
            status: r.status,
            issued_on: r.issued_at.format("%Y-%m-%d").to_string(),
        })
        .collect();

    let resources = crate::project_resources::ProjectResourcesView {
        resources: crate::project_resources::visible_resources(
            &crate::project_resources::ProjectResourceLinks {
                private_slack_channel_url: project.internal_slack_channel_url.clone(),
                private_notion_page_url: project.private_notion_page_url.clone(),
                drive_folder_id: project.drive_folder_id.clone(),
                shared_slack_channel_url: project.external_slack_channel_url.clone(),
                shared_notion_page_url: project.shared_notion_page_url.clone(),
            },
            &project.code,
            role,
        ),
        // A client never configures a resource; the affordance is a lawyer's.
        can_configure: false,
        project_code: project.code.clone(),
    };
    Ok(ProjectDetailView {
        id: project.id.to_string(),
        code: project.code,
        name: project.name,
        status: project.status,
        invoices,
        notations: notation_rows,
        documents,
        review_docs: review_rows,
        resources,
        csrf_token,
        role,
        viewing_as_dri,
        logo,
        tokens_href,
        pending_intake,
    })
}

/// Start the per-Project Slack virtual object after a client has passed the
/// same server-side visibility check used to render this page. Local in-memory
/// development has no Restate broker, so it intentionally does not enqueue.
#[cfg(feature = "server")]
async fn queue_client_project_view(project_id: uuid::Uuid) {
    let Some(broker) = std::env::var("RESTATE_BROKER_URL")
        .ok()
        .map(|url| url.trim_end_matches('/').to_string())
        .filter(|url| !url.trim().is_empty())
    else {
        return;
    };
    let token = std::env::var("RESTATE_AUTH_TOKEN").ok();
    if let Err(error) = workflows::start_workflow(
        &broker,
        token.as_deref(),
        "project-slack",
        &project_id.to_string(),
        "client_project_view",
        &serde_json::json!({}),
        true,
    )
    .await
    {
        tracing::error!(%project_id, %error, "client Project view Slack notice was not queued");
    }
}

/// Build the per-notation rows for a matter: title, a client-friendly status,
/// and which of the three PDFs exist. `exists` is a metadata-only HEAD, so a
/// handful of probes per matter is cheap. `person_id` is the current
/// session's person, used only to decide [`NotationRow::signable`] — never
/// to filter which notations appear, since every participant's notations
/// still show on the shared matter page. Mirrors the `notation_rows`.
///
/// `signed_ready` is the one field this loop does not answer from storage:
/// an object at the signed-document key only proves bytes were written
/// there, never that a provider confirmed execution — a lawyer-uploaded PDF
/// that merely looks signed would satisfy the same probe. It reads
/// [`store::signatures::completed_for_notation`] instead, so the client
/// only sees "Signed" when a completed signature record — provider id and
/// `signed_at` — backs the document.
#[cfg(feature = "server")]
async fn notation_rows(
    surreal: &store::surreal::SurrealDb,
    storage: &dyn cloud::StorageService,
    project_id: uuid::Uuid,
    person_id: Option<uuid::Uuid>,
) -> Result<Vec<NotationRow>, ServerFnError> {
    let notations = store::notations::list_by_project(surreal, project_id)
        .await
        .map_err(server_error)?;
    let mut rows = Vec::with_capacity(notations.len());
    for n in &notations {
        let title = store::templates::find_by_id(surreal, n.template_id)
            .await
            .ok()
            .flatten()
            .map_or_else(|| "Agreement".to_string(), |t| t.title);
        let rendered_ready = storage
            .exists(&store::notations::document_pdf_storage_key(n.id))
            .await
            .unwrap_or(false);
        let signed_ready = store::signatures::completed_for_notation(surreal, n.id)
            .await
            .ok()
            .flatten()
            .is_some();
        let signature_state = store::signatures::latest_for_notation(surreal, n.id)
            .await
            .ok()
            .flatten()
            .map(|s| s.state);
        let certificate_ready = storage
            .exists(&store::notations::certificate_of_completion_storage_key(
                n.id,
            ))
            .await
            .unwrap_or(false);
        // Live envelope, addressed to this exact signer, that the sign
        // route can actually open — a matter can carry several client
        // participants, so a live envelope bound to one of them must never
        // offer the sign action to another (mirrors the identity gate
        // `esign_view::sign_get` applies before minting a recipient view).
        //
        // `emailed` delivery is excluded for the same reason: a
        // non-captive recipient carries no `clientUserId`, so there is no
        // embedded session to mint and that route answers `409` rather
        // than a redirect (`server/tests/esign_redirect.rs`,
        // `an_emailed_envelope_has_no_embedded_session_to_open`). Offering
        // an action whose only outcome is a conflict is the same defect as
        // offering one for a dead envelope.
        let signable = person_id == Some(n.person_id)
            && n.delivery != store::notations::DELIVERY_EMAILED
            && matches!(
                signature_state,
                Some(store::signatures::SignatureState::Requested)
            );
        rows.push(NotationRow {
            id: n.id.to_string(),
            title,
            status: notation_status_label(
                &n.state,
                signature_state,
                signed_ready,
                rendered_ready,
                false,
            )
            .to_string(),
            rendered_ready,
            signed_ready,
            certificate_ready,
            signable,
        });
    }
    Ok(rows)
}

/// Commit a `500` and wrap a query error, mirroring the sibling list pages: a
/// matter whose detail cannot be loaded is a server error, not a `200` with an
/// error body.
#[cfg(feature = "server")]
fn server_error(e: impl std::fmt::Display) -> ServerFnError {
    dioxus_fullstack_core::FullstackContext::commit_http_status(
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        None,
    );
    ServerFnError::new(e.to_string())
}

/// Commit the `404` the handler returned for a matter the caller cannot see
/// (or that does not exist) and return an empty view — the render shows the
/// not-found state under the committed status.
#[cfg(feature = "server")]
fn not_found(
    id: uuid::Uuid,
    role: ViewerRole,
    logo: Option<crate::components::AppLogo>,
    csrf_token: String,
) -> ProjectDetailView {
    dioxus_fullstack_core::FullstackContext::commit_http_status(
        axum::http::StatusCode::NOT_FOUND,
        None,
    );
    ProjectDetailView {
        id: id.to_string(),
        role,
        logo,
        csrf_token,
        ..ProjectDetailView::default()
    }
}

/// The client matter-detail page. Server-side rendered with the matter already
/// in the markup (via [`use_server_future`]), readable before hydration.
#[component]
pub fn ClientProjectDetail() -> Element {
    let resource = use_server_future(get_project_detail)?;

    let view = match &*resource.read() {
        Some(Ok(view)) if !view.name.is_empty() => view.clone(),
        // A committed 404 returns an empty (nameless) view; render the same
        // "not found" state the handler served under that status.
        Some(Ok(_)) => {
            return rsx! {
                main { id: "portal-project", p { "That matter was not found." } }
            }
        }
        Some(Err(_)) => {
            return rsx! {
                main { id: "portal-project", p { "Failed to load this matter." } }
            }
        }
        None => {
            return rsx! {
                main { id: "portal-project", p { "Loading…" } }
            }
        }
    };

    let has_documents = !view.documents.is_empty();
    let has_review_docs = !view.review_docs.is_empty();

    rsx! {
        document::Title { "{view.name}" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        crate::components::ClientDriViewBanner { view: view.viewing_as_dri.clone() }
        crate::components::AppNavbar {
            destinations: crate::app_chrome::app_destinations(view.role),
            logo: view.logo.clone(),
        }
        main { id: "portal-project", class: "nav-theme portal-detail",
            nav { class: "portal-detail__back",
                a { class: "nav-link", href: "/app/projects", "← Your Projects" }
            }
            h1 { "{view.name}" }
            p { span { class: "status-chip", "{view.status}" } }

            crate::project_resources::ProjectResourcesPanel { view: view.resources.clone() }

            if has_documents {
                p { class: "portal-detail__actions",
                    a {
                        class: "nav-btn nav-btn--secondary",
                        href: "/app/projects/{view.code}/documents.zip",
                        role: "button",
                        "Download all my documents"
                    }
                }
            }

            if !view.invoices.is_empty() {
                section { class: "portal-detail__section",
                    h2 { "Invoice" }
                    for inv in view.invoices.iter() {
                        div { class: "portal-card portal-card--split",
                            div {
                                div { class: "portal-card__title", "{inv.amount}" }
                                div { class: "portal-card__meta", "Status: {inv.status}" }
                                div { class: "portal-card__meta", "Issued: {inv.issued_on}" }
                            }
                            if inv.paid {
                                span { class: "status-chip status-chip--paid", "Paid" }
                            } else {
                                span { class: "status-chip status-chip--due", "Due" }
                            }
                        }
                    }
                }
            }

            if let Some(notation_id) = view.pending_intake.as_ref() {
                p { class: "portal-detail__actions",
                    a {
                        class: "nav-btn nav-btn--primary",
                        href: "/app/projects/{view.code}/intake/{notation_id}",
                        role: "button",
                        "Continue intake"
                    }
                }
            }

            if !view.notations.is_empty() {
                section { class: "portal-detail__section",
                    h2 { "Your agreements" }
                    div { class: "portal-agreements",
                        for n in view.notations.iter() {
                            div { class: "portal-agreement", key: "{n.id}",
                                span {
                                    "{n.title}"
                                    span { class: "status-chip", " {n.status}" }
                                }
                                span { class: "portal-agreement__links",
                                    if n.signable {
                                        a {
                                            class: "nav-btn nav-btn--primary",
                                            href: "/app/notations/{n.id}/sign",
                                            role: "button",
                                            "Review and sign"
                                        }
                                    }
                                    a {
                                        class: "nav-btn nav-btn--secondary",
                                        href: "{crate::notation_outline::notation_outline_href(&view.code, &n.id)}",
                                        "Outline"
                                    }
                                    if n.rendered_ready {
                                        a {
                                            class: "nav-btn nav-btn--secondary",
                                            href: "/app/notations/{n.id}/documents/retainer",
                                            "Agreement"
                                        }
                                    }
                                    if n.signed_ready {
                                        a {
                                            class: "nav-btn nav-btn--secondary",
                                            href: "/app/notations/{n.id}/documents/signed",
                                            "Signed copy"
                                        }
                                    }
                                    if n.certificate_ready {
                                        a {
                                            class: "nav-btn nav-btn--secondary",
                                            href: "/app/notations/{n.id}/documents/certificate",
                                            "Certificate"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            section { class: "portal-detail__section",
                h2 { "Your documents" }
                div { class: "nav-table-wrap",
                    table { class: "nav-table",
                        thead {
                            tr { th { scope: "col", "Document" } }
                        }
                        tbody {
                            for filename in view.documents.iter() {
                                tr { td { "{filename}" } }
                            }
                        }
                    }
                }
            }

            if has_review_docs {
                section { class: "portal-detail__section",
                    h2 { "Documents to review" }
                    div { class: "nav-table-wrap",
                        table { class: "nav-table",
                            thead {
                                tr {
                                    th { scope: "col", "Document" }
                                    th { scope: "col", "Type" }
                                    th { scope: "col", "Status" }
                                    th { scope: "col", class: "nav-table__end", "Action" }
                                }
                            }
                            tbody {
                                for doc in view.review_docs.iter() {
                                    tr {
                                        td { "{doc.title}" }
                                        td { span { class: "status-chip", "{doc.kind}" } }
                                        td { span { class: "status-chip", "{doc.status}" } }
                                        td { class: "nav-table__end",
                                            a {
                                                class: "nav-btn nav-btn--secondary",
                                                href: "/app/projects/{view.code}/review/{doc.id}",
                                                "Review"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::{notation_rows, notation_status_label};
    use cloud::StorageService;
    use store::signatures::SignatureState;

    /// Signature evidence, not workflow state, decides the label — with
    /// `state` and `rendered_ready` held fixed, only flipping evidence flips
    /// the label (ENG-421, covering test 1 & 2: an unsigned notation never
    /// gets the signed label, a signed one always does).
    #[test]
    fn signature_evidence_alone_changes_the_label() {
        let unsigned = notation_status_label(
            "sent_for_signature__pending",
            Some(SignatureState::Requested),
            false,
            true,
            false,
        );
        let signed = notation_status_label(
            "sent_for_signature__pending",
            Some(SignatureState::Completed),
            true,
            true,
            false,
        );
        assert_ne!(unsigned, signed);
    }

    /// Once there is signature evidence, the label does not depend on the
    /// workflow state or whether a rendered draft exists — evidence is the
    /// one thing that can assert execution.
    #[test]
    fn signature_evidence_produces_the_same_label_regardless_of_workflow_state() {
        let via_pending = notation_status_label(
            "sent_for_signature__pending",
            Some(SignatureState::Completed),
            true,
            true,
            false,
        );
        let via_end =
            notation_status_label("END", Some(SignatureState::Completed), true, true, false);
        let via_no_render =
            notation_status_label("BEGIN", Some(SignatureState::Completed), true, false, false);
        assert_eq!(via_pending, via_end);
        assert_eq!(via_pending, via_no_render);
    }

    /// A declined, voided, or expired envelope (which the esignature
    /// webhook leaves at the terminal `END` state without ever stamping
    /// `signed_at`) reads correctly — not merely distinctly — as told to a
    /// client: it must say signing was declined and point them to the firm,
    /// never assert that a signing action still exists (ENG-558; ENG-421
    /// covering test 3 for the distinctness half).
    #[test]
    fn a_dead_envelope_reads_as_declined_with_no_signing_action_implied() {
        const DECLINED_LABEL: &str = "Signing was declined. Contact the firm to continue.";
        let outstanding = notation_status_label(
            "sent_for_signature__pending",
            Some(SignatureState::Requested),
            false,
            true,
            false,
        );
        let signed =
            notation_status_label("END", Some(SignatureState::Completed), true, true, false);
        for state in [
            SignatureState::Declined,
            SignatureState::Voided,
            SignatureState::Expired,
        ] {
            let declined = notation_status_label("END", Some(state), false, true, false);
            assert_eq!(
                declined, DECLINED_LABEL,
                "{state:?} must read the declined copy verbatim, not merely something distinct"
            );
            assert_ne!(outstanding, declined);
            assert_ne!(declined, signed);
        }
    }

    /// The countersignature-window copy is correct and covered directly,
    /// even though no caller can produce its precondition today — see the
    /// doc comment on `notation_status_label` for why `awaiting_countersignature`
    /// is always `false` from `notation_rows`.
    #[test]
    fn the_countersignature_window_copy_never_claims_the_client_still_owes_a_signature() {
        let label = notation_status_label(
            "sent_for_signature__pending",
            Some(SignatureState::Requested),
            false,
            true,
            true,
        );
        assert_eq!(
            label,
            "You have signed. Awaiting the firm's countersignature."
        );
    }

    /// The route-level proof: `notation_rows` must not read `signed_ready`
    /// off `storage.exists()`. An object at the signed-document key with no
    /// completed `store::signatures` row must not read as signed — the
    /// exact failure mode ENG-421 reports (a lawyer-uploaded PDF landing at
    /// that key would otherwise claim execution). Once the provider's
    /// completion is recorded, the same notation reads as signed with no
    /// change to what is in storage.
    #[tokio::test]
    async fn notation_rows_reads_signed_ready_from_signature_evidence_not_storage() {
        let surreal = store::surreal::test_support::mem().await;
        let notation_id = store::test_support::seed_notation(&surreal).await;
        let notation = store::notations::find_by_id(&surreal, notation_id)
            .await
            .expect("query notation")
            .expect("seeded notation exists");
        let storage = cloud::FsStorage::new(std::env::temp_dir().join(format!(
            "navigator-webapp-portal-project-detail-{notation_id}"
        )))
        .await
        .expect("create FsStorage temp root");

        // An object at the signed-document key alone — no signature ever
        // recorded — must not read as signed.
        storage
            .put(
                &store::notations::signed_document_storage_key(notation_id),
                b"looks-signed-but-isn't",
                "application/pdf",
            )
            .await
            .expect("write object at the signed key");
        let rows = notation_rows(&surreal, &storage, notation.project_id, None)
            .await
            .expect("build notation rows");
        let row = rows.iter().find(|r| r.id == notation_id.to_string());
        assert_eq!(
            row.map(|r| r.signed_ready),
            Some(false),
            "an object at the signed key with no signature record must not be signed_ready"
        );

        // Recording the provider's completed signature — no change to what
        // is in storage — is what flips it.
        store::signatures::record_request(
            &surreal,
            notation_id,
            store::signatures::SignatureProvider::DocuSign,
            "env-eng-421",
        )
        .await
        .expect("record signature request");
        store::signatures::stamp_signed(
            &surreal,
            store::signatures::SignatureProvider::DocuSign,
            "env-eng-421",
            "2026-06-30T00:00:00Z",
        )
        .await
        .expect("stamp signed_at");
        let rows = notation_rows(&surreal, &storage, notation.project_id, None)
            .await
            .expect("build notation rows");
        let row = rows.iter().find(|r| r.id == notation_id.to_string());
        assert_eq!(
            row.map(|r| r.signed_ready),
            Some(true),
            "a completed signature record must make the notation signed_ready"
        );
    }

    /// A fresh `FsStorage` temp root, mirroring the setup every other
    /// `notation_rows` test in this module shares.
    async fn temp_storage(label: &str, notation_id: uuid::Uuid) -> cloud::FsStorage {
        cloud::FsStorage::new(
            std::env::temp_dir().join(format!("navigator-webapp-portal-{label}-{notation_id}")),
        )
        .await
        .expect("create FsStorage temp root")
    }

    /// `signable` is the gate the "Review and sign" action renders behind
    /// (ENG-558): only a live envelope (`Requested`) whose bound signer is
    /// the current session may see it. Before any envelope is sent, and
    /// when a live envelope belongs to a different participant on the same
    /// matter, it must be `false`.
    #[tokio::test]
    async fn signable_is_true_only_for_a_live_envelope_addressed_to_its_signer() {
        let surreal = store::surreal::test_support::mem().await;
        let notation_id = store::test_support::seed_notation(&surreal).await;
        let notation = store::notations::find_by_id(&surreal, notation_id)
            .await
            .expect("query notation")
            .expect("seeded notation exists");
        let storage = temp_storage("signable-addressed", notation_id).await;

        // No envelope has been sent yet: nothing to sign.
        let rows = notation_rows(
            &surreal,
            &storage,
            notation.project_id,
            Some(notation.person_id),
        )
        .await
        .expect("build notation rows");
        assert_eq!(
            rows.iter()
                .find(|r| r.id == notation_id.to_string())
                .map(|r| r.signable),
            Some(false),
            "a notation with no envelope yet must not be signable"
        );

        store::signatures::record_request(
            &surreal,
            notation_id,
            store::signatures::SignatureProvider::DocuSign,
            "env-eng-558-live",
        )
        .await
        .expect("record signature request");

        // The bound signer, viewing a live envelope, may sign it.
        let rows = notation_rows(
            &surreal,
            &storage,
            notation.project_id,
            Some(notation.person_id),
        )
        .await
        .expect("build notation rows");
        assert_eq!(
            rows.iter()
                .find(|r| r.id == notation_id.to_string())
                .map(|r| r.signable),
            Some(true),
            "a live envelope addressed to the current session's person must be signable"
        );

        // A different participant on the same matter must never see the
        // action for someone else's envelope.
        let other = store::persons::find_or_create(
            &surreal,
            &store::persons::NewPerson::new("Aries", "aries@example.com"),
        )
        .await
        .expect("seed a second participant");
        let rows = notation_rows(&surreal, &storage, notation.project_id, Some(other.id))
            .await
            .expect("build notation rows");
        assert_eq!(
            rows.iter()
                .find(|r| r.id == notation_id.to_string())
                .map(|r| r.signable),
            Some(false),
            "a live envelope must never be signable to another participant"
        );

        // No session identity at all — the client-DRI / logged-out shape.
        let rows = notation_rows(&surreal, &storage, notation.project_id, None)
            .await
            .expect("build notation rows");
        assert_eq!(
            rows.iter()
                .find(|r| r.id == notation_id.to_string())
                .map(|r| r.signable),
            Some(false),
            "no session person means no one is signable"
        );
    }

    /// A dead envelope offers no signing action to anyone, even its bound
    /// signer (ENG-558 acceptance: declined and expired must both be
    /// `false`; voided follows the same terminal-state rule).
    #[tokio::test]
    async fn signable_is_false_for_a_declined_voided_or_expired_envelope() {
        for (label, provider_id) in [
            ("declined", "env-eng-558-declined"),
            ("voided", "env-eng-558-voided"),
            ("expired", "env-eng-558-expired"),
        ] {
            let surreal = store::surreal::test_support::mem().await;
            let notation_id = store::test_support::seed_notation(&surreal).await;
            let notation = store::notations::find_by_id(&surreal, notation_id)
                .await
                .expect("query notation")
                .expect("seeded notation exists");
            let storage = temp_storage(&format!("signable-{label}"), notation_id).await;

            store::signatures::record_request(
                &surreal,
                notation_id,
                store::signatures::SignatureProvider::DocuSign,
                provider_id,
            )
            .await
            .expect("record signature request");
            match label {
                "declined" => {
                    store::signatures::stamp_declined(
                        &surreal,
                        store::signatures::SignatureProvider::DocuSign,
                        provider_id,
                    )
                    .await
                }
                "voided" => {
                    store::signatures::stamp_voided(
                        &surreal,
                        store::signatures::SignatureProvider::DocuSign,
                        provider_id,
                    )
                    .await
                }
                _ => {
                    store::signatures::stamp_expired(
                        &surreal,
                        store::signatures::SignatureProvider::DocuSign,
                        provider_id,
                    )
                    .await
                }
            }
            .expect("stamp terminal state");

            let rows = notation_rows(
                &surreal,
                &storage,
                notation.project_id,
                Some(notation.person_id),
            )
            .await
            .expect("build notation rows");
            assert_eq!(
                rows.iter()
                    .find(|r| r.id == notation_id.to_string())
                    .map(|r| r.signable),
                Some(false),
                "a {label} envelope must not be signable"
            );
        }
    }

    /// An `emailed` (non-captive) recipient has no embedded session to
    /// open: `esign_view::sign_get` resolves the recipient through the send
    /// path's own function, finds no `clientUserId`, and answers `409` —
    /// pinned by `server/tests/esign_redirect.rs`,
    /// `an_emailed_envelope_has_no_embedded_session_to_open`. A live
    /// envelope addressed to its own signer must therefore still not be
    /// signable when the notation was delivered by email, or the portal
    /// offers an action whose only outcome is a conflict.
    #[tokio::test]
    async fn signable_is_false_for_an_emailed_delivery_even_with_a_live_envelope() {
        let surreal = store::surreal::test_support::mem().await;
        let seeded_id = store::test_support::seed_notation(&surreal).await;
        let seeded = store::notations::find_by_id(&surreal, seeded_id)
            .await
            .expect("query notation")
            .expect("seeded notation exists");
        let storage = temp_storage("signable-emailed", seeded_id).await;

        // Same template, signer, and matter as the seeded notation; only the
        // delivery mode differs, so delivery is the single variable.
        let emailed_id = store::notations::create(
            &surreal,
            &store::notations::NewNotation::new(
                seeded.template_id,
                seeded.person_id,
                seeded.project_id,
                "sent_for_signature__pending",
            )
            .with_delivery(store::notations::DELIVERY_EMAILED),
        )
        .await
        .expect("create an emailed-delivery notation")
        .id;
        // Both notations get their own live envelope, so delivery is the
        // only difference between the two rows compared below.
        for (notation_id, provider_id) in [
            (emailed_id, "env-eng-558-emailed"),
            (seeded_id, "env-eng-558-embedded"),
        ] {
            store::signatures::record_request(
                &surreal,
                notation_id,
                store::signatures::SignatureProvider::DocuSign,
                provider_id,
            )
            .await
            .expect("record signature request");
        }

        let rows = notation_rows(
            &surreal,
            &storage,
            seeded.project_id,
            Some(seeded.person_id),
        )
        .await
        .expect("build notation rows");
        assert_eq!(
            rows.iter()
                .find(|r| r.id == emailed_id.to_string())
                .map(|r| r.signable),
            Some(false),
            "an emailed recipient has no embedded session, so the action must not be offered"
        );
        // The embedded sibling — same matter, same signer, same live
        // envelope state — stays signable, so delivery alone decided it.
        assert_eq!(
            rows.iter()
                .find(|r| r.id == seeded_id.to_string())
                .map(|r| r.signable),
            Some(true),
            "embedded delivery with the same live envelope and signer must remain signable"
        );
    }

    /// Once fully executed, the client cannot re-enter the ceremony — a
    /// completed envelope is not signable even to its own signer (ENG-558
    /// acceptance: "already signed by this client").
    #[tokio::test]
    async fn signable_is_false_once_the_envelope_is_fully_signed() {
        let surreal = store::surreal::test_support::mem().await;
        let notation_id = store::test_support::seed_notation(&surreal).await;
        let notation = store::notations::find_by_id(&surreal, notation_id)
            .await
            .expect("query notation")
            .expect("seeded notation exists");
        let storage = temp_storage("signable-completed", notation_id).await;

        store::signatures::record_request(
            &surreal,
            notation_id,
            store::signatures::SignatureProvider::DocuSign,
            "env-eng-558-completed",
        )
        .await
        .expect("record signature request");
        store::signatures::stamp_signed(
            &surreal,
            store::signatures::SignatureProvider::DocuSign,
            "env-eng-558-completed",
            "2026-06-30T00:00:00Z",
        )
        .await
        .expect("stamp signed_at");

        let rows = notation_rows(
            &surreal,
            &storage,
            notation.project_id,
            Some(notation.person_id),
        )
        .await
        .expect("build notation rows");
        let row = rows.iter().find(|r| r.id == notation_id.to_string());
        assert_eq!(
            row.map(|r| r.signable),
            Some(false),
            "an already-signed notation must not be signable"
        );
        assert_eq!(row.map(|r| r.signed_ready), Some(true));
    }
}
