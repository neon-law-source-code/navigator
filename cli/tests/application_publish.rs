//! Pin how `.github/actions/application-publish` moves the bytes.
//!
//! The action uploads a Project's built portal into the shared, private
//! applications bucket. Two properties of *how* it uploads are load-bearing and
//! neither is visible from reading the step name:
//!
//! 1. **It must overwrite unconditionally.** The bucket carries an object-age
//!    Delete rule ([`APPLICATIONS_RETENTION_DAYS`] in
//!    `cli::devx::gcp::buckets`). An upload that skips unchanged objects leaves
//!    a live asset's age running while `index.html` keeps naming it, so the
//!    entry document outlives the assets it points at. `gcloud storage rsync`
//!    skips unchanged objects by definition, which is why it is forbidden here.
//! 2. **It must merge, not nest.** `gcloud storage cp --recursive <dir> <dst>`
//!    writes `<dst>/<dir>/...` — a trailing slash on the source does not change
//!    that — so uploading `portal/dist` would publish
//!    `<code>/portal/dist/assets/...`, a path Navigator does not serve. The
//!    action uploads the directory's *entries* instead.
//!
//! Unlike `project_gate.rs`, which asserts presence in source because executing
//! that gate would need a runner, the upload step here *is* executed: it is a
//! self-contained bash block whose only outside call is `gcloud`. Stubbing
//! `gcloud` and reading back its argv tests the real thing, and it is the only
//! way to catch the nesting trap — a source-text assertion cannot tell
//! `cp -r dist dst` from `cp -r dist/* dst`.
//!
//! That stub is why the whole crate is Unix-only. It is a `#!/usr/bin/env bash`
//! script made executable through a Unix mode bit and resolved off a
//! `:`-separated `PATH`, and the step it drives is run under `bash`. None of
//! those three has a Windows equivalent, so gating only the import would leave
//! the three stub-driven tests compiling on Windows and failing at run time,
//! and would make the three stub helpers dead code under `-D warnings`. Linux
//! and macOS compile and run every test in this file unchanged.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The workspace root (`CARGO_MANIFEST_DIR` points at `cli/`).
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("workspace root exists")
}

fn action_path() -> PathBuf {
    workspace_root().join(".github/actions/application-publish/action.yml")
}

fn action_source() -> String {
    let path = action_path();
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The composite's steps, in declaration order.
fn steps() -> Vec<serde_yaml::Mapping> {
    let action: serde_yaml::Value =
        serde_yaml::from_str(&action_source()).expect("action.yml parses as YAML");
    action
        .get("runs")
        .and_then(|r| r.get("steps"))
        .and_then(|s| s.as_sequence())
        .expect("the composite declares steps")
        .iter()
        .map(|s| s.as_mapping().expect("each step is a mapping").clone())
        .collect()
}

/// The index of the one step whose `name` starts with `prefix`.
fn step_index(prefix: &str) -> usize {
    let steps = steps();
    let matches: Vec<usize> = steps
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            s.get(serde_yaml::Value::from("name"))
                .and_then(|n| n.as_str())
                .is_some_and(|n| n.starts_with(prefix))
        })
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "expected exactly one step named `{prefix}...`, found {}",
        matches.len()
    );
    matches[0]
}

/// The `run:` script of the step named `prefix...`.
fn step_script(prefix: &str) -> String {
    steps()[step_index(prefix)]
        .get(serde_yaml::Value::from("run"))
        .and_then(|r| r.as_str())
        .expect("the step runs a script")
        .to_string()
}

