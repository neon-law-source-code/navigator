use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chrono::TimeZone as _;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use serde::Deserialize;

mod assets;
mod authorities;
mod credentials;
mod cut_release;
mod devx;
mod document_read;
mod document_sync;
mod firms_doctor;
mod forms_sync;
mod glossary;
#[allow(dead_code)]
mod intake;
mod login;
mod lsp_download;
mod lsp_publish;
mod mcp_bridge;
mod notations_preview;
mod notices;
mod palette;
mod projects;
mod release;
mod release_check;
mod release_default_tag;
mod release_pins;
mod release_version;
#[allow(dead_code)]
mod remote;
mod sas;
mod sendgrid_openapi;
mod surreal_archive;

use cli::import;
use devx::brand::BrandCmd;
use devx::{DnsCmd, GcpCmd, RestateCmd, StagingAction, WorktreeEnvCmd};
use projects::repository::is_project_repository;

/// The version `navigator --version` / `-V` reports.
///
/// Precedence, highest first:
/// 1. A runtime `NAVIGATOR_RELEASE_TAG` — the workspace-wide convention `web`
///    and `lsp` already follow, and the seam tests assert against.
/// 2. The tag baked at build time by `build.rs` (`NAVIGATOR_CLI_VERSION`), so a
///    *downloaded* release binary self-reports its `YY.M.D` release with no
///    environment set.
/// 3. The workspace crate version on a plain local build — `0.1.0` between
///    releases, or the `YY.M.D` a release stamped into `Cargo.toml` — since
///    `build.rs` falls back to `CARGO_PKG_VERSION` when no tag is present.
pub(crate) fn cli_version() -> &'static str {
    if let Ok(tag) = std::env::var("NAVIGATOR_RELEASE_TAG") {
        let tag = tag.trim();
        if !tag.is_empty() {
            // Leak the single resolved version string: it lives for the whole
            // process, and clap's `version` wants a `&'static str`.
            return Box::leak(tag.to_owned().into_boxed_str());
        }
    }
    env!("NAVIGATOR_CLI_VERSION")
}

/// The version `ops github setup`'s `--action-version` pins a reconciled
/// Project repository's generated gate to by default — [`cli_version`]
/// narrowed to the sources that name an *actually published* release, never
/// the bare `CARGO_PKG_VERSION` fallback `build.rs` uses so `--version` still
/// prints something on a plain local build.
///
/// A runtime `NAVIGATOR_RELEASE_TAG` only appears in a deployed container,
/// started from an image published under that tag, so it cannot precede the
/// tag. The build-time-baked `NAVIGATOR_CLI_VERSION` is trustworthy the same
/// way, but only when `NAVIGATOR_CLI_VERSION_IS_RELEASE` confirms `build.rs`
/// actually saw `NAVIGATOR_RELEASE_TAG` rather than falling back to the crate
/// version — which is bumped on `main` before the tag naming it exists. When
/// neither source is available this returns empty, and `is_release_tag`'s own
/// refusal then asks the operator to name `--action-version` themselves
/// rather than have the gate guess.
pub(crate) fn published_cli_version() -> &'static str {
    if let Ok(tag) = std::env::var("NAVIGATOR_RELEASE_TAG") {
        let tag = tag.trim();
        if !tag.is_empty() {
            return Box::leak(tag.to_owned().into_boxed_str());
        }
    }
    option_env!("NAVIGATOR_CLI_VERSION_IS_RELEASE")
        .map(|_| env!("NAVIGATOR_CLI_VERSION"))
        .unwrap_or_default()
}

/// The licence, compiled into the binary.
///
/// A downloaded `navigator` arrives as one executable with no repository and
/// no accompanying files, so the terms it is licensed under have to travel
/// inside it. `--license` prints [`NOTICE`] and then this, verbatim. Root
/// `LICENSE` stays the source of truth and `cli/tests/license_of_record.rs`
/// pins the two together, so the printed terms cannot drift from the ones the
/// repository publishes.
///
/// BUSL requires this rather than merely inviting it: the licence conditions the
/// permission to convey on displaying this License conspicuously on every copy.
/// A bare executable someone was given is a copy, and its parameters are what
/// tell that holder whether their own use needs a commercial licence — which
/// they cannot work out from terms they were never shown.
const LICENSE: &str = include_str!("../../LICENSE");

/// The Firm's own statements about the grant, compiled in beside it.
///
/// `LICENSE` is the licence text plus its parameters, so beyond naming the
/// Licensor and the Licensed Work it says little about how the grant applies
/// here. `NOTICE` is what does: the copyright line, the marks the grant does not
/// reach, the government forms nobody here can license, and where the production
/// boundary falls.
/// `--license` prints it first for that reason — the holder of a bare
/// executable has no other way to learn any of it.
const NOTICE: &str = include_str!("../../NOTICE");

/// The third-party licence notices, compiled into the binary for the same
/// reason as [`LICENSE`]: a single downloaded executable has to be able to
/// show the attributions it is obliged to carry. Regenerate with
/// `navigator ops notices`.
const THIRD_PARTY_NOTICES: &str = include_str!("../../THIRD-PARTY-NOTICES.txt");

#[derive(Parser)]
#[command(
    name = "navigator",
    version = cli_version(),
    about = "Navigator CLI, not legal advice.",
    long_about = "Navigator CLI, not legal advice."
)]
struct Cli {
    /// Print the licence this binary is distributed under, then exit. Stands
    /// alone, like `--version`.
    #[arg(long, exclusive = true)]
    license: bool,
    /// Print the licence notices for the third-party open-source components
    /// this binary incorporates, then exit. Stands alone.
    #[arg(long, exclusive = true)]
    third_party_notices: bool,
    /// Optional only so `--license` can stand alone; every other invocation
    /// still requires one, and a bare `navigator` prints help and exits 2.
    #[command(subcommand)]
    command: Option<Command>,
}

/// The live clap tree this binary dispatches. Project-repository validation
/// walks documented `navigator …` invocations against it.
pub(crate) fn navigator_command() -> clap::Command {
    Cli::command()
}

/// Return the first sentence of `text`, capped at ten words.
///
/// Clap derives help from the detailed Rust documentation beside each command.
/// The documentation remains the source of operational detail, while terminal
/// help is deliberately just a scan-friendly headline.
fn help_headline(text: &str) -> String {
    let sentence_end = text
        .char_indices()
        .find_map(|(index, character)| {
            (matches!(character, '.' | '!' | '?' | ':' | ';' | '—')
                && text[index + character.len_utf8()..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace))
            .then_some(index)
        })
        .unwrap_or(text.len());
    let words = text[..sentence_end]
        .split_whitespace()
        .take(10)
        .collect::<Vec<_>>();
    if words.is_empty() {
        return String::new();
    }
    format!("{}.", words.join(" ").trim_end_matches(['.', '!', '?']))
}

/// Replace every Clap description with its terse terminal headline.
///
/// The command tree carries all authored help, including nested subcommands
/// and arguments, so centralizing this rule keeps every path consistent.
fn concise_help(mut command: clap::Command) -> clap::Command {
    let about = command.get_about().map(ToString::to_string);
    let long_about = command.get_long_about().map(ToString::to_string);
    if let Some(about) = about {
        command = command.about(help_headline(&about));
    }
    if let Some(long_about) = long_about {
        command = command.long_about(help_headline(&long_about));
    }

    let arguments = command
        .get_arguments()
        .map(|argument| {
            (
                argument.get_id().as_str().to_owned(),
                argument.get_help().map(ToString::to_string),
                argument.get_long_help().map(ToString::to_string),
            )
        })
        .collect::<Vec<_>>();
    for (id, help, long_help) in arguments {
        command = command.mut_arg(id, |argument| {
            let argument = match help {
                Some(text) => argument.help(help_headline(&text)),
                None => argument,
            };
            match long_help {
                Some(text) => argument.long_help(help_headline(&text)),
                None => argument,
            }
        });
    }

    for subcommand in command.get_subcommands_mut() {
        let original = std::mem::replace(subcommand, clap::Command::new("placeholder"));
        *subcommand = concise_help(original);
    }
    let mut names = command
        .get_subcommands()
        .map(|subcommand| subcommand.get_name().to_owned())
        .collect::<Vec<_>>();
    names.sort_by_key(|name| (name == "help", name.clone()));
    command = command.mut_subcommands(|subcommand| {
        let order = names
            .iter()
            .position(|name| name == subcommand.get_name())
            .unwrap_or(names.len());
        subcommand.display_order(order)
    });
    command
}

#[cfg(test)]
mod help_tests {
    use super::*;

    fn assert_headlines(command: &clap::Command) {
        for text in [command.get_about(), command.get_long_about()]
            .into_iter()
            .flatten()
        {
            let text = text.to_string();
            assert!(
                text.split_whitespace().count() <= 10,
                "command `{}` is not terse: {text}",
                command.get_name()
            );
            assert!(text.ends_with('.'));
        }
        for argument in command.get_arguments() {
            for text in [argument.get_help(), argument.get_long_help()]
                .into_iter()
                .flatten()
            {
                let text = text.to_string();
                assert!(
                    text.split_whitespace().count() <= 10,
                    "argument `{}` on `{}` is not terse: {text}",
                    argument.get_id(),
                    command.get_name()
                );
                assert!(text.ends_with('.'));
            }
        }
        for subcommand in command.get_subcommands() {
            assert_headlines(subcommand);
        }
    }

    #[test]
    fn every_cli_help_description_is_a_ten_word_headline() {
        assert_headlines(&concise_help(Cli::command()));
    }

    #[test]
    fn headline_preserves_the_first_sentence() {
        assert_eq!(
            help_headline("One two three. Four five six."),
            "One two three."
        );
    }

    #[test]
    fn notation_render_timestamp_uses_source_commit_or_fixed_fallback() {
        let repository = tempfile::tempdir().expect("temporary repository");
        let source = repository.path().join("notation.md");
        std::fs::write(&source, "source").expect("write source");
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(repository.path())
                .args(args)
                .output()
                .expect("git runs");
            assert!(
                output.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "--quiet", "--initial-branch=main"]);
        git(&["config", "user.email", "notation-render@example.com"]);
        git(&["config", "user.name", "notation render"]);
        git(&["config", "commit.gpgsign", "false"]);
        git(&["add", "notation.md"]);
        let commit = std::process::Command::new("git")
            .arg("-C")
            .arg(repository.path())
            .args(["commit", "--quiet", "-m", "source"])
            .env("GIT_AUTHOR_DATE", "2024-01-02T03:04:05Z")
            .env("GIT_COMMITTER_DATE", "2024-01-02T03:04:05Z")
            .output()
            .expect("git commit runs");
        assert!(commit.status.success());

        assert_eq!(
            notation_render_timestamp(&source).timestamp(),
            chrono::DateTime::parse_from_rfc3339("2024-01-02T03:04:05Z")
                .expect("fixed test timestamp")
                .timestamp()
        );

        let non_git = tempfile::tempdir().expect("temporary input directory");
        let untracked = non_git.path().join("notation.md");
        std::fs::write(&untracked, "source").expect("write untracked source");
        assert_eq!(
            notation_render_timestamp(&untracked).timestamp(),
            NOTATION_RENDER_FALLBACK_EPOCH
        );
    }
}

// These Clap enums live only for the duration of one CLI invocation. Their
// explicit variants keep operator help and dispatch exhaustive and readable.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
enum Command {
    // ─────────────── Authoring a notation locally ───────────────
    // The notation author's workbench: everything here runs offline (or
    // against a local store), no live site required.
    /// Validate Markdown and YAML files in `<dir>` (default `.`).
    ///
    /// Walks an arbitrary directory with no assumption about the surrounding
    /// repository. A tree that is neither a Navigator checkout nor a Project
    /// repository still has this command. `project gate` remains the check
    /// over a recognised repository root.
    Validate {
        /// Directory to walk.
        #[arg(default_value = ".")]
        dir: PathBuf,
        /// Apply every safe-by-construction rule autofix
        /// (whitespace, ATX heading spacing, blockquote spacing, S102
        /// paragraph packing) to the files in place, re-scanning each
        /// file until it stops changing, then re-validate.
        /// Diagnostic-only rules (N-family notation-template, M024
        /// duplicate headings, M026 trailing punctuation) are still
        /// reported but not auto-fixed. The autofixed-source view is
        /// what the `navigator-lsp` `source.fixAll` action ships in
        /// editors.
        #[arg(long)]
        fix: bool,
        /// Print only the findings that fail the gate, hiding the
        /// Warning-severity advisories. The summary line still counts
        /// both and the exit code is unchanged: this narrows the
        /// listing for a CI-triage read, not the gate itself. Rejected
        /// with `--fix`, where a remaining warning still has to be
        /// resolved before the run passes and so must stay on screen.
        #[arg(long, conflicts_with = "fix")]
        errors_only: bool,
        /// Hold the origin pass (`Y009`) to a tree that has already been
        /// built. A Project repository's CI runs its applications' builds
        /// and then this command, so a declared application with no `dist/`
        /// means the scan read nothing and is a finding. Without the flag a
        /// missing `dist/` is skipped, which is what lets a source-only
        /// checkout validate before anyone runs a build.
        #[arg(long)]
        ci: bool,
    },
    /// Read the Neon Law Navigator glossary — the ontology, one term per
    /// file under `docs/glossary/`, embedded in this binary.
    Glossary {
        #[command(subcommand)]
        action: GlossaryCmd,
    },
    /// Interact with a project hosted on a navigator site.
    ///
    /// Singular because a checkout is one Project — the repository name *is* the Project code.
    #[command(name = "project")]
    Projects {
        #[command(subcommand)]
        action: ProjectsCmd,
    },
    /// The notation author's workbench: a Project repository's
    /// `templates/<stem>.md` (the fleet layout), or Navigator's own bundled
    /// catalog under `templates/notations/`.
    Notation {
        #[command(subcommand)]
        action: NotationCmd,
    },
    /// Vendor, pin, and inspect the blank government forms in the public assets bucket
    /// (`templates/notations/forms/`).
    Forms {
        #[command(subcommand)]
        action: FormsAction,
    },
    /// Drive a running deployment with the bearer token `navigator site login` stores.
    Site {
        #[command(subcommand)]
        action: SiteCmd,
    },
    /// Download the `navigator-lsp` release archive matching this binary's
    /// own version and the host platform, and write a working
    /// `navigator-lsp` executable to the Downloads directory.
    ///
    /// Resolves the exact `navigator-lsp-<tag>-<platform>` archive
    /// `.github/workflows/deploy.yml` attaches to *this* binary's own
    /// GitHub Release — never "latest" — via `views::lsp::LSP_RELEASE_ARCHIVES`.
    /// A version mismatch or missing release asset is refused with the tag
    /// it looked for, not a silent fallback.
    ///
    /// Distinct from `ops lsp publish` (the operator upload side) and from
    /// the Zed extension's own runtime resolution — this is the door a
    /// human without operator access uses to get the binary locally. See
    /// `docs/lsp/README.md`.
    Lsp {
        /// Directory to write the extracted `navigator-lsp` executable
        /// into. Defaults to the platform's Downloads directory.
        #[arg(long)]
        dir: Option<PathBuf>,
    },

    // ─────────────── Operator ───────────────
    /// Local, reversible KIND developer loop.
    #[command(subcommand)]
    Dev(DevCmd),
    /// Production and cloud operations with operator blast radius.
    #[command(subcommand)]
    Ops(OpsCmd),
}

