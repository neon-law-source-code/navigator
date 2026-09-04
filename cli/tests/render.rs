//! End-to-end tests for `navigator notations render <file> --out <pdf>`. Each
//! test writes a notation fixture to a tempdir, invokes the real
//! binary, and checks the produced PDF (or the refusal).

use std::fs;
use std::process::Command;

use assert_cmd::cargo::cargo_bin;
use tempfile::TempDir;

/// A minimal notation template that passes the full validation gate:
/// `kind:` (the declared classifier) / `title` / `respondent_type` /
/// `code` / `confidential`, a `questionnaire:` + `workflow:` with the
/// required lawyer review, and a clean Markdown body. `output:` is
/// `letter`; callers can override on the CLI.
const VALID: &str = "\
---
kind: letter
title: Test Demand
respondent_type: entity
code: test__demand
confidential: true
output: letter
questionnaire:
  BEGIN:
    _: END
  END: {}
workflow:
  BEGIN:
    intake_submitted: lawyer_review
  lawyer_review:
    approved: END
    rejected: END
  END: {}
---

# Demand

Pay the sum of `{{amount}}` to **NEON LAW** without delay.

- First point
- Second point
";

/// Same fixture as `VALID` but with no `output:` declared at all — the
/// regression case: a `kind: letter` template must still render on
/// letterhead by default, derived from `Kind::default_output`.
const VALID_NO_OUTPUT: &str = "\
---
kind: letter
title: Test Demand
respondent_type: entity
code: test__demand
confidential: true
questionnaire:
  BEGIN:
    _: END
  END: {}
workflow:
  BEGIN:
    intake_submitted: lawyer_review
  lawyer_review:
    approved: END
    rejected: END
  END: {}
---

# Demand

Pay the sum of `{{amount}}` to **NEON LAW** without delay.

- First point
- Second point
";

/// Same shape, `kind: will` — a kind whose default is `plain`, so this
/// proves the derivation does not blanket every notation kind in
/// letterhead.
const VALID_WILL_NO_OUTPUT: &str = "\
---
kind: will
title: Test Will
respondent_type: person
code: test__will
confidential: true
questionnaire:
  BEGIN:
    _: END
  END: {}
workflow:
  BEGIN:
    intake_submitted: lawyer_review
  lawyer_review:
    approved: END
    rejected: END
  END: {}
---

# Last Will and Testament

I hereby revoke all prior wills.
";

const VALID_TYPED: &str = "\
---
kind: letter
title: Typed Demand
respondent_type: entity
code: test__typed_demand
confidential: true
questionnaire:
  BEGIN:
    _: person__client
  person__client:
    _: people__members
  people__members:
    _: END
  END: {}
workflow:
  BEGIN:
    intake_submitted: lawyer_review
  lawyer_review:
    approved: END
    rejected: END
  END: {}
---

Client: {{person__client.name}}

Members:
{{#for m in people__members}}- {{m.name}} from {{m.city}}
{{/for}}
";

fn write(dir: &TempDir, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    fs::write(&path, body).expect("write fixture");
    path
}

fn render(args: &[&std::ffi::OsStr]) -> std::process::Output {
    Command::new(cargo_bin("navigator"))
        .args(["notations", "render"])
        .args(args)
        .output()
        .expect("run navigator notations render")
}

#[test]
fn renders_a_letter_pdf_from_a_valid_template() {
    let work = TempDir::new().unwrap();
    let src = write(&work, "demand.md", VALID);
    let out = work.path().join("demand.pdf");
    let result = render(&[src.as_os_str(), "--out".as_ref(), out.as_os_str()]);
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let bytes = fs::read(&out).expect("pdf written");
    assert_eq!(&bytes[..4], b"%PDF", "output is not a PDF");
}

#[test]
fn cli_format_overrides_frontmatter_and_letter_is_larger_than_plain() {
    let work = TempDir::new().unwrap();
    let src = write(&work, "demand.md", VALID);

    let letter_out = work.path().join("letter.pdf");
    // `output: letter` from frontmatter — no flag.
    let letter = render(&[src.as_os_str(), "--out".as_ref(), letter_out.as_ref()]);
    assert!(letter.status.success());

    let plain_out = work.path().join("plain.pdf");
    // `--format plain` overrides the `output: letter` frontmatter.
    let plain = render(&[
        src.as_os_str(),
        "--out".as_ref(),
        plain_out.as_ref(),
        "--format".as_ref(),
        "plain".as_ref(),
    ]);
    assert!(plain.status.success());
    assert!(
        String::from_utf8_lossy(&plain.stdout).contains("Plain"),
        "override should report Plain, got: {}",
        String::from_utf8_lossy(&plain.stdout)
    );

    let letter_len = fs::read(&letter_out).unwrap().len();
    let plain_len = fs::read(&plain_out).unwrap().len();
    assert!(
        letter_len > plain_len,
        "letterhead PDF ({letter_len}) should exceed plain ({plain_len}) — logo missing?"
    );
}

#[test]
fn a_letter_kind_renders_on_letterhead_with_no_output_declared() {
    // The regression case: a `kind: letter` template with no `output:`
    // field must still derive letterhead by default, not silently fall
    // back to plain.
    let work = TempDir::new().unwrap();
    let src = write(&work, "demand.md", VALID_NO_OUTPUT);

    let derived_out = work.path().join("derived.pdf");
    let derived = render(&[src.as_os_str(), "--out".as_ref(), derived_out.as_ref()]);
    assert!(
        derived.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&derived.stderr)
    );

    let plain_out = work.path().join("plain.pdf");
    let plain = render(&[
        src.as_os_str(),
        "--out".as_ref(),
        plain_out.as_ref(),
        "--format".as_ref(),
        "plain".as_ref(),
    ]);
    assert!(plain.status.success());

    let derived_len = fs::read(&derived_out).unwrap().len();
    let plain_len = fs::read(&plain_out).unwrap().len();
    assert!(
        derived_len > plain_len,
        "a `kind: letter` template with no `output:` should default to \
         letterhead ({derived_len}) rather than plain ({plain_len}) — logo missing?"
    );

    // An explicit `--format` still overrides the derived default.
    let stdout = String::from_utf8_lossy(&plain.stdout);
    assert!(
        stdout.contains("Plain"),
        "override should report Plain, got: {stdout}"
    );
}

#[test]
fn a_plain_default_kind_renders_plain_with_no_output_declared() {
    // The mirror case: `kind: will` defaults to plain, so a template
    // declaring no `output:` must not pick up letterhead by accident.
    let work = TempDir::new().unwrap();
    let src = write(&work, "will.md", VALID_WILL_NO_OUTPUT);
    let out = work.path().join("will.pdf");
    let result = render(&[src.as_os_str(), "--out".as_ref(), out.as_os_str()]);
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        stdout.contains("Plain"),
        "a `kind: will` template with no `output:` should render Plain, got: {stdout}"
    );
}

