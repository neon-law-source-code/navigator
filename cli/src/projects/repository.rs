//! One Project, one repository: scaffold and validation.
//!
//! A Project's repository is named for its Project code and holds notation
//! templates under `templates/` plus zero or more applications under
//! `apps/<app>/`. There is one layout and one command for both, because there
//! is one repository. A root `portal/` remains accepted while existing
//! repositories move that application to `apps/portal/`.
//!
//! ```text
//! <organization>/<project-code>
//! ├── .github/workflows/gate.yml
//! ├── .github/workflows/publish.yml
//! ├── .claude/skills/    # synced from Navigator via `sync-skills`
//! ├── apps/              # React + Vite workspaces, discovered by package.json
//! │   └── portal/
//! ├── templates/         # *.md notation blueprints
//! ├── seeds/             # lookup_fields / records YAML for `navigator site import`
//! ├── AGENTS.md
//! ├── CLAUDE.md
//! ├── README.md
//! └── navigator.yaml     # the Project this repository declares
//! ```
//!
//! # Where the Project code comes from
//!
//! [`validate`] still takes it from the repository name, and CI has that name
//! as `github.event.repository.name`. Each application mount is that name plus
//! the application directory name.
//!
//! A repository also declares its Project in a root manifest — `navigator.yaml`,
//! `project:` — and that manifest is part of the layout. So the code is
//! derived in one place and declared in another, and nothing makes the two
//! agree. Every repository shipping today aligns them by convention —
//! `neon-law-staging/sample-litigation` is named for the code it publishes
//! under — but a repository named for anything else would split them.
//! `store::sample_project::project_code_for` is what refuses a bundle
//! declaring a code other than the one it is published under, so a
//! disagreement is rejected rather than unrepresentable.
//!
//! `.github/actions/application-publish` reads this same manifest rather than
//! the repository name — its `repository:` input is now an override for a
//! checkout without one, not the primary source. [`validate`] here does not
//! follow suit: it runs inside one repository's own CI with no access to the
//! live row, so it cannot tell a repository whose manifest is wrong from one
//! whose name is; `navigator site projects drift` (`super::drift`) is where that
//! disagreement is reported, against the live rows it needs to judge it.
//!
//! One filename, one key: the earlier `.yml` spelling and its `name:` key are
//! retired.
//! `store::sample_project::MANIFEST_FILE` names the same file for the same
//! reason a bundle staged locally reads — a Project repository's manifest and
//! a staged sample bundle's manifest are the same file, not two schemas that
//! happen to overlap.
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
//! # The scaffold does not write applications
//!
//! It writes the repository shell and the templates half. `apps/` arrives from
//! the vibe-coding lane, which is what knows how to make a Vite application and
//! which released `@neon-law/ux` to pin. An application is discovered from a
//! direct `apps/<app>/package.json`; the tree, not a second manifest list,
//! declares what the repository carries.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::devx::github_setup::REQUIRED_CHECK;

/// The notation blueprints Navigator imports.
const TEMPLATE_DIRECTORY: &str = "templates";
/// Ephemeral document staging plus committed pointer YAML.
pub(crate) const DOCUMENT_DIRECTORY: &str = "documents";
/// The legacy client portal's Vite workspace.
///
/// `pub(crate)` because [`super::super::devx::github_setup`] still uses its
/// presence to decide whether the legacy single-portal publisher is applicable.
pub(crate) const PORTAL_DIRECTORY: &str = "portal";
/// Application workspaces, each declared by `apps/<app>/package.json`.
const APPLICATIONS_DIRECTORY: &str = "apps";
/// `pub(crate)` because [`super::super::devx::github_setup`] reconciles the
/// live file at this path against [`workflow`]'s output, the same template
/// `scaffold` writes, read back rather than duplicated.
pub(crate) const WORKFLOW: &str = ".github/workflows/ci.yml";
/// The retired generated-gate filename. Reconcile still reads it so a
/// repository that has not yet been rewritten is classified, not ignored.
pub(crate) const RETIRED_WORKFLOW: &str = ".github/workflows/gate.yml";
/// `pub(crate)` for the same reason as [`WORKFLOW`], but for [`cd_workflow`].
pub(crate) const CD_WORKFLOW: &str = ".github/workflows/publish.yml";
/// The manifest a Project repository declares its Project in.
///
/// `pub(crate)` rather than private because [`super::drift`] and
/// [`super::super::devx::github_setup`] each read the same file and must not
/// spell it a second time: two constants for one filename is a rename waiting
/// to leave one of them stale, and the gate that *admits* the file, the
/// command that *reads* it locally, and the command that *reads* it live over
/// the API are exactly the trio that must agree. The same filename
/// `store::sample_project::MANIFEST_FILE` names, a Project repository's
/// manifest and a staged sample bundle's manifest are the same file, read by
/// different tools, not two schemas that happen to overlap.
pub(crate) const PROJECT_MANIFEST: &str = "navigator.yaml";
/// Seed-shaped YAML documents for `navigator site import`, one file per model.
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
    "LICENSE",
    "README.md",
    // A Vite application at the repository root (no `apps/` grouping) is named
    // `portal`. These are the files that shape requires at the root.
    "package.json",
    "vite.config.ts",
    "vite.config.js",
    "index.html",
    "pnpm-lock.yaml",
    "package-lock.json",
    "yarn.lock",
    "bun.lockb",
    "tsconfig.json",
    "tsconfig.node.json",
    "tsconfig.app.json",
    ".oxlintrc.json",
    "public",
    "src",
    APPLICATIONS_DIRECTORY,
    "fixtures",
    DOCUMENT_DIRECTORY,
    // The manifest a Project repository declares its Project code in. Refusing
    // it made the layout unsatisfiable for every repository that carries one,
    // which is why the pinned validate action had to be pulled from all six
    // Project gates rather than the manifest being removed.
    //
    // One entry, not two: `PROJECT_MANIFEST` and `store::sample_project::MANIFEST_FILE`
    // name the same file. The retired `.yml` spelling is not admitted; see
    // `cli/tests/navigator_manifest_retired.rs`.
    PROJECT_MANIFEST,
    PORTAL_DIRECTORY,
    // A seed document names real people and real entities described by this
    // Project's matter — the input to a production write through `navigator
    // site import`, not test scaffolding. That is the one distinction `fixtures/`
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

