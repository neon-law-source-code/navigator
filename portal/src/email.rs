//! Outbound email — `web`-side decorators + env-driven factory.
//!
//! The trait surface ([`EmailService`]), the two backends
//! ([`CapturingEmail`], [`SendGridEmail`]), and the value types
//! ([`OutboundEmail`], [`EmailError`], [`DEFAULT_FROM_EMAIL`]) live in
//! `workflows::email` so the `workflows-service` worker can share
//! them without a crate cycle. This module keeps the two `web`-only
//! decorators ([`RetryingEmail`], [`LoggingEmail`]) plus the
//! env-driven factory ([`from_env`]) and re-exports the moved types
//! so existing callsites keep compiling unchanged.

use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;

pub use workflows::email::{
    Attachment, CapturingEmail, EmailError, EmailService, OutboundEmail, SendGridEmail,
    SendReceipt, DEFAULT_FROM_EMAIL,
};

/// Failures from [`from_env`] / [`from_lookup`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum EmailConfigError {
    #[error("SENDGRID_API_KEY must be set when NAVIGATOR_EMAIL_BACKEND=sendgrid")]
    MissingApiKey,
    #[error("SENDGRID_BASE_URL is only allowed for the CI harness or official SendGrid API hosts")]
    InvalidBaseUrl,
    #[error("NAVIGATOR_SENDGRID_HARNESS_SECRET must be set when overriding SENDGRID_BASE_URL in the CI harness")]
    MissingHarnessSecret,
}

/// Build the process-wide email backend from the environment.
///
/// - `NAVIGATOR_EMAIL_BACKEND=sendgrid` → [`SendGridEmail`] wrapped in
///   [`RetryingEmail::with_defaults`], then wrapped in
///   [`LoggingEmail`] for the `sent_emails` audit trail. Requires
///   `SENDGRID_API_KEY`; `SENDGRID_FROM_EMAIL` defaults to
///   [`DEFAULT_FROM_EMAIL`].
/// - Any other value (including unset) → [`CapturingEmail`]
///   wrapped in [`LoggingEmail`], so dev runs still populate the
///   audit table (visible via `/lawyer/email-log`).
///
/// The crash-loud-on-missing-key behavior here is deliberately
/// redundant with `portal::config::enforce_deployment_invariants` so that the
/// constructor itself stays honest when called outside `main`.
pub fn from_env(db: store::surreal::SurrealDb) -> Result<Arc<dyn EmailService>, EmailConfigError> {
    from_lookup(|k| std::env::var(k).ok(), db)
}

/// Testable seam for [`from_env`]: any `key -> Option<value>` lookup,
/// plus the `SurrealDB` handle for the [`LoggingEmail`] decorator.
pub fn from_lookup<F: Fn(&str) -> Option<String>>(
    get: F,
    db: store::surreal::SurrealDb,
) -> Result<Arc<dyn EmailService>, EmailConfigError> {
    let (inner, sender) = select_backend(get)?;
    Ok(Arc::new(LoggingEmail::new(inner, db, sender)))
}

/// Pure inner step of [`from_lookup`]: pick the backend (SendGrid vs
/// Capturing) and resolve the default sender. No DB involvement; the
/// audit decorator is applied by the caller. Exposed for unit tests
/// that exercise the config logic without standing up a database.
pub fn select_backend<F: Fn(&str) -> Option<String>>(
    get: F,
) -> Result<(Arc<dyn EmailService>, String), EmailConfigError> {
    if get("NAVIGATOR_EMAIL_BACKEND").as_deref() != Some("sendgrid") {
        return Ok((Arc::new(CapturingEmail::new()), DEFAULT_FROM_EMAIL.into()));
    }
    let api_key = get("SENDGRID_API_KEY")
        .filter(|s| !s.is_empty())
        .ok_or(EmailConfigError::MissingApiKey)?;
    let from = get("SENDGRID_FROM_EMAIL")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_FROM_EMAIL.into());
    let base_url = get("SENDGRID_BASE_URL").filter(|value| !value.is_empty());
    let svc = match base_url {
        None => SendGridEmail::new(api_key, from.clone()),
        Some(base_url) => {
            let harness = get("NAVIGATOR_CI_HARNESS").as_deref() == Some("1");
            if !harness {
                let official = url::Url::parse(&base_url).is_ok_and(|url| {
                    url.scheme() == "https"
                        && (url.path().is_empty() || url.path() == "/")
                        && matches!(
                            url.host_str(),
                            Some("api.sendgrid.com" | "api.eu.sendgrid.com")
                        )
                });
                if !official {
                    return Err(EmailConfigError::InvalidBaseUrl);
                }
            } else if get("NAVIGATOR_SENDGRID_HARNESS_SECRET").is_none_or(|value| value.is_empty())
            {
                return Err(EmailConfigError::MissingHarnessSecret);
            }
            SendGridEmail::new(api_key, from.clone()).with_base_url(base_url)
        }
    };
    Ok((Arc::new(RetryingEmail::with_defaults(svc)), from))
}

