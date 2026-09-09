//! The door to the signing ceremony: `GET /app/notations/:id/sign` and
//! `GET /app/lawyer/notations/:id/sign`.
//!
//! After the retainer is sent for signature the client is a **captive**
//! DocuSign recipient (see [`crate::retainer_walk::client_user_id`]) —
//! DocuSign does not email them, so a short-lived [recipient view] URL is the
//! only way in. This route mints one and **redirects the browser to DocuSign**.
//! The ceremony happens on the provider's own site; Navigator does not frame
//! it.
//!
//! ## Two lenses, two gates
//!
//! One handler is registered at both paths, exactly as
//! [`crate::documents::download`] is. The lens is the session's role, not
//! the URL prefix, and the gate follows from it:
//!
//! **Participation says you may look at the matter; identity says you may
//! sign as this person.**
//!
//! 1. *Participation*, following [`crate::documents`] verbatim:
//!    [`store::access::can_see_project_as_lawyer`] for a firm-tier session
//!    (keeping the documented Owner/Admin project-scoping bypass),
//!    [`store::access::can_see_project_as_client`] otherwise.
//! 2. *Identity*, for a non-firm caller only: the session's person must be
//!    the notation's bound signer. A matter can carry several client
//!    participants, so participation alone would let one of them open the
//!    other's ceremony. The firm lens is deliberately exempt — a lawyer
//!    minting the view for the client in the room is the in-office signing
//!    this route was built for, and there the caller is never the signer.
//!
//! Both refusals answer `404`, never `403`, for the reason spelled out at
//! [`crate::documents`]: a `403` on the identity check would confirm that
//! this notation exists and that the caller is on the matter.
//!
//! ## Completion does not come back through this browser session
//!
//! Because the signer leaves, they may never return here — they close the tab,
//! or finish on their phone. Nothing legal may depend on the round trip, and
//! nothing does: [`crate::esignature_webhook`] is the authoritative return
//! path. DocuSign Connect posts the completion, its HMAC is verified over the
//! raw body, and that signal advances the retainer workflow to `END` and
//! archives the executed PDF plus the Certificate of Completion. The
//! `return_url` below is a courtesy for the signer who does come back, not a
//! mechanism.
//!
//! The signing URL is single-use and expires in minutes, so it is minted fresh
//! on each request and never cached. The handler is generic over the
//! [`crate::signature::SignatureProvider`] seam, so the stub returns a
//! deterministic fake URL in dev / KIND.
//!
//! [recipient view]: https://developers.docusign.com/docs/esign-rest-api/reference/envelopes/envelopeviews/createrecipient/