#[derive(Subcommand)]
enum ProjectsCmd {
    /// Download every live document visible on this Project into the current
    /// checkout, creating or refreshing source-safe YAML pointers.
    Sync {
        /// Preview added, updated, and unchanged documents without writing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Open a matter through the live site's `POST /app/api/projects`, the
    /// caller's own bearer token attached so the conflict attestation stays
    /// a personal act.
    Create {
        /// Human-readable matter name, e.g. `"Shook Estate"`.
        #[arg(long)]
        name: String,
        /// The matter's code, e.g. `shook-estate`, stored exactly as given.
        /// Required — lowercase letters, digits, and single hyphens. A code
        /// is chosen once at matter-open and never changes; a code already
        /// in use by another matter is refused, not disambiguated.
        #[arg(long)]
        code: String,
        /// Email of the pre-existing **client** Person this matter is
        /// opened for — its client-side DRI. Required, and must be a
        /// `role = client` person (create the client first with
        /// `navigator site import person <seed-file>`). The lawyer-side DRI
        /// is the attester, resolved from this login.
        #[arg(long)]
        client_email: String,
        /// Exact `entities.name` of an **existing** legal organization this
        /// Project tracks. Omit for an individual client with no company —
        /// this then creates a `Human` entity named for the client instead,
        /// which requires `--jurisdiction`. Conflicts with `--jurisdiction`.
        #[arg(long, conflicts_with = "jurisdiction")]
        entity_name: Option<String>,
        /// The individual client's home jurisdiction, e.g. `Nevada` —
        /// required when `--entity-name` is omitted, so the `Human` entity
        /// this command creates never silently lands in the firm's own
        /// jurisdiction. Conflicts with `--entity-name`.
        #[arg(long, conflicts_with = "entity_name")]
        jurisdiction: Option<String>,
        /// The opening attorney's conflict attestation. Required on every
        /// Project open: passing `--attest` affirms the attorney has checked
        /// for conflicts, and that either none prevent the open or this
        /// Project is not legal advice. Without it the open is refused — it
        /// is never defaulted.
        #[arg(long)]
        attest: bool,
        /// Open the matter already closed — an engagement that ended
        /// before anyone opened its row. Requires `--closed-at`.
        #[arg(long, requires = "closed_at")]
        closed: bool,
        /// The close time, required exactly when `--closed` is set and
        /// refused otherwise. May predate today — there is no prior state
        /// for it to have preceded.
        #[arg(long, requires = "closed")]
        closed_at: Option<chrono::DateTime<chrono::Utc>>,
        #[command(flatten)]
        host: HostOpt,
    },
    /// Close an existing Project through the live site's lifecycle command,
    /// then archive its repository as a `closed_repository` document: zip the
    /// working tree at HEAD (no git history), record the commit SHA, and file
    /// it. The content hash needs no separate flag — the server derives it
    /// from the uploaded bytes.
    Close {
        /// Project code, resolved only against Projects visible to the login.
        project_code: String,
        /// Why the matter closed. Pitch reasons require an offboarding document;
        /// active-matter reasons require an onboarding document.
        #[arg(long)]
        reason: store::projects::ClosureReason,
        /// RFC 3339 time when the matter actually closed.
        #[arg(long)]
        effective_at: Option<chrono::DateTime<chrono::Utc>>,
        /// The local checkout to archive. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        #[command(flatten)]
        host: HostOpt,
    },
    /// Verify this machine and a Project workspace before Navigator creates
    /// anything: the active deployment, its Google Workspace, Shared Drive
    /// and Projects root, an optional local Drive mount, the stored site
    /// login, and — with `--project` — that Project's folder path and its
    /// one repository coordinate.
    ///
    /// Strictly read-only: it creates no folder, writes no file, provisions
    /// no repository, and makes no network call. A Workspace, Drive, folder,
    /// or identity mismatch exits nonzero.
    ///
    /// Distinct from `ops doctor`, which diagnoses Kubernetes scheduled-job
    /// health in a running cluster.
    Doctor {
        #[command(flatten)]
        host: HostOpt,
        /// Project code to resolve folder and repository coordinates for,
        /// e.g. `acme`. Omit to check deployment-wide configuration only.
        #[arg(long)]
        project: Option<String>,
    },
    /// Reconcile Project repositories against the live Project rows, and
    /// report where the two disagree.
    ///
    /// One `projects.code` names both a repository and a row, and nothing
    /// makes the two agree. This reads every `navigator.yaml` under `--dir`,
    /// lists the live rows, and reports both directions: a repository whose
    /// code no row carries, a row recording no repository at all, a row whose
    /// `repository_url` names a repository that is not present, and a code
    /// the two sides spell differently.
    ///
    /// The row-side findings assume `--dir` holds the whole fleet — run
    /// against part of it, "no repository is present" is true but
    /// uninteresting, so each such finding names the directory it searched.
    ///
    /// A repository that is *meant* to have no row says so in its own
    /// `navigator.yaml`, with `no_live_row: <reason>`. Those are counted in
    /// the footer and listed by `--all`, never failed.
    ///
    /// Strictly read-only: it creates no row, patches none, and closes none.
    /// Reconciling a repository to a row is a decision about a matter.
    Drift {
        #[command(flatten)]
        host: HostOpt,
        /// Directory holding the Project repository checkouts, one per
        /// repository. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        /// Also list the repositories that declare they have no live row.
        #[arg(long)]
        all: bool,
        /// Emit the report as JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Check this repository: Markdown, YAML, seeds, locales, and — when
    /// `navigator.yaml` declares a Project — its layout, mount, and origin.
    ///
    /// Runs on the whole tree from the repository root, which it identifies by
    /// the `README` and `.git` beside it, and refuses to run anywhere else.
    /// Safe-by-construction rule fixes (whitespace, ATX heading spacing,
    /// blockquote spacing, S102 paragraph packing) are applied in place; what
    /// is left needs a human. Every Project repository's CI runs this command.
    ///
    /// `--check` (LAW-62) runs only the live document check, comparing
    /// committed pointers with the live record, and skips every offline pass
    /// above entirely — a separate CI job asks it after `verify` has already
    /// run those, including the origin pass, over the same tree.
    Gate {
        /// Never writes to the live site.
        ///
        /// Checks only the live document record; every offline pass above is
        /// skipped (LAW-62). Rewrites a drifted pointer, writes a missing
        /// pointer, and writes a missing `documents/.gitignore`. A missing or
        /// corrupt object, or a live row with no slug, needs a person. Under
        /// `--ci` any of those fixes fails the job and names the fix.
        /// Uploading or removing a document is `navigator site sync`.
        #[arg(long)]
        check: bool,
        /// Re-hash every stored object while `--check` is running.
        #[arg(long, requires = "check")]
        deep: bool,
        /// Hold the run to what CI can prove. Nothing is written, so a file the
        /// gate would have fixed is a finding rather than a silent rewrite of a
        /// checkout about to be discarded. Without `--check`, the origin pass
        /// (`Y009`) reads each declared application's built `dist/` rather than
        /// skipping it — `--check` runs no offline pass at all, so it never
        /// reaches the origin pass or needs a `dist/`. Either way, on a push to
        /// `main` (or a pull request merge ref) the live-status door opens,
        /// exchanging GitHub Actions OIDC at `POST /auth/ci/document-token` to
        /// check `navigator.yaml` against the row the deployment holds. With
        /// `--check`, a pointer or gitignore the gate would write fails the
        /// job instead. The host is the one `navigator.yaml` declares — there
        /// is nothing to pass.
        #[arg(long)]
        ci: bool,
    },
    /// Install, lint, typecheck, test, and build every application this
    /// Project repository declares, one application at a time, stopping at
    /// the first failure. A repository with none still passes.
    Build {
        /// Repository root holding the application(s). Defaults to the
        /// current directory.
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
    /// List the application(s) this Project repository declares, in the
    /// same discovery order `build` runs them in.
    Applications {
        /// Repository root holding the application(s). Defaults to the
        /// current directory.
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        /// Print only the one concrete `package.json` path pnpm's own
        /// version pin needs, instead of every discovered application.
        #[arg(long)]
        manifest: bool,
    },
    /// Complete an existing Project's Drive, repository, Slack, and Notion setup.
    ///
    /// Each resource is attempted through the logged-in deployment's existing
    /// authenticated door and reported separately. A failed resource is safe
    /// to retry; already-recorded provider identities are never replaced.
    Setup {
        /// Project code. Omit only when `--all` is supplied.
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        project_code: Option<String>,
        /// Complete every Project visible to this login.
        #[arg(long)]
        all: bool,
        /// Emit one JSON result per Project and resource.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        host: HostOpt,
    },
}

#[derive(Subcommand)]
enum NotationCmd {
    /// Push one template as a **draft** to the Project it belongs to and
    /// open that Project's real portal at it — the production renderer,
    /// production chrome, and production questionnaire engine, rather
    /// than a second local imitation that has to be kept in step with it.
    ///
    /// Reads the Project and the deployment host from `navigator.yaml`
    /// two directories up, the way `navigator project gate` does. There
    /// is no `--project` to pass and no `--host` to require: a template
    /// is always previewed as the Project whose repository it sits in,
    /// against the host that repository targets. Refuses outside a
    /// Project repository rather than falling back to a local render.
    ///
    /// The draft is stored and addressable but explicitly **not run**: no
    /// Notation row, no workflow instance, no `intake_submitted`, no
    /// PDF — nothing that could be mistaken for an executed instrument or
    /// a filed document. It carries a short TTL rather than accumulating.
    Preview {
        /// The template to push: a path, or a notation `code` looked up
        /// under `templates/` and then `templates/notations/`. Underscores
        /// and hyphens are interchangeable in a name.
        file: PathBuf,
        /// Render locally instead of pushing a draft. A **lint**, not a
        /// preview: nothing is pushed and nothing is stored, and the
        /// questionnaire steps only with a Dioxus client bundle
        /// (`navigator dev build-webapp`) — without one the page still
        /// renders every question, it just does not advance. Works
        /// outside a Project repository, since nothing is pushed anywhere.
        /// Always binds an OS-assigned port chosen at random — the bound
        /// URL is printed either way — so two lints can run at once.
        #[arg(long)]
        offline: bool,
        /// Override the deployment host `navigator.yaml` names — for
        /// previewing against a non-production deployment. `navigator.yaml`
        /// is still the source; this is an override, not a replacement for
        /// it. Ignored with `--offline`.
        #[arg(long)]
        host: Option<String>,
    },
    /// Render a single notation template to PDF, framed by the render
    /// profile its declared `kind:` selects.
    ///
    /// The file is validated against the same notation rule set as
    /// `validate` first — a template with any violation is refused.
    ///
    /// A notation already declares what it is, and that is enough to pick
    /// the frame: `Kind::default_output` derives it from `kind:`, so a
    /// `letter` gets letterhead and a `will` gets the unadorned
    /// instrument. A template's own `output:` frontmatter field remains
    /// the deliberate override for the kinds that legitimately render two
    /// ways, and omitting `output:` is how a template selects the plain
    /// frame. There is no `--format` flag: it chose the frame a second
    /// time, from outside the document, and `--format letter` passed out
    /// of habit put the firm's letterhead on an executed instrument with
    /// no warning (LAW-15).
    ///
    /// A `kind: pleading` template is the one case that needs a second
    /// field: court geometry is calibrated by its `jurisdiction:`. That is
    /// resolved here, and a jurisdiction with no calibration is refused
    /// rather than quietly rendered on the plain frame.
    ///
    /// Compiled in pure Rust (no shell-out). `{{placeholder}}` tokens
    /// render verbatim unless filled with `--answer code=value`.
    Pdf {
        /// Path to the notation template (`.md`).
        file: PathBuf,
        /// Where to write the rendered PDF. Must end in `.pdf`.
        #[arg(long)]
        out: PathBuf,
        /// Fill a `{{code}}` placeholder with `value`. Repeatable:
        /// `--answer counterparty_legal_name="NEON GmbH"`.
        #[arg(long = "answer", value_parser = parse_answer)]
        answers: Vec<(String, String)>,
    },
    /// Render a single notation template to editable Word, framed the same
    /// way as `pdf` — see its documentation for how the frame, jurisdiction,
    /// and placeholders resolve.
    Word {
        /// Path to the notation template (`.md`).
        file: PathBuf,
        /// Where to write the rendered Word document. Must end in `.docx`.
        #[arg(long)]
        out: PathBuf,
        /// Fill a `{{code}}` placeholder with `value`. Repeatable:
        /// `--answer counterparty_legal_name="NEON GmbH"`.
        #[arg(long = "answer", value_parser = parse_answer)]
        answers: Vec<(String, String)>,
    },
}

#[derive(Subcommand)]
enum SiteCmd {
    /// Upload staged `documents/` bytes through Navigator and retain YAML pointers.
    #[command(after_long_help = DOCUMENT_SYNC_HELP)]
    Sync {
        /// List staged uploads without logging in or changing files.
        #[arg(long)]
        dry_run: bool,
    },
    /// Download every committed pointer's own revision into its local ignored
    /// path. Use `project sync` to discover live documents without pointers.
    Pull {
        /// List pending pulls without logging in or writing any file.
        #[arg(long)]
        dry_run: bool,
    },
    /// Import a seed-shaped YAML document through the logged-in deployment.
    ///
    /// With neither `MODEL_NAME` nor `SEED_FILE`, imports every supported
    /// seed document in `seeds/` at the repository root — the convention
    /// every Project repository already follows. With no `seeds/` directory,
    /// prints a notice and succeeds.
    Import {
        /// Singular glossary term, such as `person`, `entity`,
        /// `person_project_role`, `person_entity_role`, or `address`.
        /// Requires `SEED_FILE`; omit both to import `seeds/` at the
        /// repository root.
        #[arg(requires = "seed_file")]
        model_name: Option<String>,
        /// YAML document using the standard `lookup_fields` / `records`
        /// shape. Requires `MODEL_NAME`; omit both to import `seeds/` at the
        /// repository root.
        #[arg(requires = "model_name")]
        seed_file: Option<PathBuf>,
        /// Replace every field represented in each matching seed record.
        #[arg(long)]
        overwrite: bool,
        /// Show the per-record reconciliation plan without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Exchange a GitHub Actions OIDC token for a project-scoped seed session
        ///
        /// Requires `--host`. Does not read or write `~/.navigator.json`.
        #[arg(long, requires = "host")]
        ci: bool,
        #[command(flatten)]
        host: HostOpt,
    },
    /// File, read, and verify a Project's documents — every document is
    /// scoped to one Project, and every Project to one brand deployment
    /// (`--host`, defaulting to the sole stored login).
    Document {
        #[command(subcommand)]
        action: DocumentAction,
    },
    /// File a global Authority — the citation apparatus' shared legal
    /// reference data (#890) — with an archived artifact. Unlike `document`,
    /// an Authority carries no `--project`.
    Authorities {
        #[command(subcommand)]
        action: AuthoritiesAction,
    },
    /// File an inbound email's attachments on a live site.
    Mail {
        #[command(subcommand)]
        action: MailAction,
    },
    /// Authenticate to a live Neon Law Navigator site via a browser-loopback
    /// flow and store a short-lived (1h) bearer token at
    /// `~/.navigator.json` (mode `0600`).
    Login {
        /// Host to authenticate to, e.g. `www.neonlaw.com`. A bare host
        /// gets `https://`; pass a full URL (e.g.
        /// `http://localhost:8080`) to target a local cluster.
        #[arg(long)]
        host: String,
        /// Print the login URL without opening it. Useful for headless
        /// sessions and automated loopback-flow tests.
        #[arg(long)]
        no_browser: bool,
    },
    /// Seed the workspace-owned template and question catalog from clean files.
    Seed {
        /// Directory to walk.
        dir: PathBuf,
    },
    /// Plan or apply the versioned synthetic staging portfolio fixture:
    /// stable synthetic people, entity, matter, document, invoice mirror,
    /// trust movement, IOLTA pool, allocation, and portal bundle
    /// (`store::synthetic_portfolio`). Connects to whatever store and
    /// storage the environment names — the same seam `site seed` uses —
    /// so applying to persistent staging is an explicit operator action
    /// with the right `NAVIGATOR_SURREAL_*`/storage env pointed there.
    ///
    /// Refuses before any write unless `--target staging` is given exactly
    /// and the deployment has already disclosed
    /// `NAVIGATOR_SIMULATED_MATTERS=true`; see `docs/environments.md`.
    /// Idempotent: a repeat `--apply` inserts nothing new.
    SyntheticPortfolio {
        /// The only accepted value is `staging`.
        #[arg(long)]
        target: Option<String>,
        /// Perform the writes. Omit for a dry-run report with zero writes.
        #[arg(long)]
        apply: bool,
    },
    /// Forget the stored token for a host (or the sole logged-in host).
    Logout {
        /// Host to log out of. Optional when exactly one host is stored.
        #[arg(long)]
        host: Option<String>,
    },
    /// Print the stored identity and how long the token has left.
    Whoami {
        /// Host to inspect. Optional when exactly one host is stored.
        #[arg(long)]
        host: Option<String>,
    },
    /// Serve the Navigator MCP tool catalog to Claude as a local MCP server over
    /// stdio, dispatching each call to the host's A2A endpoint with the
    /// stored bearer token.
    ///
    /// Claude speaks MCP and has no A2A client; A2A is where the
    /// lawyer-tier check and the `audit` trail live. This bridges the two
    /// so Claude picks the tool and Navigator authorizes it.
    ///
    /// Only tools that run without a human approving them are offered:
    /// every read, plus the CRM writers (person, project, participation,
    /// bulk contact import). A tool that needs an explicit approval is
    /// not advertised, because MCP cannot pause a call to ask a person
    /// and a confirmation the model supplies to itself is not one.
    ///
    /// Speaks protocol on stdout and diagnostics on stderr. Run it from a
    /// client's server configuration, not by hand.
    Mcp {
        /// Host to serve. Optional when exactly one host is stored.
        #[arg(long)]
        host: Option<String>,
    },
    /// Inspect or drive a notation's workflow on a live site.
    Notation {
        #[command(subcommand)]
        action: NotationAction,
    },
}

#[derive(Subcommand)]
enum DevCmd {
    /// Install the pinned host dependencies the native local tier runs
    /// (`SurrealDB`, Restate, Garage) via Homebrew. macOS
    /// only — the cluster lane (`--runtime kind`) is the fallback
    /// everywhere else. Idempotent: a converged host does one `brew
    /// list` and stops, so `worktree-env up` can call it every time.
    Install,
    /// Build the current checkout's workflow worker, load it into the KIND
    /// dependency stack (`SurrealDB`, Rauthy, Garage, Restate, `OpenObserve`),
    /// open host port-forwards, and write `.devx/env` — the
    /// developer-loop entry point for editing `web` on the host.
    Up,
    /// Kill the port-forwards and delete the KIND cluster.
    Down,
    /// Print env vars (one KEY=VALUE per line) for a host-side `web`.
    Env,
    /// Show whether the cluster and port-forwards are up.
    Status,
    /// Rebuild the current checkout's `workflows-service` image, load it
    /// into KIND, and restart the in-cluster worker. Run after changing
    /// worker or shared workflow code; `dev up` does this automatically
    /// while creating a dependency tier.
    WorkerReload,
    /// Serve the local website, rebuild Rust/catalog changes, and refresh browsers.
    Serve,
    /// Build the Dioxus client bundle (issue #641): drive `dx` to compile the
    /// `webapp` crate to `wasm32-unknown-unknown` and stage it under
    /// `server/public/dioxus`, where `web` serves it same-origin to hydrate the
    /// `/dioxus-demo` page. A build artifact — gitignored, never committed;
    /// `images/Containerfile.web` runs this at image build time.
    BuildWebapp {
        /// Build with optimizations (the deploy and CI default). Omit for a
        /// faster debug build during local iteration.
        #[arg(long)]
        release: bool,
    },
    /// Guarded create/reset/status/down lifecycle for the local KIND staging boundary.
    #[command(subcommand)]
    Staging(StagingAction),
    /// KIND cluster-only helpers.
    #[command(subcommand)]
    Kind(KindCmd),
    /// Per-worktree dev checkout — stand up (or tear down) the host state
    /// scoped to the current git worktree. The default mode runs the current
    /// checkout's worker in KIND and gives each worktree its own host `web`
    /// port; `--demo` runs the full stack in-cluster from published Artifact
    /// Registry images. `up
    /// --branch <topic>` prepares a supplied agent worktree in place or
    /// creates a sibling worktree when no checkout was supplied.
    #[command(subcommand)]
    WorktreeEnv(WorktreeEnvCmd),
    /// Pull published Artifact Registry images, `kind load` them, then
    /// `kubectl apply -k k8s/overlays/kind` — the full stack
    /// including navigator-web. CI publishes the images; this no longer
    /// builds them. Pin a release with `NAVIGATOR_IMAGE_TAG`, else the
    /// latest published `YY.M.D` tag is pulled. Ends with the
    /// navigator-web rollout settling.
    Deploy,
    /// `kubectl delete namespace navigator`. Removes every Neon Law Navigator
    /// resource without touching the cluster itself.
    Undeploy,
    /// Smoke-test the deployed stack: wait for every rollout, hit
    /// `/health` through the ingress, assert the embedded Rego policy decisions,
    /// and confirm the seed data populated. Native Rust.
    E2e,
    /// Bootstrap the Garage object-storage secrets against an
    /// already-applied stack: write the `navigator-garage-control`
    /// secret so the Garage `StatefulSet` can start, wait for it to roll
    /// out, then generate the S3 access keys and write the
    /// `navigator-garage-s3` secret `navigator-web` and
    /// `workflows-service` mount. The keys are minted by Garage at
    /// runtime (`garage key create`), so they can't be static manifests
    /// — `dev up`/`dev deploy` run this inline; CI's raw
    /// `kubectl apply -k` calls it as its own step.
    GarageBootstrap,
    /// Pre-seed the Lawyer demo user (`lawyer@neonlaw.com`) with the
    /// `lawyer` role so the browser e2e's admin-gated walk can run.
    /// Native Rust.
    GrantLawyer,
    /// Refresh and stage each sample matter's reference application for the
    /// next local web boot. The checkouts and builds happen in temporary
    /// directories; each built `dist/` and `navigator.yaml` survives under
    /// `.devx/sample-projects/<code>`, and the generated `.devx/env` points
    /// `web` at the parent. Needs `git` and `pnpm`. Native Rust.
    SampleProject {
        /// Refresh only this Project's application. Defaults to all of them;
        /// naming one is the fast loop while iterating on a single app.
        #[arg(long)]
        project: Option<String>,
        /// Repository to clone. Defaults to the URL recorded on the Project;
        /// override to build a fork or a local mirror. Requires `--project`,
        /// since one URL cannot serve every matter.
        #[arg(long, requires = "project")]
        repo: Option<String>,
        /// Branch or tag to build. Defaults to the repository's default
        /// branch.
        #[arg(long = "ref")]
        git_ref: Option<String>,
        /// Keep the temporary checkout and build tree instead of removing
        /// it, to debug a failed build.
        #[arg(long)]
        keep: bool,
    },
    /// Reproduce `deploy.yml`'s browser gate locally: resolve the
    /// pinned Chrome for Testing build, start its chromedriver, verify
    /// the host `web` is reachable, grant Lawyer in the worktree store,
    /// then run the `browser_e2e` +
    /// `accessibility_e2e` suites with `NAV_REQUIRE_HARNESS=1` (a
    /// self-skip fails instead of passing green). Bring the fixture up
    /// (`dev worktree-env up`) and start `web` first.
    BrowserE2e {
        /// Base URL of the running web server. Defaults to
        /// `$NAV_BASE_URL`, else `http://localhost:$PORT` from the
        /// worktree's sourced `.devx/env`.
        #[arg(long, env = "NAV_BASE_URL")]
        base_url: Option<String>,
    },
    /// Loop one package's tests N times and print the pass/fail wall-time
    /// distribution. Each run is classified from nextest's Summary line (or
    /// cucumber's `test result:` / scenarios line), never from a pipe's
    /// exit status.
    FlakeHunt {
        /// Cargo package to run (`store`, `portal`, `features`, …).
        package: String,
        /// Optional nextest filter expression (`-E`). For `features`, the
        /// cucumber binary name passed to `cargo test --test`.
        test_filter: Option<String>,
        /// How many consecutive runs to record.
        #[arg(long, default_value_t = 10)]
        runs: u32,
    },
    /// Tail `navigator-web` logs (`kubectl logs -f deployment/navigator-web`).
    Logs,
    /// Render Kubernetes overlays locally.
    #[command(subcommand)]
    Kustomize(KustomizeCmd),
    /// Verify or deterministically regenerate the pinned `SendGrid` Mail API
    /// client input. No schema is fetched during ordinary builds.
    SendgridOpenapi {
        /// Verify the vendored contract and generated adapter (default).
        #[arg(long)]
        verify: bool,
        /// Run the offline regeneration check.
        #[arg(long)]
        regenerate: bool,
        /// Workspace root containing vendor/sendgrid.
        #[arg(default_value = ".")]
        root: PathBuf,
    },
}

#[derive(Subcommand)]
enum KindCmd {
    /// Create the KIND cluster and install nginx-ingress + the
    /// Restate Operator. Does not apply any application manifests.
    /// Use this when you want the cluster prepared but plan to apply
    /// k8s/ manifests by hand.
    Up,
    /// Delete the KIND cluster. Does not touch the local Docker
    /// images or the host port-forward state file.
    Down,
}

#[derive(Subcommand)]
enum KustomizeCmd {
    /// `kubectl kustomize k8s/overlays/kind` — render the full local
    /// stack to stdout for inspection. Useful when debugging a
    /// kustomize overlay before applying it.
    Kind,
    /// `kubectl kustomize k8s/overlays/gke` — render the production
    /// overlay to stdout for inspection. Config Sync owns the actual
    /// apply in production; this is the local equivalent of "what
    /// will the cluster see?"
    Gke,
}

// See `Command`: boxing nested Clap subcommands would make every dispatch
// pattern noisier without reducing steady-state application memory.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
enum OpsCmd {
    /// Publish a Project application to a deployment's applications bucket.
    /// The operator lane beside a Project repository's own CI publisher: the
    /// public sample repositories carry no publish workflow, and a
    /// production-profile boot never writes a portal bundle, so putting a
    /// bundle up — or back — from a machine is this command.
    Application {
        #[command(subcommand)]
        action: ApplicationAction,
    },
    /// Reconcile one named GitHub repository's merge protections and policy.
    /// Reads `GITHUB_TOKEN`; the required repository argument is never inferred
    /// from the environment or a checkout's `origin` remote.
    #[command(subcommand)]
    Github(GithubCmd),
    /// One-shot deployment reconciler — the "do everything" button documented
    /// in `docs/cloud-operations.md`. CI (`deploy.yml`) builds and publishes
    /// the images to GHCR under an immutable release tag; `ops ship` reconciles
    /// the cluster. Flow: take the `--tag` (`YY.M.D` or
    /// `YY.M.D-hotfix.N`) →
    /// reconcile the manifests (render the embedded GKE tree with the selected
    /// deployment's `NAVIGATOR_*` coordinates into a temp dir and `kubectl
    /// apply -k` it) → confirm the selected deployment's Secret satisfies the
    /// new binary's boot invariants → roll out its runtime workloads at that
    /// tag → pin every trigger `CronJob` to the same tag → re-register the
    /// worker with Restate, so every navigator image ends in sync at one
    /// immutable release tag. Reads every project / region / domain / cluster value from
    /// the repository's `deployments/<name>/config.toml`, selected by the
    /// required `--deployment` flag — never from the process environment, so a
    /// stale shell cannot select the wrong deployment. Never builds images
    /// locally and needs no external overlay folder.
    Ship {
        /// Deployment directory under `deployments/`, such as `neon-law-stg`.
        /// A deployment exists because its directory exists; there is no
        /// environment fallback.
        #[arg(long)]
        deployment: String,
        /// The directory CONTAINING `deployments/` — the same directory
        /// `.sops.yaml` sits in. Point it at a checkout that carries the
        /// deployment tree and nothing else; the GKE manifests are compiled
        /// into this binary, so no source tree has to be present. Defaults to
        /// `NAVIGATOR_DEPLOYMENTS_DIR`, then to the discovered workspace root.
        #[arg(long, value_name = "DIR")]
        deployments_dir: Option<PathBuf>,
        /// Run read-only context, Secret, render, and diff preflights; never
        /// apply manifests, restart workloads, or change registrations.
        #[arg(long)]
        dry_run: bool,
        /// No-rebuild path: restart the selected deployment's runtime
        /// workloads so the pods re-read a rotated Secret value, then exit.
        /// Use after rotating a key in the K8s Secret.
        #[arg(long)]
        restart_only: bool,
        /// Move every navigator image to `--tag` and change nothing else —
        /// the lane an automated deploy runs under a credential that can do
        /// no more than bump a version. Refuses when the rendered manifests
        /// differ from the cluster, because it applies none of that diff.
        #[arg(long)]
        image_only: bool,
        /// Immutable registry tag to roll onto: `YY.M.D`
        /// or `YY.M.D-hotfix.N`. Required for a roll; omit only with
        /// `--restart-only`.
        #[arg(long)]
        tag: Option<String>,
        /// Verify the web service account's self-signing IAM binding instead
        /// of establishing it: when the binding is absent the roll stops and
        /// prints the `gcloud` command that would grant it, rather than
        /// running that command itself. The lane for an operator who holds
        /// the release tag but not `iam.serviceAccounts.setIamPolicy` — the
        /// binding is still asserted, since a web pod without it 500s every
        /// document download; only the grant moves to someone permitted it.
        #[arg(long)]
        assert_signing_iam: bool,
    },
    /// Export `SurrealDB` into the firm-controlled object-storage archive, or
    /// prove one stored export can be restored into a disposable namespace.
    #[command(subcommand)]
    SurrealArchive(SurrealArchiveCmd),
    /// Check a `deployments/` tree without changing anything or decrypting
    /// anything. Every row loads, no decrypted file sits beside an encrypted
    /// one, every `.sops.yaml` rule agrees with the key its row declares, and
    /// every provisioned row satisfies the boot requirements that apply to it
    /// and supplies every object its pod's Secret projects.
    ///
    /// The gate the tree's own CI runs. The workspace suite asserts the same
    /// things against `cli/tests/fixtures/deployment-tree/`, which is a
    /// fixture — this is how the real rows, which live in a private
    /// repository, are held to it too.
    ///
    /// Names only: no value is read, so it needs no KMS grant, no credential,
    /// and no network.
    Deployments {
        /// The directory CONTAINING `deployments/` — the same flag the other
        /// tree commands take. Defaults to `NAVIGATOR_DEPLOYMENTS_DIR`, then
        /// to the discovered workspace root.
        #[arg(long, value_name = "DIR")]
        deployments_dir: Option<PathBuf>,
    },
    /// Deployment key material. The `deployments/` tree is the operator
    /// source: plaintext coordinates beside SOPS-encrypted values, decrypted
    /// only by `apply` and written into that deployment's own Secret Manager.
    #[command(subcommand)]
    Secrets(SecretsCmd),
    /// Operator recovery for the inbound email-summary Restate workflow.
    #[command(subcommand)]
    EmailSummary(EmailSummaryCmd),
    /// GCP project provisioning. The actual REST plumbing lives in
    /// `cli/src/devx/gcp/`; this is the entry point operators reach for
    /// when standing up (or re-running) Neon Law Navigator on a fresh GCP
    /// project.
    #[command(subcommand)]
    Gcp(GcpCmd),
    /// Restate Cloud CLI wrappers. Saves operators from memorizing
    /// the `restate deployment register …` invocation. Assumes the
    /// caller has already run `restate -y cloud login` and
    /// `restate -y cloud env config --env <your-env-name>` (or set
    /// `RESTATE_CLOUD_TOKEN`/`RESTATE_ENVIRONMENT` in CI).
    #[command(subcommand)]
    Restate(RestateCmd),
    /// Diagnose ongoing scheduled-job health: surface trigger Jobs wedged in
    /// `ImagePullBackOff`/`CrashLoopBackOff` (which, under a `CronJob`'s
    /// `concurrencyPolicy: Forbid`, silently skip every subsequent run) and
    /// workloads that aren't fully ready, each with the command that fixes it.
    /// Read-only `kubectl get` against the current context.
    Doctor {
        /// Namespace to inspect. Defaults to `NAVIGATOR_K8S_NAMESPACE` / `navigator`.
        #[arg(long)]
        namespace: Option<String>,
    },
    /// DNS provisioning for a public deploy — reachability, the apex→www
    /// redirect, and both mail lanes — via the configured DNS provider
    /// (`DNSimple` today). Reads `DNS_ZONE` / `DNS_ACCT` / `DNS_SIMPLE`
    /// (`DNSIMPLE_API_TOKEN` remains a legacy alias). Idempotent: matching
    /// records are no-ops.
    #[command(subcommand)]
    Dns(DnsCmd),
    /// Bounded per-host release-readiness check (ENG-808): for every host
    /// the release inventory (`views::brand::release_brand_hosts`) covers on
    /// the selected deployment's environment, perform a real TLS handshake
    /// through the ordinary trust store (never `-k`) and confirm the actual
    /// response — a live brand's own `og:site_name` at `200`, a held-out
    /// brand's `404`, and (production only) every live brand's apex
    /// redirect. Never mutates anything; exits nonzero and names every
    /// failing host when any check is not ready. Safe to rerun as often as
    /// needed — this is a point-in-time receipt, not a wait-until-ready loop.
    BrandReadiness {
        /// Deployment directory under `deployments/`, such as `neon-law-stg`.
        #[arg(long)]
        deployment: String,
        /// The directory CONTAINING `deployments/` — the same flag `ops ship`
        /// takes. Defaults to `NAVIGATOR_DEPLOYMENTS_DIR`, then to the
        /// discovered workspace root.
        #[arg(long, value_name = "DIR")]
        deployments_dir: Option<PathBuf>,
        /// Per-host `curl` timeout, in seconds. Bounds the whole run to at
        /// most this many seconds times the number of hosts checked.
        #[arg(long, default_value_t = 15)]
        timeout_seconds: u32,
    },
    /// Deprecated: use the white-label bundle workflow documented in
    /// `docs/oss-install.md`, whose worked manifest is
    /// `cli/tests/fixtures/navigator.example.yaml`.
    #[command(subcommand)]
    Rebrand(BrandCmd),
    /// Stand up the `OTel` Collector seam in prod and wire the binaries to
    /// it: ensure the `navigator-otel` GSA + telemetry-write IAM +
    /// Workload Identity, apply the Collector + self-monitoring
    /// manifests, and `envFrom` the shared `navigator-otel-env` `ConfigMap`
    /// onto `navigator-web` + `workflows-service` so
    /// `OTEL_EXPORTER_OTLP_ENDPOINT` reaches `telemetry::init`. Idempotent;
    /// reads project/region/cluster/context from the selected deployment's
    /// `deployments/<name>/config.toml`. Run once per cluster, then
    /// `ops ship` (or rollout-restart) the binaries.
    Observability {
        /// Deployment directory under `deployments/`, such as `neon-law-stg`.
        #[arg(long)]
        deployment: String,
        /// The directory CONTAINING `deployments/` — the same flag `ops ship`
        /// and `ops secrets apply` take. Defaults to
        /// `NAVIGATOR_DEPLOYMENTS_DIR`, then to the discovered workspace root.
        #[arg(long, value_name = "DIR")]
        deployments_dir: Option<PathBuf>,
        /// Print every command instead of running it.
        #[arg(long)]
        dry_run: bool,
    },
    /// Distribute the `navigator-lsp` editor binary.
    Lsp {
        #[command(subcommand)]
        action: LspAction,
    },
    /// Transcode curated source photos into responsive web variants.
    Assets {
        #[command(subcommand)]
        action: AssetsAction,
    },
    /// Name today's UTC `YY.M.D` and write it as the workspace version, or
    /// fail if that name is not a new release.
    ///
    /// Compares today's date against published tags and delegates the write
    /// to `ops release version`. A covered date or operational error exits 2.
    /// Hotfixes and other explicit names use `ops release version --tag`.
    CutRelease {
        /// Git checkout supplying the release tags, workspace files, and commit.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// Root Cargo.toml in --repo; relative paths resolve from its worktree root.
        #[arg(long, default_value = "Cargo.toml")]
        manifest_path: PathBuf,
        /// Compare against the tags already in this clone instead of fetching
        /// from `origin` first. Offline, and only as current as the clone.
        #[arg(long)]
        no_fetch: bool,
        /// Write the manifest but create no commit.
        #[arg(long)]
        no_commit: bool,
        /// Print today's tag and write nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Probe today's UTC `YY.M.D` against published tags.
    ///
    /// Prints the bare candidate on stdout when it is newer than every
    /// release. A covered date prints only a reason on stderr and exits 0.
    /// `ops cut-release` writes a daily cut; `ops release version --tag`
    /// writes an explicitly named version.
    ReleaseDefaultTag {
        /// Git checkout whose tags are the record of what has been released.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// Compare against the tags already in this clone instead of fetching
        /// from `origin` first. Offline, and only as current as the clone.
        #[arg(long)]
        no_fetch: bool,
    },
    /// Release versioning and release preflight checks.
    #[command(subcommand)]
    Release(ReleaseCmd),
    /// Regenerate `THIRD-PARTY-NOTICES.txt` from `Cargo.lock` — the licence
    /// texts the downloadable binary must carry, deduplicated so each distinct
    /// text appears once with the crates that use it. Every permissive licence
    /// in the tree requires its notice to travel with the distributed work, so
    /// a release must not ship without this file being current. Reads crate
    /// sources from
    /// `$CARGO_HOME/registry/src`; run `cargo fetch` first on a cold machine.
    Notices {
        /// Where to write the notices. Defaults to the repository root file
        /// that `cli/src/main.rs` embeds into the binary.
        #[arg(long, default_value = "THIRD-PARTY-NOTICES.txt")]
        out: PathBuf,
        /// Regenerate in memory and fail if the file on disk differs, instead
        /// of rewriting it. The drift gate for CI.
        #[arg(long)]
        check: bool,
    },
    /// Firm-scoped diagnostics.
    Firms {
        #[command(subcommand)]
        action: FirmsAction,
    },
    /// Inspect the fixed Solana devnet SAS program account.
    #[command(subcommand)]
    Sas(SasCmd),
}

#[derive(Subcommand)]
enum SasCmd {
    /// Read the SAS program at finalized commitment.
    Program,
}

#[derive(Subcommand)]
enum ApplicationAction {
    /// Clone, build, and upload a Project application, entry document last.
    /// Reuses `dev sample-project`'s clone/build/validate path in a temporary
    /// directory, then writes every object of the publish plan to `--bucket`
    /// through the operator's own ADC: hashed assets first, `index.html`
    /// last, nothing ever deleted. The Project code comes from the bundle's
    /// own `navigator.yaml`, never the repository name, and a bundle naming
    /// a different Project is refused before any object is written.
    Publish {
        /// Applications bucket to publish into, such as
        /// `neon-law-stg-applications`. Required, with no environment
        /// fallback: naming a bucket is naming a deployment, and a sourced
        /// `.devx/env` carries the local `fs` path under the same variable
        /// name, so it is spelled out on every invocation.
        #[arg(long)]
        bucket: String,
        /// Publish only this Project. Defaults to every sample matter. Any
        /// valid Project code is accepted; its repository is `--repo`, else
        /// the compiled-in repository of a sample matter, else the URL
        /// recorded on the Project row (which needs this worktree's
        /// `.devx/env` sourced).
        #[arg(long)]
        project: Option<String>,
        /// Repository to clone. Defaults to the URL recorded on the Project.
        /// Requires `--project`, since one URL cannot serve every matter.
        #[arg(long, requires = "project")]
        repo: Option<String>,
        /// Branch or tag to build. Defaults to the repository's default
        /// branch.
        #[arg(long = "ref")]
        git_ref: Option<String>,
        /// Print the resolved bucket and the whole plan — every key in
        /// upload order, the object count, and the last key — and write
        /// nothing. The rehearsal before a publish to a real deployment.
        #[arg(long)]
        dry_run: bool,
        /// Keep the temporary checkout and build tree instead of removing
        /// it, to debug a failed build.
        #[arg(long)]
        keep: bool,
    },
}

#[derive(Subcommand)]
enum FirmsAction {
    /// Report every active Firm's Admin-DRI standing (ENG-499): missing,
    /// multiple, or ineligible designations. Read-only — never appoints,
    /// clears, or otherwise repairs a row; `store::firms::appoint_admin_dri`
    /// is the only writer. Connects to the `SurrealDB` the sourced
    /// `.devx/env` names, the same store `web` reads.
    Doctor,
}

#[derive(Subcommand)]
enum ReleaseCmd {
    /// Decide whether the workspace version is a new release.
    Check {
        /// The workspace manifest whose version is the candidate release.
        #[arg(long, default_value = "Cargo.toml")]
        manifest_path: PathBuf,
        /// Git checkout whose tags are the record of what has been released.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// Compare against tags already in this clone instead of fetching.
        #[arg(long)]
        no_fetch: bool,
        /// Append release fields to `$GITHUB_OUTPUT` for workflow jobs.
        #[arg(long)]
        github_output: bool,
    },
    /// Check that every self-referencing action pin names the workspace version.
    ///
    /// The reusable workflows and composite actions under `.github/` name this
    /// repository's own actions by an absolute tag, so a pin left behind ships
    /// a gate no consumer can run. `ci.yml` runs this on every pull request and
    /// the `cut-release` preflight runs it again before the bump is pushed;
    /// both reach the one rule in `release_pins` rather than re-deriving it.
    Pins {
        /// Repository root whose checked-in GitHub configuration is scanned.
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Write a release version into the workspace manifest.
    Version {
        /// Release version to write, e.g. `26.8.20` or `26.8.21-hotfix.3`.
        #[arg(long)]
        tag: String,
        /// The workspace manifest to rewrite.
        #[arg(long, default_value = "Cargo.toml")]
        manifest_path: PathBuf,
        /// Write the manifest but create no commit.
        #[arg(long)]
        no_commit: bool,
    },
}

#[derive(Subcommand)]
enum SurrealArchiveCmd {
    /// Export the configured Surreal namespace and database to object storage.
    Export,
    /// Download one export, restore it into a disposable namespace, apply the
    /// current schema, and reconcile all table row counts before removing it.
    RestoreDrill {
        /// Object-storage key printed by `surreal-archive export`.
        #[arg(long)]
        key: String,
    },
}

#[derive(Subcommand)]
enum SecretsCmd {
    /// Decrypt one deployment's `deployments/<name>/secrets.enc.yaml` and write
    /// `versions/latest` into that deployment's own Secret Manager, which the
    /// Secret Manager CSI driver projects into the pod.
    ///
    /// The repository is the operator source: plaintext coordinates in
    /// `config.toml`, key material encrypted per value against that
    /// deployment's own Cloud KMS key. Rotating a value means rotating it at
    /// the provider first and re-encrypting here second — re-encrypting alone
    /// revokes nothing, because anyone holding repository history and the KMS
    /// key can still read every prior ciphertext.
    Apply {
        /// Deployment directory under `deployments/`, such as `neon-law-stg`.
        #[arg(long)]
        deployment: String,
        /// The directory CONTAINING `deployments/` — the same directory
        /// `.sops.yaml` sits in, and the same flag `ops ship` takes. Defaults
        /// to `NAVIGATOR_DEPLOYMENTS_DIR`, then to the discovered workspace
        /// root.
        #[arg(long, value_name = "DIR")]
        deployments_dir: Option<PathBuf>,
        /// Print the target project and the object names without decrypting
        /// anything or changing Secret Manager. Needs no KMS permission.
        #[arg(long, conflicts_with = "check")]
        dry_run: bool,
        /// Decrypt in-process (the same trust `apply` uses) and compare every
        /// object's SOPS value against Secret Manager `versions/latest` and
        /// the deployment's Kubernetes Secret, by constant-time equality.
        /// Prints names and status only — `match`, `differs`, `missing in
        /// Secret Manager`, or `missing in K8s Secret` — never a value or a
        /// digest, and exits non-zero on any drift. Changes nothing.
        #[arg(long, conflicts_with = "dry_run")]
        check: bool,
    },
}

#[derive(Subcommand)]
enum EmailSummaryCmd {
    /// Re-run a completed `EmailSummary` Restate workflow for one receipt —
    /// the recovery path for a run that completed with a bounded provider
    /// failure (e.g. `input_digest_mismatch`) before intake can re-POST,
    /// since SendGrid never retries a message that already got a 202.
    ///
    /// Refuses when the receipt's Slack delivery is already `confirmed`, so
    /// this can never risk a second post. Never creates a second receipt,
    /// letter, or archive: those are digest-keyed in SurrealDB already, and
    /// this command only purges the retained invocation and resubmits the
    /// identical `EmailSummaryRequest` under the same workflow key (the
    /// receipt id). Prints the receipt id, the invocation id, and status
    /// only — never a summary, a letter, or any client content. Reads
    /// `NAVIGATOR_SURREAL_*` (the deployment's database), the
    /// `NAVIGATOR_SUMMARY_*` / `RESTATE_BROKER_URL` summary-lane
    /// configuration, and `RESTATE_ADMIN_URL` / `RESTATE_ADMIN_TOKEN` /
    /// `RESTATE_AUTH_TOKEN` — the same environment `navigator dev up`/`ops
    /// ship` already source for the target deployment, so there is no
    /// separate `--deployment` flag to keep in sync with it.
    Redrive {
        /// The receipt to redrive.
        #[arg(long)]
        receipt: uuid::Uuid,
    },
}

#[derive(Subcommand)]
enum GithubCmd {
    /// Reconcile a repository's GitHub policy. Any repository in an
    /// admissible organization on the host `NAVIGATOR_GIT_HOST` names may be
    /// reconciled — the public organization holding Navigator, and this
    /// deployment's own `NAVIGATOR_GITHUB_ORG`; anything else is refused
    /// before a token is read. Idempotent: a re-run reads the live rulesets
    /// and labels and writes only a difference.
    Setup {
        /// Repository to reconcile as `owner/name`. Defaults to
        /// `GITHUB_REPOSITORY`, then to this checkout's `origin` remote.
        repository: Option<String>,
        /// Print the reconciliation plan without writing GitHub.
        #[arg(long)]
        dry_run: bool,
        /// Exact release tag a confirmed Project repository's reconciled
        /// `ci.yml`/`cd.yml` callers pin Navigator's reusable workflows to.
        /// Defaults to this binary's own version when — and only when — that
        /// version is one this repository has actually published. Unused,
        /// and never validated, against a repository this content
        /// reconciliation does not apply to.
        #[arg(long, default_value = published_cli_version())]
        action_version: String,
    },
    /// Refuse a revision range that contains an unsigned commit. Cloud Agent
    /// pull-request heads carry an HSM `gpgsig`; a session that turned
    /// `commit.gpgsign` off does not. Squash-merge still writes a GitHub-signed
    /// commit on `main`, so this is the check that the PR head itself is signed.
    CheckSignatures {
        /// Exclusive start of the range (`base..head`).
        #[arg(long)]
        base: String,
        /// Inclusive end of the range.
        #[arg(long, default_value = "HEAD")]
        head: String,
        /// Git directory to inspect.
        #[arg(long, default_value = ".")]
        git_dir: PathBuf,
    },
}

#[derive(Subcommand)]
enum AssetsAction {
    /// Resize + re-encode every manifest photo into AVIF + WebP + JPEG
    /// width variants under `<out>/img/<slug>/`. Run after editing the
    /// `views::assets::GALLERY` manifest or replacing a source photo;
    /// variant paths are stable, so a bounded cache TTL serves the new
    /// bytes once the old ones expire (no cache-bust token).
    Build {
        /// Directory holding the source photos, named by each
        /// manifest entry's `source` field.
        #[arg(long, default_value = "/tmp/nav-photo-work/assets_src/jpeg")]
        src: PathBuf,
        /// Output root; variants land under `<out>/img/<slug>/`.
        /// Defaults to the crate-bundled `/public` mount so a local
        /// dev loop / `cargo test` serves the variants from `/public`.
        #[arg(long, default_value = "server/public")]
        out: PathBuf,
        /// Build only these manifest slugs (repeatable). Adding one photo
        /// otherwise needs every other photo's source JPEG on disk, since
        /// the build walks the whole manifest. An unknown slug is an
        /// error, not an empty run.
        #[arg(long = "only")]
        only: Vec<String>,
    },
    /// Push the built variant tree to the public assets bucket via the
    /// `cloud` crate's `StorageService` (never the GCP SDK directly).
    /// Each file lands under key `img/<slug>/<slug>-<w>w.<ext>` with a
    /// bounded `Cache-Control` (~1 week, no `immutable`). Run after
    /// `navigator ops assets build`. Auth is ADC; the emulator endpoint is honored
    /// via `NAVIGATOR_STORAGE_ENDPOINT`.
    Upload {
        /// Directory holding the built variant tree.
        #[arg(long, default_value = "server/public/img")]
        dir: PathBuf,
        /// Target bucket. Defaults to `NAVIGATOR_ASSETS_BUCKET` — the
        /// public `<project>-assets` bucket, deliberately distinct from
        /// the app's documents bucket (`NAVIGATOR_DOCUMENTS_BUCKET`) so an
        /// upload never writes photos into the documents lane.
        #[arg(long, env = "NAVIGATOR_ASSETS_BUCKET")]
        bucket: Option<String>,
    },
    /// Restore the gitignored `server/public/img/` tree from the public
    /// assets bucket — the inverse of `upload`, for local development.
    /// A fresh clone has empty photo slots (`server/public/img/` is in
    /// `.gitignore`); this downloads every variant under the bucket's
    /// `img/` prefix so the `/public` mount serves the photos again,
    /// without the original source JPEGs. Read-only against the bucket;
    /// auth is ADC, the emulator endpoint is honored via
    /// `NAVIGATOR_STORAGE_ENDPOINT`.
    Pull {
        /// Output root; variants land under `<out>/<slug>/<file>` (the
        /// bucket's `img/` prefix is stripped). Defaults to the `/public`
        /// mount so a local dev loop serves them immediately.
        #[arg(long, default_value = "server/public/img")]
        out: PathBuf,
        /// Source bucket. Defaults to `NAVIGATOR_ASSETS_BUCKET` — the
        /// public `<project>-assets` bucket.
        #[arg(long, env = "NAVIGATOR_ASSETS_BUCKET")]
        bucket: Option<String>,
    },
    /// Report objects in the assets bucket that nothing on the site
    /// reaches — the inverse of `verify`, which only catches the opposite
    /// failure. `upload` never deletes, so an image or clip dropped from a
    /// page stays publicly fetchable at its URL indefinitely; this names
    /// those. Reachability is the union of markdown `](img/…)` references
    /// and the `views::assets::GALLERY` variants (referenced from Rust
    /// views, never from markdown), and only the `img/` prefix is
    /// considered, so `fonts/` is never reported. Report-only by design:
    /// it never deletes, because a wrong reachable set in a pruning tool
    /// would remove live production photographs.
    Orphans {
        /// Content root scanned for markdown image references.
        #[arg(long, default_value = "server/content")]
        content: PathBuf,
        /// Bucket to inspect. Defaults to `NAVIGATOR_ASSETS_BUCKET`.
        #[arg(long, env = "NAVIGATOR_ASSETS_BUCKET")]
        bucket: Option<String>,
        /// Also post the report to the ops Slack channel via the
        /// `SLACK_WEBHOOK_URL` incoming webhook — the same seam the
        /// durable workflows' heartbeat uses. The report names bucket
        /// object keys only, which are already public URLs.
        #[arg(long)]
        slack: bool,
    },
    /// Check that every `![](img/…)` image referenced by the content
    /// tree is actually published at the public origin. `server/public/img/`
    /// is gitignored and no CI step uploads it, so a post can merge and
    /// deploy with a hero that 404s; this gate catches that by fetching
    /// each referenced URL (auth-free `HEAD`, exactly as a browser would)
    /// and failing if any is missing. Run it after `assets upload`,
    /// before shipping.
    Verify {
        /// Content root scanned for markdown image references.
        #[arg(long, default_value = "server/content")]
        content: PathBuf,
        /// Public origin the images are served from. Defaults to
        /// `NAVIGATOR_ASSET_BASE_URL` (the bucket's public origin in
        /// production); pass `--base-url http://localhost:PORT/public`
        /// to check a running local dev loop.
        #[arg(long, env = "NAVIGATOR_ASSET_BASE_URL")]
        base_url: Option<String>,
    },
    /// Download every content-referenced `img/…` object from a public HTTP
    /// origin into `server/public/` for a local run that should serve the
    /// real published bytes. Auth-free — no GCP ADC — using the same
    /// origin `verify` probes.
    FetchReferenced {
        /// Content root scanned for markdown image references.
        #[arg(long, default_value = "server/content")]
        content: PathBuf,
        /// Output root; each `img/slug/file` reference lands at
        /// `<out>/img/slug/file` (default `server/public`).
        #[arg(long, default_value = "server/public")]
        out: PathBuf,
        /// Public origin to download from. Defaults to
        /// `NAVIGATOR_ASSET_BASE_URL` (the bucket's public HTTPS origin).
        #[arg(long, env = "NAVIGATOR_ASSET_BASE_URL")]
        base_url: Option<String>,
    },
    /// Materialize tiny placeholder bytes for every content-referenced
    /// `img/…` path and for the licensed GORP faces. This is for ephemeral
    /// CI image builds that already verified the real public origin and only
    /// need the KIND `/public` mount to serve decodable files at the same
    /// paths; real photos and licensed fonts stay in the public assets
    /// bucket and out of git.
    StubReferenced {
        /// Content root scanned for markdown image references.
        #[arg(long, default_value = "server/content")]
        content: PathBuf,
        /// Output root; each `img/slug/file` reference lands at
        /// `<out>/img/slug/file` (default `server/public`).
        #[arg(long, default_value = "server/public")]
        out: PathBuf,
    },
    /// Publish licensed webfonts from an operator-controlled directory to the
    /// public assets bucket. The font bytes stay out of git; see
    /// `docs/assets.md`.
    Fonts {
        #[command(subcommand)]
        action: FontAction,
    },
}

/// Which web font family `assets fonts upload` publishes. Each variant names
/// one entry of [`assets::BUCKET_FONT_FAMILIES`] — its own bucket directory
/// and filename stem — so a brand's typeface is a new variant here plus a new
/// row in that table, never a second command.
///
/// `font_family_arg_names_every_bucket_family` holds the two together: a
/// family the table publishes and verifies but `--family` cannot name is one
/// an operator has no way to upload.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum FontFamilyArg {
    /// The firm's licensed serif, from `TrashType`.
    GorpSerif,
    /// DeleteYourData.com's OFL-1.1 sans, self-hosted on the same
    /// operator-upload lane as GORP's licensed delivery.
    PlusJakartaSans,
    /// Vesta Estate Planning's heading and body face.
    EbGaramond,
    /// Misericordia Injury Law's body face.
    // Both `Source` values are spelled out because clap's derivation drops the
    // hyphen before a trailing digit, and the value has to equal the bucket
    // directory.
    #[value(name = "source-sans-3")]
    SourceSans3,
    /// Misericordia Injury Law's display face.
    #[value(name = "source-serif-4")]
    SourceSerif4,
    /// Abhaya Immigration's face.
    Mukta,
    /// DeleteYourDebt.com's face.
    PublicSans,
    /// The NYC summons practice's face.
    LibreFranklin,
}

impl FontFamilyArg {
    fn resolve(self) -> &'static assets::FontFamily {
        match self {
            Self::GorpSerif => &assets::GORP_SERIF,
            Self::PlusJakartaSans => &assets::PLUS_JAKARTA_SANS,
            Self::EbGaramond => &assets::EB_GARAMOND,
            Self::SourceSans3 => &assets::SOURCE_SANS_3,
            Self::SourceSerif4 => &assets::SOURCE_SERIF_4,
            Self::Mukta => &assets::MUKTA,
            Self::PublicSans => &assets::PUBLIC_SANS,
            Self::LibreFranklin => &assets::LIBRE_FRANKLIN,
        }
    }
}

#[derive(Subcommand)]
enum FontAction {
    /// Upload a licensed web font family's Regular and Bold WOFF2 files to
    /// its bucket prefix in the public assets bucket. Auth is ADC; this is
    /// an operator action and the source directory is never committed.
    Upload {
        /// Directory containing `<Family>-Regular.woff2` and
        /// `<Family>-Bold.woff2` from the operator's delivery.
        #[arg(long)]
        dir: PathBuf,
        /// Target bucket. Defaults to `NAVIGATOR_ASSETS_BUCKET` — the public
        /// `<project>-assets` bucket.
        #[arg(long, env = "NAVIGATOR_ASSETS_BUCKET")]
        bucket: Option<String>,
        /// Which font family to publish. Defaults to GORP Serif, the
        /// original single-family command's behavior.
        #[arg(long, value_enum, default_value_t = FontFamilyArg::GorpSerif)]
        family: FontFamilyArg,
    },
    /// Package the licensed GORP Serif `.otf` desktop family into one ZIP and
    /// upload it to `fonts/gorp-serif/gorp-serif-otf.zip`, where the policy-gated
    /// `/app/team/fonts/gorp-serif.zip` route serves it. Unlike `upload` (public
    /// WOFF2 web faces), the installable family is a restricted download and
    /// goes to the *private* documents bucket; the `.otf` source is never
    /// committed.
    UploadDesktop {
        /// Directory of GORP Serif `.otf` faces from the licensed delivery.
        #[arg(long)]
        dir: PathBuf,
        /// Target bucket. Defaults to `NAVIGATOR_DOCUMENTS_BUCKET` — the
        /// private `<project>-documents` bucket, so a direct object URL can
        /// never bypass the route's authorization.
        #[arg(long, env = "NAVIGATOR_DOCUMENTS_BUCKET")]
        bucket: Option<String>,
    },
}

#[derive(Subcommand)]
enum FormsAction {
    /// Vendor + verify the blank government forms in the assets
    /// bucket. For each registry form: a local working copy at
    /// `templates/notations/<object_path>` (untracked) is uploaded and its
    /// repo `.sha256` pin rewritten; without one, the bucket object
    /// is pulled and verified against the pin. A missing object or a
    /// pin mismatch fails loudly. Auth is ADC; the emulator endpoint
    /// is honored via `NAVIGATOR_STORAGE_ENDPOINT`.
    Sync {
        /// Target bucket. Defaults to `NAVIGATOR_ASSETS_BUCKET`.
        #[arg(long, env = "NAVIGATOR_ASSETS_BUCKET")]
        bucket: Option<String>,
    },
    /// Print a blank's `AcroForm` `/T` field names, one per line,
    /// pulled from the assets bucket and verified against the repo
    /// `.sha256` pin first — the ground truth for authoring a
    /// `.fields.toml` or re-authoring the field layer (`/T` name =
    /// question code). No guessing: these are the names on the exact
    /// bytes the workflows fill.
    Fields {
        /// Form code, e.g. `nv__llc_formation`.
        code: String,
        /// Source bucket. Defaults to `NAVIGATOR_ASSETS_BUCKET`.
        #[arg(long, env = "NAVIGATOR_ASSETS_BUCKET")]
        bucket: Option<String>,
    },
    /// Re-author a blank's field layer so its `AcroForm` `/T` names
    /// *are* questionnaire state paths (#256): the form's
    /// `.fields.toml` — the recorded human mapping judgment — drives
    /// every rename, checkbox-pair → radio merge, and pre-printed
    /// literal; unmapped fields land in the `unmapped__` namespace.
    /// Writes the transformed working copy to `templates/notations/<object_path>`
    /// plus its diffable `.fields` manifest; visual QA, `forms sync`,
    /// and deleting the consumed `.fields.toml` remain human steps.
    ReAuthor {
        /// Form code, e.g. `nv__llc_formation`.
        code: String,
        /// Source bucket. Defaults to `NAVIGATOR_ASSETS_BUCKET`.
        #[arg(long, env = "NAVIGATOR_ASSETS_BUCKET")]
        bucket: Option<String>,
    },
}

#[derive(Subcommand)]
enum GlossaryCmd {
    /// List every term as its slug and title, alphabetical by slug. The
    /// slug is the term's `/glossary#<slug>` anchor.
    List,
    /// Print one term's definition. Accepts the title in any case or its
    /// slug.
    Show {
        /// Term title or slug, e.g. `"Lawyer Review"` or `lawyer-review`.
        term: String,
    },
    /// Check every term's schema box against the shipped
    /// `navigator.surql`, or rewrite them with `--write`. A term naming a
    /// `SurrealDB` table carries that table's columns and types as rendered
    /// art; the boxes are derived data.
    Tables {
        /// Rewrite the boxes in place instead of only reporting drift.
        #[arg(long)]
        write: bool,
    },
    /// Print the glossary as one Markdown page Notion can hold: every
    /// repository-relative link resolved to a public GitHub URL and
    /// sibling-term links unlinked. The push half of the Notion round trip.
    Notion,
}

#[derive(Subcommand)]
enum LspAction {
    /// Push prebuilt `navigator-lsp` binaries to the public assets
    /// bucket at `lsp/<triple>/navigator-lsp`. `--dir` is the cross-build
    /// output root laid out as `<dir>/<triple>/navigator-lsp` (see
    /// `docs/lsp/README.md`); a target whose binary is absent is skipped,
    /// not an error. Auth is ADC; the emulator endpoint is honored via
    /// `NAVIGATOR_STORAGE_ENDPOINT`.
    Publish {
        /// Directory holding the per-target binaries
        /// (`<dir>/<triple>/navigator-lsp`).
        #[arg(long, default_value = "target/lsp-dist")]
        dir: PathBuf,
        /// Target bucket. Defaults to `NAVIGATOR_ASSETS_BUCKET` — the
        /// public `<project>-assets` bucket, distinct from the documents
        /// lane so the product binary never lands among confidential
        /// client documents.
        #[arg(long, env = "NAVIGATOR_ASSETS_BUCKET")]
        bucket: Option<String>,
    },
}

/// Shared `--host` selector for the live-site commands: optional, since a
/// single stored login is used by default.
#[derive(clap::Args)]
struct HostOpt {
    /// Target host. Optional when exactly one host is logged in.
    #[arg(long)]
    host: Option<String>,
}

/// `navigator site authorities create` — the citation apparatus' write path
/// (ENG-712): file a global Authority (no `--project`; it carries no
/// `project_id`) with its archived artifact, through
/// `POST /app/api/authorities`.
#[derive(Subcommand)]
enum AuthoritiesAction {
    /// Create (or find) the global Authority for `--citation`, archiving
    /// `--file` as its artifact. Repeating an already-recorded citation
    /// returns the existing Authority untouched — its original archive and
    /// metadata are never replaced.
    Create {
        #[command(flatten)]
        host: HostOpt,
        /// One of `case_law`, `statute`, `regulation`, `administrative`, `secondary`.
        #[arg(long)]
        class: String,
        /// The citation — this is the Authority's identity: a second
        /// `create` with the same citation returns the first row.
        #[arg(long)]
        citation: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        short_cite: Option<String>,
        #[arg(long)]
        publisher: Option<String>,
        #[arg(long)]
        issued_on: Option<String>,
        #[arg(long)]
        canonical_url: Option<String>,
        #[arg(long)]
        checked_on: Option<String>,
        /// Path to the artifact to archive (the full text — a PDF or slip
        /// opinion, never committed to a Project repository).
        #[arg(long)]
        file: PathBuf,
        /// MIME type. Defaults to `application/octet-stream`.
        #[arg(long)]
        content_type: Option<String>,
    },
    /// Correct a field on an existing Authority, found by `<ID>` or
    /// `--citation`. Fields left out stay unchanged. `--citation` and
    /// `--class` are immutable — they are the Authority's identity;
    /// changing either means a new Authority, not an update of this one.
    Update {
        #[command(flatten)]
        host: HostOpt,
        /// The Authority's id. Give this or `--citation`, not both.
        id: Option<uuid::Uuid>,
        /// Look up the Authority by citation instead of id.
        #[arg(long)]
        citation: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        short_cite: Option<String>,
        #[arg(long)]
        publisher: Option<String>,
        #[arg(long)]
        issued_on: Option<String>,
        #[arg(long)]
        canonical_url: Option<String>,
        #[arg(long)]
        checked_on: Option<String>,
        /// A new artifact version to archive in place of the current one.
        #[arg(long)]
        file: Option<PathBuf>,
        /// MIME type of `--file`. Defaults to `application/octet-stream`.
        #[arg(long)]
        content_type: Option<String>,
    },
}

/// A matter document is only ever reached through a Project on a site, so
/// every verb here is either a write against a named `--project` on a
/// `--host` brand deployment (`upload`), or a read that resolves both from
/// the checkout's own `navigator.yaml` — a pointer path below `documents/`
/// (the committed `.yml`, or the staged binary it names) is enough for a
/// lawyer to name a file rather than an id or a host.
#[derive(Subcommand)]
enum DocumentAction {
    /// File a local document into a matter (`POST /app/api/projects/{id}/documents`).
    #[command(after_long_help = DOCUMENT_UPLOAD_KIND_HELP)]
    Upload {
        #[command(flatten)]
        host: HostOpt,
        /// Matter code (human-facing) to file into. Resolved against the
        /// matters this login can see.
        #[arg(long)]
        project: String,
        /// Path to the file to upload.
        #[arg(long)]
        file: PathBuf,
        /// Required asset-lane kind. An invalid value prints the accepted enum.
        #[arg(long, value_parser = parse_asset_kind)]
        kind: String,
        /// `client` makes the document client-visible; default is `internal`.
        #[arg(long, value_parser = parse_document_visibility, default_value = "internal")]
        visibility: String,
        /// Optional description stored with the document.
        #[arg(long)]
        description: Option<String>,
        /// MIME type. Defaults to `application/octet-stream`.
        #[arg(long)]
        content_type: Option<String>,
        /// Stable document identity. Must retain the local filename extension
        /// (for example, `--slug motion.pdf` for `motion.pdf`); defaults to the local filename.
        #[arg(long)]
        slug: Option<String>,
    },
    /// The revision chain, newest first, marking the operative row.
    Log {
        /// Path below `documents/`, such as `documents/pleadings/motion.pdf.yaml`.
        /// The retired `.yml` spelling is still read.
        pointer: PathBuf,
    },
    /// Fetch one revision to a local path, verified by `sha256` and size
    /// before success is reported. Refuses a destination inside `documents/`.
    Get {
        pointer: PathBuf,
        /// Revision number to fetch. Defaults to the operative revision under
        /// your lens.
        #[arg(long)]
        version: Option<usize>,
        /// Where to write the fetched bytes. Must be outside `documents/`.
        #[arg(long)]
        out: PathBuf,
    },
    /// A text redline between two revision numbers. PDF and plain text only;
    /// any other type is reported unsupported rather than diffed as bytes.
    Diff {
        pointer: PathBuf,
        a: usize,
        b: usize,
    },
}

#[derive(Subcommand)]
enum MailAction {
    /// File one inbound message's attachments into a matter, without the
    /// bytes ever touching this checkout.
    #[command(after_long_help = DOCUMENT_UPLOAD_KIND_HELP)]
    File {
        #[command(flatten)]
        host: HostOpt,
        /// Matter code (human-facing) to file into.
        #[arg(long)]
        project: String,
        /// `email_conversation_message` row id naming the inbound hop.
        #[arg(long)]
        message: uuid::Uuid,
        /// Required asset-lane kind, applied to every attachment.
        #[arg(long, value_parser = parse_asset_kind)]
        kind: String,
        /// `client` makes every filed attachment client-visible; default `internal`.
        #[arg(long, value_parser = parse_document_visibility, default_value = "internal")]
        visibility: String,
        /// List what would be filed, with size and content type; write nothing.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum NotationAction {
    /// List the private notation inventory for one matter. The output includes
    /// the template code, workflow state, and respondent so an author can
    /// check for an existing instrument before opening another one.
    List {
        #[command(flatten)]
        host: HostOpt,
        /// Matter code whose notation inventory to inspect.
        #[arg(long)]
        project: String,
        /// Emit the raw JSON inventory.
        #[arg(long)]
        json: bool,
    },
    /// Create a questionnaire-driven notation on an existing matter and
    /// leave its questionnaire ready for the site intake flow.
    ///
    /// Every notation hangs on an already-existing Project, so `--project`
    /// (the matter code) is required — open the matter first with
    /// `navigator project create`. The template is read from that
    /// Project's git repo when authored there, else from the bundled firm
    /// catalog; the notation opens pinned to it
    /// (`POST /app/projects/<id>/notations/new`).
    Create {
        /// Template code, e.g. `onboarding__letter`,
        /// `offboarding__letter`, or `nv__llc_formation`.
        template_code: String,
        #[command(flatten)]
        host: HostOpt,
        /// Client email — the notation's bound client (signer). Must be an
        /// existing client on the matter.
        #[arg(long)]
        client_email: String,
        /// Matter **code** (human-facing, from `project create`) to open the
        /// notation inside. Resolved to the Project id against the matters
        /// you can see. Required — the matter must already exist.
        #[arg(long)]
        project: String,
    },
    /// Print a notation's workflow state + signature request id
    /// (`GET …/review?format=json`).
    Status {
        /// Notation UUID.
        notation_id: uuid::Uuid,
        #[command(flatten)]
        host: HostOpt,
        /// Emit the raw JSON status body.
        #[arg(long)]
        json: bool,
    },
    /// Read the filed answers and their source provenance for one notation.
    Answers {
        /// Notation UUID.
        notation_id: uuid::Uuid,
        #[command(flatten)]
        host: HostOpt,
        /// Emit the raw JSON answers.
        #[arg(long)]
        json: bool,
    },
    /// Render + park a notation's document for review (`POST
    /// …/approve-send`) — fills the bound packet (a formation's official
    /// Secretary-of-State form, or a retainer PDF). Idempotent once
    /// rendered.
    Approve {
        /// Notation UUID.
        notation_id: uuid::Uuid,
        #[command(flatten)]
        host: HostOpt,
    },
    /// Send a notation parked at `lawyer_review` back for changes: flag the
    /// wrong answers so they get re-collected, then return to review
    /// (`POST …/request-changes`). A rejected review re-collects the wrong
    /// answers instead of dead-ending; declining the matter is separate.
    RequestChanges {
        /// Notation UUID.
        notation_id: uuid::Uuid,
        #[command(flatten)]
        host: HostOpt,
        /// A question code to flag for re-collection, e.g. `person__client`.
        /// Repeatable — one per answer to re-collect.
        #[arg(long = "question")]
        questions: Vec<String>,
        /// An optional note to the re-collector — what to fix.
        #[arg(long)]
        note: Option<String>,
    },
    /// Re-collect the flagged answers on a notation parked at
    /// `reask__client` (lawyer on the client's behalf) and resubmit for
    /// review (`POST …/reask`). Only the flagged answers are re-collected;
    /// every other answer stays as it was — answers and questions are
    /// decoupled, so a correction never re-walks the questionnaire.
    Update {
        /// Notation UUID.
        notation_id: uuid::Uuid,
        #[command(flatten)]
        host: HostOpt,
        /// A corrected answer as `code=value`, e.g.
        /// `person__client=Libra Jones`. Repeatable; each code must have
        /// been flagged by the review.
        #[arg(long = "answer")]
        answers: Vec<String>,
    },
    /// Download a notation's rendered document (the filled packet) to a
    /// local file (`GET …/documents/document`).
    Document {
        /// Notation UUID.
        notation_id: uuid::Uuid,
        /// Path to write the PDF to.
        #[arg(long)]
        out: PathBuf,
        #[command(flatten)]
        host: HostOpt,
    },
}

#[allow(clippy::too_many_lines)] // one flat dispatch match; splitting it hurts readability
fn main() -> ExitCode {
    // `.env` is picked up before `clap` reads its `env = "..."`
    // defaults. No-op when no file is present, so CI/cluster deploys
    // that inject env vars another way continue to work. The
    // `.devx/env` overlay carries values `devx up` derives at port-
    // forward time; `from_path` skips keys already set, so `.env`
    // wins. Deployment coordinates never ride this path: `ops ship`,
    // `ops observability`, and `ops secrets apply` read the repository's
    // `deployments/<name>/` tree through an explicit `--deployment` flag.
    let _ = dotenvy::dotenv();
    let _ = dotenvy::from_path(".devx/env");
    let runtime = || tokio::runtime::Runtime::new().expect("tokio runtime");
    let cli = Cli::from_arg_matches(&concise_help(Cli::command()).get_matches())
        .expect("Clap matches come from Cli's command tree");
    // `--license` is `exclusive`, so reaching here means it was the only
    // argument. Print the embedded terms and stop before any dispatch.
    // `write_all` rather than `print!`: these two are the only commands whose
    // output a reader routinely pipes into a pager and quits early, and `print!`
    // panics on the resulting broken pipe. An unread tail is not an error.
    if cli.license {
        let mut out = std::io::stdout();
        let _ = out.write_all(NOTICE.as_bytes());
        let _ = out.write_all(b"\n");
        let _ = out.write_all(LICENSE.as_bytes());
        return ExitCode::SUCCESS;
    }
    if cli.third_party_notices {
        let _ = std::io::stdout().write_all(THIRD_PARTY_NOTICES.as_bytes());
        return ExitCode::SUCCESS;
    }
    let Some(cli_command) = cli.command else {
        // A bare `navigator` is a usage error, so the help goes to stderr and
        // the exit code matches what clap returns for a missing subcommand.
        eprint!("{}", concise_help(Cli::command()).render_help());
        return ExitCode::from(2);
    };
    match cli_command {
        Command::Validate {
            dir,
            fix,
            errors_only,
            ci,
        } => run_validate(&dir, fix, errors_only, ci),
        Command::Projects { action } => runtime().block_on(run_projects(action)),
        // The docs reference helpers need no cluster, so they are handled
        // here rather than routed into the KIND dispatcher with the rest
        // of `dev`.
        Command::Glossary { action } => match action {
            GlossaryCmd::List => glossary::list(),
            GlossaryCmd::Show { term } => glossary::show(&term),
            GlossaryCmd::Tables { write } => glossary::tables(write),
            GlossaryCmd::Notion => glossary::notion(),
        },
        Command::Forms { action } => match action {
            FormsAction::Sync { bucket } => forms_sync::run_sync(bucket.as_deref()),
            FormsAction::Fields { code, bucket } => {
                forms_sync::run_fields(&code, bucket.as_deref())
            }
            FormsAction::ReAuthor { code, bucket } => {
                forms_sync::run_reauthor(&code, bucket.as_deref())
            }
        },
        Command::Site { action } => match action {
            SiteCmd::Sync { dry_run } => {
                runtime().block_on(document_sync::run(std::path::Path::new("."), dry_run))
            }
            SiteCmd::Pull { dry_run } => {
                runtime().block_on(document_sync::run_pull(std::path::Path::new("."), dry_run))
            }
            SiteCmd::Import {
                model_name,
                seed_file,
                overwrite,
                dry_run,
                ci,
                host,
            } => {
                let credential = if ci {
                    remote::SeedCredential::Ci {
                        host: host.host.expect("clap requires --host with --ci"),
                    }
                } else {
                    remote::SeedCredential::Stored { host: host.host }
                };
                match (model_name, seed_file) {
                    (Some(model_name), Some(seed_file)) => runtime().block_on(remote::seed(
                        credential,
                        &model_name,
                        &seed_file,
                        overwrite,
                        dry_run,
                    )),
                    (None, None) => runtime().block_on(remote::seed_directory(
                        credential,
                        Path::new("seeds"),
                        overwrite,
                        dry_run,
                    )),
                    (Some(_), None) | (None, Some(_)) => {
                        unreachable!("clap requires MODEL_NAME and SEED_FILE together")
                    }
                }
            }
            SiteCmd::Login { host, no_browser } => {
                runtime().block_on(login::run_login(&host, no_browser))
            }
            SiteCmd::Seed { dir } => runtime().block_on(run_catalog_seed(&dir)),
            SiteCmd::SyntheticPortfolio { target, apply } => {
                runtime().block_on(run_synthetic_portfolio(target, apply))
            }
            SiteCmd::Logout { host } => login::run_logout(host.as_deref()),
            SiteCmd::Whoami { host } => login::run_whoami(host.as_deref()),
            SiteCmd::Mcp { host } => runtime().block_on(mcp_bridge::run(host.as_deref())),
            SiteCmd::Document { action } => runtime().block_on(run_document(action)),
            SiteCmd::Mail { action } => match action {
                MailAction::File {
                    host,
                    project,
                    message,
                    kind,
                    visibility,
                    dry_run,
                } => runtime().block_on(remote::mail_file(
                    std::path::Path::new("."),
                    host.host.as_deref(),
                    &project,
                    message,
                    &kind,
                    &visibility,
                    dry_run,
                )),
            },
            SiteCmd::Notation { action } => runtime().block_on(run_notation(action)),
            SiteCmd::Authorities { action } => match action {
                AuthoritiesAction::Create {
                    host,
                    class,
                    citation,
                    title,
                    short_cite,
                    publisher,
                    issued_on,
                    canonical_url,
                    checked_on,
                    file,
                    content_type,
                } => runtime().block_on(authorities::create(
                    host.host.as_deref(),
                    &class,
                    &citation,
                    &title,
                    short_cite.as_deref(),
                    publisher.as_deref(),
                    issued_on.as_deref(),
                    canonical_url.as_deref(),
                    checked_on.as_deref(),
                    &file,
                    content_type.as_deref(),
                )),
                AuthoritiesAction::Update {
                    host,
                    id,
                    citation,
                    title,
                    short_cite,
                    publisher,
                    issued_on,
                    canonical_url,
                    checked_on,
                    file,
                    content_type,
                } => runtime().block_on(authorities::update(
                    host.host.as_deref(),
                    id,
                    citation.as_deref(),
                    title.as_deref(),
                    short_cite.as_deref(),
                    publisher.as_deref(),
                    issued_on.as_deref(),
                    canonical_url.as_deref(),
                    checked_on.as_deref(),
                    file.as_deref(),
                    content_type.as_deref(),
                )),
            },
        },
        Command::Lsp { dir } => lsp_download::run_download(cli_version(), dir),
        Command::Notation { action } => match action {
            NotationCmd::Preview {
                file,
                offline,
                host,
            } => devx_result(runtime().block_on(notations_preview::run(
                &file,
                offline,
                0,
                host.as_deref(),
            ))),
            NotationCmd::Pdf { file, out, answers } => run_render(&file, &out, &answers, "pdf"),
            NotationCmd::Word { file, out, answers } => run_render(&file, &out, &answers, "docx"),
        },
        // `lsp publish` and the `assets` pipeline
        // carry operator blast radius but are not cluster lifecycle, so they
        // are handled here rather than routed into the KIND/cloud dispatcher
        // below.
        Command::Ops(
            action @ (OpsCmd::Lsp { .. }
            | OpsCmd::Assets { .. }
            | OpsCmd::CutRelease { .. }
            | OpsCmd::ReleaseDefaultTag { .. }
            | OpsCmd::Release { .. }
            | OpsCmd::Notices { .. }
            | OpsCmd::Firms { .. }
            | OpsCmd::Sas(_)),
        ) => match action {
            OpsCmd::Firms { action } => match action {
                FirmsAction::Doctor => firms_doctor::run(),
            },
            OpsCmd::Sas(SasCmd::Program) => match runtime().block_on(sas::program()) {
                Ok(code) => code,
                Err(error) => {
                    eprintln!("navigator: SAS: {error:#}");
                    ExitCode::FAILURE
                }
            },
            OpsCmd::Notices { out, check } => notices::run(&out, check),
            OpsCmd::CutRelease {
                repo,
                manifest_path,
                no_fetch,
                no_commit,
                dry_run,
            } => cut_release::run(
                chrono::Utc::now(),
                &repo,
                !no_fetch,
                &manifest_path,
                no_commit,
                dry_run,
            ),
            OpsCmd::ReleaseDefaultTag { repo, no_fetch } => {
                release_default_tag::run(chrono::Utc::now(), &repo, !no_fetch)
            }
            OpsCmd::Release(ReleaseCmd::Version {
                tag,
                manifest_path,
                no_commit,
            }) => release_version::run(&manifest_path, &tag, no_commit),
            OpsCmd::Release(ReleaseCmd::Check {
                manifest_path,
                repo,
                no_fetch,
                github_output,
            }) => release_check::run(&manifest_path, &repo, !no_fetch, github_output),
            OpsCmd::Release(ReleaseCmd::Pins { root }) => release_pins::run(&root),
            OpsCmd::Lsp { action } => match action {
                LspAction::Publish { dir, bucket } => lsp_publish::run_publish(&dir, bucket),
            },
            OpsCmd::Assets { action } => match action {
                AssetsAction::Build { src, out, only } => assets::run_build(&src, &out, &only),
                AssetsAction::Upload { dir, bucket } => assets::run_upload(&dir, bucket),
                AssetsAction::Pull { out, bucket } => assets::run_pull(&out, bucket),
                AssetsAction::Orphans {
                    content,
                    bucket,
                    slack,
                } => assets::run_orphans(&content, bucket, slack),
                AssetsAction::Verify { content, base_url } => {
                    assets::run_verify(&content, base_url)
                }
                AssetsAction::FetchReferenced {
                    content,
                    out,
                    base_url,
                } => assets::run_fetch_referenced(&content, &out, base_url),
                AssetsAction::StubReferenced { content, out } => {
                    assets::run_stub_referenced(&content, &out)
                }
                AssetsAction::Fonts { action } => match action {
                    FontAction::Upload {
                        dir,
                        bucket,
                        family,
                    } => assets::run_upload_fonts(&dir, bucket, family.resolve()),
                    FontAction::UploadDesktop { dir, bucket } => {
                        assets::run_upload_desktop_fonts(&dir, bucket)
                    }
                },
            },
            _ => unreachable!("guarded by the outer pattern"),
        },
        Command::Ops(OpsCmd::Rebrand(action)) => {
            eprintln!("navigator: `ops rebrand` is deprecated");
            devx_result(devx::dispatch(Command::Ops(OpsCmd::Rebrand(action))))
        }
        // Local reversible loops and prod/cloud operations route through the
        // same handler that owns the cluster and operator behavior.
        c @ (Command::Dev(_) | Command::Ops(_)) => devx_result(devx::dispatch(c)),
    }
}

/// Map an orchestration command's `anyhow::Result<()>` onto a process
/// `ExitCode`. The former `devx` binary printed the error chain and exited
/// non-zero; keep that behavior now that it runs under `navigator`.
fn devx_result(result: anyhow::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("Error: {err:?}");
            ExitCode::FAILURE
        }
    }
}

/// The person store the CLI reads and writes. `persons` moved to
/// `SurrealDB` with ENG-19, so every command that resolves a person opens
/// this handle. The endpoint comes from
/// `NAVIGATOR_SURREAL_*`, the same coordinates `web` uses.
async fn open_surreal() -> Result<store::surreal::SurrealDb, ExitCode> {
    match store::surreal::connect_from_env().await {
        Ok(db) => Ok(db),
        Err(e) => {
            eprintln!("navigator: surreal: {e}");
            Err(ExitCode::from(2))
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn run_projects(action: ProjectsCmd) -> ExitCode {
    match action {
        ProjectsCmd::Sync { dry_run } => {
            document_sync::run_project_sync(std::path::Path::new("."), dry_run).await
        }
        ProjectsCmd::Create {
            name,
            code,
            client_email,
            entity_name,
            jurisdiction,
            attest,
            closed,
            closed_at,
            host,
        } => {
            remote::projects_create(
                host.host.as_deref(),
                &name,
                &code,
                &client_email,
                entity_name.as_deref(),
                jurisdiction.as_deref(),
                attest,
                closed.then_some(closed_at).flatten(),
            )
            .await
        }
        ProjectsCmd::Close {
            project_code,
            reason,
            effective_at,
            dir,
            host,
        } => {
            let closed =
                remote::matter_close(host.host.as_deref(), &project_code, reason, effective_at)
                    .await;
            if closed != ExitCode::SUCCESS {
                return closed;
            }
            remote::archive_repository(host.host.as_deref(), &project_code, &dir).await
        }
        ProjectsCmd::Doctor { host, project } => {
            projects::doctor::run(host.host.as_deref(), project.as_deref())
        }
        ProjectsCmd::Drift {
            host,
            dir,
            all,
            json,
        } => projects::drift::run(host.host.as_deref(), &dir, all, json).await,
        ProjectsCmd::Gate { ci, check, deep } => run_gate(ci, check, deep).await,
        ProjectsCmd::Build { dir } => projects::build::run(&dir),
        ProjectsCmd::Applications { dir, manifest } => projects::applications::run(&dir, manifest),
        ProjectsCmd::Setup {
            project_code,
            all,
            json,
            host,
        } => projects::setup::run(host.host.as_deref(), project_code.as_deref(), all, json).await,
    }
}

async fn run_notation(action: NotationAction) -> ExitCode {
    match action {
        NotationAction::List {
            host,
            project,
            json,
        } => remote::notation_list(host.host.as_deref(), &project, json).await,
        NotationAction::Create {
            template_code,
            host,
            client_email,
            project,
        } => {
            remote::notation_create(
                host.host.as_deref(),
                &template_code,
                &client_email,
                &project,
            )
            .await
        }
        NotationAction::Status {
            notation_id,
            host,
            json,
        } => remote::notation_status(host.host.as_deref(), notation_id, json).await,
        NotationAction::Answers {
            notation_id,
            host,
            json,
        } => remote::notation_answers(host.host.as_deref(), notation_id, json).await,
        NotationAction::Approve { notation_id, host } => {
            remote::notation_approve(host.host.as_deref(), notation_id).await
        }
        NotationAction::RequestChanges {
            notation_id,
            host,
            questions,
            note,
        } => {
            remote::notation_request_changes(
                host.host.as_deref(),
                notation_id,
                &questions,
                note.as_deref(),
            )
            .await
        }
        NotationAction::Update {
            notation_id,
            host,
            answers,
        } => remote::notation_update(host.host.as_deref(), notation_id, &answers).await,
        NotationAction::Document {
            notation_id,
            out,
            host,
        } => remote::notation_document(host.host.as_deref(), notation_id, &out).await,
    }
}

async fn run_document(action: DocumentAction) -> ExitCode {
    match action {
        DocumentAction::Upload {
            host,
            project,
            file,
            kind,
            visibility,
            description,
            content_type,
            slug,
        } => {
            remote::document_upload(
                host.host.as_deref(),
                &project,
                &file,
                &kind,
                Some(&visibility),
                description.as_deref(),
                content_type.as_deref(),
                slug.as_deref(),
            )
            .await
        }
        DocumentAction::Log { pointer } => document_read::log(&pointer).await,
        DocumentAction::Get {
            pointer,
            version,
            out,
        } => document_read::get(&pointer, version, &out).await,
        DocumentAction::Diff { pointer, a, b } => document_read::diff(&pointer, a, b).await,
    }
}

fn is_yaml_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|ext| ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml"))
}

fn include_authored_tree_entry(entry: &walkdir::DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    !entry.file_type().is_dir()
        || !matches!(
            name.as_ref(),
            ".git" | "target" | ".worktrees" | "node_modules" | "dist"
        )
}

/// Parse every `.yaml`/`.yml` file under `dir` as part of `validate`. Prints
/// one line per parse error plus a `Parsed N …` summary, and returns the
/// errors so the run can recapitulate them. Standalone YAML (k8s manifests,
/// config, reference catalogs) is disjoint from the markdown the classified
/// engine lints, so this is a second pass over the same tree rather than a
/// second command.
fn yaml_pass(dir: &std::path::Path) -> std::io::Result<Vec<GateError>> {
    let mut files_scanned = 0usize;
    let mut errors: Vec<GateError> = Vec::new();
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_entry(include_authored_tree_entry)
    {
        let entry = entry.map_err(std::io::Error::other)?;
        if !entry.file_type().is_file() || !is_yaml_path(entry.path()) {
            continue;
        }
        files_scanned += 1;
        let raw = std::fs::read_to_string(entry.path())?;
        for document in serde_yaml::Deserializer::from_str(&raw) {
            if let Err(err) = serde_yaml::Value::deserialize(document) {
                let location = match err.location() {
                    Some(location) => format!(
                        "{}:{}:{}",
                        entry.path().display(),
                        location.line(),
                        location.column()
                    ),
                    None => entry.path().display().to_string(),
                };
                eprintln!("{location}: YAML parse error: {err}");
                errors.push(GateError::new(
                    location,
                    None,
                    format!("YAML parse error: {err}"),
                ));
                break;
            }
        }
    }
    println!(
        "Parsed {files_scanned} YAML file(s), found {} error(s)",
        errors.len()
    );
    Ok(errors)
}

/// `Y001` — a `seeds/*.yaml` document must be accepted by `navigator site import`.
const SEED_DOCUMENT_CODE: &str = "Y001";

/// `Y002` — a `locales/<locale>/[<brand-key>/]<page>.yaml` catalog must deserialize as that page.
const LOCALE_DOCUMENT_CODE: &str = "Y002";

/// `Y003` — a `documents/**/*.yaml` pointer in a Project repository must name a valid asset revision.
const DOCUMENT_POINTER_CODE: &str = "Y003";

fn document_pointer_path(
    root: &std::path::Path,
    path: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let relative = path.strip_prefix(root).ok()?;
    let mut components = relative.components();
    // Both spellings, via the one contract in `document_sync`. Matching only
    // the retired `.yml` here would skip every pointer in a renamed
    // repository, and `Y003` would report a clean pass over nothing — the
    // silent-pass failure the shared constant exists to prevent (LAW-25).
    (components.next()?.as_os_str() == "documents" && crate::document_sync::is_pointer_path(path))
        .then(|| relative.to_path_buf())
}

/// Validate committed document pointers only when the walked root declares a
/// Project. An unrelated tool may have its own `documents/` YAML tree.
fn document_pointer_pass(dir: &std::path::Path) -> std::io::Result<Vec<GateError>> {
    let mut errors = Vec::new();
    let mut files_scanned = 0usize;
    if is_project_repository(dir) {
        for entry in walkdir::WalkDir::new(dir.join("documents"))
            .into_iter()
            .filter_entry(|entry| entry.file_name() != ".git")
        {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error)
                    if error
                        .io_error()
                        .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound) =>
                {
                    break;
                }
                Err(error) => return Err(std::io::Error::other(error)),
            };
            let path = entry.path();
            if !entry.file_type().is_file() || document_pointer_path(dir, path).is_none() {
                continue;
            }
            files_scanned += 1;
            let raw = std::fs::read_to_string(path)?;
            let validation =
                store::document_pointers::DocumentPointer::from_yaml(&raw).and_then(|pointer| {
                    let without_yml = path.with_extension("");
                    let has_document_extension = without_yml
                        .extension()
                        .and_then(std::ffi::OsStr::to_str)
                        .is_some_and(|extension| !extension.is_empty());
                    if has_document_extension {
                        Ok(pointer)
                    } else {
                        anyhow::bail!(
                            "pointer filename must retain the document extension before its `.yaml` pointer suffix"
                        )
                    }
                });
            if let Err(error) = validation {
                print_violation(
                    &path.display().to_string(),
                    1,
                    DOCUMENT_POINTER_CODE,
                    &error.to_string(),
                );
                errors.push(GateError::new(
                    format!("{}:1", path.display()),
                    Some(DOCUMENT_POINTER_CODE),
                    error.to_string(),
                ));
            }
        }
    }
    println!(
        "Validated {files_scanned} document pointer(s), found {} error(s)",
        errors.len()
    );
    Ok(errors)
}

fn seed_model_for_path(path: &std::path::Path) -> Option<anyhow::Result<store::seed::SeedModel>> {
    let parent = path.parent()?;
    if parent.file_name()? != "seeds" {
        return None;
    }
    let model = path.file_stem()?.to_str()?;
    let parsed = store::seed::SeedModel::parse(model);
    let is_canonical_catalog = parent
        .parent()
        .and_then(std::path::Path::file_name)
        .is_some_and(|name| name == "store");
    (!is_canonical_catalog || parsed.is_ok()).then_some(parsed)
}

/// Validate the direct `seeds/*.yaml` documents that an operator can submit
/// through `navigator site import`. The store owns the parser; this pass only
/// discovers the files and reports its refusal with the validation lint code.
fn seed_document_pass(dir: &std::path::Path) -> std::io::Result<Vec<GateError>> {
    let mut files_scanned = 0usize;
    let mut errors: Vec<GateError> = Vec::new();
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_entry(include_authored_tree_entry)
    {
        let entry = entry.map_err(std::io::Error::other)?;
        let path = entry.path();
        if !entry.file_type().is_file() || !is_yaml_path(path) {
            continue;
        }
        let Some(model) = seed_model_for_path(path) else {
            continue;
        };
        files_scanned += 1;
        let raw = std::fs::read_to_string(path)?;
        let validation = model.and_then(|model| store::seed::validate_yaml(model, &raw));
        if let Err(error) = validation {
            print_violation(
                &path.display().to_string(),
                1,
                SEED_DOCUMENT_CODE,
                &error.to_string(),
            );
            errors.push(GateError::new(
                format!("{}:1", path.display()),
                Some(SEED_DOCUMENT_CODE),
                error.to_string(),
            ));
        }
    }
    println!(
        "Validated {files_scanned} seed document(s), found {} error(s)",
        errors.len()
    );
    Ok(errors)
}

/// Validate every brand locale catalog: `locales/<locale>/<page>.yaml`.
///
/// The site publishes English only. An unknown page stem or a locale directory
/// other than [`views::locales::DEFAULT_LOCALE`] is an error, and a known page
/// must deserialize as its typed catalog so a copy-only edit cannot land a
/// document the brand crate cannot load.
fn locale_document_pass(dir: &std::path::Path) -> std::io::Result<Vec<GateError>> {
    let mut files_scanned = 0usize;
    let mut errors: Vec<GateError> = Vec::new();
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_entry(include_authored_tree_entry)
    {
        let entry = entry.map_err(std::io::Error::other)?;
        let path = entry.path();
        if !entry.file_type().is_file() {
            continue;
        }
        let Some(parts) = views::locales::locale_yaml_parts(path) else {
            continue;
        };
        files_scanned += 1;
        if parts.locale != views::locales::DEFAULT_LOCALE {
            let message = format!(
                "locale directory `{}` is not published; only `{}` is allowed",
                parts.locale,
                views::locales::DEFAULT_LOCALE
            );
            print_violation(
                &path.display().to_string(),
                1,
                LOCALE_DOCUMENT_CODE,
                &message,
            );
            errors.push(GateError::new(
                format!("{}:1", path.display()),
                Some(LOCALE_DOCUMENT_CODE),
                message,
            ));
            continue;
        }
        if let Some(brand_key) = parts.brand_key {
            let known = views::brand::BrandKey::ALL
                .iter()
                .any(|key| key.as_str() == brand_key);
            if !known {
                let message = format!(
                    "brand catalog directory `{brand_key}` is not a registry key; expected one of {}",
                    views::brand::BrandKey::ALL
                        .iter()
                        .map(|key| key.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                print_violation(
                    &path.display().to_string(),
                    1,
                    LOCALE_DOCUMENT_CODE,
                    &message,
                );
                errors.push(GateError::new(
                    format!("{}:1", path.display()),
                    Some(LOCALE_DOCUMENT_CODE),
                    message,
                ));
                continue;
            }
        }
        let raw = std::fs::read_to_string(path)?;
        if let Err(error) = views::locales::parse_locale_file(parts.stem, &raw) {
            print_violation(&path.display().to_string(), 1, LOCALE_DOCUMENT_CODE, &error);
            errors.push(GateError::new(
                format!("{}:1", path.display()),
                Some(LOCALE_DOCUMENT_CODE),
                error,
            ));
        }
    }
    println!(
        "Validated {files_scanned} locale catalog(s), found {} error(s)",
        errors.len()
    );
    Ok(errors)
}

/// True for a Containerfile/Dockerfile by filename, whose `FROM` lines this
/// guard scans the same way it scans YAML `image:` values.
fn is_containerfile_path(path: &std::path::Path) -> bool {
    path.file_name()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|name| name.starts_with("Containerfile") || name.starts_with("Dockerfile"))
}

/// Given an image reference (the value after `image:` or `FROM`), return the
/// offending tag when it is a *mutable* tag we must never consume, else `None`.
///
/// Mutable means the `latest` family: an explicit `:latest`, a `:latest-<arch>`
/// variant, or — when `flag_implicit` — an implicit latest (no tag at all). A
/// reference pinned by digest (`@sha256:…`) or to any explicit version tag
/// (`:1.18.2`, `:v1.32.6`, `:16-alpine`, our own `:dev`/`:YY.M.D` build tags)
/// is fine. `flag_implicit` is off for workflow files, where a bare, untagged
/// `- image: navigator-web` names a build-matrix target we *publish*, not a
/// container we *run*; on-cluster manifests and Containerfiles always carry a
/// real reference, so an untagged one there is a genuine implicit-latest bug.
fn mutable_image_tag(reference: &str, flag_implicit: bool) -> Option<String> {
    let reference = reference.trim();
    if reference.is_empty() {
        return None;
    }
    // A digest pin is immutable regardless of any tag that precedes it.
    if reference.contains("@sha256:") {
        return None;
    }
    // The tag is the segment after the last `:` that falls after the last `/`
    // — anything earlier is a registry-host port (`registry:5000/img`), not a
    // tag. No such `:` means the reference is untagged, i.e. implicit `latest`.
    let last_slash = reference.rfind('/');
    let tag = match reference.rfind(':') {
        Some(colon) if last_slash.is_none_or(|slash| colon > slash) => &reference[colon + 1..],
        _ => {
            return flag_implicit.then(|| "latest (implicit — no tag)".to_string());
        }
    };
    if tag == "latest" || tag.starts_with("latest-") {
        Some(tag.to_string())
    } else {
        None
    }
}

/// Strip a trailing ` # …` YAML/Dockerfile comment from a line, returning the
/// code portion and whether the comment marked the line pin-exempt.
fn split_trailing_comment(line: &str) -> (&str, bool) {
    match line.find(" #") {
        Some(idx) => (&line[..idx], line[idx..].contains("pin-exempt")),
        None => (line, false),
    }
}

/// If `trimmed` is a YAML mapping entry whose key is exactly `key`, return its
/// value. Tolerates YAML's optional whitespace before the colon (`image : x`)
/// and rejects a lookalike key that merely starts with it (`imagePullPolicy:`
/// for `image`), so a consumed tag cannot slip through on formatting alone.
fn yaml_value_for_key<'a>(trimmed: &'a str, key: &str) -> Option<&'a str> {
    let rest = trimmed.strip_prefix(key)?.trim_start();
    Some(rest.strip_prefix(':')?.trim())
}

