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
//! ├── .github/workflows/publish.yml
//! ├── .claude/skills/    # synced from Navigator via `sync-skills`
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

use crate::devx::github_setup::REQUIRED_CHECK;

/// The notation blueprints Navigator imports.
const TEMPLATE_DIRECTORY: &str = "templates";
/// The client portal's Vite workspace.
const PORTAL_DIRECTORY: &str = "portal";
const WORKFLOW: &str = ".github/workflows/gate.yml";
const CD_WORKFLOW: &str = ".github/workflows/publish.yml";
/// The manifest a Project repository declares its Project code in.
/// The manifest a Project repository declares its Project in.
///
/// `pub(super)` rather than private because [`super::drift`] reads the same
/// file and must not spell it a second time: two constants for one filename is
/// a rename waiting to leave one of them stale, and the gate that *admits* the
/// file and the command that *reads* it are exactly the pair that must agree.
pub(super) const PROJECT_MANIFEST: &str = "navigator.yaml";
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
    // Navigator's own agent skills, synced in by `sync_skills` from this
    // binary's compiled-in copies (see [`SYNCED_SKILLS`]). Refusing it would
    // make the layout unsatisfiable for the thing ENG-383 exists to let a
    // Project repository carry: a Project's portal is client-facing legal
    // copy, and the skills synced here are the review councils that critique
    // exactly that.
    ".claude",
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

