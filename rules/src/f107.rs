//! `N107` — notation template signature placeholders must resolve.
//!
//! A *signature placeholder* is a `{{ … }}` token whose trimmed inner
//! text names a signer and field — e.g. `{{client.signature}}`. Dotted
//! data paths such as `{{person__client.name}}` and aggregate loop
//! variables such as `{{m.name}}` belong to `N115`, so this rule skips
//! questionnaire states and lexical `#for` variables.
//!
//! The placeholder splits on the **first** dot into `<signer>.<field>`:
//!
//! - `signer` must be a role from the template's declared [`SignerSet`]:
//!   the optional frontmatter `signers:` list, or `[client, firm]` when
//!   that key is absent. Roles, never a person's name — each signer
//!   resolves to a real Person at notation time (`client` to the
//!   respondent, `firm` to the attorney of record, any other role to
//!   the matching `person__<role>` questionnaire state).
//! - `field` must be one of [`F107SignaturePlaceholders::FIELDS`]
//!   (`signature`, `initials`, `date`).
//!
//! An explicit `signers:` list is bidirectional with the body: every
//! listed role must appear as a `signature` or `initials` placeholder,
//! and every placeholder role must be on the list. A declared role with
//! no placeholder is an error.
//!
//! The signing declaration is **bidirectional** — a well-formed
//! signature is declared in two places that must agree:
//!
//! - **Forward:** a template that draws *any* valid signature block must
//!   declare a signing State in its workflow — a state keyed
//!   `sent_for_signature` or prefixed `sent_for_signature__`. A
//!   signature line that never becomes an attributable signature is a
//!   candor problem.
//! - **Reverse:** a template whose workflow declares a
//!   `sent_for_signature[__*]` State must carry at least one body
//!   signature anchor for the field to land on. A signing step with
//!   nowhere to sign is the same candor gap in mirror image — and it is
//!   exactly how a template reaches the e-signature provider with a
//!   signing state but no placed tab (the live retainer bug).
//!
//! Both directions are deliberate: the provider's signers and tabs are
//! derived from this two-part declaration, never from ad-hoc UI
//! placement, so the template can never reach the provider with one half
//! of the pair missing.
//!
//! Files without frontmatter are skipped: N107 is a notation-template
//! rule, not a check on arbitrary prose that happens to contain a
//! `{{x.y}}` token.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_yaml::Value;

use crate::{frontmatter, is_snake_case, Rule, SourceFile, Violation};

/// Default `signers:` when the key is absent — every existing template.
pub const DEFAULT_SIGNERS: &[&str] = &["client", "firm"];

/// The signer roles a template permits, in declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignerSet {
    /// Role names (`client`, `firm`, `confirming_assignor`, …).
    pub roles: Vec<String>,
    /// True when frontmatter carried an explicit `signers:` list.
    pub explicit: bool,
}

impl SignerSet {
    fn implicit_default() -> Self {
        Self {
            roles: DEFAULT_SIGNERS.iter().map(|s| (*s).to_string()).collect(),
            explicit: false,
        }
    }

    /// Comma-separated roles for diagnostics.
    #[must_use]
    pub fn joined(&self) -> String {
        self.roles.join(", ")
    }

    /// True when `role` is in this set.
    #[must_use]
    pub fn contains(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
    }
}

/// Parse the template's signer set from its full Markdown source.
///
/// No frontmatter, no `signers:` key, or a null `signers:` all yield the
/// implicit default `[client, firm]`. A present list must be lowercase
/// `snake_case` role names with no duplicates.
pub fn signer_set(contents: &str) -> Result<SignerSet, String> {
    let Some(fm) = frontmatter::extract(contents) else {
        return Ok(SignerSet::implicit_default());
    };
    let yaml: Value = serde_yaml::from_str(fm).map_err(|e| e.to_string())?;
    parse_signer_set(&yaml)
}

fn parse_signer_set(yaml: &Value) -> Result<SignerSet, String> {
    let Some(signers) = yaml.get("signers") else {
        return Ok(SignerSet::implicit_default());
    };
    if signers.is_null() {
        return Ok(SignerSet::implicit_default());
    }
    let Some(seq) = signers.as_sequence() else {
        return Err("signers: must be a list of lowercase snake_case role names".to_string());
    };
    let mut roles = Vec::with_capacity(seq.len());
    for (i, item) in seq.iter().enumerate() {
        let Some(name) = item.as_str() else {
            return Err(format!("signers[{i}] must be a string role name"));
        };
        if !is_signer_role_name(name) {
            return Err(format!(
                "`{name}` is not a lowercase snake_case signer role"
            ));
        }
        if roles.iter().any(|r| r == name) {
            return Err(format!("signer role `{name}` is listed twice"));
        }
        roles.push(name.to_string());
    }
    Ok(SignerSet {
        roles,
        explicit: true,
    })
}

