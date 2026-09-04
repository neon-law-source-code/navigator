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
fn validate_marks_each_diagnostic_with_its_severity() {
    let dir = TempDir::new().unwrap();
    let warning_source =
        workspace_root().join("templates/notations/neon_law/shared/onboarding_letter.md");
    let warning_path = dir
        .path()
        .join("templates/notations/neon_law/shared/onboarding_letter.md");
    fs::create_dir_all(warning_path.parent().unwrap()).unwrap();
    fs::copy(warning_source, warning_path).unwrap();
    write(
        dir.path(),
        "Bad.md",
        &format!("Intro.\n\n{}\n", "x".repeat(121)),
    );

    navigator()
        .arg("validate")
        .arg(dir.path())
        .assert()
        .failure()
        .code(1)
        .stdout(str::is_match(r"(?m)^error: .*S101:").unwrap())
        .stdout(str::is_match(r"(?m)^warning: .*N112:").unwrap());
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
    let source = workspace_root().join("templates/notations/neon_law/shared/onboarding_letter.md");
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
        .stdout(str::contains(
            "must live under `notations/neon_law/` or `notations/forms/`",
        ));
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

#[test]
fn validate_accepts_a_typed_english_locale_catalog() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "locales/en/home.yaml",
        "head_title: \"{site_name} | Home\"\n\
         meta_description: Everyone deserves to be seen.\n\
         heading: Everyone deserves to be seen.\n\
         lead: We fight for people.\n\
         contact_label: Contact us\n",
    );
    navigator()
        .arg("validate")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(str::contains(
            "Validated 1 locale catalog(s), found 0 error(s)",
        ));
}

#[test]
fn validate_refuses_an_incomplete_locale_catalog() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "locales/en/home.yaml", "heading: Hello\n");
    navigator()
        .arg("validate")
        .arg(dir.path())
        .assert()
        .failure()
        .code(1)
        .stdout(str::contains("Y002"))
        .stdout(str::contains(
            "Validated 1 locale catalog(s), found 1 error(s)",
        ));
}

#[test]
fn validate_refuses_a_locale_directory_other_than_english() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "locales/xx/home.yaml",
        "head_title: \"{site_name} | Home\"\n\
         meta_description: Everyone deserves to be seen.\n\
         heading: Everyone deserves to be seen.\n\
         lead: We fight for people.\n\
         contact_label: Contact us\n",
    );
    navigator()
        .arg("validate")
        .arg(dir.path())
        .assert()
        .failure()
        .code(1)
        .stdout(str::contains("Y002"))
        .stdout(str::contains("only `en` is allowed"));
}

#[test]
fn validate_refuses_an_unknown_locale_page_stem() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "locales/en/about.yaml", "title: About\n");
    navigator()
        .arg("validate")
        .arg(dir.path())
        .assert()
        .failure()
        .code(1)
        .stdout(str::contains("Y002"))
        .stdout(str::contains("unknown locale page `about`"));
}

#[test]
fn validate_refuses_an_unknown_brand_catalog_directory() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "locales/en/not-a-brand/home.yaml",
        "head_title: \"{site_name} | Home\"\n\
         meta_description: Everyone deserves to be seen.\n\
         heading: Everyone deserves to be seen.\n\
         lead: We fight for people.\n\
         contact_label: Contact us\n",
    );
    navigator()
        .arg("validate")
        .arg(dir.path())
        .assert()
        .failure()
        .code(1)
        .stdout(str::contains("Y002"))
        .stdout(str::contains("not a registry key"));
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

// ───────── Every run ends with the lines that failed it (ENG-413) ─────────
//
// `validate` runs six passes. The four standalone ones print after the
// markdown lint's summary line, so no ordering inside a single pass can
// gather a markdown error and a locale-catalog error together — only a
// block printed after every pass can name them all. These tests assert the
// *rendered text* rather than the exit code, because the exit code was
// never the part that was wrong.

/// Run `validate` over `dir` and return its stdout and exit code.
fn validate_output(dir: &Path, extra_args: &[&str]) -> (String, i32) {
    let mut command = navigator();
    command.arg("validate").arg(dir);
    for arg in extra_args {
        command.arg(arg);
    }
    let output = command.output().unwrap();
    (
        String::from_utf8(output.stdout).unwrap(),
        output.status.code().unwrap(),
    )
}

