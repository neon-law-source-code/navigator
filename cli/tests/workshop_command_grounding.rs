//! Extraction grounding test: every `navigator …` command the workshop docs
//! and the `cli` crate README publish must parse against the real binary.
//!
//! `server/content/workshops/navigator/{README,DEPLOY,CONTRIBUTE}.md` and `cli/README.md`
//! teach concrete `navigator` command sequences. That prose is a public
//! promise, and nothing stops it drifting from the binary — a renamed flag, a
//! dropped subcommand, or a command written with the wrong shape (a positional
//! host where `login` requires `--host`). This test pulls every `navigator`
//! invocation out of the fenced ` ```bash ` blocks and asserts each one parses.
//!
//! Validation is parse-only and side-effect-free: each extracted command is
//! invoked with `--help` appended, which makes clap short-circuit before any
//! network or browser I/O while still rejecting an unknown flag or an
//! unexpected positional argument. Angle-bracket placeholders (`<notation-id>`,
//! `<your-host>`) are normalized to a nil UUID so a typed positional (the
//! `NOTATION_ID` UUID) parses as a value rather than failing validation.
//!
//! This test pins the syntax of every published command across the workshop
//! pages.

use std::path::Path;
use std::process::Command;

/// A nil UUID standing in for any `<…>` placeholder, so a typed positional
/// such as `<notation-id>` parses as a value instead of failing UUID
/// validation.
const PLACEHOLDER: &str = "00000000-0000-0000-0000-000000000000";

/// The docs whose `navigator` commands are grounded against the binary.
const DOCS: &[&str] = &[
    "server/content/workshops/navigator/README.md",
    "server/content/workshops/navigator/DEPLOY.md",
    "server/content/workshops/navigator/CONTRIBUTE.md",
    "cli/README.md",
];

/// Read a repo-root file relative to this crate (`cli/` → workspace root is
/// one level up).
fn repo_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {} — {e}", path.display()))
}

/// Every `navigator …` invocation inside a fenced ` ```bash ` block, with
/// backslash-continued lines joined and any trailing ` # …` comment dropped.
fn navigator_commands(md: &str) -> Vec<String> {
    let mut commands = Vec::new();
    let mut in_bash = false;
    let mut pending = String::new();
    for raw in md.lines() {
        let fence = raw.trim();
        if fence.starts_with("```") {
            in_bash = fence == "```bash";
            pending.clear();
            continue;
        }
        if !in_bash {
            continue;
        }
        let line = raw.trim_end();
        if let Some(prefix) = line.strip_suffix('\\') {
            pending.push_str(prefix.trim_start());
            pending.push(' ');
            continue;
        }
        let full = if pending.is_empty() {
            line.trim_start().to_string()
        } else {
            let joined = format!("{pending}{}", line.trim_start());
            pending.clear();
            joined
        };
        if let Some(rest) = strip_comment(&full).trim().strip_prefix("navigator ") {
            commands.push(rest.trim().to_string());
        }
    }
    commands
}

/// Drop a trailing ` # comment`. The published commands carry no `#` inside a
/// value, so the first ` #` begins the comment.
fn strip_comment(line: &str) -> &str {
    match line.find(" #") {
        Some(i) => &line[..i],
        None => line,
    }
}

/// Split a shell-ish argument string on whitespace, honoring single and double
/// quotes, and normalize each `<…>` placeholder to the nil UUID.
fn tokenize(args: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in args.chars() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                cur.push(c);
            }
        } else if c == '\'' || c == '"' {
            quote = Some(c);
            started = true;
        } else if c.is_whitespace() {
            if started {
                tokens.push(normalize(&std::mem::take(&mut cur)));
                started = false;
            }
        } else {
            cur.push(c);
            started = true;
        }
    }
    if started {
        tokens.push(normalize(&cur));
    }
    tokens
}

/// Replace an angle-bracket placeholder (`<notation-id>`) with the nil UUID.
fn normalize(token: &str) -> String {
    if token.starts_with('<') && token.ends_with('>') {
        PLACEHOLDER.to_string()
    } else {
        token.to_string()
    }
}

