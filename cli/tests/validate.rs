//! Integration tests for the `navigator validate <dir>` subcommand.
//!
//! These drive the compiled binary through `assert_cmd` so the test
//! exercises the real argv parsing, exit codes, and stdout the user
//! will see — not just the library it wraps.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::str;
use tempfile::TempDir;

fn write(dir: &Path, rel: &str, contents: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn navigator() -> Command {
    Command::cargo_bin("navigator").unwrap()
}

#[test]
fn validate_succeeds_on_clean_directory() {
    let dir = TempDir::new().unwrap();
    // A plain prose file classifies as Markdown (it carries none of the
    // notation/event/content markers), so it is held only to the M/S
    // rules — no N-family expectations to satisfy.
    write(dir.path(), "Notes.md", "Plain body line.\n");
    navigator()
        .args(["validate"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(str::contains(
            "Scanned 1 file(s), found 0 error(s), 0 warning(s)",
        ));
}

#[test]
fn validate_exits_nonzero_on_violations_and_prints_each_one() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "Bad.md",
        &format!("Intro.\n\n{}\n", "x".repeat(121)),
    );
    navigator()
        .args(["validate"])
        .arg(dir.path())
        .assert()
        .failure()
        .code(1)
        .stdout(str::contains("S101"))
        .stdout(str::contains("Scanned 1 file(s), found"));
}

#[test]
fn validate_default_rule_set_flags_missing_frontmatter() {
    let dir = TempDir::new().unwrap();
    // A file self-identifies as a notation template by declaring
    // `kind:` (one of the notation kinds) — not by its path or structure.
    // Once it does, the missing required keys are flagged.
    write(
        dir.path(),
        "templates/notes.md",
        "---\nkind: onboarding\nquestionnaire:\n  BEGIN:\n    _: END\n---\n\nBody.\n",
    );
    navigator()
        .args(["validate"])
        .arg(dir.path())
        .assert()
        .failure()
        .code(1)
        .stdout(str::contains("N101"))
        .stdout(str::contains("N102"));
}

#[test]
fn validate_treats_templates_path_without_machine_as_markdown() {
    let dir = TempDir::new().unwrap();
    // Classification is frontmatter-driven: a `templates/` file with no
    // notation machine is plain Markdown, not a half-declared template, so
    // it trips no N-family rules.
    write(dir.path(), "templates/notes.md", "Just a body line.\n");
    navigator()
        .args(["validate"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(str::contains(
            "Scanned 1 file(s), found 0 error(s), 0 warning(s)",
        ));
}

#[test]
fn validate_rejects_the_retired_public_template_shelf() {
    let dir = TempDir::new().unwrap();
    let source = workspace_root().join("templates/neon_law/shared/letter.md");
    let retired = dir.path().join("templates/open_source/retainer.md");
    fs::create_dir_all(retired.parent().unwrap()).unwrap();
    fs::copy(source, &retired).unwrap();

    navigator()
        .arg("validate")
        .arg(dir.path())
        .assert()
        .failure()
        .code(1)
        .stdout(str::contains("N110"))
        .stdout(str::contains("open_source"))
        .stdout(str::contains("must live under `neon_law/` or `forms/`"));
}

#[test]
fn validate_default_treats_code_only_frontmatter_as_markdown() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "server/content/marketing/service.md",
        "---\ntitle: Service\ncode: sample\n---\n\nBody.\n",
    );
    navigator()
        .args(["validate"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(str::contains(
            "Scanned 1 file(s), found 0 error(s), 0 warning(s)",
        ));
}

#[test]
fn validate_defaults_to_current_directory() {
    // Bare `validate` (no dir argument) walks `.`.
    let dir = TempDir::new().unwrap();
    write(dir.path(), "Notes.md", "Plain body line.\n");
    navigator()
        .current_dir(dir.path())
        .arg("validate")
        .assert()
        .success()
        .stdout(str::contains(
            "Scanned 1 file(s), found 0 error(s), 0 warning(s)",
        ));
}

#[test]
fn validate_now_scans_readme_and_claude_as_prose() {
    // The former default excludes are gone: README/CLAUDE are scanned like
    // any other markdown and, as clean prose, pass.
    let dir = TempDir::new().unwrap();
    write(dir.path(), "README.md", "A readme line.\n");
    write(dir.path(), "CLAUDE.md", "Agent rules line.\n");
    write(dir.path(), "Ok.md", "Plain body line.\n");
    navigator()
        .args(["validate"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(str::contains("Scanned 3 file(s)"));
}

/// Validate remains independent of database environment variables.
#[test]
fn validate_ignores_an_exported_database_url() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "Notes.md", "Plain body line.\n");
    navigator()
        .args(["validate"])
        .arg(dir.path())
        .env("DATABASE_URL", "postgres://x:y@localhost:5432/z")
        .env_remove("NAVIGATOR_SURREAL_ENDPOINT")
        .env_remove("NAVIGATOR_SURREAL_NAMESPACE")
        .env_remove("NAVIGATOR_SURREAL_DATABASE")
        .assert()
        .success()
        .stdout(str::contains(
            "Scanned 1 file(s), found 0 error(s), 0 warning(s)",
        ));
}

/// The removed store-backed flag is rejected so validation stays local.
#[test]
fn question_codes_from_store_flag_is_removed() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "Notes.md", "Plain body line.\n");
    navigator()
        .args(["validate"])
        .arg(dir.path())
        .arg("--question-codes-from-store")
        .env_remove("NAVIGATOR_SURREAL_ENDPOINT")
        .env_remove("NAVIGATOR_SURREAL_NAMESPACE")
        .env_remove("NAVIGATOR_SURREAL_DATABASE")
        .assert()
        .failure()
        .code(2)
        .stderr(str::contains(
            "unexpected argument '--question-codes-from-store'",
        ));
}

#[test]
fn validate_returns_exit_code_2_when_directory_does_not_exist() {
    navigator()
        .args(["validate", "/definitely/does/not/exist/12345"])
        .assert()
        .failure()
        .code(2)
        .stderr(str::contains("navigator:"));
}

/// The repository root, derived from this crate's manifest dir
/// (`CARGO_MANIFEST_DIR` points at `cli/`; the workspace is one up).
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("workspace root exists")
}

