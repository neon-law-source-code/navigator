//! `navigator notations run <FILE>` — drive a template through the real
//! questionnaire service against a private embedded store.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use cloud::StorageService;
use serde::Deserialize;
use workflows::{
    AnswerAuthor, InMemoryRuntime, NextStep, PostQuestionnaireDrive, StateMachineRuntime,
};

#[derive(Default, Deserialize)]
struct Frontmatter {
    code: Option<String>,
    title: Option<String>,
    respondent_type: Option<String>,
    kind: Option<String>,
    form: Option<String>,
    #[serde(default)]
    questionnaire: BTreeMap<String, BTreeMap<String, String>>,
}

/// Run `file` in a brand-new in-process database. This command deliberately
/// reads neither deployment configuration nor provider credentials.
// The terminal transcript follows the lifecycle in its execution order, so it
// remains reviewable against the client and lawyer runner services it invokes.
#[allow(clippy::too_many_lines)]
pub async fn run(file: &Path) -> Result<()> {
    let contents = std::fs::read_to_string(file)
        .with_context(|| format!("reading notation template {}", file.display()))?;
    let frontmatter = rules::frontmatter::extract(&contents)
        .ok_or_else(|| anyhow::anyhow!("template has no YAML frontmatter"))?;
    let fm: Frontmatter =
        serde_yaml::from_str(frontmatter).context("parsing template frontmatter")?;
    let code = fm
        .code
        .as_deref()
        .filter(|code| !code.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("template frontmatter must declare a non-empty `code`"))?;
    let source = rules::SourceFile {
        path: file.to_path_buf(),
        contents: contents.clone(),
    };
    let violations: Vec<_> = rules::navigator_default_rules_with_codes(&[])
        .iter()
        .flat_map(|rule| rule.lint(&source))
        .filter(|violation| rules::severity_for_code(violation.code) == rules::Severity::Error)
        .collect();
    if !violations.is_empty() {
        let errors = violations
            .iter()
            .map(|violation| format!("{}: {}", violation.code, violation.message))
            .collect::<Vec<_>>()
            .join("; ");
        bail!("template is invalid: {errors}");
    }

    let workspace = tempfile::tempdir().context("creating ephemeral notation workspace")?;
    let storage: Arc<dyn StorageService> = Arc::new(
        cloud::FsStorage::new(workspace.path().join("objects"))
            .await
            .context("creating ephemeral object store")?,
    );
    let surreal = store::surreal::test_support::mem().await;
    store::seed::seed_canonical(&surreal, &storage)
        .await
        .context("seeding the embedded notation catalog")?;

    let asset_id =
        store::assets::ingest_content(&surreal, &storage, contents.as_bytes(), "text/markdown")
            .await
            .context("persisting the template in the embedded store")?;
    register_questionnaire_questions(&surreal, &fm).await?;
    let template = store::templates::save_version(
        &surreal,
        None,
        &code,
        store::templates::Version {
            title: fm.title.unwrap_or_else(|| code.clone()),
            respondent_type: fm.respondent_type.unwrap_or_else(|| "person".into()),
            asset_id: Some(asset_id),
            form_code: fm.form,
            kind: fm.kind,
            source_commit_sha: None,
        },
    )
    .await
    .context("persisting the template version")?
    .into_model();
    let entity = store::entities::find_by_name(&surreal, store::seed::FIRM_ENTITY_NAME)
        .await
        .context("resolving synthetic project entity")?
        .ok_or_else(|| anyhow::anyhow!("canonical seed did not create its firm entity"))?;
    let client = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Run Client",
            "run-client@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .context("creating synthetic client")?;
    let lawyer = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Run Lawyer",
            "run-lawyer@example.com",
            store::persons::Role::Lawyer,
        ),
    )
    .await
    .context("creating synthetic lawyer")?;
    let project = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: format!("notation-run-{}", uuid::Uuid::now_v7()),
            name: "Ephemeral notation run".into(),
            status: "open".into(),
            entity_id: entity.id,
            ..Default::default()
        },
    )
    .await
    .context("creating synthetic project")?;
    store::projects::designate_dri_in_surreal(
        &surreal,
        project.id,
        client.id,
        store::projects::DriSide::Client,
    )
    .await
    .context("linking synthetic client")?;
    store::projects::designate_dri_in_surreal(
        &surreal,
        project.id,
        lawyer.id,
        store::projects::DriSide::Lawyer,
    )
    .await
    .context("linking synthetic lawyer")?;

    let runtime = Arc::new(InMemoryRuntime::new());
    let started = workflows::start_notation(
        &surreal,
        runtime.as_ref(),
        Some(&storage),
        &template.code,
        client.id,
        project.id,
        Some(entity.id),
    )
    .await
    .context("starting questionnaire")?;
    let notation_id = started.notation_id;
    println!("notation {notation_id}");

    let mut client_answers = 0;
    loop {
        let step =
            workflows::notation_session::client_intake_step(&surreal, Some(&storage), notation_id)
                .await
                .context("reading the client's questionnaire subset")?;
        let workflows::notation_session::ClientIntakeStep::NeedsAnswer {
            question,
            position,
            total,
            ..
        } = step
        else {
            break;
        };
        let value = synthetic_value(&question);
        println!("client {position}/{total} {}={value}", question.code);
        workflows::notation_session::record_client_answer(
            &surreal,
            Some(&storage),
            notation_id,
            &question.code,
            value,
            client.id,
        )
        .await
        .with_context(|| format!("recording the client's `{}` answer", question.code))?;
        client_answers += 1;
    }
    println!("client subset complete: {client_answers} answer(s)");

    let mut next = started.next;
    let mut firm_answers = 0;
    while let NextStep::NeedsAnswer { question } = next {
        let value = synthetic_value(&question);
        println!("firm {}={value}", question.code);
        next = workflows::answer_step_with_reference(
            &surreal,
            runtime.as_ref(),
            Some(&storage),
            notation_id,
            &question.code,
            value,
            None,
            AnswerAuthor::lawyer(Some(lawyer.id)),
        )
        .await
        .with_context(|| format!("answering `{}`", question.code))?;
        firm_answers += 1;
    }
    let drive = portal::retainer_walk::PostQuestionnaire {
        surreal: surreal.clone(),
        workflow_runtime: runtime.clone(),
        storage: storage.clone(),
        assets_storage: storage.clone(),
        forms_registry: Arc::new(forms::registry().context("loading forms registry")?),
    };
    let final_state = drive.begin(notation_id, Some(lawyer.id)).await?;
    let answers = store::answers::for_notation(&surreal, notation_id).await?;
    println!("firm questionnaire complete: {firm_answers} answer(s)");
    println!(
        "persisted {} answer(s); workflow state {final_state}",
        answers.len()
    );
    for event in StateMachineRuntime::events(
        runtime.as_ref(),
        workflows::MachineKind::Questionnaire,
        notation_id,
    )
    .await
    {
        println!(
            "questionnaire {} --{}--> {}",
            event.from.as_str(),
            event.condition,
            event.to.as_str()
        );
    }
    for event in StateMachineRuntime::events(
        runtime.as_ref(),
        workflows::MachineKind::Workflow,
        notation_id,
    )
    .await
    {
        println!(
            "workflow {} --{}--> {}",
            event.from.as_str(),
            event.condition,
            event.to.as_str()
        );
    }
    Ok(())
}

async fn register_questionnaire_questions(
    surreal: &store::surreal::SurrealDb,
    frontmatter: &Frontmatter,
) -> Result<()> {
    for state in frontmatter.questionnaire.keys() {
        if matches!(state.as_str(), "BEGIN" | "END") {
            continue;
        }
        let code = state
            .split_once("__")
            .map_or(state.as_str(), |(code, _)| code);
        store::questions::find_or_create(
            surreal,
            &store::questions::NewQuestion::new(code, format!("(ephemeral) {code}"), "string"),
        )
        .await
        .with_context(|| format!("registering questionnaire question `{code}`"))?;
    }
    Ok(())
}

fn synthetic_value(question: &workflows::QuestionDescriptor) -> &str {
    if let Some(choice) = question.choices.first() {
        return &choice.value;
    }
    match question.code.as_str() {
        "custom_datetime__engagement_start_date" => "2026-01-01",
        _ => "Sample",
    }
}