/// Run one of the action's bash steps with `gcloud` stubbed, and return the
/// argv the step passed to it.
///
/// `dist` names the files to create under `<workdir>/portal/dist`. The stub
/// writes its argv one-per-line, so a caller can assert the exact command
/// without depending on the real `gcloud` being installed or authenticated.
fn run_step_with_gcloud_stub(script: &str, dist: &[&str]) -> Result<Vec<String>, String> {
    let tmp = tempfile::tempdir().expect("temp dir");
    let root = tmp.path();

    for relative in dist {
        let file = root.join("portal/dist").join(relative);
        fs::create_dir_all(file.parent().expect("file has a parent")).expect("create dist dirs");
        fs::write(&file, b"x").expect("write dist file");
    }

    let bin = root.join("bin");
    fs::create_dir_all(&bin).expect("create stub bin");
    let argv_log = root.join("argv.txt");
    let stub = bin.join("gcloud");
    fs::write(
        &stub,
        format!(
            "#!/usr/bin/env bash\nprintf '%s\\n' \"$@\" > {}\n",
            argv_log.display()
        ),
    )
    .expect("write gcloud stub");
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).expect("chmod stub");

    let script_path = root.join("step.sh");
    fs::write(&script_path, script).expect("write step script");

    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = Command::new("bash")
        .arg(&script_path)
        .current_dir(root)
        .env("PATH", path)
        .env("BUCKET", "a-deployment-applications")
        .env("PREFIX", "acme/portal")
        .env("DIST_DIR", "portal/dist")
        .output()
        .expect("run the step under bash");

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stdout).to_string()
            + &String::from_utf8_lossy(&output.stderr));
    }

    Ok(read_argv(&argv_log))
}

fn read_argv(log: &Path) -> Vec<String> {
    fs::read_to_string(log)
        .expect("the step invoked gcloud")
        .lines()
        .map(str::to_string)
        .collect()
}

/// Run the "derive the coordinate and verify the built mount" step in a
/// fixture checkout, and return the `key=value` pairs it wrote to
/// `$GITHUB_OUTPUT`.
///
/// `manifest` is the root `navigator.yaml` content, when the fixture carries
/// one at all. `repository` is the resolved `repository:` input — the
/// fallback this step uses only when the manifest is absent or declares no
/// `project:`. `index_html` is written verbatim as `portal/dist/index.html`,
/// so a caller controls exactly which mount the "built" bundle claims.
fn run_derive_step(
    manifest: Option<&str>,
    repository: &str,
    index_html: &str,
) -> Result<std::collections::HashMap<String, String>, String> {
    let tmp = tempfile::tempdir().expect("temp dir");
    let root = tmp.path();

    if let Some(contents) = manifest {
        fs::write(root.join("navigator.yaml"), contents).expect("write manifest");
    }

    let dist_dir = root.join("portal/dist");
    fs::create_dir_all(&dist_dir).expect("create dist dir");
    fs::write(dist_dir.join("index.html"), index_html).expect("write index.html");
    fs::write(dist_dir.join("app.js"), "x").expect("write an asset");

    let script_path = root.join("step.sh");
    fs::write(&script_path, step_script("derive the coordinate")).expect("write step script");
    let output_path = root.join("github_output.txt");
    fs::write(&output_path, "").expect("create GITHUB_OUTPUT");

    let output = Command::new("bash")
        .arg(&script_path)
        .current_dir(root)
        .env("REPOSITORY", repository)
        .env("DIST_DIR", "portal/dist")
        .env("GITHUB_OUTPUT", &output_path)
        .output()
        .expect("run the step under bash");

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stdout).to_string()
            + &String::from_utf8_lossy(&output.stderr));
    }

    Ok(fs::read_to_string(&output_path)
        .expect("read GITHUB_OUTPUT")
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect())
}

/// Split an argv into its `--flags` and its positional operands.
///
/// Asserted on rather than fixed indices because the upload carries quieting
/// flags whose order relative to the operands is not meaningful. Pinning
/// positions made adding one a test failure that looked like a regression in the
/// nesting behaviour it was actually guarding.
fn flags_and_operands(argv: &[String]) -> (Vec<&str>, Vec<&str>) {
    let (flags, operands): (Vec<&String>, Vec<&String>) =
        argv.iter().partition(|a| a.starts_with("--"));
    (
        flags.into_iter().map(String::as_str).collect(),
        operands.into_iter().map(String::as_str).collect(),
    )
}