/// Extract the image reference from a YAML `image:` entry, returning
/// `(reference, is_list_item)`. A list item (`- image: …`) is a build-matrix
/// publish target in a workflow; a plain `image:` is a runtime `container:` /
/// `services:` reference we consume — the caller uses the flag to decide
/// whether implicit-latest counts.
fn yaml_image_entry(trimmed: &str) -> Option<(&str, bool)> {
    match trimmed.strip_prefix("- ") {
        Some(rest) => yaml_value_for_key(rest.trim_start(), "image").map(|v| (v, true)),
        None => yaml_value_for_key(trimmed, "image").map(|v| (v, false)),
    }
}

/// Extract the image reference from a Containerfile `FROM` line. The `FROM`
/// keyword is case-insensitive (`From debian:latest` is valid), and a stage may
/// carry `--platform=…` flags and a trailing `AS name` — the reference is the
/// first non-flag token.
fn containerfile_from_ref(trimmed: &str) -> Option<&str> {
    let (keyword, rest) = trimmed.split_once(char::is_whitespace)?;
    keyword.eq_ignore_ascii_case("FROM").then_some(rest)
}

/// Scan one file's contents for a *consumed* mutable tag, returning
/// `(line_number, message)` for each offending line. Covers three consume
/// sites: a YAML `image:` value, a Containerfile `FROM` reference, and a
/// GitHub Actions installer step's `version: latest`. A line carrying a
/// `# pin-exempt: <reason>` comment is skipped, the documented escape hatch
/// for the rare intentional case (e.g. a publish-only construct).
fn detect_mutable_tags(path: &std::path::Path, contents: &str) -> Vec<(usize, String)> {
    let is_yaml = is_yaml_path(path);
    let is_containerfile = is_containerfile_path(path);
    // GitHub Actions workflows are the only place a `version: latest`
    // installer step is meaningful; a `version:` key elsewhere (a Helm chart
    // pin, an API version) is unrelated.
    let is_workflow = is_yaml
        && path.components().any(|c| c.as_os_str() == "workflows")
        && path.to_string_lossy().contains(".github");
    let mut findings = Vec::new();
    for (idx, raw) in contents.lines().enumerate() {
        let (code, exempt) = split_trailing_comment(raw);
        if exempt {
            continue;
        }
        let trimmed = code.trim();
        let reference = if is_yaml {
            yaml_image_entry(trimmed)
        } else if is_containerfile {
            containerfile_from_ref(trimmed).map(|r| (r, false))
        } else {
            None
        };
        if let Some((rest, is_list)) = reference {
            let refstr = rest
                .split_whitespace()
                .find(|tok| !tok.starts_with("--"))
                .unwrap_or("")
                .trim_matches(|c| c == '"' || c == '\'');
            // Implicit latest (an untagged reference) is exempt only for a
            // workflow build-matrix list item (`- image: navigator-web`), a
            // target we publish. A workflow `container:` / `services:` image,
            // and every on-cluster / Containerfile reference, is consumed —
            // an untagged one there is a genuine bug. Explicit `:latest` is
            // always caught regardless.
            let flag_implicit = !(is_workflow && is_list);
            if let Some(tag) = mutable_image_tag(refstr, flag_implicit) {
                findings.push((
                    idx + 1,
                    format!(
                        "consumed mutable image tag `{tag}` in `{refstr}` — pin an explicit \
                         version (docs/gitops.md § \"Pin every consumed image, binary, and action\")"
                    ),
                ));
            }
        } else if is_workflow {
            if let Some(value) = yaml_value_for_key(trimmed, "version") {
                let value = value.trim_matches(|c| c == '"' || c == '\'');
                if value.eq_ignore_ascii_case("latest") {
                    findings.push((
                        idx + 1,
                        "consumed mutable binary version `latest` — pin an explicit version \
                         (docs/gitops.md § \"Pin every consumed image, binary, and action\")"
                            .to_string(),
                    ));
                }
            }
        }
    }
    findings
}