/// CI guard: every shipped example notation under `templates/` must pass
/// the *classified* (default-mode) validator with zero blocking errors.
///
/// Files under `templates/` classify from their declared `kind:`: the legal
/// shelves run the full N-family (N101–N108) plus the markdown rules, and
/// `templates/github/` runs the questionnaire subset plus `N119`. It is the
/// enforcement the prompt asks for — running
/// inside `cargo test --workspace`, it fails CI the moment a template (or
/// a newly added one) drifts out of conformance. Keep the example
/// notations conforming; do not loosen this test to make a bad template
/// pass.
///
/// Yellow `N112` "not built yet" advisories (every template's
/// `lawyer_review` gate earns one today) are warnings, not errors, so
/// they are expected and do not fail the gate — assert on `0 error(s)`.
#[test]
fn every_template_notation_passes_classified_validation() {
    let templates = workspace_root().join("templates");
    assert!(
        templates.is_dir(),
        "templates/ directory must exist at {}",
        templates.display(),
    );
    navigator()
        .arg("validate")
        .arg(&templates)
        .assert()
        .success()
        .stdout(str::contains("found 0 error(s)"));
}

/// Every classified corpus file must declare a `kind:` — the guard that
/// keeps classification honest now that [`rules::classify_source`] reads
/// the declared kind and nothing else (no questionnaire/workflow/path
/// inference). If a template or content page forgets its `kind:`, it
/// classifies as plain [`rules::DocumentKind::Markdown`] and silently
/// skips its whole rule family. This walks the real content roots and
/// fails the build the moment one drifts — so inference can never quietly
/// return through the side door of an undeclared file.
///
/// `README.md` is excluded by name from the linter (it is never notation),
/// so the workshop README — which the loader still serves as a workshop —
/// is not required to declare a kind here.
#[test]
fn every_classified_corpus_file_declares_a_kind() {
    use rules::DocumentKind;

    // (content root, the kind every non-README file under it must resolve
    // to). Matched longest-prefix-first, so a nested root overrides its
    // parent: `templates/github` is the engineering intake shelf and
    // classifies as `Github`, while every other file under `templates`
    // is a legal notation.
    let roots: &[(&str, DocumentKind)] = &[
        ("templates", DocumentKind::NotationTemplate),
        ("templates/github", DocumentKind::Github),
        ("server/content/blog", DocumentKind::BlogPost),
        ("server/content/workshops", DocumentKind::Workshop),
    ];
    let root = workspace_root();
    let expected_for = |path: &Path| -> DocumentKind {
        roots
            .iter()
            .filter(|(rel, _)| path.starts_with(root.join(rel)))
            .max_by_key(|(rel, _)| rel.len())
            .map(|(_, kind)| *kind)
            .expect("every walked file lives under one of the roots")
    };
    let mut checked = 0usize;
    for (rel, _) in roots {
        let dir = root.join(rel);
        assert!(dir.is_dir(), "content root missing: {}", dir.display());
        for entry in walkdir::WalkDir::new(&dir) {
            let entry = entry.unwrap();
            let path = entry.path();
            let is_md = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("md"));
            if !is_md {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            // READMEs are excluded from linting by name.
            if name.eq_ignore_ascii_case("README.md") {
                continue;
            }
            let contents = fs::read_to_string(path).unwrap();
            let file = rules::SourceFile {
                path: path.to_path_buf(),
                contents,
            };
            let expected = expected_for(path);
            let actual = rules::classify_source(&file);
            assert_eq!(
                actual,
                expected,
                "{} must declare a `kind:` that classifies as {expected:?} (got {actual:?}); \
                 an undeclared file silently lints as prose Markdown",
                path.display(),
            );
            checked += 1;
        }
    }
    // A loose floor, not an exact count: it exists so a walker that silently
    // finds nothing fails loudly. The slim catalog (public forms plus two
    // sample letters, plus GitHub intake, blog, and workshops) is 21 files
    // because the nested `templates/github` root is walked twice.
    assert!(
        checked >= 21,
        "expected to check the whole corpus, only saw {checked} files",
    );
}

