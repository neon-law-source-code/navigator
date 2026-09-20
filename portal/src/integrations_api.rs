//! The `/app/api/integrations/*` doors.
//!
//! Four operations, all admin-tier: ensure or reconcile a Project's
//! Firm-private Notion page, and ensure or notify its Firm-private Slack
//! channel. They are the server half of `navigator project notion` and
//! `navigator project slack`.
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
//! The tier is not the whole gate. Admin tier says the caller provisions
//! external resources at all; `FirmCapability::UseIntegrations` says whose
//! credentials they may spend. Every door resolves that capability against the
//! target Project's owning Firm before a provider is looked up, so an Admin of
//! one Firm cannot reach another Firm's integrations through a Project the
//! visibility lens happens to show them.
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

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, LazyLock, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::api::{AdminSession, ApiError, ApiState};
use crate::integrations::IntegrationError;

/// Per-`(project, provider)` async locks, keyed by
/// [`workflows::IntegrationJob::job_id`] — the same stable id a durable
/// worker would key a replay-safe invocation on, given its first real caller
/// here. Concurrent `ensure` requests for the same Project and provider
/// serialize on this instead of racing find-then-create against each other,
/// which is what turns a double-submitted or double-scheduled `ensure` into
/// two provider resources instead of one.
///
/// This is a single-process guarantee — the boundary a test harness (one
/// `axum::Router` in one process) can actually exercise and assert against.
/// A multi-replica deployment still needs the provider's own idempotent
/// find-then-create (`cloud::ensure_private_page`/`ensure_private_channel`),
/// which every call here already goes through.
static ENSURE_LOCKS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

async fn ensure_lock(job_id: String) -> tokio::sync::OwnedMutexGuard<()> {
    let lock = ENSURE_LOCKS
        .lock()
        .expect("ensure-lock registry poisoned")
        .entry(job_id)
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    lock.lock_owned().await
}

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
/// Two layers, and both are needed. `store::access::visible_projects` is the
/// same scoping the list door uses, so an admin sweep can never reach a matter
/// the read surface would hide. A single-code door then asks
/// [`authorize_project`] whether this caller may spend the owning Firm's
/// integrations. The `--all` path uses the batch counterpart instead, so the
/// capability read and its telemetry happen once per sweep; visibility is not
/// that permission, so a Project only the lens admits is dropped rather than
/// provisioned.
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
        (Some(code), false) => Ok(vec![one_target(state, authed, visible, code).await?]),
        (None, true) => {
            let admitted_firm_ids = store::firm_capability::allowed_firm_ids(
                &state.surreal,
                authed.0.role,
                authed.0.person_id,
                store::firm_capability::FirmCapability::UseIntegrations,
            )
            .await
            .map_err(|error| ApiError::Db(error.to_string()))?;
            let admitted_firm_ids: BTreeSet<_> = admitted_firm_ids.into_iter().collect();
            let visible_project_count = visible.len();
            let authorized: Vec<_> = visible
                .into_iter()
                .filter(|project| {
                    project
                        .firm_id
                        .is_some_and(|firm_id| admitted_firm_ids.contains(&firm_id))
                })
                .collect();
            tracing::info!(
                target: "firm_capability.sweep",
                capability = "use_integrations",
                person_id = authed.0.person_id.map(|id| id.to_string()),
                admitted_firm_count = admitted_firm_ids.len(),
                visible_project_count,
                selected_project_count = authorized.len(),
                "firm capability sweep",
            );
            Ok(authorized)
        }
        _ => Err(selector_error()),
    }
}

async fn one_target(
    state: &ApiState,
    authed: &AdminSession,
    visible: Vec<store::projects::Project>,
    code: &str,
) -> Result<store::projects::Project, ApiError> {
    let project = visible
        .into_iter()
        .find(|project| project.code == code)
        .ok_or(ApiError::NotFound)?;
    authorize_project(state, authed, project).await
}