/// Walk `dir` for *consumed* mutable tags — the diligence guard for
/// [navigator#540](https://github.com/neon-law-source-code/navigator/issues/540).
/// Prints one line per offence and returns them. Runs over YAML manifests,
/// Containerfiles, and workflow files alike; the `.git`, `target`, and
/// `.worktrees`, `node_modules`, and `dist` trees are skipped, as in
/// [`yaml_pass`].
fn mutable_tag_pass(dir: &std::path::Path) -> std::io::Result<Vec<GateError>> {
    let mut findings: Vec<GateError> = Vec::new();
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_entry(include_authored_tree_entry)
    {
        let entry = entry.map_err(std::io::Error::other)?;
        let path = entry.path();
        if !entry.file_type().is_file() || !(is_yaml_path(path) || is_containerfile_path(path)) {
            continue;
        }
        let contents = std::fs::read_to_string(path)?;
        for (line, message) in detect_mutable_tags(path, &contents) {
            let location = format!("{}:{line}", path.display());
            eprintln!("{location}: {message}");
            findings.push(GateError::new(location, None, message));
        }
    }
    println!(
        "Checked consumed image/binary tags, found {} mutable tag(s)",
        findings.len()
    );
    Ok(findings)
}

/// The standalone passes `validate` runs over the raw tree after the markdown
/// lint — YAML syntax, seed-document shape, locale catalogs, and consumed
/// mutable tags. Every finding any of them reports fails the gate, so the four
/// lists concatenate into one.
fn standalone_tree_passes(dir: &std::path::Path, ci: bool) -> std::io::Result<Vec<GateError>> {
    let mut errors = yaml_pass(dir)?;
    errors.append(&mut seed_document_pass(dir)?);
    errors.append(&mut locale_document_pass(dir)?);
    errors.append(&mut document_pointer_pass(dir)?);
    errors.extend(project_manifest_pass(dir));
    errors.extend(project_origin_pass(dir, ci));
    errors.append(&mut mutable_tag_pass(dir)?);
    Ok(errors)
}

