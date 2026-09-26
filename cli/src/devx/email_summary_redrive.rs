//! `navigator ops email-summary redrive --receipt <uuid>` — re-run a
//! completed `EmailSummary` Restate workflow for one receipt (ENG-888).
//!
//! A workflow key admits at most one invocation. A run that completed with a
//! bounded provider failure (for example `input_digest_mismatch`) can't be
//! resubmitted through intake — `SendGrid` never re-POSTs a message that
//! already got a 202 — so an operator needs a way to purge the retained,
//! completed invocation and resubmit the identical request under the same
//! key. This never creates a second receipt, letter, or archive: those are
//! digest-keyed in `SurrealDB` already and this command never touches them.
//!
//! Diagnostics are status words only: this module never writes a receipt
//! id, an invocation id, an email body, a summary, or any letter/archive
//! content. The operator already supplied the receipt as the command
//! argument.
//!
//! The exact admin-API shape (`sys_invocation` introspection then a purge)
//! mirrors Restate's documented invocation-lifecycle model but is exercised
//! here only against a `wiremock` double, not a live cluster — confirm
//! against a real environment (staging) before depending on it in
//! production.

use anyhow::{bail, Context, Result};
use uuid::Uuid;

const SERVICE: &str = "EmailSummary";
const HANDLER: &str = "run";

/// `navigator ops email-summary redrive --receipt <uuid>`.
pub fn run(receipt: Uuid) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    runtime.block_on(redrive(receipt))
}

async fn redrive(receipt_id: Uuid) -> Result<()> {
    let surreal = store::surreal::connect_from_env()
        .await
        .context("connect to SurrealDB")?;
    let config = workflows::EmailSummaryConfig::from_env()
        .context("configure inbound email summary lane")?
        .with_context(|| "NAVIGATOR_SUMMARY_ENABLED is not set; nothing to redrive against")?;
    let admin_url = std::env::var("RESTATE_ADMIN_URL")
        .context("RESTATE_ADMIN_URL is required to purge the completed invocation")?;
    let admin_token = std::env::var("RESTATE_ADMIN_TOKEN")
        .context("RESTATE_ADMIN_TOKEN is required to purge the completed invocation")?;
    let auth_token = std::env::var("RESTATE_AUTH_TOKEN").ok();

    redrive_with(
        &surreal,
        &admin_url,
        &admin_token,
        auth_token.as_deref(),
        &config,
        receipt_id,
    )
    .await
}

/// The testable core: every I/O boundary (the database, the admin API, the
/// ingress) is a parameter, so a test can point each at a `wiremock` double
/// or an in-memory `SurrealDb` instead of a live deployment.
async fn redrive_with(
    surreal: &store::surreal::SurrealDb,
    admin_url: &str,
    admin_token: &str,
    auth_token: Option<&str>,
    config: &workflows::EmailSummaryConfig,
    receipt_id: Uuid,
) -> Result<()> {
    let receipt = store::email_receipts::find_by_id(surreal, receipt_id)
        .await
        .context("load the receipt")?
        .context("receipt not found")?;

    refuse_if_confirmed(surreal, receipt_id).await?;

    let request = config
        .request_for(receipt_id, &receipt.raw_digest)
        .context("build the EmailSummaryRequest")?;

    let key = receipt_id.to_string();
    purge_retained_invocation(admin_url, admin_token, SERVICE, &key).await?;

    workflows::start_workflow(
        &config.workflow_ingress,
        auth_token,
        SERVICE,
        &key,
        HANDLER,
        &request,
        true,
    )
    .await
    .map_err(|error| anyhow::anyhow!("email-summary trigger failed: {error}"))?;

    println!("EmailSummary workflow resubmitted");
    Ok(())
}

/// Refuse the redrive when the receipt's Slack delivery is already
/// `confirmed` — resubmitting could risk a second post. Every other state
/// (no delivery row yet, `not_attempted`, `sending`, `unknown`, `failed`) is
/// safe to redrive.
async fn refuse_if_confirmed(surreal: &store::surreal::SurrealDb, receipt_id: Uuid) -> Result<()> {
    let Some(delivery) = store::email_deliveries::find(surreal, receipt_id)
        .await
        .context("load delivery state")?
    else {
        return Ok(());
    };
    if delivery.state == store::email_deliveries::CONFIRMED {
        bail!("refused — delivery_state is confirmed; redriving would risk a second Slack post");
    }
    Ok(())
}

