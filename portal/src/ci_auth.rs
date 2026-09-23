//! Mint an explicitly scoped CI session from a GitHub Actions OIDC token.
//!
//! `POST /auth/ci/seed-token` and `POST /auth/ci/document-token` are the CI
//! counterparts of `/auth/cli/start`. A Project repository's job presents
//! GitHub's JWT; the door verifies it, binds the run to the live Project
//! whose `repository_url` is that repository, and returns an HMAC-signed
//! [`SessionData`] attributed to that Project's own lawyer DRI. Each door
//! carries an explicit capability: the seed mint can reach its one write
//! endpoint, while the document mint can reach only the metadata reads
//! `navigator project gate --check` uses.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use store::seed::SeedModel;

use crate::github_oidc::{GitHubActionsClaims, GitHubOidc};
use crate::session::{
    now_unix_secs, random_token_32, DocumentScope, SeedScope, SessionData, SessionScope,
    SessionSource, SessionStore, CI_SESSION_TTL_SECS,
};
use crate::CanonicalHost;

/// Shared state for the CI mint door.
#[derive(Clone)]
pub struct CiAuthState {
    pub sessions: SessionStore,
    pub surreal: store::surreal::SurrealDb,
    pub github_oidc: GitHubOidc,
    pub canonical_host: CanonicalHost,
}

#[derive(Debug, Deserialize)]
struct MintRequest {
    token: String,
}

#[derive(Debug, Serialize)]
struct MintResponse {
    token: String,
    exp: i64,
    project_code: String,
}

/// Build the `/auth/ci/*` sub-router.
pub fn routes(state: CiAuthState) -> Router {
    Router::new()
        .route("/auth/ci/seed-token", post(mint_seed_token))
        .route("/auth/ci/document-token", post(mint_document_token))
        .with_state(state)
}

async fn mint_seed_token(
    State(state): State<CiAuthState>,
    Json(input): Json<MintRequest>,
) -> Response {
    match mint_inner(
        &state,
        &input.token,
        "ci.seed_token.minted",
        CiSessionKind::Seed,
    )
    .await
    {
        Ok(body) => (StatusCode::OK, Json(body)).into_response(),
        Err(error) => error.into_response(),
    }
}

/// `POST /auth/ci/document-token` — the `navigator project gate --check --ci`
/// exchange. The minted session is limited to the named Project's lookup,
/// revision metadata, and document-integrity reads.
async fn mint_document_token(
    State(state): State<CiAuthState>,
    Json(input): Json<MintRequest>,
) -> Response {
    match mint_inner(
        &state,
        &input.token,
        "ci.document_token.minted",
        CiSessionKind::Document,
    )
    .await
    {
        Ok(body) => (StatusCode::OK, Json(body)).into_response(),
        Err(error) => error.into_response(),
    }
}

enum MintError {
    Unavailable(&'static str),
    Unauthorized(String),
    Forbidden(String),
    Internal(String),
}

impl IntoResponse for MintError {
    fn into_response(self) -> Response {
        let (status, error, message) = match self {
            Self::Unavailable(message) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                message.to_string(),
            ),
            Self::Unauthorized(message) => (StatusCode::UNAUTHORIZED, "unauthorized", message),
            Self::Forbidden(message) => (StatusCode::FORBIDDEN, "forbidden", message),
            Self::Internal(message) => (StatusCode::INTERNAL_SERVER_ERROR, "internal", message),
        };
        (
            status,
            Json(serde_json::json!({ "error": error, "message": message })),
        )
            .into_response()
    }
}