/// `Y004`–`Y008` and `Y011` — a Project repository's `navigator.yaml` (or the
/// retired `.yml` spelling) holds to the closed key set and value shapes.
fn project_manifest_pass(dir: &std::path::Path) -> Vec<GateError> {
    let findings = crate::projects::manifest::lint(dir);
    let mut errors = Vec::with_capacity(findings.len());
    for finding in findings {
        let location = format!("{}:{}", finding.path.display(), finding.line);
        if finding.warning {
            println!(
                "{}:{}: warning: {}: {}",
                finding.path.display(),
                finding.line,
                finding.code,
                finding.message
            );
        } else {
            print_violation(
                &finding.path.display().to_string(),
                finding.line,
                finding.code,
                &finding.message,
            );
            errors.push(GateError::new(
                location,
                Some(finding.code),
                finding.message,
            ));
        }
    }
    if dir.join(crate::projects::manifest::FILE).is_file()
        || dir.join(crate::projects::manifest::RETIRED_FILE).is_file()
    {
        println!(
            "Validated project manifest, found {} error(s)",
            errors.len()
        );
    }
    errors
}

fn project_origin_pass(dir: &std::path::Path, ci: bool) -> Vec<GateError> {
    let Some(manifest) = crate::projects::origin::load_manifest(dir) else {
        return Vec::new();
    };
    let applications = crate::projects::repository::discovered_applications(dir);
    let findings = crate::projects::origin::lint(dir, &applications, &manifest, ci);
    let mut errors = Vec::with_capacity(findings.len());
    for finding in findings {
        let location = format!("{}:{}", finding.path.display(), finding.line);
        print_violation(
            &finding.path.display().to_string(),
            finding.line,
            finding.code,
            &finding.message,
        );
        errors.push(GateError::new(
            location,
            Some(finding.code),
            finding.message,
        ));
    }
    if !applications.is_empty() {
        println!(
            "Checked built origin references, found {} error(s)",
            errors.len()
        );
    }
    errors
}

