//! Write one deployment's key material into that deployment's own Secret
//! Manager, which the Secret Manager CSI driver then projects into the pod.
//!
//! This is the second half of the chain the repository's `deployments/` tree
//! starts:
//!
//! ```text
//! repo (SOPS) -> ops secrets apply -> Secret Manager -> CSI -> Secret -> pod
//! ```
//!
//! ## Why the value never reaches a log line
//!
//! A payload rides in a JSON request body and nowhere else: not in `argv` (no
//! `gcloud secrets versions add --data-file=-` shell-out), not in an error
//! message (the operation strings below name the secret, never its value), and
//! not in [`super::client::Mode::DryRun`]'s recorder, which serializes the
//! body it was handed. The dry-run of `ops secrets apply` therefore stops
//! before it decrypts rather than routing a real payload through a recording
//! client — see [`super::super::deployments::apply`].
//!
//! ## Idempotency
//!
//! [`ensure_secret`] POSTs unconditionally and treats HTTP 409 Conflict as
//! success, the same convention every other `ensure_*` in this pipeline
//! follows. [`add_version`] is deliberately *not* idempotent: each call adds a
//! new version and moves `versions/latest`, which is exactly what applying a
//! rotated value means.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde_json::json;

use super::artifact_registry::EnsureOutcome;
use super::client::{GcpClient, GcpService};
use super::error::{SetupError, SetupResult};

/// Create `secret_id` in `project_id` if it is not already there.
///
/// Automatic replication: the projected Secret is read by workloads in one
/// region, but a user-managed replication policy would pin the secret to a
/// region list this command has no opinion about.
pub async fn ensure_secret(
    client: &GcpClient,
    project_id: &str,
    secret_id: &str,
) -> SetupResult<EnsureOutcome> {
    let response = client
        .post_json(
            GcpService::SecretManager,
            &format!("/v1/projects/{project_id}/secrets?secretId={secret_id}"),
            &json!({ "replication": { "automatic": {} } }),
        )
        .await?;

    match response.status_u16() {
        200..=299 => Ok(EnsureOutcome::Created),
        409 => Ok(EnsureOutcome::AlreadyExists),
        status => Err(SetupError::BadStatus {
            operation: format!("create secret {secret_id} in {project_id}"),
            status,
            body: response.into_text(),
        }),
    }
}

/// Add `value` as a new version of `secret_id` and make it `versions/latest`.
///
/// `value` is base64-encoded into the request body because that is the wire
/// format `SecretPayload.data` requires; it is not a confidentiality measure.
pub async fn add_version(
    client: &GcpClient,
    project_id: &str,
    secret_id: &str,
    value: &[u8],
) -> SetupResult<()> {
    let response = client
        .post_json(
            GcpService::SecretManager,
            &format!("/v1/projects/{project_id}/secrets/{secret_id}:addVersion"),
            &json!({ "payload": { "data": STANDARD.encode(value) } }),
        )
        .await?;

    let status = response.status_u16();
    if (200..=299).contains(&status) {
        return Ok(());
    }
    Err(SetupError::BadStatus {
        // The secret is named; the value it carries is not. A non-2xx body
        // from Secret Manager describes the request, never echoes the payload.
        operation: format!("add a version of {secret_id} in {project_id}"),
        status,
        body: response.into_text(),
    })
}