async fn mint_inner(
    state: &CiAuthState,
    github_token: &str,
    audit_event: &'static str,
    kind: CiSessionKind,
) -> Result<MintResponse, MintError> {
    let Some(host) = state.canonical_host.host() else {
        return Err(MintError::Unavailable(
            "CANONICAL_HOST is required to mint CI tokens",
        ));
    };
    let audience = format!("https://{host}");
    let claims = state
        .github_oidc
        .verify(github_token, &audience)
        .await
        .map_err(|error| MintError::Unauthorized(error.to_string()))?;
    authorize_github_run(&claims)?;
    state
        .github_oidc
        .spend_jti(&claims.jti, claims.exp)
        .map_err(|error| MintError::Unauthorized(error.to_string()))?;
    let project = resolve_project(&state.surreal, &claims).await?;
    let actor = lawyer_dri_actor(&state.surreal, &project, kind).await?;
    let exp = now_unix_secs() + CI_SESSION_TTL_SECS;
    let session = SessionData {
        sub: actor
            .oidc_subject
            .clone()
            .unwrap_or_else(|| actor.id.to_string()),
        email: Some(actor.email.clone()),
        person_id: Some(actor.id),
        exp,
        role: actor.role,
        csrf_token: random_token_32(),
        source: SessionSource::Ci,
        provider: None,
        viewing_as_dri: None,
        scope: Some(match kind {
            CiSessionKind::Seed => SessionScope::Seed(SeedScope {
                endpoint: crate::api::SEED_ENDPOINT.to_string(),
                models: SeedModel::ALL.to_vec(),
                project_code: project.code.clone(),
                dry_run_only: is_pull_request_run(&claims),
            }),
            CiSessionKind::Document => SessionScope::Document(DocumentScope {
                project_id: project.id,
                project_code: project.code.clone(),
            }),
        }),
    };
    let token = state.sessions.encode(&session);
    tracing::info!(
        target: "audit",
        event = audit_event,
        project_code = %project.code,
        person_id = %actor.id,
        repository = %claims.repository,
        "ci: minted a project-scoped session",
    );
    Ok(MintResponse {
        token,
        exp,
        project_code: project.code,
    })
}

fn authorize_github_run(claims: &GitHubActionsClaims) -> Result<(), MintError> {
    if claims.git_ref == "refs/heads/main"
        && matches!(claims.event_name.as_str(), "push" | "workflow_dispatch")
    {
        return Ok(());
    }
    if is_pull_request_run(claims) {
        return Ok(());
    }
    Err(MintError::Forbidden(
        "GitHub Actions OIDC token must come from a push or workflow_dispatch on main, or a pull_request merge ref".into(),
    ))
}

fn is_pull_request_run(claims: &GitHubActionsClaims) -> bool {
    let Some(number) = claims
        .git_ref
        .strip_prefix("refs/pull/")
        .and_then(|reference| reference.strip_suffix("/merge"))
    else {
        return false;
    };
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let Ok(number) = number.parse::<u64>() else {
        return false;
    };
    number > 0
        && claims.event_name == "pull_request"
        && subject_repository(&claims.sub).as_deref() == Some(claims.repository.as_str())
}

/// Extract `owner/name` from a `pull_request` subject claim, accepting both
/// GitHub's classic spelling (`repo:owner/name:pull_request`) and the
/// immutable-subject spelling (`repo:owner@id/name@id:pull_request`) that
/// repositories with immutable subject claims enabled send instead. The
/// subject is parsed field-by-field rather than compared as an opaque
/// string, since the two spellings are never byte-equal for the same repo.
fn subject_repository(sub: &str) -> Option<String> {
    let repo = sub.strip_prefix("repo:")?.strip_suffix(":pull_request")?;
    let (owner, name) = repo.split_once('/')?;
    let owner = owner.split('@').next().filter(|part| !part.is_empty())?;
    let name = name.split('@').next().filter(|part| !part.is_empty())?;
    Some(format!("{owner}/{name}"))
}

/// Shared refusal when this GitHub run cannot be bound to exactly one live
/// Project. The message names nothing about whether a repository or row
/// exists.
const UNBOUND: &str = "this GitHub Actions run is not bound to a live project";