/// The repository root the gate runs at: the directory holding both a `README`
/// and a `.git`. The gate takes no path argument, because one that could be
/// pointed at a subdirectory reports a clean tree over the files it never read.
fn gate_root() -> Result<PathBuf, String> {
    let root =
        std::env::current_dir().map_err(|error| format!("read current directory: {error}"))?;
    let readme = std::fs::read_dir(&root)
        .map_err(|error| format!("read {}: {error}", root.display()))?
        .filter_map(Result::ok)
        .any(|entry| entry.file_name().to_string_lossy().starts_with("README"));
    // A worktree's `.git` is a file holding a pointer, not a directory, so this
    // asks whether the entry exists at all rather than what shape it is.
    let git = root.join(".git").exists();
    let missing = match (readme, git) {
        (true, true) => return Ok(root),
        (false, false) => "no README and no .git",
        (false, true) => "no README",
        _ => "no .git",
    };
    Err(format!(
        "{} has {missing} — the gate runs on a whole repository, so run it from the root",
        root.display()
    ))
}

/// `navigator validate [DIR]` — the directory-scoped rule set.
///
/// Takes a path, defaults to `.`, and makes no assumption about the
/// surrounding repository. `--fix` writes every safe-by-construction
/// edit; `--errors-only` hides Warning-severity advisories; `--ci` holds
/// the origin pass to a built tree.
fn run_validate(dir: &std::path::Path, fix: bool, errors_only: bool, ci: bool) -> ExitCode {
    if fix {
        run_validate_fix(dir)
    } else {
        run_validate_scan(dir, errors_only, ci)
    }
}