/// True when the documented command is a meta-placeholder rather than a
/// runnable invocation — `navigator <COMMAND>` (subcommand normalized to the
/// placeholder UUID) or a bare `navigator --help` / `--version`.
fn is_meta(tokens: &[String]) -> bool {
    match tokens.first() {
        None => true,
        Some(first) => first == PLACEHOLDER || first.starts_with('-'),
    }
}

/// Run `navigator <tokens> --help` and return `(parsed_ok, output)`. `--help`
/// short-circuits clap before any network or browser I/O, but clap still
/// rejects an unknown flag or an unexpected positional first.
fn parses(tokens: &[String]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_navigator"))
        .args(tokens)
        .arg("--help")
        .output()
        .expect("run navigator --help");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    (out.status.success(), text)
}

#[test]
fn every_published_navigator_command_parses() {
    let mut checked = 0_usize;
    let mut failures = Vec::new();
    for doc in DOCS {
        for command in navigator_commands(&repo_file(doc)) {
            let tokens = tokenize(&command);
            if is_meta(&tokens) {
                continue;
            }
            checked += 1;
            let (ok, output) = parses(&tokens);
            if !ok {
                failures.push(format!("{doc}: `navigator {command}`\n{}", output.trim()));
            }
        }
    }
    assert!(
        checked > 0,
        "extracted no navigator commands — the extractor or the docs changed shape",
    );
    assert!(
        failures.is_empty(),
        "{} published navigator command(s) no longer parse against the binary:\n\n{}",
        failures.len(),
        failures.join("\n\n"),
    );
}

#[test]
fn the_deploy_workshop_promises_no_phantom_bin_wrapper() {
    // DEPLOY.md once told readers to `export PATH="$PWD/bin:$PATH"` and call a
    // `bin/navigator` wrapper that never shipped — there is no `bin/` dir, and a
    // shell wrapper would violate the Rust-only invariant anyway. The
    // skip-install path is `cargo run -p cli -- <args>`.
    let deploy = repo_file("server/content/workshops/navigator/DEPLOY.md");
    assert!(
        !deploy.contains("$PWD/bin"),
        "DEPLOY.md points readers at a `$PWD/bin` wrapper that does not ship — \
         use `cargo run -p cli -- <args>` for the skip-install path",
    );
}

#[test]
fn deployment_dns_is_a_direct_dnsimple_transaction_outside_doppler() {
    let deploy = repo_file("server/content/workshops/navigator/DEPLOY.md");
    let dns = deploy
        .split("### Point your domain at the instance (optional)")
        .nth(1)
        .expect("deployment workshop must contain the DNS section")
        .split("### Drive it from the CLI")
        .next()
        .expect("DNS section must end before the site CLI section");

    assert!(
        !dns.contains("doppler run"),
        "DNS must remain outside Doppler and the three-stack provisioning driver"
    );
    assert!(
        !dns.contains("navigator ops dns setup"),
        "the three-site cutover uses the reviewed DNSimple CLI transaction"
    );
    assert!(
        dns.contains("content: \"https://www.neonlaw.com\""),
        "the transaction must verify the naked neonlaw.com redirect to www"
    );

    for (host, address) in [
        ("staging", "34.160.169.219"),
        ("workflows-staging", "34.160.169.219"),
        ("www", "8.233.220.29"),
        ("workflows", "8.233.220.29"),
    ] {
        let command = format!("--name {host} \\\n  --content {address} --ttl 300");
        assert!(
            dns.contains(&command),
            "DNS transaction must pin {host} to {address}"
        );
    }
}

#[test]
fn deployment_provider_attachments_stay_local_to_each_config() {
    let deploy = repo_file("server/content/workshops/navigator/DEPLOY.md");
    let provider_boundary = deploy
        .split("#### Keep provider attachments deployment-local")
        .nth(1)
        .expect("deployment workshop must document the provider isolation boundary")
        .split("### The external surface")
        .next()
        .expect("provider section must end before the external-surface slide");

    assert!(!provider_boundary.contains("navigator ops secrets share"));
    assert!(
        provider_boundary.contains("navigator ops secrets apply --deployment"),
        "Secret Manager bootstrap must stay inside the Navigator ops CLI, per deployment"
    );
    for isolated in [
        "Restate journal",
        "DocuSign attachment",
        "SendGrid credentials",
        "OAuth clients",
        "Drive root",
        "GitHub App",
        "database",
        "session key",
        "application-signing keys",
    ] {
        assert!(
            provider_boundary.contains(isolated),
            "provider section must preserve the {isolated} isolation boundary"
        );
    }
}

