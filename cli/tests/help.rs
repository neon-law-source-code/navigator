//! Integration tests for the top-level `navigator --help` output.

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str;

fn help(args: &[&str]) -> String {
    let output = Command::cargo_bin("navigator")
        .unwrap()
        .args(args)
        .output()
        .expect("run navigator help");
    assert!(
        output.status.success(),
        "`navigator {}` failed:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("help output is utf-8")
}

/// Collapse every run of whitespace to a single space so assertions on help
/// prose survive clap's width-dependent line wrapping.
fn unwrapped(output: &str) -> String {
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn command_names(output: &str) -> Vec<&str> {
    let mut in_commands = false;
    let mut names = Vec::new();

    for line in output.lines() {
        match line.trim() {
            "Commands:" => {
                in_commands = true;
                continue;
            }
            "Options:" if in_commands => break,
            _ => {}
        }

        if in_commands {
            if let Some(command_row) = line
                .strip_prefix("  ")
                .filter(|rest| !rest.starts_with(char::is_whitespace))
            {
                let name = command_row
                    .split_whitespace()
                    .next()
                    .expect("command row has a command name");
                names.push(name);
            }
        }
    }

    names
}

#[test]
fn top_level_help_is_a_terse_legal_safe_headline() {
    Command::cargo_bin("navigator")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(str::contains("Navigator CLI, not legal advice."));
}

#[test]
fn top_level_help_keeps_orchestration_nested_under_groups() {
    let output = help(&["--help"]);

    assert_eq!(
        command_names(&output),
        vec![
            // These, and nothing else. Each names what it owns, so the top
            // layer IS the mental model rather than two dozen flat rows an
            // operator has to scan.
            "dev",
            "erd",
            "forms",
            "github",
            "notations",
            "ops",
            "project",
            "site",
            "validate",
            "help",
        ]
    );

    // Billing operations belong in the portal, not the notation author's CLI.
    assert!(!command_names(&output).contains(&"coupon"));
    assert!(!command_names(&output).contains(&"subscription"));

    // Command/description coupling, tolerant of clap's column width (which
    // grows with the longest command name).
    assert!(output.lines().any(|l| l.trim_start().starts_with("dev ")
        && l.contains("Local, reversible KIND developer loop")));
    assert!(output.lines().any(
        |l| l.trim_start().starts_with("ops ") && l.contains("Production and cloud operations")
    ));
    assert!(command_names(&output)
        .iter()
        .all(|name| !name.starts_with("start-")));
    assert!(!output.contains("  ship"));
    assert!(!output.contains("  deploy"));
    assert!(!command_names(&output).contains(&"login"));
    assert!(!command_names(&output).contains(&"projects"));
}

#[test]
fn catalog_seed_help_uses_a_headline() {
    let output = unwrapped(&help(&["site", "seed", "--help"]));

    assert!(
        output.contains("Seed the workspace-owned template and question catalog from clean files.")
    );
}

/// `notations` is the notation author's local workbench: every member
/// operates on files under `templates/notations/`, offline. Pinning the
/// membership keeps a live-site command, or the forms-vendoring and
/// engineering-intake commands (their own top-level homes), from drifting
/// into it.
#[test]
fn notations_help_lists_the_notation_authoring_workbench() {
    let output = help(&["notations", "--help"]);

    assert_eq!(
        command_names(&output),
        vec![
            "format",
            "narrate",
            "render",
            "scaffold",
            "transcribe",
            "help",
        ]
    );
}

/// `forms` owns vendoring, pinning, and inspecting the blank government
/// forms in the assets bucket. Pinning membership keeps it from drifting
/// beyond that one job.
#[test]
fn forms_help_lists_the_vendoring_operations() {
    let output = help(&["forms", "--help"]);

    assert_eq!(
        command_names(&output),
        vec!["fields", "re-author", "sync", "help"]
    );
}

/// `github` owns rendering and opening engineering-intake notations by
/// hand. Pinning membership keeps it from drifting beyond that one job.
#[test]
fn github_help_lists_the_intake_operations() {
    let output = help(&["github", "--help"]);

    assert_eq!(command_names(&output), vec!["open-issue", "render", "help"]);
}

#[test]
fn site_import_help_requires_the_seed_model_and_file() {
    Command::cargo_bin("navigator")
        .unwrap()
        .args(["site", "import", "--help"])
        .assert()
        .success()
        .stdout(str::contains("<MODEL_NAME> <SEED_FILE>"))
        .stdout(str::contains("--overwrite"))
        .stdout(str::contains("--dry-run"));
}

/// `project` is write-side and local; `site projects open` drives a running
/// deployment through the stored bearer token.
#[test]
fn project_help_lists_only_the_local_write_side() {
    assert_eq!(
        command_names(&help(&["project", "--help"])),
        vec!["create", "help"]
    );
    assert_eq!(
        command_names(&help(&["site", "projects", "--help"])),
        vec![
            "close",
            "doctor",
            "drift",
            "lifecycle",
            "list",
            "open",
            "repository",
            "surfaces",
            "help"
        ]
    );
}

/// `site projects` is the Project workspace group: the verbs that operate on
/// the Drive folder plus the one repository a code names live with the site's
/// project list and workbench.
///
/// `doctor` reads a machine, `repository` operates on a checkout, `drift`
/// reconciles the checkouts against the live rows, and `surfaces` creates
/// or adopts the Drive ingest folder and source repository. The split
/// matters, because `projects doctor` and `projects drift` promise to change
/// nothing, `repository scaffold` writes files, and `surfaces reconcile`
/// talks to Drive and the forge; `lifecycle` reads every row for admin-tier
/// oversight.
///
/// The retired `projects application` verbs are asserted gone rather than
/// merely absent from this list: a Project has one portal, so there is no
/// application name for an operator to register.
#[test]
fn projects_help_lists_the_project_workspace_verbs() {
    assert_eq!(
        command_names(&help(&["site", "projects", "--help"])),
        vec![
            "close",
            "doctor",
            "drift",
            "lifecycle",
            "list",
            "open",
            "repository",
            "surfaces",
            "help"
        ]
    );
    assert_eq!(
        command_names(&help(&["site", "projects", "repository", "--help"])),
        vec!["scaffold", "sync-skills", "validate", "help"]
    );
    assert_eq!(
        command_names(&help(&["site", "projects", "surfaces", "--help"])),
        vec!["reconcile", "help"]
    );
}

/// Two commands are spelled `doctor` and they diagnose different things.
/// `ops doctor` reads a running Kubernetes namespace; `projects doctor` reads
/// deployment workspace coordinates and this machine. Neither help text may
/// leave an operator guessing which one they want.
#[test]
fn the_two_doctors_keep_distinct_headlines() {
    let projects = unwrapped(&help(&["site", "projects", "doctor", "--help"]));
    assert!(
        projects.contains("Verify this machine and a Project workspace before Navigator creates."),
        "projects doctor headline: {projects}"
    );

    let ops = unwrapped(&help(&["ops", "doctor", "--help"]));
    assert!(
        ops.contains("Diagnose ongoing scheduled-job health."),
        "ops doctor headline: {ops}"
    );
}

/// `docs` becomes a `dev` member once `erd` moves to `db`: what is left are
/// the developer/agent reference helpers, needing no cluster and no database.
#[test]
fn dev_docs_keeps_only_the_reference_helpers() {
    assert_eq!(
        command_names(&help(&["dev", "docs", "--help"])),
        vec!["glossary", "list", "help"]
    );
}

#[test]
fn dev_help_lists_local_loop_members() {
    let output = help(&["dev", "--help"]);

    assert_eq!(
        command_names(&output),
        vec![
            "browser-e2e",
            "build-webapp",
            "deploy",
            "docs",
            "down",
            "e2e",
            "env",
            "garage-bootstrap",
            "grant-lawyer",
            "install",
            "kind",
            "kustomize",
            "logs",
            "sample-project",
            "sendgrid-openapi",
            "staging",
            "status",
            "undeploy",
            "up",
            "worker-reload",
            "worktree-env",
            "help",
        ]
    );
}

#[test]
fn ops_secrets_help_exposes_only_the_repo_apply_command() {
    assert_eq!(
        command_names(&help(&["ops", "secrets", "--help"])),
        // `apply` alone: the repository's `deployments/` tree is the operator
        // source. The Doppler-era members (`sync`, `diff`, `share`) are gone
        // with the provider.
        vec!["apply", "help"]
    );

    let apply = unwrapped(&help(&["ops", "secrets", "apply", "--help"]));
    assert!(apply.contains("--deployment"));
    assert!(apply.contains("--dry-run"));
    // The same tree-location flag `ops ship` takes. This command is the one an
    // operator runs FROM the deployment checkout, so it needs it at least as
    // much as ship does.
    assert!(apply.contains("--deployments-dir <DIR>"));
}

/// `ops ship` selects its deployment by an explicit flag — the whole safety
/// property, because an environment fallback is what lets a stale shell
/// silently ship the wrong deployment. The help must show the flag and never
/// mention the retired provider.
#[test]
fn ops_ship_help_requires_an_explicit_deployment() {
    let output = unwrapped(&help(&["ops", "ship", "--help"]));
    assert!(output.contains("--deployment"));
    assert!(output.contains("--tag"));
    assert!(output.contains("--dry-run"));
    assert!(output.contains("--restart-only"));
    // ENG-311. The flag an operator under a no-IAM-changes rule reaches for,
    // so its spelling is part of the runbook rather than an implementation
    // detail: renaming it would compile, leave every test green, and break
    // every operator who had written the old spelling down.
    assert!(output.contains("--assert-signing-iam"));
    assert!(!output.to_lowercase().contains("doppler"));

    // Omitting --deployment is a parse error, not a silent environment read.
    Command::cargo_bin("navigator")
        .unwrap()
        .args(["ops", "ship", "--tag", "26.1.1"])
        .assert()
        .failure()
        .stderr(str::contains("--deployment"));
}

/// The gate the deploy repository's CI runs, and the one thing an operator
/// must be able to read off its help: that it changes nothing and decrypts
/// nothing, so pointing it at a production tree is safe.
#[test]
fn ops_deployments_help_promises_a_read_only_names_only_check() {
    let output = unwrapped(&help(&["ops", "deployments", "--help"]));
    assert!(output.contains("--deployments-dir <DIR>"));
    assert!(
        output.contains("without changing anything or decrypting anything"),
        "the help must state what it will not do: {output}"
    );

    // Pointed at a directory with no tree it fails at the flag, rather than
    // walking up out of it into some unrelated checkout.
    Command::cargo_bin("navigator")
        .unwrap()
        .args([
            "ops",
            "deployments",
            "--deployments-dir",
            env!("CARGO_MANIFEST_DIR"),
        ])
        .assert()
        .failure()
        .stderr(str::contains("no `deployments/` directory"));
}

/// The two flags an automated deploy runs under, and the one thing an operator
/// must be able to read off the help before trusting a scheduled roll: that
/// `--image-only` changes images and nothing else, and that it refuses rather
/// than half-applying when the manifests have moved on.
#[test]
fn ops_ship_help_describes_the_narrow_automated_lane() {
    let output = unwrapped(&help(&["ops", "ship", "--help"]));
    assert!(output.contains("--image-only"));
    assert!(output.contains("--deployments-dir <DIR>"));

    // Pointed at a directory with no tree, it fails at the flag rather than
    // walking up out of it into some unrelated checkout.
    Command::cargo_bin("navigator")
        .unwrap()
        .args([
            "ops",
            "ship",
            "--deployment",
            "neon-law-stg",
            "--deployments-dir",
            env!("CARGO_MANIFEST_DIR"),
            "--image-only",
            "--tag",
            "26.1.1",
        ])
        .assert()
        .failure()
        .stderr(str::contains("no `deployments/` directory"));
}

#[test]
fn worktree_env_help_lists_its_lifecycle_and_the_reclaim_command() {
    let output = help(&["dev", "worktree-env", "--help"]);

    assert_eq!(
        command_names(&output),
        vec!["down", "status", "sweep", "up", "help"]
    );
}

#[test]
fn worktree_env_sweep_help_exposes_its_explicit_apply_flag() {
    let output = unwrapped(&help(&["dev", "worktree-env", "sweep", "--help"]));

    assert!(output.contains("--apply"));
}

#[test]
fn browser_e2e_help_lists_the_gate_overrides() {
    Command::cargo_bin("navigator")
        .unwrap()
        .args(["dev", "browser-e2e", "--help"])
        .assert()
        .success()
        .stdout(str::contains("Usage: navigator dev browser-e2e"))
        .stdout(str::contains("--base-url"))
        .stdout(str::contains("NAV_BASE_URL"));
}

#[test]
fn grant_lawyer_help_names_the_store_it_grants_in() {
    Command::cargo_bin("navigator")
        .unwrap()
        .args(["dev", "grant-lawyer", "--help"])
        .assert()
        .success()
        .stdout(str::contains("Usage: navigator dev grant-lawyer"));
}

/// `site` owns everything that authenticates to a running deployment with the
/// stored bearer token. `auth` is flattened away: once `site` names the group,
/// `site login` already reads as "log in to the site", and the extra noun only
/// lengthened the path the regroup exists to shorten.
///
/// The order is two runs, not one list. `login` / `logout` / `whoami` / `mcp`
/// operate on the stored credential itself — `mcp` sits with them because it
/// hands that credential to an agent client rather than driving a matter. The
/// rest are the matter nouns a session then acts on.
#[test]
fn site_help_lists_the_live_deployment_members() {
    let output = help(&["site", "--help"]);

    assert_eq!(
        command_names(&output),
        vec![
            "document", "import", "login", "logout", "mcp", "notation", "projects", "seed",
            "whoami", "help",
        ]
    );
}

#[test]
fn site_document_upload_help_requires_kind() {
    let output = unwrapped(&help(&["site", "document", "upload", "--help"]));
    assert!(
        output.contains("--kind <KIND>"),
        "usage must require --kind, got: {output}"
    );
    assert!(
        !output.contains("[--kind"),
        "kind must not be an optional flag, got: {output}"
    );
    for kind in rules::kind::Kind::ALL
        .iter()
        .filter(|k| k.valid_for(rules::kind::Lane::Asset))
    {
        assert!(
            output.contains(kind.as_str()),
            "long help must name asset-lane kind `{}`, got: {output}",
            kind.as_str()
        );
    }
}

#[test]
fn site_document_upload_refuses_a_missing_kind() {
    Command::cargo_bin("navigator")
        .unwrap()
        .args([
            "site",
            "document",
            "upload",
            "--project",
            "acme",
            "--file",
            "note.txt",
        ])
        .assert()
        .failure()
        .stderr(str::contains("--kind"));
}

#[test]
fn site_document_upload_refuses_a_template_lane_kind() {
    let output = Command::cargo_bin("navigator")
        .unwrap()
        .args([
            "site",
            "document",
            "upload",
            "--project",
            "acme",
            "--file",
            "note.txt",
            "--kind",
            "review_queue_workbench",
        ])
        .output()
        .expect("run navigator");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    for kind in rules::kind::Kind::ALL
        .iter()
        .filter(|k| k.valid_for(rules::kind::Lane::Asset))
    {
        assert!(
            stderr.contains(kind.as_str()),
            "kind error must name asset-lane kind `{}`, got: {stderr}",
            kind.as_str()
        );
    }
}

#[test]
fn site_login_help_exposes_headless_mode() {
    Command::cargo_bin("navigator")
        .unwrap()
        .args(["site", "login", "--help"])
        .assert()
        .success()
        .stdout(str::contains("--no-browser"));
}

#[test]
fn ops_help_lists_operator_members() {
    let output = help(&["ops", "--help"]);

    assert_eq!(
        command_names(&output),
        vec![
            "assets",
            "deployments",
            "dns",
            "doctor",
            "gcp",
            "github",
            "lsp",
            "notices",
            "observability",
            "rebrand",
            "release",
            "release-default-tag",
            "restate",
            "secrets",
            "ship",
            "surreal-archive",
            "help",
        ]
    );
}

#[test]
fn ops_gcp_help_lists_the_hub_alongside_the_environment_provisioner() {
    let output = help(&["ops", "gcp", "--help"]);

    assert_eq!(
        command_names(&output),
        vec![
            // The two provisioners, widest blast radius first: an environment,
            // then the shared registry.
            "hub", "iap", "setup", "help",
        ]
    );
}

#[test]
fn ops_gcp_hub_help_uses_a_headline() {
    let output = unwrapped(&help(&["ops", "gcp", "hub", "--help"]));

    assert!(output.contains("Provision the shared image hub."));
}

#[test]
fn ops_gcp_hub_setup_help_lists_its_flags() {
    Command::cargo_bin("navigator")
        .unwrap()
        .args(["ops", "gcp", "hub", "setup", "--help"])
        .assert()
        .success()
        .stdout(str::contains("--project-id"))
        .stdout(str::contains("--artifact-registry-repo"))
        .stdout(str::contains("--github-repo"))
        .stdout(str::contains("--ci-pusher-account-id"))
        .stdout(str::contains("--dry-run"))
        // Environment-only resources must be unreachable from this command.
        .stdout(str::contains("--cluster-name").not())
        .stdout(str::contains("--public-base-url").not());
}

#[test]
fn ops_gcp_setup_help_exposes_the_hub_registry_flag() {
    Command::cargo_bin("navigator")
        .unwrap()
        .args(["ops", "gcp", "setup", "--help"])
        .assert()
        .success()
        .stdout(str::contains("--images-project-id"));
}

/// The hidden `up` alias for `dev up` is gone. It cost nothing in `--help`,
/// but it was a second top-level spelling for a command that already has one,
/// and a surface that is exactly six entries cannot also carry a seventh only
/// some people know about. `dev up` is the spelling.
#[test]
fn the_hidden_up_alias_no_longer_parses() {
    Command::cargo_bin("navigator")
        .unwrap()
        .args(["up", "--help"])
        .assert()
        .failure()
        .stderr(str::contains("unrecognized subcommand"));

    Command::cargo_bin("navigator")
        .unwrap()
        .args(["dev", "up", "--help"])
        .assert()
        .success();
}

#[test]
fn notation_create_help_lists_template_and_client_flags() {
    Command::cargo_bin("navigator")
        .unwrap()
        .args(["site", "notation", "create", "--help"])
        .assert()
        .success()
        .stdout(str::contains("Usage: navigator site notation create"))
        .stdout(str::contains("<TEMPLATE_CODE>"))
        .stdout(str::contains("--client-email"))
        .stdout(str::contains("--project"));
}

/// `--entity-name` reads as optional to clap, because `project create`
/// resolves it to an id itself and wants to name the missing entity in its own
/// error rather than clap's. Its doc comment once said so in prose too — "Omit
/// for a Project not yet bound to any Entity" — and no door in the system
/// permits that: `projects.entity_id` is NOT NULL, `project::create` refuses a
/// `None`, and the web form marks the field required.
///
/// **This asserts on the source, not on `--help`, and that is the point.**
/// `help_headline` cuts every description at the first `.!?:;—` followed by
/// whitespace and caps it at ten words, so the rendered line is only "Exact
/// `entities.name` of the legal organization this Project tracks." The retired
/// claim lived past that cut and never reached a terminal — which is exactly
/// why nobody running the command reported it, and why a rendered-output
/// assertion would be worthless here: the claim could come back in a later
/// sentence and a `--help` test would still pass.
///
/// So the guarded surface is the one that actually carries the meaning. That
/// is not a workaround; `help_headline`'s own documentation says the Rust
/// documentation "remains the source of operational detail, while terminal
/// help is deliberately just a scan-friendly headline". The detail layer is
/// where the drift happened, so the detail layer is what gets pinned.
#[test]
fn project_create_documents_the_entity_as_required() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"),
    )
    .expect("read cli/src/main.rs");

    let doc = doc_comment_above(&source, "entity_name: Option<String>,");

    assert!(
        !doc.contains("Omit for a Project"),
        "the `--entity-name` documentation must not offer an Entity-less \
         matter, which no door in the system permits: {doc}"
    );
    assert!(
        doc.contains("Required") && doc.contains("NOT NULL"),
        "the `--entity-name` documentation must say the Entity is required, \
         and why: {doc}"
    );
}

/// The contiguous `///` block immediately above the line containing `needle`,
/// joined into one string. Intervening `#[arg(..)]` attributes and plain `//`
/// comments are stepped over, so the block found is the documentation clap
/// reads rather than whatever happens to sit closest.
fn doc_comment_above(source: &str, needle: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let at = lines
        .iter()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("`{needle}` is not in cli/src/main.rs"));

    let mut doc: Vec<&str> = Vec::new();
    for line in lines[..at].iter().rev() {
        let trimmed = line.trim_start();
        if let Some(text) = trimmed.strip_prefix("///") {
            doc.push(text.trim());
            continue;
        }
        if trimmed.starts_with("#[") || trimmed.starts_with("//") {
            continue;
        }
        break;
    }
    doc.reverse();
    doc.join(" ")
}
