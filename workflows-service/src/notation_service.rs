//! Restate virtual-object service exposing the two state-machine
//! timelines per Notation.
//!
//! One Restate virtual object per Notation (the object key is the
//! stringified `notation_id`). Hosts both machine kinds under the
//! same key so signals serialize on a single logical journal —
//! questionnaire and workflow handlers can never interleave for
//! the same Notation.
//!
//! Each `*_signal` handler:
//!   1. Reads the stored spec yaml + current state from Restate's
//!      keyed state (`ctx.get`).
//!   2. Computes the next state by parsing the spec and looking
//!      up the transition.
//!   3. Writes the new state back (`ctx.set`).
//!   4. Inside `ctx.run("append-event", …)`, appends one row to
//!      the `notation_events` journal (`SurrealDB`).
//!
//! The `ctx.run` wrapper is the load-bearing trick: it makes the
//! journal write a Restate-journaled side effect, so a replay
//! reuses the cached row id instead of double-writing.

use std::sync::Arc;

use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use tracing::Instrument;
use workflows::{
    dispatch_step, dispatches_side_effect, email::OutboundEmail, EmailService, MachineKind,
    QuestionnaireSpec, StateName, StepDeps, WorkflowSpec,
};

use crate::journal::{answer_payload, append_event, TransitionRecord};

/// Build a handler span parented on the W3C trace context the caller injected
/// into the ingress POST (see `telemetry`), extracted from this invocation's
/// headers. Lets a `web`-initiated workflow and its durable steps render as one
/// trace. Shared by every Notation handler; a no-op parent (fresh root) when no
/// `traceparent` is present (dev / KIND / OSS forks). `key` is the opaque
/// notation id — an allow-listed id, never client content.
fn traced_handler_span(handler: &'static str, headers: &HeaderMap, key: &str) -> tracing::Span {
    let span = tracing::info_span!("notation.handler", handler = handler, key = %key);
    telemetry::set_span_parent(
        &span,
        headers.get("traceparent").map(String::as_str),
        headers.get("tracestate").map(String::as_str),
    );
    span
}

/// Restate state key for the questionnaire spec yaml.
const QUESTIONNAIRE_SPEC_KEY: &str = "questionnaire_spec_yaml";
/// Restate state key for the questionnaire's current state name.
const QUESTIONNAIRE_STATE_KEY: &str = "questionnaire_state";
/// Restate state key for the workflow spec yaml.
const WORKFLOW_SPEC_KEY: &str = "workflow_spec_yaml";
/// Restate state key for the workflow's current state name.
const WORKFLOW_STATE_KEY: &str = "workflow_state";

/// JSON request body for a `*_start` handler.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StartBody {
    pub spec_yaml: String,
    /// `true` for workflows that have no `notations` row (e.g. the
    /// `onboarding__welcome` trigger). The signal handler skips the
    /// `notation_events` journal append for these, since `append_event`
    /// would fail reading back a notation that does not exist. Defaults
    /// to `false` for legacy callers that always start a
    /// notation-backed workflow.
    #[serde(default)]
    pub ephemeral: bool,
}

/// JSON request body for `questionnaire_signal`. The `value`
/// becomes the `answer_value` payload of the journaled event.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QuestionnaireSignalBody {
    pub condition: String,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub acting_person_id: Option<uuid::Uuid>,
}

/// JSON request body for `workflow_signal`. The optional `value`
/// carries a JSON-serialized payload for step dispatch: `email_send__*`
/// reads it as a [`workflows::EmailPayload`] and `generate_pdf__*`
/// reads it as a [`workflows::DocumentPayload`]; other step kinds
/// ignore it. `ephemeral` propagates the flag set at start time so each
/// signal also skips the journal — required because the worker doesn't
/// persist start-side flags across the keyed state today.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorkflowSignalBody {
    pub condition: String,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub acting_person_id: Option<uuid::Uuid>,
    #[serde(default)]
    pub ephemeral: bool,
}

/// JSON response body for a `*_signal` handler.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SignalResponse {
    pub next_state: String,
}

/// JSON response body for `*_current_state`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CurrentStateResponse {
    pub state: Option<String>,
}

/// The two human notifications emitted by a workflow transition.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
enum ReviewNotificationHop {
    Lawyer,
    Client,
}

impl ReviewNotificationHop {
    fn as_str(self) -> &'static str {
        match self {
            Self::Lawyer => "lawyer",
            Self::Client => "client",
        }
    }
}

/// Recipient selection is journaled separately from each send. That freezes
/// the DRI/fallback decision for a transition, while a fallback list can still
/// be sent one recipient at a time with one journal entry per email.
///
/// Opaque ids only. `ctx.run` persists this value in the Restate journal so a
/// replay reuses it, and the journal is an operator debugging surface: a
/// mailbox names a client (`portal::retainer_walk::send_intake`) and a Project
/// code names who retained the firm. Ids freeze the decision just as well, and
/// the address and the code are resolved inside the send step, whose own
/// journaled value is `()`.
#[derive(Debug, Clone, Deserialize, Serialize)]
struct ReviewNotificationPlan {
    project_id: uuid::Uuid,
    recipient_ids: Vec<uuid::Uuid>,
}

