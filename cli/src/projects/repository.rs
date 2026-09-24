//! One Project, one repository: layout and gate validation.
//!
//! A Project's repository is named for its Project code and holds notation
//! templates under `templates/` plus zero or more applications under
//! `apps/<app>/`. There is one layout and one command for both, because there
//! is one repository. A root `portal/` remains accepted while existing
//! repositories move that application to `apps/portal/`.
//!
//! ```text
//! <organization>/<project-code>
//! ├── .github/workflows/ci.yml
//! ├── .github/workflows/cd.yml
//! ├── .agents/skills/    # kept identical to Navigator's own canonical copies
//! ├── apps/              # React + Vite workspaces, discovered by package.json
//! │   └── portal/
//! ├── templates/         # *.md notation blueprints
//! ├── seeds/             # lookup_fields / records YAML for `navigator site import`
//! ├── AGENTS.md
//! ├── README.md
//! └── navigator.yaml     # the Project this repository declares
//! ```
//!
//! # Where the Project code comes from
//!
//! [`validate`] still uses the repository name for application mount checks, and
//! CI has that name as `github.event.repository.name`. Each application mount
//! is that name plus the application directory name.
//!
//! A repository also declares its release and Project coordinates in a root
//! manifest — `navigator.yaml`, with `version:` and a nested `project:` map —
//! and the gate checks the pinned `uses:` ref against it. The reusable
//! workflows themselves read `project`/`host` from that same manifest, so a
//! caller passes neither: the gate rejects any `with:` block on the `ci`,
//! `gate`, and `publish` jobs, closing off the duplicate coming back.
//! `store::sample_project::project_code_for` is what refuses a bundle
//! declaring a code other than the one it is published under, so a
//! disagreement is rejected rather than unrepresentable.
//!
//! `.github/actions/application-publish` reads this same manifest rather than
//! the repository name — its `repository:` input is now an override for a
//! checkout without one, not the primary source. [`validate`] here does not
//! follow suit: it runs inside one repository's own CI with no access to the
//! live row, so it cannot tell a repository whose manifest is wrong from one
//! whose name is; `navigator project drift` (`super::drift`) is where that
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
use std::io;
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
/// presence while reconciling the supported application layouts.
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
pub(crate) const CD_WORKFLOW: &str = ".github/workflows/cd.yml";
/// The retired generated-publish filename, accepted the same one-release way
/// as [`RETIRED_WORKFLOW`]: `validate_layout` reports a warning naming the
/// replacement rather than refusing outright, so a repository has a release
/// to rename it in before the gate stops accepting the old name. See
/// [`docs/gate.md`](../../../docs/gate.md) for the documented transition.
pub(crate) const RETIRED_CD_WORKFLOW: &str = ".github/workflows/publish.yml";
/// The last Navigator CLI release that still accepts [`RETIRED_WORKFLOW`] or
/// [`RETIRED_CD_WORKFLOW`] with a warning, closing the "one further release"
/// transition [`docs/gate.md`](../../../docs/gate.md) documents. `resolve_workflow`
/// refuses either retired filename outright once the running binary's own
/// [`crate::cli_version`] names a release after this one — see
/// `retired_workflow_refused`. Bump this only to deliberately extend the
/// transition; the ordinary path is for it to stay put and the next release
/// closes the door.
const FINAL_RETIRED_WORKFLOW_RELEASE: &str = "26.9.23";
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
const CODEOWNERS: &str = "# CODEOWNERS\n\n* @shicholas\n";
/// Seed-shaped YAML documents for `navigator site import`, one file per model.
const SEED_DIRECTORY: &str = "seeds";
const ALLOWED_ROOTS: &[&str] = &[
    ".github",
    ".gitignore",
    ".gitattributes",
    "AGENTS.md",
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
    // The manifest a Project repository declares its Project code in.
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
    // exactly that. One catalog root, matching Navigator's own: a
    // harness-specific mirror beside it is what this repository retired.
    ".agents",
];
/// The harness-specific agent-instruction mirrors this layout retired, paired
/// with what to do about one that comes back.
///
/// `AGENTS.md` is the contract and `.agents/skills/` the catalog: one file and
/// one directory, read directly by whichever harness is pointed at the tree. A
/// `CLAUDE.md`, a `.claude/`, or a `.codex/` beside them is the same
/// instructions under a name only one harness reads, and keeping two copies in
/// sync is the job nobody does — the mirrored-symlink arrangement Navigator
/// itself retired failed silently, checking out on a clone without symlink
/// support as a nine-byte `CLAUDE.md` whose *contents* were the string
/// `AGENTS.md`.
///
/// Named here rather than left to [`ALLOWED_ROOTS`] because that list catches
/// only a first component that is the *whole* path: a root `CLAUDE.md` fell
/// through it as an anonymous unenumerated root, and a committed
/// `.claude/skills/` or `.codex/skills/` was not examined at all. Matched on
/// any path component, like [`FORBIDDEN_COMPONENTS`], so an
/// `apps/portal/CLAUDE.md` does not walk around a root-only rule — and over
/// the same git-visible files every other layout rule reads, so a developer's
/// own untracked, ignored `.claude/` — harness-local state, and where a
/// worktree checkout lives — is not this gate's business.
const RETIRED_AGENT_MIRRORS: &[(&str, &str)] = &[
    (
        "CLAUDE.md",
        "`CLAUDE.md` is a retired agent-instruction mirror; `AGENTS.md` is the \
         whole contract for this repository, so delete it rather than keep a \
         second copy in sync",
    ),
    (
        ".claude",
        "`.claude/` is a retired agent-instruction mirror; `.agents/skills/` is \
         the whole catalog, so delete it",
    ),
    (
        ".codex",
        "`.codex/` is a retired agent-instruction mirror; `.agents/skills/` is \
         the whole catalog, so delete it",
    ),
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
/// joins them as the shared scope rule every Project repository's `AGENTS.md`
/// used to hand-write on its own, three times, in three different voices.
/// `legal-writing` (LAW-58) is the writing discipline both review councils
/// and the litigation filing gate assume the underlying prose already
/// meets — every Project repository drafts memos, letters, and briefs, so
/// it is fleet-wide rather than repo-local.
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
        "human-readable",
        include_str!("../../../.agents/skills/human-readable/SKILL.md"),
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
    (
        "legal-writing",
        include_str!("../../../.agents/skills/legal-writing/SKILL.md"),
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

/// Whether `dir` is a Project repository: it carries [`PROJECT_MANIFEST`] and
/// that manifest declares a `project`.
///
/// This is the one admission check every write into a repository's agent
/// contract owes before it touches disk — `cli/src/main.rs` applies the same
/// check before both `project gate`'s Project-repository layout pass and its
/// own document-pointer pass, so a tree with no `navigator.yaml` is read as
/// ordinary source, never as a Project repository missing its manifest.
pub(crate) fn is_project_repository(dir: &Path) -> bool {
    let Ok(raw) = fs::read_to_string(dir.join(PROJECT_MANIFEST)) else {
        return false;
    };
    serde_yaml::from_str::<serde_yaml::Value>(&raw)
        .ok()
        .and_then(|value| value.get("project").cloned())
        .is_some()
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
/// Commit-state rules read the files Git would carry, not the files on disk:
/// the gate judges what a pull request proposes, and an untracked scratch file
/// is not part of that.
pub(crate) fn validate_gate(root: &Path, repository: Option<&str>, write_fixes: bool) -> ExitCode {
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

    let manifest_valid = validate_layout(root, &mut errors, &mut warnings);
    validate_codeowners(root, &mut errors);
    validate_automerge_workflow(root, write_fixes, &mut errors);
    validate_documents_gitignore(root, write_fixes, &mut errors);
    validate_skills(root, &mut errors);
    validate_documented_cli(root, &mut errors);
    let has_templates = root.join(TEMPLATE_DIRECTORY).is_dir();
    let applications = application_workspaces(root, &mut errors);
    let templates = if has_templates && manifest_valid {
        validate_templates(root, &mut errors, &mut warnings)
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

/// A repository still carrying [`RETIRED_WORKFLOW`] or [`RETIRED_CD_WORKFLOW`]
/// is not failed for it — it is warned, once, by name. The one-release
/// transition this documents lives in `docs/gate.md`: this release still
/// reads the retired filename, and the release after
/// [`FINAL_RETIRED_WORKFLOW_RELEASE`] refuses it, so the warning is the
/// operator's whole notice to rename it.
fn retired_workflow_warning(path: &Path, canonical: &str) -> Finding {
    Finding::at(
        path,
        format!(
            "`{}` is the retired filename for `{canonical}`; releases through \
             {FINAL_RETIRED_WORKFLOW_RELEASE} still accept it, but the release after that \
             refuses it — rename the file to `{canonical}`",
            path.display()
        ),
    )
}

/// Whether this binary's own release, `current` (from [`crate::cli_version`]),
/// is a release after [`FINAL_RETIRED_WORKFLOW_RELEASE`] and must therefore
/// refuse [`RETIRED_WORKFLOW`] or [`RETIRED_CD_WORKFLOW`] outright rather than
/// warn about it.
///
/// An unparseable version on either side fails open — refusing a repository's
/// CI over a version string this check cannot read would be a worse failure
/// than accepting one release past the bound, and a plain local build without
/// a baked release tag falls back to `CARGO_PKG_VERSION`, which is exactly
/// [`FINAL_RETIRED_WORKFLOW_RELEASE`] until the next `chore(release)` bump —
/// so a developer's own tree only starts refusing once it actually is a
/// later release.
fn retired_workflow_refused(current: &str) -> bool {
    let Ok(current) = semver::Version::parse(current) else {
        return false;
    };
    let Ok(final_release) = semver::Version::parse(FINAL_RETIRED_WORKFLOW_RELEASE) else {
        return false;
    };
    current > final_release
}

/// The release named by [`FINAL_RETIRED_WORKFLOW_RELEASE`] has passed:
/// `path`'s retired filename is refused outright, not merely warned about.
fn retired_workflow_refusal(path: &Path, canonical: &str) -> Finding {
    Finding::at(
        path,
        format!(
            "`{}` is the retired filename for `{canonical}`; only releases through \
             {FINAL_RETIRED_WORKFLOW_RELEASE} accepted it, and this release refuses it — \
             rename the file to `{canonical}`",
            path.display()
        ),
    )
}

/// `retired` still exists beside its canonical replacement `current`, so a
/// repository can carry a structurally validated `current` and an
/// unvalidated `retired` that GitHub still runs — the coexistence bypass this
/// finding closes. Independent of [`retired_workflow_refused`]: this is
/// refused in every release, not only past the bound, because the retired
/// file is never even inspected once the canonical one is present.
fn retired_workflow_beside_canonical(retired: &Path, current: &Path) -> Finding {
    Finding::at(
        retired,
        format!(
            "`{}` exists alongside its canonical replacement `{}`; GitHub still runs whichever \
             workflows trigger, so a retired file beside the canonical one is a live, unvalidated \
             CI/CD path — delete `{}`",
            retired.display(),
            current.display(),
            retired.display()
        ),
    )
}

/// `expected` (a `.yml` path) is missing, but its `.yaml` sibling exists: name
/// the extension actually on disk and the one Navigator reads, rather than
/// reporting a bare "missing" that sends the operator looking for a typo they
/// already made correctly.
fn yaml_extension_finding(expected: &Path, canonical: &str) -> Option<Finding> {
    let yaml_path = expected.with_extension("yaml");
    if yaml_path.is_file() {
        Some(Finding::at(
            &yaml_path,
            format!(
                "`{}` uses the retired `.yaml` extension; Navigator only reads `{canonical}`",
                yaml_path.display()
            ),
        ))
    } else {
        None
    }
}

/// The fixed facts one workflow file's resolution needs: its current name,
/// its retired name, the canonical name to name in a diagnostic, and the
/// message for a bare "missing" finding.
struct WorkflowSpec<'a> {
    current: &'a Path,
    retired: &'a Path,
    canonical: &'a str,
    missing_message: &'a str,
}

/// Resolve one workflow file against `spec`'s current name, its retired
/// name, and a `.yaml` extension typo of either, then structurally validate
/// whichever content was found with `validate`.
///
/// Shared by both `ci.yml` (with [`validate_workflow`]) and `cd.yml` (with
/// [`validate_cd_workflow`]) in [`validate_layout`], which is what keeps this
/// resolution order — current name, then retired name with a warning, then
/// the precise extension diagnostic, then a bare "missing" — in exactly one
/// place instead of drifting between the two callers.
fn resolve_workflow(
    spec: &WorkflowSpec,
    manifest: Option<&super::manifest::Manifest>,
    errors: &mut Vec<Finding>,
    warnings: &mut Vec<Finding>,
    validate: impl Fn(&Path, &str, Option<&super::manifest::Manifest>, &mut Vec<Finding>),
) {
    match fs::read_to_string(spec.current) {
        Ok(contents) => {
            validate(spec.current, &contents, manifest, errors);
            if spec.retired.is_file() {
                errors.push(retired_workflow_beside_canonical(
                    spec.retired,
                    spec.current,
                ));
            }
        }
        Err(_) => match fs::read_to_string(spec.retired) {
            Ok(contents) => {
                if retired_workflow_refused(crate::cli_version()) {
                    errors.push(retired_workflow_refusal(spec.retired, spec.canonical));
                } else {
                    warnings.push(retired_workflow_warning(spec.retired, spec.canonical));
                }
                validate(spec.retired, &contents, manifest, errors);
            }
            Err(_) => match yaml_extension_finding(spec.current, spec.canonical)
                .or_else(|| yaml_extension_finding(spec.retired, spec.canonical))
            {
                Some(finding) => errors.push(finding),
                None => errors.push(Finding::at(spec.current, spec.missing_message)),
            },
        },
    }
}

fn validate_codeowners(root: &Path, errors: &mut Vec<Finding>) {
    let path = root.join(".github/CODEOWNERS");
    match fs::read_to_string(&path) {
        Ok(contents) if contents == CODEOWNERS => {}
        Ok(_) => errors.push(Finding::at(
            &path,
            "`.github/CODEOWNERS` must contain the canonical single-owner rule",
        )),
        Err(_) => errors.push(Finding::at(&path, "missing required `.github/CODEOWNERS`")),
    }
}

fn validate_documented_cli(root: &Path, errors: &mut Vec<Finding>) {
    let tree = crate::navigator_command();
    for path in super::cli_docs::markdown_paths(root) {
        let Ok(markdown) = fs::read_to_string(&path) else {
            continue;
        };
        for finding in super::cli_docs::unresolved_invocations(&markdown, &tree) {
            errors.push(Finding::at(
                path.clone(),
                format!(
                    "line {}: documented `{}` does not resolve; `{}` is not a subcommand of this navigator",
                    finding.line, finding.command, finding.verb
                ),
            ));
        }
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

/// Return the files Git considers tracked or stageable, applying every ignore
/// file in the checkout. Git's index is deliberately included even when an
/// ignore rule now matches a tracked path, so a tracked file cannot hide from
/// the source-only layout checks.
fn git_tracked_and_stageable_files(root: &Path) -> io::Result<Vec<PathBuf>> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .output()
        .map_err(|error| {
            io::Error::new(error.kind(), format!("could not run git ls-files: {error}"))
        })?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if detail.to_ascii_lowercase().contains("not a git repository") {
            "not a Git repository".to_string()
        } else if detail.is_empty() {
            "git ls-files exited unsuccessfully".to_string()
        } else {
            detail
        };
        return Err(io::Error::other(detail));
    }

    let mut files = Vec::new();
    for path in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let relative = std::str::from_utf8(path).map_err(|error| {
            io::Error::other(format!("git ls-files returned a non-UTF-8 path: {error}"))
        })?;
        let path = root.join(relative);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_file() => files.push(path),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(files)
}

fn layout_entries(root: &Path, errors: &mut Vec<Finding>) -> Option<Vec<(PathBuf, bool)>> {
    match git_tracked_and_stageable_files(root) {
        Ok(files) => Some(files.into_iter().map(|path| (path, true)).collect()),
        Err(error) => {
            errors.push(Finding::at(
                root,
                format!("could not enumerate git-tracked and stageable files: {error}"),
            ));
            None
        }
    }
}

/// Report one path that sits under a [`RETIRED_AGENT_MIRRORS`] entry, and say
/// whether it did.
///
/// Matched at any depth, like [`FORBIDDEN_COMPONENTS`]: an
/// `apps/portal/CLAUDE.md` is the same mirror as a root one, and a rule that
/// only reads the root is one an application directory walks around.
///
/// `seen` carries the mirrors already reported, so the finding is one per
/// mirror, anchored at the mirror itself: a `.claude/skills/` holds a file per
/// skill, and a directory that should not exist is one thing to delete, not
/// nine. A `true` return also suppresses the generic unenumerated-root
/// finding, which would otherwise say something vaguer about the same file.
fn retired_mirror(
    root: &Path,
    components: &[String],
    seen: &mut Vec<PathBuf>,
    errors: &mut Vec<Finding>,
) -> bool {
    let Some((depth, message)) = components
        .iter()
        .enumerate()
        .find_map(|(depth, component)| {
            RETIRED_AGENT_MIRRORS
                .iter()
                .copied()
                .find(|(mirror, _)| *mirror == component.as_str())
                .map(|(_, message)| (depth, message))
        })
    else {
        return false;
    };
    let mirror = root.join(components[..=depth].iter().collect::<PathBuf>());
    if !seen.contains(&mirror) {
        seen.push(mirror.clone());
        errors.push(Finding::at(mirror, message));
    }
    true
}

/// Close `.github/` to exactly [`CODEOWNERS`], the two thin workflow callers,
/// and their retired filenames during the one-release transition.
///
/// [`ALLOWED_ROOTS`] admits `.github` as a whole first path component, so
/// nothing else in [`validate_layout`]'s walk descends into it — this is that
/// descent. It closes which *paths* may exist; `.github/CODEOWNERS`'s
/// content is [`validate_codeowners`]'s job, and each workflow's content is
/// [`validate_workflow`]'s or [`validate_cd_workflow`]'s.
fn slash_separated_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// The auto-merge workflow every Project repository must carry, admitted to
/// the closed `.github` set alongside [`WORKFLOW`] and [`CD_WORKFLOW`].
///
/// Its content is machine-owned and validated byte-exact against
/// [`AUTOMERGE_WORKFLOW_CONTENTS`], the same shape as [`CODEOWNERS`] — no
/// repository has a legitimate reason to differ from another, so any
/// variation is drift rather than local intent.
pub(crate) const AUTOMERGE_WORKFLOW: &str = ".github/workflows/automerge.yml";
/// The byte-exact content [`AUTOMERGE_WORKFLOW`] must carry.
///
/// Arms as the merge-queue App, never as `GITHUB_TOKEN` — a token minted from
/// the run's own `GITHUB_TOKEN` starts no further workflow on `main`, so a
/// publish armed that way silently never runs (ENG-256) — and skips a draft
/// PR, with `ready_for_review` in the trigger types so a draft marked ready
/// later still fires a run rather than sitting green and unarmed forever.
/// Carries no comments of its own beyond the one pinned-action version, for
/// the same reason [`CODEOWNERS`] carries none: this file is machine-owned,
/// and a comment is the part of a hand-copied file that rots unnoticed in
/// twenty repositories. The rationale above lives here and in ENG-256, not
/// re-typed into the file itself.
const AUTOMERGE_WORKFLOW_CONTENTS: &str = r#"name: automerge

on:
  pull_request:
    types: [opened, synchronize, reopened, ready_for_review]

permissions:
  contents: read

jobs:
  enable-automerge:
    if: github.event.pull_request.draft == false
    runs-on: ubuntu-latest
    steps:
      - name: Look for the merge-queue App credentials
        id: credentials
        env:
          APP_ID: ${{ secrets.AUTOMERGE_APP_ID }}
          APP_PRIVATE_KEY: ${{ secrets.AUTOMERGE_APP_PRIVATE_KEY }}
        shell: bash
        run: |
          set -euo pipefail
          if [ -n "${APP_ID}" ] && [ -n "${APP_PRIVATE_KEY}" ]; then
              echo "present=true" >> "${GITHUB_OUTPUT}"
          else
              echo "present=false" >> "${GITHUB_OUTPUT}"
          fi
      - name: Mint a merge-queue App token
        id: app-token
        if: steps.credentials.outputs.present == 'true'
        uses: actions/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1 # v3.2.0
        with:
          app-id: ${{ secrets.AUTOMERGE_APP_ID }}
          private-key: ${{ secrets.AUTOMERGE_APP_PRIVATE_KEY }}
      - name: Arm auto-merge
        env:
          GH_TOKEN: ${{ steps.app-token.outputs.token }}
          PR_URL: ${{ github.event.pull_request.html_url }}
        shell: bash
        run: |
          set -euo pipefail
          if [ -z "${GH_TOKEN:-}" ]; then
              echo "::notice::merge-queue App credentials absent — arming nothing, merge by hand"
              exit 0
          fi
          gh pr merge --squash --auto "${PR_URL}"
"#;

fn validate_github_path(path: &Path, relative: &Path, errors: &mut Vec<Finding>) {
    let relative = slash_separated_path(relative);
    let allowed = [
        ".github/CODEOWNERS",
        WORKFLOW,
        CD_WORKFLOW,
        AUTOMERGE_WORKFLOW,
        RETIRED_WORKFLOW,
        RETIRED_CD_WORKFLOW,
    ];
    if !allowed.contains(&relative.as_ref()) {
        errors.push(Finding::at(
            path,
            format!(
                "`{relative}` is outside the closed `.github` file set; only CODEOWNERS, \
                 `{WORKFLOW}`, `{CD_WORKFLOW}`, and `{AUTOMERGE_WORKFLOW}` belong here"
            ),
        ));
    }
}

/// Hold [`AUTOMERGE_WORKFLOW`] to [`AUTOMERGE_WORKFLOW_CONTENTS`] byte-exact,
/// the same shape as [`validate_codeowners`] — but unlike that check, this one
/// self-repairs under `write_fixes`, the way [`validate_documents_gitignore`]
/// does: the file is machine-owned, so a drifted or missing copy has exactly
/// one correct byte sequence to converge on, and there is nothing a human
/// judgment call could add.
fn validate_automerge_workflow(root: &Path, write_fixes: bool, errors: &mut Vec<Finding>) {
    let path = root.join(AUTOMERGE_WORKFLOW);
    let current = fs::read_to_string(&path).ok();
    if current.as_deref() == Some(AUTOMERGE_WORKFLOW_CONTENTS) {
        return;
    }
    if write_fixes {
        if let Some(parent) = path.parent() {
            if let Err(error) = fs::create_dir_all(parent) {
                errors.push(Finding::at(
                    &path,
                    format!("could not create {}: {error}", parent.display()),
                ));
                return;
            }
        }
        if let Err(error) = fs::write(&path, AUTOMERGE_WORKFLOW_CONTENTS) {
            errors.push(Finding::at(
                &path,
                format!("could not write canonical `{AUTOMERGE_WORKFLOW}`: {error}"),
            ));
            return;
        }
        println!("fixed {}", path.display());
        return;
    }
    errors.push(Finding::at(
        &path,
        if current.is_some() {
            format!(
                "`{AUTOMERGE_WORKFLOW}` must match the canonical auto-merge workflow byte-exact"
            )
        } else {
            format!("missing required `{AUTOMERGE_WORKFLOW}`")
        },
    ));
}

fn validate_layout(root: &Path, errors: &mut Vec<Finding>, warnings: &mut Vec<Finding>) -> bool {
    if !root.join("README.md").is_file() {
        errors.push(Finding::at(
            root.join("README.md"),
            "missing required repository README",
        ));
    }

    validate_agent_contract(root, errors, warnings);
    let manifest_valid = validate_manifest(root, errors, warnings);
    let manifest = fs::read_to_string(root.join(PROJECT_MANIFEST))
        .ok()
        .and_then(|contents| super::manifest::parse(&contents).ok());

    resolve_workflow(
        &WorkflowSpec {
            current: &root.join(WORKFLOW),
            retired: &root.join(RETIRED_WORKFLOW),
            canonical: WORKFLOW,
            missing_message: "missing required CI gate",
        },
        manifest.as_ref(),
        errors,
        warnings,
        validate_workflow,
    );
    resolve_workflow(
        &WorkflowSpec {
            current: &root.join(CD_WORKFLOW),
            retired: &root.join(RETIRED_CD_WORKFLOW),
            canonical: CD_WORKFLOW,
            missing_message: "missing required CD workflow",
        },
        manifest.as_ref(),
        errors,
        warnings,
        validate_cd_workflow,
    );

    let Some(entries) = layout_entries(root, errors) else {
        return manifest_valid;
    };

    let mut retired_mirrors: Vec<PathBuf> = Vec::new();
    for (path, is_file) in entries {
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        if relative.as_os_str().is_empty() {
            continue;
        }
        let components: Vec<String> = relative
            .components()
            .filter_map(|component| component.as_os_str().to_str().map(str::to_string))
            .collect();
        validate_layout_entry(
            root,
            &path,
            is_file,
            relative,
            &components,
            &mut retired_mirrors,
            errors,
        );
    }
    manifest_valid
}

/// One git-tracked or stageable path's share of [`validate_layout`]'s walk:
/// the retired-mirror check, the closed root-set check, the `.github` closed
/// set, the `documents/` pointer-only check, the forbidden-component check,
/// and the forbidden-extension checks. Split out so `validate_layout` itself
/// stays a short list of passes rather than the loop body that runs them.
fn validate_layout_entry(
    root: &Path,
    path: &Path,
    is_file: bool,
    relative: &Path,
    components: &[String],
    retired_mirrors: &mut Vec<PathBuf>,
    errors: &mut Vec<Finding>,
) {
    let Some(first) = components.first() else {
        return;
    };
    let mirrored = retired_mirror(root, components, retired_mirrors, errors);
    if !mirrored && components.len() == 1 && !ALLOWED_ROOTS.contains(&first.as_str()) {
        errors.push(Finding::at(
            path,
            "path is outside the source-only Project repository layout",
        ));
    }
    if first == ".github" && is_file {
        validate_github_path(path, relative, errors);
    }
    if first == DOCUMENT_DIRECTORY && is_file {
        let is_pointer = crate::document_sync::is_pointer_path(path);
        let is_guard = components.len() == 2 && components[1] == ".gitignore";
        if !is_pointer && !is_guard {
            errors.push(Finding::at(
                path,
                "legal documents and raw document bytes must not be committed; keep only `*.yaml` pointers under `documents/`",
            ));
        }
    }
    if let Some(component) = components
        .iter()
        .find(|component| FORBIDDEN_COMPONENTS.contains(&component.as_str()))
    {
        errors.push(Finding::at(
            path,
            format!("forbidden `{component}` path; repositories hold source, never client material or build output"),
        ));
    }
    if is_file {
        let name = path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default();
        if name == ".env" || name.starts_with(".env.") || name.starts_with("answers.") {
            errors.push(Finding::at(
                path,
                "client answers and environment secrets must not be committed",
            ));
        }
        if FORBIDDEN_CREDENTIAL_EXTENSIONS.contains(&extension) {
            errors.push(Finding::at(
                path,
                "credential material must not be committed",
            ));
        }
        if FORBIDDEN_DOCUMENT_EXTENSIONS.contains(&extension) {
            errors.push(Finding::at(
                path,
                "legal documents and rendered output must not be committed",
            ));
        }
    }
}

/// Hold `AGENTS.md` to the one canonical contract every Project repository
/// receives. Project-specific coordinates belong in `navigator.yaml` and the
/// live Project row; checked-in agent instructions must not vary by matter.
fn validate_agent_contract(root: &Path, errors: &mut Vec<Finding>, warnings: &mut Vec<Finding>) {
    let agents_path = root.join("AGENTS.md");
    let Ok(agents) = fs::read_to_string(&agents_path) else {
        errors.push(Finding::at(
            agents_path,
            "missing required AGENTS.md; it is the agent contract for this repository",
        ));
        return;
    };
    if agents != AGENT_CONTRACT_BASE {
        errors.push(Finding::at(
            agents_path,
            "AGENTS.md must match the canonical contract in Navigator's own repository root",
        ));
    }
    let _ = warnings;
}

/// Hold a Project repository's `navigator.yaml` to the closed key set and
/// value shapes [`super::manifest::lint`] owns. There is no per-repository
/// exemption mechanism.
fn validate_manifest(root: &Path, errors: &mut Vec<Finding>, warnings: &mut Vec<Finding>) -> bool {
    let mut valid = true;
    for finding in super::manifest::lint(root) {
        if !finding.warning {
            valid = false;
        }
        let converted = Finding::at(
            finding.path,
            format!("{}: {}", finding.code, finding.message),
        );
        if finding.warning {
            warnings.push(converted);
        } else {
            errors.push(converted);
        }
    }
    valid
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
/// ## `.agents/` is the opt-in, and it is opt-in to the whole catalog
///
/// Absence used to be silent everywhere, on the same reasoning `templates/`
/// and `portal/` get: not adopted is not broken. That reasoning stops holding
/// the moment a repository has an `.agents/` directory, because then an agent
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
/// So: no `.agents/` directory, no findings — a repository that has not
/// adopted agent tooling is not failed for it. With one, every skill in the
/// catalog is required, copied byte-for-byte from Navigator's own
/// `.agents/skills/`.
///
/// This only reaches a repository when it bumps the validate action's pin, so
/// adoption stays staged rather than turning the fleet red at once.
fn validate_skills(root: &Path, errors: &mut Vec<Finding>) {
    let agent_directory = root.join(".agents");
    if !agent_directory.is_dir() {
        return;
    }
    for (name, canonical) in SYNCED_SKILLS {
        let path = root.join(".agents/skills").join(name).join("SKILL.md");
        match fs::read_to_string(&path) {
            Ok(contents) if contents == *canonical => {}
            Ok(_) => errors.push(Finding::at(
                &path,
                format!("synced skill `{name}` has drifted from the canonical copy"),
            )),
            Err(_) => errors.push(Finding::at(
                &path,
                format!(
                    "this repository has an `.agents/` directory but is missing synced skill \
                     `{name}`"
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
///
/// `pub(crate)` so [`super::super::devx::github_setup::assert_required_check_job`]
/// recognizes the exact same caller shape this validator does — two prefixes
/// for one convention is a rename waiting to leave one of them stale.
pub(crate) const PROJECT_GATE_WORKFLOW: &str =
    "neon-law-source-code/navigator/.github/workflows/project-gate.yml@";
/// Just enough of a workflow to check its trigger, its top-level
/// permissions, and each job's reusable-workflow call.
///
/// Unlike the old permissive reader this replaces, every field named here is
/// closed against: an unrecognized job, an added trigger, or a widened
/// `permissions` block is exactly what [`validate_workflow`] and
/// [`validate_cd_workflow`] exist to reject. `#[serde(default)]` on each
/// field only lets a workflow that omits it (no `on:`, no `permissions:`)
/// parse at all — the validators still treat the omission as a finding where
/// the canonical generator would have written something.
#[derive(serde::Deserialize)]
struct Workflow {
    #[serde(default, rename = "on")]
    on: Option<serde_yaml::Value>,
    #[serde(default)]
    permissions: Option<BTreeMap<String, String>>,
    #[serde(default)]
    jobs: BTreeMap<String, WorkflowJob>,
}

#[derive(serde::Deserialize)]
struct WorkflowJob {
    #[serde(default)]
    uses: Option<String>,
    #[serde(default)]
    with: BTreeMap<String, serde_yaml::Value>,
    #[serde(default)]
    needs: Option<serde_yaml::Value>,
    #[serde(default)]
    permissions: Option<BTreeMap<String, String>>,
}

/// Every trigger name an `on:` value declares, reading either the mapping
/// shape (`on:\n  pull_request:`) or the shorthand sequence
/// (`on: [pull_request]`) as the same set — GitHub Actions treats them as the
/// same trigger, so a caller that prefers one spelling over the other is a
/// harmless formatting difference, not a structural one.
fn trigger_names(on: &serde_yaml::Value) -> Option<Vec<String>> {
    match on {
        serde_yaml::Value::Mapping(map) => Some(
            map.keys()
                .filter_map(|key| key.as_str().map(str::to_string))
                .collect(),
        ),
        serde_yaml::Value::Sequence(sequence) => Some(
            sequence
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect(),
        ),
        serde_yaml::Value::String(value) => Some(vec![value.clone()]),
        _ => None,
    }
}

/// The CI caller triggers on `pull_request` and nothing else.
///
/// `pull_request_target` is named explicitly because it is not a formatting
/// variant of `pull_request` — it runs with the base repository's token and
/// secrets against a fork's checked-out code, which is exactly the
/// privilege escalation a thin, unreviewed caller must not be able to gain by
/// a one-word edit.
fn validate_ci_trigger(path: &Path, on: Option<&serde_yaml::Value>, errors: &mut Vec<Finding>) {
    let Some(names) = on.and_then(trigger_names) else {
        errors.push(Finding::at(
            path,
            "CI gate must trigger on `pull_request` and nothing else",
        ));
        return;
    };
    if names.iter().any(|name| name == "pull_request_target") {
        errors.push(Finding::at(
            path,
            "CI gate must trigger on `pull_request`, not `pull_request_target`, which runs \
             with the base repository's secrets and write token against a fork's code",
        ));
        return;
    }
    if names != ["pull_request"] {
        errors.push(Finding::at(
            path,
            format!("CI gate must trigger on exactly `pull_request`; found {names:?}"),
        ));
    }
}

/// The CD caller triggers on a push to `main` and `workflow_dispatch`, and
/// nothing else — the shorthand sequence spelling is refused here rather than
/// accepted the way [`validate_ci_trigger`] accepts it, because that shape
/// cannot express `branches: [main]`: a `cd.yml` that triggers on every
/// branch push mints the deployment token far wider than the manifest's one
/// host was ever granted.
fn validate_cd_trigger(path: &Path, on: Option<&serde_yaml::Value>, errors: &mut Vec<Finding>) {
    let Some(serde_yaml::Value::Mapping(map)) = on else {
        errors.push(Finding::at(
            path,
            "CD workflow must trigger on a `push` to `main` and `workflow_dispatch`",
        ));
        return;
    };
    let mut names: Vec<String> = map
        .keys()
        .filter_map(|key| key.as_str().map(str::to_string))
        .collect();
    if names
        .iter()
        .any(|name| name == "pull_request" || name == "pull_request_target")
    {
        errors.push(Finding::at(
            path,
            "CD workflow must not trigger on a pull request event; only a push to `main` and \
             `workflow_dispatch` mint the deployment token",
        ));
        return;
    }
    names.sort();
    if names != ["push".to_string(), "workflow_dispatch".to_string()] {
        errors.push(Finding::at(
            path,
            format!(
                "CD workflow must trigger on exactly `push` and `workflow_dispatch`; found {names:?}"
            ),
        ));
        return;
    }
    let branches: Vec<String> = map
        .get("push")
        .and_then(|push| push.get("branches"))
        .and_then(|value| value.as_sequence())
        .map(|sequence| {
            sequence
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if branches != ["main".to_string()] {
        errors.push(Finding::at(
            path,
            format!(
                "CD workflow's `push` trigger must be scoped to `branches: [main]`; found {branches:?}"
            ),
        ));
    }
}

/// When declared, the CI caller grants only checkout and OIDC token
/// permissions. Older repository fixtures may omit the block; generated
/// callers declare this exact minimum, and no declared write permission is
/// accepted.
const CI_PERMISSIONS: &[(&str, &str)] = &[("contents", "read"), ("id-token", "write")];

fn validate_ci_permissions(
    path: &Path,
    permissions: Option<&BTreeMap<String, String>>,
    errors: &mut Vec<Finding>,
) {
    let Some(found) = permissions else {
        return;
    };
    let expected: BTreeMap<String, String> = CI_PERMISSIONS
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    if *found != expected {
        errors.push(Finding::at(
            path,
            format!(
                "CI gate must declare exactly `permissions: {{contents: read, id-token: write}}`; found {found:?}"
            ),
        ));
    }
}

/// The exact scope the CD caller's token needs: read the checkout and mint
/// the OIDC token the reusable publisher exchanges for a deployment session.
/// Nothing wider — `contents: write` in particular is the privilege
/// escalation a compromised or careless edit would reach for.
const CD_PERMISSIONS: &[(&str, &str)] = &[("contents", "read"), ("id-token", "write")];

fn validate_cd_permissions(
    path: &Path,
    permissions: Option<&BTreeMap<String, String>>,
    errors: &mut Vec<Finding>,
) {
    let found = permissions.cloned().unwrap_or_default();
    let expected: BTreeMap<String, String> = CD_PERMISSIONS
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    if found != expected {
        errors.push(Finding::at(
            path,
            format!(
                "CD workflow must declare exactly `permissions: {{contents: read, id-token: write}}`; found {found:?}"
            ),
        ));
    }
}

/// Every name `needs:` lists, reading either the bare-string shape
/// (`needs: gate`) or the sequence shape (`needs: [gate]`) as the same
/// dependency.
fn needs_list(needs: Option<&serde_yaml::Value>) -> Vec<String> {
    match needs {
        Some(serde_yaml::Value::String(value)) => vec![value.clone()],
        Some(serde_yaml::Value::Sequence(sequence)) => sequence
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// One job's reusable-workflow call: it names no `permissions` of its own, no
/// `with:` block of its own, and it calls `expected_prefix` at a pinned
/// release that agrees with the repository's manifest.
///
/// The reusable workflow reads `project`/`host` from the caller's checked-out
/// `navigator.yaml` directly, so a caller has nothing left to repeat there —
/// any `with:` block at all is a duplicate of the manifest and is rejected
/// outright, closing off the same duplicate the one-job rule on `ci.yml`
/// already closes.
///
/// Shared by [`validate_workflow`] (the sole `ci` job, calling
/// [`PROJECT_GATE_WORKFLOW`]) and [`validate_cd_workflow`] (the `gate` and
/// `publish` jobs, calling [`PROJECT_GATE_WORKFLOW`] and
/// [`PROJECT_PUBLISH_WORKFLOW`] respectively).
fn validate_gate_call(
    path: &Path,
    job_name: &str,
    job: &WorkflowJob,
    expected_prefix: &str,
    manifest: Option<&super::manifest::Manifest>,
    errors: &mut Vec<Finding>,
) {
    if job.permissions.as_ref().is_some_and(|map| !map.is_empty()) {
        errors.push(Finding::at(
            path,
            format!("job `{job_name}` must not declare its own `permissions`"),
        ));
    }
    let Some(uses) = job.uses.as_deref().map(str::trim) else {
        errors.push(Finding::at(
            path,
            format!("job `{job_name}` must call `{expected_prefix}` at a pinned release"),
        ));
        return;
    };
    let Some(action_version) = uses.strip_prefix(expected_prefix) else {
        errors.push(Finding::at(
            path,
            format!(
                "job `{job_name}` must call `{expected_prefix}` at a pinned release; found `{uses}`"
            ),
        ));
        return;
    };
    if !job.with.is_empty() {
        errors.push(Finding::at(
            path,
            format!(
                "job `{job_name}` must not declare a `with:` block; the reusable workflow reads `project`/`host` from navigator.yaml"
            ),
        ));
        return;
    }
    if let Some(expected_version) = manifest.and_then(|manifest| manifest.version.as_deref()) {
        if action_version != expected_version {
            errors.push(Finding::at(
                path,
                format!(
                    "job `{job_name}` ref `{action_version}` must equal manifest version `{expected_version}`"
                ),
            ));
        }
    }
    if !is_release_tag(action_version) {
        errors.push(Finding::at(
            path,
            format!("job `{job_name}` ref `{action_version}` must be {RELEASE_TAG_SHAPE}"),
        ));
    }
}

/// Hold the CI gate to exactly one job, named [`REQUIRED_CHECK`], triggered
/// only by `pull_request`, carrying the minimal checkout/OIDC `permissions`, and calling
/// Navigator's pinned reusable project-gate workflow at an exact release tag
/// matching the repository manifest.
fn validate_workflow(
    path: &Path,
    contents: &str,
    manifest: Option<&super::manifest::Manifest>,
    errors: &mut Vec<Finding>,
) {
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

    validate_ci_trigger(path, workflow.on.as_ref(), errors);
    validate_ci_permissions(path, workflow.permissions.as_ref(), errors);

    if workflow.jobs.len() != 1 {
        let mut names: Vec<&String> = workflow.jobs.keys().collect();
        names.sort();
        errors.push(Finding::at(
            path,
            format!(
                "CI gate must define exactly one job, the pinned project-gate caller named \
                 `{REQUIRED_CHECK}`; found {names:?}"
            ),
        ));
        return;
    }
    let Some(job) = workflow.jobs.get(REQUIRED_CHECK) else {
        errors.push(Finding::at(
            path,
            format!("CI gate's one job must be named `{REQUIRED_CHECK}`"),
        ));
        return;
    };
    validate_gate_call(
        path,
        REQUIRED_CHECK,
        job,
        PROJECT_GATE_WORKFLOW,
        manifest,
        errors,
    );
}

/// Hold the CD workflow to exactly the `gate` and `publish` jobs, triggered
/// only by a push to `main` and `workflow_dispatch`, carrying exactly
/// `permissions: {contents: read, id-token: write}`, `publish` needing
/// `gate`, and each job calling its pinned reusable workflow at an exact
/// release tag matching the repository manifest.
fn validate_cd_workflow(
    path: &Path,
    contents: &str,
    manifest: Option<&super::manifest::Manifest>,
    errors: &mut Vec<Finding>,
) {
    let workflow: Workflow = match serde_yaml::from_str(contents) {
        Ok(workflow) => workflow,
        Err(error) => {
            errors.push(Finding::at(
                path,
                format!("CD workflow is not valid YAML: {error}"),
            ));
            return;
        }
    };

    validate_cd_trigger(path, workflow.on.as_ref(), errors);
    validate_cd_permissions(path, workflow.permissions.as_ref(), errors);

    let mut names: Vec<String> = workflow.jobs.keys().cloned().collect();
    names.sort();
    if names != vec!["gate".to_string(), "publish".to_string()] {
        errors.push(Finding::at(
            path,
            format!(
                "CD workflow must define exactly the `gate` and `publish` jobs; found {names:?}"
            ),
        ));
        return;
    }

    if let Some(gate) = workflow.jobs.get("gate") {
        validate_gate_call(path, "gate", gate, PROJECT_GATE_WORKFLOW, manifest, errors);
    }
    if let Some(publish) = workflow.jobs.get("publish") {
        validate_gate_call(
            path,
            "publish",
            publish,
            PROJECT_PUBLISH_WORKFLOW,
            manifest,
            errors,
        );
        let needs = needs_list(publish.needs.as_ref());
        if needs != vec!["gate".to_string()] {
            errors.push(Finding::at(
                path,
                format!("CD workflow's `publish` job must declare `needs: gate`; found {needs:?}"),
            ));
        }
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
        lint_project_template(path, &rules, &mut declared_codes, errors, warnings);
    }
    paths.len()
}

/// `Y014` — a Project repository's `documents/.gitignore` must be the canonical
/// four-line deny-all pointer admit, byte for byte.
///
/// The first line is what does the ignoring. The three negations re-admit
/// subdirectories, pointer files, and this file. A comment, a dropped `*`, or
/// any other edit leaves a file that still parses and that `git check-ignore`
/// still accepts, while the directory silently ignores nothing — or only the
/// extensions the root `.gitignore` happens to name. The explanation belongs
/// in `AGENTS.md`, not in this file.
pub const DOCUMENT_GITIGNORE_CODE: &str = "Y014";

/// When `documents/` exists, hold `documents/.gitignore` to
/// [`crate::document_sync::DOCUMENTS_GITIGNORE`]. Local `project gate` writes
/// the canonical bytes; `--ci` reports and leaves the file alone. This admits
/// only a fresh `.yaml` pointer into Git; it does not reject an already
/// committed `.yml` pointer, which `Y003` still validates under the LAW-25
/// read-compat contract ([`crate::document_sync::POINTER_READ_EXTENSIONS`]).
fn validate_documents_gitignore(root: &Path, write_fixes: bool, errors: &mut Vec<Finding>) {
    let documents = root.join(DOCUMENT_DIRECTORY);
    if !documents.is_dir() {
        return;
    }
    let path = documents.join(".gitignore");
    let current = fs::read(&path).ok();
    if current.as_deref() == Some(crate::document_sync::DOCUMENTS_GITIGNORE.as_bytes()) {
        return;
    }
    if write_fixes {
        if let Err(error) = fs::write(&path, crate::document_sync::DOCUMENTS_GITIGNORE) {
            errors.push(Finding::at(
                &path,
                format!("{DOCUMENT_GITIGNORE_CODE}: could not write canonical `documents/.gitignore`: {error}"),
            ));
            return;
        }
        println!("fixed {}", path.display());
        return;
    }
    errors.push(Finding::at(
        path,
        format!(
            "{DOCUMENT_GITIGNORE_CODE}: `documents/.gitignore` must be exactly `*`, `!*/`, `!*.yaml`, and `!.gitignore` (one per line, no comments); every other byte leaves the directory ignoring nothing, or only what the root `.gitignore` already covers"
        ),
    ));
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
    if let Some(code) = rules::frontmatter::extract(&contents)
        .and_then(|frontmatter| rules::frontmatter::field(frontmatter, "code"))
    {
        if code != stem {
            errors.push(Finding::at(
                path,
                format!("template `code` `{code}` must equal filename stem `{stem}`"),
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

/// The one `AGENTS.md` every Project repository carries. Project-specific
/// coordinates belong in `navigator.yaml` and Navigator's live Project row,
/// not in checked-in agent instructions.
const AGENT_CONTRACT_BASE: &str = include_str!("agent_contract.md");

/// The pinned publish action a Project repository's CD workflow calls.
const PROJECT_PUBLISH_WORKFLOW: &str =
    "neon-law-source-code/navigator/.github/workflows/project-publish.yml@";

/// A thin `ci.yml` caller: one required job named [`REQUIRED_CHECK`] that
/// calls Navigator's reusable project-gate workflow at `action_version`.
///
/// The jobs themselves live in `.github/workflows/project-gate.yml` in this
/// repository. Pinning that file is the thing that scales; a Project
/// repository does not copy them. The job carries no `with:` block: the
/// reusable workflow reads `project`/`host` from this repository's own
/// `navigator.yaml`, so the pinned `version` in `uses:` is the only value a
/// caller repeats.
pub(crate) fn workflow(action_version: &str) -> String {
    format!(
        r"name: {REQUIRED_CHECK}

on:
  pull_request:

permissions:
  contents: read
  id-token: write

jobs:
  {REQUIRED_CHECK}:
    uses: {PROJECT_GATE_WORKFLOW}{action_version}
"
    )
}

/// The thin `cd.yml` caller for Navigator's reusable publisher.
///
/// `gate` re-invokes `project-gate.yml` on this same push-to-`main` event so
/// its live document verification, live Project gate, and seed import run
/// here — `ci.yml` above calls that file only on `pull_request`, so nothing
/// else triggers those live jobs. `publish` `needs: gate`: a push publishes
/// only after the live checks it depends on have passed. Neither job carries
/// a `with:` block or a token-scoping comment: what the reusable workflow
/// does with the token, and where it reads `project`/`host` from, is
/// documented in the shared workflow, not repeated in every caller.
pub(crate) fn cd_workflow(action_version: &str) -> String {
    format!(
        r"name: cd

on:
  push:
    branches: [main]
  workflow_dispatch:

permissions:
  contents: read
  id-token: write

jobs:
  gate:
    uses: {PROJECT_GATE_WORKFLOW}{action_version}
  publish:
    needs: gate
    uses: {PROJECT_PUBLISH_WORKFLOW}{action_version}
",
    )
}

#[cfg(test)]
mod tests {
    use super::{
        cd_workflow, is_release_tag, lint_project_template, misnamed_firm_entities,
        repository_name, retired_workflow_refused, validate_cd_workflow, validate_github_path,
        validate_layout, validate_workflow, workflow, Finding, AGENT_CONTRACT_BASE, ALLOWED_ROOTS,
        CD_WORKFLOW, ENTITY_CODE, FINAL_RETIRED_WORKFLOW_RELEASE, PROJECT_MANIFEST,
        RETIRED_CD_WORKFLOW, RETIRED_WORKFLOW, SYNCED_SKILLS, WORKFLOW,
    };
    use crate::projects::manifest::Manifest;
    use std::fs;
    use std::path::{Path, PathBuf};

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
        validate_workflow(Path::new("gate.yml"), contents, None, &mut errors);
        errors.into_iter().map(|error| error.message).collect()
    }

    /// The messages `validate_cd_workflow` reports for one CD file.
    fn cd_findings(contents: &str) -> Vec<String> {
        let mut errors: Vec<Finding> = Vec::new();
        validate_cd_workflow(Path::new("cd.yml"), contents, None, &mut errors);
        errors.into_iter().map(|error| error.message).collect()
    }

    /// The warnings `validate_layout` reports for one checkout.
    fn layout_warnings(root: &Path) -> Vec<String> {
        let mut errors: Vec<Finding> = Vec::new();
        let mut warnings: Vec<Finding> = Vec::new();
        validate_layout(root, &mut errors, &mut warnings);
        warnings
            .into_iter()
            .map(|warning| warning.message)
            .collect()
    }

    /// The smallest checkout `validate_layout` accepts, so a test adding one
    /// file measures that file and nothing else.
    fn scaffold_minimal(root: &Path) {
        std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
        std::fs::write(root.join("README.md"), "# fixture\n").unwrap();
        std::fs::write(root.join(WORKFLOW), workflow(FIXTURE_PIN)).unwrap();
        std::fs::write(root.join(CD_WORKFLOW), cd_workflow(FIXTURE_PIN)).unwrap();
        std::fs::write(root.join("AGENTS.md"), AGENT_CONTRACT_BASE).unwrap();
        let status = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(root)
            .status()
            .unwrap();
        assert!(status.success(), "git init failed in {}", root.display());
    }

    fn layout_findings(root: &Path) -> Vec<String> {
        let mut errors: Vec<Finding> = Vec::new();
        let mut warnings = Vec::new();
        validate_layout(root, &mut errors, &mut warnings);
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
    fn gitattributes_at_the_root_is_part_of_the_layout() {
        assert!(ALLOWED_ROOTS.contains(&".gitattributes"));
    }

    #[test]
    fn a_reusable_workflow_call_with_a_matching_pin_passes() {
        let contents = r"name: ci
on: [pull_request]
jobs:
  ci:
    uses: neon-law-source-code/navigator/.github/workflows/project-gate.yml@26.7.27
";
        assert_eq!(findings(contents), Vec::<String>::new());
    }

    #[test]
    fn a_real_version_mismatch_is_still_caught() {
        let contents = r"name: ci
on: [pull_request]
jobs:
  ci:
    uses: neon-law-source-code/navigator/.github/workflows/project-gate.yml@26.7.27
";
        let manifest = Manifest {
            version: Some("26.7.26".to_string()),
            project: Some("acme".to_string()),
            ..Manifest::default()
        };
        let mut errors = Vec::new();
        validate_workflow(Path::new("ci.yml"), contents, Some(&manifest), &mut errors);
        let found: Vec<String> = errors.into_iter().map(|error| error.message).collect();
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].contains("must equal manifest version"),
            "{found:?}"
        );
    }

    #[test]
    fn a_moving_ref_is_still_refused() {
        let contents = r"name: ci
on: [pull_request]
jobs:
  ci:
    uses: neon-law-source-code/navigator/.github/workflows/project-gate.yml@main
";
        let found = findings(contents);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].contains("must be an exact release tag"),
            "{found:?}"
        );
    }

    /// The core of this issue: the reusable workflow reads `project`/`host`
    /// from the caller's own `navigator.yaml`, so a caller has nothing left
    /// to repeat there. A `with:` block of any shape is a duplicate of the
    /// manifest and is rejected outright, not merely checked for agreement.
    #[test]
    fn a_with_block_is_rejected() {
        let contents = r#"name: ci
on: [pull_request]
jobs:
  ci:
    uses: neon-law-source-code/navigator/.github/workflows/project-gate.yml@26.7.27
    with:
      project: "acme"
      host: "staging.neonlaw.com"
"#;
        let found = findings(contents);
        assert!(
            found
                .iter()
                .any(|message| message.contains("must not declare a `with:` block")),
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

    #[test]
    fn a_checkout_without_agents_md_is_refused() {
        let root = tempfile::tempdir().unwrap();
        scaffold_minimal(root.path());
        std::fs::remove_file(root.path().join("AGENTS.md")).unwrap();
        let found = layout_findings(root.path());
        assert!(
            found
                .iter()
                .any(|finding| finding.contains("missing required AGENTS.md")),
            "{found:?}"
        );
    }

    /// The generator and the gate have to agree on the bytes, or the check is
    /// theatre: a freshly scaffolded repository must be clean under the very
    /// rule that reads the file `scaffold` just wrote.
    #[test]
    fn the_scaffolded_contract_is_the_canonical_shared_file() {
        let root = tempfile::tempdir().unwrap();
        scaffold_minimal(root.path());
        assert_eq!(
            fs::read_to_string(root.path().join("AGENTS.md")).unwrap(),
            AGENT_CONTRACT_BASE
        );
        let warnings = layout_warnings(root.path());
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    /// The drift this catches is the quiet kind: the contract still *reads*
    /// correctly and still names the feedback destination, so every check that
    /// existed before this one passes. Only the byte comparison notices that
    /// one repository now tells an agent something slightly different from the
    /// other eighteen.
    #[test]
    fn a_reworded_contract_is_reported_as_an_error() {
        let root = tempfile::tempdir().unwrap();
        scaffold_minimal(root.path());
        let reworded = AGENT_CONTRACT_BASE.replace(
            "When Navigator's CLI is missing or wrong, open a Linear issue on the Lawyers team",
            "When Navigator's CLI is missing or wrong, just work around it",
        );
        std::fs::write(root.path().join("AGENTS.md"), reworded).unwrap();

        let mut errors: Vec<Finding> = Vec::new();
        let mut warnings = Vec::new();
        validate_layout(root.path(), &mut errors, &mut warnings);
        assert!(
            errors
                .iter()
                .any(|finding| finding.path.ends_with("AGENTS.md")
                    && finding.message.contains("canonical contract")),
            "{errors:?}"
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    /// Any hand-written contract is drift, including one that omits the
    /// canonical feedback and safety guidance.
    #[test]
    fn a_contract_without_the_canonical_content_is_refused() {
        let root = tempfile::tempdir().unwrap();
        scaffold_minimal(root.path());
        std::fs::write(root.path().join("AGENTS.md"), "# Working in acme\n").unwrap();

        let mut errors: Vec<Finding> = Vec::new();
        let mut warnings = Vec::new();
        validate_layout(root.path(), &mut errors, &mut warnings);
        assert!(
            errors
                .iter()
                .any(|finding| finding.message.contains("canonical contract")),
            "{errors:?}"
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    /// `AGENTS.md` is the contract, so a `CLAUDE.md` beside it is a retired
    /// mirror rather than a second copy to keep in sync. Refusing it by name —
    /// not as one more anonymous unenumerated root — is what tells the person
    /// reading CI which file to delete and which one survives.
    #[test]
    fn a_claude_md_is_refused_as_a_retired_mirror() {
        let root = tempfile::tempdir().unwrap();
        scaffold_minimal(root.path());
        std::fs::write(root.path().join("CLAUDE.md"), "AGENTS.md").unwrap();

        // Asserted on the path as well as the message: the message alone would
        // not say which file carried the finding, and the path alone would keep
        // passing if `CLAUDE.md` fell back to the generic unenumerated-root
        // wording this test exists to replace.
        let mut errors: Vec<Finding> = Vec::new();
        let mut warnings = Vec::new();
        validate_layout(root.path(), &mut errors, &mut warnings);
        assert!(
            errors
                .iter()
                .any(|finding| finding.path.ends_with("CLAUDE.md")
                    && finding.message.contains("retired agent-instruction mirror")
                    && finding
                        .message
                        .contains("`AGENTS.md` is the whole contract")),
            "{errors:?}"
        );
    }

    /// A committed `.claude/` or `.codex/` is the same mirror one directory
    /// deep, and it is the half that used to pass: nothing checked a path below
    /// its first component against [`ALLOWED_ROOTS`], so a mirrored `skills/`
    /// was admitted in full while a root `CLAUDE.md` was refused.
    #[test]
    fn a_committed_mirror_directory_is_refused_once() {
        for mirror in [".claude", ".codex"] {
            let root = tempfile::tempdir().unwrap();
            scaffold_minimal(root.path());
            for skill in ["council", "legal-council"] {
                let path = root.path().join(mirror).join("skills").join(skill);
                std::fs::create_dir_all(&path).unwrap();
                std::fs::write(path.join("SKILL.md"), "# mirror\n").unwrap();
            }

            let mut errors: Vec<Finding> = Vec::new();
            let mut warnings = Vec::new();
            validate_layout(root.path(), &mut errors, &mut warnings);
            let mirrored: Vec<&Finding> = errors
                .iter()
                .filter(|finding| finding.message.contains("retired agent-instruction mirror"))
                .collect();
            assert_eq!(
                mirrored.len(),
                1,
                "one directory to delete, not one finding per skill: {errors:?}"
            );
            assert!(mirrored[0].path.ends_with(mirror), "{mirrored:?}");
            assert!(
                mirrored[0].message.contains(".agents/skills/"),
                "the finding must name the canonical catalog: {mirrored:?}"
            );
        }
    }

    /// A developer's own `.claude/` is harness-local state — this very
    /// repository checks worktrees out under it — so the gate reads what Git
    /// would carry, not what happens to sit on disk. An ignored one is not a
    /// committed mirror and is not a finding.
    #[test]
    fn an_ignored_claude_directory_is_left_alone() {
        let root = tempfile::tempdir().unwrap();
        scaffold_minimal(root.path());
        std::fs::write(root.path().join(".gitignore"), ".claude/\n").unwrap();
        std::fs::create_dir_all(root.path().join(".claude/worktrees")).unwrap();
        std::fs::write(root.path().join(".claude/settings.json"), "{}\n").unwrap();

        let found = layout_findings(root.path());
        assert!(
            !found
                .iter()
                .any(|message| message.contains("retired agent-instruction mirror")),
            "{found:?}"
        );
    }

    /// A mirror inside an application directory is the same mirror. The root
    /// is where one is written by hand today, but `apps/<app>/` is a whole
    /// workspace with its own tooling, and a rule that reads only the root is
    /// one an application walks around.
    #[test]
    fn a_mirror_inside_an_application_is_refused_where_it_sits() {
        let root = tempfile::tempdir().unwrap();
        scaffold_minimal(root.path());
        let application = root.path().join("apps/portal");
        std::fs::create_dir_all(&application).unwrap();
        std::fs::write(application.join("CLAUDE.md"), "AGENTS.md").unwrap();

        let mut errors: Vec<Finding> = Vec::new();
        let mut warnings = Vec::new();
        validate_layout(root.path(), &mut errors, &mut warnings);
        assert!(
            errors
                .iter()
                .any(|finding| finding.path.ends_with("apps/portal/CLAUDE.md")
                    && finding.message.contains("retired agent-instruction mirror")),
            "the finding is anchored where the mirror sits, not at the root: {errors:?}"
        );
    }

    /// The catalog that survives is still accepted: this is a rename of the
    /// mirror check, not a refusal of agent tooling.
    #[test]
    fn the_canonical_agents_directory_is_not_a_retired_mirror() {
        let root = tempfile::tempdir().unwrap();
        scaffold_minimal(root.path());
        let path = root.path().join(".agents/skills/council");
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("SKILL.md"), "# council\n").unwrap();

        let found = layout_findings(root.path());
        assert!(
            !found
                .iter()
                .any(|message| message.contains("retired agent-instruction mirror")),
            "{found:?}"
        );
    }

    /// Synced skills are copied into Project repositories that do not carry
    /// Navigator's `docs/` tree. A relative link that only resolves here
    /// becomes `M057` the moment `project gate` scans `.agents/`.
    #[test]
    fn synced_skills_resolve_relative_links_outside_navigator() {
        use rules::{M057RelativeLinkResolves, Rule, SourceFile};

        for (name, contents) in SYNCED_SKILLS {
            let root = tempfile::tempdir().unwrap();
            let path = root
                .path()
                .join(".agents/skills")
                .join(name)
                .join("SKILL.md");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, contents).unwrap();
            let violations = M057RelativeLinkResolves.lint(&SourceFile {
                path: path.clone(),
                contents: (*contents).to_string(),
            });
            assert!(
                violations.is_empty(),
                "{name} still points at a Navigator-only path: {violations:?}"
            );
        }
    }

    /// ENG-870: a synced skill's documented `navigator …` invocation must
    /// resolve against *this build's* live clap tree, checked here — in
    /// Navigator's own test suite — rather than only when `project gate`
    /// happens to run inside a Project repository that carries the synced
    /// copy. That gap is exactly how a retired verb (`project repository
    /// sync-skills`, `project repository deliver`) survived in synced
    /// skill text after the command it named was removed: the caller lived
    /// in a Project repository, "not searchable from the Navigator
    /// checkout" (ENG-870's own words) — until now.
    #[test]
    fn every_synced_skill_documents_only_commands_that_resolve() {
        let tree = crate::navigator_command();
        for (name, contents) in SYNCED_SKILLS {
            let found = crate::projects::cli_docs::unresolved_invocations(contents, &tree);
            assert!(
                found.is_empty(),
                "synced skill `{name}` documents a command that does not resolve: {found:?}"
            );
        }
    }

    /// ENG-870 named this doc specifically: it still documented the
    /// retired `navigator site document verify` after that verb moved onto
    /// `navigator project gate --check`.
    #[test]
    fn project_repositories_doc_documents_only_commands_that_resolve() {
        let tree = crate::navigator_command();
        let contents = include_str!("../../../docs/project-repositories.md");
        let found = crate::projects::cli_docs::unresolved_invocations(contents, &tree);
        assert!(
            found.is_empty(),
            "docs/project-repositories.md documents a command that does not resolve: {found:?}"
        );
    }

    #[test]
    fn agents_md_must_match_the_canonical_contract() {
        let root = tempfile::tempdir().unwrap();
        scaffold_minimal(root.path());
        std::fs::write(root.path().join("AGENTS.md"), "# Working in acme\n").unwrap();
        let found = layout_findings(root.path());
        assert!(
            found
                .iter()
                .any(|finding| finding.contains("canonical contract")),
            "{found:?}"
        );
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

    #[test]
    fn a_documented_cli_verb_that_does_not_resolve_is_refused() {
        let root = tempfile::tempdir().unwrap();
        scaffold_minimal(root.path());
        std::fs::write(
            root.path().join("AGENTS.md"),
            "# contract\n\nRun `navigator template render file.md` then `navigator notation preview`.\n",
        )
        .unwrap();
        let mut errors: Vec<Finding> = Vec::new();
        super::validate_documented_cli(root.path(), &mut errors);
        let found: Vec<String> = errors.into_iter().map(|error| error.message).collect();
        assert!(
            found.iter().any(|finding| finding.contains("template")
                && finding.contains("does not resolve")),
            "{found:?}"
        );
        assert!(
            found
                .iter()
                .all(|finding| !finding.contains("notation preview")),
            "{found:?}"
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
        assert!(
            !generated.contains("with:"),
            "the caller must carry no `with:` block; the reusable workflow reads project/host from navigator.yaml:\n{generated}"
        );
        assert!(!generated.contains("push:"));
        assert!(generated.contains("permissions:\n  contents: read\n  id-token: write"));
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
        assert!(!generated.contains("with:"), "{generated}");
        assert!(!generated.contains("version:"), "{generated}");
        assert!(
            !generated.contains("26.7.27"),
            "a hard-coded literal is back:\n{generated}"
        );
    }

    /// One command gates the repository, so one job runs it. `verify` builds
    /// every application and then gates the whole tree; `documents` and
    /// `seeds` remain because each asks the deployment something the offline
    /// gate cannot.
    #[test]
    fn the_reusable_workflow_fans_two_jobs_into_the_required_check() {
        let generated = include_str!("../../../.github/workflows/project-gate.yml");
        for job in ["verify:", "documents:", "seeds:"] {
            assert!(
                generated.contains(&format!("\n  {job}\n")),
                "missing job `{job}`:\n{generated}"
            );
        }
        for retired in ["\n  lint:\n", "\n  notation:\n", "\n  manifest:\n"] {
            assert!(
                !generated.contains(retired),
                "`{retired}` ground now belongs to verify:\n{generated}"
            );
        }
        assert!(
            generated.contains("\n  ci:\n    needs: [read-manifest, verify, documents, seeds]\n"),
            "{generated}"
        );
    }

    #[test]
    fn the_required_check_asserts_every_dependencys_result() {
        let generated = include_str!("../../../.github/workflows/project-gate.yml");
        assert!(generated.contains("if: always()"), "{generated}");
        for job in ["read-manifest", "verify", "documents", "seeds"] {
            assert!(
                generated.contains(&format!("needs.{job}.result")),
                "the required check does not check `{job}`'s result:\n{generated}"
            );
        }
        for retired in [
            "needs.lint.result",
            "needs.notation.result",
            "needs.manifest.result",
        ] {
            assert!(
                !generated.contains(retired),
                "`{retired}` is retired, so nothing should still check it:\n{generated}"
            );
        }
    }

    /// The origin pass reads a built `dist/`, so the job that builds is the
    /// only job that can gate it. Asserting the step's *presence* would pass
    /// on a workflow that gated before the build and scanned a source tree, so
    /// the assertion is on the byte offsets: the build comes first, and the
    /// gate step carries `--ci` so a missing `dist/` is a finding rather than
    /// a skip.
    #[test]
    fn the_verify_job_gates_after_it_builds() {
        let generated = include_str!("../../../.github/workflows/project-gate.yml");
        let verify = generated
            .split_once("\n  verify:\n")
            .expect("no verify job")
            .1
            .split_once("\n  documents:\n")
            .expect("verify is not followed by documents")
            .0;
        let build = verify
            .find("navigator project build --dir .")
            .expect("verify does not build:\n{verify}");
        let gate = verify
            .find("navigator project gate --ci")
            .unwrap_or_else(|| panic!("verify does not run the origin gate:\n{verify}"));
        assert!(
            build < gate,
            "verify gates before it builds, so `Y009` reads a source tree:\n{verify}"
        );
        assert!(
            verify.contains("/.github/actions/navigator-install@"),
            "verify validates without installing the pinned CLI:\n{verify}"
        );
    }

    /// LAW-62: `documents` used to run `navigator project gate --check --ci`,
    /// which ran the whole gate — including the origin pass that reads the
    /// built `dist/` — not just the live document check. The job checked out
    /// and installed the CLI but never built, so every caller with a portal
    /// failed with a missing-`dist/` finding even though the live document
    /// check itself passed. The fix is in `--check` itself
    /// (`run_document_check_gate` in `cli/src/main.rs`): it now runs only the
    /// live document check, so `documents` has nothing to build — `verify`
    /// already ran the whole offline gate, including the origin pass, after
    /// its own build.
    #[test]
    fn the_documents_job_never_builds() {
        let generated = include_str!("../../../.github/workflows/project-gate.yml");
        let documents = generated
            .split_once("\n  documents:\n")
            .expect("no documents job")
            .1
            .split_once("\n  seeds:\n")
            .expect("documents is not followed by seeds")
            .0;
        assert!(
            !documents.contains("navigator project build"),
            "documents has nothing to build now that `--check` runs only the \
             live document check:\n{documents}"
        );
        assert!(
            documents.contains("navigator project gate --check --ci"),
            "{documents}"
        );
        assert!(
            documents.contains("/.github/actions/navigator-install@"),
            "documents validates without installing the pinned CLI:\n{documents}"
        );
    }

    /// ENG-674: application discovery is a CLI call now (`navigator site
    /// project applications --manifest`, then `navigator project
    /// build`), not a `hashFiles(...)`/glob guard reimplemented in the
    /// workflow. The CLI's own discovery (`application_workspaces`, above)
    /// is what wakes the JS steps for a root Vite workspace or any other
    /// layout, at run time rather than at whatever the workflow's own
    /// bash happened to check for.
    #[test]
    fn the_application_steps_discover_every_workspace_at_run_time() {
        let generated = include_str!("../../../.github/workflows/project-gate.yml");
        assert!(
            generated.contains("navigator project applications --manifest"),
            "{generated}"
        );
        assert!(
            generated.contains("navigator project build --dir ."),
            "{generated}"
        );
        assert!(
            !generated.contains("package_manifests=(apps/*/package.json)"),
            "application discovery must not be reimplemented in bash:\n{generated}"
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

    /// The generated CD caller delegates publication to Navigator's reusable workflow.
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
                "neon-law-source-code/navigator/.github/workflows/project-publish.yml@26.8.23"
            ),
            "{generated}"
        );
        assert!(generated.contains("workflow_dispatch:"), "{generated}");
        assert!(
            !generated.contains("with:"),
            "the gate/publish callers must carry no `with:` block; the reusable workflow reads project/host from navigator.yaml:\n{generated}"
        );
        assert!(
            !generated.contains("NAVIGATOR_APPLICATIONS_BUCKET"),
            "{generated}"
        );
    }

    /// The pin reaches the reusable publisher, and no literal survives.
    #[test]
    fn cd_workflow_pins_the_version_it_was_given() {
        let generated = cd_workflow("26.8.23");
        assert!(
            generated.contains(
                "neon-law-source-code/navigator/.github/workflows/project-publish.yml@26.8.23"
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

    /// The default `--action-version` pin `ops github setup` falls back to is
    /// empty, or it is a release tag — never a version-shaped string this
    /// build merely happens to carry.
    ///
    /// This is the guard that makes the invariant structural rather than
    /// asserted once: a hard-coded pin cannot be checked by anything, because
    /// it is correct on the day it is typed and nothing revisits it, while
    /// this assertion runs on every build. `cargo test` itself is the "cannot
    /// vouch for it" case — it bakes neither a runtime nor a build-time
    /// `NAVIGATOR_RELEASE_TAG` — so `published_cli_version()` is empty here.
    /// This test is what a release CLI build, or one built with
    /// `NAVIGATOR_RELEASE_TAG` set, has to satisfy instead.
    #[test]
    fn the_default_action_version_pin_is_a_release_tag_or_empty() {
        let default = crate::published_cli_version();
        if default.is_empty() {
            return;
        }
        assert!(
            is_release_tag(default),
            "the default action-version pin would be `{default}`, which is not an exact release tag"
        );
        assert_eq!(findings(&workflow(default)), Vec::<String>::new());
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
        let path = dir.path().join("onboarding.md");
        let template = concat!(
            "---\n",
            "kind: onboarding\n",
            "title: Onboarding letter\n",
            "respondent_type: entity\n",
            "code: onboarding\n",
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
            "This letter engages Neon Law, Inc. (the \"Firm\").\n",
        )
        .to_string();
        let line = template
            .lines()
            .position(|line| line.contains("Neon Law, Inc."))
            .expect("the body carries the spelling")
            + 1;
        std::fs::write(&path, &template).unwrap();

        let mut errors: Vec<Finding> = Vec::new();
        let mut warnings: Vec<Finding> = Vec::new();
        let mut declared = std::collections::BTreeMap::new();
        lint_project_template(&path, &[], &mut declared, &mut errors, &mut warnings);
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

    /// The generated CD caller passes its own structural validation, the same
    /// invariant `the_scaffolded_gate_passes_its_own_validation` proves for
    /// `ci.yml`: the generator and the validator must agree.
    #[test]
    fn the_scaffolded_cd_workflow_passes_its_own_validation() {
        assert_eq!(cd_findings(&cd_workflow(FIXTURE_PIN)), Vec::<String>::new());
    }

    /// ENG-675 reproduction 4: `pull_request_target` runs with the base
    /// repository's secrets and write token against a fork's checked-out
    /// code, so a caller that swaps it in for `pull_request` must be refused
    /// by name, not merely by shape.
    #[test]
    fn a_ci_gate_triggered_by_pull_request_target_is_refused() {
        let contents = r"name: ci
on:
  pull_request_target:
jobs:
  ci:
    uses: neon-law-source-code/navigator/.github/workflows/project-gate.yml@26.7.27
";
        let found = findings(contents);
        assert!(
            found
                .iter()
                .any(|message| message.contains("pull_request_target")),
            "{found:?}"
        );
    }

    /// ENG-675 reproduction 3: an extra job with `contents: write` and
    /// `id-token: write` next to the thin project-gate caller is a second,
    /// unreviewed door into the same required check, so a CI gate must be
    /// exactly one job.
    #[test]
    fn a_ci_gate_carrying_an_extra_privileged_job_is_refused() {
        let contents = r"name: ci
on:
  pull_request:
jobs:
  ci:
    uses: neon-law-source-code/navigator/.github/workflows/project-gate.yml@26.7.27
  smuggled:
    permissions:
      contents: write
      id-token: write
    runs-on: ubuntu-latest
    steps:
      - run: echo pwned
";
        let found = findings(contents);
        assert!(
            found
                .iter()
                .any(|message| message.contains("exactly one job")),
            "{found:?}"
        );
    }

    /// A CI gate may declare only the minimal checkout and OIDC permissions;
    /// write-capable or otherwise different grants are refused.
    #[test]
    fn a_ci_gate_declaring_permissions_is_refused() {
        let contents = r"name: ci
on:
  pull_request:
permissions:
  contents: write
  id-token: write
jobs:
  ci:
    uses: neon-law-source-code/navigator/.github/workflows/project-gate.yml@26.7.27
";
        let found = findings(contents);
        assert!(
            found
                .iter()
                .any(|message| message.contains("must declare exactly `permissions")),
            "{found:?}"
        );
    }

    /// The `gate` and `publish` jobs on `cd.yml` are the other two callers
    /// this issue names: a `with:` block is rejected there exactly as it is
    /// on `ci.yml`, not merely checked for agreement with the manifest.
    #[test]
    fn a_cd_gate_or_publish_with_block_is_rejected() {
        let contents = r#"name: cd
on:
  push:
    branches: [main]
  workflow_dispatch:
permissions:
  contents: read
  id-token: write
jobs:
  gate:
    uses: neon-law-source-code/navigator/.github/workflows/project-gate.yml@26.7.27
    with:
      project: "acme"
  publish:
    needs: gate
    uses: neon-law-source-code/navigator/.github/workflows/project-publish.yml@26.7.27
    with:
      host: "staging.neonlaw.com"
"#;
        let found = cd_findings(contents);
        assert!(
            found.iter().any(|message| message.contains("job `gate`")
                && message.contains("must not declare a `with:` block")),
            "{found:?}"
        );
        assert!(
            found.iter().any(|message| message.contains("job `publish`")
                && message.contains("must not declare a `with:` block")),
            "{found:?}"
        );
    }

    /// ENG-675 reproduction 2: an arbitrary replacement `cd.yml` — an
    /// unrelated job with none of the pinned reusable-workflow calls — must
    /// be refused by the same structural check that closes `ci.yml`.
    #[test]
    fn an_arbitrary_cd_workflow_is_refused() {
        let contents = r"name: cd
on:
  push:
    branches: [main]
  workflow_dispatch:
permissions:
  contents: read
  id-token: write
jobs:
  deploy:
    runs-on: ubuntu-latest
    steps:
      - run: ./deploy.sh
";
        let found = cd_findings(contents);
        assert!(
            found
                .iter()
                .any(|message| message.contains("exactly the `gate` and `publish` jobs")),
            "{found:?}"
        );
    }

    /// A `cd.yml` that triggers on every branch push, not only `main`, mints
    /// the deployment token far wider than the manifest's one host was ever
    /// granted.
    #[test]
    fn a_cd_workflow_not_scoped_to_main_is_refused() {
        let contents = r"name: cd
on:
  push:
    branches: [main, staging]
  workflow_dispatch:
permissions:
  contents: read
  id-token: write
jobs:
  gate:
    uses: neon-law-source-code/navigator/.github/workflows/project-gate.yml@26.7.27
  publish:
    needs: gate
    uses: neon-law-source-code/navigator/.github/workflows/project-publish.yml@26.7.27
";
        let found = cd_findings(contents);
        assert!(
            found
                .iter()
                .any(|message| message.contains("branches: [main]")),
            "{found:?}"
        );
    }

    /// A `cd.yml` widened to `contents: write` is the privilege escalation
    /// the closed permissions set exists to catch.
    #[test]
    fn a_cd_workflow_with_contents_write_is_refused() {
        let contents = r"name: cd
on:
  push:
    branches: [main]
  workflow_dispatch:
permissions:
  contents: write
  id-token: write
jobs:
  gate:
    uses: neon-law-source-code/navigator/.github/workflows/project-gate.yml@26.7.27
  publish:
    needs: gate
    uses: neon-law-source-code/navigator/.github/workflows/project-publish.yml@26.7.27
";
        let found = cd_findings(contents);
        assert!(
            found
                .iter()
                .any(|message| message.contains("permissions: {contents: read, id-token: write}")),
            "{found:?}"
        );
    }

    /// ENG-675 reproduction 1: an extra file under `.github/` — this
    /// repository holds only CODEOWNERS and the two thin callers, so
    /// anything else is a path the closed set has to name and reject.
    #[test]
    fn an_extra_github_file_is_refused() {
        let root = tempfile::tempdir().unwrap();
        scaffold_minimal(root.path());
        std::fs::write(root.path().join(".github/extra.txt"), "scratch\n").unwrap();

        let found = layout_findings(root.path());
        assert!(
            found
                .iter()
                .any(|message| message.contains(".github/extra.txt")
                    && message.contains("closed `.github` file set")),
            "{found:?}"
        );
    }

    #[test]
    fn canonical_github_paths_are_accepted_regardless_of_separator() {
        let paths = [
            PathBuf::from(".github/workflows/ci.yml"),
            PathBuf::from_iter([".github", "workflows", "ci.yml"]),
        ];
        for relative in paths {
            let mut errors = Vec::new();
            validate_github_path(Path::new("ci.yml"), &relative, &mut errors);
            assert!(errors.is_empty(), "{relative:?}: {errors:?}");
        }

        let mut errors = Vec::new();
        validate_github_path(
            Path::new("other.yml"),
            &PathBuf::from_iter([".github", "other.yml"]),
            &mut errors,
        );
        assert_eq!(errors.len(), 1, "{errors:?}");
    }

    /// This binary is past [`FINAL_RETIRED_WORKFLOW_RELEASE`], so a retired
    /// `gate.yml` is an error, not a warning — the door `docs/gate.md`
    /// promised this release would close.
    #[test]
    fn a_retired_workflow_filename_is_refused() {
        let root = tempfile::tempdir().unwrap();
        scaffold_minimal(root.path());
        let ci_contents = std::fs::read_to_string(root.path().join(WORKFLOW)).unwrap();
        std::fs::remove_file(root.path().join(WORKFLOW)).unwrap();
        std::fs::write(root.path().join(RETIRED_WORKFLOW), &ci_contents).unwrap();

        let found = layout_findings(root.path());
        assert!(
            found
                .iter()
                .any(|message| message.contains(RETIRED_WORKFLOW)
                    && message.contains(WORKFLOW)
                    && message.contains(FINAL_RETIRED_WORKFLOW_RELEASE)
                    && message.contains("this release refuses it")),
            "{found:?}"
        );
        assert_eq!(layout_warnings(root.path()), Vec::<String>::new());
    }

    /// The bound `docs/gate.md` documents: once this binary's own release is
    /// after [`FINAL_RETIRED_WORKFLOW_RELEASE`], the retired filename is
    /// refused outright rather than merely warned about — ENG-815's first
    /// gap, that nothing ever implemented the "one further release" the
    /// warning text promised.
    #[test]
    fn a_retired_workflow_filename_is_refused_past_the_final_accepting_release() {
        assert!(!retired_workflow_refused(FINAL_RETIRED_WORKFLOW_RELEASE));
        assert!(!retired_workflow_refused("0.1.0"));
        assert!(retired_workflow_refused("26.9.24"));
        assert!(retired_workflow_refused("27.1.1"));
        // Fails open on a version string it cannot parse, rather than
        // refusing a repository's CI over a value this check cannot read.
        assert!(!retired_workflow_refused("not-a-version"));
    }

    /// ENG-815's second gap: `resolve_workflow` used to inspect the retired
    /// file only when the canonical one was missing, so a repository could
    /// carry a structurally validated `ci.yml` and an unvalidated `gate.yml`
    /// that GitHub still runs. Both paths must now be named in one finding.
    #[test]
    fn a_retired_workflow_beside_its_canonical_replacement_is_refused() {
        let root = tempfile::tempdir().unwrap();
        scaffold_minimal(root.path());
        let ci_contents = std::fs::read_to_string(root.path().join(WORKFLOW)).unwrap();
        std::fs::write(root.path().join(RETIRED_WORKFLOW), &ci_contents).unwrap();

        let found = layout_findings(root.path());
        assert!(
            found
                .iter()
                .any(|message| message.contains(RETIRED_WORKFLOW)
                    && message.contains(WORKFLOW)
                    && message.contains("alongside")),
            "{found:?}"
        );
        assert_eq!(layout_warnings(root.path()), Vec::<String>::new());
    }

    #[test]
    fn a_retired_cd_workflow_filename_is_refused() {
        let root = tempfile::tempdir().unwrap();
        scaffold_minimal(root.path());
        let cd_contents = std::fs::read_to_string(root.path().join(CD_WORKFLOW)).unwrap();
        std::fs::remove_file(root.path().join(CD_WORKFLOW)).unwrap();
        std::fs::write(root.path().join(RETIRED_CD_WORKFLOW), &cd_contents).unwrap();

        let found = layout_findings(root.path());
        assert!(
            found
                .iter()
                .any(|message| message.contains(RETIRED_CD_WORKFLOW)
                    && message.contains(CD_WORKFLOW)
                    && message.contains("this release refuses it")),
            "{found:?}"
        );
        assert_eq!(layout_warnings(root.path()), Vec::<String>::new());
    }

    /// A `.yaml` spelling of the CI gate reports the extension it actually
    /// found and the one Navigator reads, not a bare "missing" that sends the
    /// operator looking for a typo they already made correctly.
    #[test]
    fn a_yaml_extension_ci_workflow_reports_the_expected_extension() {
        let root = tempfile::tempdir().unwrap();
        scaffold_minimal(root.path());
        let ci_contents = std::fs::read_to_string(root.path().join(WORKFLOW)).unwrap();
        std::fs::remove_file(root.path().join(WORKFLOW)).unwrap();
        std::fs::write(root.path().join(".github/workflows/ci.yaml"), ci_contents).unwrap();

        let found = layout_findings(root.path());
        assert!(
            found
                .iter()
                .any(|message| message.contains(".github/workflows/ci.yaml")
                    && message.contains(WORKFLOW)),
            "{found:?}"
        );
    }
}