use axum::extract::{Path as AxumPath, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use uuid::Uuid;

use crate::admin::AdminState;
use crate::session::SessionData;
use crate::signature::RecipientView;

/// `GET /app/notations/:id/sign` and `GET /app/lawyer/notations/:id/sign` —
/// mint a single-use recipient-view URL for the notation's captive client
/// and send the browser to DocuSign.
///
/// One handler serves both registrations; see the module docs for the two
/// gates it applies and why the identity gate is client-lens only.
pub async fn sign_get(
    State(state): State<AdminState>,
    Extension(session): Extension<SessionData>,
    headers: HeaderMap,
    AxumPath(notation_id): AxumPath<Uuid>,
) -> Response {
    let Some(notation_row) = store::notations::find_by_id(&state.surreal, notation_id)
        .await
        .ok()
        .flatten()
    else {
        return (StatusCode::NOT_FOUND, "notation not found").into_response();
    };

    let firm_lens = session.role.is_lawyer_tier();

    // (1) Project ACL follows the caller's tier, not the URL — the same fork
    // `crate::documents::download` applies, so the two notation surfaces read
    // one membership table the same way.
    let decision = if firm_lens {
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
        Ok(false) => return (StatusCode::NOT_FOUND, "notation not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, %notation_id, "esign_view: participation check failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    // (2) Identity, client lens only: a matter can carry several client
    // participants, and participation alone would let one open another's
    // ceremony. Keyed on `person_id` rather than on the resolved name or
    // email, because the name may come from a questionnaire answer while the
    // bound signer never moves.
    if !firm_lens && session.person_id != Some(notation_row.person_id) {
        return (StatusCode::NOT_FOUND, "notation not found").into_response();
    }

    // The envelope must already exist (the retainer walk records the id in
    // `signatures` when it parks at `sent_for_signature__pending`). No id →
    // there is nothing to sign yet.
    let Some(request_id) = store::signatures::request_id_for_notation(&state.surreal, notation_id)
        .await
        .ok()
        .flatten()
    else {
        return (
            StatusCode::CONFLICT,
            "this matter has not been sent for signature yet",
        )
            .into_response();
    };

    // The captive recipient is resolved on the email/name/clientUserId
    // triple, so it must match the envelope exactly. Resolved through the
    // *same* function the send path used, so the two cannot drift.
    let recipient =
        match crate::retainer_walk::client_recipient_for_notation(&state.surreal, &notation_row)
            .await
        {
            Ok(recipient) => recipient,
            Err(e) => {
                tracing::error!(error = %e, %notation_id, "esign_view: recipient resolve failed");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };
    if recipient.name.is_empty() || recipient.email.is_empty() {
        return (StatusCode::NOT_FOUND, "client not found").into_response();
    }
    // A recipient view is only valid for a recipient sent with a
    // `clientUserId`. An `emailed` notation has none: DocuSign mailed that
    // signer their own link and there is no embedded session to mint.
    let Some(client_user_id) = recipient.client_user_id else {
        return (
            StatusCode::CONFLICT,
            "this matter's envelope was emailed to the signer; there is no session to open here",
        )
            .into_response();
    };

    let view = RecipientView {
        return_url: return_url_for(&state, &headers, &notation_row, firm_lens).await,
        email: recipient.email,
        name: recipient.name,
        client_user_id,
    };

    match state
        .signature_provider
        .create_recipient_view(&crate::signature::SignatureRequestId(request_id), &view)
        .await
    {
        Ok(signing_url) => redirect_to_provider(&signing_url).unwrap_or_else(|| {
            tracing::error!(
                %notation_id,
                "esign_view: provider returned a signing URL that is not an absolute https URL"
            );
            (
                StatusCode::BAD_GATEWAY,
                "could not start the signing session; please retry",
            )
                .into_response()
        }),
        Err(e) => {
            tracing::error!(error = %e, %notation_id, "esign_view: recipient view failed");
            (
                StatusCode::BAD_GATEWAY,
                "could not start the signing session; please retry",
            )
                .into_response()
        }
    }
}

/// Where DocuSign sends the browser once the ceremony ends. A courtesy for the
/// signer who comes back, not a mechanism: the executed state arrives on
/// [`crate::esignature_webhook`] whether this redirect is ever followed or not.
///
/// **Absolute, and lens-aware.** Absolute because the provider resolves a
/// relative path against *its own* origin rather than Navigator's, so a
/// relative `return_url` strands the signer on DocuSign;
/// [`crate::openapi::base_url_for`] is the tree's one resolver for the public
/// authority (brand `base_url` → `NAV_BASE_URL` → request `Host`). Lens-aware
/// because the firm's step page lives under `/app/lawyer`, which the policy
/// refuses a client — handing a client an absolute URL into a `403` would be a
/// regression dressed as a fix. A client goes to their matter page instead,
/// and to the matter list when the Project cannot be read.
async fn return_url_for(
    state: &AdminState,
    headers: &HeaderMap,
    notation_row: &store::notations::Notation,
    firm_lens: bool,
) -> String {
    let base = crate::openapi::base_url_for(
        headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok()),
    );
    let base = base.trim_end_matches('/');
    if firm_lens {
        let notation_id = notation_row.id;
        return format!("{base}/app/lawyer/notations/{notation_id}/step");
    }
    match store::projects::find_by_id(&state.surreal, notation_row.project_id).await {
        Ok(Some(project)) => format!("{base}/app/projects/{}", project.code),
        _ => format!("{base}/app/projects"),
    }
}

/// Build the `303` that hands the signer to the provider, or `None` when the
/// URL is not one we are willing to send a browser to.
///
/// The destination comes from a third party over the network, so it is treated
/// as untrusted. Only an absolute `https` URL with a host is accepted, which
/// rules out an open redirect (`//evil.example`), a relative path back into
/// Navigator, a scheme that executes in the page (`javascript:`, `data:`), and
/// a downgrade to plaintext for a URL carrying a single-use signing
/// credential.
///
/// Header injection is handled by *parsing*, not by the accept/reject test: the
/// WHATWG parser strips CR, LF, and tab outright and percent-encodes the rest,
/// so what is emitted is the parser's normalized serialization rather than the
/// provider's bytes. Note the consequence — a hostile URL is not refused here,
/// it is defanged, and the redirect still happens to the sanitized target.
///
/// The header is nonetheless built fallibly rather than through
/// `axum::response::Redirect::to`, which panics on a value `HeaderValue`
/// rejects. Normalization should make that unreachable; a `502` is the right
/// answer if it ever is not, and a panic never is.
fn redirect_to_provider(signing_url: &str) -> Option<Response> {
    let parsed = url::Url::parse(signing_url).ok()?;
    if parsed.scheme() != "https" || !parsed.has_host() {
        return None;
    }
    let location = HeaderValue::from_str(parsed.as_str()).ok()?;
    Some((StatusCode::SEE_OTHER, [(header::LOCATION, location)]).into_response())
}

#[cfg(test)]
mod tests {
    use super::redirect_to_provider;
    use axum::http::{header, StatusCode};

    #[test]
    fn a_provider_url_becomes_a_redirect_to_docusign() {
        let url = "https://demo.docusign.net/signing/abc123";
        let response = redirect_to_provider(url).expect("an https provider URL is redirectable");
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            url,
            "the signer is sent to the provider's own page, unmodified"
        );
    }

    #[test]
    fn a_url_that_is_not_absolute_https_is_refused() {
        // The destination is a third party's network response, so every one of
        // these would be a way to point a signer somewhere we did not intend.
        for hostile in [
            "http://demo.docusign.net/signing/abc", // plaintext downgrade
            "//evil.example/signing",               // protocol-relative
            "/app/lawyer/notations",                // relative, back into Navigator
            "javascript:alert(1)",                  // executes in the page
            "data:text/html,<script>alert(1)</script>", // ditto
            "not a url at all",
            "",
        ] {
            assert!(
                redirect_to_provider(hostile).is_none(),
                "must refuse to redirect to {hostile:?}"
            );
        }
    }

    #[test]
    fn crlf_in_a_provider_url_cannot_forge_a_second_header() {
        // The parser strips CR/LF rather than rejecting the URL, so this one
        // *does* redirect — to a sanitized target. What must never happen is
        // the raw bytes reaching the header and splitting it in two.
        let response = redirect_to_provider("https://x/a\r\nX-Injected: 1")
            .expect("the parser normalizes rather than refusing");
        let location = response.headers().get(header::LOCATION).unwrap();
        assert!(
            !location.as_bytes().contains(&b'\r') && !location.as_bytes().contains(&b'\n'),
            "no bare CR/LF may survive into the Location header: {location:?}"
        );
        assert!(
            response.headers().get("x-injected").is_none(),
            "the crafted header name must not have become a real header"
        );
    }
}