/// The wording one notification carries.
///
/// Authored here rather than in `neon/locales/`, because those catalogs are
/// the firm's *page* copy and say so: the shared catalog is exported to the
/// other repository entry by entry, and `neon::locales` asserts that every key
/// it authors is read by a shipped page. Transactional email is read by no
/// page, so a key for it fails that gate. Copy with no catalog belongs in the
/// module that renders it.
struct ReviewCopy {
    subject: &'static str,
    body: &'static str,
}

/// The funnel step caused by one durable workflow transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkflowFunnelStep {
    IntakeComplete,
    ReviewEntered,
    Sent(telemetry::FunnelChannel),
}

fn workflow_funnel_step(
    _from: &StateName,
    _condition: &str,
    next: &StateName,
) -> Option<WorkflowFunnelStep> {
    if next.as_str().starts_with("lawyer_review") {
        return Some(WorkflowFunnelStep::ReviewEntered);
    }
    None
}

async fn record_notation_funnel_step(
    surreal: &store::surreal::SurrealDb,
    notation_id: uuid::Uuid,
    step: WorkflowFunnelStep,
) -> Result<(), HandlerError> {
    let notation = store::notations::find_by_id(surreal, notation_id)
        .await
        .map_err(|e| HandlerError::from(TerminalError::new(format!("funnel notation: {e}"))))?
        .ok_or_else(|| TerminalError::new("funnel notation not found"))?;
    let notation_id = notation_id.to_string();
    let project_id = notation.project_id.to_string();
    let event = match step {
        WorkflowFunnelStep::IntakeComplete => telemetry::FunnelEvent::IntakeComplete {
            notation_id: &notation_id,
            project_id: &project_id,
        },
        WorkflowFunnelStep::ReviewEntered => telemetry::FunnelEvent::ReviewEntered {
            notation_id: &notation_id,
            project_id: &project_id,
        },
        WorkflowFunnelStep::Sent(channel) => telemetry::FunnelEvent::Sent {
            notation_id: &notation_id,
            project_id: &project_id,
            channel,
        },
    };
    telemetry::record_funnel_event(event);
    Ok(())
}

const LAWYER_REVIEW_COPY: ReviewCopy = ReviewCopy {
    subject: "A draft is ready for your review",
    body: "A draft is ready for your review.",
};
const CLIENT_REVIEW_COPY: ReviewCopy = ReviewCopy {
    subject: "Your reviewed draft is ready",
    body: "Your reviewed draft is ready.",
};

/// Service struct held by the Restate endpoint. Carries the shared
/// store handle, the worker-side [`workflows::EmailService`] that
/// `email_send__*` step dispatch routes through, and the
/// [`cloud::StorageService`] that `generate_pdf__*` step dispatch
/// persists rendered PDFs to.
#[derive(Clone)]
pub struct NotationService {
    surreal: store::surreal::SurrealDb,
    email: Arc<dyn EmailService>,
    storage: Arc<dyn cloud::StorageService>,
}

impl NotationService {
    #[must_use]
    pub fn new(
        surreal: store::surreal::SurrealDb,
        email: Arc<dyn EmailService>,
        storage: Arc<dyn cloud::StorageService>,
    ) -> Self {
        Self {
            surreal,
            email,
            storage,
        }
    }
}

/// Parse the Restate object key into a notation id.
fn parse_notation_id(key: &str) -> Result<uuid::Uuid, HandlerError> {
    key.parse::<uuid::Uuid>().map_err(|e| {
        HandlerError::from(TerminalError::new(format!(
            "object key `{key}` is not a valid notation_id: {e}"
        )))
    })
}

#[restate_sdk::object(name = "notation")]
impl NotationService {
    /// Begin the questionnaire walk for this Notation. Idempotent
    /// — if the spec is already stored, this is a no-op.
    #[restate_sdk::handler]
    async fn questionnaire_start(
        &self,
        ctx: ObjectContext<'_>,
        body: Json<StartBody>,
    ) -> Result<(), HandlerError> {
        let span = traced_handler_span("questionnaire_start", ctx.headers(), ctx.key());
        async move {
            let body = body.0;
            if ctx.get::<String>(QUESTIONNAIRE_SPEC_KEY).await?.is_some() {
                return Ok(()); // idempotent
            }
            // Validate the yaml round-trips before storing so a later
            // signal can't fail mid-flight.
            QuestionnaireSpec::from_yaml(&body.spec_yaml)
                .map_err(|e| TerminalError::new(format!("questionnaire spec: {e}")))?;
            ctx.set(QUESTIONNAIRE_SPEC_KEY, body.spec_yaml);
            ctx.set(QUESTIONNAIRE_STATE_KEY, StateName::BEGIN.to_string());
            Ok(())
        }
        .instrument(span)
        .await
    }

