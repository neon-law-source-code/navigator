//! One Project, one repository: scaffold and validation.
//!
//! A Project's repository is named for its Project code and holds two kinds of
//! source side by side — that Project's notation templates under `templates/`,
//! and its client portal under `portal/`. There is one layout and one command
//! for both, because there is one repository.
//!
//! ```text
//! <organization>/<project-code>
//! ├── .github/workflows/gate.yml
//! ├── portal/            # React + Vite; the client's portal
//! ├── templates/         # *.md notation blueprints
//! ├── AGENTS.md
//! ├── CLAUDE.md
//! ├── README.md
//! └── navigator.yaml     # the Project this repository declares
//! ```
//!
//! # Where the Project code comes from
//!
//! [`validate`] takes it from the repository name, and CI has that name as
//! `github.event.repository.name`. The mount is that name plus the literal
//! `portal`.
//!
//! A repository may **also** declare its Project in a root manifest, and that
//! manifest is part of the layout. So the code is derived in one place and
//! declared in another, and nothing makes the two agree. Every repository
//! shipping today aligns them by convention — `neon-law-staging/sample-litigation`
//! is named for the code it publishes under — but a repository named for
//! anything else would split them.
//! `store::sample_project::project_code_for` is what refuses a bundle declaring
//! a code other than the one it is published under, so a disagreement is
//! rejected rather than unrepresentable.
//!
//! Collapsing the two spellings to one file and one key, and deciding whether
//! the publish action should read the manifest instead of the repository name,
//! is an open decision. Until it lands both spellings stay allowed roots:
//! refusing either would fail a repository that is correct as shipped.
//!
//! # Exemptions live here, not per repository
//!
//! [`ALLOWED_ROOTS`] is a closed list, and that is the gate rather than an
//! inconvenience: a Project repository holds client-adjacent source, so
//! "anything unenumerated is refused" is what keeps generated documents,
//! exports, and credentials out of one. A legitimate new root is admitted
//! here — reviewed once, for every repository — never by a per-repository
//! exemption file, which would make the gate advisory and let a repository
//! quietly exempt the thing the gate exists to catch.
//!
//! # The scaffold does not write the portal
//!
//! It writes the repository shell and the templates half. `portal/` arrives
//! from the vibe-coding lane, which is what knows how to make a Vite
//! application and which released `@neon-law/ux` to pin. That keeps [`validate`]
//! unambiguous: `portal/` present means there is a portal to hold to the Vite
//! contract, and absent means this Project does not have one yet.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The notation blueprints Navigator imports.
const TEMPLATE_DIRECTORY: &str = "templates";
/// The client portal's Vite workspace.
const PORTAL_DIRECTORY: &str = "portal";
const WORKFLOW: &str = ".github/workflows/gate.yml";
/// The manifest a Project repository declares its Project code in.
const PROJECT_MANIFEST: &str = "navigator.yaml";
/// Seed-shaped YAML documents for `navigator db seed`, one file per model.
const SEED_DIRECTORY: &str = "seeds";
const ALLOWED_ROOTS: &[&str] = &[
    ".github",
    ".gitignore",
    "AGENTS.md",
    "CLAUDE.md",
    // Every one of these repositories is proprietary, and a licence belongs at
    // the root where a reader looks for it. A portal-bearing repository could
    // hide one inside `portal/`; a templates-only Project has nowhere to put it
    // at all, so refusing it here made the layout unsatisfiable for that shape.
    "LICENSE.md",
    "README.md",
    "fixtures",
    // The manifest a Project repository declares its Project code in. Refusing
    // it made the layout unsatisfiable for every repository that carries one,
    // which is why the pinned validate action had to be pulled from all six
    // Project gates rather than the manifest being removed.
    //
    // Both spellings are admitted because both are live: the Project
    // repositories carry `navigator.yaml`, the sample-project bundles carry
    // `navigator.yml` (`store::sample_project::MANIFEST_FILE`). Collapsing them
    // to one file and one key is a separate decision; until it lands, refusing
    // either would fail a repository that is correct as shipped.
    PROJECT_MANIFEST,
    store::sample_project::MANIFEST_FILE,
    PORTAL_DIRECTORY,
    // A seed document names real people and real entities described by this
    // Project's matter — the input to a production write through `navigator
    // db seed`, not test scaffolding. That is the one distinction `fixtures/`
    // cannot carry, which is why seed documents get their own root rather
    // than filing under it: one file per model, using the standard
    // `lookup_fields` / `records` shape, and nothing generated.
    SEED_DIRECTORY,
    TEMPLATE_DIRECTORY,
    "tests",
];
const FORBIDDEN_COMPONENTS: &[&str] = &[
    "answers",
    "build",
    "client_uploads",
    "dependencies",
    "dist",
    "documents",
    "generated",
    "node_modules",
    "output",
    "secrets",
    "target",
    "uploads",
    "vendor",
];
const FORBIDDEN_CREDENTIAL_EXTENSIONS: &[&str] = &["env", "key", "pem", "p12", "pfx"];
const FORBIDDEN_DOCUMENT_EXTENSIONS: &[&str] = &["doc", "docx", "odt", "pdf"];

