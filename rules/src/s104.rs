//! `S104` — a file's declared `kind:` must agree with its notation/event
//! structure.
//!
//! Classification reads the declared `kind:` and nothing else (see
//! [`crate::kind`] and [`crate::classify_source`]); it never infers the
//! family from a `questionnaire:`/`workflow:` block or a `starts_at:`
//! timestamp. That opens two silent failure modes this rule closes:
//!
//! 1. **Structure without a kind.** An author writes a real template —
//!    questionnaire, workflow, the works — but forgets the `kind:` line, so
//!    the file classifies as plain prose and skips every N-family rule
//!    without a peep. If the frontmatter declares the notation machine
//!    (`questionnaire:`/`workflow:`) or the event machine (`starts_at:`)
//!    but no `kind:` key, S104 tells the author to declare one.
//! 2. **A content page carrying notation structure.** A content-page kind
//!    (`post`, `workshop`) gets only the content-page rules, so
//!    a copied `questionnaire:`/`workflow:` block would be silently
//!    accepted — no N-family checks ever run on it. S104 flags that
//!    mismatch. (`event` is exempt: it declares its own `starts_at`
//!    machine, and an event that also declares a questionnaire is
//!    [`crate::E002EventTemplateExclusive`]'s job, not this one.)
//!
//! For a present-but-*invalid* `kind:` value, S104 stays silent and lets
//! [`crate::S103KindEnum`] own the line, so the two never double-flag it.

use crate::kind::Kind;
use crate::{frontmatter, kind, line_byte_range, Rule, SourceFile, Violation};

/// `S104` — the declared `kind:` must match the file's notation/event
/// structure (or be present when structure demands one).
pub struct S104MissingKind;

impl S104MissingKind {
    pub const CODE: &'static str = "S104";
}

impl Rule for S104MissingKind {
    fn code(&self) -> &'static str {
        Self::CODE
    }

    fn description(&self) -> &'static str {
        crate::description_for_code(Self::CODE)
    }

    fn lint(&self, file: &SourceFile) -> Vec<Violation> {
        let Some(fm) = frontmatter::extract(&file.contents) else {
            return Vec::new();
        };
        let notation_machine =
            frontmatter_has_key(fm, "questionnaire") || frontmatter_has_key(fm, "workflow");
        let event_machine = frontmatter_has_key(fm, "starts_at");

        let message = if frontmatter_has_key(fm, "kind") {
            // A present `kind:` is declared. An invalid value is S103's
            // concern — stay silent. A valid *content-page* kind that also
            // carries notation structure is the mismatch we flag.
            let Some(kind) = kind::declared(&file.contents) else {
                return Vec::new();
            };
            if !kind.carries_questionnaire() && kind != Kind::Event && notation_machine {
                format!(
                    "A `{}` page must not declare `questionnaire:`/`workflow:` — that is \
                     notation-template structure, which a content page never carries",
                    kind.as_str()
                )
            } else {
                return Vec::new();
            }
        } else if event_machine {
            format!(
                "This file declares `starts_at:` but no `kind:`; declare `kind: event` \
                 (classification no longer infers the family — one of: {})",
                kind::VALID.join(", ")
            )
        } else if notation_machine {
            format!(
                "This file declares `questionnaire:`/`workflow:` but no `kind:`; declare a \
                 notation kind (classification no longer infers the family — one of: {})",
                kind::VALID.join(", ")
            )
        } else {
            return Vec::new();
        };
        vec![Violation {
            code: Self::CODE,
            path: file.path.clone(),
            line: 1,
            range: line_byte_range(&file.contents, 1),
            message,
        }]
    }
}

