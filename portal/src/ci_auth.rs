//! Mint a project-scoped seed session from a GitHub Actions OIDC token.
//!
//! `POST /auth/ci/seed-token` is the CI counterpart of `/auth/cli/start`.
//! A Project repository's job presents GitHub's JWT; this door verifies it,
//! binds the run to the live Project whose `repository_url` is that
//! repository, and returns an HMAC-signed [`SessionData`] scoped to
//! `POST /app/api/seed` for that Project's code.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use store::seed::SeedModel;

use crate::github_oidc::{GitHubActionsClaims, GitHubOidc};
use crate::session::{
    now_unix_secs, random_token_32, SeedScope, SessionData, SessionSource, SessionStore,
    CLI_SESSION_TTL_SECS,
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
        .with_state(state)
}

async fn mint_seed_token(
    State(state): State<CiAuthState>,
    Json(input): Json<MintRequest>,
) -> Response {
    match mint_inner(&state, &input.token).await {
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

async fn mint_inner(state: &CiAuthState, github_token: &str) -> Result<MintResponse, MintError> {
    let Some(host) = state.canonical_host.host() else {
        return Err(MintError::Unavailable(
            "CANONICAL_HOST is required to mint CI seed tokens",
        ));
    };
    let audience = format!("https://{host}");
    let claims = state
        .github_oidc
        .verify(github_token, &audience)
        .await
        .map_err(|error| MintError::Unauthorized(error.to_string()))?;
    authorize_github_run(&claims)?;
    let project = resolve_project(&state.surreal, &claims).await?;
    let actor = lawyer_dri_actor(&state.surreal, &project).await?;
    let exp = now_unix_secs() + CLI_SESSION_TTL_SECS;
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
        impersonation: None,
        scope: Some(SeedScope {
            endpoint: crate::api::SEED_ENDPOINT.to_string(),
            models: SeedModel::ALL.to_vec(),
            project_code: project.code.clone(),
        }),
    };
    let token = state.sessions.encode(&session);
    tracing::info!(
        target: "audit",
        event = "ci.seed_token.minted",
        project_code = %project.code,
        person_id = %actor.id,
        repository = %claims.repository,
        "ci: minted a project-scoped seed token",
    );
    Ok(MintResponse {
        token,
        exp,
        project_code: project.code,
    })
}

fn authorize_github_run(claims: &GitHubActionsClaims) -> Result<(), MintError> {
    if claims.git_ref != "refs/heads/main" {
        return Err(MintError::Forbidden(
            "GitHub Actions OIDC token must be minted for refs/heads/main".into(),
        ));
    }
    if !matches!(claims.event_name.as_str(), "push" | "workflow_dispatch") {
        return Err(MintError::Forbidden(
            "GitHub Actions OIDC token must come from a push or workflow_dispatch on main".into(),
        ));
    }
    Ok(())
}

async fn resolve_project(
    surreal: &store::surreal::SurrealDb,
    claims: &GitHubActionsClaims,
) -> Result<store::projects::Project, MintError> {
    let Some((owner, code)) = claims.repository.split_once('/') else {
        return Err(MintError::Forbidden(
            "GitHub Actions OIDC repository claim is not owner/name".into(),
        ));
    };
    if owner != claims.repository_owner {
        return Err(MintError::Forbidden(
            "GitHub Actions OIDC repository owner does not match repository".into(),
        ));
    }
    let project = store::projects::find_by_code(surreal, code)
        .await
        .map_err(|error| MintError::Internal(error.to_string()))?
        .ok_or_else(|| MintError::Forbidden(format!("no live project carries code {code:?}")))?;
    let Some(url) = project.repository_url.as_deref() else {
        return Err(MintError::Forbidden(
            "the live project has no repository_url; CI cannot bind this run".into(),
        ));
    };
    let Some(named) = github_repository_from_url(url) else {
        return Err(MintError::Forbidden(
            "the live project's repository_url is not a GitHub owner/name URL".into(),
        ));
    };
    if named != claims.repository {
        return Err(MintError::Forbidden(
            "GitHub Actions OIDC repository does not match the live project's repository_url"
                .into(),
        ));
    }
    Ok(project)
}

async fn lawyer_dri_actor(
    surreal: &store::surreal::SurrealDb,
    project: &store::projects::Project,
) -> Result<store::persons::Person, MintError> {
    let people = store::projects::lawyer_dri_people(surreal, project.id)
        .await
        .map_err(|error| MintError::Internal(error.to_string()))?;
    let Some(person) = people
        .into_iter()
        .find(|person| person.role.is_lawyer_tier())
    else {
        return Err(MintError::Forbidden(
            "the live project has no lawyer-tier lawyer DRI to attribute a CI seed write to".into(),
        ));
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
    use super::{authorize_github_run, github_repository_from_url};
    use crate::github_oidc::GitHubActionsClaims;

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
    fn a_pull_request_run_is_not_a_seed_write() {
        let claims = GitHubActionsClaims {
            event_name: "pull_request".into(),
            ..GitHubActionsClaims::default()
        };
        assert!(authorize_github_run(&claims).is_err());
    }

    #[test]
    fn a_non_main_ref_is_not_a_seed_write() {
        let claims = GitHubActionsClaims {
            git_ref: "refs/heads/topic".into(),
            ..GitHubActionsClaims::default()
        };
        assert!(authorize_github_run(&claims).is_err());
    }
}