#[test]
fn deployment_workshop_records_the_staging_oauth_checkpoint_and_nullable_gemini() {
    let deploy = repo_file("server/content/workshops/navigator/DEPLOY.md");
    let prose = deploy.split_whitespace().collect::<Vec<_>>().join(" ");

    for observed in [
        "The staging browser client exists",
        "External/Testing",
        "the authenticated operator is its initial test user",
        "deployment config carries only that browser ID and secret",
        "Gemini ID remains absent",
        "issues/1126",
    ] {
        assert!(
            prose.contains(observed),
            "deployment workshop must preserve the OAuth checkpoint `{observed}`"
        );
    }
}

#[test]
fn deployment_workshop_runs_every_persistent_row_on_the_production_profile() {
    let deploy = repo_file("server/content/workshops/navigator/DEPLOY.md");
    let prose = deploy.split_whitespace().collect::<Vec<_>>().join(" ");

    for observed in [
        "The two production clusters currently have no application namespace",
        "their public hosts do not yet answer TLS",
        "disposable `navigator dev staging` integration surface",
        "All three managed GKE rows use `production`",
        "“Test” is the `dev` profile with `NAVIGATOR_CI_HARNESS=1`",
        "The `-staging` suffix remains a deployment identity and release ring",
        "It never becomes a weaker runtime profile",
        "`NAVIGATOR_CREDENTIAL_ENVIRONMENT` to `production` in `deployments/neon-law-stg/config.toml`",
        "cargo install --path cli --force",
        "The failed guard runs before manifest rendering or cluster mutation",
    ] {
        assert!(
            prose.contains(observed),
            "deployment workshop must preserve the hosted-runtime checkpoint `{observed}`"
        );
    }

    assert!(
        !deploy.contains("No Navigator release has been shipped"),
        "the workshop must not regress the current deployment checkpoint"
    );
}

#[test]
fn deployment_workshop_separates_the_live_bucket_checkpoint_from_the_single_bucket_target() {
    let deploy = repo_file("server/content/workshops/navigator/DEPLOY.md");
    let prose = deploy.split_whitespace().collect::<Vec<_>>().join(" ");

    for observed in [
        "Each provisioned row has five private assets, documents, exports, logs, and applications buckets",
        "current checkpoint, not the target topology",
        "exactly one private object-storage bucket per deployment",
        "never one bucket per Project",
        "`{project-code}/documents/`",
        "issues/1103",
        "does not grant `allUsers`",
    ] {
        assert!(
            prose.contains(observed),
            "deployment workshop must preserve the storage-topology checkpoint `{observed}`"
        );
    }
}

// ---------------------------------------------------------------------------
// Template-code grounding
// ---------------------------------------------------------------------------
//
// The `--help` grounding above pins command *syntax*: clap short-circuits on
// `--help` before it validates a positional, so
// `navigator site notation create onboarding__retainer --help` parses happily
// long after that code has left the catalog. It did — #229 folded
// `onboarding__retainer` into `onboarding__engagement_letter` and three decks
// went on teaching the dead code with every test green.
//
// These two guards close that gap by resolving the code itself against
// `store::seed::seeded_template_codes()`, the same frontmatter parse the
// seeder uses, so the decks cannot drift from the shipped catalog again.
//
// Scope is deliberate: a template code is checked where it must *resolve* —
// inside a `navigator site notation create` invocation, and as the `code:` key
// of a notation-frontmatter sample. Prose that merely names a code in
// backticks is not checked, because `AGENTS.md` ("Leave a slide's words
// alone") makes a deck's wording the author's call, and a guard over prose
// would turn a wording judgment into a CI failure.