    /// Advance the questionnaire one step.
    #[restate_sdk::handler]
    async fn questionnaire_signal(
        &self,
        ctx: ObjectContext<'_>,
        body: Json<QuestionnaireSignalBody>,
    ) -> Result<Json<SignalResponse>, HandlerError> {
        let span = traced_handler_span("questionnaire_signal", ctx.headers(), ctx.key());
        async move {
            let body = body.0;
            let notation_id = parse_notation_id(ctx.key())?;
            let spec_yaml: String = ctx
                .get::<String>(QUESTIONNAIRE_SPEC_KEY)
                .await?
                .ok_or_else(|| TerminalError::new("questionnaire has not been started"))?;
            let spec = QuestionnaireSpec::from_yaml(&spec_yaml)
                .map_err(|e| TerminalError::new(format!("spec: {e}")))?;
            let from = ctx
                .get::<String>(QUESTIONNAIRE_STATE_KEY)
                .await?
                .unwrap_or_else(|| StateName::BEGIN.to_string());
            let from_state = StateName::from(from.as_str());
            let next = next_state(spec.inner(), &from_state, &body.condition)?;

            ctx.set(QUESTIONNAIRE_STATE_KEY, next.as_str().to_string());

            let surreal = self.surreal.clone();
            let payload = body.value.as_deref().map(answer_payload);
            let from_str = from.clone();
            let to_str = next.as_str().to_string();
            let condition = body.condition.clone();
            ctx.run(|| async move {
                let recorded_at = chrono::Utc::now().to_rfc3339();
                append_event(
                    &surreal,
                    TransitionRecord {
                        notation_id,
                        acting_person_id: body.acting_person_id,
                        machine_kind: MachineKind::Questionnaire.as_str(),
                        from_state: &from_str,
                        to_state: &to_str,
                        condition: &condition,
                        payload_json: payload,
                        recorded_at: &recorded_at,
                    },
                )
                .await
                .map(|_| ())
                .map_err(|e| HandlerError::from(TerminalError::new(format!("journal: {e}"))))
            })
            .name("append-questionnaire-event")
            .await?;

            if next == StateName::end() {
                let surreal = self.surreal.clone();
                ctx.run(move || async move {
                    record_notation_funnel_step(
                        &surreal,
                        notation_id,
                        WorkflowFunnelStep::IntakeComplete,
                    )
                    .await
                })
                .name("record-funnel-intake-complete")
                .await?;
            }

            Ok(Json(SignalResponse {
                next_state: next.as_str().to_string(),
            }))
        }
        .instrument(span)
        .await
    }

    #[restate_sdk::handler]
    async fn questionnaire_current_state(
        &self,
        ctx: SharedObjectContext<'_>,
    ) -> Result<Json<CurrentStateResponse>, HandlerError> {
        let span = traced_handler_span("questionnaire_current_state", ctx.headers(), ctx.key());
        async move {
            let state = ctx.get::<String>(QUESTIONNAIRE_STATE_KEY).await?;
            Ok(Json(CurrentStateResponse { state }))
        }
        .instrument(span)
        .await
    }

    /// Begin the post-intake workflow for this Notation. Idempotent.
    #[restate_sdk::handler]
    async fn workflow_start(
        &self,
        ctx: ObjectContext<'_>,
        body: Json<StartBody>,
    ) -> Result<(), HandlerError> {
        let span = traced_handler_span("workflow_start", ctx.headers(), ctx.key());
        async move {
            let body = body.0;
            if ctx.get::<String>(WORKFLOW_SPEC_KEY).await?.is_some() {
                return Ok(()); // idempotent
            }
            WorkflowSpec::from_yaml(&body.spec_yaml)
                .map_err(|e| TerminalError::new(format!("workflow spec: {e}")))?;
            ctx.set(WORKFLOW_SPEC_KEY, body.spec_yaml);
            ctx.set(WORKFLOW_STATE_KEY, StateName::BEGIN.to_string());
            Ok(())
        }
        .instrument(span)
        .await
    }