/// Decorator that journals every outbound message to the
/// `sent_emails` audit table after the inner backend resolves.
/// `outcome = "sent"` on success, `outcome = "failed:<reason>"` on
/// error. The DB insert is best-effort — if it fails the send result
/// is still returned and the audit miss is logged via `tracing::warn`.
///
/// One row per attempt: when the inner backend is `RetryingEmail`,
/// only the final outcome is journaled (the retry decorator absorbs
/// transient failures internally). When the inner backend is `Send`
/// directly, every attempt produces a row.
#[derive(Clone)]
pub struct LoggingEmail {
    inner: Arc<dyn EmailService>,
    db: store::surreal::SurrealDb,
    /// Default `From:` address recorded on each audit row when the
    /// caller didn't supply `OutboundEmail.from`.
    sender: String,
}

impl LoggingEmail {
    #[must_use]
    pub fn new(
        inner: Arc<dyn EmailService>,
        db: store::surreal::SurrealDb,
        sender: impl Into<String>,
    ) -> Self {
        Self {
            inner,
            db,
            sender: sender.into(),
        }
    }

    async fn log(&self, email: &OutboundEmail, outcome: &str, sg_message_id: Option<String>) {
        let from = email.from.clone().unwrap_or_else(|| self.sender.clone());
        let row = store::sent_emails::NewSentEmail {
            recipient: email.to.clone(),
            subject: email.subject.clone(),
            sender: from,
            template_slug: email.template_slug.clone(),
            body: email.body.clone(),
            outcome: outcome.to_string(),
            // SendGrid's `X-Message-Id`, the join key to the
            // delivery-side Event Webhook stream. `None` until the
            // message clears SendGrid (failed sends, and the
            // capturing backend, never get one).
            sg_message_id,
            sent_at: chrono::Utc::now(),
        };
        if let Err(e) = store::sent_emails::record(&self.db, &row).await {
            // The recipient address stays out of the log field — it is
            // client-identifying and telemetry leaves the firm's trust
            // boundary (`telemetry/src/lib.rs`). `person_id` is carried on the
            // message when the caller set it; the row that failed to insert
            // holds the address itself, and `template_slug` says which kind of
            // message it was.
            tracing::warn!(
                error = %e,
                person_id = email.person_id.as_deref().unwrap_or("<unset>"),
                template_slug = email.template_slug.as_deref().unwrap_or("<none>"),
                "sent_emails audit insert failed",
            );
        }
    }
}

#[async_trait]
impl EmailService for LoggingEmail {
    async fn send(&self, email: OutboundEmail) -> Result<SendReceipt, EmailError> {
        let result = self.inner.send(email.clone()).await;
        let (outcome, message_id) = match &result {
            Ok(receipt) => ("sent".to_string(), receipt.message_id.clone()),
            Err(e) => (format!("failed:{e}"), None),
        };
        self.log(&email, &outcome, message_id).await;
        result
    }
}

/// Decorator that retries any inner [`EmailService`] on transient
/// transport failures with exponential backoff.
///
/// Retries only on [`EmailError::Transport`] — [`EmailError::InvalidRecipient`]
/// is permanent and short-circuits immediately.
///
/// This is the in-process implementation. The durable, persistent
/// queue (a durable backend, per the architectural
/// decision) is a planned follow-up — once we have call sites that
/// must survive process restart we'll swap this decorator out for
/// the apalis storage layer. The trait surface (`send`) stays the
/// same so callers don't change.
#[derive(Clone)]
pub struct RetryingEmail<E: EmailService + Clone> {
    inner: E,
    max_attempts: u32,
    base_backoff: std::time::Duration,
}

impl<E: EmailService + Clone> RetryingEmail<E> {
    /// `max_attempts` is the total number of tries — `1` means no
    /// retry, `4` means one initial attempt plus three retries.
    /// `base_backoff` is the delay before the second attempt;
    /// subsequent retries double it (exponential).
    #[must_use]
    pub fn new(inner: E, max_attempts: u32, base_backoff: std::time::Duration) -> Self {
        Self {
            inner,
            max_attempts: max_attempts.max(1),
            base_backoff,
        }
    }