/// The files a Vite-built portal must have at the root of its directory.
///
/// Deliberately **no dependency allowlist**: third-party libraries are the
/// point of a Project carrying a Vite portal, so the contract is the build
/// shape, not the package list. A lockfile is required but its flavor is not —
/// a Project repository picks its own package manager, and Node never enters
/// the Navigator workspace.
const VITE_ENTRYPOINTS: &[&str] = &["package.json", "index.html"];
const VITE_LOCKFILES: &[&str] = &[
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lockb",
];

#[derive(Debug)]
struct Finding {
    path: PathBuf,
    message: String,
}

impl Finding {
    fn at(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

/// Create the reviewed scaffold without overwriting existing work.
pub fn scaffold(root: &Path, project_code: &str) -> ExitCode {
    if !store::projects::is_valid_code(project_code) {
        eprintln!(
            "navigator: invalid Project code `{project_code}`; use lowercase letters, digits, and single hyphens (80 characters maximum), and not a segment Navigator routes itself"
        );
        return ExitCode::from(2);
    }

    let files = [
        (root.join("README.md"), readme(project_code)),
        (root.join("AGENTS.md"), agents(project_code)),
        (root.join("CLAUDE.md"), agents(project_code)),
        (
            root.join(TEMPLATE_DIRECTORY).join("project_template.md"),
            example_template(),
        ),
        (root.join("tests/README.md"), tests_readme()),
        (root.join(WORKFLOW), workflow()),
    ];

    for (path, contents) in files {
        if path.exists() {
            println!("exists    {} (left alone)", path.display());
            continue;
        }
        if let Some(parent) = path.parent() {
            if let Err(error) = fs::create_dir_all(parent) {
                eprintln!("navigator: create {}: {error}", parent.display());
                return ExitCode::from(2);
            }
        }
        if let Err(error) = fs::write(&path, contents) {
            eprintln!("navigator: write {}: {error}", path.display());
            return ExitCode::from(2);
        }
        println!("created   {}", path.display());
    }

    println!(
        "\nValidate with: navigator projects repository validate {}",
        root.display()
    );
    ExitCode::SUCCESS
}

/// Validate one Project's repository.
///
/// Templates are intentionally passed to the rule engine under bare filenames:
/// they are Project blueprints, not members of Navigator's shared
/// `templates/neon_law` / `templates/forms` catalog. This mirrors
/// `store::template_source::persist_from_repo` exactly.
///
/// Three shapes are all valid — templates only, a portal only, or both — and a
/// repository carrying neither is reported distinctly rather than failed. A
/// Project may legitimately have opened before either half exists.
pub fn validate(root: &Path, repository: Option<&str>) -> ExitCode {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    if !root.is_dir() {
        eprintln!(
            "navigator: Project repository root is not a directory: {}",
            root.display()
        );
        return ExitCode::from(2);
    }

    // The repository name is the Project code. Nothing declares it, so nothing
    // can disagree with it — but a checkout named something a Project code
    // could never be is a checkout this validator cannot speak about.
    let code = repository_name(root, repository);
    if !store::projects::is_valid_code(&code) {
        errors.push(Finding::at(
            root,
            format!(
                "repository name `{code}` is not a valid Navigator Project code; \
                 the repository name *is* the code"
            ),
        ));
    }

    let has_templates = root.join(TEMPLATE_DIRECTORY).is_dir();
    let has_portal = root.join(PORTAL_DIRECTORY).is_dir();

    validate_layout(root, &mut errors);
    let templates = if has_templates {
        validate_templates(root, &mut errors, &mut warnings)
    } else {
        0
    };
    if has_portal {
        validate_portal(root, &mut errors);
    }
    if !has_templates && !has_portal {
        println!(
            "note: {code} carries neither `{TEMPLATE_DIRECTORY}/` nor `{PORTAL_DIRECTORY}/` yet"
        );
    }

    for warning in &warnings {
        println!("{}: warning: {}", warning.path.display(), warning.message);
    }
    for error in &errors {
        eprintln!("{}: error: {}", error.path.display(), error.message);
    }
    println!(
        "Validated Project repository `{code}`: {templates} template(s), {} portal, {} error(s), {} warning(s)",
        if has_portal { "1" } else { "0" },
        errors.len(),
        warnings.len()
    );

    if errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn repository_name(root: &Path, explicit: Option<&str>) -> String {
    if let Some(name) = explicit.map(str::trim).filter(|name| !name.is_empty()) {
        return name.rsplit('/').next().unwrap_or(name).to_string();
    }
    if let Ok(repository) = std::env::var("GITHUB_REPOSITORY") {
        if let Some(name) = repository
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
        {
            return name.to_string();
        }
    }
    root.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("<unknown>")
        .to_string()
}

fn validate_layout(root: &Path, errors: &mut Vec<Finding>) {
    if !root.join("README.md").is_file() {
        errors.push(Finding::at(
            root.join("README.md"),
            "missing required repository README",
        ));
    }

    let workflow_path = root.join(WORKFLOW);
    match fs::read_to_string(&workflow_path) {
        Ok(contents) => validate_workflow(&workflow_path, &contents, errors),
        Err(_) => errors.push(Finding::at(workflow_path, "missing required CI gate")),
    }

    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            // A portal's own build output and dependencies are forbidden by
            // name below; descending into them would report thousands of
            // findings for one mistake.
            entry.file_name() != ".git"
                && entry.file_name() != "node_modules"
                && entry.file_name() != "dist"
        })
    {
        let Ok(entry) = entry else {
            errors.push(Finding::at(root, "could not walk repository"));
            return;
        };
        let Ok(relative) = entry.path().strip_prefix(root) else {
            continue;
        };
        if relative.as_os_str().is_empty() {
            continue;
        }
        let components: Vec<String> = relative
            .components()
            .filter_map(|component| component.as_os_str().to_str().map(str::to_string))
            .collect();
        let Some(first) = components.first() else {
            continue;
        };
        if components.len() == 1 && !ALLOWED_ROOTS.contains(&first.as_str()) {
            errors.push(Finding::at(
                entry.path(),
                "path is outside the source-only Project repository layout",
            ));
        }
        if let Some(component) = components
            .iter()
            .find(|component| FORBIDDEN_COMPONENTS.contains(&component.as_str()))
        {
            errors.push(Finding::at(
                entry.path(),
                format!("forbidden `{component}` path; repositories hold source, never client material or build output"),
            ));
        }
        if entry.file_type().is_file() {
            let name = entry.file_name().to_string_lossy();
            let extension = entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or_default();
            if name == ".env" || name.starts_with(".env.") || name.starts_with("answers.") {
                errors.push(Finding::at(
                    entry.path(),
                    "client answers and environment secrets must not be committed",
                ));
            }
            if FORBIDDEN_CREDENTIAL_EXTENSIONS.contains(&extension) {
                errors.push(Finding::at(
                    entry.path(),
                    "credential material must not be committed",
                ));
            }
            if FORBIDDEN_DOCUMENT_EXTENSIONS.contains(&extension) {
                errors.push(Finding::at(
                    entry.path(),
                    "legal documents and rendered output must not be committed",
                ));
            }
        }
    }
}

/// The portal's build shape, where a portal exists.
fn validate_portal(root: &Path, errors: &mut Vec<Finding>) {
    let portal = root.join(PORTAL_DIRECTORY);
    let mut missing: Vec<&str> = VITE_ENTRYPOINTS
        .iter()
        .copied()
        .filter(|file| !portal.join(file).is_file())
        .collect();
    if !VITE_LOCKFILES
        .iter()
        .any(|file| portal.join(file).is_file())
    {
        missing.push("a lockfile");
    }
    if !missing.is_empty() {
        errors.push(Finding::at(
            &portal,
            format!(
                "`{PORTAL_DIRECTORY}/` is present but is not a Vite workspace: missing {}",
                missing.join(", ")
            ),
        ));
    }
}

/// The pinned validate action a Project repository's gate must call.
const VALIDATE_ACTION: &str = "neon-law-source-code/navigator/.github/actions/validate@";

/// Just enough of a workflow to find one step and read its inputs.
///
/// Deliberately permissive: every field is optional and unknown keys are
/// ignored, because this gate speaks about one step and must not fail on an
/// unrelated addition elsewhere in the file.
#[derive(serde::Deserialize)]
struct Workflow {
    #[serde(default)]
    jobs: BTreeMap<String, WorkflowJob>,
}

#[derive(serde::Deserialize)]
struct WorkflowJob {
    #[serde(default)]
    steps: Vec<WorkflowStep>,
}

#[derive(serde::Deserialize)]
struct WorkflowStep {
    #[serde(default)]
    uses: Option<String>,
    #[serde(default)]
    with: BTreeMap<String, serde_yaml::Value>,
}

/// A YAML scalar as the string a workflow input actually carries.
///
/// `version: "26.7.27"` is a string and `project_repository: true` is a bool,
/// but a caller may quote either, so both spellings have to read the same.
fn scalar(value: &serde_yaml::Value) -> Option<String> {
    match value {
        serde_yaml::Value::String(value) => Some(value.trim().to_string()),
        serde_yaml::Value::Number(value) => Some(value.to_string()),
        serde_yaml::Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

/// Hold the CI gate to calling Navigator's pinned validate action, at a release
/// tag matching the CLI version it downloads, in Project-repository mode.
///
/// # Read the step, not the lines
///
/// Every check here is anchored to **the one step that `uses` the validate
/// action**, and that is load-bearing rather than tidiness. Scanning raw lines
/// got all three wrong: matching `- uses: ` saw only a step whose first key was
/// `uses`, so an ordinarily labelled `- name:` step was reported absent while
/// calling the action on the next line; the first `version:` line anywhere won,
/// so the pnpm setup every portal repository runs first supplied its own
/// version as the CLI's; and `project_repository: true` was a substring search
/// a comment could satisfy. Parsing costs nothing and the three findings then
/// describe the step they name.
fn validate_workflow(path: &Path, contents: &str, errors: &mut Vec<Finding>) {
    let workflow: Workflow = match serde_yaml::from_str(contents) {
        Ok(workflow) => workflow,
        // A gate that does not parse is its own failure. Reporting it as a
        // missing action sends the reader hunting for a step that is right
        // there in front of them.
        Err(error) => {
            errors.push(Finding::at(
                path,
                format!("CI gate is not valid YAML: {error}"),
            ));
            return;
        }
    };

    let step = workflow
        .jobs
        .values()
        .flat_map(|job| &job.steps)
        .find(|step| {
            step.uses
                .as_deref()
                .is_some_and(|uses| uses.trim().starts_with(VALIDATE_ACTION))
        });
    let Some(step) = step else {
        errors.push(Finding::at(
            path,
            "CI gate must call Navigator's pinned validate action",
        ));
        return;
    };

    let action_version = step
        .uses
        .as_deref()
        .unwrap_or_default()
        .trim()
        .strip_prefix(VALIDATE_ACTION)
        .unwrap_or_default();
    let Some(input_version) = step.with.get("version").and_then(scalar) else {
        errors.push(Finding::at(
            path,
            "CI gate must pass the action's exact release tag as `version`",
        ));
        return;
    };

    if action_version != input_version {
        errors.push(Finding::at(
            path,
            format!(
                "validation action ref `{action_version}` must equal its downloaded CLI version `{input_version}`"
            ),
        ));
    }
    if !is_release_tag(action_version) {
        errors.push(Finding::at(
            path,
            format!(
                "validation action ref `{action_version}` must be an exact release version, such as YY.M.D or YY.M.D-hotfix.N"
            ),
        ));
    }
    if step
        .with
        .get("project_repository")
        .and_then(scalar)
        .as_deref()
        != Some("true")
    {
        errors.push(Finding::at(
            path,
            "CI gate must set `project_repository: true`",
        ));
    }
}

fn is_release_tag(version: &str) -> bool {
    crate::devx::registry::is_release_tag(version)
}

fn validate_templates(
    root: &Path,
    errors: &mut Vec<Finding>,
    warnings: &mut Vec<Finding>,
) -> usize {
    let directory = root.join(TEMPLATE_DIRECTORY);
    let mut paths = Vec::new();
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) => {
            errors.push(Finding::at(directory, format!("read templates: {error}")));
            return 0;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                errors.push(Finding::at(
                    &directory,
                    format!("read template entry: {error}"),
                ));
                continue;
            }
        };
        let path = entry.path();
        if path.is_dir() {
            errors.push(Finding::at(
                path,
                "Project templates must be direct `templates/<code>.md` files",
            ));
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("md") {
            paths.push(path);
        } else {
            errors.push(Finding::at(
                path,
                "only Markdown template blueprints belong in `templates/`",
            ));
        }
    }
    paths.sort();
    if paths.is_empty() {
        errors.push(Finding::at(
            directory,
            "`templates/` is present but empty; at least one `templates/<code>.md` blueprint is required",
        ));
        return 0;
    }

    let rules = rules::navigator_default_rules_with_codes(&rules::canonical_question_codes());
    let mut declared_codes = BTreeMap::new();
    for path in &paths {
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) => {
                errors.push(Finding::at(path, format!("read template: {error}")));
                continue;
            }
        };
        let filename = path.file_name().map_or_else(PathBuf::new, PathBuf::from);
        let source = rules::SourceFile {
            path: filename,
            contents: contents.clone(),
        };
        for violation in rules.iter().flat_map(|rule| rule.lint(&source)) {
            let finding = Finding::at(path, format!("{}: {}", violation.code, violation.message));
            if rules::severity_for_code(violation.code) == rules::Severity::Error {
                errors.push(finding);
            } else {
                warnings.push(finding);
            }
        }
        if let Some(code) = rules::frontmatter::extract(&contents)
            .and_then(|frontmatter| rules::frontmatter::field(frontmatter, "code"))
        {
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default();
            if code != stem {
                errors.push(Finding::at(
                    path,
                    format!("template `code` `{code}` must equal filename stem `{stem}`"),
                ));
            }
            if let Some(first) = declared_codes.insert(code.clone(), path.clone()) {
                errors.push(Finding::at(
                    path,
                    format!(
                        "duplicate template `code` `{code}`; first declared in {}",
                        first.display()
                    ),
                ));
            }
        }
    }
    paths.len()
}

