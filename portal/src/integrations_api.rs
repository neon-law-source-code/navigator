//! The `/app/api/integrations/*` doors.
//!
//! Four operations, all admin-tier: ensure or reconcile a Project's
//! Firm-private Notion page, and ensure or notify its Firm-private Slack
//! channel. They are the server half of `navigator site projects notion` and
//! `navigator site projects slack`.
//!
//! ## Why its own noun
//!
//! Not `/app/api/projects/{id}/integrations`. The policy rule for the
//! `projects` prefix admits any authenticated caller up to five segments, so
//! a provisioning path nested there would be policy-reachable by a client
//! even though the handler refuses one — the same reason
//! `project-surfaces`, `project-repositories`, and `project-lifecycle` each
//! carry their own noun.
//!
//! ## Why admin-tier
//!
//! `project-surfaces` is the exact sibling operation — create or adopt a
//! Project's external resources — and it is admin-only. Provisioning a
//! Firm-private channel or page is the same kind of act, and `--all` sweeps
//! every Project the login can see, so the gate matches rather than being
//! loosened for convenience.
//!
//! The gate is [`AdminSession`], an extractor, and not a check in the handler
//! body. Extractors run before the body is deserialized, so a caller outside
//! the tier gets a 403 whatever they sent; a tier check placed after `Json`
//! answers a malformed-body 422 to someone who was never allowed to send a
//! body at all.
//!
//! ## What the response carries
//!
//! An outcome slug per Project, and nothing else. No page id, no channel id,
//! no provider URL. Those are the Firm-private coordinates the rest of this
//! surface exists to keep on the firm side, and an operator running `ensure`
//! needs to know what happened, not where it happened — the coordinate is
//! already recorded on the Project row for the surfaces that may read it.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::api::{AdminSession, ApiError, ApiState};
use crate::integrations::IntegrationError;

/// One Project's result. `outcome` is a closed slug; `detail` is present only
/// for the outcomes that name a mechanism the operator has to act on.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct ProjectOutcome {
    pub(crate) project_code: String,
    pub(crate) outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<String>,
}

impl ProjectOutcome {
    fn plain(project_code: String, outcome: &'static str) -> Self {
        Self {
            project_code,
            outcome,
            detail: None,
        }
    }