/// The files a Vite-built application must have at the root of its directory.
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
/// plus the general engineering council, are the initial set. `stay-in-repo`
/// joins them as the shared scope rule every Project repository's `CLAUDE.md`
/// used to hand-write on its own, three times, in three different voices.
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
    (
        "stay-in-repo",
        include_str!("../../../.agents/skills/stay-in-repo/SKILL.md"),
    ),
    (
        "portal-chrome",
        include_str!("../../../.agents/skills/portal-chrome/SKILL.md"),
    ),
    (
        "server",
        include_str!("../../../.agents/skills/server/SKILL.md"),
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
pub fn scaffold(
    root: &Path,
    project_code: &str,
    action_version: &str,
    host: &str,
    replace_gate: bool,
) -> ExitCode {
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
            eprintln!(
                "navigator: invalid validate-action version `{action_version}`; use {RELEASE_TAG_SHAPE}"
            );
        }
        return ExitCode::from(2);
    }

    let host = host.trim();
    if !super::manifest::is_hostname(host) {
        eprintln!("navigator: `--host` must be a hostname, not `{host}`");
        return ExitCode::from(2);
    }

    let workflow_path = root.join(WORKFLOW);
    if workflow_path.is_file() && !replace_gate {
        if let Ok(live) = fs::read_to_string(&workflow_path) {
            if live.lines().count() >= HAND_COPIED_GATE_LINES {
                eprintln!(
                    "navigator: {} has {} lines; pass --replace-gate to replace the named jobs with the thin project-gate caller",
                    workflow_path.display(),
                    live.lines().count()
                );
                return ExitCode::from(2);
            }
        }
    }

    let manifest = format!("host: {host}\nproject: {project_code}\n");
    let template_stem = placeholder_template_stem(project_code);
    let files = [
        (root.join("README.md"), readme(project_code)),
        (root.join("AGENTS.md"), agents(project_code)),
        (root.join("tests/README.md"), tests_readme()),
        (root.join(WORKFLOW), workflow(action_version)),
        (root.join(CD_WORKFLOW), cd_workflow(action_version)),
        (root.join(PROJECT_MANIFEST), manifest),
        (
            root.join(TEMPLATE_DIRECTORY)
                .join(format!("{template_stem}.md")),
            placeholder_template(&template_stem),
        ),
    ];

    for (path, contents) in files {
        if path.exists() && !(replace_gate && path == workflow_path) {
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

    let claude = root.join("CLAUDE.md");
    if claude.exists() {
        println!("exists    {} (left alone)", claude.display());
    } else {
        match link_claude_to_agents(root, &claude) {
            Ok(mechanism) => println!("created   {} ({mechanism})", claude.display()),
            Err(error) => {
                eprintln!("navigator: {}: {error}", claude.display());
                return ExitCode::from(2);
            }
        }
    }

    // Do not interpolate the CLI root here: `Command` also carries `Secrets`,
    // and CodeQL treats any printed Command field as cleartext logging.
    println!("\nValidate with: navigator validate .");
    ExitCode::SUCCESS
}

/// Make `CLAUDE.md` deliver the bytes of `AGENTS.md`.
///
/// One contract, read by whichever harness is pointed at the tree: the same
/// invariant Navigator's own `cli/tests/agent_instruction_links.rs` guards,
/// and it is stated in resolved bytes rather than link type. On Unix the
/// cheapest way to keep two paths reading one document is a relative symlink.
/// Windows cannot be asked for one: `symlink_file` needs a privilege an
/// ordinary account lacks unless Developer Mode is on, and a link a Windows
/// clone materialises without `core.symlinks` is a stub holding its own target
/// path, which is the exact failure the guard exists to catch. So there the
/// contract is copied from the `AGENTS.md` on disk, whether `scaffold` wrote
/// it just now or left an existing one alone, so both platforms resolve to the
/// same file. The returned string names the mechanism for the `created` line.
#[cfg(unix)]
fn link_claude_to_agents(_root: &Path, claude: &Path) -> std::io::Result<&'static str> {
    std::os::unix::fs::symlink("AGENTS.md", claude)?;
    Ok("symlink to AGENTS.md")
}