fn is_signer_role_name(name: &str) -> bool {
    is_snake_case(name) && !name.contains("__")
}

const FOR_OPEN: &str = "{{#for ";

pub struct F107SignaturePlaceholders;

impl F107SignaturePlaceholders {
    pub const CODE: &'static str = "N107";

    /// Default signer roles when a template omits `signers:`.
    pub const SIGNERS: &'static [&'static str] = DEFAULT_SIGNERS;

    /// Recognized signature field types, each mapping to a distinct
    /// downstream e-signature tab (signHere / initialHere / dateSigned).
    pub const FIELDS: &'static [&'static str] = &["signature", "initials", "date"];

    /// The workflow State prefix that marks a signing step.
    const SIGNING_STATE: &'static str = "sent_for_signature";
}

/// One `{{ signer.field }}` token found in a source file. `offset` is
/// the byte offset of the opening `{{`; `end` is the byte offset just
/// past the closing `}}` (so `contents[offset..end]` is the whole
/// token); `signer`/`field` are the halves of the trimmed inner text
/// split on the first `.`.
///
/// Exposed so the renderer and the signature-manifest builder consume
/// the *same* grammar this rule validates — one parser, one source of
/// truth for what a signature placeholder is. `offset`/`end` let the
/// renderer splice the token out and replace it without re-scanning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignaturePlaceholder {
    pub offset: usize,
    pub end: usize,
    pub signer: String,
    pub field: String,
}