/// `validate` parses standalone `.yaml`/`.yml` in the same walk and reports
/// the count.
#[test]
fn validate_parses_yaml_and_yml_files() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "config.yaml",
        "name: navigator\nitems:\n  - one\n",
    );
    write(dir.path(), "nested/multi.yml", "---\na: 1\n---\nb: 2\n");
    write(dir.path(), "notes.txt", "not: [valid\n");
    navigator()
        .arg("validate")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(str::contains("Parsed 2 YAML file(s), found 0 error(s)"));
}

#[test]
fn validate_rejects_yaml_parse_errors() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "bad.yaml", "root:\n  - ok\n  - [broken\n");
    navigator()
        .arg("validate")
        .arg(dir.path())
        .assert()
        .failure()
        .code(1)
        .stdout(str::contains("Parsed 1 YAML file(s), found 1 error(s)"))
        .stderr(str::contains("bad.yaml"))
        .stderr(str::contains("YAML parse error"));
}

#[test]
fn validate_checks_seed_documents_before_any_deployment_write() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "seeds/Person.yaml",
        "lookup_fields:\n  - email\nrecords:\n  - email: person@example.com\n    name: Person\n",
    );
    write(
        dir.path(),
        "seeds/Entity.yaml",
        "lookup_fields:\n  - name\n  - entity_type_id\nrecords: []\n",
    );
    navigator()
        .arg("validate")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(str::contains(
            "Validated 2 seed document(s), found 0 error(s)",
        ));
}

#[test]
fn validate_ignores_unsupported_canonical_seed_catalogs() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "store/seeds/Question.yaml",
        "lookup_fields: []\nrecords: []\n",
    );
    write(
        dir.path(),
        "store/seeds/Person.yaml",
        "lookup_fields:\n  - email\nrecords: []\n",
    );
    navigator()
        .arg("validate")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(str::contains(
            "Validated 1 seed document(s), found 0 error(s)",
        ));
}

#[test]
fn validate_refuses_invalid_seed_documents() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "seeds/Person.yaml",
        "lookup_fields:\n  - email\nrecords:\n  - email: person@example.com\n    display_name: Person\n",
    );
    write(
        dir.path(),
        "seeds/Notation.yaml",
        "lookup_fields: []\nrecords: []\n",
    );
    navigator()
        .arg("validate")
        .arg(dir.path())
        .assert()
        .failure()
        .code(1)
        .stdout(str::contains("Y001"))
        .stdout(str::contains("unknown field `display_name`"))
        .stdout(str::contains("unsupported seed model `Notation`"))
        .stdout(str::contains(
            "Validated 2 seed document(s), found 2 error(s)",
        ));
}