fn readme(project_code: &str) -> String {
    format!(
        "# {project_code}\n\nThis repository holds source-only material for Project `{project_code}`: its notation\n\
         templates under `templates/`, and its client portal under `portal/`.\n\n\
         The repository name *is* the Project code. Nothing in here declares it, so nothing can\n\
         disagree with it: Navigator's portal mount is `/app/projects/{project_code}/portal/`, derived from\n\
         the repository name plus one literal segment.\n\n\
         Navigator imports each direct `templates/<code>.md` file at the current commit, preserving both\n\
         that commit SHA and the template body's content hash as provenance.\n\n\
         Do not commit client uploads, answers, generated documents, secrets, dependencies, or build\n\
         output. Legal files live in Drive and in Navigator's assets, never in Git.\n\n\
         Run `navigator projects repository validate .` before opening a pull request.\n"
    )
}

fn agents(project_code: &str) -> String {
    format!(
        "# Working in {project_code}\n\n\
         This is one Project's repository. It holds two kinds of source and nothing else.\n\n\
         * `templates/` — notation blueprints, one `templates/<code>.md` per notation. Navigator\n\
           imports them and records the commit SHA as provenance.\n\
         * `portal/` — the client's React + Vite portal. Build it for the base\n\
           `/app/projects/{project_code}/portal/`, and derive every in-app path from\n\
           `import.meta.env.BASE_URL` rather than writing an absolute path by hand: a Vite base\n\
           rewrites module and asset URLs and never an `href` in source.\n\n\
         Read matter data through Navigator's `/api` read surfaces and write through its one REST\n\
         command boundary. Do not add a second backend, and do not put a legal file, a client upload,\n\
         an answer, a generated document, or a secret in this repository.\n"
    )
}