/// Every deck under the workshops directory, discovered rather than listed, so
/// a new deck is covered the day it lands. `cli/README.md` teaches the same
/// commands and is checked alongside them.
fn template_code_docs() -> Vec<String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("server/content/workshops/navigator");
    let mut docs: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {} — {e}", dir.display()))
        .map(|entry| entry.expect("read workshops dir entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .map(|path| format!("server/content/workshops/navigator/{}", file_name(&path)))
        .collect();
    docs.push("cli/README.md".to_string());
    docs.sort();
    assert!(
        docs.len() > 1,
        "found no workshop decks — the workshops directory moved or changed shape",
    );
    docs
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .expect("deck path has a file name")
        .to_string_lossy()
        .into_owned()
}

/// The codes in the shipped catalog, parsed from the seeded templates'
/// frontmatter.
fn seeded_codes() -> Vec<String> {
    store::seed::seeded_template_codes().expect("parse seeded template frontmatter")
}

/// Render the catalog for a failure message.
fn catalog(codes: &[String]) -> String {
    codes
        .iter()
        .map(|code| format!("  - {code}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn every_documented_notation_create_names_a_seeded_template_code() {
    let codes = seeded_codes();
    let mut checked = 0_usize;
    let mut failures = Vec::new();

    for doc in template_code_docs() {
        for command in navigator_commands(&repo_file(&doc)) {
            let tokens = tokenize(&command);
            // `site notation create <CODE> …`
            let code = match tokens.as_slice() {
                [site, notation, create, code, ..]
                    if site == "site" && notation == "notation" && create == "create" =>
                {
                    code
                }
                _ => continue,
            };
            if code == PLACEHOLDER || code.starts_with('-') {
                continue;
            }
            checked += 1;
            if !codes.iter().any(|seeded| seeded == code) {
                failures.push(format!(
                    "{doc}: `navigator site notation create {code}` names a template code \
                     that is not in the seeded catalog",
                ));
            }
        }
    }

    assert!(
        checked > 0,
        "extracted no `notation create` commands — the extractor or the docs changed shape",
    );
    assert!(
        failures.is_empty(),
        "{} documented notation-create command(s) name an unseeded template code:\n\n{}\n\nseeded catalog:\n{}",
        failures.len(),
        failures.join("\n"),
        catalog(&codes),
    );
}

#[test]
fn every_documented_frontmatter_sample_names_a_seeded_template_code() {
    let codes = seeded_codes();
    let mut checked = 0_usize;
    let mut failures = Vec::new();

    for doc in template_code_docs() {
        for (code, line) in frontmatter_sample_codes(&repo_file(&doc)) {
            checked += 1;
            if !codes.iter().any(|seeded| seeded == &code) {
                failures.push(format!(
                    "{doc}:{line}: frontmatter sample declares `code: {code}`, \
                     which is not in the seeded catalog",
                ));
            }
        }
    }

    assert!(
        checked > 0,
        "extracted no frontmatter samples — the extractor or the docs changed shape",
    );
    assert!(
        failures.is_empty(),
        "{} documented frontmatter sample(s) name an unseeded template code:\n\n{}\n\nseeded catalog:\n{}",
        failures.len(),
        failures.join("\n"),
        catalog(&codes),
    );
}

/// Every `code:` key declared inside a fenced ` ```yaml ` block that is a
/// notation-template frontmatter sample, with its 1-based line number.
///
/// A block qualifies only when it also carries one of the frontmatter keys
/// that mark a notation template (`respondent_type:`, `questionnaire:`,
/// `workflow:`). Unrelated YAML in a deck — a manifest, a config excerpt —
/// may legitimately hold its own `code:` and must not be resolved against the
/// notation catalog.
fn frontmatter_sample_codes(md: &str) -> Vec<(String, usize)> {
    const MARKERS: &[&str] = &["respondent_type:", "questionnaire:", "workflow:"];
    let mut found = Vec::new();
    let lines: Vec<&str> = md.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() != "```yaml" {
            i += 1;
            continue;
        }
        let start = i + 1;
        let mut end = start;
        while end < lines.len() && !lines[end].trim().starts_with("```") {
            end += 1;
        }
        let block = &lines[start..end];
        let is_frontmatter = block.iter().any(|line| {
            MARKERS
                .iter()
                .any(|marker| line.trim_start().starts_with(marker))
        });
        if is_frontmatter {
            for (offset, line) in block.iter().enumerate() {
                if let Some(value) = line.strip_prefix("code:") {
                    found.push((value.trim().to_string(), start + offset + 1));
                }
            }
        }
        i = end + 1;
    }
    found
}