/// Find and purge the retained invocation for `service`/`key`, if one
/// exists. A workflow key with nothing retained (never run, or already
/// purged) is not an error — there is simply nothing to free.
async fn purge_retained_invocation(
    admin_url: &str,
    admin_token: &str,
    service: &str,
    key: &str,
) -> Result<()> {
    if let Some(invocation_id) = find_invocation_id(admin_url, admin_token, service, key).await? {
        delete_invocation(admin_url, admin_token, &invocation_id).await?;
        println!("purged retained invocation");
    } else {
        println!("no retained invocation to purge");
    }
    Ok(())
}

/// Look up the invocation id retained for one workflow key via Restate's
/// introspection query endpoint (`sys_invocation`). Never surfaces anything
/// but the id: the query result carries no client content.
async fn find_invocation_id(
    admin_url: &str,
    admin_token: &str,
    service: &str,
    key: &str,
) -> Result<Option<String>> {
    let query = format!(
        "SELECT id FROM sys_invocation WHERE target_service_name = '{service}' AND \
         target_service_key = '{key}' LIMIT 1"
    );
    let response = reqwest::Client::new()
        .post(format!("{}/query", admin_url.trim_end_matches('/')))
        .bearer_auth(admin_token)
        .json(&serde_json::json!({ "query": query }))
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("email-summary purge transport failure"))?;
    if !response.status().is_success() {
        let status = response.status();
        bail!(
            "email-summary purge rejected with status {}",
            status.as_u16()
        );
    }
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|_| anyhow::anyhow!("email-summary purge response was invalid"))?;
    Ok(body
        .get("rows")
        .and_then(serde_json::Value::as_array)
        .and_then(|rows| rows.first())
        .and_then(|row| row.get("id"))
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned))
}

/// Purge one retained invocation, freeing its workflow key for a new
/// invocation.
async fn delete_invocation(admin_url: &str, admin_token: &str, invocation_id: &str) -> Result<()> {
    let response = reqwest::Client::new()
        .delete(format!(
            "{}/invocations/{invocation_id}",
            admin_url.trim_end_matches('/')
        ))
        .bearer_auth(admin_token)
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("email-summary purge transport failure"))?;
    if !response.status().is_success() {
        let status = response.status();
        bail!(
            "email-summary purge rejected with status {}",
            status.as_u16()
        );
    }
    Ok(())
}