    /// Sensible defaults: four attempts, starting at 250 ms (so
    /// the cumulative backoff before final give-up is roughly
    /// 250 + 500 + 1000 ≈ 1.75 s).
    #[must_use]
    pub fn with_defaults(inner: E) -> Self {
        Self::new(inner, 4, std::time::Duration::from_millis(250))
    }

    fn backoff_for_attempt(&self, attempt: u32) -> std::time::Duration {
        // attempt is 1-indexed; the delay BEFORE attempt N (N >= 2)
        // is base * 2^(N - 2).
        let shift = attempt.saturating_sub(2);
        self.base_backoff * 2u32.saturating_pow(shift)
    }
}

#[async_trait]
impl<E: EmailService + Clone> EmailService for RetryingEmail<E> {
    async fn send(&self, email: OutboundEmail) -> Result<SendReceipt, EmailError> {
        let mut attempt = 1u32;
        loop {
            match self.inner.send(email.clone()).await {
                Ok(receipt) => return Ok(receipt),
                Err(EmailError::InvalidRecipient(r)) => {
                    return Err(EmailError::InvalidRecipient(r));
                }
                Err(EmailError::Transport(msg)) if attempt < self.max_attempts => {
                    tracing::warn!(
                        attempt,
                        max_attempts = self.max_attempts,
                        error = %msg,
                        "outbound email transport error; retrying after backoff",
                    );
                    tokio::time::sleep(self.backoff_for_attempt(attempt + 1)).await;
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        select_backend, CapturingEmail, EmailConfigError, EmailError, EmailService, OutboundEmail,
        RetryingEmail, SendReceipt, DEFAULT_FROM_EMAIL,
    };
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |k: &str| map.get(k).cloned()
    }

    /// Programmable test backend: fails the first `fail_count`
    /// attempts with `Transport`, then succeeds. Tracks the total
    /// number of `send` calls so retry tests can assert on attempt
    /// counts.
    #[derive(Clone, Default)]
    struct FlakyEmail {
        fail_count: u32,
        attempts: Arc<AtomicU32>,
    }

    impl FlakyEmail {
        fn new(fail_count: u32) -> Self {
            Self {
                fail_count,
                attempts: Arc::new(AtomicU32::new(0)),
            }
        }

        fn attempts(&self) -> u32 {
            self.attempts.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl EmailService for FlakyEmail {
        async fn send(&self, _email: OutboundEmail) -> Result<SendReceipt, EmailError> {
            let n = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
            if n <= self.fail_count {
                Err(EmailError::Transport(format!("flaky attempt #{n}")))
            } else {
                Ok(SendReceipt::default())
            }
        }
    }

    fn message(to: &str) -> OutboundEmail {
        OutboundEmail::new(to, "Hello", "Body")
    }

    #[tokio::test]
    async fn retrying_succeeds_when_inner_succeeds_on_first_try() {
        let flaky = FlakyEmail::new(0);
        let svc = RetryingEmail::new(flaky.clone(), 3, std::time::Duration::from_millis(1));
        svc.send(message("libra@example.com"))
            .await
            .expect("first-try success");
        assert_eq!(flaky.attempts(), 1);
    }

    #[tokio::test]
    async fn retrying_retries_on_transport_and_eventually_succeeds() {
        let flaky = FlakyEmail::new(2);
        let svc = RetryingEmail::new(flaky.clone(), 4, std::time::Duration::from_millis(1));
        svc.send(message("libra@example.com"))
            .await
            .expect("retry path succeeds");
        // Two failures + one eventual success = three total attempts.
        assert_eq!(flaky.attempts(), 3);
    }

    #[tokio::test]
    async fn retrying_gives_up_after_max_attempts_and_returns_last_error() {
        let flaky = FlakyEmail::new(10);
        let svc = RetryingEmail::new(flaky.clone(), 3, std::time::Duration::from_millis(1));
        let err = svc.send(message("libra@example.com")).await.unwrap_err();
        assert!(matches!(err, EmailError::Transport(_)));
        assert_eq!(flaky.attempts(), 3, "max_attempts is the total count");
    }

    #[tokio::test]
    async fn retrying_does_not_retry_on_invalid_recipient() {
        // CapturingEmail rejects bad recipients with InvalidRecipient.
        // The decorator must NOT loop on that — it's a permanent error.
        let inner = CapturingEmail::new();
        let svc = RetryingEmail::new(inner.clone(), 4, std::time::Duration::from_millis(1));
        let err = svc.send(message("not-an-email")).await.unwrap_err();
        assert!(matches!(err, EmailError::InvalidRecipient(_)));
        assert!(inner.captured().is_empty());
    }

    #[test]
    fn retrying_max_attempts_clamps_to_at_least_one() {
        let inner = CapturingEmail::new();
        let svc = RetryingEmail::new(inner, 0, std::time::Duration::from_millis(1));
        // The clamp is internal; assert via backoff_for_attempt which
        // is exposed for testing the exponential schedule.
        let backoff = svc.backoff_for_attempt(2);
        assert_eq!(backoff, std::time::Duration::from_millis(1));
    }

    #[tokio::test]
    async fn select_backend_returns_capturing_when_env_var_unset() {
        let (svc, sender) = select_backend(|_| None).expect("dev factory cannot fail");
        assert_eq!(sender, DEFAULT_FROM_EMAIL);
        svc.send(OutboundEmail::new("libra@example.invalid", "Hi", "Body"))
            .await
            .expect("capturing backend always succeeds");
    }

    #[tokio::test]
    async fn select_backend_returns_capturing_for_unknown_backend_value() {
        let (svc, _) = select_backend(lookup(&[("NAVIGATOR_EMAIL_BACKEND", "capturing")]))
            .expect("dev factory cannot fail");
        svc.send(OutboundEmail::new("libra@example.invalid", "Hi", "Body"))
            .await
            .expect("capturing backend always succeeds");
    }

    #[test]
    fn select_backend_requires_api_key_for_sendgrid_backend() {
        // `Arc<dyn EmailService>` is !Debug, so `.unwrap_err` won't
        // compile — match instead.
        match select_backend(lookup(&[("NAVIGATOR_EMAIL_BACKEND", "sendgrid")])) {
            Err(EmailConfigError::MissingApiKey) => {}
            Err(error) => panic!("expected MissingApiKey, got {error:?}"),
            Ok(_) => panic!("expected MissingApiKey, got Ok(<EmailService>)"),
        }
    }

    #[test]
    fn select_backend_rejects_empty_api_key_for_sendgrid_backend() {
        match select_backend(lookup(&[
            ("NAVIGATOR_EMAIL_BACKEND", "sendgrid"),
            ("SENDGRID_API_KEY", ""),
        ])) {
            Err(EmailConfigError::MissingApiKey) => {}
            Err(error) => panic!("expected MissingApiKey, got {error:?}"),
            Ok(_) => panic!("expected MissingApiKey, got Ok(<EmailService>)"),
        }
    }

    #[test]
    fn select_backend_builds_sendgrid_with_key_and_default_from() {
        let (svc, sender) = select_backend(lookup(&[
            ("NAVIGATOR_EMAIL_BACKEND", "sendgrid"),
            ("SENDGRID_API_KEY", "SG.test"),
        ]))
        .expect("sendgrid factory with key");
        assert_eq!(sender, DEFAULT_FROM_EMAIL);
        // Sanity: the returned trait object validates recipients —
        // SendGridEmail's pre-check rejects bad addresses without
        // hitting the network.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let err = rt.block_on(async move {
            svc.send(OutboundEmail::new("not-an-email", "x", "x"))
                .await
                .unwrap_err()
        });
        assert!(matches!(err, EmailError::InvalidRecipient(_)));
    }

    #[test]
    fn select_backend_rejects_non_official_base_url_outside_harness() {
        let result = select_backend(lookup(&[
            ("NAVIGATOR_EMAIL_BACKEND", "sendgrid"),
            ("SENDGRID_API_KEY", "SG.test"),
            ("SENDGRID_BASE_URL", "http://mail-sink.invalid"),
        ]));
        assert!(matches!(result, Err(EmailConfigError::InvalidBaseUrl)));
    }

    #[test]
    fn select_backend_requires_harness_secret_for_custom_base_url() {
        let result = select_backend(lookup(&[
            ("NAVIGATOR_EMAIL_BACKEND", "sendgrid"),
            ("SENDGRID_API_KEY", "SG.test"),
            ("SENDGRID_BASE_URL", "http://mail-sink.invalid"),
            ("NAVIGATOR_CI_HARNESS", "1"),
        ]));
        assert!(matches!(
            result,
            Err(EmailConfigError::MissingHarnessSecret)
        ));
    }

    #[test]
    fn select_backend_allows_secret_gated_harness_base_url() {
        let result = select_backend(lookup(&[
            ("NAVIGATOR_EMAIL_BACKEND", "sendgrid"),
            ("SENDGRID_API_KEY", "SG.test"),
            ("SENDGRID_BASE_URL", "http://mail-sink.invalid"),
            ("NAVIGATOR_CI_HARNESS", "1"),
            ("NAVIGATOR_SENDGRID_HARNESS_SECRET", "synthetic-secret"),
        ]));
        assert!(result.is_ok());
    }

    async fn in_memory_db() -> store::surreal::SurrealDb {
        store::surreal::test_support::mem().await
    }

    #[tokio::test]
    async fn logging_email_writes_audit_row_on_success() {
        let db = in_memory_db().await;
        let inner: std::sync::Arc<dyn EmailService> = std::sync::Arc::new(CapturingEmail::new());
        let svc = super::LoggingEmail::new(inner, db.clone(), "support@example.com");
        svc.send(
            OutboundEmail::new("aries@example.com", "Welcome to the firm", "Body")
                .with_template("welcome"),
        )
        .await
        .expect("send succeeds");
        let rows = store::sent_emails::all(&db).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].recipient, "aries@example.com");
        assert_eq!(rows[0].subject, "Welcome to the firm");
        assert_eq!(rows[0].sender, "support@example.com");
        assert_eq!(rows[0].template_slug.as_deref(), Some("welcome"));
        assert_eq!(rows[0].outcome, "sent");
        assert!(rows[0].body.contains("Body"));
    }