/// A tree shaped like the run this was filed over: one Error-severity
/// violation (`S101`, an over-long line) among several Warning-severity
/// advisories (`M061`, a docs link to a code file) — the same shape that
/// produced the misreading, not a shape that hides it.
///
/// Which line lands last in the listing is not fixed: the listing follows
/// the directory walk, and `walkdir` reports entries in filesystem order,
/// which is sorted on NTFS and arbitrary on ext4/tmpfs. Tests over this
/// fixture assert the severity mixing and the position of the
/// recapitulation, never a specific walk order.
fn error_buried_among_warnings(dir: &Path) {
    // M061 is the web-portability advisory, not the disk-resolution
    // error, so its target has to exist for M057 to stay quiet. A
    // same-directory `lib.rs` is the shape the renderer cannot rewrite.
    write(dir, "docs/lib.rs", "pub fn placeholder() {}\n");
    write(
        dir,
        "docs/a_long.md",
        &format!("Intro.\n\n{}\n", "x".repeat(130)),
    );
    for name in ["m_one.md", "n_two.md", "z_three.md"] {
        write(
            dir,
            &format!("docs/{name}"),
            "Body.\n\nSee [lib](lib.rs) for detail.\n",
        );
    }
}

/// Collect the recapitulation block: every line after the
/// `N error(s) fail this run:` header.
fn recap_lines(stdout: &str) -> Vec<&str> {
    stdout
        .lines()
        .skip_while(|line| !line.ends_with("error(s) fail this run:"))
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .collect()
}

/// The run closes by naming every failing line again, on its own. This is
/// the half a per-line severity marker cannot cover: the marker helps a
/// reader looking *at* a line, and does nothing when the error scrolled
/// past hundreds of lines ago.
#[test]
fn validate_recapitulates_only_the_errors_at_the_tail() {
    let dir = TempDir::new().unwrap();
    error_buried_among_warnings(dir.path());
    let (stdout, code) = validate_output(dir.path(), &[]);
    assert_eq!(code, 1, "expected exit 1:\n{stdout}");

    // The primary listing mixes the severities, which is the condition the
    // recapitulation exists to answer. Assert the mixing itself rather than
    // which line lands last: the listing follows the directory walk, and
    // `walkdir` yields entries in whatever order the filesystem reports,
    // sorted on NTFS but arbitrary on ext4/tmpfs. Pinning the last line
    // would be a test of `walkdir`, not of this output.
    let listing: Vec<&str> = stdout
        .lines()
        .take_while(|line| !line.starts_with("Scanned "))
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert!(
        listing.iter().any(|line| line.starts_with("warning: "))
            && listing.iter().any(|line| line.starts_with("error: ")),
        "the listing must mix both severities for this test to mean anything; got: {listing:?}",
    );

    // The error is buried in the sense that matters: its place in the
    // listing is separated from the end of the run by the pass-summary
    // lines, so it is not what the reader is left looking at. The
    // recapitulation is what puts it back there.
    let lines: Vec<&str> = stdout.lines().collect();
    let first_error = lines
        .iter()
        .position(|line| line.starts_with("error: "))
        .expect("an error line in the listing");
    let summary = lines
        .iter()
        .position(|line| line.starts_with("Scanned "))
        .expect("the markdown summary line");
    let recap_header = lines
        .iter()
        .position(|line| line.ends_with("error(s) fail this run:"))
        .expect("the recapitulation header");
    assert!(
        first_error < summary && summary < recap_header,
        "expected listing then summaries then recapitulation, got indices \
         error={first_error}, summary={summary}, recap={recap_header}:\n{stdout}",
    );

    assert!(
        stdout.contains("1 error(s) fail this run:"),
        "expected an errors-only recapitulation in:\n{stdout}",
    );
    let recap = recap_lines(&stdout);
    assert_eq!(
        recap.len(),
        1,
        "expected one recapped error, got: {recap:?}"
    );
    assert!(
        recap[0].starts_with("error: ")
            && recap[0].contains("a_long.md")
            && recap[0].contains("S101"),
        "the recapped line must name the failing file and code; got: {:?}",
        recap[0],
    );
    assert!(
        !recap.iter().any(|line| line.contains("M061")),
        "an advisory must never appear in the errors recapitulation: {recap:?}",
    );

    // The recapitulation is the last thing on screen, where the terminal
    // leaves the reader.
    assert!(
        stdout
            .trim_end()
            .lines()
            .next_back()
            .unwrap()
            .starts_with("error: "),
        "the run must end on the failing line:\n{stdout}",
    );
}

