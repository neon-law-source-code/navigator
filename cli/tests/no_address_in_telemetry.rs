//! The no-address-in-telemetry gate — a static scan keeping email addresses
//! out of `tracing` structured fields, enforced by the workspace test suite.
//!
//! ## The invariant
//!
//! Telemetry leaves the firm's trust boundary and an email address is
//! client-identifying content, so the standing order in `telemetry/src/lib.rs`
//! is that an address must never reach a log field. What those fields actually
//! needed the address *for* is carried instead by a sanitizer in
//! `portal/src/audit_fields.rs`: an opaque `person_id` for who the caller is,
//! and the address's `domain` for which organization they came from.
//!
//! ## Why a scan rather than review
//!
//! Line coverage cannot see this. Every call site that emits a field executes
//! inside the existing suites, so coverage is satisfied while the invariant is
//! completely unasserted — a change that reintroduces an address leaves the
//! suites green. Reading cannot see it reliably either: a manual sweep of these
//! sites missed two, one crate apart, and both were found mechanically in
//! seconds.
//!
//! So the control is mechanical and lives in the required workspace test job.
//!
//! ## What it flags
//!
//! A `tracing::{trace,debug,info,warn,error}!` field whose **value expression**
//! mentions `email`. The field name is not the signal — `principal_kind` is
//! fine, and a field innocently named `recipient` carrying `p.email` is not —
//! so the scan reads the right-hand side.
//!
//! ## What it deliberately does not flag
//!
//! - **A sanitizer's return value.** `principal_kind(principal_email)` yields
//!   `"authenticated"` and `domain_of(&email.from)` yields `example.com`.
//!   These are the *fix*, so flagging them would make the gate fight the
//!   invariant it enforces. See `SANITIZERS`.
//! - **The macro's message string.** Prose mentioning the word is not a field.
//!   String literals are removed before a line is read, so this holds for a
//!   single-line invocation whose message follows the fields on the same line.
//! - **Anything outside a `tracing!` invocation.** A struct field, a variable,
//!   or a database column named `email` is ordinary code; only the telemetry
//!   boundary is in scope.
//! - **Test sources.** They carry synthetic fixtures by design, reviewed by
//!   humans rather than by this scan.
//!
//! ## Known limits
//!
//! It reads text, not an AST. A field assembled through an intermediate
//! binding — `let who = p.email;` then `who = %who` — is not caught, and
//! neither would a macro imported unqualified (`use tracing::warn;` then
//! `warn!(…)`); no crate does that today. The gate is a ratchet on new code
//! rather than a proof of absence. It also says nothing about the export path
//! beyond these source call sites: the selected direct `OpenObserve` subscriber
//! has its own runtime enforcement, which is why this static ratchet remains a
//! separate, narrower control.

use std::path::{Path, PathBuf};

/// The `tracing` macros that emit structured fields.
const TRACING_MACROS: &[&str] = &[
    "tracing::trace!",
    "tracing::debug!",
    "tracing::info!",
    "tracing::warn!",
    "tracing::error!",
];

/// Directories under the workspace root that are not shipped source.
const SKIPPED_DIRS: &[&str] = &[
    "target",
    ".git",
    ".worktrees",
    "node_modules",
    "vendor",
    "tests",
    "examples",
];

/// Sites that emit an address today and are known.
///
/// **Empty, and the goal is that it stays that way.** It exists as a ratchet
/// rather than a permission: were an entry ever added, `known_sites_are_still_flagged`
/// would hold it to documenting a live defect, and a listed file is a file the
/// gate no longer guards at all — including against a *different* leak added
/// later. That is a real cost, so prefer sanitizing the site.
const KNOWN_SITES: &[&str] = &[];

/// One flagged field, located for a human.
#[derive(Debug)]
struct Finding {
    file: String,
    line: usize,
    text: String,
}

/// Strip string literals and trailing line comments, leaving code.
///
/// The message string is the easy place for the word to appear innocently, and
/// on a single-line invocation it sits on the same line as the fields — so
/// removing literals is what lets one line hold both. It also keeps a
/// parenthesis inside a literal from skewing the depth walk.
///
/// Char literals are left alone deliberately: `'` also opens a lifetime
/// (`&'static str`), and treating it as a delimiter would swallow the rest of
/// the line.
fn code_only(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        match c {
            // Consume the escaped character so `\"` does not close the literal.
            '\\' if in_string => {
                chars.next();
            }
            '"' => in_string = !in_string,
            '/' if !in_string && chars.peek() == Some(&'/') => break,
            _ if !in_string => out.push(c),
            _ => {}
        }
    }
    out
}

/// Drop a `tracing::warn!(` prefix so the first field on the macro's own line
/// is read as a field.
///
/// Without this a single-line invocation is invisible: the name side of its
/// first `=` is `tracing::warn!(to`, which is not a bare identifier, so the
/// whole line is dismissed.
fn strip_macro_prefix(code: &str) -> &str {
    for macro_name in TRACING_MACROS {
        if let Some(pos) = code.find(macro_name) {
            let rest = &code[pos + macro_name.len()..];
            return rest.strip_prefix('(').unwrap_or(rest);
        }
    }
    code
}