    /// Advance the workflow one step.
    #[restate_sdk::handler]
    #[allow(clippy::too_many_lines)]
    async fn workflow_signal(
        &self,
        ctx: ObjectContext<'_>,
        body: Json<WorkflowSignalBody>,
    ) -> Result<Json<SignalResponse>, HandlerError> {
        let span = traced_handler_span("workflow_signal", ctx.headers(), ctx.key());
        async move {
            let body = body.0;
            let notation_id = parse_notation_id(ctx.key())?;
            let spec_yaml: String = ctx
                .get::<String>(WORKFLOW_SPEC_KEY)
                .await?
                .ok_or_else(|| TerminalError::new("workflow has not been started"))?;
            let spec = WorkflowSpec::from_yaml(&spec_yaml)
                .map_err(|e| TerminalError::new(format!("spec: {e}")))?;
            let from = ctx
                .get::<String>(WORKFLOW_STATE_KEY)
                .await?
                .unwrap_or_else(|| StateName::BEGIN.to_string());
            let from_state = StateName::from(from.as_str());
            let next = next_state(&spec, &from_state, &body.condition)?;

            ctx.set(WORKFLOW_STATE_KEY, next.as_str().to_string());

            // Step-kind side effects, routed through the one shared
            // `workflows::dispatch_step` registry so the prod worker and the
            // in-process dev/BDD runtime run the exact same dispatch arm —
            // `email_send__*` (SendGrid), `generate_pdf__*` (render + GCS
            // persist + blob/`documents` file), and the submission steps
            // `mailroom_send` / `certified_mail` / `e_filing` / `filing__*`
            // (a durable `filings` row, guaranteed `lawyer_review`-gated by
            // the spec).
            //
            // The side effect runs BEFORE the journal append so a step that
            // produces a durable artifact (the `generate_pdf` blob ref) can
            // record it on the transition's own `notation_events.payload` —
            // set at append time, honoring the append-only journal (rows are
            // never updated). The `ctx.run` boundary stays here, outside the
            // registry, so the effect is journaled and a replay reuses the
            // cached outcome rather than re-emailing / double-filing /
            // double-writing. We only open the journal entry when the step
            // actually dispatches, so human/wait states (`lawyer_review`,
            // `_signature`, …) stay dispatch-free as before.
            let dispatch_payload: Option<String> = if dispatches_side_effect(&next) {
                let deps = StepDeps::new(Arc::clone(&self.email), Arc::clone(&self.storage))
                    // `assets` and `templates` moved to SurrealDB with ENG-121:
                    // `document_intake` files its row and `generate_pdf` reads
                    // the pinned template's declared kind through this handle.
                    .with_surreal(self.surreal.clone());
                let state = next.clone();
                let value = body.value.clone();
                ctx.run(|| async move {
                    dispatch_workflow_step(&deps, notation_id, &state, value.as_deref()).await
                })
                .name("dispatch-step")
                .await?
            } else {
                None
            };

            // Ephemeral workflows (e.g. `onboarding__welcome`) have no
            // `notations` row, so `append_event`'s read-back of the
            // notation (`notations` moved to SurrealDB with ENG-121) would
            // fail. Skip the journal in that case; the durability for
            // ephemeral steps lives downstream (the `sent_emails` audit row
            // for `email_send__*` dispatch).
            if !body.ephemeral {
                let surreal = self.surreal.clone();
                let from_str = from.clone();
                let to_str = next.as_str().to_string();
                let condition = body.condition.clone();
                let acting_person_id = body.acting_person_id;
                ctx.run(|| async move {
                    let recorded_at = chrono::Utc::now().to_rfc3339();
                    append_event(
                        &surreal,
                        TransitionRecord {
                            notation_id,
                            acting_person_id,
                            machine_kind: MachineKind::Workflow.as_str(),
                            from_state: &from_str,
                            to_state: &to_str,
                            condition: &condition,
                            payload_json: dispatch_payload,
                            recorded_at: &recorded_at,
                        },
                    )
                    .await
                    .map(|_| ())
                    .map_err(|e| HandlerError::from(TerminalError::new(format!("journal: {e}"))))
                })
                .name("append-workflow-event")
                .await?;
            }

            if !body.ephemeral {
                if let Some(step) = workflow_funnel_step(&from_state, &body.condition, &next) {
                    let surreal = self.surreal.clone();
                    ctx.run(move || async move {
                        record_notation_funnel_step(&surreal, notation_id, step).await
                    })
                    .name("record-funnel-step")
                    .await?;
                }
            }

            // A notification follows the record of the transition it announces.
            // The only reason anything precedes `append-workflow-event` is a
            // step that produces a payload for that row, and a notification
            // produces none — running it first meant a mail-provider failure
            // left the transition unjournaled.
            if !body.ephemeral {
                if let Some(hop) = review_notification_hop(&from_state, &body.condition, &next) {
                    let surreal = self.surreal.clone();
                    let plan = ctx
                        .run(move || async move {
                            resolve_review_notification_plan(&surreal, notation_id, hop)
                                .await
                                .map(Json)
                        })
                        .name("resolve-review-notification-recipients")
                        .await?
                        .into_inner();
                    let record_client_handoff = matches!(hop, ReviewNotificationHop::Client)
                        && !plan.recipient_ids.is_empty();

                    for recipient_id in plan.recipient_ids.iter().copied() {
                        let email = Arc::clone(&self.email);
                        let surreal = self.surreal.clone();
                        let project_id = plan.project_id;
                        // The mailbox and the Project code are read inside the
                        // send step, never carried across it: this run's
                        // journaled value is `()`, so neither reaches the
                        // journal.
                        ctx.run(move || async move {
                            let target =
                                review_notification_target(&surreal, project_id, recipient_id)
                                    .await?;
                            send_review_notification(
                                email,
                                notation_id,
                                project_id,
                                &target.project_code,
                                recipient_id,
                                &target.address,
                                hop,
                            )
                            .await
                        })
                        .name("send-review-notification")
                        .await?;
                    }

                    if record_client_handoff {
                        let surreal = self.surreal.clone();
                        ctx.run(move || async move {
                            record_notation_funnel_step(
                                &surreal,
                                notation_id,
                                WorkflowFunnelStep::Sent(telemetry::FunnelChannel::Email),
                            )
                            .await
                        })
                        .name("record-funnel-sent")
                        .await?;
                    }
                }
            }

            // The firm signing the closing letter (`firm_signature__*`)
            // closes the matter: flip the bound Project `open` → `closed`.
            // The symmetric bookend to the client-signed retainer that
            // opened it. Journaled so a replay reuses the outcome rather
            // than re-updating; skipped for ephemeral (notation-less)
            // workflows, which never carry a firm-signature step.
            if !body.ephemeral && workflows::closes_matter(&from_state) {
                let surreal = self.surreal.clone();
                ctx.run(move || async move {
                    workflows::close_matter(&surreal, notation_id)
                        .await
                        .map_err(|e| HandlerError::from(TerminalError::new(format!("close: {e}"))))
                })
                .name("close-matter")
                .await?;
            }

            Ok(Json(SignalResponse {
                next_state: next.as_str().to_string(),
            }))
        }
        .instrument(span)
        .await
    }

    #[restate_sdk::handler]
    async fn workflow_current_state(
        &self,
        ctx: SharedObjectContext<'_>,
    ) -> Result<Json<CurrentStateResponse>, HandlerError> {
        let span = traced_handler_span("workflow_current_state", ctx.headers(), ctx.key());
        async move {
            let state = ctx.get::<String>(WORKFLOW_STATE_KEY).await?;
            Ok(Json(CurrentStateResponse { state }))
        }
        .instrument(span)
        .await
    }
}