/// The recapitulation spans every pass, not just the markdown lint. The
/// four standalone passes print *after* the markdown summary line, so
/// ordering inside one pass could never have gathered them; one block at
/// the tail is the only place that can name them all.
#[test]
fn validate_recapitulation_gathers_errors_from_every_pass() {
    let dir = TempDir::new().unwrap();
    error_buried_among_warnings(dir.path());
    // A locale catalog in a directory the site does not publish: Y002,
    // reported by the locale pass long after the markdown listing ended.
    write(dir.path(), "locales/xx/home.yaml", "heading: Hello\n");
    let (stdout, code) = validate_output(dir.path(), &[]);
    assert_eq!(code, 1, "expected exit 1:\n{stdout}");

    assert!(
        stdout.contains("2 error(s) fail this run:"),
        "expected both passes' errors counted together in:\n{stdout}",
    );
    let recap = recap_lines(&stdout);
    assert!(
        recap.iter().any(|line| line.contains("S101")),
        "markdown error missing from the recapitulation: {recap:?}",
    );
    assert!(
        recap.iter().any(|line| line.contains("Y002")),
        "locale-pass error missing from the recapitulation: {recap:?}",
    );
    assert!(
        recap.iter().all(|line| line.starts_with("error: ")),
        "every recapped line is an error: {recap:?}",
    );
}

/// A clean run says nothing extra: no recapitulation, no empty header.
#[test]
fn validate_prints_no_recapitulation_when_there_are_no_errors() {
    let dir = TempDir::new().unwrap();
    // Advisories only — M061 never fails the gate.
    write(dir.path(), "docs/lib.rs", "pub fn placeholder() {}\n");
    write(
        dir.path(),
        "docs/guide.md",
        "Body.\n\nSee [lib](lib.rs) for detail.\n",
    );
    let (stdout, code) = validate_output(dir.path(), &[]);
    assert_eq!(code, 0, "advisories must not fail the gate:\n{stdout}");
    assert!(
        stdout.contains("found 0 error(s), 1 warning(s)"),
        "expected the advisory counted:\n{stdout}",
    );
    assert!(
        !stdout.contains("fail this run"),
        "a clean run must not print a recapitulation:\n{stdout}",
    );
}

/// `--errors-only` narrows the listing and nothing else: the summary still
/// counts every advisory, and the exit code is unchanged. It is a triage
/// read, not a quieter gate.
#[test]
fn validate_errors_only_hides_advisories_but_not_their_count() {
    let dir = TempDir::new().unwrap();
    error_buried_among_warnings(dir.path());
    let (stdout, code) = validate_output(dir.path(), &["--errors-only"]);
    assert_eq!(code, 1, "the gate is unchanged by --errors-only:\n{stdout}");
    assert!(
        !stdout.contains("M061"),
        "--errors-only must hide the advisories:\n{stdout}",
    );
    assert!(
        stdout.contains("found 1 error(s), 3 warning(s)"),
        "the summary still counts the hidden advisories:\n{stdout}",
    );
    let recap = recap_lines(&stdout);
    assert_eq!(recap.len(), 1, "expected the error still named: {recap:?}");
    assert!(recap[0].contains("S101"), "got: {:?}", recap[0]);
}

/// `--errors-only` is rejected with `--fix`, where a remaining advisory
/// still has to be resolved before the run passes — hiding one there would
/// hide a line that fails the run.
#[test]
fn validate_errors_only_is_rejected_with_fix() {
    let dir = TempDir::new().unwrap();
    navigator()
        .args(["validate", "--errors-only", "--fix"])
        .arg(dir.path())
        .assert()
        .failure()
        .stderr(str::contains(
            "the argument '--errors-only' cannot be used with '--fix'",
        ));
}
