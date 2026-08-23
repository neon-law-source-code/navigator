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
fn top_level_help_disclaims_legal_advice() {
    Command::cargo_bin("navigator")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(str::contains("Nothing here is legal advice"))
        .stdout(str::contains("sovereign software for law firms"));
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
            "validate", // the one externally consumed command; it never moves
            "template", // the notation author's offline workbench
            "db",       // deployment data command boundary
            "login",    // browser loopback for the bearer db seed uses
            "site",     // a running deployment, via the stored bearer token
            "projects", // the Drive folder + the one repository a code names
            "dev",      // the local KIND loop plus the reference helpers
            "ops",      // operator blast radius
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
    assert!(!command_names(&output).contains(&"import"));
}

#[test]
fn catalog_seed_help_explains_the_partial_seed_contract() {
    // clap re-wraps long help paragraphs to the terminal width, so a phrase can
    // straddle a line break. Collapse whitespace before matching on prose.
    let output = unwrapped(&help(&["db", "catalog-seed", "--help"]));

    assert!(output.contains("workspace-owned template and question catalog"));
    assert!(output.contains("workspace-shared"));
    assert!(output.contains("clean files may seed"));
    assert!(output.contains("exit nonzero"));
}

/// `template` is the notation author's local workbench: every member operates
/// on files under `templates/`, offline. Pinning the membership keeps a
/// live-site or database command from drifting into it.
#[test]
fn template_help_lists_the_notation_authoring_workbench() {
    let output = help(&["template", "--help"]);

    assert_eq!(
        command_names(&output),
        vec![
            "format",
            "render",
            "scaffold",
            "transcribe",
            "forms",
            "github",
            "help",
        ]
    );
}

/// `db` owns the direct store operations. `erd` belongs here rather than
/// under `docs` because it introspects the schema — only its output is
/// documentation. Pinning membership keeps a live-site command, which
/// authenticates to a deployment instead, from drifting in.
#[test]
fn db_help_lists_the_direct_store_operations() {
    let output = help(&["db", "--help"]);

    assert_eq!(
        command_names(&output),
        vec!["catalog-seed", "seed", "list", "project", "erd", "help",]
    );
}

#[test]
fn db_seed_help_requires_the_seed_model_and_file() {
    Command::cargo_bin("navigator")
        .unwrap()
        .args(["db", "seed", "--help"])
        .assert()
        .success()
        .stdout(str::contains("<MODEL_NAME> <SEED_FILE>"))
        .stdout(str::contains("--overwrite"));
}

/// `db project` is write-side and local; `site project open` drives a running
/// deployment through the stored bearer token. Same noun, two groups, because
/// they are two different kinds of command.
#[test]
fn db_project_holds_only_the_local_write_side() {
    assert_eq!(
        command_names(&help(&["db", "project", "--help"])),
        vec!["create", "help"]
    );
    assert_eq!(
        command_names(&help(&["site", "project", "--help"])),
        vec!["open", "help"]
    );
}

/// `projects` is the seventh top-level group: Project is the organizing noun
/// of the product, so the verbs that operate on the Drive folder plus the one
/// repository a code names sit at the top layer rather than inside `site`.
///
/// `doctor` reads a machine and `repository` operates on a checkout — the split
/// matters, because `projects doctor` promises to change nothing and
/// `repository scaffold` writes files.
///
/// The retired `projects application` verbs are asserted gone rather than
/// merely absent from this list: a Project has one portal, so there is no
/// application name for an operator to register.
#[test]
fn projects_help_lists_the_project_workspace_verbs() {
    assert_eq!(
        command_names(&help(&["projects", "--help"])),
        vec!["doctor", "repository", "help"]
    );
    assert_eq!(
        command_names(&help(&["projects", "repository", "--help"])),
        vec!["scaffold", "validate", "help"]
    );
}

/// Two commands are spelled `doctor` and they diagnose different things.
/// `ops doctor` reads a running Kubernetes namespace; `projects doctor` reads
/// deployment workspace coordinates and this machine. Neither help text may
/// leave an operator guessing which one they want.
#[test]
fn the_two_doctors_say_which_one_they_are() {
    let projects = unwrapped(&help(&["projects", "doctor", "--help"]));
    assert!(
        projects.contains("read-only"),
        "projects doctor must state it writes nothing: {projects}"
    );
    assert!(
        projects.contains("ops doctor"),
        "projects doctor must disambiguate itself from ops doctor: {projects}"
    );

    let ops = unwrapped(&help(&["ops", "doctor", "--help"]));
    assert!(
        ops.contains("scheduled-job") || ops.contains("CronJob"),
        "ops doctor must name the cluster health it reports: {ops}"
    );
}

/// `docs` becomes a `dev` member once `erd` moves to `db`: what is left are
/// the developer/agent reference helpers, needing no cluster and no database.
#[test]
fn dev_docs_keeps_only_the_reference_helpers() {
    assert_eq!(
        command_names(&help(&["dev", "docs", "--help"])),
        vec!["list", "glossary", "help"]
    );
}