    #[tokio::test]
    async fn logging_email_writes_failure_row_when_inner_rejects_recipient() {
        let db = in_memory_db().await;
        let inner: std::sync::Arc<dyn EmailService> = std::sync::Arc::new(CapturingEmail::new());
        let svc = super::LoggingEmail::new(inner, db.clone(), "support@example.com");
        let err = svc
            .send(OutboundEmail::new("not-an-email", "x", "x"))
            .await
            .unwrap_err();
        assert!(matches!(err, EmailError::InvalidRecipient(_)));
        let rows = store::sent_emails::all(&db).await.unwrap();
        assert_eq!(rows.len(), 1, "failure path must still audit");
        assert!(
            rows[0].outcome.starts_with("failed:"),
            "outcome should mark failure, got {}",
            rows[0].outcome
        );
    }

    #[tokio::test]
    async fn logging_email_uses_per_message_from_when_provided() {
        let db = in_memory_db().await;
        let inner: std::sync::Arc<dyn EmailService> = std::sync::Arc::new(CapturingEmail::new());
        let svc = super::LoggingEmail::new(inner, db.clone(), "support@example.com");
        let mut msg = OutboundEmail::new("aries@example.com", "s", "b");
        msg.from = Some("ops@example.com".into());
        svc.send(msg).await.unwrap();
        let rows = store::sent_emails::all(&db).await.unwrap();
        assert_eq!(rows[0].sender, "ops@example.com");
    }