/// Whether this caller may use the owning Firm's integrations for `project`.
///
/// Runs before any provider lookup, so a refusal costs no credential
/// resolution and no provider call. A Project whose Firm denies the caller and
/// a Project with no owning Firm both collapse to `NotFound`, the same answer a
/// code nobody can see gets: which of the three it was is a Firm-boundary fact
/// the caller is not owed.
async fn authorize_project(
    state: &ApiState,
    authed: &AdminSession,
    project: store::projects::Project,
) -> Result<store::projects::Project, ApiError> {
    let Some(firm_id) = project.firm_id else {
        return Err(ApiError::NotFound);
    };
    let decision = store::firm_capability::resolve(
        &state.surreal,
        authed.0.role,
        authed.0.person_id,
        firm_id,
        store::firm_capability::FirmCapability::UseIntegrations,
    )
    .await
    .map_err(|error| ApiError::Db(error.to_string()))?;
    if decision.is_allowed() {
        Ok(project)
    } else {
        Err(ApiError::NotFound)
    }
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
    let _guard =
        ensure_lock(workflows::IntegrationJob::notion(project.id, &project.code).job_id).await;
    let notion = match state
        .integration_providers
        .notion(&state.surreal, project.id)
        .await
    {
        Ok(notion) => notion,
        Err(error) => return unavailable(project, &error),
    };
    // A recorded page is validated by its own id, never rediscovered by
    // title search: a search cannot see a page after it was renamed away
    // from the recorded title or archived, so treating "the search found
    // nothing" as "nothing was ever provisioned" would create a duplicate
    // beside a page that still exists. Only a genuinely unrecorded or
    // unresolvable coordinate falls through to create-or-adopt below.
    if let Some(recorded_url) = project.private_notion_page_url.clone() {
        if let Some(page_id) = cloud::notion_page_id_from_url(&recorded_url) {
            match notion.get_page(&page_id).await {
                Ok(Some(page)) => {
                    let decision = cloud::reconcile_notion_project(
                        &cloud::NotionProjectInput {
                            project_code: project.code.clone(),
                            canonical_url: recorded_url,
                        },
                        std::slice::from_ref(&cloud::NotionPageSnapshot {
                            accessible: !page.archived,
                            id: page.id,
                            url: page.url,
                            project_code: page.project_code,
                        }),
                    );
                    return apply_notion_decision(
                        state,
                        project,
                        notion.as_ref(),
                        decision,
                        "adopted",
                    )
                    .await;
                }
                Ok(None) => { /* the recorded id is gone; fall through */ }
                Err(_) => {
                    return ProjectOutcome::plain(project.code.clone(), "provider_unavailable")
                }
            }
        }
    }
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

/// Map a reconcile decision to the reported outcome, applying the one
/// decision that is a repair. Shared by `ensure` (which validates a
/// recorded page before falling back to create-or-adopt) and `reconcile`
/// (which validates first via a broad title search, then this same single-id
/// path when that search sees nothing). `unchanged_slug` is the only axis
/// the two doors disagree on: `ensure` calls its steady state `"adopted"`,
/// `reconcile` calls it `"unchanged"`.
async fn apply_notion_decision(
    state: &ApiState,
    project: &store::projects::Project,
    notion: &dyn cloud::NotionService,
    decision: cloud::NotionRepairDecision,
    unchanged_slug: &'static str,
) -> ProjectOutcome {
    match decision {
        // Unreachable from either caller here (both always pass exactly one
        // snapshot), kept exhaustive rather than panicking on a future
        // caller that passes more.
        cloud::NotionRepairDecision::Missing => {
            ProjectOutcome::plain(project.code.clone(), "missing")
        }
        cloud::NotionRepairDecision::Duplicate { page_ids } => ProjectOutcome::with_detail(
            project.code.clone(),
            "duplicate",
            format!("{} pages carry this code", page_ids.len()),
        ),
        cloud::NotionRepairDecision::Conflict { observed_code, .. } => {
            ProjectOutcome::with_detail(project.code.clone(), "renamed", observed_code)
        }
        cloud::NotionRepairDecision::Unavailable { .. } => {
            ProjectOutcome::plain(project.code.clone(), "archived")
        }
        cloud::NotionRepairDecision::Unchanged { .. } => {
            ProjectOutcome::plain(project.code.clone(), unchanged_slug)
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
    // The recorded address is the canonical one. Reconciliation never guesses
    // through a stale URL, so a Project with no recorded page is `not_recorded`
    // even when the provider holds one — adopting it is `ensure`'s decision,
    // not a repair.
    let Some(canonical) = project.private_notion_page_url.clone() else {
        return ProjectOutcome::plain(project.code.clone(), "not_recorded");
    };
    let Ok(pages) = notion.list_private_pages(&project.code).await else {
        return ProjectOutcome::plain(project.code.clone(), "provider_unavailable");
    };
    if !pages.is_empty() {
        let snapshots: Vec<cloud::NotionPageSnapshot> = pages
            .into_iter()
            .map(|page| cloud::NotionPageSnapshot {
                accessible: !page.archived,
                id: page.id,
                url: page.url,
                project_code: page.project_code,
            })
            .collect();
        let input = cloud::NotionProjectInput {
            project_code: project.code.clone(),
            canonical_url: canonical,
        };
        let decision = cloud::reconcile_notion_project(&input, &snapshots);
        return apply_notion_decision(state, project, notion.as_ref(), decision, "unchanged").await;
    }
    // A title search cannot see a page that was renamed away from its
    // recorded title or archived — Notion excludes archived pages from
    // `/search`, and a rename means the title no longer matches the query.
    // Before reporting the recorded page truly missing, look it up by its
    // own id directly.
    let Some(page_id) = cloud::notion_page_id_from_url(&canonical) else {
        return ProjectOutcome::plain(project.code.clone(), "missing");
    };
    match notion.get_page(&page_id).await {
        Ok(Some(page)) => {
            let input = cloud::NotionProjectInput {
                project_code: project.code.clone(),
                canonical_url: canonical,
            };
            let snapshot = cloud::NotionPageSnapshot {
                accessible: !page.archived,
                id: page.id,
                url: page.url,
                project_code: page.project_code,
            };
            let decision = cloud::reconcile_notion_project(&input, std::slice::from_ref(&snapshot));
            apply_notion_decision(state, project, notion.as_ref(), decision, "unchanged").await
        }
        Ok(None) => ProjectOutcome::plain(project.code.clone(), "missing"),
        Err(_) => ProjectOutcome::plain(project.code.clone(), "provider_unavailable"),
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
    let project = one_target(&state, &authed, visible, &request.project_code).await?;
    Ok(report_one(ensure_one_channel(&state, &project).await))
}

async fn ensure_one_channel(
    state: &ApiState,
    project: &store::projects::Project,
) -> ProjectOutcome {
    let _guard =
        ensure_lock(workflows::IntegrationJob::slack(project.id, &project.code).job_id).await;
    let slack = match state
        .integration_providers
        .slack(&state.surreal, project.id)
        .await
    {
        Ok(slack) => slack,
        Err(error) => return unavailable(project, &error),
    };
    // A recorded channel is validated by its own id, never rediscovered by
    // name search: `conversations.list` excludes archived channels and a
    // rename means the name no longer matches the query, so treating "the
    // search found nothing" as "nothing was ever provisioned" would create a
    // duplicate beside a channel that still exists.
    if let Some(recorded_id) = project.internal_slack_channel_id.clone() {
        match slack.get_channel(&recorded_id).await {
            Ok(Some(channel)) if channel.is_archived => {
                return ProjectOutcome::plain(project.code.clone(), "archived")
            }
            Ok(Some(channel)) if channel.name != project.code => {
                return ProjectOutcome::with_detail(project.code.clone(), "renamed", channel.name)
            }
            Ok(Some(_)) => return ProjectOutcome::plain(project.code.clone(), "adopted"),
            Ok(None) => { /* the recorded id is gone; fall through */ }
            Err(_) => return ProjectOutcome::plain(project.code.clone(), "provider_unavailable"),
        }
    }
    let (channel, created) =
        match cloud::ensure_private_channel(slack.as_ref(), &project.code, &[]).await {
            Ok(result) => result,
            // A name collision is reported as its own outcome — an operator
            // decision (something already holds this name) — never folded into
            // the generic provider-outage slug.
            Err(cloud::SlackError::NameTaken) => {
                return ProjectOutcome::plain(project.code.clone(), "conflict")
            }
            Err(_) => return ProjectOutcome::plain(project.code.clone(), "provider_unavailable"),
        };
    // Record the id and its canonical URL together so the two coordinates the
    // Project row carries for this channel never disagree (ENG-807).
    if store::projects::set_internal_slack_channel_id(&state.surreal, project.id, &channel.id)
        .await
        .is_err()
        || store::projects::set_internal_slack_channel_url(
            &state.surreal,
            project.id,
            &cloud::slack_channel_url(&channel.id),
        )
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
    let project = one_target(&state, &authed, visible, &request.project_code).await?;
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