#[test]
fn dev_help_lists_local_loop_members() {
    let output = help(&["dev", "--help"]);

    assert_eq!(
        command_names(&output),
        vec![
            "install",
            "up",
            "down",
            "env",
            "status",
            "worker-reload",
            "build-webapp",
            "staging",
            "kind",
            "worktree-env",
            "deploy",
            "undeploy",
            "e2e",
            "garage-bootstrap",
            "grant-lawyer",
            "sample-project",
            "browser-e2e",
            "logs",
            "kustomize",
            "sendgrid-openapi",
            // The developer/agent reference helpers, left after `erd` moved
            // to `db`. No cluster, no database.
            "docs",
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
    // The rotation contract belongs in the help text. An operator who reads
    // only this must still learn that re-encrypting the file revokes nothing.
    assert!(apply.contains("rotating it at the provider first"));
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
    assert!(output.contains("deployments/<name>/config.toml"));
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
    assert!(
        output.contains("no KMS grant, no credential, and no network"),
        "the help must say what it does NOT need, or nobody will run it in CI: {output}"
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
    assert!(
        output.contains("Refuses when the rendered manifests differ from the cluster"),
        "the help must state the refusal, not just the flag: {output}"
    );

    // `--deployments-dir` is what lets the tree live in a checkout of its own.
    assert!(output.contains("--deployments-dir <DIR>"));
    assert!(
        output.contains("directory CONTAINING `deployments/`"),
        "the help must resolve the tree-or-parent ambiguity: {output}"
    );
    assert!(
        output.contains("NAVIGATOR_DEPLOYMENTS_DIR"),
        "the help must name the environment fallback: {output}"
    );

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
        vec!["up", "down", "status", "sweep", "help"]
    );
}

#[test]
fn worktree_env_sweep_help_states_the_dry_run_default_and_its_guards() {
    let output = unwrapped(&help(&["dev", "worktree-env", "sweep", "--help"]));

    // The safety contract belongs in the help text: an operator reaching for
    // a delete command must be able to see, without reading the source, that
    // it defaults to a dry run and what it refuses to touch.
    assert!(output.contains("changes nothing unless `--apply` is given"));
    assert!(output.contains("Never selects the shared `dev up` cluster"));
    assert!(output.contains("never prunes Docker volumes"));
    assert!(output.contains("Without it, `sweep` is a dry run"));
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
            "login", "logout", "whoami", "mcp", "projects", "intake", "notation", "retainer",
            "project", "help",
        ]
    );
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
            "github",
            "ship",
            "surreal-archive",
            "deployments",
            "secrets",
            "gcp",
            "restate",
            "doctor",
            "dns",
            "rebrand",
            "observability",
            // Operator blast radius that is not cluster lifecycle: the two
            // distribution pipelines and the release-packaging steps that
            // regenerate the licence notices and stamp the release version into
            // the manifest.
            //
            // `release-version` writes `[workspace.package].version`, which is
            // the act that cuts a release; `release-check` is what decides
            // whether a given commit's version IS one, and `deploy.yml` runs it
            // on every push to `main`. Together they replaced the pushed tag:
            // `release-provenance` proved a tag came from `main`, which a push to
            // `main` now asserts by construction.
            "lsp",
            "assets",
            "release-version",
            "release-check",
            "notices",
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
            // The three provisioners, widest blast radius first: an
            // environment, then the shared registry, then the static sites.
            "setup",
            "hub",
            "marketing", // Post-provisioning operations.
            "iap",
            "help",
        ]
    );
}

/// `marketing` provisions world-readable buckets. The environment provisioner
/// provisions private client documents. The only thing separating the two at
/// the terminal is which word the operator types, so the help has to say what
/// this one refuses to build.
#[test]
fn ops_gcp_marketing_setup_help_states_what_it_will_not_create() {
    let output = unwrapped(&help(&["ops", "gcp", "marketing", "--help"]));

    assert!(output.contains("never creates GKE, private document storage"));
    assert!(output.contains("The marketing project is not an environment"));
}

/// Certificate issuance is blocked until DNS resolves to the printed address,
/// so an operator who reads only the help still learns that DNS is their next
/// step and that this command will not do it for them.
#[test]
fn ops_gcp_marketing_setup_help_names_dns_as_the_operator_s_next_step() {
    let output = unwrapped(&help(&["ops", "gcp", "marketing", "setup", "--help"]));

    assert!(output.contains("Managed certificates cannot finish issuing"));
    assert!(output.contains("not something this command performs"));
}

/// The hub provisions a registry and an identity; the environment provisioner
/// provisions buckets and a cluster. Its help must say so, because the only
/// thing separating the two commands at the terminal is which one the operator
/// types.
#[test]
fn ops_gcp_hub_setup_help_states_what_it_will_not_create() {
    let output = unwrapped(&help(&["ops", "gcp", "hub", "--help"]));

    assert!(output.contains("never creates buckets, GKE, or IAP"));
    assert!(output.contains("The hub is not an environment"));
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
