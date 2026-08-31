use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use serde::Deserialize;

mod assets;
mod credentials;
mod devx;
mod docs;
mod erd;
mod format;
mod forms_sync;
mod github;
#[allow(dead_code)]
mod import;
#[allow(dead_code)]
mod intake;
mod list;
mod login;
mod lsp_publish;
mod mcp_bridge;
mod narrate;
mod notices;
mod palette;
mod project;
mod projects;
mod release;
mod release_check;
mod release_default_tag;
mod release_version;
#[allow(dead_code)]
mod remote;
mod scaffold;
mod sendgrid_openapi;
mod surreal_archive;
mod transcribe;

use devx::brand::BrandCmd;
use devx::{DnsCmd, GcpCmd, RestateCmd, StagingAction, WorktreeEnvCmd};

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
fn cli_version() -> &'static str {
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

/// The version [`ProjectRepositoryAction::Scaffold`] pins its generated gate
/// to by default — [`cli_version`] narrowed to the sources that name an
/// *actually published* release, never the bare `CARGO_PKG_VERSION` fallback
/// `build.rs` uses so `--version` still prints something on a plain local
/// build.
///
/// A runtime `NAVIGATOR_RELEASE_TAG` only appears in a deployed container,
/// started from an image published under that tag, so it cannot precede the
/// tag. The build-time-baked `NAVIGATOR_CLI_VERSION` is trustworthy the same
/// way, but only when `NAVIGATOR_CLI_VERSION_IS_RELEASE` confirms `build.rs`
/// actually saw `NAVIGATOR_RELEASE_TAG` rather than falling back to the crate
/// version — which is bumped on `main` before the tag naming it exists. When
/// neither source is available this returns empty, and `scaffold`'s own
/// `is_release_tag` refusal then asks the operator to name `--action-version`
/// themselves rather than have the gate guess.
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
    Validate {
        /// Directory to walk.
        #[arg(default_value = ".")]
        dir: PathBuf,
        /// Apply every safe-by-construction rule autofix
        /// (whitespace, ATX heading spacing, blockquote spacing) to
        /// the files in place, then re-validate. Diagnostic-only
        /// rules (N-family notation-template, M024 duplicate headings,
        /// M026 trailing punctuation) are still reported but not
        /// auto-fixed. The autofixed-source view is what the
        /// `navigator-lsp` `source.fixAll` action ships in editors.
        #[arg(long)]
        fix: bool,
    },
    /// The notation author's offline workbench for everything under
    /// `templates/`.
    Template {
        #[command(subcommand)]
        action: TemplateCmd,
    },

    // ─────────────── Local database ───────────────
    // Load and inspect a local store — the notation registry and the
    // firm's own reference data.
    /// Deprecated: use the site and local authoring commands instead.
    #[command(
        long_about = "Deprecated: use `navigator site seed` or the local authoring commands instead."
    )]
    Db {
        #[command(subcommand)]
        action: DbCmd,
    },

    /// Drive a running deployment with the bearer token `navigator site login` stores.
    Site {
        #[command(subcommand)]
        action: SiteCmd,
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
    /// List the live site's Projects as a table or JSON.
    List {
        #[command(flatten)]
        host: HostOpt,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Open an existing Project workbench on the live site.
    Open {
        /// Project code, resolved only against Projects visible to the login.
        project_code: String,
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
    /// Create or validate the one source repository that belongs to a Project.
    ///
    /// One repository per Project code, holding that Project's notation
    /// templates under `templates/` and its client portal under `portal/`.
    Repository {
        #[command(subcommand)]
        action: ProjectRepositoryAction,
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
    /// Create or adopt the three handles a Project opens with.
    ///
    /// The documents-bucket prefix `projects/<code>`, the Drive ingest folder
    /// named for the code, and one private source repository named for the
    /// code. Matter-open already runs this pass best-effort; this command is
    /// the operator retry when Drive or the forge was down, or when a legacy
    /// row never received one.
    Surfaces {
        #[command(subcommand)]
        action: SurfacesAction,
    },
}

#[derive(Subcommand)]
enum SurfacesAction {
    /// Create or adopt this Project's Drive ingest folder and source
    /// repository, and name its documents-bucket prefix.
    Reconcile {
        /// Project code, e.g. `acme`.
        #[arg(long)]
        project: String,
    },
}

#[derive(Subcommand)]
enum ProjectRepositoryAction {
    /// Create the reviewed, source-only Project repository scaffold.
    Scaffold {
        /// Stable Project code. This becomes the repository name.
        project_code: String,
        /// Directory to create or complete. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        /// Exact release tag the generated gate pins Navigator's validate
        /// action to. Defaults to this binary's own version when — and only
        /// when — that version is one this repository has actually
        /// published; a plain local build cannot vouch for its own crate
        /// version, so it carries no default and this must be named.
        #[arg(long, default_value = published_cli_version())]
        action_version: String,
    },
    /// Validate a Project repository: its layout, its notation templates, and
    /// its portal's build shape.
    Validate {
        /// Repository root. Defaults to the current directory.
        #[arg(default_value = ".")]
        dir: PathBuf,
        /// Repository name, which is the Project code. Defaults to
        /// `GITHUB_REPOSITORY`'s final segment, then the directory name.
        #[arg(long)]
        repository: Option<String>,
    },
    /// Write Navigator's canonical agent-skill catalog into a Project
    /// repository, from this binary's own compiled-in copies.
    SyncSkills {
        /// Repository root. Defaults to the current directory.
        #[arg(default_value = ".")]
        dir: PathBuf,
    },
}

#[derive(Subcommand)]
enum TemplateCmd {
    /// Normalize whitespace and bullet style in a Markdown notation.
    /// Frontmatter passes through untouched; the body has `- `
    /// bullets converted to `* ` and trailing spaces stripped.
    Format {
        /// File to format in place.
        file: PathBuf,
    },
    /// Write a Harvard-outline narration stage from Markdown.
    ///
    /// Depth-1 headings numbered `I.` (contracts) or `1.` (motion practice),
    /// plus `> **A.**` block-quote subsections, become highlightable units.
    /// Open the HTML in a browser and step with Arrow keys, J/K, or Space
    /// while recording. `H` hides the hint for a clean frame.
    Narrate {
        /// Markdown file to parse (a notation template, or a plain draft).
        file: PathBuf,
        /// Where to write the self-contained HTML stage.
        #[arg(long)]
        out: PathBuf,
    },
    /// Render a single notation template to a PDF, framed by an output
    /// format (a plain document, a firm `letter` on Neon Law letterhead
    /// with the logo, or an `agreement` — the same letterhead typeset
    /// curtly for a contract).
    ///
    /// The file is validated against the same notation rule set as
    /// `validate` first — a template with any violation is refused. The
    /// output format is taken from the template's `output:` frontmatter
    /// field, overridable with `--format`; absent both, it renders
    /// plain. Markdown is converted to Typst and compiled in pure Rust
    /// (no shell-out). `{{placeholder}}` tokens render verbatim unless
    /// filled with `--answer code=value`.
    Render {
        /// Path to the notation template (`.md`).
        file: PathBuf,
        /// Where to write the rendered PDF.
        #[arg(long)]
        out: PathBuf,
        /// Output format (`plain`, `letter`, or `agreement`). Overrides
        /// the template's `output:` frontmatter field when set.
        #[arg(long)]
        format: Option<String>,
        /// Fill a `{{code}}` placeholder with `value`. Repeatable:
        /// `--answer counterparty_legal_name="NEON GmbH"`.
        #[arg(long = "answer", value_parser = parse_answer)]
        answers: Vec<(String, String)>,
    },
    /// Drop the three files that a new legal workflow starts with:
    /// `templates/<category>/<jurisdiction>.md`,
    /// `workflows/specs/<code>.yaml`, and
    /// `features/tests/features/<matter>.feature`. Idempotent —
    /// existing files are left alone.
    Scaffold {
        /// Snake-case matter slug, e.g. `incorporation`,
        /// `estate_planning`. Forms the prefix of the template `code`.
        matter: String,
        /// Directory under `templates/` to drop the markdown into.
        #[arg(long)]
        category: String,
        /// Jurisdiction name (`PascalCase` for the filename,
        /// `snake_case` for the template `code`), e.g. `Nevada`.
        #[arg(long)]
        jurisdiction: String,
    },
    /// Transcribe a recording (or replay a transcript) into Inquiry
    /// Coverage JSON for a notation template questionnaire.
    ///
    /// This is the offline/upload path; real-time streaming ("live")
    /// transcription is a separate `web` feature, not a CLI command.
    Transcribe {
        /// Template markdown file whose `questionnaire:` becomes the
        /// Inquiry Set. Required — pass `--template` or set
        /// `NAVIGATOR_NOTATION_TEMPLATE`.
        #[arg(long, env = "NAVIGATOR_NOTATION_TEMPLATE")]
        template: PathBuf,
        /// Plain-text transcript to replay without calling speech-to-text.
        #[arg(long, conflicts_with = "audio")]
        transcript: Option<PathBuf>,
        /// Audio file to transcribe. By default this uses the `fake`
        /// backend (no cloud call); pass `--speech-backend google` to
        /// transcribe with real Google Speech-to-Text. Any common format
        /// works (m4a/AAC, mp3, flac, wav, ogg) — it is decoded locally.
        #[arg(long, conflicts_with = "transcript")]
        audio: Option<PathBuf>,
        /// Speech backend for `--audio`: `fake` (default, deterministic,
        /// no cloud call) or `google` (real Speech-to-Text — needs a
        /// project and credentials). Real cloud is opt-in.
        #[arg(long, env = "NAVIGATOR_SPEECH_BACKEND", default_value = "fake")]
        speech_backend: String,
        /// Google Cloud project for Speech-to-Text.
        #[arg(long, env = "GOOGLE_CLOUD_PROJECT")]
        google_project: Option<String>,
        /// Google Speech-to-Text v2 location.
        #[arg(long, default_value = "global")]
        google_location: String,
        /// BCP-47 language code for the audio.
        #[arg(long, default_value = "en-US")]
        google_language: String,
        /// Google Speech-to-Text recognition model.
        #[arg(long, default_value = "latest_long")]
        google_model: String,
        /// Pretty-print the JSON output.
        #[arg(long)]
        pretty: bool,
    },
    /// Vendored government forms (`templates/forms/`).
    Forms {
        #[command(subcommand)]
        action: FormsAction,
    },
    /// Engineering intake notations (`templates/github/`) — render an
    /// issue or pull-request body from answers, or open the issue.
    Github {
        #[command(subcommand)]
        action: GithubAction,
    },
}

#[derive(Subcommand)]
enum DbCmd {
    /// List rows from the store, after running the full canonical seed
    /// pass. The seed is idempotent so re-running list against an
    /// already-populated database is safe.
    List {
        #[command(subcommand)]
        subject: ListSubject,
    },
    /// Insert a new row in the `projects` table.
    Project {
        #[command(subcommand)]
        action: DbProjectAction,
    },
    /// Print an ERD describing every table in the schema. Default
    /// format is a Mermaid `erDiagram` block; `--format svg` emits a
    /// deterministic, hand-written SVG (suitable for piping into
    /// `docs/erd.svg`). Introspects `INFO FOR DB` / `INFO FOR TABLE`
    /// over the `NAVIGATOR_SURREAL_*` connection, applying the schema
    /// first: a diagram of a database no one has prepared is an empty
    /// diagram, not an error worth guessing at.
    ///
    /// It lives beside the other database commands, not under `docs`:
    /// it prepares and introspects a database, and only its output
    /// happens to be documentation.
    Erd {
        /// Output format. `mermaid` (default) → GitHub-renderable
        /// `erDiagram` block. `svg` → deterministic standalone SVG.
        #[arg(long, value_enum, default_value_t = erd::OutputFormat::Mermaid)]
        format: erd::OutputFormat,
    },
}

#[derive(Subcommand)]
enum SiteCmd {
    /// Import a seed-shaped YAML document through the logged-in deployment.
    Import {
        /// Singular glossary term and Surreal table, such as `person` or
        /// `entity`.
        model_name: String,
        /// YAML document using the standard `lookup_fields` / `records` shape.
        seed_file: PathBuf,
        /// Replace every field represented in each matching seed record.
        #[arg(long)]
        overwrite: bool,
        #[command(flatten)]
        host: HostOpt,
    },
    /// File a document into a matter on a live site.
    Document {
        #[command(subcommand)]
        action: DocumentAction,
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
    /// Serve the AIDA tool catalog to Claude as a local MCP server over
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
    /// Read-side operations against a live site's matters.
    Projects {
        #[command(subcommand)]
        action: ProjectsCmd,
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
    /// Published workspace docs helpers — list every published page, or
    /// print a canonical glossary term.
    Docs {
        #[command(subcommand)]
        action: DocsAction,
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
    /// Deprecated: use the white-label bundle workflow documented in `navigator.example.yaml`.
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
    /// The version the `cut-release` skill should hand to `--tag` on
    /// `ops release version` when the operator names none: today's UTC date
    /// under the `YY.M.D` convention, unless a release already exists that
    /// makes today's date no improvement over what is already published.
    ///
    /// Prints the bare tag on stdout and nothing else when there is one, so a
    /// caller can capture it directly: `tag=$(navigator ops
    /// release-default-tag)`. Prints nothing to stdout — only a
    /// human-readable reason on stderr — when today is already covered, so an
    /// empty capture means "nothing to cut" rather than a value to parse.
    /// Exits 0 either way: "nothing to cut today" is the ordinary answer on
    /// most days, not a failure.
    ///
    /// This changes nothing about `ops release version`, which still requires
    /// `--tag` and still derives nothing — see its own doc for why. This
    /// command only answers the narrower question of what today's date would
    /// even be called and whether it is worth asking for; naming the release
    /// is still `--tag`'s job.
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
        #[arg(long)]
        dry_run: bool,
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
        /// `gate.yml`/`publish.yml` pins Navigator's validate action to.
        /// Defaults to this binary's own version when — and only when — that
        /// version is one this repository has actually published, the same
        /// default `projects repository scaffold --action-version` uses.
        /// Unused, and never validated, against a repository this content
        /// reconciliation does not apply to.
        #[arg(long, default_value = published_cli_version())]
        action_version: String,
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
    /// `cli assets build`. Auth is ADC; the emulator endpoint is honored
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

#[derive(Subcommand)]
enum FontAction {
    /// Upload the licensed GORP Serif Regular and Bold WOFF2 files to
    /// `fonts/gorp-serif/` in the public assets bucket. Auth is ADC; this is
    /// an operator action and the source directory is never committed.
    Upload {
        /// Directory containing GORPSerif-Regular.woff2 and
        /// GORPSerif-Bold.woff2 from the operator's licensed delivery.
        #[arg(long)]
        dir: PathBuf,
        /// Target bucket. Defaults to `NAVIGATOR_ASSETS_BUCKET` — the public
        /// `<project>-assets` bucket.
        #[arg(long, env = "NAVIGATOR_ASSETS_BUCKET")]
        bucket: Option<String>,
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
enum GithubAction {
    /// Fill a `templates/github/` notation's `{{…}}` placeholders from
    /// `--answer` pairs and print the resulting Markdown.
    ///
    /// Local and DB-free: the notation is validated against the same rule
    /// set as `validate` first, then rendered. Placeholders with no
    /// `--answer` render verbatim and are reported on stderr, so a draft
    /// is obvious. This command opens nothing — it produces the body you
    /// paste into a pull request.
    Render {
        /// Which notation to render.
        #[arg(value_enum)]
        notation: github::Notation,
        /// Fill a `{{code}}` placeholder with `value`. Repeatable:
        /// `--answer custom_text__change_summary="Adds the shelf."`.
        #[arg(long = "answer", value_parser = parse_answer)]
        answers: Vec<(String, String)>,
        /// Write to this file instead of stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Render `create_issue.md` and open it as a GitHub issue.
    ///
    /// Calls the GitHub REST API directly through the same
    /// `workflows::github::IssueOpener` seam the `github_issue__*`
    /// workflow step dispatches through — never the `gh` CLI. Needs
    /// `NAVIGATOR_GITHUB_TOKEN` (or `GITHUB_TOKEN`); without one it opens
    /// nothing and says so.
    OpenIssue {
        /// Fill a `{{code}}` placeholder with `value`. Repeatable.
        #[arg(long = "answer", value_parser = parse_answer)]
        answers: Vec<(String, String)>,
        /// Target `owner/repo`. Defaults to `NAVIGATOR_GITHUB_REPO`.
        #[arg(long, env = "NAVIGATOR_GITHUB_REPO")]
        repo: Option<String>,
        /// Issue title. Defaults to the notation's frontmatter `title`.
        #[arg(long)]
        title: Option<String>,
        /// Label to apply. Repeatable.
        #[arg(long = "label")]
        labels: Vec<String>,
        /// Render and report the target without calling GitHub.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum FormsAction {
    /// Vendor + verify the blank government forms in the assets
    /// bucket. For each registry form: a local working copy at
    /// `templates/<object_path>` (untracked) is uploaded and its
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
    /// Writes the transformed working copy to `templates/<object_path>`
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
enum DocsAction {
    /// List every published docs page, including the ERD page and each
    /// glossary term anchor.
    List,
    /// Print canonical Neon Law Navigator vocabulary from
    /// `docs/glossary.md`. With no argument prints every term; with one
    /// argument prints just that term or anchor slug.
    Glossary {
        /// Optional term or glossary anchor slug to look up.
        term: Option<String>,
    },
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
        #[arg(long, value_parser = parse_document_visibility)]
        visibility: Option<String>,
        /// Optional description stored with the document.
        #[arg(long)]
        description: Option<String>,
        /// MIME type. Defaults to `application/octet-stream`.
        #[arg(long)]
        content_type: Option<String>,
    },
}

#[derive(Subcommand)]
enum NotationAction {
    /// Create a questionnaire-driven notation on an existing matter and
    /// leave its questionnaire ready for the site intake flow.
    ///
    /// Every notation hangs on an already-existing Project, so `--project`
    /// (the matter code) is required — open the matter first with
    /// `navigator db project create`. The template is read from that Project's
    /// git repo when authored there, else from the bundled firm catalog; the
    /// notation opens pinned to it
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

#[derive(Subcommand)]
enum DbProjectAction {
    /// Insert a new row in the `projects` table. By default runs
    /// migrate + seed first so the named `--entity-name` can
    /// resolve against the canonical seed. Pass
    /// `--skip-migrate-and-seed` when pointing at an
    /// already-managed store (e.g. a production database) to
    /// avoid touching the schema or upserting seed rows.
    Create {
        /// Human-readable matter name, e.g. `"Shook Estate"`.
        #[arg(long)]
        name: String,
        /// The matter code, e.g. `shook-estate`. Required: it names the
        /// matter's bare git repo *and* its folder in the firm's shared
        /// drive, and the two must match exactly (#938), so it is never
        /// derived. Lowercase letters, digits, and single hyphens.
        #[arg(long)]
        code: String,
        /// Exact `entities.name` of the legal organization this
        /// Project tracks. Omit for a Project not yet bound to any
        /// Entity.
        #[arg(long)]
        entity_name: Option<String>,
        /// Email of the pre-existing **client** Person this matter is
        /// opened for — its client-side DRI. Required: every matter has a
        /// client of record, and it must be a `role = client` person
        /// (create the client first). The lawyer-side DRI defaults to the
        /// firm principal.
        #[arg(long)]
        client_email: String,
        /// The opening attorney's conflict attestation. Required on every
        /// matter open: passing `--attest` affirms the attorney has checked
        /// for and cleared conflicts. Without it the open is refused — it is
        /// never defaulted. (At this firm the firm principal that opens a
        /// matter is an attorney; see navigator#355.)
        #[arg(long)]
        attest: bool,
        /// Skip the canonical seed — the caller owns the schema. Use
        /// this against an already-managed deployment where you don't
        /// want the canonical seed re-applied.
        #[arg(long)]
        skip_seed: bool,
    },
}

#[derive(Subcommand)]
enum ListSubject {
    /// List every row in the `questions` table.
    Questions,
    /// List every row in the `templates` table.
    Templates,
    /// List every row in the `jurisdictions` table.
    Jurisdictions,
    /// List every row in the `persons` table.
    Persons,
    /// List every row in the `entities` table.
    Entities,
    /// List every row in the `entity_types` table.
    EntityTypes,
    /// List every row in the `projects` table.
    Projects,
    /// List every row in the `letters` table.
    Letters,
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
        Command::Validate { dir, fix } => run_validate(&dir, fix),
        // The docs reference helpers need no cluster, so they are handled
        // here rather than routed into the KIND dispatcher with the rest
        // of `dev`.
        Command::Dev(DevCmd::Docs { action }) => match action {
            DocsAction::List => docs::list(),
            DocsAction::Glossary { term } => docs::glossary(term.as_deref()),
        },
        Command::Db { action } => {
            eprintln!("navigator: `db` is deprecated; use `navigator site seed` or local authoring commands");
            match action {
                DbCmd::List { subject } => runtime().block_on(run_list(subject)),
                DbCmd::Project { action } => runtime().block_on(run_db_project(action)),
                DbCmd::Erd { format } => runtime().block_on(run_erd(format)),
            }
        }
        Command::Site { action } => match action {
            SiteCmd::Import {
                model_name,
                seed_file,
                overwrite,
                host,
            } => runtime().block_on(remote::seed(
                host.host.as_deref(),
                &model_name,
                &seed_file,
                overwrite,
            )),
            SiteCmd::Login { host, no_browser } => {
                runtime().block_on(login::run_login(&host, no_browser))
            }
            SiteCmd::Seed { dir } => runtime().block_on(run_catalog_seed(&dir)),
            SiteCmd::Logout { host } => login::run_logout(host.as_deref()),
            SiteCmd::Whoami { host } => login::run_whoami(host.as_deref()),
            SiteCmd::Mcp { host } => runtime().block_on(mcp_bridge::run(host.as_deref())),
            SiteCmd::Document { action } => runtime().block_on(run_document(action)),
            SiteCmd::Projects { action } => runtime().block_on(run_projects(action)),
            SiteCmd::Notation { action } => runtime().block_on(run_notation(action)),
        },
        Command::Template { action } => match action {
            TemplateCmd::Format { file } => format::run(&file),
            TemplateCmd::Narrate { file, out } => narrate::run(&file, &out),
            TemplateCmd::Render {
                file,
                out,
                format,
                answers,
            } => run_render(&file, &out, format.as_deref(), &answers),
            TemplateCmd::Scaffold {
                matter,
                category,
                jurisdiction,
            } => scaffold::run(
                &scaffold::workspace_root_from_cli_dir(),
                &matter,
                &category,
                &jurisdiction,
            ),
            TemplateCmd::Transcribe {
                template,
                transcript,
                audio,
                speech_backend,
                google_project,
                google_location,
                google_language,
                google_model,
                pretty,
            } => runtime().block_on(run_transcribe(transcribe::CoverArgs {
                template,
                transcript,
                audio,
                speech_backend,
                google_project,
                google_location,
                google_language,
                google_model,
                pretty,
            })),
            TemplateCmd::Forms { action } => match action {
                FormsAction::Sync { bucket } => forms_sync::run_sync(bucket.as_deref()),
                FormsAction::Fields { code, bucket } => {
                    forms_sync::run_fields(&code, bucket.as_deref())
                }
                FormsAction::ReAuthor { code, bucket } => {
                    forms_sync::run_reauthor(&code, bucket.as_deref())
                }
            },
            TemplateCmd::Github { action } => match github::workspace_root() {
                Err(e) => {
                    eprintln!("navigator: {e}");
                    ExitCode::from(2)
                }
                Ok(root) => match action {
                    GithubAction::Render {
                        notation,
                        answers,
                        out,
                    } => github::run_render(&root, notation, &answers, out.as_deref()),
                    GithubAction::OpenIssue {
                        answers,
                        repo,
                        title,
                        labels,
                        dry_run,
                    } => runtime().block_on(github::run_open_issue(
                        &root,
                        &answers,
                        repo.as_deref(),
                        title.as_deref(),
                        &labels,
                        dry_run,
                    )),
                },
            },
        },
        // `lsp publish` and the `assets` pipeline
        // carry operator blast radius but are not cluster lifecycle, so they
        // are handled here rather than routed into the KIND/cloud dispatcher
        // below.
        Command::Ops(
            action @ (OpsCmd::Lsp { .. }
            | OpsCmd::Assets { .. }
            | OpsCmd::ReleaseDefaultTag { .. }
            | OpsCmd::Release { .. }
            | OpsCmd::Notices { .. }),
        ) => match action {
            OpsCmd::Notices { out, check } => notices::run(&out, check),
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
                    FontAction::Upload { dir, bucket } => assets::run_upload_fonts(&dir, bucket),
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

async fn run_transcribe(args: transcribe::CoverArgs) -> ExitCode {
    match transcribe::cover(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("navigator: transcribe: {e:?}");
            ExitCode::from(2)
        }
    }
}

/// Apply the schema before introspecting: a diagram of a database no one
/// has prepared is an empty diagram, not an error worth guessing at.
async fn run_erd(format: erd::OutputFormat) -> ExitCode {
    let db = match store::surreal::connect_from_env().await {
        Ok(db) => db,
        Err(e) => {
            eprintln!("navigator: surreal: {e}");
            return ExitCode::from(2);
        }
    };
    if let Err(e) = store::schema::apply(&db).await {
        eprintln!("navigator: schema: {e}");
        return ExitCode::from(2);
    }
    if let Err(e) = erd::run_surreal(&db, format).await {
        eprintln!("navigator: erd: {e}");
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
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

async fn run_list(subject: ListSubject) -> ExitCode {
    let surreal = match open_surreal().await {
        Ok(d) => d,
        Err(code) => return code,
    };
    // The schema is applied rather than migrated, and idempotently: a
    // fresh engine (the deploy interop's throwaway container) has no
    // tables until someone applies them.
    if let Err(e) = store::schema::apply(&surreal).await {
        eprintln!("navigator: schema: {e}");
        return ExitCode::from(2);
    }
    let storage = match cloud::from_env().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("navigator: storage: {e}");
            return ExitCode::from(2);
        }
    };
    if let Err(e) = store::seed::seed_canonical(&surreal, &storage).await {
        eprintln!("navigator: seed: {e}");
        return ExitCode::from(2);
    }
    let result = match subject {
        ListSubject::Questions => list::list_questions(&surreal).await,
        ListSubject::Templates => list::list_templates(&surreal).await,
        ListSubject::Jurisdictions => list::list_jurisdictions(&surreal).await,
        ListSubject::Persons => list::list_persons(&surreal).await,
        ListSubject::Entities => list::list_entities(&surreal).await,
        ListSubject::EntityTypes => list::list_entity_types(&surreal).await,
        ListSubject::Projects => list::list_projects(&surreal).await,
        ListSubject::Letters => list::list_letters(&surreal).await,
    };
    if let Err(e) = result {
        eprintln!("navigator: list: {e}");
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
}

async fn run_db_project(action: DbProjectAction) -> ExitCode {
    match action {
        DbProjectAction::Create {
            name,
            code,
            entity_name,
            client_email,
            attest,
            skip_seed,
        } => {
            run_project_create(
                &name,
                &code,
                entity_name.as_deref(),
                &client_email,
                attest,
                skip_seed,
            )
            .await
        }
    }
}

// One flag per argument the matter-open command needs; `--code` is among
// them (#938). A struct here would only rename the same list.
async fn run_project_create(
    name: &str,
    code: &str,
    entity_name: Option<&str>,
    client_email: &str,
    attest: bool,
    skip_seed: bool,
) -> ExitCode {
    let surreal = match open_surreal().await {
        Ok(conn) => conn,
        Err(code) => return code,
    };
    if !skip_seed {
        let storage = match cloud::from_env().await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("navigator: storage: {e}");
                return ExitCode::from(2);
            }
        };
        if let Err(e) = store::seed::seed_canonical(&surreal, &storage).await {
            eprintln!("navigator: seed: {e}");
            return ExitCode::from(2);
        }
    }
    match project::create(&surreal, name, code, entity_name, client_email, attest).await {
        Ok(p) => {
            println!(
                "{} {} (code={}, status={}, entity_id={})",
                palette::dim(format!("created project {}", p.id)),
                palette::highlight(&p.name),
                p.code,
                p.status,
                p.entity_id,
            );
            println!(
                "{}",
                palette::dim(format!(
                    "open a notation with: \
                     navigator site notation create <template_code> --project {}",
                    p.code
                )),
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("navigator: project create: {e}");
            ExitCode::from(2)
        }
    }
}

async fn run_projects(action: ProjectsCmd) -> ExitCode {
    match action {
        ProjectsCmd::List { host, json } => remote::projects_list(host.host.as_deref(), json).await,
        ProjectsCmd::Open { project_code, host } => {
            remote::matter_open(host.host.as_deref(), &project_code).await
        }
        ProjectsCmd::Doctor { host, project } => {
            projects::doctor::run(host.host.as_deref(), project.as_deref())
        }
        ProjectsCmd::Repository { action } => match action {
            ProjectRepositoryAction::Scaffold {
                project_code,
                dir,
                action_version,
            } => projects::repository::scaffold(&dir, &project_code, &action_version),
            ProjectRepositoryAction::Validate { dir, repository } => {
                projects::repository::validate(&dir, repository.as_deref())
            }
            ProjectRepositoryAction::SyncSkills { dir } => projects::repository::sync_skills(&dir),
        },
        ProjectsCmd::Drift {
            host,
            dir,
            all,
            json,
        } => projects::drift::run(host.host.as_deref(), &dir, all, json).await,
        ProjectsCmd::Surfaces { action } => match action {
            SurfacesAction::Reconcile { project } => projects::surfaces::reconcile(&project).await,
        },
    }
}

async fn run_notation(action: NotationAction) -> ExitCode {
    match action {
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
        } => {
            remote::document_upload(
                host.host.as_deref(),
                &project,
                &file,
                &kind,
                visibility.as_deref(),
                description.as_deref(),
                content_type.as_deref(),
            )
            .await
        }
    }
}

fn is_yaml_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|ext| ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml"))
}

/// Parse every `.yaml`/`.yml` file under `dir` as part of `validate`. Prints
/// one line per parse error plus a `Parsed N …` summary, and returns the
/// error count. Standalone YAML (k8s manifests, config, reference catalogs)
/// is disjoint from the markdown the classified engine lints, so this is a
/// second pass over the same tree rather than a second command.
fn yaml_pass(dir: &std::path::Path) -> std::io::Result<usize> {
    let mut files_scanned = 0usize;
    let mut errors = 0usize;
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !entry.file_type().is_dir()
                || (name != ".git" && name != "target" && name != ".worktrees")
        })
    {
        let entry = entry.map_err(std::io::Error::other)?;
        if !entry.file_type().is_file() || !is_yaml_path(entry.path()) {
            continue;
        }
        files_scanned += 1;
        let raw = std::fs::read_to_string(entry.path())?;
        for document in serde_yaml::Deserializer::from_str(&raw) {
            if let Err(err) = serde_yaml::Value::deserialize(document) {
                if let Some(location) = err.location() {
                    eprintln!(
                        "{}:{}:{}: YAML parse error: {err}",
                        entry.path().display(),
                        location.line(),
                        location.column()
                    );
                } else {
                    eprintln!("{}: YAML parse error: {err}", entry.path().display());
                }
                errors += 1;
                break;
            }
        }
    }
    println!("Parsed {files_scanned} YAML file(s), found {errors} error(s)");
    Ok(errors)
}

/// `Y001` — a `seeds/*.yaml` document must be accepted by `navigator site import`.
const SEED_DOCUMENT_CODE: &str = "Y001";

/// `Y002` — a `locales/<locale>/<page>.yaml` catalog must deserialize as that page.
const LOCALE_DOCUMENT_CODE: &str = "Y002";

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
fn seed_document_pass(dir: &std::path::Path) -> std::io::Result<usize> {
    let mut files_scanned = 0usize;
    let mut errors = 0usize;
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !entry.file_type().is_dir()
                || (name != ".git" && name != "target" && name != ".worktrees")
        })
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
            errors += 1;
        }
    }
    println!("Validated {files_scanned} seed document(s), found {errors} error(s)");
    Ok(errors)
}

/// Validate every brand locale catalog: `locales/<locale>/<page>.yaml`.
///
/// The site publishes English only. An unknown page stem or a locale directory
/// other than [`views::locales::DEFAULT_LOCALE`] is an error, and a known page
/// must deserialize as its typed catalog so a copy-only edit cannot land a
/// document the brand crate cannot load.
fn locale_document_pass(dir: &std::path::Path) -> std::io::Result<usize> {
    let mut files_scanned = 0usize;
    let mut errors = 0usize;
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !entry.file_type().is_dir()
                || (name != ".git" && name != "target" && name != ".worktrees")
        })
    {
        let entry = entry.map_err(std::io::Error::other)?;
        let path = entry.path();
        if !entry.file_type().is_file() {
            continue;
        }
        let Some((locale, stem)) = views::locales::locale_yaml_parts(path) else {
            continue;
        };
        files_scanned += 1;
        if locale != views::locales::DEFAULT_LOCALE {
            print_violation(
                &path.display().to_string(),
                1,
                LOCALE_DOCUMENT_CODE,
                &format!(
                    "locale directory `{locale}` is not published; only `{}` is allowed",
                    views::locales::DEFAULT_LOCALE
                ),
            );
            errors += 1;
            continue;
        }
        let raw = std::fs::read_to_string(path)?;
        if let Err(error) = views::locales::parse_locale_file(stem, &raw) {
            print_violation(&path.display().to_string(), 1, LOCALE_DOCUMENT_CODE, &error);
            errors += 1;
        }
    }
    println!("Validated {files_scanned} locale catalog(s), found {errors} error(s)");
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
/// Prints one line per offence and returns the count. Runs over YAML manifests,
/// Containerfiles, and workflow files alike; the `.git`, `target`, and
/// `.worktrees` trees are skipped, as in [`yaml_pass`].
fn mutable_tag_pass(dir: &std::path::Path) -> std::io::Result<usize> {
    let mut findings = 0usize;
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !entry.file_type().is_dir()
                || (name != ".git" && name != "target" && name != ".worktrees")
        })
    {
        let entry = entry.map_err(std::io::Error::other)?;
        let path = entry.path();
        if !entry.file_type().is_file() || !(is_yaml_path(path) || is_containerfile_path(path)) {
            continue;
        }
        let contents = std::fs::read_to_string(path)?;
        for (line, message) in detect_mutable_tags(path, &contents) {
            eprintln!("{}:{line}: {message}", path.display());
            findings += 1;
        }
    }
    println!("Checked consumed image/binary tags, found {findings} mutable tag(s)");
    Ok(findings)
}

/// The standalone passes `validate` runs over the raw tree after the markdown
/// lint — YAML syntax, seed-document shape, locale catalogs, and consumed
/// mutable tags.
fn standalone_tree_passes(dir: &std::path::Path) -> std::io::Result<(usize, usize, usize, usize)> {
    let yaml_errors = yaml_pass(dir)?;
    let seed_errors = seed_document_pass(dir)?;
    let locale_errors = locale_document_pass(dir)?;
    let mutable_tags = mutable_tag_pass(dir)?;
    Ok((yaml_errors, seed_errors, locale_errors, mutable_tags))
}

fn run_validate(dir: &std::path::Path, fix: bool) -> ExitCode {
    let question_codes = rules::canonical_question_codes();
    if fix {
        let fix_report = match fix_directory(dir, &rules::DefaultFileFilter::default(), |file| {
            rules::navigator_classified_rules_with_codes(file, &question_codes)
        }) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("navigator: {e}");
                return ExitCode::from(2);
            }
        };
        for path in &fix_report.fixed_files {
            println!("{}", palette::dim(format!("fixed {}", path.display())));
        }
        for v in &fix_report.remaining {
            print_violation(&v.path.display().to_string(), v.line, v.code, &v.message);
        }
        println!(
            "{}",
            palette::dim(format!(
                "Fixed {} file(s); {} remaining violation(s) need a human.",
                fix_report.fixed_files.len(),
                fix_report.remaining.len(),
            ))
        );
        return if fix_report.remaining.is_empty() {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        };
    }
    let report = {
        let engine = rules::ClassifiedRuleEngine::new().with_question_codes(question_codes);
        engine.lint_directory(dir)
    };
    let mut report = match report {
        Ok(r) => r,
        Err(e) => {
            eprintln!("navigator: {e}");
            return ExitCode::from(2);
        }
    };
    // Cross-file `N111`: notation template `code` must be unique across
    // the tree. Always run — only notation templates carry a `code`, so a
    // prose-only tree simply finds nothing.
    match rules::code_uniqueness_violations(dir, &rules::DefaultFileFilter::default()) {
        Ok(mut v) => report.violations.append(&mut v),
        Err(e) => {
            eprintln!("navigator: {e}");
            return ExitCode::from(2);
        }
    }
    for v in &report.violations {
        print_violation(&v.path.display().to_string(), v.line, v.code, &v.message);
    }
    let (error_count, warning_count) = severity_counts(&report.violations);

    println!(
        "{}",
        palette::dim(format!(
            "Scanned {} file(s), found {error_count} error(s), {warning_count} warning(s)",
            report.files_scanned,
        ))
    );

    // Standalone raw-tree passes over the same walk: YAML parse errors, seed
    // document shape, locale catalogs, and consumed mutable image/binary tags
    // (navigator#540).
    let (yaml_errors, seed_errors, locale_errors, mutable_tags) = match standalone_tree_passes(dir)
    {
        Ok(counts) => counts,
        Err(e) => {
            eprintln!("navigator: {e}");
            return ExitCode::from(2);
        }
    };

    // Fail the gate on Error-severity markdown violations, malformed YAML,
    // seed documents, locale catalogs, or a consumed mutable tag.
    // Warning-severity advisories (e.g. a step that's allowed but not built
    // yet) are printed but do not fail.
    if error_count > 0
        || yaml_errors > 0
        || seed_errors > 0
        || locale_errors > 0
        || mutable_tags > 0
    {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

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

const DOCUMENT_UPLOAD_KIND_HELP: &str = "Accepted --kind values: letter, filing, will, trust, directive, agreement, onboarding, offboarding, memo, transcript, inbound_contract, certificate_of_naturalization, unclassified.";

/// Render one notation template to a PDF. Validates the file against the
/// notation rule set, resolves the output format (CLI override →
/// `output:` frontmatter → plain), fills any `{{code}}` placeholders
/// from `answers`, and writes the compiled PDF to `out`.
fn run_render(
    file: &std::path::Path,
    out: &std::path::Path,
    format_override: Option<&str>,
    answers: &[(String, String)],
) -> ExitCode {
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
    // not built yet" — which every lawyer_review gate earns) are printed
    // but must not block rendering, mirroring `validate` / `site seed`.
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

    // Resolve the output format: explicit flag wins, else the
    // template's `output:` field, else plain.
    let declared = rules::frontmatter::extract(&contents)
        .and_then(|fm| rules::frontmatter::field(fm, "output"))
        .filter(|s| !s.is_empty());
    let format_name = format_override.map(str::to_string).or(declared);
    let format = match format_name.as_deref().map(pdf::OutputFormat::parse) {
        // No format declared anywhere: render a plain document.
        None => pdf::OutputFormat::Plain,
        Some(Some(f)) => f,
        Some(None) => {
            let name = format_name.unwrap_or_default();
            // Derive the accepted list from the format enum so a new
            // variant shows up in the hint without a manual edit here.
            // `plain` is the implicit default and absent from
            // `FRONTMATTER_VALUES`, so prepend it.
            let known = std::iter::once("plain")
                .chain(pdf::OutputFormat::FRONTMATTER_VALUES.iter().copied())
                .collect::<Vec<_>>()
                .join(", ");
            eprintln!("navigator: unknown --format `{name}` (expected one of: {known})");
            return ExitCode::from(2);
        }
    };

    // Body is everything after the frontmatter block; fill placeholders
    // through the same evaluator as preview and final document generation.
    // Bold each answer as it goes in, so a reader can find every fact
    // particular to their matter without reading the boilerplate. The other
    // half of the same idea lives in `pdf::markdown`: a placeholder that no
    // answer filled gets a yellow wash instead, so an unfinished document
    // is unmistakably unfinished.
    let answer_context = answers
        .iter()
        .map(|(code, value)| (code.clone(), pdf::markdown::bold_answer(value)))
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
    let bytes = match pdf::render_document(&body, format, &letterhead) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("navigator: render {}: {e}", file.display());
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
            "Rendered {} ({format:?}, {} bytes) → {}",
            file.display(),
            bytes.len(),
            out.display()
        ))
    );
    ExitCode::SUCCESS
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
    fixed_files: Vec<PathBuf>,
    remaining: Vec<rules::Violation>,
}