/// The consumed-mutable-tag guard (navigator#540) fires on every consume
/// site: a YAML `image:` value, a Containerfile `FROM`, and a workflow
/// installer step's `version: latest`. Each is a way production could change
/// with no commit; the gate must fail so the diff has to pin them.
#[test]
fn validate_flags_consumed_mutable_tags_at_every_site() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "k8s/pod.yaml",
        "spec:\n  containers:\n    - name: policy\n      image: example/policy:latest\n",
    );
    write(
        dir.path(),
        "images/Containerfile.demo",
        "FROM debian:latest AS runtime\n",
    );
    write(
        dir.path(),
        ".github/workflows/ci.yml",
        "jobs:\n  a:\n    steps:\n      - name: install\n        with:\n          version: latest\n",
    );
    navigator()
        .arg("validate")
        .arg(dir.path())
        .assert()
        .failure()
        .code(1)
        .stdout(str::contains("found 3 mutable tag(s)"))
        .stderr(str::contains("example/policy:latest"))
        .stderr(str::contains("debian:latest"))
        .stderr(str::contains("consumed mutable binary version `latest`"));
}

/// The guard also catches a `latest-<arch>` variant and an implicit latest (an
/// untagged reference) in an on-cluster manifest — both are the `latest` family.
#[test]
fn validate_flags_latest_variant_and_implicit_latest_in_manifest() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "k8s/a.yaml",
        "spec:\n  containers:\n    - name: c\n      image: example/tool:latest-amd64\n",
    );
    write(
        dir.path(),
        "k8s/b.yaml",
        "spec:\n  containers:\n    - name: c\n      image: example/untagged\n",
    );
    navigator()
        .arg("validate")
        .arg(dir.path())
        .assert()
        .failure()
        .code(1)
        .stdout(str::contains("found 2 mutable tag(s)"))
        .stderr(str::contains("latest-amd64"))
        .stderr(str::contains("implicit"));
}

/// References that are properly pinned — an explicit version tag, a digest, our
/// own `:dev`/`:YY.M.D` build tags — pass, and a `# pin-exempt:` comment is the
/// documented escape hatch for an intentional case.
#[test]
fn validate_accepts_pinned_digest_and_exempt_references() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "k8s/ok.yaml",
        "spec:\n  containers:\n\
         \x20   - name: a\n      image: example/policy:1.18.2\n\
         \x20   - name: b\n      image: postgres:16-alpine\n\
         \x20   - name: c\n      image: navigator-web:dev\n\
         \x20   - name: d\n      image: us-west4-docker.pkg.dev/x/y/z:YY.M.D\n\
         \x20   - name: e\n      image: repo/img@sha256:abc123\n\
         \x20   - name: f\n      image: publisher/tool:latest # pin-exempt: publish-only pointer\n",
    );
    navigator()
        .arg("validate")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(str::contains("found 0 mutable tag(s)"));
}

/// A bare, untagged `- image: navigator-web` in a workflow file names a
/// build-matrix target we *publish*, not a container we *run*, so it is not an
/// implicit-latest consume site — the guard leaves it alone. The identical line
/// in an on-cluster manifest IS flagged, proving the distinction is by file
/// role, not by luck.
#[test]
fn validate_ignores_bare_matrix_image_in_workflow_but_flags_it_in_manifest() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        ".github/workflows/deploy.yml",
        "jobs:\n  build:\n    strategy:\n      matrix:\n        include:\n          - image: navigator-web\n",
    );
    navigator()
        .arg("validate")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(str::contains("found 0 mutable tag(s)"));

    let dir2 = TempDir::new().unwrap();
    write(
        dir2.path(),
        "k8s/pod.yaml",
        "spec:\n  containers:\n    - image: navigator-web\n",
    );
    navigator()
        .arg("validate")
        .arg(dir2.path())
        .assert()
        .failure()
        .code(1)
        .stdout(str::contains("found 1 mutable tag(s)"));
}

/// A workflow `container:` / `services:` image is a runtime image we *consume*,
/// unlike the build-matrix `- image:` list item — so a plain untagged
/// `image: ubuntu` (implicit latest) there IS flagged, while the matrix list
/// item is not. Guards against a bypass where a real runtime image hides behind
/// the matrix exemption.
#[test]
fn validate_flags_plain_runtime_image_in_workflow_not_matrix_list() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        ".github/workflows/ci.yml",
        "jobs:\n  test:\n    container:\n      image: ubuntu\n    strategy:\n      matrix:\n        include:\n          - image: navigator-web\n",
    );
    navigator()
        .arg("validate")
        .arg(dir.path())
        .assert()
        .failure()
        .code(1)
        .stdout(str::contains("found 1 mutable tag(s)"))
        .stderr(str::contains("`ubuntu`"))
        .stderr(str::contains("implicit"));
}