/// The publish never uses `gcloud storage rsync`.
///
/// This is the whole of ENG-273 in one assertion, and it guards two independent
/// failures at once. `rsync` skips unchanged objects, which lets a live asset
/// age past the bucket's Delete rule while `index.html` still names it; and
/// `rsync` compares against the destination, so it needs
/// `storage.objects.list`, a permission evaluated against the *bucket* that no
/// prefix condition can scope. A comment is allowed to name `rsync` — the
/// action explains at length why it is not used — so only executable lines are
/// searched.
#[test]
fn the_publish_never_rsyncs() {
    for (number, line) in action_source().lines().enumerate() {
        let code = line.trim_start();
        if code.starts_with('#') {
            continue;
        }
        assert!(
            !code.contains("storage rsync"),
            "line {} publishes with `gcloud storage rsync`; it skips unchanged \
             objects, so a live asset ages out under the bucket's Delete rule: {code}",
            number + 1,
        );
    }
}

/// The action tells the next reader why, naming the constant that decides it.
///
/// `rsync` is the faster call, so "optimizing" back to it is the obvious change
/// to make. Someone who does should first hit the sentence explaining that
/// unconditional overwrite is a correctness requirement of the bucket's
/// retention policy, and be able to follow the name to the rule itself.
#[test]
fn the_action_explains_the_retention_coupling() {
    let source = action_source();
    assert!(
        source.contains("APPLICATIONS_RETENTION_DAYS"),
        "the action must name the retention constant its upload is coupled to",
    );
    assert!(
        source.contains("cli/src/devx/gcp/buckets.rs"),
        "the action must point at the file defining that rule",
    );
}

/// Pass 1 uploads the dist directory's *entries*, never the directory.
///
/// The nesting trap: `cp --recursive portal/dist gs://b/acme/portal/` writes
/// `acme/portal/dist/assets/...`, which Navigator does not serve, and a trailing
/// slash on the source does not change it. Uploading each entry instead makes
/// `assets/` land at `acme/portal/assets/`. Asserted on the argv the step
/// actually builds, because the two spellings differ by three characters and
/// read identically.
#[test]
fn pass_one_uploads_entries_so_objects_are_not_nested_under_dist() {
    let argv = run_step_with_gcloud_stub(
        &step_script("publish assets"),
        &[
            "index.html",
            "assets/index-ABC.js",
            "assets/index-DEF.css",
            "documents/engagement.pdf",
            "pdf.worker.mjs",
        ],
    )
    .expect("the publish step succeeds");

    let (flags, operands) = flags_and_operands(&argv);
    assert_eq!(
        operands.first().copied(),
        Some("storage"),
        "pass 1 must call `gcloud storage`, got {argv:?}",
    );
    assert_eq!(
        operands.get(1).copied(),
        Some("cp"),
        "pass 1 must upload with `cp`, never `rsync`, got {argv:?}",
    );
    assert!(
        flags.contains(&"--recursive"),
        "pass 1 must upload directories recursively, got {argv:?}",
    );

    let (sources, destination) = operands[2..].split_at(operands.len() - 3);
    assert_eq!(
        destination,
        ["gs://a-deployment-applications/acme/portal/"],
        "pass 1 must upload into the Project's own `<code>/portal/` prefix",
    );

    let mut sources: Vec<&str> = sources.to_vec();
    sources.sort_unstable();
    assert_eq!(
        sources,
        [
            "portal/dist/assets",
            "portal/dist/documents",
            "portal/dist/pdf.worker.mjs",
        ],
        "pass 1 must pass the dist directory's entries, not the directory \
         itself — `cp --recursive portal/dist` nests every object under \
         `acme/portal/dist/`",
    );
}

/// Pass 1 holds `index.html` back.
///
/// `gcloud storage cp` has no `--exclude`, so the exclusion `rsync` expressed as
/// a flag is now a filter over the entry list. If it regressed, `index.html`
/// would publish in the same pass as the assets it names rather than after
/// them, and a reader arriving mid-publish could load an entry document naming
/// a hashed asset that does not exist yet.
#[test]
fn pass_one_holds_index_html_back() {
    let argv = run_step_with_gcloud_stub(
        &step_script("publish assets"),
        &["index.html", "assets/index-ABC.js"],
    )
    .expect("the publish step succeeds");

    assert!(
        !argv.iter().any(|a| a.ends_with("index.html")),
        "pass 1 uploaded index.html; it belongs in pass 2, after its assets: {argv:?}",
    );
}