    fn with_detail(project_code: String, outcome: &'static str, detail: String) -> Self {
        Self {
            project_code,
            outcome,
            detail: Some(detail),
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct IntegrationReport {
    pub(crate) results: Vec<ProjectOutcome>,
}

/// `{ "project_code": "…", "all": false }`. Exactly one of the two selects
/// the work, so neither a bare body nor both together is accepted: `--all`
/// with a code would leave it ambiguous whether the code narrowed the sweep
/// or was ignored.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProjectSelector {
    #[serde(default)]
    pub(crate) project_code: Option<String>,
    #[serde(default)]
    pub(crate) all: bool,
}

/// `{ "project_code": "…", "event": "project_opened" }`. `event` is absent
/// for `ensure` and required for `notify`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SlackRequest {
    pub(crate) project_code: String,
    #[serde(default)]
    pub(crate) event: Option<String>,
}

fn selector_error() -> ApiError {
    ApiError::MalformedBody("provide either project_code or all, not both".to_string())
}

/// Resolve the selector to the matters this login may act on.
///
/// `--all` means "every Project visible to this login", which is
/// `store::access::visible_projects` — the same scoping the list door uses,
/// so an admin sweep can never reach a matter the read surface would hide.
async fn targets(
    state: &ApiState,
    authed: &AdminSession,
    selector: &ProjectSelector,
) -> Result<Vec<store::projects::Project>, ApiError> {
    let visible =
        store::access::visible_projects(&state.surreal, authed.0.person_id, authed.0.role)
            .await
            .map_err(ApiError::Db)?;
    match (&selector.project_code, selector.all) {
        (Some(code), false) => visible
            .into_iter()
            .find(|project| project.code == *code)
            .map(|project| vec![project])
            .ok_or(ApiError::NotFound),
        (None, true) => Ok(visible),
        _ => Err(selector_error()),
    }
}

fn one_target(
    visible: Vec<store::projects::Project>,
    code: &str,
) -> Result<store::projects::Project, ApiError> {
    visible
        .into_iter()
        .find(|project| project.code == code)
        .ok_or(ApiError::NotFound)
}

async fn visible_for(
    state: &ApiState,
    authed: &AdminSession,
) -> Result<Vec<store::projects::Project>, ApiError> {
    store::access::visible_projects(&state.surreal, authed.0.person_id, authed.0.role)
        .await
        .map_err(ApiError::Db)
}

/// `POST /app/api/integrations/notion/ensure` — create or adopt each
/// selected Project's Firm-private page and record its address.
pub(crate) async fn notion_ensure_door(
    State(state): State<ApiState>,
    authed: AdminSession,
    Json(selector): Json<ProjectSelector>,
) -> Result<Response, ApiError> {
    let mut results = Vec::new();
    for project in targets(&state, &authed, &selector).await? {
        results.push(ensure_one_page(&state, &project).await);
    }
    Ok((StatusCode::OK, Json(IntegrationReport { results })).into_response())
}

async fn ensure_one_page(state: &ApiState, project: &store::projects::Project) -> ProjectOutcome {
    let notion = match state
        .integration_providers
        .notion(&state.surreal, project.id)
        .await
    {
        Ok(notion) => notion,
        Err(error) => return unavailable(project, &error),
    };
    let Ok((page, created)) = cloud::ensure_private_page(notion.as_ref(), &project.code).await
    else {
        return ProjectOutcome::plain(project.code.clone(), "provider_unavailable");
    };
    // Record the address before reporting success. The row is what every
    // later surface reads; an ensure that provisioned a page and lost its
    // address would look done and leave the next run creating another.
    if store::projects::set_private_notion_page_url(&state.surreal, project.id, &page.url)
        .await
        .is_err()
    {
        return ProjectOutcome::plain(project.code.clone(), "address_not_recorded");
    }
    ProjectOutcome::plain(
        project.code.clone(),
        if created { "created" } else { "adopted" },
    )
}

/// `POST /app/api/integrations/notion/reconcile` — compare each selected
/// Project against the environment-selected Notion database and report the
/// repair decision, applying the one decision that is a repair.
pub(crate) async fn notion_reconcile_door(
    State(state): State<ApiState>,
    authed: AdminSession,
    Json(selector): Json<ProjectSelector>,
) -> Result<Response, ApiError> {
    let mut results = Vec::new();
    for project in targets(&state, &authed, &selector).await? {
        results.push(reconcile_one_page(&state, &project).await);
    }
    Ok((StatusCode::OK, Json(IntegrationReport { results })).into_response())
}

async fn reconcile_one_page(
    state: &ApiState,
    project: &store::projects::Project,
) -> ProjectOutcome {
    let notion = match state
        .integration_providers
        .notion(&state.surreal, project.id)
        .await
    {
        Ok(notion) => notion,
        Err(error) => return unavailable(project, &error),
    };
    let Ok(pages) = notion.list_private_pages(&project.code).await else {
        return ProjectOutcome::plain(project.code.clone(), "provider_unavailable");
    };
    let snapshots: Vec<cloud::NotionPageSnapshot> = pages
        .iter()
        .map(|page| cloud::NotionPageSnapshot {
            id: page.id.clone(),
            url: page.url.clone(),
            project_code: page.project_code.clone(),
            person_ids: Vec::new(),
            manual_fields: std::collections::BTreeMap::new(),
            accessible: true,
        })
        .collect();
    // The recorded address is the canonical one. Reconciliation never guesses
    // through a stale URL, so a Project with no recorded page is `missing`
    // even when the provider holds one — adopting it is `ensure`'s decision,
    // not a repair.
    let Some(canonical) = project.private_notion_page_url.clone() else {
        return ProjectOutcome::plain(project.code.clone(), "not_recorded");
    };
    let input = cloud::NotionProjectInput {
        project_code: project.code.clone(),
        canonical_url: canonical,
        person_ids: Vec::new(),
    };
    match cloud::reconcile_notion_project(&input, &snapshots) {
        cloud::NotionRepairDecision::Missing => {
            ProjectOutcome::plain(project.code.clone(), "missing")
        }
        cloud::NotionRepairDecision::Duplicate { page_ids } => ProjectOutcome::with_detail(
            project.code.clone(),
            "duplicate",
            format!("{} pages carry this code", page_ids.len()),
        ),
        cloud::NotionRepairDecision::Conflict { .. } => {
            ProjectOutcome::plain(project.code.clone(), "conflict")
        }
        cloud::NotionRepairDecision::Unavailable { .. } => {
            ProjectOutcome::plain(project.code.clone(), "unavailable")
        }
        cloud::NotionRepairDecision::Unchanged { .. } => {
            ProjectOutcome::plain(project.code.clone(), "unchanged")
        }
        cloud::NotionRepairDecision::Repair { page_id, .. } => {
            match notion.update_private_page(&page_id, &project.code).await {
                Ok(page) => {
                    if store::projects::set_private_notion_page_url(
                        &state.surreal,
                        project.id,
                        &page.url,
                    )
                    .await
                    .is_err()
                    {
                        return ProjectOutcome::plain(project.code.clone(), "address_not_recorded");
                    }
                    ProjectOutcome::plain(project.code.clone(), "repaired")
                }
                Err(_) => ProjectOutcome::plain(project.code.clone(), "provider_unavailable"),
            }
        }
    }
}

/// `POST /app/api/integrations/slack/ensure` — create or adopt one Project's
/// Firm-private channel and record its channel id.
///
/// The invite list is empty by construction. The adapter accepts only
/// provider-issued member ids, Navigator stores none, and it will not turn a
/// participation row or an email address into an invite — so the Firm's own
/// Slack membership governs who joins the channel it just made.
pub(crate) async fn slack_ensure_door(
    State(state): State<ApiState>,
    authed: AdminSession,
    Json(request): Json<SlackRequest>,
) -> Result<Response, ApiError> {
    let visible = visible_for(&state, &authed).await?;
    let project = one_target(visible, &request.project_code)?;
    let slack = match state
        .integration_providers
        .slack(&state.surreal, project.id)
        .await
    {
        Ok(slack) => slack,
        Err(error) => return Ok(report_one(unavailable(&project, &error))),
    };
    let Ok((channel, created)) =
        cloud::ensure_private_channel(slack.as_ref(), &project.code, &[]).await
    else {
        return Ok(report_one(ProjectOutcome::plain(
            project.code.clone(),
            "provider_unavailable",
        )));
    };
    if store::projects::set_internal_slack_channel_id(&state.surreal, project.id, &channel.id)
        .await
        .is_err()
    {
        return Ok(report_one(ProjectOutcome::plain(
            project.code.clone(),
            "address_not_recorded",
        )));
    }
    Ok(report_one(ProjectOutcome::plain(
        project.code.clone(),
        if created { "created" } else { "adopted" },
    )))
}

/// `POST /app/api/integrations/slack/notify` — post one closed-vocabulary,
/// mechanism-only notice to a Project's Firm-private channel.
///
/// An unrecognized event is a 400 before any provider call. The vocabulary is
/// closed precisely so this door cannot become a way to put arbitrary text —
/// a client name, a document title — into a channel.
pub(crate) async fn slack_notify_door(
    State(state): State<ApiState>,
    authed: AdminSession,
    Json(request): Json<SlackRequest>,
) -> Result<Response, ApiError> {
    let event = request
        .event
        .as_deref()
        .and_then(workflows::SlackNoticeEvent::parse)
        .ok_or_else(|| {
            ApiError::MalformedBody(format!(
                "event must be one of {}",
                workflows::SlackNoticeEvent::ALL
                    .map(workflows::SlackNoticeEvent::as_str)
                    .join(", ")
            ))
        })?;
    let visible = visible_for(&state, &authed).await?;
    let project = one_target(visible, &request.project_code)?;
    let slack = match state
        .integration_providers
        .slack(&state.surreal, project.id)
        .await
    {
        Ok(slack) => slack,
        Err(error) => return Ok(report_one(unavailable(&project, &error))),
    };
    // Notify never provisions. A missing channel is reported so an operator
    // runs `ensure` deliberately, rather than a notice quietly creating the
    // Firm-private channel it wanted to post into.
    let Ok(found) = slack.find_private_channel(&project.code).await else {
        return Ok(report_one(ProjectOutcome::plain(
            project.code.clone(),
            "provider_unavailable",
        )));
    };
    let Some(channel) = found else {
        return Ok(report_one(ProjectOutcome::plain(
            project.code.clone(),
            "no_channel",
        )));
    };
    match workflows::post_slack_notice(slack.as_ref(), &channel.id, event).await {
        Ok(()) => Ok(report_one(ProjectOutcome::plain(
            project.code.clone(),
            "notified",
        ))),
        Err(_) => Ok(report_one(ProjectOutcome::plain(
            project.code.clone(),
            "provider_unavailable",
        ))),
    }
}

fn unavailable(project: &store::projects::Project, error: &IntegrationError) -> ProjectOutcome {
    ProjectOutcome::plain(project.code.clone(), error.slug())
}

fn report_one(outcome: ProjectOutcome) -> Response {
    (
        StatusCode::OK,
        Json(IntegrationReport {
            results: vec![outcome],
        }),
    )
        .into_response()
}
