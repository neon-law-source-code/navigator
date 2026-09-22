//! `POST /app/projects/{project_code}/notations/draft` (LAW-29) — push one
//! template as a **draft**: stored against the Project, addressable, and
//! explicitly **not run**.
//!
//! The sibling of [`crate::project_notation::create_project_notation`],
//! which this deliberately does **not** call: that function calls
//! `workflows::create_notation_from_repo`, which creates a real
//! [`store::notations::Notation`] row and starts the questionnaire
//! state-machine instance. A draft is a preview artifact, not an
//! executed instrument, so this stops at [`store::notation_drafts::create`]
//! — no Notation, no workflow instance, no `intake_submitted`, no PDF.
//!
//! Matter-scoped the same way `notation create` is: the acting lawyer must
//! participate in the target Project (`store::access::can_see_project_as_lawyer`;
//! admin bypasses), so a miss and an out-of-scope project both read as "not
//! found" rather than disclosing which.
//!
//! The route lives under `/app/projects`, wrapped (with the rest of that
//! router) in `require_auth` + `require_policy` + the bearer-session
//! injector, exactly like `notation create`'s route — so `navigator
//! notations preview`, authenticated the same way `navigator site login`
//! leaves every other CLI command authenticated, can call it. The CSRF
//! layer's bearer exemption (no session cookie → pass through) is what lets
//! a JSON body reach here with no `_csrf` field.

use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};

use crate::admin::AdminState;
use crate::session::SessionData;

/// POST body — the template's own slug and raw Markdown source. Nothing
/// else: the title, the questionnaire, and the workflow are all read back
/// out of `source` itself, the same projection
/// [`crate::notation_preview_doc::from_markdown`] runs for every other
/// caller, so a client cannot claim a title the document itself disagrees
/// with.
#[derive(Debug, Deserialize)]
pub struct NewNotationDraftBody {
    pub slug: String,
    pub source: String,
}

/// The pushed draft's id and where to find it.
#[derive(Debug, Serialize, Deserialize)]
pub struct NotationDraftCreated {
    pub id: uuid::Uuid,
    /// Relative path — the caller (the CLI) already knows its own base URL
    /// and joins the two, so this handler never has to guess its own
    /// public hostname.
    pub path: String,
}