/// The state of one secret version — `ENABLED`, `DISABLED`, or `DESTROYED` —
/// or `None` when neither the secret nor that version exists.
///
/// **Metadata only, never a payload.** `GET …/versions/{v}` returns the
/// version resource: its name, state, and timestamps. Reading the value needs
/// the separate `:access` call, which this module deliberately does not make —
/// the question a preflight asks is whether the CSI driver will find something
/// to mount, and that is answered by the state alone. Asking it with a payload
/// read would put every projected credential through this process for nothing.
///
/// `version` is whatever the `SecretProviderClass` pinned, normally `latest`.
/// Resolving the alias here rather than assuming it is what makes the check
/// true of the reference the driver will actually request.
pub async fn version_state(
    client: &GcpClient,
    project_id: &str,
    secret_id: &str,
    version: &str,
) -> SetupResult<Option<String>> {
    let response = client
        .get(
            GcpService::SecretManager,
            &format!("/v1/projects/{project_id}/secrets/{secret_id}/versions/{version}"),
        )
        .await?;

    let status = response.status_u16();
    if status == 404 {
        return Ok(None);
    }
    if !(200..=299).contains(&status) {
        return Err(SetupError::BadStatus {
            operation: format!("read the state of {secret_id}/versions/{version} in {project_id}"),
            status,
            body: response.into_text(),
        });
    }
    let body: serde_json::Value =
        serde_json::from_str(&response.into_text()).map_err(|source| SetupError::Json {
            what: "secret version",
            source,
        })?;
    Ok(body
        .get("state")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned))
}