/// Pull `invocationId` out of the ingress's JSON response body. Used by
/// tests to pin the Restate response shape; the command itself never
/// writes the id.
#[cfg(test)]
fn parse_invocation_id(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("invocationId")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    #[test]
    fn parse_invocation_id_reads_the_ingress_response_shape() {
        assert_eq!(
            parse_invocation_id(r#"{"invocationId":"inv_1iwasQyBdOd86uz984WISv0Xx43rFVuH0o"}"#),
            Some("inv_1iwasQyBdOd86uz984WISv0Xx43rFVuH0o".to_string())
        );
        assert_eq!(parse_invocation_id("not json"), None);
        assert_eq!(parse_invocation_id("{}"), None);
    }

    #[tokio::test]
    async fn find_invocation_id_reads_the_query_result_row() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/query"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "rows": [{ "id": "inv_abc123" }]
            })))
            .mount(&server)
            .await;

        let id = find_invocation_id(&server.uri(), "admin-token", "EmailSummary", "receipt-1")
            .await
            .expect("query succeeds");
        assert_eq!(id.as_deref(), Some("inv_abc123"));
    }

    #[tokio::test]
    async fn find_invocation_id_of_an_unretained_key_is_none() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/query"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "rows": [] })),
            )
            .mount(&server)
            .await;

        let id = find_invocation_id(&server.uri(), "admin-token", "EmailSummary", "receipt-1")
            .await
            .expect("query succeeds");
        assert_eq!(id, None);
    }

    #[tokio::test]
    async fn delete_invocation_sends_a_bearer_authenticated_delete() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/invocations/inv_abc123"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        delete_invocation(&server.uri(), "admin-token", "inv_abc123")
            .await
            .expect("purge succeeds");
    }

    #[tokio::test]
    async fn purge_retained_invocation_is_a_no_op_when_nothing_is_retained() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/query"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "rows": [] })),
            )
            .mount(&server)
            .await;
        // No DELETE mock at all: this test fails (unmatched request) if the
        // "nothing retained" path ever tries to delete something anyway.

        purge_retained_invocation(&server.uri(), "admin-token", "EmailSummary", "receipt-1")
            .await
            .expect("no-op purge succeeds");
    }

    fn test_config(ingress: &str) -> workflows::EmailSummaryConfig {
        workflows::EmailSummaryConfig {
            envelope_recipients: vec!["support@example.com".to_string()],
            inbound_public_key: "test-key".to_string(),
            deployment: "staging".to_string(),
            workflow_ingress: ingress.to_string(),
            project_id: "synthetic-project".to_string(),
            channel_id: "C-SYNTHETIC".to_string(),
            gemini_model: "gemini-test".to_string(),
            gemini_location: "us-central1".to_string(),
            max_input_chars: 32_000,
            max_output_tokens: 1_024,
        }
    }

    async fn setup_receipt(db: &store::surreal::SurrealDb) -> Uuid {
        use sha2::{Digest as _, Sha256};
        let digest = Sha256::digest(Uuid::now_v7().as_bytes()).iter().fold(
            String::new(),
            |mut out, byte| {
                use std::fmt::Write as _;
                let _ = write!(out, "{byte:02x}");
                out
            },
        );
        store::email_receipts::ensure(
            db,
            &store::email_receipts::NewEmailReceipt {
                receiving_mailbox: "support@example.com",
                deployment: "staging",
                raw_digest: &digest,
                source_message_id: None,
                archive_key: "inbound/example.eml",
                letter_id: Uuid::now_v7(),
            },
        )
        .await
        .expect("receipt setup")
        .receipt
        .id
    }

    #[tokio::test]
    async fn redrive_refuses_when_delivery_is_confirmed() {
        let db = store::surreal::test_support::mem().await;
        let receipt_id = setup_receipt(&db).await;
        store::email_deliveries::ensure(&db, receipt_id, "C-SYNTHETIC")
            .await
            .expect("delivery setup");
        store::email_deliveries::mark_confirmed(&db, receipt_id, "C-SYNTHETIC", "1700000000.1")
            .await
            .expect("mark confirmed");

        // No mocks at all on either server: this test fails on an unexpected
        // request if the refusal doesn't short-circuit before touching
        // Restate.
        let admin = MockServer::start().await;
        let ingress = MockServer::start().await;
        let config = test_config(&ingress.uri());

        let error = redrive_with(&db, &admin.uri(), "admin-token", None, &config, receipt_id)
            .await
            .expect_err("a confirmed delivery refuses the redrive");
        let message = error.to_string();
        assert!(message.contains("confirmed"));
        assert!(
            !message.contains(&receipt_id.to_string()),
            "CodeQL cleartext-logging: errors must not carry a receipt id: {message}"
        );

        let receipt = store::email_receipts::find_by_id(&db, receipt_id)
            .await
            .expect("receipt still readable")
            .expect("receipt still exists");
        assert_eq!(receipt.archive_key, "inbound/example.eml");
    }

    #[tokio::test]
    async fn redrive_of_a_missing_receipt_names_the_gap_without_the_id() {
        let db = store::surreal::test_support::mem().await;
        let receipt_id = Uuid::now_v7();
        let admin = MockServer::start().await;
        let ingress = MockServer::start().await;
        let config = test_config(&ingress.uri());

        let error = redrive_with(&db, &admin.uri(), "admin-token", None, &config, receipt_id)
            .await
            .expect_err("a missing receipt refuses the redrive");
        let message = error.to_string();
        assert!(message.contains("receipt not found"));
        assert!(
            !message.contains(&receipt_id.to_string()),
            "CodeQL cleartext-logging: errors must not carry a receipt id: {message}"
        );
    }

    #[tokio::test]
    async fn redrive_reuses_the_receipt_and_yields_one_new_invocation() {
        let db = store::surreal::test_support::mem().await;
        let receipt_id = setup_receipt(&db).await;
        let before = store::email_receipts::find_by_id(&db, receipt_id)
            .await
            .expect("receipt readable")
            .expect("receipt exists");

        let admin = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/query"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "rows": [] })),
            )
            .mount(&admin)
            .await;
        let ingress = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/EmailSummary/{receipt_id}/run/send")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "invocationId": "inv_new_1" })),
            )
            .expect(1)
            .mount(&ingress)
            .await;
        let config = test_config(&ingress.uri());

        redrive_with(
            &db,
            &admin.uri(),
            "admin-token",
            Some("ingress-token"),
            &config,
            receipt_id,
        )
        .await
        .expect("redrive succeeds");

        let after = store::email_receipts::find_by_id(&db, receipt_id)
            .await
            .expect("receipt readable")
            .expect("receipt exists");
        assert_eq!(
            before, after,
            "the receipt, letter, and archive it names are untouched"
        );
    }

    #[tokio::test]
    async fn purge_retained_invocation_deletes_what_the_query_finds() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/query"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "rows": [{ "id": "inv_abc123" }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/invocations/inv_abc123"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        purge_retained_invocation(&server.uri(), "admin-token", "EmailSummary", "receipt-1")
            .await
            .expect("purge succeeds");
    }

    #[tokio::test]
    async fn redrive_trigger_failure_is_status_only() {
        let db = store::surreal::test_support::mem().await;
        let receipt_id = setup_receipt(&db).await;
        let admin = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/query"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "rows": [] })),
            )
            .mount(&admin)
            .await;
        let ingress = MockServer::start().await;
        let invocation = "invocation-sentinel-906";
        let request_url = format!("{}/EmailSummary/{receipt_id}/run/send", ingress.uri());
        Mock::given(method("POST"))
            .and(path(format!("/EmailSummary/{receipt_id}/run/send")))
            .respond_with(ResponseTemplate::new(503).set_body_string(format!(
                "trigger failure {receipt_id} {invocation} {request_url}"
            )))
            .mount(&ingress)
            .await;

        let error = redrive_with(
            &db,
            &admin.uri(),
            "admin-token",
            None,
            &test_config(&ingress.uri()),
            receipt_id,
        )
        .await
        .expect_err("the rejected trigger must fail redrive");
        let message = error.to_string();
        assert!(message.contains("503"));
        for unsafe_value in [receipt_id.to_string(), invocation.to_string(), request_url] {
            assert!(
                !message.contains(&unsafe_value),
                "unsafe value in redrive error: {message}"
            );
        }
    }

    #[tokio::test]
    async fn purge_failure_is_status_only() {
        let server = MockServer::start().await;
        let receipt = "receipt-sentinel-906";
        let invocation = "invocation-sentinel-906";
        let request_url = format!("{}/invocations/{invocation}", server.uri());
        Mock::given(method("POST"))
            .and(path("/query"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "rows": [{ "id": invocation }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path(format!("/invocations/{invocation}")))
            .respond_with(ResponseTemplate::new(503).set_body_string(format!(
                "purge failure {receipt} {invocation} {request_url}"
            )))
            .mount(&server)
            .await;

        let error =
            purge_retained_invocation(&server.uri(), "admin-token", "EmailSummary", receipt)
                .await
                .expect_err("the rejected purge must fail");
        let message = error.to_string();
        assert!(message.contains("503"));
        for unsafe_value in [receipt, invocation, request_url.as_str()] {
            assert!(
                !message.contains(unsafe_value),
                "unsafe value in purge error: {message}"
            );
        }
    }

    #[tokio::test]
    async fn purge_transport_failure_is_status_only() {
        let admin_url = "http://[::1";
        let receipt = "receipt-sentinel-transport-906";
        let error = purge_retained_invocation(admin_url, "admin-token", "EmailSummary", receipt)
            .await
            .expect_err("the unreachable purge endpoint must fail");
        let message = error.to_string();
        assert!(!message.contains(admin_url));
        assert!(!message.contains(receipt));
    }
}
