//! The door to the signing ceremony: `GET /app/lawyer/notations/:id/sign`.
//!
//! After the retainer is sent for signature the client is a **captive**
//! DocuSign recipient (see [`crate::retainer_walk::client_user_id`]) —
//! DocuSign does not email them, so a short-lived [recipient view] URL is the
//! only way in. This route mints one and **redirects the browser to DocuSign**.
//! The ceremony happens on the provider's own site; Navigator does not frame
//! it.
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
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

use crate::admin::AdminState;
use crate::retainer_walk::client_user_id;
use crate::signature::RecipientView;

/// `GET /app/lawyer/notations/:id/sign` — mint a single-use recipient-view URL for
/// the notation's captive client and send the browser to DocuSign.
pub async fn sign_get(
    State(state): State<AdminState>,
    AxumPath(notation_id): AxumPath<Uuid>,
) -> Response {
    let Some(notation_row) = store::notations::find_by_id(&state.surreal, notation_id)
        .await
        .ok()
        .flatten()
    else {
        return (StatusCode::NOT_FOUND, "notation not found").into_response();
    };

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
    // triple, so they must match the envelope exactly: the client's
    // Person row + the notation-derived client_user_id.
    let Some(client) = store::persons::find_by_id(&state.surreal, notation_row.person_id)
        .await
        .ok()
        .flatten()
    else {
        return (StatusCode::NOT_FOUND, "client not found").into_response();
    };

    // Where DocuSign sends the browser once the ceremony ends — the matter's
    // step page, which reflects the post-signature state. A courtesy for the
    // signer who comes back, not a mechanism: the executed state arrives on
    // `crate::esignature_webhook` whether this redirect is ever followed or
    // not.
    let return_url = format!("/app/lawyer/notations/{notation_id}/step");
    let view = RecipientView {
        return_url,
        email: client.email,
        name: client.name,
        client_user_id: client_user_id(notation_id),
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
