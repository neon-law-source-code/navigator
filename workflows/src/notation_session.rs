//! Walk a Notation's questionnaire one answer at a time.
//!
//! Both the admin HTML form (`portal::retainer_walk`) and the MCP
//! tools (`aida_create_notation`, `aida_answer_notation`) drive a
//! Notation through the same two state machines: a questionnaire
//! that asks one question per signal, then a post-intake workflow.
//! This module owns the questionnaire half; the workflow half is
//! caller-driven for now (the dev-loop short-circuit in
//! `retainer_walk::drive_post_questionnaire_workflow` stays in
//! `web`).
//!
//! The runtime — not the application — is the source of truth for
//! questionnaire state. That mirrors `retainer_walk` exactly: in
//! production, the `workflows-service` worker journals each
//! transition inside `ctx.run("append-…", …)`; in tests, the
//! in-memory runtime records the transition in its own `Vec`.
//! Callers do not write `notation_events` themselves.

use std::collections::BTreeMap;
use std::sync::Arc;

use cloud::StorageService;
use thiserror::Error;
use uuid::Uuid;

use crate::runtime::{SignalContext, StateMachineRuntime, WorkflowRuntimeError};
use crate::spec::{MachineKind, QuestionnaireSpec, StateName, WorkflowSpecError};
use crate::specs::{
    audiences_from_template, audiences_from_yaml, catalog_spec_yaml, choices_from_template,
    choices_from_yaml, custom_questions_from_template, custom_questions_from_yaml,
    merge_custom_questions, prompt_overrides_from_template, prompt_overrides_from_yaml,
    questionnaire_spec_from_template, questionnaire_spec_from_yaml, template_has_questionnaire,
};

/// One question presented to the caller — the prompt, the answer
/// shape, and the stable code the caller must echo back on the
/// next `answer_step`. `id` is the row id of the question; the
/// MCP surface ignores it but the admin HTML form uses it to look
/// up any prior answer for the (question, person) pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionDescriptor {
    pub id: Uuid,
    pub code: String,
    pub prompt: String,
    pub answer_type: String,
    pub choices: Vec<QuestionChoice>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct QuestionnaireDefinition {
    spec: QuestionnaireSpec,
    prompts: BTreeMap<String, String>,
    #[serde(default)]
    audiences: BTreeMap<String, String>,
    #[serde(default)]
    choices: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct QuestionChoice {
    pub value: String,
    pub label: String,
}

/// Where the questionnaire is after a `start_notation` /
/// `answer_step` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextStep {
    /// The caller must collect this answer and call `answer_step`.
    NeedsAnswer { question: QuestionDescriptor },
    /// The questionnaire reached `END`. The post-intake workflow
    /// has *not* been started by this module — the caller decides
    /// when and how to kick it off.
    QuestionnaireComplete,
}

/// Output of [`start_notation`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartOutcome {
    pub notation_id: Uuid,
    pub next: NextStep,
}

/// Who entered an answer. The notation's bound Person is always the
/// *respondent* (`answers.person_id`); this records who actually typed
/// the value — the lawyer filling it in on the client's behalf, or
/// the client themselves through the magic link — and the authorship
/// `source` ([`answer::SOURCE_LAWYER`] / [`answer::SOURCE_CLIENT`]) that
/// the data lake groups by.
#[derive(Debug, Clone, Copy)]
pub struct AnswerAuthor<'a> {
    /// FK → persons: who typed the answer. `None` for system/agent
    /// answers with no individual Person row.
    pub authored_by: Option<Uuid>,
    /// `answer::SOURCE_LAWYER` or `answer::SOURCE_CLIENT`.
    pub source: &'a str,
}

impl AnswerAuthor<'static> {
    /// A lawyer-sourced answer typed by `authored_by` (the logged-in
    /// lawyer/admin person, or `None` for the agent surface).
    #[must_use]
    pub fn lawyer(authored_by: Option<Uuid>) -> Self {
        Self {
            authored_by,
            source: store::answers::SOURCE_LAWYER,
        }
    }

    /// A client-sourced answer self-entered by `authored_by` through the
    /// magic link.
    #[must_use]
    pub fn client(authored_by: Option<Uuid>) -> Self {
        Self {
            authored_by,
            source: store::answers::SOURCE_CLIENT,
        }
    }

    /// A machine-extracted answer proposed by batch transcript coverage. It
    /// belongs to no individual typist (`authored_by = None`) and is written
    /// [`answer::SOURCE_EXTRACTED`] so the walk surfaces it as a default the
    /// lawyer confirms or edits — never a silently-accepted answer.
    #[must_use]
    pub fn extracted() -> Self {
        Self {
            authored_by: None,
            source: store::answers::SOURCE_EXTRACTED,
        }
    }
}