/// A character that may appear in a `tracing` field name.
fn is_field_char(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'.'
}

/// How far one field's value expression runs: to the next comma that is not
/// nested inside a call, or to the invocation's own closing paren.
///
/// Bounding the value matters on a line carrying several fields. Reading to
/// end-of-line instead would let one field's sanitizer vouch for another
/// field's raw address — `who = %p.email, kind = %principal_kind(addr)` would
/// see `principal_kind(` in `who`'s value and wave the leak through.
fn value_extent(rest: &str) -> &str {
    let mut depth: usize = 0;
    for (idx, c) in rest.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            // At depth zero this closes the macro invocation itself.
            ')' | ']' | '}' => {
                if depth == 0 {
                    return &rest[..idx];
                }
                depth -= 1;
            }
            ',' if depth == 0 => return &rest[..idx],
            _ => {}
        }
    }
    rest
}

/// Does this line assign a `tracing` field from an expression mentioning an
/// address?
///
/// Shape: `name = <expr>`, where `<expr>` may carry a `%` or `?` sigil. Every
/// `=` on the line is considered, not just the first, because a single-line
/// invocation puts several fields — and often a `target:` — on one line. The
/// message string is gone by the time this runs, and `==` is rejected because
/// it is a comparison rather than a field.
fn flags_line(line: &str) -> bool {
    let stripped = code_only(line);
    let code = strip_macro_prefix(&stripped);
    let bytes = code.as_bytes();

    for (eq, _) in code.match_indices('=') {
        // The name is the identifier run immediately before the `=`. An empty
        // run means this `=` is not a field assignment: the second half of
        // `==`, or the `!`/`>`/`<` of a comparison.
        let mut end = eq;
        while end > 0 && bytes[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        let mut start = end;
        while start > 0 && is_field_char(bytes[start - 1]) {
            start -= 1;
        }
        if start == end {
            continue;
        }
        let value = value_extent(&code[eq + 1..]);
        // `==` is a comparison, not a field.
        if value.trim_start().starts_with('=') {
            continue;
        }
        let value = value.to_ascii_lowercase();
        if !value.contains("email") {
            continue;
        }
        // `email` in the expression is necessary but not sufficient: a struct
        // named `email` has fields that are not addresses, an env-var name is a
        // string, and a sanitizer takes an address precisely so it can return
        // something else. Exclude the expressions that demonstrably reduce to a
        // non-address, so the gate flags the address itself rather than every
        // mention of the word.
        if !SANITIZERS
            .iter()
            .chain(NON_ADDRESS_MARKERS)
            .any(|marker| value.contains(marker))
        {
            return true;
        }
    }
    false
}

/// Calls whose return value is provably not an address, however address-shaped
/// the argument is.
///
/// These are the invariant's own remedy, so the gate must recognize them or it
/// would report every fixed site as a defect and pressure the next author to
/// undo the fix. Each is a total function to a non-address:
/// `principal_kind` returns the static `"anonymous"` or `"authenticated"`
/// (`portal/src/a2a.rs`), and `domain_of` returns the domain half with the
/// identifying local part removed (`portal/src/audit_fields.rs`).
///
/// Adding an entry here is a real widening — it must be a function that
/// *cannot* return its argument, verified by reading it, not a name that merely
/// sounds safe.
const SANITIZERS: &[&str] = &["principal_kind(", "domain_of("];

/// Substrings proving a value expression is not an address, even though it
/// mentions one.
///
/// Each entry earned its place against a real site in this workspace:
/// `email.person_id` and `email.template_slug` read other fields off a message
/// struct, `inbound_email_secret.is_some()` is a boolean, `email.dkim` is a
/// verification result, `email.attachments.len()` is a count, and
/// `env::var("NAVIGATOR_EMAIL_BACKEND")` names a variable rather than a person.
const NON_ADDRESS_MARKERS: &[&str] = &[
    "_id",
    "_slug",
    "_secret",
    "dkim",
    "env::var",
    ".len()",
    ".count()",
    ".is_some()",
    ".is_none()",
];

/// Scan one file's `tracing!` invocations.
///
/// Tracks parenthesis depth from the macro name so the walk ends at the real
/// close rather than at the first `)` inside an argument.
fn scan_source(rel: &str, body: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut depth: usize = 0;
    let mut inside = false;

    for (idx, line) in body.lines().enumerate() {
        let code = code_only(line);
        if !inside && TRACING_MACROS.iter().any(|m| code.contains(m)) {
            inside = true;
            depth = 0;
        }
        if !inside {
            continue;
        }
        if flags_line(line) {
            findings.push(Finding {
                file: rel.to_string(),
                line: idx + 1,
                text: line.trim().to_string(),
            });
        }
        depth += code.matches('(').count();
        depth = depth.saturating_sub(code.matches(')').count());
        if depth == 0 {
            inside = false;
        }
    }
    findings
}

/// Walk the workspace's shipped Rust sources.
fn scan(root: &Path) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                if !SKIPPED_DIRS.contains(&name.as_str()) && !name.starts_with('.') {
                    stack.push(path);
                }
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let Ok(body) = std::fs::read_to_string(&path) else {
                continue;
            };
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            findings.extend(scan_source(&rel, &body));
        }
    }
    findings
}