/// The guard cannot be bypassed by valid-but-unusual formatting: a
/// case-varied Dockerfile `From` (the instruction is case-insensitive) and a
/// YAML `image :` with a space before the colon are both still caught.
#[test]
fn validate_catches_mixed_case_from_and_spaced_yaml_key() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "images/Containerfile.mc",
        "From debian:latest AS runtime\n",
    );
    write(
        dir.path(),
        "k8s/spaced.yaml",
        "spec:\n  containers:\n    - name: c\n      image : example/policy:latest\n",
    );
    navigator()
        .arg("validate")
        .arg(dir.path())
        .assert()
        .failure()
        .code(1)
        .stdout(str::contains("found 2 mutable tag(s)"))
        .stderr(str::contains("debian:latest"))
        .stderr(str::contains("example/policy:latest"));
}

/// A lookalike key must not be mistaken for `image:`: `imagePullPolicy: Always`
/// is not an image reference and is left alone.
#[test]
fn validate_ignores_image_lookalike_keys() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "k8s/pod.yaml",
        "spec:\n  containers:\n    - name: c\n      image: example/policy:1.18.2\n      imagePullPolicy: Always\n",
    );
    navigator()
        .arg("validate")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(str::contains("found 0 mutable tag(s)"));
}

/// CI guard: the real infrastructure tree — every k8s manifest, Containerfile,
/// and GitHub workflow — carries no consumed mutable tag. This is the
/// enforcement navigator#540 asks for: a `latest` reintroduced anywhere under
/// these roots fails `cargo nextest`, so the pin cannot silently regress.
#[test]
fn workspace_infra_tree_has_no_consumed_mutable_tags() {
    for root in ["k8s", "examples", "images", ".github"] {
        let dir = workspace_root().join(root);
        assert!(dir.is_dir(), "infra root missing: {}", dir.display());
        navigator()
            .arg("validate")
            .arg(&dir)
            .assert()
            .stdout(str::contains("found 0 mutable tag(s)"));
    }
}

/// The collapsed commands are gone from the CLI surface: invoking one now
/// fails with clap's unknown-subcommand error.
#[test]
fn removed_validate_subcommands_error() {
    for removed in ["validate-events", "validate-yaml", "validate-i18n"] {
        navigator()
            .arg(removed)
            .assert()
            .failure()
            .stderr(str::contains("unrecognized subcommand"));
    }
}

#[test]
fn missing_subcommand_prints_usage_and_fails() {
    navigator()
        .assert()
        .failure()
        .stderr(str::contains("Usage:"));
}

#[test]
fn validate_fix_writes_back_autofixable_edits_and_reports_remaining() {
    let dir = TempDir::new().unwrap();
    // Three trailing spaces (M009 violates — two-space hard break is
    // exempt, three is not) + a hard tab (M010). Both autofixable.
    write(
        dir.path(),
        "Mixed.md",
        "Body line with trailing spaces   \nTabbed\there\n",
    );
    navigator()
        .args(["validate", "--fix"])
        .arg(dir.path())
        .assert()
        .stdout(str::contains("fixed"))
        .stdout(str::contains("Fixed 1 file(s)"));
    let after = fs::read_to_string(dir.path().join("Mixed.md")).unwrap();
    assert_eq!(
        after, "Body line with trailing spaces\nTabbed  here\n",
        "expected M009 + M010 autofixes; got: {after:?}",
    );
}

#[test]
fn validate_fix_leaves_diagnostic_only_violations_for_human() {
    let dir = TempDir::new().unwrap();
    // M010 (autofixable) + N101 (diagnostic-only) in the same
    // notation-template file. The declared `kind:` marks it as a template;
    // `title` is still missing, so N101 fires.
    write(
        dir.path(),
        "templates/needs.md",
        "---\nkind: onboarding\nrespondent_type: entity\nquestionnaire:\n  BEGIN:\n    _: END\n---\n\n\tTabbed\n",
    );
    navigator()
        .args(["validate", "--fix"])
        .arg(dir.path())
        .assert()
        .failure()
        .code(1)
        .stdout(str::contains("N101"))
        .stdout(str::contains("remaining violation"));
    // The autofixable tab is gone.
    let after = fs::read_to_string(dir.path().join("templates/needs.md")).unwrap();
    assert!(
        !after.contains('\t'),
        "tab should be replaced; got: {after:?}"
    );
}

#[test]
fn validate_fix_is_idempotent() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "OnlyFixable.md", "Body  \n\tIndent\n");
    navigator()
        .args(["validate", "--fix"])
        .arg(dir.path())
        .assert()
        .success();
    // Second run finds nothing to fix.
    navigator()
        .args(["validate", "--fix"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(str::contains("Fixed 0 file(s)"));
}