async fn resolve_project(
    surreal: &store::surreal::SurrealDb,
    claims: &GitHubActionsClaims,
) -> Result<store::projects::Project, MintError> {
    let Some((owner, code)) = claims.repository.split_once('/') else {
        return Err(MintError::Forbidden(UNBOUND.into()));
    };
    if owner != claims.repository_owner || code.is_empty() {
        return Err(MintError::Forbidden(UNBOUND.into()));
    }
    let matches: Vec<store::projects::Project> = store::projects::all(surreal)
        .await
        .map_err(|error| MintError::Internal(error.to_string()))?
        .into_iter()
        .filter(|project| {
            project
                .repository_url
                .as_deref()
                .and_then(github_repository_from_url)
                .as_deref()
                == Some(claims.repository.as_str())
        })
        .collect();
    let [project] = matches.as_slice() else {
        return Err(MintError::Forbidden(UNBOUND.into()));
    };
    if project.code != code {
        return Err(MintError::Forbidden(UNBOUND.into()));
    }
    Ok(project.clone())
}

#[derive(Clone, Copy)]
enum CiSessionKind {
    Seed,
    Document,
}

async fn lawyer_dri_actor(
    surreal: &store::surreal::SurrealDb,
    project: &store::projects::Project,
    kind: CiSessionKind,
) -> Result<store::persons::Person, MintError> {
    let people = store::projects::lawyer_dri_people(surreal, project.id)
        .await
        .map_err(|error| MintError::Internal(error.to_string()))?;
    let Some(person) = people
        .into_iter()
        .find(|person| person.role.is_lawyer_tier())
    else {
        let purpose = if matches!(kind, CiSessionKind::Seed) {
            "attribute a CI seed write to"
        } else {
            "attribute CI document verification to"
        };
        return Err(MintError::Forbidden(format!(
            "the live project has no lawyer-tier lawyer DRI to {purpose}"
        )));
    };
    Ok(person)
}

/// Extract `owner/name` from a GitHub HTTPS or SSH remote.
#[must_use]
pub fn github_repository_from_url(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
        .or_else(|| url.strip_prefix("git@github.com:"))?;
    let rest = rest.trim_start_matches('/');
    let mut parts = rest.split('/');
    let owner = parts.next().filter(|part| !part.is_empty())?;
    let name = parts.next().filter(|part| !part.is_empty())?;
    if parts.next().is_some() {
        return None;
    }
    Some(format!("{owner}/{name}"))
}

#[cfg(test)]
mod tests {
    use super::{authorize_github_run, github_repository_from_url, routes, CiAuthState};
    use crate::github_oidc::{GitHubActionsClaims, GitHubOidc};
    use crate::session::{now_unix_secs, SessionStore};
    use crate::CanonicalHost;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[test]
    fn github_https_and_ssh_remotes_yield_owner_name() {
        assert_eq!(
            github_repository_from_url("https://github.com/neon-law-staging/acme.git"),
            Some("neon-law-staging/acme".into())
        );
        assert_eq!(
            github_repository_from_url("git@github.com:neon-law-staging/acme"),
            Some("neon-law-staging/acme".into())
        );
        assert_eq!(
            github_repository_from_url("https://gitlab.example/acme"),
            None
        );
    }

    #[test]
    fn a_pull_request_run_is_authorized_only_for_the_merge_ref_shape() {
        let claims = GitHubActionsClaims {
            sub: "repo:neon-law-staging/acme:pull_request".into(),
            repository: "neon-law-staging/acme".into(),
            repository_owner: "neon-law-staging".into(),
            git_ref: "refs/pull/42/merge".into(),
            event_name: "pull_request".into(),
            ..GitHubActionsClaims::default()
        };
        assert!(authorize_github_run(&claims).is_ok());
    }

    #[test]
    fn a_pull_request_run_with_an_immutable_subject_is_authorized() {
        let claims = GitHubActionsClaims {
            sub: "repo:neon-law-staging@318426496/sample-litigation@1336521864:pull_request".into(),
            repository: "neon-law-staging/sample-litigation".into(),
            repository_owner: "neon-law-staging".into(),
            git_ref: "refs/pull/42/merge".into(),
            event_name: "pull_request".into(),
            ..GitHubActionsClaims::default()
        };
        assert!(authorize_github_run(&claims).is_ok());
    }