/// The workspace root, resolved from this crate's manifest directory so the
/// gate scans the real tree no matter which cwd the test runner uses.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cli/ has a parent workspace root")
        .to_path_buf()
}

fn describe(findings: &[&Finding]) -> String {
    findings
        .iter()
        .map(|f| format!("  {}:{} — {}", f.file, f.line, f.text))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn tracing_fields_carry_no_email_address() {
    let root = workspace_root();
    let findings = scan(&root);
    let unexpected: Vec<&Finding> = findings
        .iter()
        .filter(|f| !KNOWN_SITES.contains(&f.file.as_str()))
        .collect();

    assert!(
        unexpected.is_empty(),
        "{} tracing field(s) carry an email address. Telemetry leaves the \
         firm's trust boundary, so an address must never reach a log field \
         (telemetry/src/lib.rs). Use the sanitizers in \
         portal/src/audit_fields.rs: `person_id` for who the caller is, \
         `domain_of` for where they came from:\n{}",
        unexpected.len(),
        describe(&unexpected)
    );
}

/// The ratchet must not rust shut: an entry that no longer flags anything is a
/// fixed site, and leaving it listed would silently re-permit a regression
/// there. Vacuous while `KNOWN_SITES` is empty, which is the state to keep.
#[test]
fn known_sites_are_still_flagged() {
    let root = workspace_root();
    let findings = scan(&root);
    for site in KNOWN_SITES {
        assert!(
            findings.iter().any(|f| f.file == *site),
            "{site} is listed in KNOWN_SITES but flags nothing — it has been \
             fixed, so delete the entry rather than leaving the gate open on \
             that file"
        );
    }
}

mod line_shapes {
    use super::flags_line;

    #[test]
    fn flags_an_address_bearing_field() {
        assert!(flags_line("            author = author.email,"));
        assert!(flags_line(
            r#"    principal = principal.map_or("<anonymous>", |p| p.email.as_str()),"#
        ));
        assert!(flags_line("    recipient = %p.email,"));
        assert!(flags_line("    who = ?person.email_address,"));
    }

    /// A whole invocation on one line is the shape a field-name-based check
    /// misses, and it is as much a leak as the multi-line form.
    #[test]
    fn flags_a_single_line_invocation() {
        assert!(flags_line(
            r#"        tracing::warn!(to = %p.email, "notify: send failed");"#
        ));
        assert!(flags_line(
            r#"    tracing::info!(target: "audit", actor = %claims.email, "seen");"#
        ));
    }

    /// A message string mentioning the word is prose even when it shares the
    /// line with real fields.
    #[test]
    fn ignores_a_message_string_beside_real_fields() {
        assert!(!flags_line(
            r#"        tracing::warn!(error = %e, person_id = %person_id, "email-confirm: gate send failed");"#
        ));
        assert!(!flags_line(
            r#"    tracing::info!(events = events.len(), key = %key, "email-events: persisted batch");"#
        ));
    }

    /// The sanitized shape is the fix. Flagging it would make the gate demand
    /// its own invariant be reverted, so these are the exact expressions the
    /// tree emits today.
    #[test]
    fn ignores_a_field_that_only_reports_a_kind() {
        assert!(!flags_line(
            "    principal_kind = %principal_kind(principal_email),"
        ));
        assert!(!flags_line(
            "    proposer_kind = %principal_kind(&pending.principal_email),"
        ));
        assert!(!flags_line(
            "    principal_kind = principal_kind(principal.map_or(ANONYMOUS_PRINCIPAL, |p| p.email.as_str())),"
        ));
        assert!(!flags_line("    person_id = %person_id_field(approver),"));
    }

    /// The domain half names an organization, not a person.
    #[test]
    fn ignores_a_field_that_only_reports_a_domain() {
        assert!(!flags_line("    from_domain = %domain_of(&email.from),"));
        assert!(!flags_line("    to_domain = %domain_of(&email.to),"));
        assert!(!flags_line(
            "    got_domain = %domain_of(info.email.as_deref().unwrap_or_default()),"
        ));
    }

    /// The macro's message is prose, not a field.
    #[test]
    fn ignores_the_message_string() {
        assert!(!flags_line(
            r#"        "a2a: refusing an email address in a log field","#
        ));
    }

    /// A comparison is not an assignment.
    #[test]
    fn ignores_comparisons_and_comments() {
        assert!(!flags_line("    if principal_email == other.email {"));
        assert!(!flags_line("    // the email never reaches a field"));
    }
}