/// `index.html` is published last, in its own pass, and overwritten with `cp`.
///
/// The ordering is what makes a mid-publish read safe: every asset the entry
/// document names is already readable before the document that names it is.
#[test]
fn index_html_publishes_after_the_assets_it_names() {
    assert!(
        step_index("publish assets") < step_index("publish index.html"),
        "index.html must publish after the assets pass",
    );
    let script = step_script("publish index.html");
    assert!(
        script.contains("gcloud storage cp"),
        "index.html must be overwritten with `cp` on every publish, so its age \
         restarts along with the assets': {script}",
    );
}

/// A build that produced no assets is refused rather than published.
///
/// With `rsync`'s `--exclude` gone, an empty entry list would otherwise reach
/// `gcloud` as a `cp` with no sources. That fails obscurely at best, and the
/// real defect is upstream: a `dist/` holding only `index.html` means the build
/// emitted nothing, and publishing it would replace a working portal's entry
/// document with one naming assets that were never uploaded.
#[test]
fn a_dist_holding_only_index_html_is_refused() {
    let error = run_step_with_gcloud_stub(&step_script("publish assets"), &["index.html"])
        .expect_err("a dist with no assets must fail the publish");
    assert!(
        error.contains("holds nothing but index.html"),
        "the refusal must say why the build is unpublishable, got: {error}",
    );
}

/// No line the action prints carries the deployment's bucket name.
///
/// The Project repositories consuming this action are public and so are their
/// Actions logs, and the bucket is named `<deployment>-applications`, so echoing
/// it publishes the deployment. `${BUCKET}` inside a `gs://` URL handed to
/// `gcloud` is fine — that is the call, not the log — so only `echo` lines are
/// searched, and the convention they follow instead is the literal `<bucket>`.
///
/// Disclosure reduction, not access control: see the header of the action, and
/// do not read this test as a security boundary.
#[test]
fn no_echoed_line_prints_the_bucket() {
    for (number, line) in action_source().lines().enumerate() {
        let code = line.trim_start();
        if code.starts_with('#') || !code.starts_with("echo ") {
            continue;
        }
        assert!(
            !code.contains("${BUCKET}") && !code.contains("$BUCKET"),
            "line {} echoes the applications bucket into a public Actions log; \
             print the literal `<bucket>` instead: {code}",
            number + 1,
        );
    }
}

/// Every `gcloud` call is quieted by both levers.
///
/// `gcloud storage cp` narrates itself object by object, printing
/// `gs://<bucket>/<prefix>/...` once per file, so suppressing this action's own
/// `echo`s is not enough on its own. The two levers are independent —
/// `CLOUDSDK_CORE_VERBOSITY` drops informational output wherever in gcloud it
/// originates, `--no-user-output-enabled` suppresses the progress narration of
/// the one command — and neither subsumes the other, so both are asserted.
/// Failures stay loud: errors print at `error` verbosity and `set -euo pipefail`
/// still fails the step.
#[test]
fn every_gcloud_call_is_quiet() {
    for prefix in ["publish assets", "publish index.html", "stamp index.html"] {
        let all = steps();
        let step = &all[step_index(prefix)];
        let verbosity = step
            .get(serde_yaml::Value::from("env"))
            .and_then(|e| e.get("CLOUDSDK_CORE_VERBOSITY"))
            .and_then(|v| v.as_str());
        assert_eq!(
            verbosity,
            Some("error"),
            "step `{prefix}...` must set CLOUDSDK_CORE_VERBOSITY=error so gcloud \
             does not narrate the bucket into a public log",
        );

        let script = step_script(prefix);
        for (number, line) in script.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with('#') || !code.contains("gcloud ") {
                continue;
            }
            assert!(
                code.contains("--no-user-output-enabled"),
                "step `{prefix}...` line {} invokes gcloud without \
                 --no-user-output-enabled: {code}",
                number + 1,
            );
        }
    }
}

