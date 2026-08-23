//! The `BillingCanary` Restate workflow — a live, multi-step health check
//! for the billing integration.
//!
//! Two durable steps, each journaled independently (which is exactly why
//! this is a Restate workflow and not a one-shot batch — a retry of the
//! Xero step must not re-send the confirmation email, and vice versa):
//!
//! 1. `ctx.run("xero")` — find-or-create a single **stable** canary
//!    contact in the connected Xero org and confirm the resolve is
//!    idempotent. Because [`billing`]'s find-or-create keys on email, a
//!    fixed canary email means exactly one canary contact ever exists
//!    (created on the first run, found on every run after) — a heartbeat,
//!    not nightly junk. This proves token minting, tenant routing, the
//!    contact lookup `where` predicate, and the create payload all still
//!    agree with Xero's API.
//! 2. `ctx.run("email")` — send a confirmation email with the result.
//!
//! Boundary (per the legal-council review): the canary touches **Contacts
//! only** — never an invoice, payment, or trust transaction — so it never
//! writes a financial record into the firm's books of account.
//!
//! The Xero step's core ([`run_canary`]) and the email body
//! ([`build_confirmation`]) are pure/provider-agnostic so they unit-test
//! against the [`billing::StubBillingProvider`] without a worker.

use std::sync::Arc;

use billing::{BillingProvider, ContactRequest, XeroBillingProvider};
use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use workflows::{EmailService, OutboundEmail};

/// The canary's stable display name (Xero's required unique `Name` on
/// create). Labelled a system probe so no bookkeeper mistakes it for a
/// real client (legal-council ask).
const CANARY_NAME: &str = "Neon Law Navigator Billing Canary [system health check]";

/// The canary's stable email — the find-or-create match key. Never
/// emailed; it only anchors the lookup so the same contact is reused
/// every run.
const CANARY_EMAIL: &str = "billing-canary@neonlaw.com";

/// Default confirmation-email recipient when `BILLING_CANARY_NOTIFY_EMAIL`
/// is unset.
const DEFAULT_NOTIFY_EMAIL: &str = "nick@neonlaw.com";

/// Request body for `BillingCanary::run`. Empty — the trigger only starts
/// the workflow — but kept as a struct so fields can be threaded later
/// without changing the handler signature.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct RunRequest {}

/// Result of the Xero step, surfaced as the Restate invocation output.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CanaryReport {
    /// The Xero `ContactID` the canary resolved to.
    pub contact_id: String,
    /// True when the second resolve returned the same id — find-or-create
    /// is idempotent on email, as it must be.
    pub idempotent: bool,
}

/// Service registered with the Restate endpoint. Holds the worker-side
/// [`EmailService`] (for the confirmation step); the Xero provider is
/// built from env inside the Xero step so no token or HTTP client is held
/// idle between runs. Same shape as `archives`'s `ArchivesService`.
#[derive(Clone)]
pub struct BillingCanaryService {
    email: Arc<dyn EmailService>,
}

impl BillingCanaryService {
    #[must_use]
    pub fn new(email: Arc<dyn EmailService>) -> Self {
        Self { email }
    }
}

#[restate_sdk::workflow(name = "BillingCanary")]
impl BillingCanaryService {
    #[restate_sdk::handler]
    async fn run(
        &self,
        ctx: WorkflowContext<'_>,
        _req: Json<RunRequest>,
    ) -> Result<Json<CanaryReport>, HandlerError> {
        // Step 1 — Xero: build the real provider and run the canary. A
        // missing Xero config is terminal (no point retrying); a
        // provider/API error is retryable, so Restate replays just this
        // step without re-sending the email below.
        let report: CanaryReport = ctx
            .run(|| async {
                let provider = XeroBillingProvider::from_env().ok_or_else(|| {
                    TerminalError::new("Xero is not configured (XERO_* env unset)")
                })?;
                Ok(Json(run_canary(&provider).await?))
            })
            .name("xero")
            .await?
            .into_inner();

        // Step 2 — email confirmation, journaled separately so a Xero-step
        // retry never re-sends and an email-send retry never re-resolves.
        let email = build_confirmation(&report, &notify_recipient(|k| std::env::var(k).ok()));
        let svc = Arc::clone(&self.email);
        ctx.run(move || async move {
            svc.send(email)
                .await
                .map(|_| ())
                .map_err(HandlerError::from)
        })
        .name("email")
        .await?;

        Ok(Json(report))
    }
}