/// Pure state-machine step: given a spec, a current state, and a
/// condition, return the next state name. Surface as a
/// `TerminalError` (caller-visible failure, not Restate-retryable)
/// on `AlreadyTerminal`, `NoTransition`, or `UnknownState`, since
/// those are deterministic outcomes that a replay would reproduce.
fn next_state(
    spec: &WorkflowSpec,
    from: &StateName,
    condition: &str,
) -> Result<StateName, HandlerError> {
    if spec.is_terminal(from) {
        return Err(TerminalError::new(format!("`{}` is terminal", from.as_str())).into());
    }
    let transitions = spec.transitions_from(from).ok_or_else(|| {
        HandlerError::from(TerminalError::new(format!(
            "unknown state `{}`",
            from.as_str()
        )))
    })?;
    transitions.lookup(condition).cloned().ok_or_else(|| {
        HandlerError::from(TerminalError::new(format!(
            "no transition from `{}` on condition `{condition}`",
            from.as_str()
        )))
    })
}

async fn dispatch_workflow_step(
    deps: &StepDeps,
    notation_id: uuid::Uuid,
    next: &StateName,
    payload: Option<&str>,
) -> Result<Option<String>, HandlerError> {
    if !dispatches_side_effect(next) {
        return Ok(None);
    }
    dispatch_step(deps, notation_id, next, payload)
        .await
        .map_err(|e| HandlerError::from(TerminalError::new(format!("dispatch: {e}"))))
}

fn review_notification_hop(
    from: &StateName,
    condition: &str,
    next: &StateName,
) -> Option<ReviewNotificationHop> {
    if next.as_str().starts_with("lawyer_review") {
        return Some(ReviewNotificationHop::Lawyer);
    }
    if from.as_str().starts_with("lawyer_review")
        && !next.as_str().starts_with("reask__")
        && matches!(condition, "approved" | "_")
    {
        return Some(ReviewNotificationHop::Client);
    }
    None
}

async fn resolve_review_notification_plan(
    surreal: &store::surreal::SurrealDb,
    notation_id: uuid::Uuid,
    hop: ReviewNotificationHop,
) -> Result<ReviewNotificationPlan, HandlerError> {
    let notation = store::notations::find_by_id(surreal, notation_id)
        .await
        .map_err(|e| {
            HandlerError::from(TerminalError::new(format!(
                "review notification notation: {e}"
            )))
        })?
        .ok_or_else(|| TerminalError::new("review notification notation not found"))?;
    let project = store::projects::find_by_id(surreal, notation.project_id)
        .await
        .map_err(|e| {
            HandlerError::from(TerminalError::new(format!(
                "review notification project: {e}"
            )))
        })?
        .ok_or_else(|| TerminalError::new("review notification project not found"))?;
    let rows = store::projects::participations_for_project(surreal, project.id)
        .await
        .map_err(|e| {
            HandlerError::from(TerminalError::new(format!(
                "review notification participants: {e}"
            )))
        })?;
    let candidate_ids = review_recipient_ids(&rows, hop);
    let mut candidates = Vec::new();
    for person_id in candidate_ids {
        if let Some(person) = store::persons::find_by_id(surreal, person_id)
            .await
            .map_err(|e| {
                HandlerError::from(TerminalError::new(format!(
                    "review notification person: {e}"
                )))
            })?
        {
            candidates.push(person);
        }
    }
    let recipient_ids: Vec<uuid::Uuid> = match hop {
        ReviewNotificationHop::Lawyer => candidates
            .into_iter()
            .filter(|person| person.role.is_lawyer_tier())
            .map(|person| person.id)
            .collect(),
        ReviewNotificationHop::Client => candidates
            .into_iter()
            .next()
            .map(|person| person.id)
            .into_iter()
            .collect(),
    };
    let recipient_ids = if matches!(hop, ReviewNotificationHop::Lawyer) && recipient_ids.is_empty()
    {
        let mut fallback = Vec::new();
        for person_id in review_fallback_ids(&rows) {
            if let Some(person) = store::persons::find_by_id(surreal, person_id)
                .await
                .map_err(|e| {
                    HandlerError::from(TerminalError::new(format!(
                        "review notification fallback person: {e}"
                    )))
                })?
            {
                if person.role.is_lawyer_tier() {
                    fallback.push(person.id);
                }
            }
        }
        fallback
    } else {
        recipient_ids
    };

    if recipient_ids.is_empty() {
        tracing::warn!(
            target: "audit",
            audit = true,
            notation_id = %notation_id,
            project_id = %project.id,
            recipient_role = "none",
            hop = hop.as_str(),
            outcome = "skipped_no_recipient",
            "review notification skipped: no recipient"
        );
    }

    Ok(ReviewNotificationPlan {
        project_id: project.id,
        recipient_ids,
    })
}

/// What one send needs that the journaled plan deliberately does not carry.
struct ReviewNotificationTarget {
    address: String,
    project_code: String,
}