/// Walk `dir` honoring `filter`, apply every safe-by-construction
/// autofix to each markdown file in place, and then re-lint to
/// collect the diagnostic-only violations a human still needs to
/// address. Edits within a file are applied highest-offset-first so
/// earlier offsets stay valid; on overlap the rule with the lower
/// code string wins (deterministic).
fn fix_directory(
    dir: &std::path::Path,
    filter: &dyn rules::FileFilter,
    rules_for_file: impl Fn(&rules::SourceFile) -> Vec<Box<dyn rules::Rule>>,
) -> std::io::Result<FixReport> {
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
        let contents = std::fs::read_to_string(path)?;
        let mut file = rules::SourceFile {
            path: path.to_path_buf(),
            contents,
        };
        let rule_set = rules_for_file(&file);
        let mut edits: Vec<(rules::TextEdit, &'static str)> = Vec::new();
        for rule in &rule_set {
            for v in rule.lint(&file) {
                if let Some(edit) = rule.fix(&file, &v) {
                    edits.push((edit, rule.code()));
                }
            }
        }
        if !edits.is_empty() {
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
            if new_contents != file.contents {
                std::fs::write(path, &new_contents)?;
                fixed_files.push(path.to_path_buf());
                file.contents = new_contents;
            }
        }
        for rule in &rule_set {
            remaining.extend(rule.lint(&file));
        }
    }
    Ok(FixReport {
        fixed_files,
        remaining,
    })
}

/// Render a single rule violation: path/line in dim cyan-700, rule
/// code in cyan-500, message in default. Shared by validate and
/// site seed so both subcommands have the same look.
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

fn print_violation(path: &str, line: usize, code: &str, message: &str) {
    println!(
        "{} {}: {}",
        palette::dim(format!("{path}:{line}")),
        palette::highlight(code),
        message,
    );
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