/// Whether the leading frontmatter declares `key` as a top-level mapping
/// key. Unlike [`frontmatter::field`], this is true for a non-scalar
/// value too (a `questionnaire:` mapping, a `workflow:` mapping), which is
/// exactly the structure this rule keys on.
fn frontmatter_has_key(fm: &str, key: &str) -> bool {
    serde_yaml::from_str::<serde_yaml::Value>(fm)
        .ok()
        .and_then(|v| {
            v.as_mapping()
                .map(|m| m.contains_key(serde_yaml::Value::String(key.to_string())))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::S104MissingKind;
    use crate::{Rule, SourceFile};
    use std::path::PathBuf;

    fn file(body: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("test.md"),
            contents: body.to_string(),
        }
    }

    #[test]
    fn plain_prose_and_kindful_files_pass() {
        // No frontmatter, no structure — nothing to flag.
        assert!(S104MissingKind.lint(&file("# Just prose\n")).is_empty());
        // Frontmatter without a machine — a marketing page — is fine.
        assert!(S104MissingKind
            .lint(&file("---\ntitle: Service\ncode: northstar\n---\n"))
            .is_empty());
        // A template that DOES declare its kind is fine.
        assert!(S104MissingKind
            .lint(&file(
                "---\nkind: onboarding\nquestionnaire:\n  BEGIN:\n    _: END\n---\n"
            ))
            .is_empty());
    }

    #[test]
    fn questionnaire_without_kind_is_flagged() {
        let v = S104MissingKind.lint(&file(
            "---\ntitle: Draft\nquestionnaire:\n  BEGIN:\n    _: END\n---\n",
        ));
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].code, "S104");
        assert!(v[0].message.contains("questionnaire"));
    }

    #[test]
    fn workflow_without_kind_is_flagged() {
        let v = S104MissingKind.lint(&file(
            "---\ntitle: Draft\nworkflow:\n  BEGIN:\n    created: END\n---\n",
        ));
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].code, "S104");
    }

    #[test]
    fn starts_at_without_kind_names_event() {
        let v = S104MissingKind.lint(&file(
            "---\ntitle: E\nstarts_at: \"2026-07-02T11:00:00\"\n---\n",
        ));
        assert_eq!(v.len(), 1);
        assert!(
            v[0].message.contains("kind: event"),
            "should point authors at kind: event, got {}",
            v[0].message
        );
    }

    #[test]
    fn present_but_invalid_kind_is_left_to_s103() {
        // The `kind:` key IS present (just wrong) — S103 owns it, S104 is
        // silent so they never double-flag the same line.
        let v = S104MissingKind.lint(&file(
            "---\nkind: bogus\nquestionnaire:\n  BEGIN:\n    _: END\n---\n",
        ));
        assert!(v.is_empty(), "S104 must defer to S103 here, got {v:?}");
    }

    #[test]
    fn a_content_page_carrying_notation_structure_is_flagged() {
        // A workshop or post page gets only content-page rules, so a copied
        // questionnaire/workflow block would never be N-checked. S104 flags
        // the mismatch so the structure can't be silently kept.
        //
        // `event` is deliberately absent: the rule exempts `Kind::Event`,
        // which carries its own E-family structure.
        for kind in ["workshop", "post"] {
            let v = S104MissingKind.lint(&file(&format!(
                "---\nkind: {kind}\ntitle: T\nquestionnaire:\n  BEGIN:\n    _: END\n---\n"
            )));
            assert_eq!(v.len(), 1, "kind `{kind}` should be flagged, got {v:?}");
            assert!(
                v[0].message.contains("must not declare"),
                "got {}",
                v[0].message
            );
        }
        // A clean content page (no notation structure) is fine.
        assert!(S104MissingKind
            .lint(&file(
                "---\nkind: workshop\ntitle: T\ndescription: D\n---\n"
            ))
            .is_empty());
    }

    #[test]
    fn an_event_with_a_questionnaire_is_left_to_e002_not_double_flagged() {
        // `event` declares its own `starts_at` machine; an event that also
        // carries a questionnaire is E002's exclusivity conflict, so S104
        // stays silent to avoid double-flagging the same file.
        let v = S104MissingKind.lint(&file(
            "---\nkind: event\nstarts_at: \"2026-07-02T11:00:00\"\nquestionnaire:\n  BEGIN:\n    _: END\n---\n",
        ));
        assert!(
            v.is_empty(),
            "S104 must defer to E002 for events, got {v:?}"
        );
    }
}
