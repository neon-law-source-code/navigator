//! `N109` — a notation template's optional `output:` frontmatter field
//! declares its render profile, and that profile governs the form keys.
//!
//! The field is **optional**: a template that declares no `output:`
//! renders as a plain typeset document, so its absence is not a
//! violation on its own. When a template *does* declare one, a typo
//! (`output: leter`) would silently fall back to plain at render time —
//! this rule turns that into a loud validation failure instead.
//!
//! [`VALID`] is the closed set of render profiles. Most are Typst
//! styles (`letter` is letterhead; the implicit default is a plain
//! page), but `form` is a different render **mode** entirely — the
//! `AcroForm` fill path (`pdf::fill_acroform`), not a Typst format. So the
//! set is deliberately *not* a mirror of `pdf::OutputFormat`'s Typst
//! formats; `rules` does not depend on `pdf` and the two are decoupled.
//!
//! Because `form` names the `AcroForm` mode, it carries a companion-key
//! contract that this rule enforces (mirroring how `N110` keys
//! `origin_url:` off form templates): `output: form` requires both
//! `form:` and `origin_url:`, and conversely a `form:` key is only
//! allowed alongside `output: form`. Net: `form:` present ⇔
//! `output: form`. A typeset profile (`letter`, or no `output:` at all)
//! must not carry a stray `form:`.

use crate::{frontmatter, line_byte_range, Rule, SourceFile, Violation};

pub struct F109OutputFormat;

impl F109OutputFormat {
    pub const CODE: &'static str = "N109";
    /// Render profiles a template may declare. `letter` and `agreement`
    /// are Typst letterhead styles — the same chrome, typeset airily for
    /// a letter and curtly for a contract; `pleading` is court paper,
    /// calibrated by the template's `jurisdiction:`
    /// (`pdf::pleading::variant_for_jurisdiction`); `form` is the
    /// `AcroForm` fill **mode**, not a Typst format. `plain` is the
    /// implicit default and is intentionally not declarable. This set is
    /// decoupled from `pdf::OutputFormat::FRONTMATTER_VALUES` on purpose —
    /// `form` has no Typst format, `pleading` needs a jurisdiction `parse`
    /// never sees, and `rules` does not depend on `pdf`.
    pub const VALID: &'static [&'static str] = &["letter", "agreement", "pleading", "form"];
    /// The render profile that selects the `AcroForm` fill mode and so
    /// requires the `form:` / `origin_url:` companion keys.
    const FORM: &'static str = "form";
}

impl Rule for F109OutputFormat {
    fn code(&self) -> &'static str {
        Self::CODE
    }

    fn lint(&self, file: &SourceFile) -> Vec<Violation> {
        let Some(fm) = frontmatter::extract(&file.contents) else {
            // Absent frontmatter is N101/N102/N105's concern, not ours.
            return Vec::new();
        };
        let violation = |message: String| Violation {
            code: Self::CODE,
            path: file.path.clone(),
            line: 1,
            range: line_byte_range(&file.contents, 1),
            message,
        };
        let has = |key: &str| frontmatter::field(fm, key).is_some_and(|v| !v.is_empty());
        let form_present = has("form");

        // The `output:` field is optional. With it absent the template
        // is a plain typeset document, so the only thing to police is a
        // stray `form:` key with no profile to ride on.
        let Some(value) = frontmatter::field(fm, "output") else {
            if form_present {
                return vec![violation(
                    "Frontmatter `form:` requires `output: form` (a `form:` key only \
                     rides with the AcroForm render profile)"
                        .to_string(),
                )];
            }
            return Vec::new();
        };

        if !Self::VALID.contains(&value.as_str()) {
            let message = if value.is_empty() {
                format!(
                    "Frontmatter `output:` is empty (expected one of: {})",
                    Self::VALID.join(", ")
                )
            } else {
                format!(
                    "Invalid `output:` value `{value}` (expected one of: {})",
                    Self::VALID.join(", ")
                )
            };
            return vec![violation(message)];
        }

        // Require a companion key, distinguishing a missing key from a
        // present-but-empty one so the author sees the accurate message
        // (mirroring the empty/missing split on `output:` above).
        let require = |key: &str, noun: &str| match frontmatter::field(fm, key) {
            Some(v) if !v.is_empty() => None,
            Some(_) => Some(violation(format!(
                "`output: form` has an empty `{key}:` key ({noun})"
            ))),
            None => Some(violation(format!(
                "`output: form` requires a `{key}:` key ({noun})"
            ))),
        };

        let mut violations = Vec::new();
        if value == Self::FORM {
            // The AcroForm render mode: the form keys ride *with* it.
            violations.extend(require("form", "naming the bundled form"));
            violations.extend(require("origin_url", "for the bundled form"));
        } else if form_present {
            // A typeset profile (`letter`) must not carry a form key.
            violations.push(violation(format!(
                "Typeset `output: {value}` must not carry a `form:` key (a `form:` key \
                 belongs only with `output: form`)"
            )));
        }
        violations
    }
}