    #[test]
    fn a_pull_request_run_rejects_branch_tag_and_untrusted_event_shapes() {
        for (git_ref, event_name, sub) in [
            (
                "refs/heads/main",
                "pull_request",
                "repo:neon-law-staging/acme:pull_request",
            ),
            (
                "refs/tags/v1",
                "pull_request",
                "repo:neon-law-staging/acme:pull_request",
            ),
            (
                "refs/pull/42/merge",
                "pull_request",
                "repo:neon-law-staging/acme:ref:refs/heads/main",
            ),
            (
                "refs/pull/42/merge",
                "pull_request_target",
                "repo:neon-law-staging/acme:pull_request",
            ),
            (
                "refs/pull/42/merge",
                "pull_request",
                "repo:someone-else/other-repo:pull_request",
            ),
            (
                "refs/pull/42/merge",
                "pull_request",
                "repo:someone-else@1/other-repo@2:pull_request",
            ),
        ] {
            let claims = GitHubActionsClaims {
                sub: sub.into(),
                repository: "neon-law-staging/acme".into(),
                repository_owner: "neon-law-staging".into(),
                git_ref: git_ref.into(),
                event_name: event_name.into(),
                ..GitHubActionsClaims::default()
            };
            assert!(
                authorize_github_run(&claims).is_err(),
                "unexpectedly authorized {event_name} {git_ref} {sub}"
            );
        }
    }

    #[test]
    fn a_non_main_ref_is_not_a_seed_write() {
        let claims = GitHubActionsClaims {
            git_ref: "refs/heads/topic".into(),
            ..GitHubActionsClaims::default()
        };
        assert!(authorize_github_run(&claims).is_err());
    }

    /// A live Project bound to `claims.repository`, carrying no lawyer-tier
    /// lawyer DRI — the shared refusal both doors below hit.
    async fn project_with_no_lawyer_dri(
        surreal: &store::surreal::SurrealDb,
        code: &str,
    ) -> store::projects::Project {
        let entity_id = store::test_support::seed_entity(surreal).await;
        let project = store::projects::create(
            surreal,
            &store::projects::NewProject {
                code: code.to_string(),
                name: code.to_string(),
                status: "open".to_string(),
                entity_id,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        store::projects::set_repository_url(
            surreal,
            project.id,
            Some(&format!("https://github.com/neon-law-staging/{code}")),
        )
        .await
        .unwrap();
        project
    }

    fn ci_state(surreal: store::surreal::SurrealDb, repository: &str, jti: &str) -> CiAuthState {
        CiAuthState {
            sessions: SessionStore::new("test-ci-auth-session-key"),
            surreal,
            github_oidc: GitHubOidc::fixed(GitHubActionsClaims {
                repository: repository.to_string(),
                repository_owner: "neon-law-staging".to_string(),
                jti: jti.to_string(),
                exp: now_unix_secs() + 60,
                ..GitHubActionsClaims::default()
            }),
            canonical_host: CanonicalHost::new(Some("staging.neonlaw.com".to_string())),
        }
    }

    async fn mint_request(state: CiAuthState, path: &str) -> (StatusCode, serde_json::Value) {
        let response = routes(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"token":"anything"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn the_seed_door_names_a_ci_seed_write_when_no_lawyer_dri_exists() {
        let surreal = store::test_support::mem_surreal().await;
        project_with_no_lawyer_dri(&surreal, "no-dri-seed").await;
        let state = ci_state(surreal, "neon-law-staging/no-dri-seed", "seed-door-jti");

        let (status, body) = mint_request(state, "/auth/ci/seed-token").await;

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(
            body["message"],
            "the live project has no lawyer-tier lawyer DRI to attribute a CI seed write to"
        );
    }

    #[tokio::test]
    async fn the_document_door_names_verification_not_a_write_when_no_lawyer_dri_exists() {
        let surreal = store::test_support::mem_surreal().await;
        project_with_no_lawyer_dri(&surreal, "no-dri-document").await;
        let state = ci_state(
            surreal,
            "neon-law-staging/no-dri-document",
            "document-door-jti",
        );

        let (status, body) = mint_request(state, "/auth/ci/document-token").await;

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(
            body["message"],
            "the live project has no lawyer-tier lawyer DRI to attribute CI document verification to"
        );
    }
}