/// Read the recipient's mailbox and the matter's code at send time.
///
/// Both are client identifiers, so they are resolved here rather than frozen
/// into [`ReviewNotificationPlan`]: this runs inside the send step, whose
/// journaled value is `()`.
async fn review_notification_target(
    surreal: &store::surreal::SurrealDb,
    project_id: uuid::Uuid,
    recipient_id: uuid::Uuid,
) -> Result<ReviewNotificationTarget, HandlerError> {
    let person = store::persons::find_by_id(surreal, recipient_id)
        .await
        .map_err(|e| {
            HandlerError::from(TerminalError::new(format!(
                "review notification recipient: {e}"
            )))
        })?
        .ok_or_else(|| TerminalError::new("review notification recipient not found"))?;
    let project = store::projects::find_by_id(surreal, project_id)
        .await
        .map_err(|e| {
            HandlerError::from(TerminalError::new(format!(
                "review notification project: {e}"
            )))
        })?
        .ok_or_else(|| TerminalError::new("review notification project not found"))?;
    Ok(ReviewNotificationTarget {
        address: person.email,
        project_code: project.code,
    })
}

fn review_recipient_ids(
    rows: &[store::projects::PersonProjectRole],
    hop: ReviewNotificationHop,
) -> Vec<uuid::Uuid> {
    match hop {
        ReviewNotificationHop::Lawyer => {
            let eligible = rows.iter().filter(|row| {
                !store::projects::PARTICIPATION_CLIENT_SIDE.contains(&row.participation.as_str())
            });
            // The role tier is resolved after the row query. A DRI marker is
            // the first choice; person lookup rejects stale links and filters
            // the result to the licensed lawyer tiers.
            let dris: Vec<uuid::Uuid> = eligible
                .clone()
                .filter(|row| row.is_lawyer_dri)
                .map(|row| row.person_id)
                .collect();
            if dris.is_empty() {
                eligible.map(|row| row.person_id).collect()
            } else {
                dris
            }
        }
        ReviewNotificationHop::Client => rows
            .iter()
            .filter(|row| row.participation == "client")
            .find(|row| row.is_client_dri)
            .or_else(|| rows.iter().find(|row| row.participation == "client"))
            .map(|row| vec![row.person_id])
            .unwrap_or_default(),
    }
}

fn review_fallback_ids(rows: &[store::projects::PersonProjectRole]) -> Vec<uuid::Uuid> {
    rows.iter()
        .filter(|row| {
            !store::projects::PARTICIPATION_CLIENT_SIDE.contains(&row.participation.as_str())
        })
        .map(|row| row.person_id)
        .collect()
}

async fn send_review_notification(
    email: Arc<dyn EmailService>,
    notation_id: uuid::Uuid,
    project_id: uuid::Uuid,
    project_code: &str,
    recipient_id: uuid::Uuid,
    recipient_email: &str,
    hop: ReviewNotificationHop,
) -> Result<(), HandlerError> {
    let copy = review_copy(hop);
    let base_url = workflows::email::base_url_from_env();
    let path = match hop {
        ReviewNotificationHop::Lawyer => {
            format!("/app/lawyer/notations/{notation_id}/review")
        }
        ReviewNotificationHop::Client => format!("/app/projects/{project_code}"),
    };
    let body = format!(
        "{}\n\n{}{}",
        copy.body,
        base_url.trim_end_matches('/'),
        path
    );
    // Every person-addressed Navigator email carries the inline-styled firm
    // layout as its `text/html` alternative beside the plain part, so a rich
    // client shows the wordmark rather than a bare URL.
    let html = workflows::email::render_email_html(&body, &base_url);
    let outbound = OutboundEmail::new(recipient_email, copy.subject, body)
        .with_html(html)
        .with_template(format!("notation-review-{}", hop.as_str()))
        .with_person(recipient_id.to_string());
    match email.send(outbound).await {
        Ok(_) => {
            tracing::info!(
                target: "audit",
                audit = true,
                notation_id = %notation_id,
                project_id = %project_id,
                recipient_role = hop.as_str(),
                hop = hop.as_str(),
                outcome = "sent",
                "review notification sent"
            );
            Ok(())
        }
        Err(error) => {
            // The variant, never the message: `EmailError::InvalidRecipient`
            // carries the address it refused, and telemetry leaves the firm's
            // trust boundary. The two words still separate a bad mailbox from
            // a provider outage, which is the whole diagnostic question, and
            // they ride the handler error rather than a new audit field — the
            // collector's allow-list is fail-closed, so a key added here would
            // export as a blank until it is allow-listed too.
            let reason = match error {
                workflows::email::EmailError::InvalidRecipient(_) => "invalid recipient",
                workflows::email::EmailError::Transport(_) => "transport",
            };
            tracing::info!(
                target: "audit",
                audit = true,
                notation_id = %notation_id,
                project_id = %project_id,
                recipient_role = hop.as_str(),
                hop = hop.as_str(),
                outcome = "failed",
                "review notification failed"
            );
            Err(TerminalError::new(format!("review notification send failed: {reason}")).into())
        }
    }
}