fn run_validate_fix(dir: &std::path::Path) -> ExitCode {
    let question_codes = rules::canonical_question_codes();
    let fix_report = match fix_directory(
        dir,
        &rules::DefaultFileFilter::default(),
        |file| rules::navigator_classified_rules_with_codes(file, &question_codes),
        true,
    ) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("navigator: {error}");
            return ExitCode::from(2);
        }
    };
    for path in &fix_report.fixed_files {
        println!("{}", palette::dim(format!("fixed {}", path.display())));
    }
    for violation in &fix_report.remaining {
        print_violation(
            &violation.path.display().to_string(),
            violation.line,
            violation.code,
            &violation.message,
        );
    }
    println!(
        "{}",
        palette::dim(format!(
            "Fixed {} file(s); {} remaining violation(s) need a human.",
            fix_report.fixed_files.len(),
            fix_report.remaining.len(),
        ))
    );
    if fix_report.remaining.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn run_validate_scan(dir: &std::path::Path, errors_only: bool, ci: bool) -> ExitCode {
    let question_codes = rules::canonical_question_codes();
    let mut report = match rules::ClassifiedRuleEngine::new()
        .with_question_codes(question_codes)
        .lint_directory(dir)
    {
        Ok(report) => report,
        Err(error) => {
            eprintln!("navigator: {error}");
            return ExitCode::from(2);
        }
    };
    match rules::code_uniqueness_violations(dir, &rules::DefaultFileFilter::default()) {
        Ok(mut found) => report.violations.append(&mut found),
        Err(error) => {
            eprintln!("navigator: {error}");
            return ExitCode::from(2);
        }
    }
    match rules::service_template_violations(dir, &rules::DefaultFileFilter::default()) {
        Ok(mut found) => report.violations.append(&mut found),
        Err(error) => {
            eprintln!("navigator: {error}");
            return ExitCode::from(2);
        }
    }
    for violation in &report.violations {
        if errors_only && rules::severity_for_code(violation.code) != rules::Severity::Error {
            continue;
        }
        print_violation(
            &violation.path.display().to_string(),
            violation.line,
            violation.code,
            &violation.message,
        );
    }
    let (error_count, warning_count) = severity_counts(&report.violations);
    let mut gate_errors: Vec<GateError> = report
        .violations
        .iter()
        .filter(|violation| rules::severity_for_code(violation.code) == rules::Severity::Error)
        .map(|violation| {
            GateError::new(
                format!("{}:{}", violation.path.display(), violation.line),
                Some(violation.code),
                violation.message.clone(),
            )
        })
        .collect();

    println!(
        "{}",
        palette::dim(format!(
            "Scanned {} file(s), found {error_count} error(s), {warning_count} warning(s)",
            report.files_scanned,
        ))
    );

    match standalone_tree_passes(dir, ci) {
        Ok(mut errors) => gate_errors.append(&mut errors),
        Err(error) => {
            eprintln!("navigator: {error}");
            return ExitCode::from(2);
        }
    }

    print_error_recap(&gate_errors);

    let project_layout_failed = is_project_repository(dir)
        && projects::repository::validate_gate(dir, None, false) != ExitCode::SUCCESS;

    if gate_errors.is_empty() && !project_layout_failed {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// `navigator project gate` — the one check over one repository.
///
/// Fixes what is safe to fix, reports what needs a human, and fails on either
/// an Error-severity finding or, under `--ci`, a file it had to fix. With
/// `--ci` it also asks the deployment whether `navigator.yaml` agrees with the
/// live row, which is the one question an offline pass cannot answer.
///
/// `--check` (LAW-62) runs only the live document check, comparing committed
/// pointers against what the deployment holds: it makes no offline pass over
/// the tree at all. That offline pass — content rules, YAML, seeds, layout,
/// and, once it has built, the origin pass reading `dist/` — is `verify`'s
/// job; running it again here from a checkout that `documents` never builds
/// is both redundant and, for the origin pass specifically, impossible.
async fn run_gate(ci: bool, check: bool, deep: bool) -> ExitCode {
    let root = match gate_root() {
        Ok(root) => root,
        Err(message) => {
            eprintln!("navigator: {message}");
            return ExitCode::from(2);
        }
    };
    let dir = root.as_path();

    if check {
        return run_document_check_gate(dir, ci, deep).await;
    }

    let question_codes = rules::canonical_question_codes();

    // The markdown pass and the autofix are one walk: every safe-by-construction
    // edit lands first, and what it reports is what survived them.
    let mut report = match fix_directory(
        dir,
        &rules::DefaultFileFilter::default(),
        |file| rules::navigator_classified_rules_with_codes(file, &question_codes),
        !ci,
    ) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("navigator: {error}");
            return ExitCode::from(2);
        }
    };
    let mut violations = std::mem::take(&mut report.remaining);

    // Cross-file `N111`: notation template `code` must be unique across the
    // tree. Always run — only notation templates carry a `code`, so a
    // prose-only tree simply finds nothing.
    match rules::code_uniqueness_violations(dir, &rules::DefaultFileFilter::default()) {
        Ok(mut found) => violations.append(&mut found),
        Err(error) => {
            eprintln!("navigator: {error}");
            return ExitCode::from(2);
        }
    }
    // Cross-file `N124`: every notation template named by a services catalog
    // must exist under `templates/notations/`.
    match rules::service_template_violations(dir, &rules::DefaultFileFilter::default()) {
        Ok(mut found) => violations.append(&mut found),
        Err(error) => {
            eprintln!("navigator: {error}");
            return ExitCode::from(2);
        }
    }

    let mut gate_errors = report_content_findings(dir, ci, &report, &violations);

    // Standalone raw-tree passes over the same walk: YAML parse errors, seed
    // document shape, locale catalogs, and consumed mutable image/binary tags
    // (navigator#540).
    match standalone_tree_passes(dir, ci) {
        Ok(mut errors) => gate_errors.append(&mut errors),
        Err(error) => {
            eprintln!("navigator: {error}");
            return ExitCode::from(2);
        }
    }

    // Close by naming every failing line again in one block, so the answer
    // to "which line do I fix" is the last thing on screen rather than
    // something to be found by scrolling.
    print_error_recap(&gate_errors);

    let project = is_project_repository(dir);
    let layout_failed =
        project && projects::repository::validate_gate(dir, None, !ci) != ExitCode::SUCCESS;

    if !gate_errors.is_empty() || layout_failed {
        return ExitCode::from(1);
    }
    if !ci || !project {
        return ExitCode::SUCCESS;
    }
    // The one question this tree cannot answer about itself: whether the
    // manifest still agrees with the row the deployment holds.
    projects::gate::live_status(dir).await
}

/// `--check` alone (LAW-62): the live document check, and nothing an offline
/// pass could have already told the caller. `verify` runs the full offline
/// gate — including this same live-row question, at the end of [`run_gate`]
/// above — so this makes only the one request unique to `documents`:
/// comparing committed pointers against what the deployment holds.
async fn run_document_check_gate(dir: &std::path::Path, ci: bool, deep: bool) -> ExitCode {
    let mut gate_errors = Vec::new();
    match projects::document_check::run(dir, ci, deep).await {
        Ok(outcome) => append_document_check(ci, &outcome, &mut gate_errors),
        Err(error) => {
            eprintln!("navigator: {error:#}");
            return crate::remote::exit_code_for(&error);
        }
    }
    print_error_recap(&gate_errors);
    if !gate_errors.is_empty() {
        return ExitCode::from(1);
    }
    if !ci || !is_project_repository(dir) {
        return ExitCode::SUCCESS;
    }
    projects::gate::live_status(dir).await
}

/// Print the live document check and keep the failures that fail the gate.
///
/// A fix is printed as `fixed` when this run may write. Under `--ci` the same
/// fix is a finding: the checkout is about to be discarded, so the job names
/// the edit and stops.
fn append_document_check(
    ci: bool,
    outcome: &projects::document_check::Outcome,
    gate_errors: &mut Vec<GateError>,
) {
    let notice = if ci {
        "documents: checking live records; --ci writes nothing, and a fix fails this job"
    } else {
        "documents: checking live records; fixes stay in this checkout and the live site is not written"
    };
    println!("{}", palette::dim(notice));
    for failure in &outcome.errors {
        println!(
            "{}",
            diagnostic_line(
                rules::Severity::Error,
                &failure.location,
                None,
                &failure.message,
            )
        );
        gate_errors.push(GateError::new(
            failure.location.clone(),
            None,
            failure.message.clone(),
        ));
    }
    for fix in &outcome.fixes {
        if ci {
            let message = format!(
                "would {}; run `navigator project gate --check` and commit the result",
                fix.action
            );
            println!(
                "{}",
                diagnostic_line(rules::Severity::Error, &fix.path, None, &message)
            );
            gate_errors.push(GateError::new(fix.path.clone(), None, message));
        } else {
            println!(
                "{}",
                palette::dim(format!("fixed {}: {}", fix.path, fix.action))
            );
        }
    }
    let verb = if ci { "unfixed" } else { "fixed" };
    println!(
        "{}",
        palette::dim(format!(
            "documents: found {} error(s), {verb} {} file(s)",
            outcome.errors.len(),
            outcome.fixes.len(),
        ))
    );
}

/// Print everything the content pass found — the files it fixed, then each
/// surviving violation, then the counts — and return the findings that fail the
/// run. Split out of [`run_gate`] because it is the whole of what a reader sees,
/// and reads better on its own than as the middle of a longer function.
fn report_content_findings(
    dir: &std::path::Path,
    ci: bool,
    report: &FixReport,
    violations: &[rules::Violation],
) -> Vec<GateError> {
    for path in &report.fixed_files {
        let relative = path.strip_prefix(dir).unwrap_or(path);
        if ci {
            // A formatting finding is about the file, not one line of it, so it
            // carries no line number — naming one would send a reader to a line
            // that may be nowhere near the edit.
            println!(
                "{}",
                diagnostic_line(
                    rules::severity_for_code("F001"),
                    &relative.display().to_string(),
                    Some("F001"),
                    FIXABLE_IN_CI,
                )
            );
        } else {
            println!("{}", palette::dim(format!("fixed {}", relative.display())));
        }
    }
    for violation in violations {
        print_violation(
            &violation.path.display().to_string(),
            violation.line,
            violation.code,
            &violation.message,
        );
    }

    let (error_count, warning_count) = severity_counts(violations);
    let mut gate_errors: Vec<GateError> = violations
        .iter()
        .filter(|violation| rules::severity_for_code(violation.code) == rules::Severity::Error)
        .map(|violation| {
            GateError::new(
                format!("{}:{}", violation.path.display(), violation.line),
                Some(violation.code),
                violation.message.clone(),
            )
        })
        .collect();
    if ci {
        gate_errors.extend(report.fixed_files.iter().map(|path| {
            GateError::new(
                path.strip_prefix(dir).unwrap_or(path).display().to_string(),
                Some("F001"),
                FIXABLE_IN_CI.to_string(),
            )
        }));
    }

    // Under `--ci` nothing was written, so "fixed" would be a lie: the same
    // count is what a local run would have to fix.
    let fixes = if ci { "unformatted" } else { "fixed" };
    println!(
        "{}",
        palette::dim(format!(
            "Scanned {} file(s), found {error_count} error(s), {warning_count} warning(s), {fixes} {} file(s)",
            report.files_scanned,
            report.fixed_files.len(),
        ))
    );
    gate_errors
}

/// What `--ci` says about a file the gate could have fixed but must not write.
const FIXABLE_IN_CI: &str = "not formatted; run `navigator project gate` and commit the result";

/// Parse a `--answer code=value` argument into its halves. The value
/// may itself contain `=`; only the first `=` splits.
fn parse_answer(raw: &str) -> Result<(String, String), String> {
    let (code, value) = raw
        .split_once('=')
        .ok_or_else(|| format!("expected `code=value`, got `{raw}`"))?;
    if code.is_empty() {
        return Err(format!("empty answer code in `{raw}`"));
    }
    Ok((code.to_string(), value.to_string()))
}

fn parse_asset_kind(value: &str) -> Result<String, String> {
    let accepted: Vec<&str> = rules::kind::Kind::ALL
        .iter()
        .filter(|k| k.valid_for(rules::kind::Lane::Asset))
        .map(|k| k.as_str())
        .collect();
    match rules::kind::Kind::parse(value).filter(|k| k.valid_for(rules::kind::Lane::Asset)) {
        Some(k) => Ok(k.as_str().to_string()),
        None => Err(format!(
            "`{value}` is not a document kind. Accepted values are: {}.",
            accepted.join(", ")
        )),
    }
}

fn parse_document_visibility(value: &str) -> Result<String, String> {
    match value {
        "client" | "internal" => Ok(value.to_string()),
        _ => Err("visibility must be `client` or `internal`".into()),
    }
}

const DOCUMENT_UPLOAD_KIND_HELP: &str = "Accepted --kind values: letter, filing, will, trust, directive, agreement, pleading, onboarding, offboarding, memo, transcript, inbound_contract, certificate_of_naturalization, exhibit, closed_repository, invoice, unclassified.";

const DOCUMENT_SYNC_HELP: &str = "Defaults: staged pointers are internal-visible and preserve that visibility when they already exist. Kind inference maps pleadings to filing, exhibits to exhibit, agreements to agreement, invoices to invoice, memos to memo, transcripts to transcript, and everything else to unclassified — except documents/cases/** and documents/rules/**, which are not Project documents at all: each is routed through `site authorities create` (a sidecar carrying citation/class/title/canonical_url/checked_on is required beside each capture) and its committed pointer carries an authority_id rather than a plain kind inference. documents/cases/** is case_law only; documents/rules/** is every other Authority class (statute, regulation, administrative, secondary) — a sidecar whose class disagrees with its folder is refused. A documents/invoices/** filename must match `INV-<digits>.<ext>`. Storage remains content-addressed under the existing Project documents keys; sync does not rename or migrate those keys. A folder outside those categories is therefore intentionally unclassified, not an error.";

/// Render one notation template to PDF or editable Word (`pdf`/`word`,
/// `output_extension` `"pdf"`/`"docx"` respectively). Validates the file
/// against the notation rule set, resolves the render frame (`output:`
/// frontmatter → the `kind:`-derived default → plain), fills any
/// `{{code}}` placeholders from `answers`, and writes the compiled document
/// to `out`, which must carry the matching extension.
/// The render profile a template selects by declaring no `output:` and no
/// `kind:` with a frame of its own. Never a declarable `output:` value —
/// omitting the key is how a template selects it.
const PLAIN_PROFILE: &str = "plain";

/// The one profile `pdf::OutputFormat::parse` cannot construct from its
/// name, because its calibration comes from the template's
/// `jurisdiction:`. `run_render` resolves it itself.
const PLEADING_PROFILE: &str = "pleading";

