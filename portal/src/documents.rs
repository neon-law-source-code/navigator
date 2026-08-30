//! `/app/lawyer/notations/:id/documents/:doc_id` and
//! `/app/notations/:id/documents/:doc_id` — issue a 302 to a
//! short-lived signed URL on the storage backend so the browser
//! downloads the blob directly, or stream the bytes through the app
//! when the backend doesn't support signed URLs (e.g.
//! [`cloud::FsStorage`] in local dev).
//!
//! Authorization is layered:
//!
//! 1. The route lives under the authenticated app router, which is
//!    already gated by the embedded Rego policy middleware (every request must pass
//!    `(path, method, session)` through `policy::PolicyClient`).
//! 2. On top of embedded Rego policy, this handler enforces the project ACL
//!    from the caller's tier and their participation row, never from the URL.
//! 3. A non-participant, an unknown notation, and an unknown `doc_id`
//!    slug all return 404 — no leakage about which exists.

use std::time::Duration;
use uuid::Uuid;

use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Extension;

use crate::admin::AdminState;
use crate::session::SessionData;
use store::notations::{
    certificate_of_completion_storage_key, document_pdf_storage_key, signed_document_storage_key,
};

/// Signed-URL validity window. 5 minutes is long enough for the
/// browser to follow the redirect and start the download, short
/// enough that a logged copy in someone else's history can't be
/// replayed an hour later.
const SIGNED_URL_TTL: Duration = Duration::from_mins(5);

/// Slug for the rendered (unsigned) retainer PDF.
const DOC_ID_RETAINER: &str = "retainer";
/// Template-neutral alias for the rendered document PDF — the same
/// per-notation key as [`DOC_ID_RETAINER`]. `retainer` reads wrong for a
/// formation packet (an LLC's filled Articles, a trust certificate), so
/// the `navigator notation document` CLI command fetches `document`.
const DOC_ID_DOCUMENT: &str = "document";
/// Slug for the executed (signed) document the webhook archives.
const DOC_ID_SIGNED: &str = "signed";
/// Slug for the e-signature Certificate of Completion.
const DOC_ID_CERTIFICATE: &str = "certificate";

/// `GET /app/lawyer/notations/:id/documents/:doc_id` or
/// `GET /app/notations/:id/documents/:doc_id`.
pub async fn download(
    State(state): State<AdminState>,
    Extension(session): Extension<SessionData>,
    AxumPath((notation_id, doc_id)): AxumPath<(Uuid, String)>,
) -> Response {
    let Ok(Some(notation_row)) = store::notations::find_by_id(&state.surreal, notation_id).await
    else {
        return StatusCode::NOT_FOUND.into_response();
    };

    // Project ACL follows the caller's tier, not the URL. These notation paths
    // are outside the `/app/projects` collapse, so they keep the documented
    // Owner/Admin project-scoping bypass: `can_see_project_as_lawyer` retains
    // it, while `can_see_project` (the matter surface's gate) does not.
    let decision = if session.role.is_lawyer_tier() {
        store::access::can_see_project_as_lawyer(
            &state.surreal,
            session.person_id,
            session.role,
            notation_row.project_id,
        )
        .await
    } else {
        store::access::can_see_project_as_client(
            &state.surreal,
            session.person_id,
            notation_row.project_id,
        )
        .await
    };
    match decision {
        Ok(true) => {}
        Ok(false) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(error = %e, %notation_id, "documents: participation check failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    let Some((storage_key, content_type)) = resolve_doc(notation_id, &doc_id) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    match state.storage.signed_url(&storage_key, SIGNED_URL_TTL).await {
        Ok(url) => Redirect::temporary(&url).into_response(),
        Err(cloud::StorageError::Unsupported(_)) => {
            stream_through(state, storage_key, content_type).await
        }
        Err(cloud::StorageError::NotFound(_)) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(error = %e, %notation_id, doc_id, "signed_url failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Map `doc_id` to the storage key + content-type. The slug -> key
/// mapping lives here (rather than on the route) so the URL stays
/// stable as the underlying storage layout shifts. The three slugs are
/// the three artifacts a matter accrues: the rendered retainer, the
/// executed (signed) document, and the Certificate of Completion — the
/// latter two archived by `esignature_webhook`.
fn resolve_doc(notation_id: Uuid, doc_id: &str) -> Option<(String, &'static str)> {
    match doc_id {
        DOC_ID_RETAINER | DOC_ID_DOCUMENT => {
            Some((document_pdf_storage_key(notation_id), "application/pdf"))
        }
        DOC_ID_SIGNED => Some((signed_document_storage_key(notation_id), "application/pdf")),
        DOC_ID_CERTIFICATE => Some((
            certificate_of_completion_storage_key(notation_id),
            "application/pdf",
        )),
        _ => None,
    }
}

/// Fetch the blob through the app and stream it to the client.
/// Used when the storage backend has no signed-URL concept
/// (`FsStorage` in local dev); production GCS always 302s.
async fn stream_through(state: AdminState, key: String, content_type: &str) -> Response {
    match state.storage.get(&key).await {
        Ok(obj) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type)
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
            tracing::error!(error = %e, key, "storage get failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    // The route-level participation ACL (`can_see_project`) is covered by
    // `store::access` unit tests and the `matter_documents` integration
    // test; here we pin the slug → storage-key mapping.
    use super::{resolve_doc, DOC_ID_CERTIFICATE, DOC_ID_DOCUMENT, DOC_ID_RETAINER, DOC_ID_SIGNED};
    use uuid::Uuid;

    const ID42: Uuid = Uuid::from_u128(42);

    #[test]
    fn resolve_doc_returns_pdf_key_for_retainer_slug() {
        let (key, ct) = resolve_doc(ID42, DOC_ID_RETAINER).expect("retainer resolves");
        assert_eq!(key, format!("notations/{ID42}/document.pdf"));
        assert_eq!(ct, "application/pdf");
    }

    #[test]
    fn resolve_doc_document_alias_maps_to_the_same_key_as_retainer() {
        // `document` is the template-neutral alias the formation CLI uses;
        // it must resolve to the identical per-notation PDF key.
        let (alias, _) = resolve_doc(ID42, DOC_ID_DOCUMENT).expect("document resolves");
        let (retainer, _) = resolve_doc(ID42, DOC_ID_RETAINER).expect("retainer resolves");
        assert_eq!(alias, retainer);
        assert_eq!(alias, format!("notations/{ID42}/document.pdf"));
    }

    #[test]
    fn resolve_doc_returns_signed_and_certificate_keys() {
        let (signed, _) = resolve_doc(ID42, DOC_ID_SIGNED).expect("signed resolves");
        assert_eq!(signed, format!("notations/{ID42}/signed-document.pdf"));
        let (cert, _) = resolve_doc(ID42, DOC_ID_CERTIFICATE).expect("certificate resolves");
        assert_eq!(
            cert,
            format!("notations/{ID42}/certificate-of-completion.pdf")
        );
    }

    #[test]
    fn resolve_doc_returns_none_for_unknown_slug() {
        assert!(resolve_doc(ID42, "bogus").is_none());
    }
}