#[cfg(not(unix))]
fn link_claude_to_agents(root: &Path, claude: &Path) -> std::io::Result<&'static str> {
    fs::copy(root.join("AGENTS.md"), claude)?;
    Ok("copy of AGENTS.md")
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
/// `templates/notations/neon_law` / `templates/notations/forms` catalog. This mirrors
/// `store::template_source::persist_from_repo` exactly.
///
/// Templates and applications are independently optional, and a repository
/// carrying neither is reported distinctly rather than failed. A Project may
/// legitimately have opened before either half exists.
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

    validate_layout(root, &mut errors);
    validate_skills(root, &mut errors);
    let has_templates = root.join(TEMPLATE_DIRECTORY).is_dir();
    let applications = application_workspaces(root, &mut errors);
    let templates = if has_templates {
        validate_templates(root, &code, &mut errors, &mut warnings)
    } else {
        0
    };
    for application in &applications {
        validate_application(application, &mut errors);
    }
    if !has_templates && applications.is_empty() {
        println!("note: {code} carries neither `{TEMPLATE_DIRECTORY}/` nor an application yet");
    }

    for warning in &warnings {
        println!("{}: warning: {}", warning.path.display(), warning.message);
    }
    for error in &errors {
        eprintln!("{}: error: {}", error.path.display(), error.message);
    }
    println!(
        "Validated Project repository `{code}`: {templates} template(s), {} application(s), {} error(s), {} warning(s)",
        applications.len(),
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
    if let Ok(contents) = fs::read_to_string(root.join(PROJECT_MANIFEST)) {
        if let Ok(manifest) = super::manifest::parse(&contents) {
            if let Some(code) = manifest
                .project
                .as_deref()
                .map(str::trim)
                .filter(|code| !code.is_empty())
            {
                return code.to_string();
            }
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

    validate_manifest(root, errors);

    let workflow_path = root.join(WORKFLOW);
    let retired_workflow = root.join(RETIRED_WORKFLOW);
    match fs::read_to_string(&workflow_path) {
        Ok(contents) => validate_workflow(&workflow_path, &contents, errors),
        Err(_) => match fs::read_to_string(&retired_workflow) {
            Ok(contents) => validate_workflow(&retired_workflow, &contents, errors),
            Err(_) => errors.push(Finding::at(workflow_path, "missing required CI gate")),
        },
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
        if first == DOCUMENT_DIRECTORY && entry.file_type().is_file() {
            let is_pointer = entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension == "yml");
            let is_guard = components.len() == 2 && components[1] == ".gitignore";
            if !is_pointer && !is_guard {
                errors.push(Finding::at(
                    entry.path(),
                    "legal documents and raw document bytes must not be committed; keep only `*.yml` pointers under `documents/`",
                ));
            }
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

/// Hold a Project repository's `navigator.yaml` to the closed key set and
/// value shapes [`super::manifest::lint`] owns. There is no per-repository
/// exemption mechanism.
fn validate_manifest(root: &Path, errors: &mut Vec<Finding>) {
    for finding in super::manifest::lint(root) {
        errors.push(Finding::at(
            finding.path,
            format!("{}: {}", finding.code, finding.message),
        ));
    }
}

/// A synced skill that is missing, or whose content has drifted from the
/// canonical copy.
///
/// Freshness is judged against the copy compiled into *this* binary, not a
/// live `.agents/skills` clone or a fetch of the pinned release. That mirrors
/// [`validate_workflow`]'s own pin check exactly (see ENG-356): CI runs the
/// validate action at the version the gate pins, so the binary performing
/// this comparison already *is* "the canonical copy at the pinned CLI
/// version" in the one place this check runs for real.
///
/// ## `.claude/` is the opt-in, and it is opt-in to the whole catalog
///
/// Absence used to be silent everywhere, on the same reasoning `templates/`
/// and `portal/` get: not adopted is not broken. That reasoning stops holding
/// the moment a repository has a `.claude/` directory, because then an agent
/// *is* working in it under whatever skills it happens to find — and the ones
/// it does not find are the conventions nobody told it about.
///
/// The catalog is the fleet's answer to conventions that live only in a
/// comment. `portal-chrome` is the worked example: the rule that a portal
/// wears the library's teal and never repaints it existed for months as a
/// header comment inside the very file that violated it, in sixteen
/// repositories, claiming a fleet-wide uniformity that had already broken in
/// two directions. A convention an agent cannot see is a convention that
/// drifts.
///
/// So: no `.claude/` directory, no findings — a repository that has not
/// adopted agent tooling is not failed for it. With one, every skill in the
/// catalog is required, and `sync-skills` is how a repository gets them.
///
/// This only reaches a repository when it bumps the validate action's pin, so
/// adoption stays staged rather than turning the fleet red at once.
fn validate_skills(root: &Path, errors: &mut Vec<Finding>) {
    let agent_directory = root.join(".claude");
    if !agent_directory.is_dir() {
        return;
    }
    for (name, canonical) in SYNCED_SKILLS {
        let path = root.join(".claude/skills").join(name).join("SKILL.md");
        match fs::read_to_string(&path) {
            Ok(contents) if contents == *canonical => {}
            Ok(_) => errors.push(Finding::at(
                &path,
                format!(
                    "synced skill `{name}` has drifted from the canonical copy; \
                     run `navigator site projects repository sync-skills`"
                ),
            )),
            Err(_) => errors.push(Finding::at(
                &path,
                format!(
                    "this repository has a `.claude/` directory but is missing synced skill \
                     `{name}`; run `navigator site projects repository sync-skills`"
                ),
            )),
        }
    }
}

/// Discover direct application workspaces from the tree.
///
/// A root `package.json` plus `vite.config.ts`, with no `apps/portal/` and no
/// legacy `portal/` directory, is the application named `portal`. The legacy
/// root `portal/` is directory-discovered so a half-migrated, malformed portal
/// still receives the Vite finding it always did. Nested applications are
/// declared only by a direct `apps/<app>/package.json`.
fn application_workspaces(root: &Path, errors: &mut Vec<Finding>) -> Vec<PathBuf> {
    let mut applications = Vec::new();
    let legacy_portal = root.join(PORTAL_DIRECTORY);
    let has_legacy_portal = legacy_portal.is_dir();
    if has_legacy_portal {
        applications.push(legacy_portal);
    }
    let root_vite = root.join("package.json").is_file()
        && (root.join("vite.config.ts").is_file() || root.join("vite.config.js").is_file());
    let has_apps_portal = root
        .join(APPLICATIONS_DIRECTORY)
        .join(PORTAL_DIRECTORY)
        .join("package.json")
        .is_file();
    if root_vite && (has_legacy_portal || has_apps_portal) {
        errors.push(Finding::at(
            root.join("package.json"),
            "a root Vite workspace and `portal/` (or `apps/portal/`) claim the same application route; keep one",
        ));
    } else if root_vite {
        applications.push(root.to_path_buf());
    }

    let apps = root.join(APPLICATIONS_DIRECTORY);
    if !apps.is_dir() {
        return applications;
    }
    let Ok(entries) = fs::read_dir(&apps) else {
        errors.push(Finding::at(apps, "could not read application directory"));
        return applications;
    };
    let mut discovered = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            errors.push(Finding::at(&apps, "could not read application entry"));
            continue;
        };
        let path = entry.path();
        if path.is_dir() && path.join("package.json").is_file() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !cloud::workspace::is_valid_slug(&name) {
                errors.push(Finding::at(
                    &path,
                    format!(
                        "`{name}` is not a valid application name; use lowercase letters, digits, and single hyphens"
                    ),
                ));
            }
            if name == PORTAL_DIRECTORY && (has_legacy_portal || root_vite) {
                errors.push(Finding::at(
                    &path,
                    "`apps/portal/` and a root or legacy `portal/` claim the same application route; keep one",
                ));
            }
            discovered.push(path);
        }
    }
    discovered.sort();
    applications.extend(discovered);
    applications
}

pub(crate) fn discovered_applications(root: &Path) -> Vec<PathBuf> {
    let mut errors = Vec::new();
    application_workspaces(root, &mut errors)
}

/// One discovered application's build shape.
fn validate_application(application: &Path, errors: &mut Vec<Finding>) {
    let mut missing: Vec<&str> = VITE_ENTRYPOINTS
        .iter()
        .copied()
        .filter(|file| !application.join(file).is_file())
        .collect();
    if !VITE_LOCKFILES
        .iter()
        .any(|file| application.join(file).is_file())
    {
        missing.push("a lockfile");
    }
    if !missing.is_empty() {
        errors.push(Finding::at(
            application,
            format!(
                "application is not a Vite workspace: missing {}",
                missing.join(", ")
            ),
        ));
    }
}

/// The reusable workflow a Project repository's thin `ci.yml` must call.
const PROJECT_GATE_WORKFLOW: &str =
    "neon-law-source-code/navigator/.github/workflows/project-gate.yml@";
/// The retired composite-action pin a `gate.yml` used to call.
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

/// Hold the CI gate to calling Navigator's reusable project-gate workflow, at
/// an exact release tag matching the `version` input.
fn validate_workflow(path: &Path, contents: &str, errors: &mut Vec<Finding>) {
    let workflow: Workflow = match serde_yaml::from_str(contents) {
        Ok(workflow) => workflow,
        Err(error) => {
            errors.push(Finding::at(
                path,
                format!("CI gate is not valid YAML: {error}"),
            ));
            return;
        }
    };

    let job = workflow.jobs.values().find(|job| {
        job.uses
            .as_deref()
            .is_some_and(|uses| uses.trim().starts_with(PROJECT_GATE_WORKFLOW))
    });
    let Some(job) = job else {
        errors.push(Finding::at(
            path,
            "CI gate must call Navigator's pinned project-gate reusable workflow",
        ));
        return;
    };

    let action_version = job
        .uses
        .as_deref()
        .unwrap_or_default()
        .trim()
        .strip_prefix(PROJECT_GATE_WORKFLOW)
        .unwrap_or_default();
    let Some(input_version) = job.with.get("version").and_then(scalar) else {
        errors.push(Finding::at(
            path,
            "CI gate must pass the reusable workflow's exact release tag as `version`",
        ));
        return;
    };

    if action_version != input_version {
        errors.push(Finding::at(
            path,
            format!(
                "project-gate workflow ref `{action_version}` must equal its `version` input `{input_version}`"
            ),
        ));
    }
    if !is_release_tag(action_version) {
        errors.push(Finding::at(
            path,
            format!("project-gate workflow ref `{action_version}` must be {RELEASE_TAG_SHAPE}"),
        ));
    }
}

fn is_release_tag(version: &str) -> bool {
    crate::devx::registry::is_release_tag(version)
}

/// What `is_release_tag` requires, spelled out once so `scaffold`'s CLI-time
/// refusal, `validate_workflow`'s CI-time finding, and
/// [`super::super::devx::github_setup`]'s own refusal describe the one rule in
/// one sentence rather than three that are free to drift.
pub(crate) const RELEASE_TAG_SHAPE: &str =
    "an exact release tag, such as YY.M.D or YY.M.D-hotfix.N — never `main` or `latest`";

fn validate_templates(
    root: &Path,
    project_code: &str,
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
    let prefix = template_code_prefix(project_code);
    let mut declared_codes = BTreeMap::new();
    for path in &paths {
        lint_project_template(path, &prefix, &rules, &mut declared_codes, errors, warnings);
    }
    paths.len()
}

/// `Y010` — a Project template names `Neon Law` with a corporate suffix that is
/// not the firm entity of record.
///
/// `Neon Law` is a mark, and a mark may head any sentence in a template. The
/// mark followed by `, Inc.`, `LLC`, `PLLC`, or another corporate suffix is a
/// claim about which legal person the client is engaging, and on an
/// engagement letter that claim sits above a signature block. The one legal
/// person is [`store::seed::FIRM_ENTITY_NAME`]; this compares against that
/// constant and never against a literal of its own, so the gate cannot become
/// a further spelling. `Neon Law IP LLC`, the Licensor, is a different name
/// with a different word after the mark, not a suffix case.
pub const ENTITY_CODE: &str = "Y010";

/// The mark a template may trade under without naming a legal person.
const FIRM_MARK: &str = "Neon Law";

/// Corporate suffixes that turn the mark into an entity claim. Longer
/// spellings precede the shorter spelling they contain, so `Incorporated` is
/// read whole rather than as `Inc` with letters after it.
const CORPORATE_SUFFIXES: &[&str] = &[
    "Incorporated",
    "Inc.",
    "Inc",
    "P.L.L.C.",
    "PLLC",
    "L.L.C.",
    "LLC",
    "L.L.P.",
    "LLP",
    "Corporation",
    "Corp.",
    "Corp",
    "Limited",
    "Ltd.",
    "Ltd",
    "P.C.",
    "PC",
    "Co.",
];

/// Every `(line, spelling)` at which `contents` names the mark with a
/// corporate suffix and the result is not `firm`, the entity of record.
///
/// Lines are one-based, as the finding prints them. The spelling is the mark
/// plus the suffix as the template wrote it, with one space between, so the
/// message names what the file says rather than a normalized form.
fn misnamed_firm_entities(contents: &str, firm: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    for (index, line) in contents.lines().enumerate() {
        for (at, _) in line.match_indices(FIRM_MARK) {
            let rest = &line[at + FIRM_MARK.len()..];
            let Some(suffix) = entity_suffix(rest) else {
                continue;
            };
            let spelled = format!("{FIRM_MARK}{suffix}");
            if spelled != firm {
                found.push((index + 1, spelled));
            }
        }
    }
    found
}

/// The corporate suffix `rest` opens with — an optional comma, at least one
/// whitespace character, and one of [`CORPORATE_SUFFIXES`] ending at a word
/// boundary — rendered as `, Inc.` or ` PLLC`. `None` when the mark ran into
/// more letters (`Neon Lawyers`), ended the sentence (`Neon Law.`), or was
/// followed by any other word (`Neon Law IP LLC`, `Neon Law helps`).
fn entity_suffix(rest: &str) -> Option<String> {
    let (comma, rest) = match rest.strip_prefix(',') {
        Some(rest) => (",", rest),
        None => ("", rest),
    };
    let trimmed = rest.trim_start();
    if trimmed.len() == rest.len() {
        return None;
    }
    CORPORATE_SUFFIXES.iter().find_map(|suffix| {
        let head = trimmed.get(..suffix.len())?;
        if !head.eq_ignore_ascii_case(suffix) {
            return None;
        }
        let boundary = trimmed[suffix.len()..]
            .chars()
            .next()
            .is_none_or(|next| !next.is_alphanumeric());
        boundary.then(|| format!("{comma} {head}"))
    })
}

fn lint_project_template(
    path: &Path,
    prefix: &str,
    rules: &[Box<dyn rules::Rule>],
    declared_codes: &mut BTreeMap<String, PathBuf>,
    errors: &mut Vec<Finding>,
    warnings: &mut Vec<Finding>,
) {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) => {
            errors.push(Finding::at(path, format!("read template: {error}")));
            return;
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
    for (line, spelling) in misnamed_firm_entities(&contents, store::seed::FIRM_ENTITY_NAME) {
        errors.push(Finding::at(
            path,
            format!(
                "{ENTITY_CODE}: line {line} names `{spelling}` as the firm, but the entity of \
                 record is `{}`; a template that gives the mark a corporate suffix must name \
                 the contracting entity exactly",
                store::seed::FIRM_ENTITY_NAME
            ),
        ));
    }
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    if !stem.starts_with(prefix) {
        errors.push(Finding::at(
            path,
            format!("template filename stem `{stem}` must start with `{prefix}`"),
        ));
    }
    if let Some(code) = rules::frontmatter::extract(&contents)
        .and_then(|frontmatter| rules::frontmatter::field(frontmatter, "code"))
    {
        if code != stem {
            errors.push(Finding::at(
                path,
                format!(
                    "template `code` `{code}` must equal filename stem `{stem}` \
                     (expected prefix `{prefix}`)"
                ),
            ));
        }
        if let Some(first) = declared_codes.insert(code.clone(), path.to_path_buf()) {
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

/// Filename prefix for a Project template: hyphens in the Project code
/// become underscores, then `__`. Every `templates/<stem>.md` stem starts
/// with this, and frontmatter `code:` equals the stem.
fn template_code_prefix(project_code: &str) -> String {
    format!("{}__", project_code.replace('-', "_"))
}

/// Filename stem for the scaffolded placeholder.
fn placeholder_template_stem(project_code: &str) -> String {
    format!("{}engagement", template_code_prefix(project_code))
}

fn placeholder_template(stem: &str) -> String {
    [
        "---\n",
        "kind: letter\n",
        "title: Engagement letter\n",
        "respondent_type: entity\n",
        "code: ",
        stem,
        "\n",
        "jurisdiction: NV\n",
        "confidential: true\n",
        "questionnaire:\n",
        "  BEGIN:\n",
        "    _: END\n",
        "  END: {}\n",
        "workflow:\n",
        "  BEGIN:\n",
        "    intake_submitted: lawyer_review\n",
        "  lawyer_review:\n",
        "    approved: END\n",
        "    rejected: END\n",
        "  END: {}\n",
        "---\n",
        "\n",
        "Replace this placeholder with the notation this Project actually uses.\n",
    ]
    .concat()
}

fn readme(project_code: &str) -> String {
    format!(
        "# {project_code}\n\n\
         This repository holds source-only material for Project `{project_code}`.\n\n\
         Notation templates live under `templates/`, and application workspaces live under `apps/<app>/`.\n\n\
         The repository name *is* the Project code. Nothing in here declares it, so nothing can disagree with it.\n\n\
         Each app name comes from its directory and builds for `/app/projects/{project_code}/<app>/`.\n\n\
         `apps/` is not part of that URL.\n\n\
         A root `portal/` is also accepted while repositories move it to `apps/portal/`.\n\n\
         Navigator imports each direct `templates/<code>.md` file at the current commit.\n\n\
         It preserves that commit SHA and the template body's content hash as provenance.\n\n\
         Do not commit client uploads, answers, generated documents, secrets, dependencies, or build output.\n\n\
         Legal files live in Drive and in Navigator's assets, never in Git.\n\n\
         Run `navigator validate .` before opening a pull request.\n"
    )
}

fn agents(project_code: &str) -> String {
    format!(
        "# Working in {project_code}\n\n\
         This is one Project's repository. It holds two kinds of source and nothing else.\n\n\
         * `templates/` — notation blueprints, one `templates/<code>.md` per notation.\n\
         * `apps/<app>/` — React + Vite applications, each discovered from its direct `package.json`.\n\n\
         Filename stems use the Project code (hyphens become `_`) then `__name`; `code:` matches.\n\n\
         Navigator imports each template and records the commit SHA as provenance.\n\n\
         Build each app for `/app/projects/{project_code}/<app>/`; the `apps/` source grouping is not a URL segment.\n\n\
         Derive every in-app path from `import.meta.env.BASE_URL` rather than writing an absolute path by hand.\n\n\
         A Vite base rewrites module and asset URLs and never an `href` in source.\n\n\
         A root `portal/` is also accepted while repositories move that workspace to `apps/portal/`.\n\n\
         ## Project codes are client identifiers\n\n\
         A Project code names a matter and its repository. It identifies a client, so it is client data.\n\n\
         The one legitimate use here is this repository naming itself, as in `navigator.yaml`, its paths, and its portal mount.\n\n\
         Do not copy a Project code from another repository into this codebase.\n\n\
         Do not put it into a commit message, code comment, branch name, or pull-request body.\n\n\
         A precedent citation is still a breach; cite the governing issue by its bare identifier instead.\n\n\
         Read matter data through Navigator's `/api` read surfaces and write through its one REST command boundary.\n\n\
         Do not add a second backend.\n\n\
         Do not put a legal file, a client upload, an answer, a generated document, or a secret in this repository.\n"
    )
}

fn tests_readme() -> String {
    "# Tests\n\nKeep source-level tests for this Project's templates here. Generated documents and dependencies do not belong here.\n"
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
const SEED_IMPORT_ACTION: &str = "neon-law-source-code/navigator/.github/actions/seed-import@";

/// The tree-derived condition for generated application steps.
///
/// Both patterns are load-bearing: the first discovers every direct app and
/// the second keeps the root-portal transition green. This gates tool setup on
/// repositories that actually need Node; the shell loop below still derives
/// the complete list independently rather than trusting a declared matrix.
const IF_APPLICATION_PRESENT: &str =
    "hashFiles('apps/*/package.json', 'portal/package.json', 'vite.config.ts', 'vite.config.js') != ''";

/// The standard install/lint/typecheck/build/test sequence, one line per
/// script, package-manager-agnostic in what it checks but pnpm in what it
/// runs: every Project repository observed today uses pnpm, and a repository
/// that genuinely needs a different one remains free to hand-edit the
/// generated file, the same way it is already free to add anything else.
fn pnpm_step(name: &str, script: &str) -> String {
    format!(
        r#"      - name: {name}
        if: {IF_APPLICATION_PRESENT}
        shell: bash
        run: |
          set -euo pipefail
          shopt -s nullglob
          package_manifests=(apps/*/package.json)
          if [ -f portal/package.json ]; then
              package_manifests+=(portal/package.json)
          fi
          if [ -f package.json ] && {{ [ -f vite.config.ts ] || [ -f vite.config.js ]; }}; then
              if [ ! -f portal/package.json ] && [ ! -f apps/portal/package.json ]; then
                  package_manifests+=(package.json)
              fi
          fi
          for package_json in "${{package_manifests[@]}}"; do
              app_dir="${{package_json%/package.json}}"
              pnpm --dir "${{app_dir}}" {script}
          done
"#
    )
}

fn setup_steps() -> String {
    format!(
        "      - uses: {CHECKOUT_ACTION}\n      \
         - uses: {SETUP_NODE_ACTION}\n        if: {IF_APPLICATION_PRESENT}\n        with:\n          node-version: \"22\"\n      \
         - uses: {PNPM_SETUP_ACTION}\n        if: {IF_APPLICATION_PRESENT}\n"
    )
}

/// A thin `ci.yml` caller: one required job named [`REQUIRED_CHECK`] that
/// calls Navigator's reusable project-gate workflow at `action_version`.
///
/// The jobs themselves live in `.github/workflows/project-gate.yml` in this
/// repository. Pinning that file is the thing that scales; a Project
/// repository does not copy them.
pub(crate) fn workflow(action_version: &str) -> String {
    format!(
        r#"name: {REQUIRED_CHECK}

on:
  pull_request:
  push:
    branches: [main]

permissions:
  contents: read
  id-token: write

jobs:
  {REQUIRED_CHECK}:
    uses: {PROJECT_GATE_WORKFLOW}{action_version}
    secrets: inherit
    with:
      version: "{action_version}"
      host: ${{{{ vars.NAVIGATOR_HOST }}}}
"#
    )
}

/// Hand-copied Project `ci.yml` files from the Python-gate era are this long.
/// Scaffold will not replace one unless `--replace-gate` is passed, because
/// replacing it drops the named jobs (`lint`, `verify`, `notation`) a ruleset
/// or a human may still be looking at.
pub(crate) const HAND_COPIED_GATE_LINES: usize = 268;

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
///
/// `pub(crate)` for the same reason as [`workflow`].
pub(crate) fn cd_workflow(action_version: &str) -> String {
    let setup = setup_steps();
    let install = pnpm_step(
        "Install application dependencies",
        "install --frozen-lockfile",
    );
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
    if: vars.NAVIGATOR_HOST != ''
    runs-on: ubuntu-latest
    steps:
{setup}{install}{lint_step}{typecheck_step}{test_step}{build_step}      - uses: {VALIDATE_ACTION}{action_version}
        with:
          version: "{action_version}"
      - name: Import seed documents
        uses: {SEED_IMPORT_ACTION}{action_version}
        with:
          version: "{action_version}"
          host: ${{{{ vars.NAVIGATOR_HOST }}}}
      # Multi-application publication needs a separate prefix/IAM and runtime
      # authorization decision. This preserves only the existing root portal
      # publisher during the source-layout transition.
      - name: Publish the legacy root portal
        if: hashFiles('portal/package.json', 'vite.config.ts', 'vite.config.js') != ''
        uses: {APPLICATION_PUBLISH_ACTION}{action_version}
        with:
          applications_bucket: ${{{{ secrets.NAVIGATOR_APPLICATIONS_BUCKET }}}}
          workload_identity_provider: ${{{{ secrets.NAVIGATOR_APP_PUBLISHER_WIF_PROVIDER }}}}
          service_account: ${{{{ secrets.NAVIGATOR_APP_PUBLISHER_SERVICE_ACCOUNT }}}}
"#,
        lint_step = pnpm_step("Lint applications", "lint"),
        typecheck_step = pnpm_step("Typecheck applications", "typecheck"),
        test_step = pnpm_step("Test applications", "test"),
        build_step = pnpm_step("Build applications", "build"),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        cd_workflow, is_release_tag, lint_project_template, misnamed_firm_entities,
        placeholder_template, repository_name, scaffold, validate_layout, validate_workflow,
        workflow, Finding, ALLOWED_ROOTS, CD_WORKFLOW, ENTITY_CODE, PROJECT_MANIFEST, WORKFLOW,
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
    fn scaffold_validate_hint_does_not_echo_the_cli_root() {
        let src = include_str!("repository.rs");
        let production = src
            .split("#[cfg(test)]")
            .next()
            .expect("production source precedes the test module");
        assert!(
            production.contains("Validate with: navigator validate ."),
            "the post-scaffold hint must name the validate command"
        );
        assert!(
            !production.contains("repository validate {}"),
            "echoing the CLI root trips CodeQL cleartext-logging because Command also carries Secrets"
        );
    }

    #[test]
    fn repository_name_prefers_the_explicit_coordinate() {
        assert_eq!(
            repository_name(Path::new("/tmp/renamed"), Some("org/example")),
            "example"
        );
    }

    #[test]
    fn repository_name_uses_github_repository_before_the_manifest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(PROJECT_MANIFEST),
            "project: acme\nhost: staging.neonlaw.com\n",
        )
        .unwrap();
        let previous = std::env::var("GITHUB_REPOSITORY").ok();
        std::env::set_var("GITHUB_REPOSITORY", "org/from-ci");
        let name = repository_name(dir.path(), None);
        match previous {
            Some(value) => std::env::set_var("GITHUB_REPOSITORY", value),
            None => std::env::remove_var("GITHUB_REPOSITORY"),
        }
        assert_eq!(name, "from-ci");
    }

    #[test]
    fn repository_name_uses_the_manifest_when_github_is_unset() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(PROJECT_MANIFEST),
            "project: acme\nhost: staging.neonlaw.com\n",
        )
        .unwrap();
        let previous = std::env::var("GITHUB_REPOSITORY").ok();
        std::env::remove_var("GITHUB_REPOSITORY");
        let name = repository_name(dir.path(), None);
        if let Some(value) = previous {
            std::env::set_var("GITHUB_REPOSITORY", value);
        }
        assert_eq!(name, "acme");
    }

    #[test]
    fn generated_template_has_a_stable_code() {
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

    #[test]
    fn a_reusable_workflow_call_with_a_matching_pin_passes() {
        let contents = r#"name: ci
on: [pull_request]
jobs:
  ci:
    uses: neon-law-source-code/navigator/.github/workflows/project-gate.yml@26.7.27
    with:
      version: "26.7.27"
"#;
        assert_eq!(findings(contents), Vec::<String>::new());
    }

    #[test]
    fn a_real_version_mismatch_is_still_caught() {
        let contents = r#"name: ci
on: [pull_request]
jobs:
  ci:
    uses: neon-law-source-code/navigator/.github/workflows/project-gate.yml@26.7.27
    with:
      version: "26.7.26"
"#;
        let found = findings(contents);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].contains("must equal its `version` input"),
            "{found:?}"
        );
    }

    #[test]
    fn a_moving_ref_is_still_refused() {
        let contents = r#"name: ci
on: [pull_request]
jobs:
  ci:
    uses: neon-law-source-code/navigator/.github/workflows/project-gate.yml@main
    with:
      version: "main"
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
    }

    /// `seeds/` is where a Project repository's `navigator site import` documents
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
            "host: staging.neonlaw.com\nproject: acme\n",
        )
        .unwrap();

        assert_eq!(layout_findings(root.path()), Vec::<String>::new());
    }

    /// An unknown key, including a retired exemption key, is refused and names
    /// the accepted set. There is no per-repository exemption mechanism.
    #[test]
    fn a_manifest_carrying_an_unknown_key_is_refused() {
        let root = tempfile::tempdir().unwrap();
        scaffold_minimal(root.path());
        std::fs::write(
            root.path().join("navigator.yaml"),
            "host: staging.neonlaw.com\nproject: acme\nexempt_roots:\n  - documents\n  - evidence\n",
        )
        .unwrap();

        let found = layout_findings(root.path());
        assert!(
            found.iter().any(|finding| finding.contains("Y006")
                && finding.contains("exempt_roots")
                && finding.contains("host")),
            "{found:?}"
        );
    }

    #[test]
    fn a_manifest_carrying_an_arbitrary_unknown_key_is_refused() {
        let root = tempfile::tempdir().unwrap();
        scaffold_minimal(root.path());
        std::fs::write(
            root.path().join("navigator.yaml"),
            "host: staging.neonlaw.com\nproject: acme\ntotally_made_up_key: whatever\n",
        )
        .unwrap();

        let found = layout_findings(root.path());
        assert!(
            found
                .iter()
                .any(|finding| finding.contains("Y006") && finding.contains("totally_made_up_key")),
            "{found:?}"
        );
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
        std::fs::write(
            root.path().join("navigator.yaml"),
            "host: staging.neonlaw.com\nproject: acme\n",
        )
        .unwrap();
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
        assert!(generated.contains("project-gate.yml@"));
        assert!(!generated.contains("project_repository: true"));
    }

    /// The pin the caller names reaches both places that carry it, and no
    /// literal survives in the generator.
    #[test]
    fn the_generated_gate_pins_the_version_it_was_given() {
        let generated = workflow("26.8.23");
        assert!(
            generated.contains(
                "uses: neon-law-source-code/navigator/.github/workflows/project-gate.yml@26.8.23"
            ),
            "{generated}"
        );
        assert!(generated.contains(r#"version: "26.8.23""#), "{generated}");
        assert!(
            !generated.contains("26.7.27"),
            "a hard-coded literal is back:\n{generated}"
        );
    }

    #[test]
    fn the_reusable_workflow_fans_five_jobs_into_the_required_check() {
        let generated = include_str!("../../../.github/workflows/project-gate.yml");
        for job in ["lint:", "verify:", "notation:", "documents:", "manifest:"] {
            assert!(
                generated.contains(&format!("\n  {job}\n")),
                "missing job `{job}`:\n{generated}"
            );
        }
        assert!(
            generated
                .contains("\n  ci:\n    needs: [lint, verify, notation, documents, manifest]\n"),
            "{generated}"
        );
    }

    #[test]
    fn the_required_check_asserts_every_dependencys_result() {
        let generated = include_str!("../../../.github/workflows/project-gate.yml");
        assert!(generated.contains("if: always()"), "{generated}");
        for job in ["lint", "verify", "notation", "documents", "manifest"] {
            assert!(
                generated.contains(&format!("needs.{job}.result")),
                "the required check does not check `{job}`'s result:\n{generated}"
            );
        }
    }

    #[test]
    fn the_application_steps_discover_every_workspace_at_run_time() {
        let generated = include_str!("../../../.github/workflows/project-gate.yml");
        assert!(
            generated.contains("package_manifests=(apps/*/package.json)"),
            "{generated}"
        );
        assert!(
            generated.contains("vite.config.ts"),
            "root-layout portals must wake the JS jobs:\n{generated}"
        );
    }

    #[test]
    fn a_root_vite_workspace_is_the_portal_application() {
        let root = tempfile::tempdir().unwrap();
        scaffold_minimal(root.path());
        std::fs::write(root.path().join("package.json"), "{}\n").unwrap();
        std::fs::write(root.path().join("vite.config.ts"), "export default {}\n").unwrap();
        std::fs::write(root.path().join("index.html"), "<!doctype html>\n").unwrap();
        std::fs::write(
            root.path().join("pnpm-lock.yaml"),
            "lockfileVersion: '9.0'\n",
        )
        .unwrap();
        let mut errors = Vec::new();
        let apps = super::application_workspaces(root.path(), &mut errors);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(apps, vec![root.path().to_path_buf()]);
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
        assert!(
            generated
                .contains("neon-law-source-code/navigator/.github/actions/seed-import@26.8.23"),
            "{generated}"
        );
        assert!(generated.contains("vars.NAVIGATOR_HOST"), "{generated}");
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
            generated
                .contains("neon-law-source-code/navigator/.github/actions/seed-import@26.8.23"),
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
            scaffold(
                root.path(),
                "example-project",
                refused,
                "staging.neonlaw.com",
                false,
            );
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
        scaffold(
            root.path(),
            "example-project",
            " 26.8.23 ",
            "staging.neonlaw.com",
            false,
        );
        let generated = std::fs::read_to_string(root.path().join(WORKFLOW)).unwrap();
        assert!(
            generated.contains(
                "uses: neon-law-source-code/navigator/.github/workflows/project-gate.yml@26.8.23"
            ),
            "{generated}"
        );
        assert!(generated.contains(r#"version: "26.8.23""#), "{generated}");
    }

    /// `Y010`: the mark with a corporate suffix is an entity claim, and the
    /// claim is measured against the entity of record the caller passes, so
    /// the spelling that *is* the entity passes and every other one fails.
    #[test]
    fn a_template_naming_the_mark_with_a_corporate_suffix_is_refused() {
        for (body, spelling) in [
            (
                "The Company, Neon Law, Inc., engages the Client.",
                "Neon Law, Inc.",
            ),
            ("**Neon Law PLLC** (the \"Firm\")", "Neon Law PLLC"),
            ("between Neon Law LLC and the Client", "Neon Law LLC"),
            ("Neon Law,   P.L.L.C.", "Neon Law, P.L.L.C."),
            (
                "engaged Neon Law Incorporated today",
                "Neon Law Incorporated",
            ),
            ("Neon Law, inc. signs below", "Neon Law, inc."),
        ] {
            assert_eq!(
                misnamed_firm_entities(body, "Shook Law PLLC"),
                vec![(1, spelling.to_string())],
                "{body}"
            );
        }
        assert_eq!(
            misnamed_firm_entities("---\ntitle: x\n---\n\nNeon Law, Inc.\n", "Shook Law PLLC"),
            vec![(5, "Neon Law, Inc.".to_string())]
        );
        // The comparison is against the entity of record, not against every
        // suffix: were the firm itself `Neon Law PLLC`, that spelling passes.
        assert_eq!(
            misnamed_firm_entities("engages Neon Law PLLC", "Neon Law PLLC"),
            Vec::new()
        );
    }

    /// The bare mark, the Licensor's own name, a longer word that merely
    /// starts with the mark, and the entity of record itself are not entity
    /// claims: a template may trade under the mark and may cite
    /// `Neon Law IP LLC` as the Licensor.
    #[test]
    fn the_mark_alone_and_the_licensor_are_not_entity_claims() {
        for body in [
            "Neon Law",
            "Neon Law.",
            "Neon Law helps you.",
            "Neon Law, a practice of Shook Law PLLC, will",
            "Neon Law IP LLC licenses the mark.",
            "Neon Lawyers Inc",
            "Neon Law Incorporates the terms",
            "Shook Law PLLC",
        ] {
            assert_eq!(
                misnamed_firm_entities(body, "Shook Law PLLC"),
                Vec::new(),
                "{body}"
            );
        }
    }

    /// Through the real template lint: the finding carries the rule code, the
    /// line, the spelling the file used, and the entity of record.
    #[test]
    fn lint_project_template_reports_y010_with_the_line_and_the_spelling() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("acme__engagement.md");
        let template = placeholder_template("acme__engagement").replace(
            "Replace this placeholder with the notation this Project actually uses.",
            "This letter engages Neon Law, Inc. (the \"Firm\").",
        );
        let line = template
            .lines()
            .position(|line| line.contains("Neon Law, Inc."))
            .expect("the body carries the spelling")
            + 1;
        std::fs::write(&path, &template).unwrap();

        let mut errors: Vec<Finding> = Vec::new();
        let mut warnings: Vec<Finding> = Vec::new();
        let mut declared = std::collections::BTreeMap::new();
        lint_project_template(
            &path,
            "acme__",
            &[],
            &mut declared,
            &mut errors,
            &mut warnings,
        );
        let messages: Vec<String> = errors.into_iter().map(|error| error.message).collect();
        assert!(
            messages.iter().any(|message| {
                message.starts_with(&format!("{ENTITY_CODE}: line {line} "))
                    && message.contains("`Neon Law, Inc.`")
                    && message.contains(store::seed::FIRM_ENTITY_NAME)
            }),
            "{messages:?}"
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    /// The scaffolded placeholder names no entity, so a fresh Project
    /// repository does not start out failing its own gate.
    #[test]
    fn the_scaffolded_placeholder_template_carries_no_y010() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("acme__engagement.md");
        std::fs::write(&path, placeholder_template("acme__engagement")).unwrap();

        let mut errors: Vec<Finding> = Vec::new();
        let mut warnings: Vec<Finding> = Vec::new();
        let mut declared = std::collections::BTreeMap::new();
        lint_project_template(
            &path,
            "acme__",
            &[],
            &mut declared,
            &mut errors,
            &mut warnings,
        );
        assert!(
            errors
                .iter()
                .all(|error| !error.message.starts_with(ENTITY_CODE)),
            "{errors:?}"
        );
    }
}