fn run_render(
    file: &std::path::Path,
    out: &std::path::Path,
    answers: &[(String, String)],
    output_extension: &str,
) -> ExitCode {
    if !has_extension(out, output_extension) {
        eprintln!("navigator: output extension must be `.{output_extension}`");
        return ExitCode::from(2);
    }
    let contents = match std::fs::read_to_string(file) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("navigator: read {}: {e}", file.display());
            return ExitCode::from(2);
        }
    };

    // Gate on validation: render only when there are no blocking
    // (Error-severity) violations. Use the same DB-free classified rule
    // set as `validate`. Yellow advisories (e.g. N112, "step allowed but
    // not built yet") are printed but must not block rendering, mirroring
    // `validate` / `site seed`.
    let source = rules::SourceFile {
        path: file.to_path_buf(),
        contents: contents.clone(),
    };
    let violations: Vec<rules::Violation> =
        rules::navigator_classified_rules_with_codes(&source, &[])
            .iter()
            .flat_map(|r| r.lint(&source))
            .collect();
    let (error_count, _) = severity_counts(&violations);
    if !violations.is_empty() {
        for v in &violations {
            print_violation(&v.path.display().to_string(), v.line, v.code, &v.message);
        }
    }
    if error_count > 0 {
        eprintln!("navigator: {error_count} validation error(s); not rendering");
        return ExitCode::from(1);
    }

    // Resolve the render frame from the document itself: the template's
    // own `output:` override first, else the default `Kind::default_output`
    // derives from its declared `kind:`, else plain.
    //
    // There is no third input. `--format` used to win over both, which
    // meant the frame was chosen twice and the outside choice silently
    // beat a correct header — `--format letter` on a will put the firm's
    // letterhead on an executed instrument with nothing to warn the
    // author (LAW-15). `output:` is now the only override, and it lives
    // in the document it frames.
    //
    // An `output:` value with no Typst counterpart (`form`, the AcroForm
    // mode this preview never renders) falls back to plain, as does a
    // template that declares no `kind:` at all. A *misspelled* `output:`
    // never reaches here: N109 refuses it at the validation gate above.
    let field = |key: &str| {
        rules::frontmatter::extract(&contents)
            .and_then(|fm| rules::frontmatter::field(fm, key))
            .filter(|value| !value.is_empty())
    };
    let profile = field("output")
        .or_else(|| {
            field("kind")
                .and_then(|k| rules::Kind::parse(&k))
                .map(|k| k.default_output().to_string())
        })
        .unwrap_or_else(|| PLAIN_PROFILE.to_string());
    // `pleading` is the one profile a bare name cannot construct: court
    // geometry is calibrated by the template's `jurisdiction:`, a second
    // field `OutputFormat::parse` never sees, so it returns `None` for the
    // name by design. Feeding it through that parser and taking
    // `unwrap_or_default()` put a validation-passing motion on the plain
    // frame — no numbered rail, wrong margins, wrong typeface — and said
    // nothing. Court paper rendered to the wrong geometry is a filing a
    // clerk can reject, so resolve the calibration here and refuse when
    // the table has not been extended to that jurisdiction:
    // `variant_for_jurisdiction` returning `None` means a template that
    // cannot render as a pleading yet, never a reason to guess.
    let format = if profile == PLEADING_PROFILE {
        let jurisdiction = field("jurisdiction").unwrap_or_default();
        if let Some(variant) = pdf::pleading::variant_for_jurisdiction(&jurisdiction) {
            pdf::OutputFormat::Pleading(variant)
        } else {
            // Neither the template's `jurisdiction:` nor its path is echoed
            // back. Both reach this line from caller-supplied input — the
            // one from a parsed document, the other from the command line —
            // and `rust/cleartext-logging` flags a new log of either. There
            // is nothing to lose by leaving them out: `notation pdf`/`notation
            // word` take exactly one file, named on the command line a moment
            // earlier, and what the author cannot already see is which
            // calibrations exist.
            eprintln!(
                "navigator: this template declares `kind: pleading`, but its `jurisdiction:` \
                 has no court-paper calibration; the calibrated jurisdictions are {}. Add one \
                 to `pdf::pleading::variant_for_jurisdiction` before rendering it",
                pdf::pleading::CALIBRATED_JURISDICTIONS.join(", ")
            );
            return ExitCode::from(2);
        }
    } else {
        pdf::OutputFormat::parse(&profile).unwrap_or_default()
    };

    // Body is everything after the frontmatter block; fill placeholders
    // through the same evaluator as preview and final document generation.
    // Bold each answer as it goes in, so a reader can find every fact
    // particular to their matter without reading the boilerplate. The other
    // half of the same idea lives in `pdf::markdown`: a placeholder that no
    // answer filled gets a yellow wash instead, so an unfinished document
    // is unmistakably unfinished.
    // A choice answer is *stored* as its declared key (`nevada`), but the
    // body interpolates it as prose ("the law of Nevada"). Resolve the key
    // back to its label through the template's own `choices:` /
    // `choices.<key>` frontmatter — the same map
    // `portal::retainer_walk::render_context_from_answers` resolves a
    // generated document against, so a preview renders the document the
    // matter will actually get rather than a second, differently-worded
    // one. A state with no declared options, or a value that is not one of
    // them, keeps its answer verbatim (`choice_label` returns `None`), so
    // free text is untouched.
    let choices = rules::frontmatter::extract(&contents)
        .and_then(|fm| workflows::choices_from_yaml(fm).ok())
        .unwrap_or_default();
    let answer_context = answers
        .iter()
        .map(|(code, value)| {
            let display =
                workflows::choice_label(&choices, code, value).unwrap_or_else(|| value.clone());
            (code.clone(), pdf::markdown::bold_answer(&display))
        })
        .collect();
    let body = views::notation::fill(strip_frontmatter(&contents), &answer_context);

    // The firm's own letterhead, hard-coded in one place: `pdf::Letterhead`'s
    // `Default`. A letter that goes out over a lawyer's signature says the
    // same thing every time, so the identity is a constant rather than
    // something assembled per render from brand accessors.
    //
    // This deliberately trades away the white-label seam rather than
    // forgetting it: `views::brand_bundle::BrandManifest` carries
    // `support_email`, `firm_phone`, `firm_address`, and `primary_domain`,
    // so a mounted bundle *could* re-sign this letterhead the way it
    // re-signs the website footer. It does not, by choice — a rendered
    // letter is a binding artifact, and its identity is pinned to source
    // rather than to whatever bundle happens to be mounted at render time.
    // Restore the plumbing here, not somewhere new, if that call changes.
    let letterhead = pdf::Letterhead::default();
    let format_debug = format!("{format:?}");
    let bytes = match render_notation_artifact(file, &body, output_extension, format, &letterhead) {
        Ok(bytes) => bytes,
        Err(_error) => {
            eprintln!("navigator: render failed");
            return ExitCode::from(2);
        }
    };
    if let Err(e) = std::fs::write(out, &bytes) {
        eprintln!("navigator: write {}: {e}", out.display());
        return ExitCode::from(2);
    }
    println!(
        "{}",
        palette::dim(format!(
            "Rendered {} ({format_debug}, {} bytes) → {}",
            file.display(),
            bytes.len(),
            out.display()
        ))
    );
    ExitCode::SUCCESS
}

fn has_extension(out: &Path, expected: &str) -> bool {
    out.extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
}

fn render_notation_artifact(
    file: &Path,
    body: &str,
    output_extension: &str,
    format: pdf::OutputFormat,
    letterhead: &pdf::Letterhead,
) -> Result<Vec<u8>, String> {
    if output_extension == "docx" {
        let word_letterhead =
            matches!(&format, pdf::OutputFormat::Letter(_)).then(|| word::RenderLetterhead {
                name: &letterhead.name,
                phone: &letterhead.phone,
                email: &letterhead.email,
                web: &letterhead.web,
                logo_png: pdf::firm_logo_png(),
            });
        let source_revision = notation_render_revision(file);
        return word::render_notation(
            body,
            word_letterhead.as_ref(),
            &word::RenderOptions {
                source_revision: source_revision.as_deref(),
            },
        )
        .map_err(|error| format!("render Word document: {error}"));
    }

    pdf::render_document_with_options(
        body,
        format,
        letterhead,
        &pdf::RenderOptions {
            creation_timestamp: notation_render_timestamp(file),
        },
    )
    .map_err(|error| format!("render {}: {error}", file.display()))
}

fn notation_render_revision(file: &Path) -> Option<String> {
    let parent = file.parent()?;
    let name = file.file_name()?;
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(parent)
        .args(["log", "-1", "--format=%H", "--"])
        .arg(name)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|revision| !revision.is_empty())
}

/// The fixed timestamp for notation files that have no Git history.
///
/// A source file outside Git still needs a stable artifact, so the renderer uses
/// the Unix epoch rather than the wall clock. Tracked files use their latest
/// commit timestamp, which makes the artifact follow the source's own history.
const NOTATION_RENDER_FALLBACK_EPOCH: i64 = 0;

fn notation_render_timestamp(file: &Path) -> chrono::DateTime<chrono::Utc> {
    let timestamp = file
        .parent()
        .and_then(|parent| {
            let name = file.file_name()?;
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(parent)
                .args(["log", "-1", "--format=%ct", "--"])
                .arg(name)
                .output()
                .ok()?;
            if !output.status.success() {
                return None;
            }
            String::from_utf8(output.stdout)
                .ok()?
                .trim()
                .parse::<i64>()
                .ok()
        })
        .and_then(|seconds| chrono::Utc.timestamp_opt(seconds, 0).single());
    timestamp.unwrap_or_else(|| {
        chrono::Utc
            .timestamp_opt(NOTATION_RENDER_FALLBACK_EPOCH, 0)
            .single()
            .unwrap_or(chrono::DateTime::<chrono::Utc>::UNIX_EPOCH)
    })
}

/// Return the body of a notation file — everything after the leading
/// YAML frontmatter block. When there is no recognized frontmatter, the
/// whole string is the body.
fn strip_frontmatter(contents: &str) -> &str {
    let Some(after_open) = contents.strip_prefix("---\n") else {
        return contents;
    };
    if let Some(end) = after_open.find("\n---\n") {
        return after_open[end + "\n---\n".len()..].trim_start_matches('\n');
    }
    // Closer at EOF with no body, or no closer at all.
    after_open.strip_suffix("\n---").map_or(contents, |_| "")
}

struct FixReport {
    files_scanned: usize,
    fixed_files: Vec<PathBuf>,
    remaining: Vec<rules::Violation>,
}

/// The most passes `fix_directory` will make over one file before
/// giving up. A pass that changes nothing ends the loop, so this bound
/// is only reached by two rules that disagree — and reaching it leaves
/// the file valid, just not finished.
const MAX_FIX_PASSES: usize = 8;

/// Walk `dir` honoring `filter`, apply every safe-by-construction
/// autofix to each markdown file in place, and then re-lint to
/// collect the diagnostic-only violations a human still needs to
/// address. Edits within a file are applied highest-offset-first so
/// earlier offsets stay valid; on overlap the rule with the lower
/// code string wins (deterministic), and the file is re-scanned until
/// it stops changing so a deferred or newly uncovered fix still lands
/// in the same run.
fn fix_directory(
    dir: &std::path::Path,
    filter: &dyn rules::FileFilter,
    rules_for_file: impl Fn(&rules::SourceFile) -> Vec<Box<dyn rules::Rule>>,
    write: bool,
) -> std::io::Result<FixReport> {
    let mut files_scanned = 0usize;
    let mut fixed_files = Vec::new();
    let mut remaining = Vec::new();
    for entry in walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            if e.file_type().is_dir() && e.depth() > 0 {
                filter.include_dir(e.path())
            } else {
                true
            }
        })
    {
        let entry = entry.map_err(std::io::Error::other)?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if !filter.include_file(path) {
            continue;
        }
        files_scanned += 1;
        let contents = std::fs::read_to_string(path)?;
        let mut file = rules::SourceFile {
            path: path.to_path_buf(),
            contents,
        };
        let original = file.contents.clone();
        // Repeat until the file stops changing: one fix routinely
        // uncovers another. Dropping an overlapping edit defers it to
        // the next pass, and a fix can create a fresh violation outright
        // — trimming a hard break off a short line hands that line to
        // `S102`. The bound only guards against a pair of rules that
        // undo each other; every pass either changes the file or is the
        // last one.
        for _ in 0..MAX_FIX_PASSES {
            let rule_set = rules_for_file(&file);
            let mut edits: Vec<(rules::TextEdit, &'static str)> = Vec::new();
            for rule in &rule_set {
                for v in rule.lint(&file) {
                    if let Some(edit) = rule.fix(&file, &v) {
                        edits.push((edit, rule.code()));
                    }
                }
            }
            if edits.is_empty() {
                break;
            }
            // Sort ascending by start; resolve overlap by keeping the
            // lower-coded edit. Then apply descending.
            edits.sort_by(|a, b| a.0.range.start.cmp(&b.0.range.start).then(a.1.cmp(b.1)));
            let mut kept: Vec<(rules::TextEdit, &'static str)> = Vec::with_capacity(edits.len());
            for (edit, code) in edits {
                if let Some(prev) = kept.last() {
                    if edit.range.start < prev.0.range.end {
                        continue;
                    }
                }
                kept.push((edit, code));
            }
            kept.sort_by_key(|edit| std::cmp::Reverse(edit.0.range.start));
            let mut new_contents = file.contents.clone();
            for (edit, _) in &kept {
                new_contents.replace_range(edit.range.clone(), &edit.new_text);
            }
            if new_contents == file.contents {
                break;
            }
            file.contents = new_contents;
        }
        if file.contents != original {
            // `write` is off under `--ci`, where the checkout is discarded the
            // moment the job ends. The fix is still computed — that is how the
            // caller knows the file needs one — but writing it would let a
            // formatting problem pass a gate and never reach `main`.
            if write {
                std::fs::write(path, &file.contents)?;
            }
            fixed_files.push(path.to_path_buf());
        }
        for rule in &rules_for_file(&file) {
            remaining.extend(rule.lint(&file));
        }
    }
    Ok(FixReport {
        files_scanned,
        fixed_files,
        remaining,
    })
}

/// Split a violation list into `(error_count, warning_count)` by each
/// code's [`rules::Severity`]. Used for the `validate` summary line so
/// blocking errors and "not built yet" advisories are tallied apart.
fn severity_counts(violations: &[rules::Violation]) -> (usize, usize) {
    let errors = violations
        .iter()
        .filter(|v| rules::severity_for_code(v.code) == rules::Severity::Error)
        .count();
    (errors, violations.len() - errors)
}

/// Render one diagnostic: its severity marker, then `path:line` in dim
/// cyan-700, then the rule code in cyan-500 and the message.
///
/// `location` is pre-rendered rather than taken as `(path, line)` because
/// the YAML syntax pass reports a column too, and `code` is `None` for the
/// two passes that carry no rule code at all — the YAML syntax check and
/// the consumed-mutable-tag guard. Both shapes only arise in the error
/// recapitulation; the per-violation listing always has a code.
fn diagnostic_line(
    severity: rules::Severity,
    location: &str,
    code: Option<&str>,
    message: &str,
) -> String {
    let marker = match severity {
        rules::Severity::Error => "error:",
        rules::Severity::Warning => "warning:",
    };
    match code {
        Some(code) => format!(
            "{marker} {} {}: {message}",
            palette::dim(location),
            palette::highlight(code),
        ),
        None => format!("{marker} {}: {message}", palette::dim(location)),
    }
}

/// Render a single rule violation with its severity: path/line in dim
/// cyan-700, rule code in cyan-500, message in default. Shared by validate
/// and site seed so both subcommands have the same look.
fn print_violation(path: &str, line: usize, code: &str, message: &str) {
    println!(
        "{}",
        diagnostic_line(
            rules::severity_for_code(code),
            &format!("{path}:{line}"),
            Some(code),
            message,
        )
    );
}

/// One finding that fails the gate, retained so a run can
/// recapitulate every error at the end.
///
/// The primary listing prints in tree order — per pass, per file, per
/// line — which is what keeps a file's violations adjacent and the body
/// readable. That order also scatters the errors: among the warnings
/// inside the markdown pass, and across the four standalone passes that
/// print after it. Collecting them here is what lets the run close with
/// a single block naming every line that failed.
struct GateError {
    /// `path:line`, or `path:line:column` where the pass knows one.
    location: String,
    /// The rule code, or `None` for the two passes that have none: the
    /// YAML syntax check and the consumed-mutable-tag guard.
    code: Option<String>,
    message: String,
}

impl GateError {
    fn new(location: impl Into<String>, code: Option<&str>, message: impl Into<String>) -> Self {
        Self {
            location: location.into(),
            code: code.map(str::to_string),
            message: message.into(),
        }
    }

    fn render(&self) -> String {
        diagnostic_line(
            rules::Severity::Error,
            &self.location,
            self.code.as_deref(),
            &self.message,
        )
    }
}

/// Close a `validate` run by naming every error again, on its own, after
/// the passes have finished printing.
///
/// `validate` runs six passes and the four standalone ones print *after*
/// the markdown summary, so no ordering within a single pass can gather a
/// YAML error and a mutable-tag error together — only a block after every
/// pass can name every failing line. It is additive, so the primary
/// listing keeps the tree order that makes it readable in the first
/// place, and it puts the failing lines where a terminal leaves the
/// reader: at the tail.
fn print_error_recap(errors: &[GateError]) {
    if errors.is_empty() {
        return;
    }
    println!();
    println!(
        "{}",
        palette::header(format!("{} error(s) fail this run:", errors.len()))
    );
    for error in errors {
        println!("{}", error.render());
    }
}

async fn run_catalog_seed(dir: &std::path::Path) -> ExitCode {
    let surreal = match open_surreal().await {
        Ok(s) => s,
        Err(code) => return code,
    };
    let storage = match cloud::from_env().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("navigator: storage: {e}");
            return ExitCode::from(2);
        }
    };
    let report = match import::import_directory(&surreal, &storage, dir).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("navigator: site seed: {e}");
            return ExitCode::from(2);
        }
    };
    for v in &report.violations {
        print_violation(&v.path.display().to_string(), v.line, v.code, &v.message);
    }
    println!(
        "{}",
        palette::dim(format!(
            "Seeded {} workspace-shared template(s), {} question catalog row(s); skipped {} file(s) with error-level rule violations.",
            report.templates_created,
            report.questions_created,
            report.files_skipped_due_to_violations,
        ))
    );
    if report.files_skipped_due_to_violations > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

async fn run_synthetic_portfolio(target: Option<String>, apply: bool) -> ExitCode {
    let environment = match store::DeploymentEnvironment::from_env() {
        Ok(environment) => environment,
        Err(e) => {
            eprintln!("navigator: {e}");
            return ExitCode::from(2);
        }
    };
    let surreal = match open_surreal().await {
        Ok(s) => s,
        Err(code) => return code,
    };
    let storage = match cloud::from_env().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("navigator: storage: {e}");
            return ExitCode::from(2);
        }
    };
    let result = if apply {
        store::synthetic_portfolio::apply(&surreal, &storage, environment, target.as_deref()).await
    } else {
        store::synthetic_portfolio::dry_run(&surreal, &storage, environment, target.as_deref())
            .await
    };
    let plan = match result {
        Ok(plan) => plan,
        Err(e) => {
            eprintln!("navigator: synthetic portfolio: {e}");
            return ExitCode::from(2);
        }
    };
    for item in &plan.items {
        println!(
            "{}",
            palette::dim(format!("{:?} {} {}", item.action, item.kind, item.key))
        );
    }
    println!(
        "{}",
        palette::dim(format!(
            "synthetic portfolio v{} against `{}`: {} to create, {} unchanged{}.",
            plan.version,
            plan.target,
            plan.created(),
            plan.unchanged(),
            if apply {
                ""
            } else {
                " (dry run; nothing written)"
            },
        ))
    );
    ExitCode::SUCCESS
}