#[cfg(test)]
mod tests {
    use super::F109OutputFormat;
    use crate::{Rule, SourceFile};
    use std::path::PathBuf;

    fn file(body: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("trademark_coexistence.md"),
            contents: body.to_string(),
        }
    }

    #[test]
    fn passes_when_output_is_absent() {
        // The field is optional; no declaration means plain rendering.
        let f = file("---\ntitle: T\nconfidential: true\n---\n");
        assert!(F109OutputFormat.lint(&f).is_empty());
    }

    #[test]
    fn passes_for_a_known_format() {
        let f = file("---\ntitle: T\noutput: letter\n---\n");
        assert!(F109OutputFormat.lint(&f).is_empty());
    }

    #[test]
    fn passes_for_the_agreement_profile() {
        // `agreement` is the curt letterhead style for an executed
        // contract (`pdf::OutputFormat::Agreement`). A template may
        // declare it, and — being typeset rather than an AcroForm fill —
        // it needs no companion keys.
        let f = file("---\ntitle: T\noutput: agreement\n---\n");
        let v = F109OutputFormat.lint(&f);
        assert!(v.is_empty(), "{v:?}");
    }

    #[test]
    fn passes_for_the_pleading_profile() {
        // `pleading` is court paper, calibrated by the template's
        // `jurisdiction:` (`pdf::pleading::variant_for_jurisdiction`). Being
        // typeset rather than an AcroForm fill, it needs no companion keys.
        let f = file("---\ntitle: T\noutput: pleading\n---\n");
        let v = F109OutputFormat.lint(&f);
        assert!(v.is_empty(), "{v:?}");
    }

    #[test]
    fn flags_a_stray_form_key_on_a_pleading_template() {
        let f = file("---\ntitle: T\noutput: pleading\nform: nv__llc_formation\n---\n");
        let v = F109OutputFormat.lint(&f);
        assert_eq!(v.len(), 1);
        assert!(
            v[0].message.contains("must not carry a `form:` key"),
            "{v:?}"
        );
    }

    #[test]
    fn flags_a_junk_value_that_merely_looks_like_the_agreement_profile() {
        // Widening the set for `agreement` must not widen it to anything
        // agreement-shaped: the closed set is still closed.
        for junk in ["agreements", "Agreement", "contract", "agreement_letter"] {
            let f = file(&format!("---\ntitle: T\noutput: {junk}\n---\n"));
            let v = F109OutputFormat.lint(&f);
            assert_eq!(v.len(), 1, "`{junk}` was accepted: {v:?}");
            assert_eq!(v[0].code, "N109");
            assert!(v[0].message.contains(junk), "{v:?}");
            // The message lists what the author may write instead.
            assert!(v[0].message.contains("agreement"), "{v:?}");
        }
    }

    #[test]
    fn flags_a_stray_form_key_on_an_agreement_template() {
        // `agreement` is a typeset profile, so the `form:` ⇔
        // `output: form` equivalence binds it exactly as it binds
        // `letter`.
        let f = file("---\ntitle: T\noutput: agreement\nform: nv__llc_formation\n---\n");
        let v = F109OutputFormat.lint(&f);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].code, "N109");
        assert!(
            v[0].message.contains("must not carry a `form:` key"),
            "{v:?}"
        );
    }

    #[test]
    fn flags_an_unknown_format() {
        let f = file("---\noutput: demand_letter\n---\n");
        let v = F109OutputFormat.lint(&f);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].code, "N109");
        assert!(v[0].message.contains("demand_letter"));
        assert!(v[0].message.contains("letter"));
    }

    #[test]
    fn flags_an_empty_value() {
        let f = file("---\noutput:\n---\n");
        let v = F109OutputFormat.lint(&f);
        assert_eq!(v.len(), 1);
        assert!(v[0].message.contains("empty"));
    }

    #[test]
    fn passes_with_no_frontmatter_at_all() {
        // Frontmatter presence is enforced by other rules; N109 is silent.
        assert!(F109OutputFormat.lint(&file("Just prose.")).is_empty());
    }

    #[test]
    fn passes_for_output_form_with_both_companion_keys() {
        // `form` is a valid profile; with its companion keys it is clean.
        let f = file(
            "---\ntitle: T\noutput: form\nform: nv__llc_formation\n\
             origin_url: https://www.nvsos.gov/forms\n---\n",
        );
        let v = F109OutputFormat.lint(&f);
        assert!(v.is_empty(), "{v:?}");
    }

    #[test]
    fn flags_output_form_missing_form_key() {
        let f = file("---\ntitle: T\noutput: form\norigin_url: https://www.nvsos.gov/forms\n---\n");
        let v = F109OutputFormat.lint(&f);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].code, "N109");
        assert!(v[0].message.contains("requires a `form:` key"), "{v:?}");
    }

    #[test]
    fn flags_output_form_missing_origin_url() {
        let f = file("---\ntitle: T\noutput: form\nform: nv__llc_formation\n---\n");
        let v = F109OutputFormat.lint(&f);
        assert_eq!(v.len(), 1);
        assert!(v[0].message.contains("origin_url"), "{v:?}");
    }

    #[test]
    fn flags_output_form_missing_both_companion_keys() {
        let f = file("---\ntitle: T\noutput: form\n---\n");
        let v = F109OutputFormat.lint(&f);
        assert_eq!(v.len(), 2, "{v:?}");
    }

    #[test]
    fn distinguishes_an_empty_companion_key_from_a_missing_one() {
        // A bare `form:`/`origin_url:` line is present but empty; the
        // message must say "empty", not "requires", so the author is not
        // told to add a key that is visibly already there.
        let f = file("---\ntitle: T\noutput: form\nform:\norigin_url:\n---\n");
        let v = F109OutputFormat.lint(&f);
        assert_eq!(v.len(), 2, "{v:?}");
        assert!(
            v.iter().all(|x| x.message.contains("has an empty")),
            "{v:?}"
        );
        assert!(v.iter().any(|x| x.message.contains("`form:`")), "{v:?}");
        assert!(
            v.iter().any(|x| x.message.contains("`origin_url:`")),
            "{v:?}"
        );
    }

    #[test]
    fn invalid_output_value_short_circuits_before_the_companion_check() {
        // Incremental linting: an unrecognized `output:` value is reported
        // on its own. The companion-key contract is only evaluated once the
        // profile parses, so a simultaneously stray `form:` surfaces after
        // the typo is corrected rather than alongside it.
        let f = file("---\ntitle: T\noutput: form_pdf\nform: nv__llc_formation\n---\n");
        let v = F109OutputFormat.lint(&f);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].message.contains("form_pdf"), "{v:?}");
    }

    #[test]
    fn flags_stray_form_key_on_a_letter_template() {
        // A typeset profile must not carry a form key.
        let f = file("---\ntitle: T\noutput: letter\nform: nv__llc_formation\n---\n");
        let v = F109OutputFormat.lint(&f);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].code, "N109");
        assert!(
            v[0].message.contains("must not carry a `form:` key"),
            "{v:?}"
        );
    }

    #[test]
    fn flags_stray_form_key_when_output_is_absent() {
        // `form:` present ⇔ `output: form`; a bare `form:` needs the profile.
        let f = file("---\ntitle: T\nform: nv__llc_formation\n---\n");
        let v = F109OutputFormat.lint(&f);
        assert_eq!(v.len(), 1);
        assert!(v[0].message.contains("requires `output: form`"), "{v:?}");
    }
}