pub async fn create_notation_draft_post(
    State(state): State<AdminState>,
    AxumPath(project_code): AxumPath<String>,
    session: Option<Extension<SessionData>>,
    Json(body): Json<NewNotationDraftBody>,
) -> Response {
    let Some(project_id) = store::projects::id_for_code(&state.surreal, &project_code).await else {
        return (StatusCode::NOT_FOUND, "matter not found").into_response();
    };

    // The route lives under `/app/projects` (require_auth already ran); the
    // command re-checks matter scope from the session, matching
    // `project_notation_new_post`. No session → no scope.
    let (acting_person_id, acting_role) = match session.as_deref() {
        Some(s) => (s.person_id, s.role),
        None => (None, store::persons::Role::Client),
    };
    let in_scope = store::access::can_see_project_as_lawyer(
        &state.surreal,
        acting_person_id,
        acting_role,
        project_id,
    )
    .await
    .unwrap_or(false);
    if !in_scope {
        // Collapses with "matter not found" so this door never discloses a
        // project outside the caller's scope, same as notation create.
        return (StatusCode::NOT_FOUND, "matter not found").into_response();
    }

    let slug = body.slug.trim();
    let source = body.source.trim();
    if slug.is_empty() || source.is_empty() {
        return (StatusCode::BAD_REQUEST, "slug and source are required").into_response();
    }

    // The same projection every other reader of a template's Markdown
    // runs — the title recorded is the document's own, not whatever a
    // caller happened to send.
    let doc = crate::notation_preview_doc::from_markdown(slug, "", source);

    match store::notation_drafts::create(
        &state.surreal,
        &store::notation_drafts::NewNotationDraft {
            project_id,
            slug,
            title: &doc.title,
            source,
        },
    )
    .await
    {
        Ok(draft) => Json(NotationDraftCreated {
            id: draft.id,
            path: format!("/notations/drafts/{}", draft.id),
        })
        .into_response(),
        Err(e) => {
            tracing::error!(error = %e, %project_id, "notation draft create failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin::AdminState;
    use axum::body::to_bytes;
    use axum::routing::post;
    use axum::Router;
    use store::surreal::test_support::mem;
    use store::test_support::seed_project_surreal;

    /// An [`AdminState`] wired against a fresh embedded engine, plus the one
    /// matter to push a draft against. Mirrors `project_documents::tests::fixtures`.
    async fn fixtures(code: &str) -> (AdminState, uuid::Uuid) {
        let db = mem().await;
        let project_id = seed_project_surreal(&db, code).await;
        let app = crate::test_support::app_state(db).await;
        let state = AdminState {
            surreal: app.surreal,
            workflow_runtime: app.workflow_runtime,
            signature_provider: app.signature_provider,
            retainer_intake_questionnaire: workflows::retainer_intake_questionnaire(),
            questionnaire_runtime: app.questionnaire_runtime,
            storage: app.storage,
            assets_storage: app.assets_storage,
            forms_registry: app.forms_registry,
            email: app.email,
            billing_provider: app.billing_provider,
            contract_reviewer: app.contract_reviewer,
            bootstrap_owner_email: app.bootstrap_owner_email,
            on_call_lawyer_email: app.on_call_lawyer_email,
            bootstrap_company: crate::admin::bootstrap_company_from_env(),
            sessions: app.sessions,
            secure_cookies: false,
            attachment_scanner: app.attachment_scanner,
            runtime_kms: app.runtime_kms,
        };
        (state, project_id)
    }

    fn router(state: AdminState) -> Router {
        Router::new()
            .route(
                "/app/projects/{project_code}/notations/draft",
                post(create_notation_draft_post),
            )
            .with_state(state)
    }

    /// An unknown project code refuses before touching the store any
    /// further — the same "not found" shape an out-of-scope project gets,
    /// so the two are indistinguishable from outside.
    #[tokio::test]
    async fn an_unknown_project_code_is_refused() {
        let (state, _project_id) = fixtures("draft-unknown").await;

        let response = tower::ServiceExt::oneshot(
            router(state),
            axum::http::Request::builder()
                .method("POST")
                .uri("/app/projects/no-such-matter/notations/draft")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    "{\"slug\":\"sample-letter\",\"source\":\"Sample body\"}",
                ))
                .expect("request"),
        )
        .await
        .expect("response");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"matter not found");
    }

    /// Blank `slug`/`source` is refused before any store write. Carries an
    /// admin session (which bypasses matter scope, same as
    /// `create_project_notation`) so the request reaches the field
    /// validation rather than being turned away earlier by the scope check.
    #[tokio::test]
    async fn blank_fields_are_refused() {
        let (state, project_id) = fixtures("draft-refuse").await;
        let project = store::projects::find_by_id(&state.surreal, project_id)
            .await
            .expect("read project")
            .expect("project exists");
        let code = project.code.clone();
        let session = crate::session::SessionData::fresh(
            "admin@example.com".to_string(),
            store::persons::Role::Admin,
        );

        let response = tower::ServiceExt::oneshot(
            router(state),
            axum::http::Request::builder()
                .method("POST")
                .uri(format!("/app/projects/{code}/notations/draft"))
                .header("content-type", "application/json")
                .extension(session)
                .body(axum::body::Body::from(r#"{"slug":"","source":""}"#))
                .expect("request"),
        )
        .await
        .expect("response");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    /// No session (no `Authorization: Bearer …`, no cookie) is refused —
    /// with no auth or policy layer mounted in this focused router, the
    /// handler's own `can_see_project_as_lawyer` check is what stands in
    /// for the full `/app/projects` stack's `require_auth`.
    #[tokio::test]
    async fn no_session_is_refused_even_for_a_real_project() {
        let (state, project_id) = fixtures("draft-no-session").await;
        let project = store::projects::find_by_id(&state.surreal, project_id)
            .await
            .expect("read project")
            .expect("project exists");
        let code = project.code.clone();

        let response = tower::ServiceExt::oneshot(
            router(state),
            axum::http::Request::builder()
                .method("POST")
                .uri(format!("/app/projects/{code}/notations/draft"))
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    "{\"slug\":\"sample-letter\",\"source\":\"Sample body\"}",
                ))
                .expect("request"),
        )
        .await
        .expect("response");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    /// The happy path, and the acceptance test for LAW-29's core claim: a
    /// pushed draft is stored and addressable, and creating it touches no
    /// `notation` row — so it starts no workflow instance, since a
    /// state-machine instance is always keyed by `(kind, notation_id)`.
    #[tokio::test]
    async fn pushing_a_draft_creates_no_notation_and_starts_no_workflow() {
        let (state, project_id) = fixtures("draft-happy-path").await;
        let surreal = state.surreal.clone();
        let project = store::projects::find_by_id(&surreal, project_id)
            .await
            .expect("read project")
            .expect("project exists");
        let code = project.code.clone();
        let session = crate::session::SessionData::fresh(
            "admin@example.com".to_string(),
            store::persons::Role::Admin,
        );

        assert!(
            !store::notations::exists_for_project(&surreal, project_id)
                .await
                .expect("exists check"),
            "no notation exists before the draft is pushed"
        );

        let response = tower::ServiceExt::oneshot(
            router(state),
            axum::http::Request::builder()
                .method("POST")
                .uri(format!("/app/projects/{code}/notations/draft"))
                .header("content-type", "application/json")
                .extension(session)
                .body(axum::body::Body::from(
                    "{\"slug\":\"sample-letter\",\"source\":\"---\\ntitle: Sample Letter\\n---\\n\\nBody.\\n\"}",
                ))
                .expect("request"),
        )
        .await
        .expect("response");

        assert_eq!(response.status(), StatusCode::OK, "the draft is created");
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let created: NotationDraftCreated = serde_json::from_slice(&body).expect("json body");
        assert!(
            created.path.starts_with("/notations/drafts/"),
            "{}",
            created.path
        );

        // The whole point: pushing a draft never creates a Notation, and
        // therefore never starts a workflow-machine instance, which is
        // always keyed by `(kind, notation_id)`.
        assert!(
            !store::notations::exists_for_project(&surreal, project_id)
                .await
                .expect("exists check"),
            "pushing a draft must never create a notation row"
        );
        let draft = store::notation_drafts::find_live(&surreal, created.id)
            .await
            .expect("lookup")
            .expect("the pushed draft is stored and addressable");
        assert_eq!(draft.title, "Sample Letter");
    }
}