#[test]
fn answer_substitutes_a_placeholder() {
    let work = TempDir::new().unwrap();
    let src = write(&work, "demand.md", VALID);
    let out = work.path().join("demand.pdf");
    let result = render(&[
        src.as_os_str(),
        "--out".as_ref(),
        out.as_ref(),
        "--answer".as_ref(),
        "amount=5000 USD".as_ref(),
    ]);
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    // The rendered PDF compresses text, so we can't grep the value out;
    // success plus a valid PDF is the contract. The substitution logic
    // itself is unit-tested in the pdf crate's markdown round-trip.
    assert_eq!(&fs::read(&out).unwrap()[..4], b"%PDF");
}

#[test]
fn render_uses_shared_notation_evaluator_for_dotted_fields_and_loops() {
    let work = TempDir::new().unwrap();
    let src = write(&work, "typed_demand.md", VALID_TYPED);
    let out = work.path().join("typed_demand.pdf");
    let result = render(&[
        src.as_os_str(),
        "--out".as_ref(),
        out.as_ref(),
        "--answer".as_ref(),
        "person__client.name=Libra Prime".as_ref(),
        "--answer".as_ref(),
        r#"people__members=[{"name":"Aries","city":"Las Vegas"},{"name":"Virgo","city":"Reno"}]"#
            .as_ref(),
    ]);
    assert!(
        result.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(&fs::read(&out).unwrap()[..4], b"%PDF");
}

#[test]
fn renders_despite_a_non_blocking_advisory() {
    // The `VALID` fixture's mandatory `lawyer_review` gate earns the
    // yellow N112 "not built yet" advisory — a Warning, not an Error.
    // Rendering must not be blocked by it (it is, however, still printed
    // so the author sees it), mirroring `validate` / `import`.
    let work = TempDir::new().unwrap();
    let src = write(&work, "demand.md", VALID);
    let out = work.path().join("demand.pdf");
    let result = render(&[src.as_os_str(), "--out".as_ref(), out.as_os_str()]);
    assert!(
        result.status.success(),
        "a Warning-only template must still render, stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        stdout.contains("N112"),
        "the advisory should still be surfaced, got stdout: {stdout}"
    );
    assert_eq!(
        &fs::read(&out).unwrap()[..4],
        b"%PDF",
        "output is not a PDF"
    );
}

#[test]
fn refuses_a_template_that_fails_validation() {
    let work = TempDir::new().unwrap();
    // Drop the required `code:` field (N108) — still classifies as a
    // notation template via its workflow, so the gate fires.
    let bad = VALID.replace("code: test__demand\n", "");
    let src = write(&work, "demand.md", &bad);
    let out = work.path().join("demand.pdf");
    let result = render(&[src.as_os_str(), "--out".as_ref(), out.as_os_str()]);
    assert!(
        !result.status.success(),
        "should refuse an invalid template"
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("validation error"),
        "expected a validation refusal, got: {stderr}"
    );
    assert!(!out.exists(), "no PDF should be written on refusal");
}

#[test]
fn rejects_an_unknown_format() {
    let work = TempDir::new().unwrap();
    let src = write(&work, "demand.md", VALID);
    let out = work.path().join("demand.pdf");
    let result = render(&[
        src.as_os_str(),
        "--out".as_ref(),
        out.as_ref(),
        "--format".as_ref(),
        "demand_letter".as_ref(),
    ]);
    assert!(!result.status.success(), "unknown format should fail");
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("unknown --format"),
        "expected unknown-format error, got: {stderr}"
    );
}