fn tests_readme() -> String {
    "# Tests\n\nKeep source-level tests for this Project's templates here. Generated documents and dependencies do not belong here.\n"
        .to_string()
}

fn example_template() -> String {
    r"---
kind: letter
title: Project Template Placeholder
respondent_type: entity
code: project_template
confidential: false
jurisdiction: NV
questionnaire:
  BEGIN:
    _: END
  END: {}
workflow:
  BEGIN:
    _: lawyer_review
  lawyer_review:
    _: END
  END: {}
---

Replace this placeholder with the Project-specific template approved for import.
"
    .to_string()
}

/// The one CI gate, in the one job name `ops github setup` requires.
///
/// There is deliberately **no** `paths:` filter. A filtered job that skips
/// reports success for work it never did, and a required check that can be
/// satisfied by a skip is not a gate. So the job always runs and the action
/// no-ops internally over whichever half this repository carries.
/// A raw string, not a `\`-continued one. A backslash continuation strips the
/// leading whitespace of the next line, which silently reflows YAML into
/// something that no longer parses — and a generated workflow that does not
/// parse fails in the Project repository rather than here.
fn workflow() -> String {
    r#"name: ci

on:
  pull_request:
  push:
    branches: [main]

jobs:
  # The one required check. It always runs: the gate no-ops over a half this
  # repository does not carry, rather than being skipped by a path filter and
  # reporting success for a job that never ran.
  ci:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
      - uses: neon-law-source-code/navigator/.github/actions/validate@26.7.27
        with:
          version: "26.7.27"
          project_repository: true
"#
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        example_template, is_release_tag, repository_name, validate_layout, validate_workflow,
        workflow, Finding, ALLOWED_ROOTS, WORKFLOW,
    };
    use std::path::Path;

    /// The messages `validate_workflow` reports for one gate file.
    fn findings(contents: &str) -> Vec<String> {
        let mut errors: Vec<Finding> = Vec::new();
        validate_workflow(Path::new("gate.yml"), contents, &mut errors);
        errors.into_iter().map(|error| error.message).collect()
    }

    /// The smallest checkout `validate_layout` accepts, so a test adding one
    /// file measures that file and nothing else.
    fn scaffold_minimal(root: &Path) {
        std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
        std::fs::write(root.join("README.md"), "# fixture\n").unwrap();
        std::fs::write(root.join(WORKFLOW), workflow()).unwrap();
    }

    fn layout_findings(root: &Path) -> Vec<String> {
        let mut errors: Vec<Finding> = Vec::new();
        validate_layout(root, &mut errors);
        errors.into_iter().map(|error| error.message).collect()
    }

    #[test]
    fn repository_name_prefers_the_explicit_coordinate() {
        assert_eq!(
            repository_name(Path::new("/tmp/renamed"), Some("org/example")),
            "example"
        );
    }

    #[test]
    fn generated_template_has_a_stable_code() {
        assert!(example_template().contains("code: project_template"));
        assert!(is_release_tag("26.7.27"));
        assert!(is_release_tag("26.8.19-hotfix.14"));
        assert!(!is_release_tag("main"));
        assert!(!is_release_tag("26.8.19-hotfix."));
        // The legacy four-component spelling is not a version Cargo can parse,
        // so no release has been able to carry it since the tag started coming
        // from `[workspace.package].version`.
        assert!(!is_release_tag("26.7.27.4"));
    }

    /// The generated gate is one always-running required job.
    ///
    /// A `paths:` filter here would let a required check pass by being skipped,
    /// so the job name matches the one `ops github setup` binds and the gate
    /// carries no filter at all.
    #[test]
    fn a_licence_at_the_root_is_part_of_the_layout() {
        // Every one of these repositories is proprietary. A templates-only
        // Project has no `portal/` to hide a licence inside, so refusing it at
        // the root made the layout unsatisfiable for that shape rather than
        // merely opinionated.
        assert!(ALLOWED_ROOTS.contains(&"LICENSE.md"));
    }

    /// A step labelled with `name:` before `uses:` is the ordinary way to write
    /// one, and the gate must see it. Matching on a line beginning `- uses: `
    /// only ever saw a step whose *first* key was `uses`, so a labelled step
    /// was reported absent while calling the action on the very next line —
    /// and the early return meant its version was never checked either.
    #[test]
    fn a_labelled_step_calls_the_action() {
        let contents = r#"name: ci
on: [pull_request]
jobs:
  ci:
    runs-on: ubuntu-latest
    steps:
      - name: Validate the Project repository
        uses: neon-law-source-code/navigator/.github/actions/validate@26.7.27
        with:
          version: "26.7.27"
          project_repository: true
"#;
        assert_eq!(findings(contents), Vec::<String>::new());
    }

    /// `version` belongs to the validate step, not to the file. Reading the
    /// first `version:` line anywhere meant an earlier action's own input won:
    /// every portal repository sets pnpm up before validating, so the standard
    /// layout reported a mismatch naming pnpm's version as the CLI's.
    #[test]
    fn an_earlier_actions_version_input_is_not_the_cli_version() {
        let contents = r#"name: ci
on: [pull_request]
jobs:
  ci:
    runs-on: ubuntu-latest
    steps:
      - uses: pnpm/action-setup@v4
        with:
          version: "9.1.0"
      - uses: neon-law-source-code/navigator/.github/actions/validate@26.7.27
        with:
          version: "26.7.27"
          project_repository: true
"#;
        assert_eq!(findings(contents), Vec::<String>::new());
    }

    /// The input has to be *passed*, not merely present in the file. A bare
    /// substring search was satisfied by a comment, or by another step.
    #[test]
    fn project_repository_must_be_passed_to_the_validate_step() {
        let contents = r#"name: ci
on: [pull_request]
jobs:
  ci:
    runs-on: ubuntu-latest
    steps:
      # project_repository: true
      - uses: other/action@v1
        with:
          project_repository: true
      - uses: neon-law-source-code/navigator/.github/actions/validate@26.7.27
        with:
          version: "26.7.27"
"#;
        assert_eq!(
            findings(contents),
            vec!["CI gate must set `project_repository: true`".to_string()]
        );
    }

    /// The fix must not stop checking. A genuine disagreement between the ref
    /// and the downloaded version is still the whole point of the gate.
    #[test]
    fn a_real_version_mismatch_is_still_caught() {
        let contents = r#"name: ci
on: [pull_request]
jobs:
  ci:
    runs-on: ubuntu-latest
    steps:
      - uses: neon-law-source-code/navigator/.github/actions/validate@26.7.27
        with:
          version: "26.7.26"
          project_repository: true
"#;
        let found = findings(contents);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].contains("must equal its downloaded CLI version"),
            "{found:?}"
        );
    }

    /// A moving ref is not a pin, so `@main` stays refused.
    #[test]
    fn a_moving_ref_is_still_refused() {
        let contents = r#"name: ci
on: [pull_request]
jobs:
  ci:
    runs-on: ubuntu-latest
    steps:
      - uses: neon-law-source-code/navigator/.github/actions/validate@main
        with:
          version: "main"
          project_repository: true
"#;
        let found = findings(contents);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].contains("must be an exact release version"),
            "{found:?}"
        );
    }

    /// A gate that does not parse is its own failure. Reporting it as a missing
    /// action sends the reader looking for a step that is right there.
    #[test]
    fn an_unparseable_gate_says_so() {
        let found = findings("name: ci\njobs: [oops\n");
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("not valid YAML"), "{found:?}");
    }

    /// The generator and the validator must agree. Nothing asserted this, which
    /// is how three defects lived in one twelve-line function.
    #[test]
    fn the_scaffolded_gate_passes_its_own_validation() {
        assert_eq!(findings(&workflow()), Vec::<String>::new());
    }

    /// Six Project repositories declare their Project in a root manifest. The
    /// layout gate refused it, which is why the pinned action had to be pulled
    /// from every one of their gates.
    #[test]
    fn the_project_manifest_is_part_of_the_layout() {
        assert!(ALLOWED_ROOTS.contains(&"navigator.yaml"));
        assert!(ALLOWED_ROOTS.contains(&"navigator.yml"));
    }

    /// `seeds/` is where a Project repository's `navigator db seed` documents
    /// belong. Refusing it left nowhere in the layout for real actors a
    /// matter names, and `fixtures/` is the wrong root: a fixture is invented
    /// or firm-owned, while a seed document is the input to a production
    /// write.
    #[test]
    fn the_seed_directory_is_part_of_the_layout() {
        assert!(ALLOWED_ROOTS.contains(&"seeds"));
    }

    #[test]
    fn a_checkout_carrying_seeds_has_no_layout_finding() {
        let root = tempfile::tempdir().unwrap();
        scaffold_minimal(root.path());
        std::fs::create_dir_all(root.path().join("seeds")).unwrap();
        std::fs::write(
            root.path().join("seeds/Person.yaml"),
            "lookup_fields:\n  - email\nrecords: []\n",
        )
        .unwrap();

        assert_eq!(layout_findings(root.path()), Vec::<String>::new());
    }

    #[test]
    fn a_checkout_carrying_the_manifest_has_no_layout_finding() {
        let root = tempfile::tempdir().unwrap();
        scaffold_minimal(root.path());
        std::fs::write(
            root.path().join("navigator.yaml"),
            "host: www.neonlaw.com\nproject: acme\n",
        )
        .unwrap();

        assert_eq!(layout_findings(root.path()), Vec::<String>::new());
    }

    /// Admitting the manifest must not make the closed list permissive: the
    /// point of `ALLOWED_ROOTS` is that anything unenumerated is refused.
    #[test]
    fn an_unlisted_root_is_still_refused() {
        let root = tempfile::tempdir().unwrap();
        scaffold_minimal(root.path());
        std::fs::write(root.path().join("notes.md"), "scratch\n").unwrap();

        assert_eq!(
            layout_findings(root.path()),
            vec!["path is outside the source-only Project repository layout".to_string()]
        );
    }

    /// The forbidden-path checks run independently of the allowed-root list, so
    /// admitting a new root cannot open a door for build output beside it.
    #[test]
    fn a_forbidden_component_still_wins() {
        let root = tempfile::tempdir().unwrap();
        scaffold_minimal(root.path());
        std::fs::write(root.path().join("navigator.yaml"), "project: acme\n").unwrap();
        std::fs::write(root.path().join(".env"), "SECRET=1\n").unwrap();

        let found = layout_findings(root.path());
        assert!(
            found
                .iter()
                .any(|finding| finding.contains("must not be committed")),
            "{found:?}"
        );
    }

    #[test]
    fn the_generated_gate_is_one_unfiltered_required_job() {
        let generated = workflow();
        assert!(generated.contains("\n  ci:\n"), "{generated}");
        assert!(
            !generated.contains("paths:"),
            "a path-filtered required check can be satisfied by a skip"
        );
        assert!(generated.contains("project_repository: true"));
    }
}
