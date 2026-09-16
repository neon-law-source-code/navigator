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
fn frontmatter_output_selects_the_frame_and_letterhead_is_larger_than_plain() {
    // `output:` is the template's own deliberate override, and the only
    // one: a `kind: will` template that declares none renders plain, an
    // `output: letter` one renders on letterhead, and the frame is read
    // from the document either way.
    let work = TempDir::new().unwrap();

    let letter_src = write(&work, "demand.md", VALID);
    let letter_out = work.path().join("letter.pdf");
    let letter = render(&[
        letter_src.as_os_str(),
        "--out".as_ref(),
        letter_out.as_ref(),
    ]);
    assert!(
        letter.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&letter.stderr)
    );

    let plain_src = write(&work, "will.md", VALID_WILL_NO_OUTPUT);
    let plain_out = work.path().join("plain.pdf");
    let plain = render(&[plain_src.as_os_str(), "--out".as_ref(), plain_out.as_ref()]);
    assert!(plain.status.success());
    assert!(
        String::from_utf8_lossy(&plain.stdout).contains("Plain"),
        "a `kind: will` template should report Plain, got: {}",
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

    // The contrast case is a different `kind:`, not a flag: `will`
    // derives plain, so the two derivations are what differ.
    let plain_src = write(&work, "will.md", VALID_WILL_NO_OUTPUT);
    let plain_out = work.path().join("plain.pdf");
    let plain = render(&[plain_src.as_os_str(), "--out".as_ref(), plain_out.as_ref()]);
    assert!(plain.status.success());

    let derived_len = fs::read(&derived_out).unwrap().len();
    let plain_len = fs::read(&plain_out).unwrap().len();
    assert!(
        derived_len > plain_len,
        "a `kind: letter` template with no `output:` should default to \
         letterhead ({derived_len}) rather than plain ({plain_len}) — logo missing?"
    );
    let stdout = String::from_utf8_lossy(&plain.stdout);
    assert!(
        stdout.contains("Plain"),
        "a `kind: will` template should report Plain, got: {stdout}"
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
fn the_format_flag_is_retired_and_cannot_reframe_an_instrument() {
    // LAW-15: `--format` chose the frame a second time, from outside the
    // document, and won over a correct header. A `kind: will` template
    // renders the unadorned instrument; `--format letter`, passed out of
    // habit, silently put the firm's letterhead on it. The flag is gone,
    // so the mistake is now a refusal at the argument parser rather than
    // a wrongly-framed PDF nobody was warned about.
    let work = TempDir::new().unwrap();
    let src = write(&work, "will.md", VALID_WILL_NO_OUTPUT);
    let out = work.path().join("will.pdf");
    let result = render(&[
        src.as_os_str(),
        "--out".as_ref(),
        out.as_ref(),
        "--format".as_ref(),
        "letter".as_ref(),
    ]);
    assert!(
        !result.status.success(),
        "--format must no longer be accepted"
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("unexpected argument") && stderr.contains("--format"),
        "expected clap to refuse the retired flag, got: {stderr}"
    );
    assert!(!out.exists(), "no PDF should be written");
}

/// A choice question whose options carry the prose the body reads.
/// `custom_questions.<key>.choices` is a `value: label` map, and the body
/// interpolates the state — so the rendered instrument must read the
/// label ("Nevada"), never the stored key ("nevada").
const VALID_CHOICE: &str = "\
---
kind: letter
title: Governed Demand
respondent_type: entity
code: test__governed_demand
confidential: true
custom_questions:
  governing_law:
    prompt: Which state's law governs this engagement?
    choices:
      nevada: Nevada
      california: California
questionnaire:
  BEGIN:
    _: custom_single_choice__governing_law
  custom_single_choice__governing_law:
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

This letter is governed by the law of {{custom_single_choice__governing_law}}.
";

#[test]
fn a_choice_answer_renders_its_label_not_its_stored_key() {
    // LAW-13: a `{{custom_single_choice__*}}` placeholder filled with a
    // declared choice *key* must reach the page as that choice's *label*.
    // The key is an answer code, not prose; substituting it verbatim put
    // "governed by the law of nevada" into the firm's own onboarding
    // letter. The portal's document path already resolves the label
    // (`retainer_walk::render_context_from_answers`); this proves the CLI
    // preview agrees with it rather than rendering a different document.
    let work = TempDir::new().unwrap();
    let src = write(&work, "governed.md", VALID_CHOICE);
    let out = work.path().join("governed.pdf");
    let result = render(&[
        src.as_os_str(),
        "--out".as_ref(),
        out.as_ref(),
        "--answer".as_ref(),
        "custom_single_choice__governing_law=nevada".as_ref(),
    ]);
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let bytes = fs::read(&out).expect("pdf written");
    assert_eq!(
        pdf::occurrence_count(&bytes, "the law of Nevada").expect("scan the rendered pdf"),
        1,
        "the choice label must reach the page"
    );
    assert_eq!(
        pdf::occurrence_count(&bytes, "the law of nevada").expect("scan the rendered pdf"),
        0,
        "the stored choice key must not reach the page"
    );
}

#[test]
fn a_free_text_answer_is_unaffected_by_choice_label_resolution() {
    // The other side of LAW-13: a state with no declared `choices:` keeps
    // its answer verbatim, so label resolution cannot swallow free text.
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
    let bytes = fs::read(&out).expect("pdf written");
    assert_eq!(
        pdf::occurrence_count(&bytes, "5000 USD").expect("scan the rendered pdf"),
        1,
        "a free-text answer must render verbatim"
    );
}