/// The skill catalog synced into every Project repository, embedded at build
/// time from Navigator's own canonical `.agents/skills/` — never read from a
/// live checkout at runtime, so a downloaded release binary can write it with
/// no wider clone. Kept intentionally small: every skill synced here is one
/// to keep true in every repository that carries it. A Project's portal is
/// client-facing legal copy — an engagement summary, a documents tab, a
/// matter timeline — so the two review councils that critique exactly that,
/// plus the general engineering council, are the initial set.
const SYNCED_SKILLS: &[(&str, &str)] = &[
    (
        "council",
        include_str!("../../../.agents/skills/council/SKILL.md"),
    ),
    (
        "legal-council",
        include_str!("../../../.agents/skills/legal-council/SKILL.md"),
    ),
    (
        "client-council",
        include_str!("../../../.agents/skills/client-council/SKILL.md"),
    ),
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

/// Create the reviewed scaffold without overwriting existing work, pinning the
/// generated gate to `action_version`.
///
/// The pin is refused here rather than in the generated file. A gate emitted at
/// `main`, at `latest`, or at a version this repository does not publish is a
/// gate the Project cannot run, and the operator learns that on the run that
/// blocks their first pull request rather than on the command that wrote it.
/// `docs/project-repositories.md` requires an exact immutable release tag, so
/// this is that rule enforced at the one place the file is written.
pub fn scaffold(root: &Path, project_code: &str, action_version: &str) -> ExitCode {
    // Trimmed once, here, before it is either checked or written: `is_release_tag`
    // trims internally, so an untrimmed value could pass this refusal and still
    // reach `workflow` with the whitespace intact, corrupting the `uses:` ref it
    // was just cleared to write.
    let action_version = action_version.trim();

    if !store::projects::is_valid_code(project_code) {
        eprintln!(
            "navigator: invalid Project code `{project_code}`; use lowercase letters, digits, and single hyphens (80 characters maximum), and not a segment Navigator routes itself"
        );
        return ExitCode::from(2);
    }

    if !is_release_tag(action_version) {
        if action_version.is_empty() {
            eprintln!(
                "navigator: no --action-version was given, and this build cannot confirm its \
                 own version is one this repository has published (only a downloaded release \
                 binary, or one built with `NAVIGATOR_RELEASE_TAG` set, can); pass {RELEASE_TAG_SHAPE}"
            );
        } else {
            eprintln!("navigator: invalid validate-action version `{action_version}`; use {RELEASE_TAG_SHAPE}");
        }
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
        (root.join(WORKFLOW), workflow(action_version)),
        (root.join(CD_WORKFLOW), cd_workflow(action_version)),
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

/// Write Navigator's canonical skill catalog into a Project repository, from
/// this binary's own compiled-in copies (see [`SYNCED_SKILLS`]).
///
/// Unlike [`scaffold`], which leaves an existing file alone, this always
/// overwrites: the point of syncing is that the copy in the repository stays
/// identical to the canonical one, not that it is merely present. A hand
/// edit is exactly the drift [`validate`] is meant to catch, and catching it
/// is only useful if re-running this command is also how an operator fixes
/// it.
pub fn sync_skills(root: &Path) -> ExitCode {
    for (name, contents) in SYNCED_SKILLS {
        let path = root.join(".claude/skills").join(name).join("SKILL.md");
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
        println!("synced    {}", path.display());
    }
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

    // The repository name is the Project code, and that is the value this
    // validator speaks about: a checkout named something a Project code could
    // never be is a checkout it cannot judge.
    //
    // The manifest also declares a code, so the two *can* disagree — see
    // `super::drift`, which reports it. This validator deliberately does not
    // resolve that disagreement. It runs inside one repository's own CI with no
    // access to the live row, and the rule is that the row wins and the
    // repository is corrected; a gate that picked a winner without seeing the
    // row would be guessing, and would fail a repository mid-correction.
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
    validate_skills(root, &mut errors);
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

/// A synced skill whose content has drifted from the canonical copy.
///
/// Freshness is judged against the copy compiled into *this* binary, not a
/// live `.agents/skills` clone or a fetch of the pinned release. That mirrors
/// [`validate_workflow`]'s own pin check exactly (see ENG-356): CI runs the
/// validate action at the version the gate pins, so the binary performing
/// this comparison already *is* "the canonical copy at the pinned CLI
/// version" in the one place this check runs for real. A skill that has not
/// been synced yet is not a finding — sync is opt-in per repository, the same
/// policy `templates/` and `portal/` get: absent means "hasn't adopted this,"
/// not "broken."
fn validate_skills(root: &Path, errors: &mut Vec<Finding>) {
    for (name, canonical) in SYNCED_SKILLS {
        let path = root.join(".claude/skills").join(name).join("SKILL.md");
        match fs::read_to_string(&path) {
            Ok(contents) if contents == *canonical => {}
            Ok(_) => errors.push(Finding::at(
                &path,
                format!(
                    "synced skill `{name}` has drifted from the canonical copy; \
                     run `navigator projects repository sync-skills`"
                ),
            )),
            Err(_) => {}
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
            format!("validation action ref `{action_version}` must be {RELEASE_TAG_SHAPE}"),
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

/// What `is_release_tag` requires, spelled out once so `scaffold`'s CLI-time
/// refusal and `validate_workflow`'s CI-time finding describe the one rule in
/// one sentence rather than two that are free to drift.
const RELEASE_TAG_SHAPE: &str =
    "an exact release tag, such as YY.M.D or YY.M.D-hotfix.N — never `main` or `latest`";

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

/// The pinned actions every generated workflow installs Node and pnpm with.
///
/// SHA-pinned per `docs/gitops.md`, each resolved from the tag named in its
/// trailing comment via the GitHub API rather than typed from memory: a wrong
/// SHA is indistinguishable from a correct one until the run that needs it.
const SETUP_NODE_ACTION: &str =
    "actions/setup-node@820762786026740c76f36085b0efc47a31fe5020 # v7.0.0";
const PNPM_SETUP_ACTION: &str =
    "pnpm/action-setup@0977fd99725f1db4007ccb2928dbb4e90d06cc86 # v6.0.10";
const CHECKOUT_ACTION: &str = "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1";

/// The pinned publish action a Project repository's CD workflow calls.
const APPLICATION_PUBLISH_ACTION: &str =
    "neon-law-source-code/navigator/.github/actions/application-publish@";

/// The condition every portal-specific step in a generated workflow carries.
///
/// `scaffold` never writes `portal/` — it arrives later from the vibe-coding
/// lane, onto a `gate.yml` that is already written and never regenerated. So
/// whether a portal exists cannot be decided once at generation time; it has
/// to be asked at every run, which is what this expression does.
const IF_PORTAL_PRESENT: &str = "hashFiles('portal/package.json') != ''";

/// The standard install/lint/typecheck/build/test sequence, one line per
/// script, package-manager-agnostic in what it checks but pnpm in what it
/// runs: every Project repository observed today uses pnpm, and a repository
/// that genuinely needs a different one remains free to hand-edit the
/// generated file, the same way it is already free to add anything else.
fn pnpm_step(name: &str, script: &str) -> String {
    format!("      - name: {name}\n        if: {IF_PORTAL_PRESENT}\n        run: pnpm --dir portal {script}\n")
}

fn setup_steps() -> String {
    format!(
        "      - uses: {CHECKOUT_ACTION}\n      \
         - uses: {SETUP_NODE_ACTION}\n        if: {IF_PORTAL_PRESENT}\n        with:\n          node-version: \"22\"\n      \
         - uses: {PNPM_SETUP_ACTION}\n        if: {IF_PORTAL_PRESENT}\n"
    )
}

/// The CI gate, promoted from the hand-built shape the Project repositories
/// had already converged on before this generator caught up to them, pinned
/// to `action_version`.
///
/// # Fan-in, not one flat job
///
/// `lint`, `verify`, and `notation` each run unconditionally and no-op
/// internally over a half this repository does not carry ([`IF_PORTAL_PRESENT`]
/// gates every portal-specific step; the pinned validate action already no-ops
/// over an absent portal on its own). [`REQUIRED_CHECK`] is the one job the
/// ruleset actually binds to, and it asserts each dependency's `result`
/// explicitly rather than trusting a bare `needs:` — a **skipped** job reports
/// no result at all, so a bare `needs:` would read that as success and leave a
/// required check nothing ever fails, stuck "Expected" forever. There is
/// deliberately **no** `paths:` filter anywhere: a filtered job that skips
/// reports success for work it never did, and a required check that can be
/// satisfied by a skip is not a gate.
///
/// # The pin is an argument, not a literal
///
/// `[scaffold]` refuses to call this with anything [`is_release_tag`] rejects,
/// the way `ops release-version` refuses a malformed `--tag`: a gate emitted
/// at `main`, at `latest`, or at a version this repository has not published
/// is a gate the Project cannot run, so the choice belongs to the operator
/// (or to `main.rs`'s `published_cli_version`, when this binary can vouch for
/// its own version) rather than to a literal frozen in this string.
///
/// A raw string, not a `\`-continued one. A backslash continuation strips the
/// leading whitespace of the next line, which silently reflows YAML into
/// something that no longer parses — and a generated workflow that does not
/// parse fails in the Project repository rather than here.
fn workflow(action_version: &str) -> String {
    let setup = setup_steps();
    let install = pnpm_step("Install portal dependencies", "install --frozen-lockfile");
    format!(
        r#"name: {REQUIRED_CHECK}

on:
  pull_request:
  push:
    branches: [main]

jobs:
  lint:
    runs-on: ubuntu-latest
    steps:
{setup}{install}{lint_step}
  verify:
    runs-on: ubuntu-latest
    steps:
{setup}{install}{typecheck_step}{test_step}{build_step}
  notation:
    runs-on: ubuntu-latest
    steps:
      - uses: {CHECKOUT_ACTION}
      - uses: {VALIDATE_ACTION}{action_version}
        with:
          version: "{action_version}"
          project_repository: true

  # The one required check. See the doc comment above for why it asserts
  # `needs.<job>.result` explicitly instead of trusting a bare `needs:`.
  {REQUIRED_CHECK}:
    needs: [lint, verify, notation]
    if: always()
    runs-on: ubuntu-latest
    steps:
      - name: Require every job to have succeeded
        run: |
          test "${{{{ needs.lint.result }}}}" = "success"
          test "${{{{ needs.verify.result }}}}" = "success"
          test "${{{{ needs.notation.result }}}}" = "success"
"#,
        lint_step = pnpm_step("Lint the portal", "lint"),
        typecheck_step = pnpm_step("Typecheck the portal", "typecheck"),
        test_step = pnpm_step("Test the portal", "test"),
        build_step = pnpm_step("Build the portal", "build"),
    )
}

/// The Project publication workflow: install, lint, typecheck, test, and build
/// the portal, re-validate the whole repository, then publish through the
/// pinned `application-publish` action — the same shape
/// `docs/project-repositories.md` already documents as the thin caller a
/// Project repository carries, generated here instead of hand-copied into
/// each one.
///
/// The three deployment coordinates (`applications_bucket`,
/// `workload_identity_provider`, `service_account`) are read from repository
/// secrets, never written as literals: they are this deployment's own, not a
/// Project's, and a Project repository's own generated workflow must not
/// carry them.
fn cd_workflow(action_version: &str) -> String {
    let setup = setup_steps();
    let install = pnpm_step("Install portal dependencies", "install --frozen-lockfile");
    format!(
        r#"name: publish

on:
  push:
    branches: [main]

permissions:
  contents: read
  id-token: write

jobs:
  publish:
    runs-on: ubuntu-latest
    steps:
{setup}{install}{lint_step}{typecheck_step}{test_step}{build_step}      - uses: {VALIDATE_ACTION}{action_version}
        with:
          version: "{action_version}"
          project_repository: true
      - name: Publish the built portal
        if: {IF_PORTAL_PRESENT}
        uses: {APPLICATION_PUBLISH_ACTION}{action_version}
        with:
          applications_bucket: ${{{{ secrets.NAVIGATOR_APPLICATIONS_BUCKET }}}}
          workload_identity_provider: ${{{{ secrets.NAVIGATOR_APP_PUBLISHER_WIF_PROVIDER }}}}
          service_account: ${{{{ secrets.NAVIGATOR_APP_PUBLISHER_SERVICE_ACCOUNT }}}}
"#,
        lint_step = pnpm_step("Lint the portal", "lint"),
        typecheck_step = pnpm_step("Typecheck the portal", "typecheck"),
        test_step = pnpm_step("Test the portal", "test"),
        build_step = pnpm_step("Build the portal", "build"),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        cd_workflow, example_template, is_release_tag, repository_name, scaffold, validate_layout,
        validate_workflow, workflow, Finding, ALLOWED_ROOTS, CD_WORKFLOW, REQUIRED_CHECK, WORKFLOW,
    };
    use std::path::Path;

    /// The pin the fixtures below scaffold with.
    ///
    /// A literal, not `crate::published_cli_version()`: a fixture that reads
    /// the running binary's version makes every layout test depend on how
    /// this build was stamped, and an ambient `NAVIGATOR_RELEASE_TAG` would
    /// then decide whether they pass. The one test that must speak about the
    /// real default says so itself.
    const FIXTURE_PIN: &str = "26.8.23";

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
        std::fs::write(root.join(WORKFLOW), workflow(FIXTURE_PIN)).unwrap();
        std::fs::write(root.join(CD_WORKFLOW), cd_workflow(FIXTURE_PIN)).unwrap();
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
            found[0].contains("must be an exact release tag"),
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
        assert_eq!(findings(&workflow(FIXTURE_PIN)), Vec::<String>::new());
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
        let generated = workflow(FIXTURE_PIN);
        assert!(generated.contains("\n  ci:\n"), "{generated}");
        assert!(
            !generated.contains("paths:"),
            "a path-filtered required check can be satisfied by a skip"
        );
        assert!(generated.contains("project_repository: true"));
    }

    /// The pin the caller names reaches both places that carry it, and no
    /// literal survives in the generator.
    #[test]
    fn the_generated_gate_pins_the_version_it_was_given() {
        let generated = workflow("26.8.23");
        assert!(
            generated.contains(
                "- uses: neon-law-source-code/navigator/.github/actions/validate@26.8.23"
            ),
            "{generated}"
        );
        assert!(generated.contains(r#"version: "26.8.23""#), "{generated}");
        assert!(
            !generated.contains("26.7.27"),
            "a hard-coded literal is back:\n{generated}"
        );
    }

    /// The gate fans three feeder jobs into the one required check, rather
    /// than cramming install/lint/typecheck/build/test into the required job
    /// itself — the shape the Project repositories had already converged on
    /// before this generator caught up to them.
    #[test]
    fn the_generated_gate_fans_three_jobs_into_the_required_check() {
        let generated = workflow(FIXTURE_PIN);
        for job in ["lint:", "verify:", "notation:"] {
            assert!(
                generated.contains(&format!("\n  {job}\n")),
                "missing job `{job}`:\n{generated}"
            );
        }
        assert!(
            generated.contains(&format!(
                "\n  {REQUIRED_CHECK}:\n    needs: [lint, verify, notation]\n"
            )),
            "{generated}"
        );
    }

    /// A **skipped** job reports no result at all, so the required check
    /// asserts each dependency's result explicitly rather than trusting a
    /// bare `needs:` — the failure mode `ops github setup`'s own docstring
    /// warns leaves a pull request stuck on an expected check forever.
    #[test]
    fn the_required_check_asserts_every_dependencys_result() {
        let generated = workflow(FIXTURE_PIN);
        assert!(generated.contains("if: always()"), "{generated}");
        for job in ["lint", "verify", "notation"] {
            assert!(
                generated.contains(&format!("needs.{job}.result")),
                "the required check does not check `{job}`'s result:\n{generated}"
            );
        }
    }

    /// The portal-specific steps no-op at run time rather than being decided
    /// once at generation time: `scaffold` never writes `portal/`, so a gate
    /// written before the portal exists must still work once it arrives
    /// later. The validate action no-ops internally over an absent portal
    /// already, so its own step must not carry a second, redundant
    /// condition.
    #[test]
    fn the_portal_steps_are_conditioned_not_generated_away() {
        let generated = workflow(FIXTURE_PIN);
        for step in [
            "Install portal dependencies",
            "Lint the portal",
            "Typecheck the portal",
            "Test the portal",
            "Build the portal",
        ] {
            assert!(
                generated.contains(&format!(
                    "- name: {step}\n        if: hashFiles('portal/package.json') != ''\n"
                )),
                "`{step}` is missing its run-time no-op condition:\n{generated}"
            );
        }
        assert!(
            !generated.contains(
                "if: hashFiles('portal/package.json') != ''\n        uses: neon-law-source-code/navigator"
            ),
            "{generated}"
        );
    }

    /// The publish workflow is the real thing now, not a placeholder that
    /// reads as configured while doing nothing.
    #[test]
    fn cd_workflow_publishes_through_the_pinned_actions() {
        let generated = cd_workflow(FIXTURE_PIN);
        assert!(!generated.contains("TBD"), "{generated}");
        assert!(
            generated.contains("push:\n    branches: [main]"),
            "{generated}"
        );
        assert!(generated.contains("id-token: write"), "{generated}");
        assert!(
            generated.contains(
                "neon-law-source-code/navigator/.github/actions/application-publish@26.8.23"
            ),
            "{generated}"
        );
        for secret in [
            "secrets.NAVIGATOR_APPLICATIONS_BUCKET",
            "secrets.NAVIGATOR_APP_PUBLISHER_WIF_PROVIDER",
            "secrets.NAVIGATOR_APP_PUBLISHER_SERVICE_ACCOUNT",
        ] {
            assert!(generated.contains(secret), "{generated}");
        }
        // The three deployment coordinates are read from secrets, never
        // written as literals: they are this deployment's own, not a
        // Project's.
        assert!(!generated.contains("neon-law-applications"), "{generated}");
    }

    /// The pin reaches the publish action too, and no literal survives.
    #[test]
    fn cd_workflow_pins_the_version_it_was_given() {
        let generated = cd_workflow("26.8.23");
        assert!(
            generated.contains(
                "neon-law-source-code/navigator/.github/actions/application-publish@26.8.23"
            ),
            "{generated}"
        );
        assert!(
            !generated.contains("26.7.27"),
            "a hard-coded literal is back:\n{generated}"
        );
    }

    /// Neither generated workflow takes a Project code, and this proves
    /// nothing project-specific leaks in behind that: the only organization
    /// named in either file is Navigator's own, the one exception
    /// `cli/tests/forge_coordinate_retired.rs` allows.
    #[test]
    fn the_generated_workflows_name_no_project() {
        for generated in [workflow(FIXTURE_PIN), cd_workflow(FIXTURE_PIN)] {
            for line in generated.lines() {
                let Some(reference) = line.trim_start().strip_prefix("uses: ") else {
                    continue;
                };
                let owner = reference.split('/').next().unwrap_or_default();
                assert!(
                    matches!(owner, "actions" | "pnpm" | "neon-law-source-code"),
                    "unexpected organization `{owner}` in generated workflow:\n{generated}"
                );
            }
            assert!(
                !generated.contains("NAVIGATOR_GCP_PROJECT_ID"),
                "a deployment coordinate leaked into the generated workflow:\n{generated}"
            );
        }
    }

    /// Neither generated workflow ever names Google Drive as a publish
    /// destination.
    ///
    /// ENG-73: object storage is the working-file authority and Drive is a
    /// per-Project ingest source only — CI writes into the documents or
    /// applications bucket, never into Drive. A folder ID committed to a
    /// generated workflow would also be attacker-controlled the same way a
    /// literal bucket name would be, so this guards the same class of
    /// mistake `the_generated_workflows_name_no_project` guards for
    /// organizations. Neither generated workflow requests a Drive OAuth
    /// scope or writes Drive at all, so `contains("drive")` is exact — no
    /// legitimate line needs the word.
    #[test]
    fn the_generated_workflows_never_target_drive() {
        for generated in [workflow(FIXTURE_PIN), cd_workflow(FIXTURE_PIN)] {
            let lowered = generated.to_lowercase();
            assert!(
                !lowered.contains("drive"),
                "a generated workflow names Drive; CI must publish only to \
                 object storage:\n{generated}"
            );
        }
    }

    /// The default pin is empty, or it is a release tag — never a
    /// version-shaped string this build merely happens to carry.
    ///
    /// This is the guard that makes the invariant structural rather than
    /// asserted once: a hard-coded pin cannot be checked by anything, because
    /// it is correct on the day it is typed and nothing revisits it, while
    /// this assertion runs on every build. `cargo test` itself is the "cannot
    /// vouch for it" case — it bakes neither a runtime nor a build-time
    /// `NAVIGATOR_RELEASE_TAG` — so `published_cli_version()` is empty here,
    /// and `the_scaffold_refuses_a_pin_that_is_not_a_release_tag` covers what
    /// `scaffold` does with that. This test is what a release CLI build, or
    /// one built with `NAVIGATOR_RELEASE_TAG` set, has to satisfy instead.
    #[test]
    fn the_scaffold_default_pin_is_a_release_tag_or_empty() {
        let default = crate::published_cli_version();
        if default.is_empty() {
            return;
        }
        assert!(
            is_release_tag(default),
            "the scaffold would emit `{default}`, which is not an exact release tag"
        );
        assert_eq!(findings(&workflow(default)), Vec::<String>::new());
    }

    /// A pin the gate could never resolve is refused where the file is
    /// written.
    ///
    /// `validate_workflow` cannot catch a version-shaped-but-unpublished pin:
    /// it holds the pin to the *shape* of a release tag, which `main` fails
    /// but a plausible-looking version that was never published passes. So
    /// the shape rule is enforced at the command, before a Project repository
    /// carries the result — and an empty default (this build cannot vouch for
    /// its own version) is refused the same way as an explicit `main`.
    #[test]
    fn the_scaffold_refuses_a_pin_that_is_not_a_release_tag() {
        for refused in ["main", "latest", ""] {
            let root = tempfile::tempdir().unwrap();
            scaffold(root.path(), "example-project", refused);
            assert!(
                !root.path().join(WORKFLOW).exists(),
                "`{refused}` was accepted and a gate was written"
            );
            assert!(
                !root.path().join("README.md").exists(),
                "`{refused}` was refused only after writing other files"
            );
        }
    }

    /// A pin surrounded by whitespace is trimmed before it is either checked
    /// or written, rather than sailing past the shape check (which trims
    /// internally) and reaching the generated `uses:`/`version:` lines intact
    /// — which would corrupt a ref that the check had just approved.
    #[test]
    fn the_scaffold_trims_the_pin_before_checking_and_writing() {
        let root = tempfile::tempdir().unwrap();
        scaffold(root.path(), "example-project", " 26.8.23 ");
        let generated = std::fs::read_to_string(root.path().join(WORKFLOW)).unwrap();
        assert!(
            generated.contains(
                "- uses: neon-law-source-code/navigator/.github/actions/validate@26.8.23"
            ),
            "{generated}"
        );
        assert!(generated.contains(r#"version: "26.8.23""#), "{generated}");
    }
}
