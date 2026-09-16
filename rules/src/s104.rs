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
//! 3. **A template that declares no machine at all.** The first two
//!    triggers both read *structure*, which only closes the gap for a file
//!    that happens to carry a questionnaire or a workflow. An instrument
//!    that is pure body prose — a will, a directive — carries neither, so
//!    dropping its `kind:` left it classifying as plain Markdown with
//!    nothing to report: N105, N107, N115 and N122 all went quiet, an
//!    unrelated set of prose findings appeared in their place, and the
//!    file *looked* like it passed. Being under a `templates/` tree is
//!    what makes a file a template, so that is what requires the field,
//!    rather than a shape the file may or may not have. A templates tree
//!    carries its own README and agent contract; those are
//!    [`TEMPLATE_LANE_FURNITURE`] and stay exempt.
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
        let template_lane = in_template_lane(&file.path);
        let Some(fm) = frontmatter::extract(&file.contents) else {
            // A template with no frontmatter has no `kind:` either, and
            // the N-family rules that would have demanded frontmatter
            // never run on an unclassified file — so the lane check has
            // to precede this lookup rather than sit behind it.
            return if template_lane {
                vec![missing_kind_in_template_lane(file)]
            } else {
                Vec::new()
            };
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
        } else if template_lane {
            return vec![missing_kind_in_template_lane(file)];
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

/// The files a `templates/` tree carries that are not templates: its own
/// README and the agent contract. Neither is a notation and neither has a
/// kind to declare.
const TEMPLATE_LANE_FURNITURE: &[&str] = &["README.md", "AGENTS.md", "CLAUDE.md"];

/// Whether `path` sits inside a templates tree — Navigator's own
/// `templates/` catalog or a Project repository's `templates/` root, which
/// are the same directory name by design. Mirrors the component walk
/// [`crate::F110JurisdictionPath`] uses to find the legal shelves.
fn in_template_lane(path: &std::path::Path) -> bool {
    if path.extension().and_then(|e| e.to_str()) != Some("md") {
        return false;
    }
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| TEMPLATE_LANE_FURNITURE.contains(&name))
    {
        return false;
    }
    path.components()
        .any(|c| matches!(c, std::path::Component::Normal(seg) if seg == "templates"))
}

/// The finding for a template-lane file that declares no `kind:`.
fn missing_kind_in_template_lane(file: &SourceFile) -> Violation {
    Violation {
        code: S104MissingKind::CODE,
        path: file.path.clone(),
        line: 1,
        range: line_byte_range(&file.contents, 1),
        message: format!(
            "This file is under `templates/` but declares no `kind:`; every template \
             declares what it is, and without the field it is linted as plain prose \
             rather than as a notation (one of: {})",
            kind::VALID.join(", ")
        ),
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

    fn file_at(path: &str, body: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from(path),
            contents: body.to_string(),
        }
    }

    #[test]
    fn plain_prose_and_kindful_files_pass() {
        // No frontmatter, no structure — nothing to flag.
        assert!(S104MissingKind.lint(&file("# Just prose\n")).is_empty());
        // Frontmatter without a machine — a marketing page — is fine.
        assert!(S104MissingKind
            .lint(&file("---\ntitle: Service\ncode: sample\n---\n"))
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

    #[test]
    fn a_template_lane_file_without_kind_is_flagged() {
        // LAW-15: `kind:` was only required by *inference* — a file that
        // declared the notation machine. A template carrying neither a
        // questionnaire nor a workflow (an instrument that is pure body
        // prose) could drop its `kind:` and validate clean, while quietly
        // losing every N-family check. Living under `templates/` is what
        // makes a file a template, so that is what requires the field.
        for path in [
            "templates/notations/neon_law/shared/will.md",
            "templates/notations/forms/united_states/nevada/state/nv__llc_formation.md",
        ] {
            let v = S104MissingKind.lint(&file_at(
                path,
                "---\ntitle: Last Will\ncode: test__will\nconfidential: true\n---\n",
            ));
            assert_eq!(v.len(), 1, "`{path}` should be flagged, got {v:?}");
            assert_eq!(v[0].code, "S104");
            assert!(
                v[0].message.contains("under `templates/`"),
                "the message must name the lane, got {}",
                v[0].message
            );
        }
    }

    #[test]
    fn a_template_lane_file_with_no_frontmatter_at_all_is_flagged() {
        // No frontmatter means no `kind:` either, and the N-family rules
        // that would have demanded frontmatter never run on an
        // unclassified file. The lane check therefore precedes the
        // frontmatter lookup rather than returning early behind it.
        let v = S104MissingKind.lint(&file_at(
            "templates/notations/neon_law/shared/will.md",
            "# Last Will and Testament\n",
        ));
        assert_eq!(v.len(), 1, "got {v:?}");
        assert_eq!(v[0].code, "S104");
    }

    #[test]
    fn template_lane_repository_furniture_is_exempt() {
        // A templates tree carries its own README and agent contract.
        // Neither is a notation, and neither has a kind to declare.
        for name in ["README.md", "AGENTS.md", "CLAUDE.md"] {
            assert!(
                S104MissingKind
                    .lint(&file_at(&format!("templates/{name}"), "# Templates\n"))
                    .is_empty(),
                "`{name}` must stay exempt"
            );
        }
    }

    #[test]
    fn a_template_lane_file_that_declares_its_kind_passes() {
        assert!(S104MissingKind
            .lint(&file_at(
                "templates/notations/neon_law/shared/will.md",
                "---\nkind: will\ntitle: Last Will\n---\n",
            ))
            .is_empty());
    }

    #[test]
    fn a_kindless_file_outside_the_template_lane_is_untouched() {
        // `docs/`, `server/content/`, and a bare prose file are not
        // templates; the lane check must not turn every Markdown file in
        // the workspace into a notation.
        for path in [
            "docs/glossary.md",
            "README.md",
            "server/content/blog/post.md",
        ] {
            assert!(
                S104MissingKind
                    .lint(&file_at(path, "---\ntitle: T\n---\n\n# T\n"))
                    .is_empty(),
                "`{path}` must not be held to the template lane"
            );
        }
    }
}