#[derive(Debug, Error)]
pub enum NotationSessionError {
    #[error("template `{0}` not found")]
    TemplateNotFound(String),
    #[error(
        "the first notation on a matter must be the engagement that opens it — a retainer or an \
         onboarding — not `{code}` (kind: {kind})"
    )]
    EngagementMustBeFirst { code: String, kind: String },
    #[error("template `{0}` has no questionnaire frontmatter")]
    TemplateHasNoQuestionnaire(String),
    #[error("reading template from the project repo: {0}")]
    TemplateSource(#[from] store::template_source::TemplateSourceError),
    #[error("template: {0}")]
    Template(#[from] store::templates::TemplateError),
    #[error("notation `{0}` not found")]
    NotationNotFound(Uuid),
    #[error("question `{0}` not seeded in store")]
    QuestionNotSeeded(String),
    #[error("question `{0}` is not a client-facing question on this notation's intake")]
    QuestionNotClientFacing(String),
    #[error("question `{0}` was not flagged for re-collection by the lawyer review")]
    QuestionNotFlagged(String),
    #[error("question code mismatch: questionnaire is currently asking `{expected}`, got `{got}`")]
    QuestionMismatch { expected: String, got: String },
    /// An answer named an option its question never declared.
    ///
    /// Carries the state and the declared option keys — both firm-authored
    /// template metadata — and deliberately **not** the submitted value,
    /// which is a client answer. This error is surfaced to callers and
    /// logged, and answers are client content that never reaches a log line.
    #[error("`{state}` accepts only these options: {}", declared.join(", "))]
    UndeclaredChoice {
        state: String,
        declared: Vec<String>,
    },
    #[error("questionnaire is already complete")]
    AlreadyComplete,
    #[error("workflow runtime: {0}")]
    Runtime(#[from] WorkflowRuntimeError),
    #[error("database: {0}")]
    Db(String),
    #[error("notation store: {0}")]
    Notation(#[from] store::notations::NotationError),
    /// The questionnaire's questions and answers moved to SurrealDB with
    /// wave five (ENG-121); the notation and its template, then the
    /// `notation_event` / `reask` journal tables, followed with later
    /// slices of the same wave, so a session now fails against Surreal
    /// throughout.
    #[error("question store: {0}")]
    Question(#[from] store::questions::QuestionError),
    #[error("reask journal: {0}")]
    Reask(#[from] store::reask::ReaskError),
    #[error("answer store: {0}")]
    Answer(#[from] store::answers::AnswerError),
    #[error("spec parse: {0}")]
    Spec(#[from] WorkflowSpecError),
    #[error("encoding questionnaire snapshot: {0}")]
    SnapshotEncode(String),
    #[error("decoding questionnaire snapshot: {0}")]
    SnapshotDecode(String),
}

impl From<String> for NotationSessionError {
    fn from(message: String) -> Self {
        Self::Db(message)
    }
}

/// Create a fresh Notation for `template_code`, start the
/// questionnaire runtime, and return the first question.
///
/// `person_id`, `project_id`, and `entity_id` are caller-resolved —
/// neither this module nor the runtime invents respondents,
/// matters, or entities. Every Notation belongs to exactly one
/// Project; see `docs/notation.md#notation`.
#[allow(clippy::too_many_arguments)]
pub async fn start_notation(
    surreal: &store::surreal::SurrealDb,
    runtime: &dyn StateMachineRuntime,
    storage: Option<&Arc<dyn StorageService>>,
    template_code: &str,
    person_id: Uuid,
    project_id: Uuid,
    entity_id: Option<Uuid>,
) -> Result<StartOutcome, NotationSessionError> {
    // Prefer a Project-scoped template, falling back to the shared one.
    let template_row = store::templates::resolve(surreal, Some(project_id), template_code)
        .await?
        .ok_or_else(|| NotationSessionError::TemplateNotFound(template_code.into()))?;

    let definition = questionnaire_definition_for(surreal, storage, &template_row).await?;
    let snapshot = questionnaire_snapshot_from_definition(&definition)?;

    let mut new_notation = store::notations::NewNotation::new(
        template_row.id,
        person_id,
        project_id,
        StateName::BEGIN,
    )
    .with_questionnaire_snapshot(snapshot);
    if let Some(entity_id) = entity_id {
        new_notation = new_notation.with_entity(entity_id);
    }
    let notation_id = store::notations::create(surreal, &new_notation).await?.id;

    runtime
        .start(
            MachineKind::Questionnaire,
            notation_id,
            definition.spec.inner(),
        )
        .await?;
    let next = first_step(surreal, &definition).await?;
    Ok(StartOutcome { notation_id, next })
}

/// The one `notation create` front door: auto-save the template from the
/// Project's git repo, then open the notation pinned to that exact version.
///
/// It runs the shared `read-repo → validate → persist` engine
/// ([`store::template_source::persist_from_repo`]) — reading
/// `templates/<code>.md` from repo HEAD, refusing loudly on any invalid
/// template, and appending an immutable project-scoped version pinned to
/// `(commit SHA + content hash)` — and then hands off to [`start_notation`],
/// which resolves that just-written project-scoped version (preferred over
/// the shared catalog, IsCurrent) and freezes its questionnaire. `web` and
/// `cli` both reach the engine through here, so the two surfaces open a
/// notation the same way; there is no separate `import` step.
#[allow(clippy::too_many_arguments)]
pub async fn create_notation_from_repo(
    surreal: &store::surreal::SurrealDb,
    runtime: &dyn StateMachineRuntime,
    storage: &Arc<dyn StorageService>,
    repo: &repos::RepoStore,
    template_code: &str,
    person_id: Uuid,
    project_id: Uuid,
    entity_id: Option<Uuid>,
) -> Result<StartOutcome, NotationSessionError> {
    // Prefer a template authored in the Project's own repo — auto-save that
    // exact version and open pinned to it. When the code is not in the repo
    // (or the repo has no commits yet), fall back to the bundled firm
    // catalog: `start_notation` resolves the shared template line, so a
    // standard blueprint (a retainer, an LLC formation) opens on a fresh
    // matter with no per-project authoring. A template that IS in the repo
    // but fails validation still hard-errors — bad paper never opens.
    use store::template_source::TemplateSourceError;
    if let Err(e) =
        store::template_source::persist_from_repo(surreal, storage, repo, project_id, template_code)
            .await
    {
        match e {
            // Not authored in the repo (or the repo has no commits yet) —
            // fall through to the bundled catalog below.
            TemplateSourceError::TemplateNotFound { .. } | TemplateSourceError::RepoEmpty(_) => {}
            // A template that IS in the repo but is invalid hard-errors.
            other => return Err(other.into()),
        }
    }

    // The first notation opened on a matter must be the engagement that
    // makes the matter official — a retainer, or the intake-driven
    // onboarding that opens a bundle of instruments. Later notations
    // (filings, letters) may be any kind. The template's kind is resolved
    // from the row just persisted (repo source) or the shared catalog
    // (bundled source), so this holds for both. A template with no declared
    // kind can never be a matter's first notation.
    let is_first = !store::notations::exists_for_project(surreal, project_id).await?;
    if is_first {
        // Only gate a template that actually resolves — a code present in
        // neither the repo nor the catalog falls through so `start_notation`
        // surfaces `TemplateNotFound` (not a misleading engagement-first
        // error). Which kinds open a matter is `rules::kind`'s call, not
        // this module's: `opens_a_matter` is an exhaustive match, so a new
        // `Kind` must declare its side of the line, and the lawyer matter
        // list (`portal::admin::template_opens_a_matter`) reads the very same
        // classifier — one answer, two callers, no drift.
        if let Some(template) =
            store::templates::resolve(surreal, Some(project_id), template_code).await?
        {
            let opens = template
                .kind
                .as_deref()
                .and_then(rules::kind::Kind::parse)
                .is_some_and(rules::kind::Kind::opens_a_matter);
            if !opens {
                return Err(NotationSessionError::EngagementMustBeFirst {
                    code: template_code.to_string(),
                    kind: template.kind.unwrap_or_else(|| "none".into()),
                });
            }
        }
    }

    start_notation(
        surreal,
        runtime,
        Some(storage),
        template_code,
        person_id,
        project_id,
        entity_id,
    )
    .await
}

/// Encode the questionnaire graph for a Template row so callers that
/// create Notations directly still freeze the same askable set as
/// [`start_notation`].
pub async fn questionnaire_snapshot_for_template(
    surreal: &store::surreal::SurrealDb,
    storage: Option<&Arc<dyn StorageService>>,
    template_row: &store::templates::Template,
) -> Result<serde_json::Value, NotationSessionError> {
    let definition = questionnaire_definition_for(surreal, storage, template_row).await?;
    questionnaire_snapshot_from_definition(&definition)
}

/// The ordered question-code chain (BEGIN → … → END, following the
/// unconditional `_` edge) the notation actually walks, sourced from its
/// frozen questionnaire snapshot — the scoped blob's questionnaire when the
/// pinned template carried one. The admin walker's progress indicator reads
/// this so the total tracks the questionnaire the client is answering, not
/// the compile-time bundled spec.
pub async fn questionnaire_chain_for_notation(
    surreal: &store::surreal::SurrealDb,
    storage: Option<&Arc<dyn StorageService>>,
    notation_id: Uuid,
) -> Result<Vec<String>, NotationSessionError> {
    let (_, definition) = load_notation_and_spec(surreal, storage, notation_id).await?;
    Ok(ordered_question_codes(&definition.spec))
}

fn questionnaire_snapshot_from_definition(
    definition: &QuestionnaireDefinition,
) -> Result<serde_json::Value, NotationSessionError> {
    serde_json::to_value(definition)
        .map_err(|e| NotationSessionError::SnapshotEncode(e.to_string()))
}

/// Look up the question the questionnaire is *currently* asking,
/// without writing anything. Returns `QuestionnaireComplete` when
/// the questionnaire has already reached END.
pub async fn current_step(
    surreal: &store::surreal::SurrealDb,
    runtime: &dyn StateMachineRuntime,
    storage: Option<&Arc<dyn StorageService>>,
    notation_id: Uuid,
) -> Result<NextStep, NotationSessionError> {
    let (_, definition) = load_notation_and_spec(surreal, storage, notation_id).await?;
    let current_state = runtime
        .current_state(MachineKind::Questionnaire, notation_id)
        .await
        .unwrap_or_else(StateName::begin);
    next_step_from(surreal, &definition, &current_state).await
}

/// Persist one answer, advance the questionnaire, and return the
/// next question — or `QuestionnaireComplete` if that answer
/// landed the machine at END.
///
/// `question_code` MUST match the question the runtime is
/// currently expecting; mismatches return [`NotationSessionError::QuestionMismatch`]
/// so a confused caller fails fast rather than silently writing an
/// answer against the wrong question.
///
/// `author` records who typed the answer and the authorship source (see
/// [`AnswerAuthor`]). The notation's bound Person stays the *respondent*
/// (`answers.person_id`) regardless of who entered it, so a lawyer-entered
/// and a client-entered answer to the same question share a respondent
/// but differ in authorship.
#[allow(clippy::too_many_arguments)]
pub async fn answer_step(
    surreal: &store::surreal::SurrealDb,
    runtime: &dyn StateMachineRuntime,
    storage: Option<&Arc<dyn StorageService>>,
    notation_id: Uuid,
    question_code: &str,
    value: &str,
    author: AnswerAuthor<'_>,
) -> Result<NextStep, NotationSessionError> {
    answer_step_with_reference(
        surreal,
        runtime,
        storage,
        notation_id,
        question_code,
        value,
        None,
        author,
    )
    .await
}

/// [`answer_step`], plus the id of the existing row a DB-backed picker
/// selected for a record/reference question. `value` stays the row's
/// display name (what the placeholder renders); `reference_id` is embedded
/// in the stored envelope so the read-back surfaces `<state>.id`. Pass
/// `None` for a free-typed answer.
#[allow(clippy::too_many_arguments)]
pub async fn answer_step_with_reference(
    surreal: &store::surreal::SurrealDb,
    runtime: &dyn StateMachineRuntime,
    storage: Option<&Arc<dyn StorageService>>,
    notation_id: Uuid,
    question_code: &str,
    value: &str,
    reference_id: Option<Uuid>,
    author: AnswerAuthor<'_>,
) -> Result<NextStep, NotationSessionError> {
    let (notation_row, definition) = load_notation_and_spec(surreal, storage, notation_id).await?;
    let person_id = notation_row.person_id;

    let current_state = runtime
        .current_state(MachineKind::Questionnaire, notation_id)
        .await
        .unwrap_or_else(StateName::begin);

    let expected = definition
        .spec
        .transitions_from(&current_state)
        .and_then(|t| t.lookup("_"))
        .cloned()
        .ok_or(NotationSessionError::AlreadyComplete)?;
    if expected == StateName::end() {
        return Err(NotationSessionError::AlreadyComplete);
    }
    if expected.as_str() != question_code {
        return Err(NotationSessionError::QuestionMismatch {
            expected: expected.0,
            got: question_code.into(),
        });
    }

    let canonical_code = question_code_for_state(question_code);
    let question_row = store::questions::find_by_code(surreal, canonical_code)
        .await?
        .ok_or_else(|| NotationSessionError::QuestionNotSeeded(question_code.into()))?;
    // Close the declared choice set before writing, alongside the
    // question-code and completeness checks above: an off-list value must
    // not advance the walk, and must not be stored for a later render.
    ensure_declared_choice(&definition.choices, question_code, value)?;

    // The Answer row is application data; the worker doesn't know
    // about it, so we own the write here. Single insert — no txn.
    // `person_id` is the respondent; `authored_by`/`source` record who
    // actually entered it (lawyer on the client's behalf, or the client).
    store::answers::record(
        surreal,
        // The walked state name carries the `<type>__<role>` discriminator
        // (`entity__company`); the question row points at the bare code.
        &store::answers::NewAnswer::new(
            question_row.id,
            person_id,
            answer_value_for_state(question_code, value, reference_id),
        )
        .in_notation(notation_id, question_code)
        .authored_by(author.source, author.authored_by),
    )
    .await?;

    // `start` is idempotent; subsequent calls are no-ops. `signal`
    // advances state and (in production) triggers the worker's
    // `ctx.run` journal write — including stamping the answer
    // value as `payload`.
    runtime
        .start(
            MachineKind::Questionnaire,
            notation_id,
            definition.spec.inner(),
        )
        .await?;
    let signal_context = SignalContext {
        acting_person_id: author.authored_by.unwrap_or(person_id),
    };
    runtime
        .signal_with_context(
            MachineKind::Questionnaire,
            notation_id,
            "_",
            Some(value),
            signal_context,
        )
        .await?;

    // If the next transition would land at END, fire the final
    // signal so the machine actually reaches END before we report
    // completion.
    let next_after = definition
        .spec
        .transitions_from(&expected)
        .and_then(|t| t.lookup("_"))
        .cloned();
    if matches!(&next_after, Some(s) if s == &StateName::end()) {
        runtime
            .signal_with_context(
                MachineKind::Questionnaire,
                notation_id,
                "_",
                None,
                signal_context,
            )
            .await?;
        return Ok(NextStep::QuestionnaireComplete);
    }

    let next_state = next_after.ok_or(NotationSessionError::AlreadyComplete)?;
    Ok(NextStep::NeedsAnswer {
        question: load_question(
            surreal,
            &next_state,
            &definition.prompts,
            &definition.choices,
        )
        .await?,
    })
}

/// The client's place in *their* portion of a notation's intake.
///
/// The client sees only the questions whose `audience` is `client` or
/// `both` ([`store::questions::is_client_facing`]), in spec order.
/// Unlike [`answer_step`], the client surface does **not** drive the
/// questionnaire runtime — that pointer is lawyer's progress toward the
/// post-intake workflow. The client's answers are written straight to the
/// `answers` table ([`record_client_answer`]); reads key the latest answer
/// by the authored questionnaire state (`state_name`), with a bare-code
/// fallback only for rows that predate state-scoped answer writes, so a
/// client edit lands without disturbing lawyer's walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientIntakeStep {
    /// The client should answer (or confirm) this question. `prior_value`
    /// pre-fills any current answer — including one lawyer entered on the
    /// client's behalf — so the client confirms rather than re-types.
    NeedsAnswer {
        question: QuestionDescriptor,
        prior_value: Option<String>,
        /// 1-based position among this notation's client-facing questions.
        position: usize,
        /// Count of client-facing questions on this notation.
        total: usize,
    },
    /// The client has answered every client-facing question; the rest is
    /// the firm's to finish.
    Complete { total: usize },
}

/// The ordered question codes a questionnaire walks, BEGIN → … → END
/// (following the unconditional `_` edge each step). The same ordering
/// the admin walker's progress indicator uses.
fn ordered_question_codes(spec: &QuestionnaireSpec) -> Vec<String> {
    let mut codes = Vec::new();
    let mut here = StateName::begin();
    while let Some(next) = spec
        .transitions_from(&here)
        .and_then(|t| t.lookup("_"))
        .cloned()
    {
        if next == StateName::end() {
            break;
        }
        codes.push(next.as_str().to_string());
        here = next;
    }
    codes
}

/// Resolve where the client is in their portion of `notation_id`'s
/// intake: the first client-facing question the client has not yet
/// answered (no `client`-sourced answer), pre-filled with any current
/// value, or [`ClientIntakeStep::Complete`] when the client has answered
/// them all. Save-per-step: a drop-off resumes at the first question
/// still missing a client answer.
pub async fn client_intake_step(
    surreal: &store::surreal::SurrealDb,
    storage: Option<&Arc<dyn StorageService>>,
    notation_id: Uuid,
) -> Result<ClientIntakeStep, NotationSessionError> {
    let (notation_row, definition) = load_notation_and_spec(surreal, storage, notation_id).await?;
    let person_id = notation_row.person_id;

    let codes = ordered_question_codes(&definition.spec);
    let canonical_codes: Vec<String> = codes
        .iter()
        .map(|code| question_code_for_state(code).to_string())
        .collect();
    let rows = store::questions::find_by_codes(surreal, &canonical_codes).await?;
    let by_code: BTreeMap<String, store::questions::Question> =
        rows.into_iter().map(|q| (q.code.clone(), q)).collect();
    let id_to_code: BTreeMap<Uuid, String> =
        by_code.values().map(|q| (q.id, q.code.clone())).collect();

    // Client-facing questions, in spec order.
    let client_codes: Vec<String> = codes
        .iter()
        .filter(|c| {
            metadata_lookup(&definition.audiences, c).map_or_else(
                || {
                    by_code
                        .get(question_code_for_state(c))
                        .is_some_and(|q| store::questions::is_client_facing(&q.audience))
                },
                |audience| store::questions::is_client_facing(audience),
            )
        })
        .cloned()
        .collect();
    let total = client_codes.len();

    // One pass over the respondent's answers: latest value per code (for
    // pre-fill) and the set of codes the client has answered themselves.
    let answers = store::answers::for_person_in_notation(surreal, person_id, notation_id).await?;
    let mut latest_value: BTreeMap<String, String> = BTreeMap::new();
    let mut client_answer_counts: BTreeMap<String, usize> = BTreeMap::new();
    for a in answers {
        let Some(canonical_code) = id_to_code.get(&a.question_id) else {
            continue;
        };
        let answer_code = a
            .state_name
            .clone()
            .unwrap_or_else(|| canonical_code.clone());
        if a.source == store::answers::SOURCE_CLIENT {
            *client_answer_counts.entry(answer_code.clone()).or_default() += 1;
        }
        latest_value.insert(answer_code, store::answers::display_value(&a.value));
    }
    let client_answered = answered_client_states(&client_codes, client_answer_counts);

    for (idx, code) in client_codes.iter().enumerate() {
        if client_answered.contains(code) {
            continue;
        }
        let question = load_question(
            surreal,
            &StateName::from(code.as_str()),
            &definition.prompts,
            &definition.choices,
        )
        .await?;
        return Ok(ClientIntakeStep::NeedsAnswer {
            question,
            prior_value: latest_value
                .get(code)
                .or_else(|| latest_value.get(question_code_for_state(code)))
                .cloned(),
            position: idx + 1,
            total,
        });
    }
    Ok(ClientIntakeStep::Complete { total })
}

/// Record one client-sourced answer to a client-facing question on
/// `notation_id`, without advancing the lawyer questionnaire runtime.
/// Rejects a question that is lawyer-only or outside the notation's
/// questionnaire so a hand-crafted POST can't write an arbitrary answer.
pub async fn record_client_answer(
    surreal: &store::surreal::SurrealDb,
    storage: Option<&Arc<dyn StorageService>>,
    notation_id: Uuid,
    question_code: &str,
    value: &str,
    authored_by: Uuid,
) -> Result<(), NotationSessionError> {
    record_client_answer_with_reference(
        surreal,
        storage,
        notation_id,
        question_code,
        value,
        None,
        authored_by,
    )
    .await
}

/// [`record_client_answer`], plus the id of the existing row a DB-backed
/// picker selected. Mirrors [`answer_step_with_reference`] on the client
/// self-serve surface: `value` is the row's display name, `reference_id`
/// is embedded in the stored envelope. Pass `None` for a free-typed answer.
#[allow(clippy::too_many_arguments)]
pub async fn record_client_answer_with_reference(
    surreal: &store::surreal::SurrealDb,
    storage: Option<&Arc<dyn StorageService>>,
    notation_id: Uuid,
    question_code: &str,
    value: &str,
    reference_id: Option<Uuid>,
    authored_by: Uuid,
) -> Result<(), NotationSessionError> {
    let (notation_row, definition) = load_notation_and_spec(surreal, storage, notation_id).await?;
    if !ordered_question_codes(&definition.spec)
        .iter()
        .any(|c| c == question_code)
    {
        return Err(NotationSessionError::QuestionNotClientFacing(
            question_code.into(),
        ));
    }
    let canonical_code = question_code_for_state(question_code);
    let question_row = store::questions::find_by_code(surreal, canonical_code)
        .await?
        .ok_or_else(|| NotationSessionError::QuestionNotSeeded(question_code.into()))?;
    let audience = metadata_lookup(&definition.audiences, question_code)
        .map_or(question_row.audience.as_str(), String::as_str);
    if !store::questions::is_client_facing(audience) {
        return Err(NotationSessionError::QuestionNotClientFacing(
            question_code.into(),
        ));
    }
    ensure_declared_choice(&definition.choices, question_code, value)?;
    store::answers::record(
        surreal,
        &store::answers::NewAnswer::new(
            question_row.id,
            notation_row.person_id,
            answer_value_for_state(question_code, value, reference_id),
        )
        .in_notation(notation_id, question_code)
        .authored_by(store::answers::SOURCE_CLIENT, Some(authored_by)),
    )
    .await?;
    Ok(())
}

/// Re-collect one flagged answer on a notation parked at `reask__client`
/// after a `lawyer_review` change request. Writes a fresh, latest-wins answer
/// row attributed to `author` — the client self-serve, or lawyer on their
/// behalf — **without** touching the completed questionnaire runtime.
///
/// Answers and questions are decoupled: a correction re-collects a specific
/// answer, it never re-walks intake, so the questionnaire pointer (already at
/// END) stays put and every other answer is left as-is. The write is gated to
/// the flagged set ([`store::reask::flagged_questions`]) — a question the
/// review did not flag is rejected, so a hand-crafted POST can't rewrite an
/// un-flagged answer. Audience gating (client-facing only) is the caller's,
/// applied on the client self-serve surface; lawyer-on-behalf may re-collect
/// any flagged question.
///
/// Generic over the connection so a caller can re-collect several flagged
/// answers inside one transaction (all-or-nothing) before resubmitting for
/// review. The re-rendered document (on the next approve) assembles from the
/// current answers, so the corrected value is what the attorney re-reviews
/// and, ultimately, what gets signed.
pub async fn record_reask_answer(
    surreal: &store::surreal::SurrealDb,
    notation_id: Uuid,
    question_code: &str,
    value: &str,
    reference_id: Option<Uuid>,
    author: AnswerAuthor<'_>,
) -> Result<(), NotationSessionError> {
    // The frozen questionnaire snapshot every notation carries supplies the
    // declared choice set, so the correction is closed against the same
    // options the original answer was — a re-collected value is what the
    // attorney re-reviews and what ultimately gets signed.
    let (notation_row, definition) = load_notation_and_spec(surreal, None, notation_id).await?;
    let flagged = store::reask::flagged_questions(surreal, notation_id).await?;
    if !flagged.iter().any(|c| c == question_code) {
        return Err(NotationSessionError::QuestionNotFlagged(
            question_code.into(),
        ));
    }
    ensure_declared_choice(&definition.choices, question_code, value)?;
    let canonical_code = question_code_for_state(question_code);
    let question_row = store::questions::find_by_code(surreal, canonical_code)
        .await?
        .ok_or_else(|| NotationSessionError::QuestionNotSeeded(question_code.into()))?;
    store::answers::record(
        surreal,
        &store::answers::NewAnswer::new(
            question_row.id,
            notation_row.person_id,
            answer_value_for_state(question_code, value, reference_id),
        )
        .in_notation(notation_id, question_code)
        .authored_by(author.source, author.authored_by),
    )
    .await?;
    Ok(())
}

async fn load_notation_and_spec(
    surreal: &store::surreal::SurrealDb,
    storage: Option<&Arc<dyn StorageService>>,
    notation_id: Uuid,
) -> Result<(store::notations::Notation, QuestionnaireDefinition), NotationSessionError> {
    let notation_row = store::notations::find_by_id(surreal, notation_id)
        .await?
        .ok_or(NotationSessionError::NotationNotFound(notation_id))?;

    // Resolve against the frozen snapshot, immune to later template/binary
    // changes. Only a Notation created before the snapshot column
    // (`questionnaire_snapshot IS NULL`) re-resolves from the template.
    if let Some(snapshot) = &notation_row.questionnaire_snapshot {
        let definition = serde_json::from_value(snapshot.clone())
            .map_err(|e| NotationSessionError::SnapshotDecode(e.to_string()))?;
        return Ok((notation_row, definition));
    }
    let template_row = store::templates::find_by_id(surreal, notation_row.template_id)
        .await?
        .ok_or(NotationSessionError::NotationNotFound(notation_id))?;
    let definition = questionnaire_definition_for(surreal, storage, &template_row).await?;
    Ok((notation_row, definition))
}

/// Resolve a template's questionnaire spec.
///
/// A **project-scoped** row whose blob body carries its own `questionnaire:`
/// block wins: a Project that took the trouble to version its own
/// questionnaire must not have it silently shadowed by the compile-time
/// binary, so its blob is parsed even when the `code` also has a bundled spec.
///
/// A scoped row that overrides only the *document body* (a blob with no
/// `questionnaire:` block) still uses the bundled questionnaire — bundled
/// codes may be body-only overridden per Project. Bundled rows, blob-less
/// scoped rows, and scoped rows whose blob can't be read likewise fall back to
/// the bundled standalone YAML (compile-time `include_str!`). A non-bundled
/// template parses its spec from the markdown body; one with no body in
/// storage (or no `storage` handle) cannot drive a questionnaire and surfaces
/// [`NotationSessionError::TemplateHasNoQuestionnaire`].
async fn questionnaire_definition_for(
    surreal: &store::surreal::SurrealDb,
    storage: Option<&Arc<dyn StorageService>>,
    template_row: &store::templates::Template,
) -> Result<QuestionnaireDefinition, NotationSessionError> {
    // A project-scoped row that carries its own questionnaire blob wins over
    // the bundled YAML — but only if the blob body actually declares a
    // `questionnaire:` block. A body-only override (or an unreadable asset)
    // keeps the bundled questionnaire, so a scoped body customization never
    // breaks intake for a bundled code.
    if template_row.project_id.is_some() && template_row.asset_id.is_some() {
        if let Some(storage) = storage {
            if let Ok(body) = store::templates::body(surreal, storage, template_row).await {
                if template_has_questionnaire(&body) {
                    return definition_from_body(&body);
                }
            }
        }
    }
    if let Some(yaml) = catalog_spec_yaml(&template_row.code) {
        return definition_from_yaml(yaml);
    }
    let storage = storage.ok_or_else(|| {
        NotationSessionError::TemplateHasNoQuestionnaire(template_row.code.clone())
    })?;
    let body = store::templates::body(surreal, storage, template_row)
        .await
        .map_err(|_| NotationSessionError::TemplateHasNoQuestionnaire(template_row.code.clone()))?;
    definition_from_body(&body)
}

/// Build a [`QuestionnaireDefinition`] from a bundled standalone spec YAML.
fn definition_from_yaml(yaml: &str) -> Result<QuestionnaireDefinition, NotationSessionError> {
    let mut prompts = prompt_overrides_from_yaml(yaml)?;
    let mut choices = choices_from_yaml(yaml)?;
    merge_custom_questions(
        &custom_questions_from_yaml(yaml)?,
        &mut prompts,
        &mut choices,
    );
    Ok(QuestionnaireDefinition {
        spec: questionnaire_spec_from_yaml(yaml)?,
        prompts,
        audiences: audiences_from_yaml(yaml)?,
        choices,
    })
}

/// Build a [`QuestionnaireDefinition`] from a template's markdown body
/// (its `questionnaire:` frontmatter block).
fn definition_from_body(body: &str) -> Result<QuestionnaireDefinition, NotationSessionError> {
    let mut prompts = prompt_overrides_from_template(body)?;
    let mut choices = choices_from_template(body)?;
    merge_custom_questions(
        &custom_questions_from_template(body)?,
        &mut prompts,
        &mut choices,
    );
    Ok(QuestionnaireDefinition {
        spec: questionnaire_spec_from_template(body)?,
        prompts,
        audiences: audiences_from_template(body)?,
        choices,
    })
}

async fn first_step(
    surreal: &store::surreal::SurrealDb,
    definition: &QuestionnaireDefinition,
) -> Result<NextStep, NotationSessionError> {
    next_step_from(surreal, definition, &StateName::begin()).await
}

async fn next_step_from(
    surreal: &store::surreal::SurrealDb,
    definition: &QuestionnaireDefinition,
    current_state: &StateName,
) -> Result<NextStep, NotationSessionError> {
    let Some(next) = definition
        .spec
        .transitions_from(current_state)
        .and_then(|t| t.lookup("_"))
        .cloned()
    else {
        return Ok(NextStep::QuestionnaireComplete);
    };
    if next == StateName::end() {
        return Ok(NextStep::QuestionnaireComplete);
    }
    Ok(NextStep::NeedsAnswer {
        question: load_question(surreal, &next, &definition.prompts, &definition.choices).await?,
    })
}

async fn load_question(
    surreal: &store::surreal::SurrealDb,
    state: &StateName,
    prompts: &BTreeMap<String, String>,
    choices: &BTreeMap<String, BTreeMap<String, String>>,
) -> Result<QuestionDescriptor, NotationSessionError> {
    let code = question_code_for_state(state.as_str());
    let row = store::questions::find_by_code(surreal, code)
        .await?
        .ok_or_else(|| NotationSessionError::QuestionNotSeeded(state.0.clone()))?;
    let prompt = if let Some(prompt) = prompt_override_for_state(prompts, state.as_str()) {
        prompt.to_string()
    } else {
        localize_prompt_for_state(&row.prompt, state.as_str())
    };
    Ok(QuestionDescriptor {
        id: row.id,
        code: state.0.clone(),
        prompt,
        answer_type: row.answer_type,
        choices: choices_for_state(choices, state.as_str()),
    })
}

fn question_code_for_state(state: &str) -> &str {
    state.split_once("__").map_or(state, |(code, _)| code)
}

/// Persist one machine-extracted answer for a notation's respondent, keyed by
/// the questionnaire `state_name` and tagged [`answer::SOURCE_EXTRACTED`].
///
/// This is the batch-coverage sibling of [`answer_step`]: it converges on the
/// same one-row-per-question `answers` write, but does **not** touch the state
/// machine. Transcript coverage proposes answers out of walk order, so it
/// cannot drive the questionnaire forward — it seeds proposed defaults that the
/// walk then surfaces (via the latest-per-state read-back) for a lawyer to confirm
/// or edit. Returns `false` when `state_name`'s registry question isn't seeded
/// (skipped, mirroring [`seed`]-style writers); `true` once a row is inserted.
///
/// Takes `storage` because resolving the questionnaire is what supplies the
/// declared choice set a proposal is checked against, and a notation created
/// directly — rather than through [`start_notation`] — carries no frozen
/// snapshot, so its spec is read from the template body in storage.
pub async fn record_extracted_answer(
    surreal: &store::surreal::SurrealDb,
    storage: Option<&Arc<dyn StorageService>>,
    notation_id: Uuid,
    state_name: &str,
    value: &str,
) -> Result<bool, String> {
    // The notation and its questionnaire in one load: the spec supplies the
    // declared choice set this proposal is closed against.
    let (notation_row, definition) =
        match load_notation_and_spec(surreal, storage, notation_id).await {
            Ok(loaded) => loaded,
            // A vanished notation is skipped, mirroring the unseeded-question
            // arm below rather than failing the whole coverage run.
            Err(NotationSessionError::NotationNotFound(_)) => return Ok(false),
            Err(e) => return Err(e.to_string()),
        };
    let canonical = question_code_for_state(state_name);
    let Some(question_row) = store::questions::find_by_code(surreal, canonical)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(false);
    };
    // A transcript proposal is read back by the render exactly like a typed
    // answer (latest-per-state wins, with no filter on `source`), so an
    // off-list extraction is refused here too. It is *skipped* rather than
    // an error: the caller reports a skipped finding as uncovered, which is
    // the right outcome — the lawyer is asked the question instead of being
    // shown a proposal the questionnaire never offered.
    if ensure_declared_choice(&definition.choices, state_name, value).is_err() {
        return Ok(false);
    }
    let author = AnswerAuthor::extracted();
    store::answers::record(
        surreal,
        &store::answers::NewAnswer::new(
            question_row.id,
            notation_row.person_id,
            store::answers::primitive(value),
        )
        .in_notation(notation_id, state_name)
        .authored_by(author.source, author.authored_by),
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(true)
}

/// Build the stored `answers.value` envelope for one answer. Singular
/// record/reference states mirror the row they create or select
/// (`{"value":name,"name":name}`); when the answer picked an existing row
/// by id (a DB-backed picker selection), that row's `id` is embedded so
/// the read-back surfaces `<state>.id` without changing what a placeholder
/// renders (`value`/`name` stay the display string). Everything else is a
/// primitive `{"value":…}`.
fn answer_value_for_state(
    state: &str,
    value: &str,
    reference_id: Option<Uuid>,
) -> serde_json::Value {
    match question_code_for_state(state) {
        "person" | "entity" | "project" | "jurisdiction" | "country" => {
            let mut envelope = serde_json::json!({ "value": value, "name": value });
            if let Some(id) = reference_id {
                envelope["id"] = serde_json::json!(id);
            }
            envelope
        }
        _ => store::answers::primitive(value),
    }
}

fn role_key_for_state(state: &str) -> &str {
    state.split_once("__").map_or(state, |(_, role)| role)
}

fn prompt_override_for_state<'a>(
    prompts: &'a BTreeMap<String, String>,
    state: &str,
) -> Option<&'a str> {
    metadata_lookup(prompts, state).map(String::as_str)
}

fn choices_for_state(
    choices: &BTreeMap<String, BTreeMap<String, String>>,
    state: &str,
) -> Vec<QuestionChoice> {
    metadata_lookup(choices, state)
        .into_iter()
        .flat_map(|entries| entries.iter())
        .map(|(value, label)| QuestionChoice {
            value: value.clone(),
            label: label.clone(),
        })
        .collect()
}

fn metadata_lookup<'a, T>(map: &'a BTreeMap<String, T>, state: &str) -> Option<&'a T> {
    metadata_keys_for_state(state)
        .into_iter()
        .find_map(|key| map.get(key))
}

/// Resolve a stored choice `value` to its human label for a question
/// `state`, given the template's merged choice metadata (`value → label`
/// keyed by custom-question key / question code — see
/// [`merged_choices_from_yaml`]). Returns `None` for a free-text state
/// (no choice metadata) or a value that isn't a declared option, so the
/// caller falls back to the raw value. Lets a rendered document show the
/// label ("Married"), not the stored key ("married"), everywhere the
/// walker would have shown the label.
#[must_use]
pub fn choice_label(
    choices: &BTreeMap<String, BTreeMap<String, String>>,
    state: &str,
    value: &str,
) -> Option<String> {
    metadata_lookup(choices, state)
        .and_then(|by_value| by_value.get(value))
        .cloned()
}

/// Whether `state`'s answer is a single declared choice key, so it can be
/// checked against the declared set.
///
/// `custom_single_choice` and `custom_yes_no` store exactly one key. A
/// `custom_multiple_choice` answer is a *set* of keys rather than one, and
/// needs the checkbox-group field shape that ENG-454 tracks separately — so
/// it is deliberately left open here rather than closed against a check that
/// would reject a legitimate multi-value answer. Record and reference types
/// declare no YAML choices at all; their closed set is the database
/// candidate list, matched where the pick is resolved.
fn stores_one_declared_choice(state: &str) -> bool {
    use store::question_registry::QuestionType;
    matches!(
        QuestionType::from_state_name(state),
        Some(QuestionType::CustomSingleChoice | QuestionType::CustomYesNo)
    )
}

/// Refuse an answer that names an option its question never declared — the
/// closed-choice check every answer write shares.
///
/// A choice question declares a closed `value → label` set
/// (`custom_questions.<key>.choices`). The render side resolves a stored
/// value through [`choice_label`] and falls back to the raw string when the
/// value is not a declared option, so an undeclared value is substituted
/// verbatim into whatever the template says — including the engagement
/// letter's governing-law and arbitration clause, whose choices are
/// `nevada`/`california`/`washington`. Closing the set at render time is not
/// enough: the answer is already stored, and a document assembled from
/// stored answers is what a client signs.
///
/// So the set is closed here, where an answer enters. The browser's radio
/// group is the only surface that *cannot* post an off-list value; the CLI,
/// the REST command boundary, the AIDA tool surface, and a hand-crafted POST
/// all can.
///
/// Reads the declared set through the same [`metadata_lookup`] the render
/// side uses, so the guard and the label resolution cannot drift: exactly
/// the values [`choice_label`] can map are the values this accepts.
fn ensure_declared_choice(
    choices: &BTreeMap<String, BTreeMap<String, String>>,
    state: &str,
    value: &str,
) -> Result<(), NotationSessionError> {
    if !stores_one_declared_choice(state) {
        return Ok(());
    }
    // No declared set means "not a choice question" (a free-text or date
    // custom primitive), not "no valid options" — leave it alone.
    let Some(declared) = metadata_lookup(choices, state).filter(|d| !d.is_empty()) else {
        return Ok(());
    };
    if declared.contains_key(value) {
        return Ok(());
    }
    Err(NotationSessionError::UndeclaredChoice {
        state: state.to_string(),
        declared: declared.keys().cloned().collect(),
    })
}

/// The merged `value → label` choice metadata for a bundled spec YAML,
/// keyed by custom-question key / question code — the same map the walker
/// resolves a question's radio options from ([`choices_from_yaml`] merged
/// with each `custom_questions.<key>.choices`). Pair with [`choice_label`]
/// to turn a stored choice key back into its label at render time.
pub fn merged_choices_from_yaml(
    yaml: &str,
) -> Result<BTreeMap<String, BTreeMap<String, String>>, WorkflowSpecError> {
    let mut choices = choices_from_yaml(yaml)?;
    merge_custom_questions(
        &custom_questions_from_yaml(yaml)?,
        &mut BTreeMap::new(),
        &mut choices,
    );
    Ok(choices)
}

fn metadata_keys_for_state(state: &str) -> Vec<&str> {
    let role = role_key_for_state(state);
    let ty = question_code_for_state(state);
    match (ty, role) {
        ("person", "client") => vec!["client", "client_name"],
        ("project", "engagement") => vec!["engagement", "project_name"],
        ("entity", "company") => vec!["company", "entity_name"],
        ("entity", "nonprofit") => vec!["nonprofit", "nonprofit_legal_name"],
        ("person", "worker") => vec!["worker", "worker_legal_name"],
        _ => vec![role],
    }
}

fn answered_client_states(
    client_codes: &[String],
    mut answer_counts_by_code: BTreeMap<String, usize>,
) -> std::collections::BTreeSet<String> {
    let mut answered = std::collections::BTreeSet::new();
    for code in client_codes {
        if let Some(remaining) = answer_counts_by_code.get_mut(code) {
            if *remaining > 0 {
                answered.insert(code.clone());
                *remaining -= 1;
                continue;
            }
        }
        if let Some(remaining) = answer_counts_by_code.get_mut(question_code_for_state(code)) {
            if *remaining > 0 {
                answered.insert(code.clone());
                *remaining -= 1;
            }
        }
    }
    answered
}

/// Substitute a question prompt's `{{for_label}}` / `{{label}}` / `{label}`
/// placeholders with the state's role discriminator (`person__client` →
/// `client`), the same way the walker renders a step. Public so a review
/// surface listing the questionnaire's questions (the "Request changes" and
/// re-ask surfaces) renders the same clean label the client saw, rather than
/// the raw templated prompt.
#[must_use]
pub fn localize_prompt_for_state(prompt: &str, state: &str) -> String {
    let label = state
        .split_once("__")
        .map_or(state, |(_, label)| label)
        .replace('_', " ");
    prompt
        .replace("{{for_label}}", &label)
        .replace("{{label}}", &label)
        .replace("{label}", &label)
}

#[cfg(test)]
mod tests {
    use super::{
        answer_step, answer_value_for_state, answered_client_states, current_step,
        ordered_question_codes, questionnaire_chain_for_notation, questionnaire_definition_for,
        record_reask_answer, start_notation, AnswerAuthor, NextStep, NotationSessionError,
        QuestionDescriptor, QuestionnaireDefinition, StateName,
    };

    #[test]
    fn reference_envelope_embeds_the_selected_row_id() {
        // A record/reference state mirrors the row: value/name are the
        // display string; a picker selection embeds the row id so the
        // read-back surfaces `<state>.id` without changing what renders.
        let picked = Uuid::now_v7();
        let with_id = answer_value_for_state("country__of_birth", "Mexico", Some(picked));
        assert_eq!(with_id["value"], "Mexico");
        assert_eq!(with_id["name"], "Mexico");
        assert_eq!(with_id["id"], picked.to_string());

        // No selection → the same envelope without an id (a free-typed row).
        let no_id = answer_value_for_state("entity__company", "Bright Star", None);
        assert_eq!(no_id["value"], "Bright Star");
        assert_eq!(no_id["name"], "Bright Star");
        assert!(no_id.get("id").is_none());

        // A custom primitive is unchanged — never grows an id field.
        let primitive = answer_value_for_state("custom_text__note", "hello", Some(picked));
        assert_eq!(primitive, store::answers::primitive("hello"));
    }
    use crate::questionnaire_spec_from_yaml;
    use crate::runtime::InMemoryRuntime;
    use cloud::StorageService;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use store::answers::{SOURCE_CLIENT, SOURCE_LAWYER};
    use store::persons::{self, NewPerson};
    use store::surreal::SurrealDb;
    use uuid::Uuid;

    /// The store: persons, projects, templates, questions, answers, the
    /// notation itself, and the append-only `notation_event` / `reask`
    /// journal this module writes through.
    async fn db() -> SurrealDb {
        store::test_support::mem_surreal().await
    }

    async fn seed_person(surreal: &SurrealDb, email: &str) -> Uuid {
        persons::create(surreal, &NewPerson::new(email, email))
            .await
            .unwrap()
            .id
    }

    async fn seed_project(surreal: &SurrealDb) -> Uuid {
        store::projects::create(
            surreal,
            &store::projects::NewProject {
                code: format!("test-project-{}", Uuid::now_v7()),
                name: "test project".into(),
                status: "open".into(),
                entity_id: store::test_support::seed_entity(surreal).await,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .id
    }

    async fn seed_retainer_template(surreal: &SurrealDb) {
        // The retainer template body is bundled via include_str!;
        // for tests we only need the row to exist with the
        // matching `code` so the spec lookup hits the bundled YAML.
        seed_template(surreal, "onboarding__letter", "Retainer").await;
    }

    async fn seed_template(surreal: &SurrealDb, code: &str, title: &str) {
        store::templates::save_version(
            surreal,
            None,
            code,
            store::templates::Version {
                title: title.into(),
                respondent_type: "person".into(),
                asset_id: None,
                form_code: None,
                kind: None,
                source_commit_sha: None,
            },
        )
        .await
        .unwrap();
    }

    async fn seed_question(surreal: &SurrealDb, code: &str) {
        store::questions::create(
            surreal,
            &store::questions::NewQuestion::new(code, format!("Prompt for {code}"), "string"),
        )
        .await
        .unwrap();
    }

    async fn seed_retainer_questions(surreal: &SurrealDb) {
        // The retainer's leading entity / principal-office questions.
        seed_question(surreal, "entity").await;
        seed_question(surreal, "address").await;
        seed_question(surreal, "person").await;
        seed_question(surreal, "project").await;
        seed_question(surreal, "custom_text").await;
        // The retainer's engagement-start-date question (N120).
        seed_question(surreal, "custom_datetime").await;
        // The retainer's governing-law question (ENG-145).
        seed_question(surreal, "custom_single_choice").await;
    }

    /// The plausible entity/address values the retainer walk tests answer
    /// its two leading questions with.
    const TEST_ENTITY_NAME: &str = "Northstar Ventures LLC";
    const TEST_ENTITY_ADDRESS: &str = "100 Innovation Way, Reno, NV 89501";

    async fn seed_retainer_questions_with_audiences(surreal: &SurrealDb) {
        seed_retainer_questions(surreal).await;
    }

    #[tokio::test]
    async fn start_notation_creates_row_and_returns_first_question() {
        let surreal = db().await;
        seed_retainer_template(&surreal).await;
        seed_retainer_questions(&surreal).await;
        let person_id = seed_person(&surreal, "libra@example.com").await;
        let runtime = InMemoryRuntime::new();

        let outcome = start_notation(
            &surreal,
            &runtime,
            None,
            "onboarding__letter",
            person_id,
            seed_project(&surreal).await,
            None,
        )
        .await
        .unwrap();

        // Notation row exists, linked to the right person.
        let row = store::notations::find_by_id(&surreal, outcome.notation_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.person_id, person_id);
        assert_eq!(row.entity_id, None);
        assert_eq!(row.state, "BEGIN");

        // First question per retainer questionnaire is entity.
        match outcome.next {
            NextStep::NeedsAnswer {
                question: QuestionDescriptor { code, .. },
            } => {
                assert_eq!(code, "entity");
            }
            NextStep::QuestionnaireComplete => {
                panic!("expected NeedsAnswer, got QuestionnaireComplete")
            }
        }
    }

    #[tokio::test]
    async fn record_reask_answer_gates_to_the_flagged_set() {
        // After a lawyer_review flags one answer, re-collection is scoped to
        // that answer: the flagged question is re-answerable (client
        // self-serve or lawyer-on-behalf), an un-flagged one is refused — a
        // rejected review re-collects the wrong answers, never the whole
        // questionnaire, and a hand-crafted POST can't rewrite an un-flagged
        // answer.
        let surreal = db().await;
        seed_retainer_template(&surreal).await;
        seed_retainer_questions(&surreal).await;
        let person_id = seed_person(&surreal, "libra@example.com").await;
        let runtime = InMemoryRuntime::new();
        let notation_id = start_notation(
            &surreal,
            &runtime,
            None,
            "onboarding__letter",
            person_id,
            seed_project(&surreal).await,
            None,
        )
        .await
        .unwrap()
        .notation_id;
        let lawyer = store::test_support::dri_person(&surreal).await;

        // The review flags only person__client.
        store::reask::record_change_request(
            &surreal,
            notation_id,
            lawyer,
            &["person__client".into()],
            Some("confirm the client's legal name"),
        )
        .await
        .unwrap();

        // Lawyer re-collect the flagged answer on the client's behalf.
        record_reask_answer(
            &surreal,
            notation_id,
            "person__client",
            "Libra Jones",
            None,
            AnswerAuthor::lawyer(Some(lawyer)),
        )
        .await
        .unwrap();

        // The corrected answer is on record, attributed to lawyer, without
        // disturbing the completed questionnaire runtime.
        let row = store::answers::for_notation(&surreal, notation_id)
            .await
            .unwrap()
            .pop()
            .expect("the corrected answer is on record");
        assert_eq!(row.source, SOURCE_LAWYER);
        assert_eq!(row.authored_by_person_id, Some(lawyer));
        assert!(store::answers::display_value(&row.value).contains("Libra Jones"));

        // An un-flagged question is refused, even for lawyer.
        let err = record_reask_answer(
            &surreal,
            notation_id,
            "project__engagement",
            "New Co",
            None,
            AnswerAuthor::client(Some(person_id)),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&err, NotationSessionError::QuestionNotFlagged(q) if q == "project__engagement"),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn create_notation_from_repo_auto_saves_the_template_and_opens_pinned() {
        use super::create_notation_from_repo;
        // A Project repo carrying its own template blueprint at HEAD (a
        // corpus body, guaranteed to validate clean).
        const TEMPLATE: &str = include_str!("../../templates/neon_law/shared/onboarding_letter.md");

        let surreal = db().await;
        let storage: Arc<dyn StorageService> = Arc::new(
            cloud::FsStorage::new(std::env::temp_dir().join("navigator-create-from-repo-test"))
                .await
                .unwrap(),
        );
        let person_id = seed_person(&surreal, "libra@example.com").await;
        let project_id = seed_project(&surreal).await;
        let project = store::projects::find_by_id(&surreal, project_id)
            .await
            .unwrap()
            .unwrap();
        let runtime = InMemoryRuntime::new();

        let dir = tempfile::tempdir().unwrap();
        let repo = repos::RepoStore::new(dir.path());
        repo.ensure_code(&project.code).unwrap();
        repo.commit_as_code(
            &project.code,
            repos::Author {
                name: "Lawyer",
                email: "lawyer@example.com",
            },
            "add amendment template",
            &[("templates/amendment.md", TEMPLATE.as_bytes())],
        )
        .unwrap();
        let head = repo.head_oid_code(&project.code).unwrap().unwrap();

        let outcome = create_notation_from_repo(
            &surreal,
            &runtime,
            &storage,
            &repo,
            "amendment",
            person_id,
            project_id,
            None,
        )
        .await
        .unwrap();

        // The notation pinned the just-saved, project-scoped version, whose
        // provenance is the repo commit it was read from — no separate
        // `import` step ran.
        let notation = store::notations::find_by_id(&surreal, outcome.notation_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(notation.person_id, person_id);
        let pinned = store::templates::find_by_id(&surreal, notation.template_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pinned.code, "amendment");
        assert_eq!(
            pinned.project_id,
            Some(project_id),
            "the version is project-scoped, not the shared catalog"
        );
        assert!(pinned.is_current);
        assert_eq!(pinned.source_commit_sha.as_deref(), Some(head.as_str()));

        // The questionnaire started — the first question is ready to walk.
        assert!(
            matches!(outcome.next, NextStep::NeedsAnswer { .. }),
            "opening the notation must leave the questionnaire ready to answer"
        );
    }

    #[tokio::test]
    async fn start_notation_freezes_the_questionnaire_snapshot() {
        let surreal = db().await;
        seed_retainer_template(&surreal).await;
        seed_retainer_questions(&surreal).await;
        let person_id = seed_person(&surreal, "libra@example.com").await;
        let runtime = InMemoryRuntime::new();

        let outcome = start_notation(
            &surreal,
            &runtime,
            None,
            "onboarding__letter",
            person_id,
            seed_project(&surreal).await,
            None,
        )
        .await
        .unwrap();
        let nid = outcome.notation_id;

        // The snapshot is written at creation.
        let row = store::notations::find_by_id(&surreal, nid)
            .await
            .unwrap()
            .unwrap();
        assert!(
            row.questionnaire_snapshot.is_some(),
            "start_notation must freeze the askable set"
        );

        // Overwrite the snapshot with a *different* questionnaire that starts
        // at project__engagement. The template's bundled spec still
        // starts at entity, so if resolution re-read the
        // template it would ask that; reading the frozen snapshot asks
        // project__engagement.
        let alt = QuestionnaireDefinition {
            spec: questionnaire_spec_from_yaml(
                "questionnaire:\n  BEGIN:\n    _: project__engagement\n  \
                 project__engagement:\n    _: END\n  END: {}\n",
            )
            .unwrap(),
            prompts: BTreeMap::new(),
            audiences: BTreeMap::new(),
            choices: BTreeMap::new(),
        };
        store::notations::update_questionnaire_snapshot(
            &surreal,
            row.id,
            serde_json::to_value(&alt).unwrap(),
        )
        .await
        .unwrap();

        let next = current_step(&surreal, &runtime, None, nid).await.unwrap();
        match next {
            NextStep::NeedsAnswer { question } => assert_eq!(
                question.code, "project__engagement",
                "resolution must read the frozen snapshot, not the template"
            ),
            NextStep::QuestionnaireComplete => panic!("expected NeedsAnswer"),
        }
    }

    async fn fs_storage() -> Arc<dyn StorageService> {
        Arc::new(
            cloud::FsStorage::new(std::env::temp_dir().join("navigator-notation-session-test"))
                .await
                .unwrap(),
        )
    }

    /// A project-scoped template row for `code` whose markdown body — with a
    /// `questionnaire:` frontmatter block — is ingested to blob storage.
    async fn scoped_template_with_blob(
        surreal: &SurrealDb,
        storage: &Arc<dyn StorageService>,
        code: &str,
        project_id: Uuid,
        body: &str,
    ) -> store::templates::Template {
        let asset_id =
            store::assets::ingest_content(surreal, storage, body.as_bytes(), "text/markdown")
                .await
                .unwrap();
        store::templates::save_version(
            surreal,
            Some(project_id),
            code,
            store::templates::Version {
                title: "Scoped Retainer".into(),
                respondent_type: "person".into(),
                asset_id: Some(asset_id),
                form_code: None,
                kind: None,
                source_commit_sha: None,
            },
        )
        .await
        .unwrap()
        .into_model()
    }

    /// A project-scoped template with a *bundled* code (`onboarding__letter`,
    /// whose bundled questionnaire starts at `entity`) that carries its
    /// own divergent questionnaire blob — that blob's questionnaire must win.
    #[tokio::test]
    async fn scoped_template_blob_questionnaire_wins_over_bundled_yaml() {
        let surreal = db().await;
        let storage = fs_storage().await;
        let project_id = seed_project(&surreal).await;

        let divergent = "---\nquestionnaire:\n  BEGIN:\n    _: project__engagement\n  \
             project__engagement:\n    _: END\n  END: {}\n---\nScoped retainer body.\n";
        let scoped = scoped_template_with_blob(
            &surreal,
            &storage,
            "onboarding__letter",
            project_id,
            divergent,
        )
        .await;

        let definition = questionnaire_definition_for(&surreal, Some(&storage), &scoped)
            .await
            .unwrap();
        assert_eq!(
            ordered_question_codes(&definition.spec),
            vec!["project__engagement".to_string()],
            "a project-scoped template's own questionnaire blob must win over the bundled YAML"
        );
    }

    /// A project-scoped bundled-code template that overrides only the
    /// *document body* — a readable blob whose frontmatter carries no
    /// `questionnaire:` block — must still use the bundled questionnaire, not
    /// fail. Scoped versions may body-only override a bundled code.
    #[tokio::test]
    async fn scoped_body_only_override_falls_back_to_bundled_questionnaire() {
        let surreal = db().await;
        let storage = fs_storage().await;
        let project_id = seed_project(&surreal).await;

        // A frontmatter with a non-questionnaire key, then a body: a genuine
        // body-only override of the bundled retainer.
        let body_only = "---\ntitle: Scoped Retainer\n---\n# Retainer\n\nBody-only override.\n";
        let scoped = scoped_template_with_blob(
            &surreal,
            &storage,
            "onboarding__letter",
            project_id,
            body_only,
        )
        .await;

        let definition = questionnaire_definition_for(&surreal, Some(&storage), &scoped)
            .await
            .expect("a body-only override must fall back to the bundled questionnaire");
        assert_eq!(
            ordered_question_codes(&definition.spec),
            vec![
                "entity".to_string(),
                "address__principal_office".to_string(),
                "person__client".to_string(),
                "person__lawyer_dri".to_string(),
                "project__engagement".to_string(),
                "custom_datetime__engagement_start_date".to_string(),
                "custom_text__engagement_scope".to_string(),
                "custom_single_choice__governing_law".to_string(),
            ],
            "a scoped body-only override keeps the bundled questionnaire"
        );
    }

    /// The fallback, pinned in both directions: a bundled (shared) row and a
    /// project-scoped row with *no* blob both keep yielding the compile-time
    /// bundled questionnaire.
    #[tokio::test]
    async fn bundled_and_blobless_scoped_rows_fall_back_to_bundled_yaml() {
        let surreal = db().await;
        let storage = fs_storage().await;
        let project_id = seed_project(&surreal).await;
        let bundled_chain = vec![
            "entity".to_string(),
            "address__principal_office".to_string(),
            "person__client".to_string(),
            "person__lawyer_dri".to_string(),
            "project__engagement".to_string(),
            "custom_datetime__engagement_start_date".to_string(),
            "custom_text__engagement_scope".to_string(),
            "custom_single_choice__governing_law".to_string(),
        ];

        // Shared bundled row (no project, no blob) → bundled retainer YAML.
        let shared = store::templates::save_version(
            &surreal,
            None,
            "onboarding__letter",
            store::templates::Version {
                title: "Retainer".into(),
                respondent_type: "person".into(),
                asset_id: None,
                form_code: None,
                kind: None,
                source_commit_sha: None,
            },
        )
        .await
        .unwrap()
        .into_model();
        let shared_def = questionnaire_definition_for(&surreal, Some(&storage), &shared)
            .await
            .unwrap();
        assert_eq!(
            ordered_question_codes(&shared_def.spec),
            bundled_chain,
            "a bundled row keeps yielding the bundled questionnaire"
        );

        // Project-scoped row for the same bundled code but with no blob →
        // still falls back to the bundled YAML.
        let scoped_no_blob = store::templates::save_version(
            &surreal,
            Some(project_id),
            "onboarding__letter",
            store::templates::Version {
                title: "Retainer".into(),
                respondent_type: "person".into(),
                asset_id: None,
                form_code: None,
                kind: None,
                source_commit_sha: None,
            },
        )
        .await
        .unwrap()
        .into_model();
        let scoped_def = questionnaire_definition_for(&surreal, Some(&storage), &scoped_no_blob)
            .await
            .unwrap();
        assert_eq!(
            ordered_question_codes(&scoped_def.spec),
            bundled_chain,
            "a scoped row with no blob falls back to the bundled questionnaire"
        );
    }

    /// End-to-end: a scoped-blob template freezes its own questionnaire at
    /// `start_notation`, and the progress-chain helper the admin walker uses
    /// reads that frozen snapshot — so the progress total tracks the scoped
    /// questionnaire, not the bundled spec.
    #[tokio::test]
    async fn questionnaire_chain_for_notation_reads_the_scoped_snapshot() {
        let surreal = db().await;
        let storage = fs_storage().await;
        let project_id = seed_project(&surreal).await;
        let person_id = seed_person(&surreal, "libra@example.com").await;
        let runtime = InMemoryRuntime::new();

        let divergent = "---\nquestionnaire:\n  BEGIN:\n    _: project__engagement\n  \
             project__engagement:\n    _: END\n  END: {}\n---\nScoped retainer body.\n";
        scoped_template_with_blob(
            &surreal,
            &storage,
            "onboarding__letter",
            project_id,
            divergent,
        )
        .await;
        seed_question(&surreal, "project").await;

        let outcome = start_notation(
            &surreal,
            &runtime,
            Some(&storage),
            "onboarding__letter",
            person_id,
            project_id,
            None,
        )
        .await
        .unwrap();

        let chain = questionnaire_chain_for_notation(&surreal, Some(&storage), outcome.notation_id)
            .await
            .unwrap();
        assert_eq!(
            chain,
            vec!["project__engagement".to_string()],
            "the progress chain must reflect the scoped questionnaire the notation froze"
        );
    }

    fn prompt_of(next: &NextStep) -> &str {
        match next {
            NextStep::NeedsAnswer { question } => question.prompt.as_str(),
            NextStep::QuestionnaireComplete => panic!("expected NeedsAnswer"),
        }
    }

    #[tokio::test]
    async fn custom_question_uses_template_prompt_override() {
        let surreal = db().await;
        seed_template(&surreal, "nv__dissolution", "Dissolution").await;
        seed_question(&surreal, "custom_text").await;
        let person_id = seed_person(&surreal, "libra@example.com").await;
        let runtime = InMemoryRuntime::new();

        let outcome = start_notation(
            &surreal,
            &runtime,
            None,
            "nv__dissolution",
            person_id,
            seed_project(&surreal).await,
            None,
        )
        .await
        .unwrap();

        assert_eq!(prompt_of(&outcome.next), "What is the dissolution reason?");
    }

    #[tokio::test]
    async fn start_notation_unknown_template_is_template_not_found() {
        let surreal = db().await;
        let person_id = seed_person(&surreal, "libra@example.com").await;
        let runtime = InMemoryRuntime::new();
        let err = start_notation(
            &surreal,
            &runtime,
            None,
            "does_not_exist",
            person_id,
            seed_project(&surreal).await,
            None,
        )
        .await
        .unwrap_err();
        match err {
            NotationSessionError::TemplateNotFound(c) => assert_eq!(c, "does_not_exist"),
            other => panic!("expected TemplateNotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn answer_step_walks_to_next_question() {
        let surreal = db().await;
        seed_retainer_template(&surreal).await;
        seed_retainer_questions(&surreal).await;
        let person_id = seed_person(&surreal, "libra@example.com").await;
        let runtime = InMemoryRuntime::new();

        let started = start_notation(
            &surreal,
            &runtime,
            None,
            "onboarding__letter",
            person_id,
            seed_project(&surreal).await,
            None,
        )
        .await
        .unwrap();
        let id = started.notation_id;

        answer_step(
            &surreal,
            &runtime,
            None,
            id,
            "entity",
            TEST_ENTITY_NAME,
            AnswerAuthor::lawyer(None),
        )
        .await
        .unwrap();
        answer_step(
            &surreal,
            &runtime,
            None,
            id,
            "address__principal_office",
            TEST_ENTITY_ADDRESS,
            AnswerAuthor::lawyer(None),
        )
        .await
        .unwrap();
        let next = answer_step(
            &surreal,
            &runtime,
            None,
            id,
            "person__client",
            "Libra",
            AnswerAuthor::lawyer(None),
        )
        .await
        .unwrap();
        match next {
            NextStep::NeedsAnswer { question } => {
                assert_eq!(question.code, "person__lawyer_dri");
            }
            NextStep::QuestionnaireComplete => {
                panic!("expected NeedsAnswer(person__lawyer_dri), got QuestionnaireComplete");
            }
        }
    }

    #[tokio::test]
    async fn answer_step_with_wrong_code_returns_mismatch() {
        let surreal = db().await;
        seed_retainer_template(&surreal).await;
        seed_retainer_questions(&surreal).await;
        let person_id = seed_person(&surreal, "libra@example.com").await;
        let runtime = InMemoryRuntime::new();
        let started = start_notation(
            &surreal,
            &runtime,
            None,
            "onboarding__letter",
            person_id,
            seed_project(&surreal).await,
            None,
        )
        .await
        .unwrap();

        let err = answer_step(
            &surreal,
            &runtime,
            None,
            started.notation_id,
            "project__engagement",
            "anything",
            AnswerAuthor::lawyer(None),
        )
        .await
        .unwrap_err();
        match err {
            NotationSessionError::QuestionMismatch { expected, got } => {
                assert_eq!(expected, "entity");
                assert_eq!(got, "project__engagement");
            }
            other => panic!("expected QuestionMismatch, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn full_walk_ends_at_questionnaire_complete() {
        let surreal = db().await;
        seed_retainer_template(&surreal).await;
        seed_retainer_questions(&surreal).await;
        let person_id = seed_person(&surreal, "libra@example.com").await;
        let runtime = InMemoryRuntime::new();
        let started = start_notation(
            &surreal,
            &runtime,
            None,
            "onboarding__letter",
            person_id,
            seed_project(&surreal).await,
            None,
        )
        .await
        .unwrap();
        let id = started.notation_id;

        let walk = [
            ("entity", TEST_ENTITY_NAME),
            ("address__principal_office", TEST_ENTITY_ADDRESS),
            ("person__client", "Libra"),
            ("person__lawyer_dri", "Firm Principal"),
            ("project__engagement", "Apollo"),
            ("custom_datetime__engagement_start_date", "2026-09-01"),
            (
                "custom_text__engagement_scope",
                "Draft and file the Apollo formation documents.",
            ),
            ("custom_single_choice__governing_law", "nevada"),
        ];
        let mut last = NextStep::QuestionnaireComplete;
        for (i, (code, value)) in walk.iter().enumerate() {
            last = answer_step(
                &surreal,
                &runtime,
                None,
                id,
                code,
                value,
                AnswerAuthor::lawyer(None),
            )
            .await
            .unwrap();
            if i < walk.len() - 1 {
                let expected_next = walk[i + 1].0;
                match &last {
                    NextStep::NeedsAnswer { question } => {
                        assert_eq!(question.code, expected_next);
                    }
                    NextStep::QuestionnaireComplete => {
                        panic!("step {i}: expected NeedsAnswer, got QuestionnaireComplete");
                    }
                }
            }
        }
        assert!(matches!(last, NextStep::QuestionnaireComplete));
    }

    #[tokio::test]
    async fn answering_after_complete_is_already_complete() {
        let surreal = db().await;
        seed_retainer_template(&surreal).await;
        seed_retainer_questions(&surreal).await;
        let person_id = seed_person(&surreal, "libra@example.com").await;
        let runtime = InMemoryRuntime::new();
        let started = start_notation(
            &surreal,
            &runtime,
            None,
            "onboarding__letter",
            person_id,
            seed_project(&surreal).await,
            None,
        )
        .await
        .unwrap();
        let id = started.notation_id;
        for (code, value) in [
            ("entity", TEST_ENTITY_NAME),
            ("address__principal_office", TEST_ENTITY_ADDRESS),
            ("person__client", "Libra"),
            ("person__lawyer_dri", "Firm Principal"),
            ("project__engagement", "Apollo"),
            ("custom_datetime__engagement_start_date", "2026-09-01"),
            (
                "custom_text__engagement_scope",
                "Draft and file the Apollo formation documents.",
            ),
            ("custom_single_choice__governing_law", "nevada"),
        ] {
            answer_step(
                &surreal,
                &runtime,
                None,
                id,
                code,
                value,
                AnswerAuthor::lawyer(None),
            )
            .await
            .unwrap();
        }
        // One more should fail.
        let err = answer_step(
            &surreal,
            &runtime,
            None,
            id,
            "person__client",
            "again",
            AnswerAuthor::lawyer(None),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, NotationSessionError::AlreadyComplete));
    }

    #[tokio::test]
    async fn current_step_reports_the_question_about_to_be_asked() {
        let surreal = db().await;
        seed_retainer_template(&surreal).await;
        seed_retainer_questions(&surreal).await;
        let person_id = seed_person(&surreal, "libra@example.com").await;
        let runtime = InMemoryRuntime::new();
        let started = start_notation(
            &surreal,
            &runtime,
            None,
            "onboarding__letter",
            person_id,
            seed_project(&surreal).await,
            None,
        )
        .await
        .unwrap();
        let id = started.notation_id;
        // Before any answer: should be entity.
        match current_step(&surreal, &runtime, None, id).await.unwrap() {
            NextStep::NeedsAnswer { question } => {
                assert_eq!(question.code, "entity");
            }
            NextStep::QuestionnaireComplete => {
                panic!("expected NeedsAnswer(entity), got QuestionnaireComplete");
            }
        }
        answer_step(
            &surreal,
            &runtime,
            None,
            id,
            "entity",
            TEST_ENTITY_NAME,
            AnswerAuthor::lawyer(None),
        )
        .await
        .unwrap();
        // After entity: should be address__principal_office.
        match current_step(&surreal, &runtime, None, id).await.unwrap() {
            NextStep::NeedsAnswer { question } => {
                assert_eq!(question.code, "address__principal_office");
            }
            NextStep::QuestionnaireComplete => {
                panic!(
                    "expected NeedsAnswer(address__principal_office), got QuestionnaireComplete"
                );
            }
        }
        answer_step(
            &surreal,
            &runtime,
            None,
            id,
            "address__principal_office",
            TEST_ENTITY_ADDRESS,
            AnswerAuthor::lawyer(None),
        )
        .await
        .unwrap();
        // After address: should be person__client.
        match current_step(&surreal, &runtime, None, id).await.unwrap() {
            NextStep::NeedsAnswer { question } => {
                assert_eq!(question.code, "person__client");
            }
            NextStep::QuestionnaireComplete => {
                panic!("expected NeedsAnswer(person__client), got QuestionnaireComplete");
            }
        }
        answer_step(
            &surreal,
            &runtime,
            None,
            id,
            "person__client",
            "Libra",
            AnswerAuthor::lawyer(None),
        )
        .await
        .unwrap();
        // After one answer: should be person__lawyer_dri.
        match current_step(&surreal, &runtime, None, id).await.unwrap() {
            NextStep::NeedsAnswer { question } => {
                assert_eq!(question.code, "person__lawyer_dri");
            }
            NextStep::QuestionnaireComplete => {
                panic!("expected NeedsAnswer(person__lawyer_dri), got QuestionnaireComplete");
            }
        }
    }

    #[tokio::test]
    async fn current_step_for_unknown_notation_is_notation_not_found() {
        let surreal = db().await;
        let runtime = InMemoryRuntime::new();
        let err = current_step(&surreal, &runtime, None, Uuid::nil())
            .await
            .unwrap_err();
        assert!(matches!(err, NotationSessionError::NotationNotFound(_)));
    }

    #[tokio::test]
    async fn answer_step_persists_the_answer_row() {
        let surreal = db().await;
        seed_retainer_template(&surreal).await;
        seed_retainer_questions(&surreal).await;
        let person_id = seed_person(&surreal, "libra@example.com").await;
        let runtime = InMemoryRuntime::new();
        let started = start_notation(
            &surreal,
            &runtime,
            None,
            "onboarding__letter",
            person_id,
            seed_project(&surreal).await,
            None,
        )
        .await
        .unwrap();
        // The questionnaire now leads with entity and address; walk those
        // first so the runtime is positioned at person__client.
        answer_step(
            &surreal,
            &runtime,
            None,
            started.notation_id,
            "entity",
            TEST_ENTITY_NAME,
            AnswerAuthor::lawyer(None),
        )
        .await
        .unwrap();
        answer_step(
            &surreal,
            &runtime,
            None,
            started.notation_id,
            "address__principal_office",
            TEST_ENTITY_ADDRESS,
            AnswerAuthor::lawyer(None),
        )
        .await
        .unwrap();
        // The client self-answered this one through the magic link, so
        // the row must record both the source and the typist.
        answer_step(
            &surreal,
            &runtime,
            None,
            started.notation_id,
            "person__client",
            "Libra",
            AnswerAuthor::client(Some(person_id)),
        )
        .await
        .unwrap();

        let q = store::questions::find_by_code(&surreal, "person")
            .await
            .unwrap()
            .unwrap();
        let rows = store::answers::for_question_and_person(
            &surreal,
            q.id,
            person_id,
            Some("person__client"),
        )
        .await
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(store::answers::display_value(&rows[0].value), "Libra");
        assert_eq!(rows[0].notation_id, Some(started.notation_id));
        assert_eq!(rows[0].state_name.as_deref(), Some("person__client"));
        // person_id is the respondent; source + authored_by record who
        // actually entered it.
        assert_eq!(rows[0].source, SOURCE_CLIENT);
        assert_eq!(rows[0].authored_by_person_id, Some(person_id));
    }

    #[tokio::test]
    async fn lawyer_entered_answer_records_lawyer_source() {
        let surreal = db().await;
        seed_retainer_template(&surreal).await;
        seed_retainer_questions(&surreal).await;
        let client_id = seed_person(&surreal, "libra@example.com").await;
        let lawyer_id = seed_person(&surreal, "lawyer@neonlaw.com").await;
        let runtime = InMemoryRuntime::new();
        let started = start_notation(
            &surreal,
            &runtime,
            None,
            "onboarding__letter",
            client_id,
            seed_project(&surreal).await,
            None,
        )
        .await
        .unwrap();
        // The questionnaire now leads with entity and address; walk those
        // first so the runtime is positioned at person__client.
        answer_step(
            &surreal,
            &runtime,
            None,
            started.notation_id,
            "entity",
            TEST_ENTITY_NAME,
            AnswerAuthor::lawyer(Some(lawyer_id)),
        )
        .await
        .unwrap();
        answer_step(
            &surreal,
            &runtime,
            None,
            started.notation_id,
            "address__principal_office",
            TEST_ENTITY_ADDRESS,
            AnswerAuthor::lawyer(Some(lawyer_id)),
        )
        .await
        .unwrap();
        // Lawyer types the client's answer on their behalf: the respondent
        // is the client, the typist is lawyer, the source is `lawyer`.
        answer_step(
            &surreal,
            &runtime,
            None,
            started.notation_id,
            "person__client",
            "Libra",
            AnswerAuthor::lawyer(Some(lawyer_id)),
        )
        .await
        .unwrap();
        let row = store::answers::latest_for_person(&surreal, client_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.person_id, client_id);
        assert_eq!(row.source, SOURCE_LAWYER);
        assert_eq!(row.authored_by_person_id, Some(lawyer_id));
    }

    use super::{client_intake_step, record_client_answer, ClientIntakeStep};

    /// Start a retainer notation whose questions carry the shipped
    /// audiences, returning `(notation_id, respondent_id)`.
    async fn start_audienced_retainer(
        surreal: &SurrealDb,
        runtime: &InMemoryRuntime,
    ) -> (Uuid, Uuid) {
        seed_retainer_template(surreal).await;
        seed_retainer_questions_with_audiences(surreal).await;
        let person_id = seed_person(surreal, "libra@example.com").await;
        let started = start_notation(
            surreal,
            runtime,
            None,
            "onboarding__letter",
            person_id,
            seed_project(surreal).await,
            None,
        )
        .await
        .unwrap();
        (started.notation_id, person_id)
    }

    async fn start_audienced_retainer_for_person(
        surreal: &SurrealDb,
        runtime: &InMemoryRuntime,
        person_id: Uuid,
    ) -> Uuid {
        start_notation(
            surreal,
            runtime,
            None,
            "onboarding__letter",
            person_id,
            seed_project(surreal).await,
            None,
        )
        .await
        .unwrap()
        .notation_id
    }

    async fn seed_client_question(surreal: &SurrealDb, code: &str, answer_type: &str) -> Uuid {
        store::questions::create(
            surreal,
            &store::questions::NewQuestion::new(code, format!("Prompt for {code}"), answer_type)
                .with_audience(store::questions::AUDIENCE_BOTH),
        )
        .await
        .unwrap()
        .id
    }

    async fn start_snapshot_notation(
        surreal: &SurrealDb,
        yaml: &str,
        prompts: BTreeMap<String, String>,
    ) -> (Uuid, Uuid) {
        let template_row = store::templates::save_version(
            surreal,
            None,
            &format!("state_scope_test_{}", Uuid::now_v7()),
            store::templates::Version {
                title: "State scope test".into(),
                respondent_type: "person".into(),
                asset_id: None,
                form_code: None,
                kind: None,
                source_commit_sha: None,
            },
        )
        .await
        .unwrap()
        .into_model();
        let person_id = seed_person(surreal, "state-scope@example.com").await;
        let definition = QuestionnaireDefinition {
            spec: questionnaire_spec_from_yaml(yaml).unwrap(),
            prompts,
            audiences: BTreeMap::new(),
            choices: BTreeMap::new(),
        };
        let notation_id = store::notations::create(
            surreal,
            &store::notations::NewNotation::new(
                template_row.id,
                person_id,
                seed_project(surreal).await,
                StateName::BEGIN,
            )
            .with_questionnaire_snapshot(serde_json::to_value(&definition).unwrap()),
        )
        .await
        .unwrap()
        .id;
        (notation_id, person_id)
    }

    async fn insert_answer(
        surreal: &SurrealDb,
        notation_id: Uuid,
        person_id: Uuid,
        question_id: Uuid,
        state_name: Option<&str>,
        value: &str,
        source: &str,
    ) {
        let mut new = store::answers::NewAnswer::new(
            question_id,
            person_id,
            store::answers::primitive(value),
        )
        .authored_by(source, None);
        new.notation_id = Some(notation_id);
        new.state_name = state_name.map(ToString::to_string);
        store::answers::record(surreal, &new).await.unwrap();
    }

    #[tokio::test]
    async fn client_intake_walks_only_client_facing_questions() {
        let surreal = db().await;
        let runtime = InMemoryRuntime::new();
        let (id, person) = start_audienced_retainer(&surreal, &runtime).await;

        // Only the client-facing person question is offered — entity and
        // address__principal_office are lawyer-only (the Firm resolves the
        // client entity and its address at matter-open, not the client).
        let step = client_intake_step(&surreal, None, id).await.unwrap();
        let ClientIntakeStep::NeedsAnswer {
            question,
            position,
            total,
            ..
        } = step
        else {
            panic!("expected NeedsAnswer(person__client)");
        };
        assert_eq!(question.code, "person__client");
        assert_eq!((position, total), (1, 1));

        record_client_answer(&surreal, None, id, "person__client", "Libra", person)
            .await
            .unwrap();
        // The lawyer-only entity / address / project / product-description
        // states are never offered to the client.
        assert!(matches!(
            client_intake_step(&surreal, None, id).await.unwrap(),
            ClientIntakeStep::Complete { total: 1 }
        ));
    }

    #[tokio::test]
    async fn client_intake_progress_is_scoped_to_notation() {
        let surreal = db().await;
        let runtime = InMemoryRuntime::new();
        seed_retainer_template(&surreal).await;
        seed_retainer_questions_with_audiences(&surreal).await;
        let person = seed_person(&surreal, "libra@example.com").await;
        let first_id = start_audienced_retainer_for_person(&surreal, &runtime, person).await;
        let second_id = start_audienced_retainer_for_person(&surreal, &runtime, person).await;

        record_client_answer(&surreal, None, first_id, "person__client", "Libra", person)
            .await
            .unwrap();
        let step = client_intake_step(&surreal, None, second_id).await.unwrap();
        let ClientIntakeStep::NeedsAnswer {
            question,
            prior_value,
            position,
            total,
        } = step
        else {
            panic!("expected second notation to still need person__client");
        };
        assert_eq!(question.code, "person__client");
        assert_eq!(prior_value, None);
        assert_eq!((position, total), (1, 1));
    }

    #[tokio::test]
    async fn lawyer_prefilled_answer_shows_and_is_editable() {
        let surreal = db().await;
        let runtime = InMemoryRuntime::new();
        let (id, person) = start_audienced_retainer(&surreal, &runtime).await;

        // Lawyer walks the two leading questions first, then fills
        // client_name on the client's behalf.
        answer_step(
            &surreal,
            &runtime,
            None,
            id,
            "entity",
            TEST_ENTITY_NAME,
            AnswerAuthor::lawyer(None),
        )
        .await
        .unwrap();
        answer_step(
            &surreal,
            &runtime,
            None,
            id,
            "address__principal_office",
            TEST_ENTITY_ADDRESS,
            AnswerAuthor::lawyer(None),
        )
        .await
        .unwrap();
        answer_step(
            &surreal,
            &runtime,
            None,
            id,
            "person__client",
            "Lawyer-typed Libra",
            AnswerAuthor::lawyer(None),
        )
        .await
        .unwrap();

        // The client sees that lawyer answer pre-filled and editable —
        // client_name is still *their* step because they haven't answered
        // it themselves yet.
        let step = client_intake_step(&surreal, None, id).await.unwrap();
        let ClientIntakeStep::NeedsAnswer {
            question,
            prior_value,
            ..
        } = step
        else {
            panic!("expected NeedsAnswer(person__client) pre-filled");
        };
        assert_eq!(question.code, "person__client");
        assert_eq!(prior_value.as_deref(), Some("Lawyer-typed Libra"));

        // The client corrects it; the latest answer (client-sourced) wins.
        record_client_answer(&surreal, None, id, "person__client", "Libra Prime", person)
            .await
            .unwrap();
        let latest = store::answers::latest_for_person(&surreal, person)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(store::answers::display_value(&latest.value), "Libra Prime");
        assert_eq!(latest.notation_id, Some(id));
        assert_eq!(latest.source, SOURCE_CLIENT);
    }

    #[tokio::test]
    async fn client_intake_duplicate_custom_states_prefill_by_state_name() {
        let surreal = db().await;
        let custom_q = seed_client_question(&surreal, "custom_text", "string").await;
        let (id, person) = start_snapshot_notation(
            &surreal,
            "questionnaire:\n  BEGIN:\n    _: custom_text__mission_statement\n  \
             custom_text__mission_statement:\n    _: custom_text__revenue_strategy\n  \
             custom_text__revenue_strategy:\n    _: END\n  END: {}\n",
            BTreeMap::from([
                (
                    "mission_statement".to_string(),
                    "Mission statement?".to_string(),
                ),
                (
                    "revenue_strategy".to_string(),
                    "Revenue strategy?".to_string(),
                ),
            ]),
        )
        .await;
        insert_answer(
            &surreal,
            id,
            person,
            custom_q,
            Some("custom_text__mission_statement"),
            "Expand legal access",
            SOURCE_LAWYER,
        )
        .await;
        insert_answer(
            &surreal,
            id,
            person,
            custom_q,
            Some("custom_text__revenue_strategy"),
            "Flat-fee retainers",
            SOURCE_LAWYER,
        )
        .await;

        let step = client_intake_step(&surreal, None, id).await.unwrap();
        let ClientIntakeStep::NeedsAnswer {
            question,
            prior_value,
            position,
            total,
        } = step
        else {
            panic!("expected mission statement question");
        };
        assert_eq!(question.code, "custom_text__mission_statement");
        assert_eq!(prior_value.as_deref(), Some("Expand legal access"));
        assert_eq!((position, total), (1, 2));

        record_client_answer(
            &surreal,
            None,
            id,
            "custom_text__mission_statement",
            "Client-approved mission",
            person,
        )
        .await
        .unwrap();
        let step = client_intake_step(&surreal, None, id).await.unwrap();
        let ClientIntakeStep::NeedsAnswer {
            question,
            prior_value,
            position,
            total,
        } = step
        else {
            panic!("expected revenue strategy question");
        };
        assert_eq!(question.code, "custom_text__revenue_strategy");
        assert_eq!(prior_value.as_deref(), Some("Flat-fee retainers"));
        assert_eq!((position, total), (2, 2));
    }

    #[tokio::test]
    async fn client_intake_duplicate_entity_states_prefill_by_state_name() {
        let surreal = db().await;
        let entity_q = seed_client_question(&surreal, "entity", "entity").await;
        let (id, person) = start_snapshot_notation(
            &surreal,
            "questionnaire:\n  BEGIN:\n    _: entity__company\n  \
             entity__company:\n    _: entity__subsidiary\n  \
             entity__subsidiary:\n    _: END\n  END: {}\n",
            BTreeMap::from([
                ("company".to_string(), "Company?".to_string()),
                ("subsidiary".to_string(), "Subsidiary?".to_string()),
            ]),
        )
        .await;
        insert_answer(
            &surreal,
            id,
            person,
            entity_q,
            Some("entity__company"),
            "Neon Law LLC",
            SOURCE_LAWYER,
        )
        .await;
        insert_answer(
            &surreal,
            id,
            person,
            entity_q,
            Some("entity__subsidiary"),
            "Neon Law Labs LLC",
            SOURCE_LAWYER,
        )
        .await;

        let step = client_intake_step(&surreal, None, id).await.unwrap();
        let ClientIntakeStep::NeedsAnswer {
            question,
            prior_value,
            ..
        } = step
        else {
            panic!("expected entity__company");
        };
        assert_eq!(question.code, "entity__company");
        assert_eq!(prior_value.as_deref(), Some("Neon Law LLC"));

        record_client_answer(
            &surreal,
            None,
            id,
            "entity__company",
            "Neon Law LLC",
            person,
        )
        .await
        .unwrap();
        let step = client_intake_step(&surreal, None, id).await.unwrap();
        let ClientIntakeStep::NeedsAnswer {
            question,
            prior_value,
            position,
            total,
        } = step
        else {
            panic!("expected entity__subsidiary");
        };
        assert_eq!(question.code, "entity__subsidiary");
        assert_eq!(prior_value.as_deref(), Some("Neon Law Labs LLC"));
        assert_eq!((position, total), (2, 2));
    }

    #[tokio::test]
    async fn client_intake_null_state_prefill_is_legacy_bare_code_fallback() {
        let surreal = db().await;
        let custom_q = seed_client_question(&surreal, "custom_text", "string").await;
        let (id, person) = start_snapshot_notation(
            &surreal,
            "questionnaire:\n  BEGIN:\n    _: custom_text__mission_statement\n  \
             custom_text__mission_statement:\n    _: custom_text__revenue_strategy\n  \
             custom_text__revenue_strategy:\n    _: END\n  END: {}\n",
            BTreeMap::new(),
        )
        .await;
        insert_answer(
            &surreal,
            id,
            person,
            custom_q,
            None,
            "Legacy bare-code value",
            SOURCE_LAWYER,
        )
        .await;

        let step = client_intake_step(&surreal, None, id).await.unwrap();
        let ClientIntakeStep::NeedsAnswer {
            question,
            prior_value,
            ..
        } = step
        else {
            panic!("expected first custom_text state");
        };
        assert_eq!(question.code, "custom_text__mission_statement");
        assert_eq!(prior_value.as_deref(), Some("Legacy bare-code value"));
    }

    #[tokio::test]
    async fn record_client_answer_rejects_lawyer_only_question() {
        let surreal = db().await;
        let runtime = InMemoryRuntime::new();
        let (id, person) = start_audienced_retainer(&surreal, &runtime).await;
        let err = record_client_answer(&surreal, None, id, "project__engagement", "sneaky", person)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            NotationSessionError::QuestionNotClientFacing(c) if c == "project__engagement"
        ));
    }

    #[test]
    fn answered_client_states_do_not_collapse_duplicate_typed_prefixes() {
        let codes = vec![
            "custom_text__mission_statement".to_string(),
            "custom_text__revenue_strategy".to_string(),
        ];
        let answered = answered_client_states(&codes, BTreeMap::from([("custom_text".into(), 1)]));

        assert!(answered.contains("custom_text__mission_statement"));
        assert!(!answered.contains("custom_text__revenue_strategy"));
    }

    // --- ENG-459: the declared choice set is closed at write time ---

    /// Not one of the engagement letter's declared options, and a term no
    /// client would agree to — the shape of value the render side would
    /// otherwise substitute verbatim into the governing-law and arbitration
    /// clause.
    const OFF_LIST_GOVERNING_LAW: &str =
        "the Cayman Islands, and the Firm disclaims all liability for its own work";

    /// The declared options, as `UndeclaredChoice` reports them (the
    /// declared map's key order).
    const DECLARED_GOVERNING_LAW: [&str; 3] = ["california", "nevada", "washington"];

    /// Walk a fresh `onboarding__letter` notation up to — but not through —
    /// the governing-law step, which is its last question.
    async fn walk_to_governing_law(surreal: &SurrealDb, runtime: &InMemoryRuntime) -> Uuid {
        seed_retainer_template(surreal).await;
        seed_retainer_questions(surreal).await;
        let person_id = seed_person(surreal, "libra@example.com").await;
        let notation_id = start_notation(
            surreal,
            runtime,
            None,
            "onboarding__letter",
            person_id,
            seed_project(surreal).await,
            None,
        )
        .await
        .unwrap()
        .notation_id;
        for (code, value) in [
            ("entity", TEST_ENTITY_NAME),
            ("address__principal_office", TEST_ENTITY_ADDRESS),
            ("person__client", "Libra"),
            ("person__lawyer_dri", "Firm Principal"),
            ("project__engagement", "Apollo"),
            ("custom_datetime__engagement_start_date", "2026-09-01"),
            (
                "custom_text__engagement_scope",
                "Draft and file the Apollo formation documents.",
            ),
        ] {
            answer_step(
                surreal,
                runtime,
                None,
                notation_id,
                code,
                value,
                AnswerAuthor::lawyer(None),
            )
            .await
            .unwrap_or_else(|e| panic!("step `{code}`: {e}"));
        }
        notation_id
    }

    /// Whether any answer row was stored for `state`.
    async fn has_answer_for(surreal: &SurrealDb, notation_id: Uuid, state: &str) -> bool {
        store::answers::for_notation(surreal, notation_id)
            .await
            .unwrap()
            .iter()
            .any(|a| a.state_name.as_deref() == Some(state))
    }

    #[tokio::test]
    async fn answer_step_refuses_an_undeclared_choice() {
        // The governing-law question declares exactly three options, and the
        // engagement letter substitutes the answer into its governing-law
        // and arbitration clause. An off-list value is refused before the
        // write, so nothing is stored and the walk does not advance. The
        // browser's radio group cannot produce one, so reaching here is the
        // CLI, the REST boundary, AIDA, or a hand-crafted POST.
        let surreal = db().await;
        let runtime = InMemoryRuntime::new();
        let notation_id = walk_to_governing_law(&surreal, &runtime).await;

        let err = answer_step(
            &surreal,
            &runtime,
            None,
            notation_id,
            "custom_single_choice__governing_law",
            OFF_LIST_GOVERNING_LAW,
            AnswerAuthor::lawyer(None),
        )
        .await
        .unwrap_err();

        match &err {
            NotationSessionError::UndeclaredChoice { state, declared } => {
                assert_eq!(state, "custom_single_choice__governing_law");
                assert_eq!(declared.as_slice(), &DECLARED_GOVERNING_LAW);
            }
            other => panic!("expected UndeclaredChoice, got {other:?}"),
        }

        assert!(
            !has_answer_for(&surreal, notation_id, "custom_single_choice__governing_law").await,
            "the refused answer must not be on record"
        );

        // The walk still asks the same question, so a legitimate answer can
        // still be given — a refusal does not strand a part-finished walk.
        match current_step(&surreal, &runtime, None, notation_id)
            .await
            .unwrap()
        {
            NextStep::NeedsAnswer { question } => {
                assert_eq!(question.code, "custom_single_choice__governing_law");
            }
            NextStep::QuestionnaireComplete => {
                panic!("a refused answer must not complete the questionnaire")
            }
        }
    }

    #[tokio::test]
    async fn answer_step_accepts_every_declared_choice() {
        // The guard closes the set without narrowing it: each declared
        // option is still accepted and still completes the walk.
        for declared in DECLARED_GOVERNING_LAW {
            let surreal = db().await;
            let runtime = InMemoryRuntime::new();
            let notation_id = walk_to_governing_law(&surreal, &runtime).await;
            let next = answer_step(
                &surreal,
                &runtime,
                None,
                notation_id,
                "custom_single_choice__governing_law",
                declared,
                AnswerAuthor::lawyer(None),
            )
            .await
            .unwrap_or_else(|e| panic!("`{declared}` must be accepted: {e}"));
            assert!(
                matches!(next, NextStep::QuestionnaireComplete),
                "`{declared}` must complete the walk"
            );
        }
    }

    #[tokio::test]
    async fn a_state_with_no_declared_choices_is_left_open() {
        // The same walk's free-text and date questions declare no options,
        // so the guard must not touch them: "no declared set" means "not a
        // choice question", not "no valid options".
        let surreal = db().await;
        let runtime = InMemoryRuntime::new();
        let notation_id = walk_to_governing_law(&surreal, &runtime).await;
        assert!(
            has_answer_for(&surreal, notation_id, "custom_text__engagement_scope").await,
            "the free-text scope answer must still be recorded"
        );
        assert!(
            has_answer_for(
                &surreal,
                notation_id,
                "custom_datetime__engagement_start_date"
            )
            .await,
            "the date answer must still be recorded"
        );
    }

    #[test]
    fn multiple_choice_answers_are_left_open() {
        use super::stores_one_declared_choice;
        // A `custom_multiple_choice` answer is a set of keys, not one key,
        // so closing it against a single-key lookup would reject a
        // legitimate multi-value answer. It needs the checkbox-group field
        // shape ENG-454 tracks, and is deliberately left open here.
        assert!(stores_one_declared_choice(
            "custom_single_choice__governing_law"
        ));
        assert!(stores_one_declared_choice("custom_yes_no__has_counsel"));
        assert!(!stores_one_declared_choice(
            "custom_multiple_choice__practice_areas"
        ));
        // A record/reference pick is closed against the database candidate
        // list where the pick resolves, not against YAML choices.
        assert!(!stores_one_declared_choice("country__of_birth"));
        assert!(!stores_one_declared_choice("person__client"));
    }

    #[test]
    fn undeclared_choice_never_carries_the_submitted_value() {
        // The error is returned to callers and logged. A submitted answer is
        // client content, so it must appear in neither the message nor the
        // debug form — only the state and the firm-authored options do.
        let err = NotationSessionError::UndeclaredChoice {
            state: "custom_single_choice__governing_law".to_string(),
            declared: DECLARED_GOVERNING_LAW.map(String::from).to_vec(),
        };
        let shown = format!("{err}");
        let debugged = format!("{err:?}");
        assert!(!shown.contains(OFF_LIST_GOVERNING_LAW), "{shown}");
        assert!(!debugged.contains(OFF_LIST_GOVERNING_LAW), "{debugged}");
        assert!(shown.contains("nevada"), "{shown}");
    }

    #[tokio::test]
    async fn record_reask_answer_refuses_an_undeclared_choice() {
        // A post-review correction is what the attorney re-reviews and what
        // ultimately gets signed, so it is closed against the same declared
        // set the original answer was.
        let surreal = db().await;
        let runtime = InMemoryRuntime::new();
        let notation_id = walk_to_governing_law(&surreal, &runtime).await;
        answer_step(
            &surreal,
            &runtime,
            None,
            notation_id,
            "custom_single_choice__governing_law",
            "nevada",
            AnswerAuthor::lawyer(None),
        )
        .await
        .unwrap();
        let lawyer = store::test_support::dri_person(&surreal).await;
        store::reask::record_change_request(
            &surreal,
            notation_id,
            lawyer,
            &["custom_single_choice__governing_law".into()],
            Some("confirm the governing law"),
        )
        .await
        .unwrap();

        let err = record_reask_answer(
            &surreal,
            notation_id,
            "custom_single_choice__governing_law",
            OFF_LIST_GOVERNING_LAW,
            None,
            AnswerAuthor::lawyer(Some(lawyer)),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(
                &err,
                NotationSessionError::UndeclaredChoice { state, .. }
                    if state == "custom_single_choice__governing_law"
            ),
            "{err:?}"
        );

        // A declared correction still lands.
        record_reask_answer(
            &surreal,
            notation_id,
            "custom_single_choice__governing_law",
            "california",
            None,
            AnswerAuthor::lawyer(Some(lawyer)),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn record_extracted_answer_skips_an_undeclared_proposal() {
        // A transcript proposal is read back by the render exactly like a
        // typed answer, so an off-list extraction is refused. It is skipped
        // (reported as uncovered) rather than an error, so the lawyer is
        // asked the question instead of shown an option the questionnaire
        // never offered.
        use super::record_extracted_answer;
        let surreal = db().await;
        let runtime = InMemoryRuntime::new();
        let notation_id = walk_to_governing_law(&surreal, &runtime).await;

        let stored = record_extracted_answer(
            &surreal,
            None,
            notation_id,
            "custom_single_choice__governing_law",
            OFF_LIST_GOVERNING_LAW,
        )
        .await
        .unwrap();
        assert!(!stored, "an off-list proposal must be skipped");
        assert!(
            !has_answer_for(&surreal, notation_id, "custom_single_choice__governing_law").await,
            "the skipped proposal must not be on record"
        );

        // A declared proposal is still recorded for the lawyer to confirm.
        let stored = record_extracted_answer(
            &surreal,
            None,
            notation_id,
            "custom_single_choice__governing_law",
            "washington",
        )
        .await
        .unwrap();
        assert!(stored, "a declared proposal must still be recorded");
    }
}