/// Resolve the stable canary contact twice and confirm the same id comes
/// back. Provider-agnostic so it unit-tests against the stub.
pub async fn run_canary(
    provider: &dyn BillingProvider,
) -> Result<CanaryReport, billing::BillingError> {
    let request = ContactRequest {
        name: CANARY_NAME.to_string(),
        email: CANARY_EMAIL.to_string(),
    };
    let first = provider.ensure_contact(&request).await?;
    let second = provider.ensure_contact(&request).await?;
    Ok(CanaryReport {
        idempotent: first == second,
        contact_id: first.0,
    })
}

/// The confirmation-email recipient: `BILLING_CANARY_NOTIFY_EMAIL`, else
/// the default. Takes a `key -> value` lookup so it is unit-testable
/// without mutating process env.
fn notify_recipient<F: Fn(&str) -> Option<String>>(get: F) -> String {
    get("BILLING_CANARY_NOTIFY_EMAIL")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_NOTIFY_EMAIL.to_string())
}

/// Build the plain-text confirmation email for a completed canary run.
/// Pure — exposed for unit-testing the rendered subject/body.
#[must_use]
pub fn build_confirmation(report: &CanaryReport, recipient: &str) -> OutboundEmail {
    let status = if report.idempotent { "OK" } else { "FAILED" };
    let subject = format!(
        "Billing canary {status} — Xero contact {}",
        report.contact_id
    );
    let body = format!(
        "The weekly billing canary ran against Xero.\n\n\
         Resolved contact: {}\n\n\
         Idempotent (same id on re-resolve): {}\n\n\
         This is an automated health check — it find-or-creates one stable \
         contact and never touches invoices or payments.\n",
        report.contact_id, report.idempotent,
    );
    // Wrap the same body in the firm-branded HTML layout so the email
    // carries the Neon Law logo (this is a firm billing email); the
    // plain-text body stays the fallback part.
    let html = workflows::email::render_email_html(&body, &workflows::email::base_url_from_env());
    OutboundEmail::new(recipient.to_string(), subject, body).with_html(html)
}

#[cfg(test)]
mod tests {
    use super::{build_confirmation, notify_recipient, run_canary, CanaryReport};
    use billing::StubBillingProvider;

    #[tokio::test]
    async fn canary_resolves_idempotently_against_the_stub() {
        let stub = StubBillingProvider::new();
        let report = run_canary(&stub).await.expect("stub never errors");
        assert!(report.idempotent, "two resolves return the same id");
        assert!(
            report.contact_id.starts_with("stub-contact-"),
            "real provider id shape: {}",
            report.contact_id
        );
        // The canary resolved the contact exactly twice.
        assert_eq!(stub.contact_calls().len(), 2);
    }

    #[test]
    fn confirmation_email_carries_status_and_contact_id() {
        let report = CanaryReport {
            contact_id: "c-123".into(),
            idempotent: true,
        };
        let email = build_confirmation(&report, "ops@example.com");
        assert_eq!(email.to, "ops@example.com");
        assert!(email.subject.contains("OK"), "subject: {}", email.subject);
        assert!(email.subject.contains("c-123"));
        assert!(email.body.contains("Idempotent"));
    }

    #[test]
    fn notify_recipient_defaults_then_honors_env() {
        assert_eq!(notify_recipient(|_| None), "nick@neonlaw.com");
        assert_eq!(
            notify_recipient(|_| Some(String::new())),
            "nick@neonlaw.com"
        );
        assert_eq!(
            notify_recipient(
                |k| (k == "BILLING_CANARY_NOTIFY_EMAIL").then(|| "ops@example.com".to_string())
            ),
            "ops@example.com"
        );
    }
}