/// Read `secret_id`'s `version` payload — the one call in this module that
/// touches a value rather than metadata.
///
/// `ops secrets apply --check` is the only caller: it needs the live value to
/// compare against the freshly decrypted SOPS value, by constant-time
/// equality, so it can report `differs` before a stale value reaches
/// production. The value returned here must never be logged, printed, or
/// included in an error — only the comparison's `match`/`differs` verdict may
/// leave this process. A missing secret or version is `Ok(None)`, matching
/// [`version_state`]'s convention of treating "not there" as an answer, not a
/// transport failure.
pub async fn access_version(
    client: &GcpClient,
    project_id: &str,
    secret_id: &str,
    version: &str,
) -> SetupResult<Option<Vec<u8>>> {
    let response = client
        .get(
            GcpService::SecretManager,
            &format!("/v1/projects/{project_id}/secrets/{secret_id}/versions/{version}:access"),
        )
        .await?;

    let status = response.status_u16();
    if status == 404 {
        return Ok(None);
    }
    if !(200..=299).contains(&status) {
        return Err(SetupError::BadStatus {
            // Named, not valued: a non-2xx here is Secret Manager's error
            // object, which never echoes payload data back.
            operation: format!("access {secret_id}/versions/{version} in {project_id}"),
            status,
            body: response.into_text(),
        });
    }
    let body: serde_json::Value =
        serde_json::from_str(&response.into_text()).map_err(|source| SetupError::Json {
            what: "secret version payload",
            source,
        })?;
    let Some(data) = body
        .get("payload")
        .and_then(|payload| payload.get("data"))
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(None);
    };
    let value = STANDARD
        .decode(data)
        .map_err(|_| SetupError::Malformed("secret version payload was not valid base64"))?;
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use wiremock::matchers::{body_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::super::client::StaticToken;
    use super::*;

    fn client(server: &MockServer) -> GcpClient {
        GcpClient::new(Arc::new(StaticToken("test-token".into())))
            .with_base_url(GcpService::SecretManager, server.uri())
    }

    #[tokio::test]
    async fn creating_an_existing_secret_is_not_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/projects/neon-law-stg/secrets"))
            .and(query_param("secretId", "SESSION_SECRET"))
            .respond_with(ResponseTemplate::new(409).set_body_string("ALREADY_EXISTS"))
            .mount(&server)
            .await;

        let outcome = ensure_secret(&client(&server), "neon-law-stg", "SESSION_SECRET")
            .await
            .expect("409 converges");
        assert_eq!(outcome, EnsureOutcome::AlreadyExists);
    }

    #[tokio::test]
    async fn a_version_carries_the_value_base64_encoded_in_the_request_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/v1/projects/neon-law-stg/secrets/SESSION_SECRET:addVersion",
            ))
            .and(body_json(json!({
                "payload": { "data": STANDARD.encode(b"do-not-leak-this-value") }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;

        add_version(
            &client(&server),
            "neon-law-stg",
            "SESSION_SECRET",
            b"do-not-leak-this-value",
        )
        .await
        .expect("the version is added");
    }

    #[tokio::test]
    async fn a_refused_version_names_the_secret_but_not_its_value() {
        // The failure path is where a value most easily escapes: the obvious
        // implementation interpolates what it was writing into the message.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(403).set_body_string("PERMISSION_DENIED"))
            .mount(&server)
            .await;

        let error = add_version(
            &client(&server),
            "neon-law-stg",
            "SESSION_SECRET",
            b"do-not-leak-this-value",
        )
        .await
        .expect_err("403 fails the write");

        let rendered = error.to_string();
        assert!(rendered.contains("SESSION_SECRET"));
        assert!(rendered.contains("403"));
        assert!(!rendered.contains("do-not-leak-this-value"));
        assert!(
            !rendered.contains(&STANDARD.encode(b"do-not-leak-this-value")),
            "not the encoded form either"
        );
    }

    #[tokio::test]
    async fn a_resolvable_version_reports_its_state_from_metadata_alone() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(
                "/v1/projects/neon-law-stg/secrets/SESSION_SECRET/versions/latest",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "projects/1/secrets/SESSION_SECRET/versions/2",
                "state": "ENABLED",
            })))
            .mount(&server)
            .await;

        let state = version_state(&client(&server), "neon-law-stg", "SESSION_SECRET", "latest")
            .await
            .expect("the version resolves");
        assert_eq!(state.as_deref(), Some("ENABLED"));
    }

    #[tokio::test]
    async fn a_missing_object_is_absent_rather_than_an_error() {
        // The case the ship preflight exists for: the manifest references an
        // object nothing ever wrote. That is a finding to report by name, not
        // a transport failure to propagate — so the caller can list every
        // missing object at once instead of aborting on the first.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404).set_body_string("NOT_FOUND"))
            .mount(&server)
            .await;

        let state = version_state(
            &client(&server),
            "neon-law",
            "NAVIGATOR_GITHUB_WEBHOOK_SECRET",
            "latest",
        )
        .await
        .expect("a 404 is an answer, not a failure");
        assert_eq!(state, None);
    }

    #[tokio::test]
    async fn access_version_decodes_the_payload() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(
                "/v1/projects/neon-law-stg/secrets/SESSION_SECRET/versions/latest:access",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "projects/1/secrets/SESSION_SECRET/versions/2",
                "payload": { "data": STANDARD.encode(b"the-live-value") },
            })))
            .mount(&server)
            .await;

        let value = access_version(&client(&server), "neon-law-stg", "SESSION_SECRET", "latest")
            .await
            .expect("the version resolves");
        assert_eq!(value.as_deref(), Some(b"the-live-value".as_slice()));
    }

    #[tokio::test]
    async fn access_version_of_a_missing_secret_is_none_not_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404).set_body_string("NOT_FOUND"))
            .mount(&server)
            .await;

        let value = access_version(&client(&server), "neon-law", "MISSING", "latest")
            .await
            .expect("a 404 is an answer, not a failure");
        assert_eq!(value, None);
    }

    #[tokio::test]
    async fn a_refused_access_names_the_secret_but_not_a_value() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(403).set_body_string("PERMISSION_DENIED"))
            .mount(&server)
            .await;

        let error = access_version(&client(&server), "neon-law-stg", "SESSION_SECRET", "latest")
            .await
            .expect_err("403 fails the read");

        let rendered = error.to_string();
        assert!(rendered.contains("SESSION_SECRET"));
        assert!(rendered.contains("403"));
        assert!(!rendered.contains(&STANDARD.encode(b"the-live-value")));
    }

    #[tokio::test]
    async fn a_disabled_version_is_reported_rather_than_read_as_present() {
        // A disabled version still answers the GET, so treating any 2xx as
        // "resolves" would wave through a mount the CSI driver cannot serve.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "state": "DISABLED" })))
            .mount(&server)
            .await;

        let state = version_state(&client(&server), "neon-law", "SESSION_SECRET", "latest")
            .await
            .expect("the version resolves");
        assert_eq!(state.as_deref(), Some("DISABLED"));
    }
}