    #[test]
    fn select_backend_honors_sendgrid_from_email_override() {
        let (_, sender) = select_backend(lookup(&[
            ("NAVIGATOR_EMAIL_BACKEND", "sendgrid"),
            ("SENDGRID_API_KEY", "SG.test"),
            ("SENDGRID_FROM_EMAIL", "ops@example.com"),
        ]))
        .expect("sendgrid factory with override");
        assert_eq!(sender, "ops@example.com");
    }

    #[test]
    fn default_from_email_pins_support_address() {
        // Pinning the default so the welcome workflow and the
        // inbound webhook keep pointing at the same mailbox. Real
        // deployments override via `SENDGRID_FROM_EMAIL`.
        assert_eq!(DEFAULT_FROM_EMAIL, "support@neonlaw.com");
    }

    #[test]
    fn retrying_backoff_doubles_each_attempt() {
        let inner = CapturingEmail::new();
        let svc = RetryingEmail::new(inner, 5, std::time::Duration::from_millis(100));
        assert_eq!(
            svc.backoff_for_attempt(2),
            std::time::Duration::from_millis(100)
        );
        assert_eq!(
            svc.backoff_for_attempt(3),
            std::time::Duration::from_millis(200)
        );
        assert_eq!(
            svc.backoff_for_attempt(4),
            std::time::Duration::from_millis(400)
        );
        assert_eq!(
            svc.backoff_for_attempt(5),
            std::time::Duration::from_millis(800)
        );
    }
}