/// Scan `contents` for signature placeholders: every `{{ … }}` token
/// whose trimmed inner text contains a `.`. Data placeholders without
/// a dot are ignored. The split is on the *first* dot, so `{{a.b.c}}`
/// yields `signer = "a"`, `field = "b.c"` (which the rule then flags
/// as an unknown field).
#[must_use]
pub fn signature_placeholders(contents: &str) -> Vec<SignaturePlaceholder> {
    let bytes = contents.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'{' && bytes[i + 1] == b'{' {
            if let Some(rel) = contents[i + 2..].find("}}") {
                let end = i + 2 + rel + 2;
                let inner = contents[i + 2..i + 2 + rel].trim();
                if let Some((signer, field)) = inner.split_once('.') {
                    out.push(SignaturePlaceholder {
                        offset: i,
                        end,
                        signer: signer.trim().to_string(),
                        field: field.trim().to_string(),
                    });
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Lexical variables introduced by `{{#for <var> in <state>}}` blocks.
fn loop_variables(contents: &str) -> std::collections::BTreeSet<String> {
    let mut vars = std::collections::BTreeSet::new();
    let mut rest = contents;
    while let Some(start) = rest.find(FOR_OPEN) {
        let after_open = &rest[start + FOR_OPEN.len()..];
        let Some(header_len) = after_open.find("}}") else {
            break;
        };
        let header = after_open[..header_len].trim();
        if let Some((var, _state)) = header.split_once(" in ") {
            vars.insert(var.trim().to_string());
        }
        rest = &after_open[header_len + 2..];
    }
    vars
}

#[derive(Debug, Deserialize)]
struct FrontmatterShape {
    #[serde(default)]
    workflow: Option<BTreeMap<String, BTreeMap<String, String>>>,
    #[serde(default)]
    questionnaire: Option<BTreeMap<String, BTreeMap<String, String>>>,
}

/// 1-based line number containing the byte at `offset`.
fn line_at(contents: &str, offset: usize) -> usize {
    contents[..offset.min(contents.len())]
        .bytes()
        .filter(|b| *b == b'\n')
        .count()
        + 1
}

impl Rule for F107SignaturePlaceholders {
    fn code(&self) -> &'static str {
        Self::CODE
    }

    fn lint(&self, file: &SourceFile) -> Vec<Violation> {
        let Some(fm) = frontmatter::extract(&file.contents) else {
            return Vec::new();
        };

        let mut violations = Vec::new();
        let set = match signer_set(&file.contents) {
            Ok(set) => set,
            Err(message) => {
                violations.push(n107(file, 1, 0..0, message));
                SignerSet::implicit_default()
            }
        };

        let parsed = serde_yaml::from_str::<FrontmatterShape>(fm).ok();
        let questionnaire_states: std::collections::BTreeSet<&str> = parsed
            .as_ref()
            .and_then(|p| p.questionnaire.as_ref())
            .into_iter()
            .flat_map(|q| q.keys().map(String::as_str))
            .collect();
        let loops = loop_variables(&file.contents);
        let scan = scan_placeholders(file, &set, &questionnaire_states, &loops);
        violations.extend(scan.violations);
        if set.explicit {
            require_declared_placeholders(file, &set, &scan.signed_roles, &mut violations);
        }
        cross_check_signing_state(
            file,
            parsed,
            scan.saw_valid,
            scan.saw_relevant,
            &mut violations,
        );
        violations
    }
}

struct PlaceholderScan {
    violations: Vec<Violation>,
    saw_valid: bool,
    saw_relevant: bool,
    signed_roles: std::collections::BTreeSet<String>,
}

fn n107(
    file: &SourceFile,
    line: usize,
    range: std::ops::Range<usize>,
    message: impl Into<String>,
) -> Violation {
    Violation {
        code: F107SignaturePlaceholders::CODE,
        path: file.path.clone(),
        line,
        range,
        message: message.into(),
    }
}

fn scan_placeholders(
    file: &SourceFile,
    set: &SignerSet,
    questionnaire_states: &std::collections::BTreeSet<&str>,
    loops: &std::collections::BTreeSet<String>,
) -> PlaceholderScan {
    let mut scan = PlaceholderScan {
        violations: Vec::new(),
        saw_valid: false,
        saw_relevant: false,
        signed_roles: std::collections::BTreeSet::new(),
    };
    for ph in signature_placeholders(&file.contents) {
        if questionnaire_states.contains(ph.signer.as_str()) || loops.contains(ph.signer.as_str()) {
            continue;
        }
        scan.saw_relevant = true;
        let signer_ok = set.contains(&ph.signer);
        let field_ok = F107SignaturePlaceholders::FIELDS.contains(&ph.field.as_str());
        let line = line_at(&file.contents, ph.offset);
        let range = ph.offset..ph.offset;
        if !signer_ok {
            scan.violations.push(n107(
                file,
                line,
                range.clone(),
                format!(
                    "unknown signer role `{}` in signature placeholder (expected one of: {})",
                    ph.signer,
                    set.joined()
                ),
            ));
        }
        if !field_ok {
            scan.violations.push(n107(
                file,
                line,
                range,
                format!(
                    "unknown signature field `{}` in signature placeholder (expected one of: {})",
                    ph.field,
                    F107SignaturePlaceholders::FIELDS.join(", ")
                ),
            ));
        }
        if signer_ok && field_ok {
            scan.saw_valid = true;
            if ph.field == "signature" || ph.field == "initials" {
                scan.signed_roles.insert(ph.signer.clone());
            }
        }
    }
    scan
}

fn require_declared_placeholders(
    file: &SourceFile,
    set: &SignerSet,
    signed_roles: &std::collections::BTreeSet<String>,
    violations: &mut Vec<Violation>,
) {
    for role in &set.roles {
        if !signed_roles.contains(role.as_str()) {
            violations.push(n107(
                file,
                1,
                0..0,
                format!(
                    "declared signer role `{role}` has no placeholder in the body \
                     (expected `{{{{{role}.signature}}}}` or `{{{{{role}.initials}}}}`)"
                ),
            ));
        }
    }
}

fn cross_check_signing_state(
    file: &SourceFile,
    parsed: Option<FrontmatterShape>,
    saw_valid: bool,
    saw_relevant: bool,
    violations: &mut Vec<Violation>,
) {
    let has_signing_state = parsed.and_then(|p| p.workflow).is_some_and(|wf| {
        wf.keys().any(|state| {
            state == F107SignaturePlaceholders::SIGNING_STATE
                || state.starts_with(&format!("{}__", F107SignaturePlaceholders::SIGNING_STATE))
        })
    });
    if saw_valid && !has_signing_state {
        violations.push(n107(
            file,
            1,
            0..0,
            format!(
                "template draws a signature block but its workflow has no \
                 `{}` (or `{}__*`) state to collect the signature",
                F107SignaturePlaceholders::SIGNING_STATE,
                F107SignaturePlaceholders::SIGNING_STATE
            ),
        ));
    }
    if has_signing_state && !saw_relevant {
        violations.push(n107(
            file,
            1,
            0..0,
            format!(
                "workflow declares a `{}` state but the body carries no signature \
                 anchor (expected at least one `{{{{<signer>.<field>}}}}`, e.g. \
                 `{{{{client.signature}}}}`) for the tab to land on",
                F107SignaturePlaceholders::SIGNING_STATE
            ),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::{signature_placeholders, F107SignaturePlaceholders};
    use crate::{Rule, SourceFile};
    use std::path::PathBuf;

    fn file(body: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("retainer.md"),
            contents: body.to_string(),
        }
    }

    /// A frontmatter block with a valid signing workflow, plus whatever
    /// body the test appends.
    fn with_signing_workflow(body: &str) -> String {
        format!(
            "---\ntitle: Retainer\nworkflow:\n  BEGIN:\n    created: sent_for_signature__pending\n  \
             sent_for_signature__pending:\n    signature_received: END\n  END: {{}}\n---\n{body}"
        )
    }

    #[test]
    fn parser_ignores_data_placeholders_without_a_dot() {
        let found = signature_placeholders("Hello {{client_name}} and {{project_name}}.");
        assert!(found.is_empty(), "data placeholders have no dot: {found:?}");
    }

    #[test]
    fn parser_extracts_signer_and_field_split_on_first_dot() {
        let found = signature_placeholders("{{client.signature}} {{firm.date}}");
        assert_eq!(found.len(), 2);
        assert_eq!(
            (found[0].signer.as_str(), found[0].field.as_str()),
            ("client", "signature")
        );
        assert_eq!(
            (found[1].signer.as_str(), found[1].field.as_str()),
            ("firm", "date")
        );
    }

    #[test]
    fn parser_offset_and_end_span_the_whole_token() {
        let body = "x {{client.signature}} y";
        let found = signature_placeholders(body);
        assert_eq!(found.len(), 1);
        assert_eq!(&body[found[0].offset..found[0].end], "{{client.signature}}");
    }

    #[test]
    fn parser_trims_inner_whitespace() {
        let found = signature_placeholders("{{  client.signature  }}");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].signer, "client");
        assert_eq!(found[0].field, "signature");
    }

    #[test]
    fn loop_variable_paths_are_not_signature_placeholders() {
        let body = "---\nquestionnaire:\n  BEGIN:\n    _: people__members\n  people__members:\n    _: END\n  END: {}\nworkflow:\n  BEGIN:\n    intake_submitted: lawyer_review\n  lawyer_review:\n    approved: END\n  END: {}\n---\n{{#for m in people__members}}{{m.name}} from {{m.city}}{{/for}}";
        assert!(
            F107SignaturePlaceholders.lint(&file(body)).is_empty(),
            "loop row fields are N115 data grammar, not N107 signature grammar"
        );
    }

    #[test]
    fn passes_valid_client_and_firm_blocks_with_signing_state() {
        let body = with_signing_workflow(
            "Signature: {{client.signature}} {{client.date}}\nCountersigned: {{firm.signature}}\n",
        );
        assert!(
            F107SignaturePlaceholders.lint(&file(&body)).is_empty(),
            "well-formed signature blocks with a signing state should pass",
        );
    }

    #[test]
    fn passes_initials_block() {
        let body = with_signing_workflow("Initials: {{client.initials}}\n");
        assert!(F107SignaturePlaceholders.lint(&file(&body)).is_empty());
    }

    #[test]
    fn no_frontmatter_means_no_violation() {
        // Arbitrary prose with a dotted token is NOT a template.
        let v = F107SignaturePlaceholders.lint(&file("see {{config.value}} in the docs"));
        assert!(v.is_empty());
    }

    #[test]
    fn flags_unknown_signer_role() {
        let body = with_signing_workflow("{{spouse.signature}}\n");
        let v = F107SignaturePlaceholders.lint(&file(&body));
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].code, "N107");
        assert!(v[0].message.contains("unknown signer role `spouse`"));
    }

    #[test]
    fn flags_unknown_field_type() {
        let body = with_signing_workflow("{{client.fingerprint}}\n");
        let v = F107SignaturePlaceholders.lint(&file(&body));
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0]
            .message
            .contains("unknown signature field `fingerprint`"));
    }

    #[test]
    fn rejects_person_name_signer() {
        // Role, never a name: `{{nick.signature}}` is not a valid block.
        let body = with_signing_workflow("{{nick.signature}}\n");
        let v = F107SignaturePlaceholders.lint(&file(&body));
        assert_eq!(v.len(), 1);
        assert!(v[0].message.contains("unknown signer role `nick`"));
    }

    #[test]
    fn flags_signature_block_without_signing_workflow_state() {
        // Valid grammar, but the workflow has no place to collect it.
        let body = "---\ntitle: Retainer\nworkflow:\n  BEGIN:\n    created: lawyer_review\n  \
                    lawyer_review:\n    approved: END\n  END: {}\n---\n{{client.signature}}\n";
        let v = F107SignaturePlaceholders.lint(&file(body));
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].message.contains("no `sent_for_signature`"));
        assert_eq!(v[0].line, 1);
    }

    #[test]
    fn flags_signing_state_without_a_body_anchor() {
        // Reverse direction: the workflow declares a signing state but
        // the body has no signature anchor for the tab to land on — the
        // mirror of the live retainer bug (signing step, no placed tab).
        let body = "---\ntitle: Retainer\nworkflow:\n  BEGIN:\n    \
                    created: sent_for_signature__pending\n  \
                    sent_for_signature__pending:\n    signature_received: END\n  \
                    END: {}\n---\nDear {{client_name}}, please sign offline.\n";
        let v = F107SignaturePlaceholders.lint(&file(body));
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].code, "N107");
        assert!(
            v[0].message.contains("no signature anchor"),
            "message was: {}",
            v[0].message
        );
        assert_eq!(v[0].line, 1);
    }

    #[test]
    fn signing_state_with_a_malformed_anchor_does_not_double_flag_reverse() {
        // A present-but-malformed token draws its own unknown-signer
        // violation; the reverse "no anchor" check must stay silent so
        // the author gets one clear message, not a contradictory pair.
        let body = "---\ntitle: R\nworkflow:\n  BEGIN:\n    \
                    created: sent_for_signature\n  sent_for_signature:\n    \
                    signature_received: END\n  END: {}\n---\n{{spouse.signature}}\n";
        let v = F107SignaturePlaceholders.lint(&file(body));
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].message.contains("unknown signer role"));
        assert!(
            !v.iter().any(|x| x.message.contains("no signature anchor")),
            "reverse check must not fire when a (malformed) anchor is present",
        );
    }

    #[test]
    fn signing_state_with_only_loop_tokens_still_flags_missing_anchor() {
        // A signing workflow whose body uses a `{{#for}}` loop but forgets
        // its real signature block. The loop row tokens (`{{m.name}}`) are
        // N115 data grammar, not signature anchors, so the reverse check
        // must still fire — else `expand_signatures` places zero tabs and
        // the template reaches the provider with a signing step and nowhere
        // to sign (the very bug this guard exists to catch).
        let body = "---\ntitle: Retainer\nquestionnaire:\n  BEGIN:\n    _: people__members\n  \
                    people__members:\n    _: END\n  END: {}\nworkflow:\n  BEGIN:\n    \
                    created: sent_for_signature__pending\n  \
                    sent_for_signature__pending:\n    signature_received: END\n  \
                    END: {}\n---\nMembers:\n{{#for m in people__members}}- {{m.name}}\n{{/for}}\n";
        let v = F107SignaturePlaceholders.lint(&file(body));
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].code, "N107");
        assert!(
            v[0].message.contains("no signature anchor"),
            "message was: {}",
            v[0].message
        );
        assert_eq!(v[0].line, 1);
    }

    #[test]
    fn bare_sent_for_signature_state_satisfies_cross_check() {
        let body = "---\ntitle: R\nworkflow:\n  BEGIN:\n    created: sent_for_signature\n  \
                    sent_for_signature:\n    signature_received: END\n  END: {}\n---\n\
                    {{client.signature}}\n";
        assert!(F107SignaturePlaceholders.lint(&file(body)).is_empty());
    }

    #[test]
    fn reports_violation_at_the_token_line() {
        let body = with_signing_workflow("line one\nline two has {{client.oops}}\n");
        let v = F107SignaturePlaceholders.lint(&file(&body));
        assert_eq!(v.len(), 1);
        // The frontmatter is 6 lines; body line "line one" follows, then
        // the offending token is on the next line. Assert it is NOT 1.
        assert!(
            v[0].line > 1,
            "violation should point at the token, not line 1"
        );
    }

    #[test]
    fn no_signature_blocks_means_workflow_cross_check_is_silent() {
        // A template with only data placeholders never triggers the
        // signing-state requirement.
        let body = "---\ntitle: T\nworkflow:\n  BEGIN:\n    created: END\n  END: {}\n---\n\
                    Dear {{client_name}}, welcome.\n";
        assert!(F107SignaturePlaceholders.lint(&file(body)).is_empty());
    }

    /// A three-party assignment: client, firm, and a confirming assignor.
    /// Questionnaire states back the respondent-side roles; `firm` is the
    /// configured countersignature. Fixtures use the sample transactional
    /// matter, never a live Project code.
    fn three_party_assignment(signers_yaml: &str, extra_body: &str) -> String {
        format!(
            "---\ntitle: Assignment\n{signers_yaml}questionnaire:\n  BEGIN:\n    \
             _: person__client\n  person__client:\n    _: person__confirming_assignor\n  \
             person__confirming_assignor:\n    _: END\n  END: {{}}\nworkflow:\n  BEGIN:\n    \
             created: sent_for_signature__pending\n  sent_for_signature__pending:\n    \
             signature_received: END\n  END: {{}}\n---\n\
             {{{{client.signature}}}}\n{{{{firm.signature}}}}\n{{{{confirming_assignor.signature}}}}\n\
             {extra_body}"
        )
    }

    #[test]
    fn declared_three_party_signer_set_validates() {
        let body = three_party_assignment(
            "signers:\n  - client\n  - firm\n  - confirming_assignor\n",
            "",
        );
        assert!(
            F107SignaturePlaceholders.lint(&file(&body)).is_empty(),
            "a template that declares its three signer roles must lint clean: {:?}",
            F107SignaturePlaceholders.lint(&file(&body)),
        );
    }

    #[test]
    fn absent_signers_rejects_a_third_role_and_names_the_permitted_set() {
        let body = three_party_assignment("", "");
        let v = F107SignaturePlaceholders.lint(&file(&body));
        assert!(
            v.iter().any(|x| {
                x.code == "N107"
                    && x.message
                        .contains("unknown signer role `confirming_assignor`")
                    && x.message.contains("client, firm")
            }),
            "absent signers: must permit only client, firm; got {v:?}"
        );
    }

    #[test]
    fn declared_role_without_a_placeholder_is_an_error() {
        let body =
            "---\ntitle: Assignment\nsigners:\n  - client\n  - firm\n  - confirming_assignor\n\
                    questionnaire:\n  BEGIN:\n    _: person__client\n  person__client:\n    \
                    _: person__confirming_assignor\n  person__confirming_assignor:\n    _: END\n  \
                    END: {}\nworkflow:\n  BEGIN:\n    created: sent_for_signature__pending\n  \
                    sent_for_signature__pending:\n    signature_received: END\n  END: {}\n---\n\
                    {{client.signature}}\n{{firm.signature}}\n";
        let v = F107SignaturePlaceholders.lint(&file(body));
        assert!(
            v.iter().any(|x| {
                x.code == "N107"
                    && x.message.contains("confirming_assignor")
                    && x.message.contains("no placeholder")
            }),
            "a declared role with no body placeholder must fail N107; got {v:?}"
        );
    }

    #[test]
    fn shipped_onboarding_letter_validates_without_a_signers_key() {
        let body = include_str!("../../templates/notations/neon_law/shared/onboarding_letter.md");
        assert!(
            F107SignaturePlaceholders.lint(&file(body)).is_empty(),
            "the bundled onboarding letter must stay valid with the default signer set: {:?}",
            F107SignaturePlaceholders.lint(&file(body)),
        );
    }

    #[test]
    fn shipped_offboarding_letter_validates_without_a_signers_key() {
        let body = include_str!("../../templates/notations/neon_law/shared/offboarding_letter.md");
        assert!(
            F107SignaturePlaceholders.lint(&file(body)).is_empty(),
            "the bundled closing letter must stay valid with the default signer set: {:?}",
            F107SignaturePlaceholders.lint(&file(body)),
        );
    }
}