/// The bare project id and project number are masked, decomposed from the two
/// coordinates that carry them.
///
/// GitHub redacts a secret's exact text and not the identifiers inside it, so
/// passing the service-account email and the provider resource as secrets does
/// not redact the project id or project number that `gcloud` prints on their
/// own. That is why this step is not redundant with making them secrets, and it
/// is the part a future reader is most likely to delete as duplicated effort.
#[test]
fn the_bare_project_identifiers_are_masked_before_any_step_prints() {
    assert_eq!(
        step_index("mask the deployment coordinates"),
        0,
        "the mask step must run first; a mask registered after a value has \
         already printed does not retroactively redact it",
    );

    let script = step_script("mask the deployment coordinates");
    assert!(
        script.contains("::add-mask::"),
        "the step must register masks with the ::add-mask:: workflow command: {script}",
    );
    assert!(
        script.contains(".iam.gserviceaccount.com"),
        "the project id must be decomposed from the service-account email, \
         which is where it lives: {script}",
    );
    assert!(
        script.contains("projects/"),
        "the project number must be decomposed from the provider resource, \
         which is where it lives: {script}",
    );
}

/// The action records *why* the coordinates are secrets, in the terms that stop
/// the next reader drawing the wrong conclusion.
///
/// This is the one requirement of the change that is prose rather than
/// behaviour, and it exists because the change looks like a security fix and is
/// not one. Someone who reads it as the access control could weaken the Workload
/// Identity binding believing this compensates. It does not.
#[test]
fn the_action_records_this_as_disclosure_reduction_not_access_control() {
    let source = action_source();
    assert!(
        source.contains("disclosure reduction"),
        "the action must name what this is",
    );
    assert!(
        source.contains("Workload Identity binding") && source.contains("access control"),
        "the action must say which mechanism is the actual access control",
    );
    assert!(
        source.contains("cli/src/devx/gcp/app_publisher.rs"),
        "the action must point at where real access is granted, so a reader \
         needing to change it does not edit the masking instead",
    );
}

// ── ENG-290: the manifest is the source of truth for the Project code ─────

/// The manifest wins even when the `repository:` fallback names a different,
/// otherwise-plausible Project — the settled decision this action now
/// implements, not merely tolerates.
#[test]
fn the_manifests_declared_project_wins_over_the_repository_fallback() {
    let outputs = run_derive_step(
        Some("host: www.example.com\nproject: acme\n"),
        "sample-litigation",
        "<script type=\"module\" src=\"/app/projects/acme/portal/assets/app.js\"></script>",
    )
    .expect("the step succeeds when the manifest matches the built mount");

    assert_eq!(outputs.get("code").map(String::as_str), Some("acme"));
    assert_eq!(
        outputs.get("prefix").map(String::as_str),
        Some("acme/portal")
    );
}

/// A checkout with no manifest yet falls back to `repository:`, so a
/// not-yet-migrated caller keeps publishing exactly as before.
#[test]
fn the_repository_input_is_the_fallback_when_no_manifest_is_present() {
    let outputs = run_derive_step(
        None,
        "sample-litigation",
        "<script type=\"module\" src=\"/app/projects/sample-litigation/portal/assets/app.js\"></script>",
    )
    .expect("the step succeeds using the repository fallback");

    assert_eq!(
        outputs.get("code").map(String::as_str),
        Some("sample-litigation")
    );
}

/// A manifest carrying keys this step does not need — `host:`, the one a
/// Project repository's own manifest adds — does not block reading `project:`.
#[test]
fn an_unknown_manifest_key_does_not_block_the_derived_code() {
    let outputs = run_derive_step(
        Some("project: acme\nno_live_row: the matter closed\n"),
        "sample-litigation",
        "<script type=\"module\" src=\"/app/projects/acme/portal/assets/app.js\"></script>",
    )
    .expect("an unrelated manifest key must not fail the derive step");

    assert_eq!(outputs.get("code").map(String::as_str), Some("acme"));
}

/// The mount check still refuses a manifest whose declared code does not match
/// the bundle actually built — the property that keeps a malformed or wrong
/// declaration from silently publishing under someone else's prefix.
#[test]
fn a_manifest_naming_an_unbuilt_mount_is_still_refused() {
    let error = run_derive_step(
        Some("project: acme\n"),
        "acme",
        "<script type=\"module\" src=\"/app/projects/henderson/portal/assets/app.js\"></script>",
    )
    .expect_err("a declared code the build was not mounted under must fail");

    assert!(
        error.contains("is not mounted at"),
        "the refusal must name the mount mismatch: {error}"
    );
}