const fn review_copy(hop: ReviewNotificationHop) -> ReviewCopy {
    match hop {
        ReviewNotificationHop::Lawyer => LAWYER_REVIEW_COPY,
        ReviewNotificationHop::Client => CLIENT_REVIEW_COPY,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        dispatch_workflow_step, next_state, review_copy, review_notification_hop,
        review_recipient_ids, send_review_notification, ReviewNotificationHop,
    };
    use std::sync::Arc;
    use workflows::{CapturingEmail, EmailPayload, StateName, StepDeps, WorkflowSpec};

    const SPEC: &str = r"
BEGIN:
  _: client_name
client_name:
  _: client_email
client_email:
  _: END
END: {}
";

    #[test]
    fn next_state_advances_through_a_questionnaire_walk() {
        let spec = WorkflowSpec::from_yaml(SPEC).unwrap();
        let n = next_state(&spec, &StateName::begin(), "_").unwrap();
        assert_eq!(n.as_str(), "client_name");
        let n = next_state(&spec, &n, "_").unwrap();
        assert_eq!(n.as_str(), "client_email");
        let n = next_state(&spec, &n, "_").unwrap();
        assert_eq!(n, StateName::end());
    }

    #[test]
    fn next_state_returns_terminal_error_on_already_end() {
        let spec = WorkflowSpec::from_yaml(SPEC).unwrap();
        let err = next_state(&spec, &StateName::end(), "_").unwrap_err();
        // HandlerError implements Debug only; format that for the
        // message assertion.
        assert!(format!("{err:?}").contains("terminal"));
    }

    #[test]
    fn next_state_returns_terminal_error_on_unknown_condition() {
        let spec = WorkflowSpec::from_yaml(SPEC).unwrap();
        let err = next_state(&spec, &StateName::begin(), "bogus").unwrap_err();
        assert!(format!("{err:?}").contains("no transition"));
    }

    #[test]
    fn workflow_transition_identifies_review_entry_without_premature_send() {
        assert!(matches!(
            super::workflow_funnel_step(
                &StateName::from("intake_persisted__client"),
                "rendered",
                &StateName::from("lawyer_review")
            ),
            Some(super::WorkflowFunnelStep::ReviewEntered)
        ));
        assert!(super::workflow_funnel_step(
            &StateName::from("lawyer_review"),
            "approved",
            &StateName::from("generate_pdf__draft")
        )
        .is_none());
        assert!(super::workflow_funnel_step(
            &StateName::from("generate_pdf__draft"),
            "pdf_persisted",
            &StateName::from("sent_for_signature__pending")
        )
        .is_none());
        assert!(super::workflow_funnel_step(
            &StateName::from("lawyer_review"),
            "changes_requested",
            &StateName::from("reask__client")
        )
        .is_none());
    }

    #[tokio::test]
    async fn notation_funnel_events_carry_ids_without_client_content() {
        use std::io::Write;
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::fmt::MakeWriter;

        #[derive(Clone)]
        struct Buffer(Arc<Mutex<Vec<u8>>>);

        impl Write for Buffer {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0
                    .lock()
                    .expect("capture lock")
                    .extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        impl<'a> MakeWriter<'a> for Buffer {
            type Writer = Buffer;

            fn make_writer(&'a self) -> Self::Writer {
                self.clone()
            }
        }

        let output = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_ansi(false)
            .with_writer(Buffer(output.clone()))
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        let surreal = store::test_support::mem_surreal().await;
        let notation_id = store::test_support::seed_notation(&surreal).await;
        let project_id = store::notations::find_by_id(&surreal, notation_id)
            .await
            .expect("notation lookup")
            .expect("seeded notation")
            .project_id;

        super::record_notation_funnel_step(
            &surreal,
            notation_id,
            super::WorkflowFunnelStep::IntakeComplete,
        )
        .await
        .expect("intake funnel event");
        super::record_notation_funnel_step(
            &surreal,
            notation_id,
            super::WorkflowFunnelStep::ReviewEntered,
        )
        .await
        .expect("review funnel event");
        super::record_notation_funnel_step(
            &surreal,
            notation_id,
            super::WorkflowFunnelStep::Sent(telemetry::FunnelChannel::Email),
        )
        .await
        .expect("sent funnel event");

        let rendered = String::from_utf8(output.lock().expect("capture lock").clone())
            .expect("capture is UTF-8");
        let funnel_lines: Vec<_> = rendered
            .lines()
            .filter(|line| line.contains(r#""target":"funnel""#))
            .collect();
        assert_eq!(funnel_lines.len(), 3, "funnel: {rendered}");
        let funnel = funnel_lines.join("\n");
        assert!(
            funnel.contains("funnel.intake_complete"),
            "funnel: {funnel}"
        );
        assert!(funnel.contains("funnel.review_entered"), "funnel: {funnel}");
        assert!(funnel.contains("funnel.sent"), "funnel: {funnel}");
        assert!(funnel.contains(r#""channel":"email""#), "funnel: {funnel}");
        assert!(
            funnel.contains(&notation_id.to_string()),
            "funnel: {funnel}"
        );
        assert!(funnel.contains(&project_id.to_string()), "funnel: {funnel}");
        for forbidden in [
            "libra@example.com",
            "Libra",
            "libra-estate",
            "Estate Plan",
            "email=",
            "phone=",
            "name=",
            "address=",
            "project_code=",
        ] {
            assert!(!funnel.contains(forbidden), "{forbidden} leaked: {funnel}");
        }
    }

    async fn fs_storage(suite: &str) -> Arc<dyn cloud::StorageService> {
        Arc::new(
            cloud::FsStorage::new(
                std::env::temp_dir().join(format!("navigator-notation-service-{suite}")),
            )
            .await
            .expect("temp FsStorage"),
        )
    }

    #[tokio::test]
    async fn lawyer_review_dispatch_is_noop_but_sends_one_notification() {
        let email = Arc::new(CapturingEmail::new());
        let deps = StepDeps::new(email.clone(), fs_storage("lawyer-review").await);

        let payload = dispatch_workflow_step(
            &deps,
            uuid::Uuid::from_u128(1),
            &StateName::from("lawyer_review"),
            None,
        )
        .await
        .expect("human gate dispatch is a no-op");

        assert!(payload.is_none());
        assert!(email.captured().is_empty());

        send_review_notification(
            email.clone(),
            uuid::Uuid::from_u128(11),
            uuid::Uuid::from_u128(12),
            "sample-litigation",
            uuid::Uuid::from_u128(13),
            "lawyer@example.com",
            ReviewNotificationHop::Lawyer,
        )
        .await
        .expect("lawyer review notification");

        let captured = email.captured();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].subject, "A draft is ready for your review");
        let html = captured[0]
            .html_body
            .as_deref()
            .expect("the firm layout is the html alternative");
        assert!(html.contains("/app/lawyer/notations/00000000-0000-0000-0000-00000000000b/review"));
        assert!(captured[0]
            .body
            .contains("/app/lawyer/notations/00000000-0000-0000-0000-00000000000b/review"));
        assert!(!captured[0].body.contains("Jane Roe"));
        assert!(!captured[0].body.contains("Cruller v. Prine"));
        assert!(!captured[0].body.contains("Estate Plan"));
    }

    #[tokio::test]
    async fn approved_review_sends_one_client_notification() {
        let email = Arc::new(CapturingEmail::new());
        send_review_notification(
            email.clone(),
            uuid::Uuid::from_u128(21),
            uuid::Uuid::from_u128(22),
            "sample-litigation",
            uuid::Uuid::from_u128(23),
            "client@example.com",
            ReviewNotificationHop::Client,
        )
        .await
        .expect("client review notification");

        let captured = email.captured();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].subject, "Your reviewed draft is ready");
        let html = captured[0]
            .html_body
            .as_deref()
            .expect("the firm layout is the html alternative");
        assert!(html.contains("/app/projects/sample-litigation"));
        assert!(captured[0]
            .body
            .ends_with("/app/projects/sample-litigation"));
    }

    #[tokio::test]
    async fn no_lawyer_tier_participant_yields_no_recipient() {
        // The fixture designates a lawyer DRI, but `dri_person` is seeded at
        // the default `Role::Client` tier, so its participation is client-side
        // and the DRI marker resolves to nobody the firm may notify.
        let surreal = store::test_support::mem_surreal().await;
        let notation_id = store::test_support::seed_notation(&surreal).await;
        let plan = super::resolve_review_notification_plan(
            &surreal,
            notation_id,
            ReviewNotificationHop::Lawyer,
        )
        .await
        .expect("resolve review recipients");

        assert!(plan.recipient_ids.is_empty());
    }

    /// `ctx.run` persists this value in the Restate journal, which is an
    /// operator debugging surface. A mailbox names a client and a Project code
    /// names who retained the firm, so neither may be frozen into the plan —
    /// both are read inside the send step instead.
    #[tokio::test]
    async fn the_journaled_plan_carries_opaque_ids_only() {
        let surreal = store::test_support::mem_surreal().await;
        let notation_id = store::test_support::seed_notation(&surreal).await;
        let plan = super::resolve_review_notification_plan(
            &surreal,
            notation_id,
            ReviewNotificationHop::Client,
        )
        .await
        .expect("resolve review recipients");

        assert_eq!(
            plan.recipient_ids.len(),
            1,
            "the client DRI is the recipient"
        );
        let journaled = serde_json::to_string(&plan).expect("the plan is journaled as JSON");
        assert!(
            !journaled.contains('@'),
            "a mailbox reached the journal: {journaled}"
        );
        assert!(
            !journaled.contains("libra-estate"),
            "a Project code reached the journal: {journaled}"
        );
    }

    #[test]
    fn review_hops_cover_approval_and_exclude_reask_or_refusal() {
        let review = StateName::from("lawyer_review");
        assert!(matches!(
            review_notification_hop(&StateName::from("draft"), "ready", &review),
            Some(ReviewNotificationHop::Lawyer)
        ));
        assert!(matches!(
            review_notification_hop(&review, "approved", &StateName::from("generate_pdf__draft")),
            Some(ReviewNotificationHop::Client)
        ));
        assert!(review_notification_hop(
            &review,
            "changes_requested",
            &StateName::from("reask__client")
        )
        .is_none());
        assert!(review_notification_hop(&review, "rejected", &StateName::end()).is_none());
    }

    #[test]
    fn no_lawyer_participants_produce_no_recipient_ids() {
        assert!(review_recipient_ids(&[], ReviewNotificationHop::Lawyer).is_empty());
        let copy = review_copy(ReviewNotificationHop::Lawyer);
        assert_eq!(copy.subject, "A draft is ready for your review");
    }

    #[tokio::test]
    async fn dispatch_workflow_step_runs_dispatched_states() {
        let email = Arc::new(CapturingEmail::new());
        let deps = StepDeps::new(email.clone(), fs_storage("email-send").await);
        let email_payload =
            serde_json::to_string(&EmailPayload::new("Aries", "aries@example.com")).unwrap();

        let payload = dispatch_workflow_step(
            &deps,
            uuid::Uuid::from_u128(2),
            &StateName::from("email_send__welcome"),
            Some(&email_payload),
        )
        .await
        .expect("email_send dispatch succeeds");

        assert!(payload.is_none());
        let captured = email.captured();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].to, "aries@example.com");
        assert_eq!(captured[0].template_slug.as_deref(), Some("welcome"));
    }
}
