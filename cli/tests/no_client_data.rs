//! The no-client-data gate — a static scan that keeps client contact PII out
//! of the repository, enforced by the workspace test suite.
//!
//! Invariant (`AGENTS.md`): the repo holds the firm's OWN data and synthetic
//! fixtures — never a client's. The only human-identifying contact data that
//! may ship is the firm's (`@neonlaw.com` and `@neonlaw.org`), and Nick Shook's
//! (`shook.family`); every other address must be a reserved synthetic domain
//! (`example.com`, or a `.example` / `.invalid` / `.test` address, per RFC
//! 2606 / RFC 6761). Anything else is a client-data suspect and fails the
//! gate.
//!
//! ## Scope — the shipped data surfaces
//!
//! A real client's email or phone number leaks through the surfaces that
//! carry *data*: seeded rows, the example fills baked into a notation
//! template, and the published marketing / portal content. Those are
//! [`DATA_SURFACES`]. Source and test code are deliberately out of scope —
//! they carry synthetic fixtures (`a@b.com`, `nick@gmail.com` in a negative
//! test) that are reviewed by humans and the `AGENTS.md` checklist, not by a
//! domain allowlist that would drown the signal in fixture noise.
//!
//! ## What it flags
//!
//! - **`NCD-EMAIL`** — an email whose domain is not [`is_allowed_email_domain`].
//! - **`NCD-PHONE`** — a US-shaped domestic number, or a `+`-prefixed
//!   international number. The firm's own voice line is brand identity and
//!   lives in `views::brand` (`Branding::firm_phone`), never in a data
//!   surface, so a phone number here is a person's, not the firm's.
//!
//! The gate lives here, not behind a CLI subcommand: its only caller was one
//! CI step, and [`shipped_data_surfaces_hold_no_client_contact_data`] runs the
//! identical scan over the real tree inside the required workspace test job.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

/// Directories, relative to the scan root, that ship data to real people and
/// must therefore never carry a client's contact information.
const DATA_SURFACES: &[&str] = &["store/seeds", "templates", "server/content"];

/// Text extensions worth reading. Everything else in a data surface (a
/// rendered PDF, an image, a font) is skipped — it is not human-authored copy.
const TEXT_EXTENSIONS: &[&str] = &[
    "md", "markdown", "yaml", "yml", "json", "toml", "txt", "html", "csv", "fields",
];

/// Domains whose local part identifies the firm or its principal, matched
/// against the address's registrable domain and any subdomain of it.
///
/// One entry per host that actually exists, and there are three.
/// `neonlaw.com` is the firm's own host, where it reads its mail
/// (`views::brand::support_domain`); `neonlaw.org` is the firm's `.org`, which
/// carries the contributions address `NOTICE` and `CONTRIBUTING.md` publish;
/// `shook.family` is the principal's family trust. An address on any of them is
/// the firm's, never a client's.
const FIRM_DOMAINS: &[&str] = &["neonlaw.com", "neonlaw.org", "shook.family"];

/// One suspected client-data leak, located at a file and line.
#[derive(Debug, PartialEq, Eq)]
struct Finding {
    path: PathBuf,
    line: usize,
    code: &'static str,
    value: String,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{} {} {:?}",
            self.path.display(),
            self.line,
            self.code,
            self.value
        )
    }
}

/// Walk every [`DATA_SURFACES`] directory under `root` and collect findings.
/// A missing surface is skipped, not an error, so the gate runs from any cwd.
fn scan(root: &Path) -> std::io::Result<Vec<Finding>> {
    let mut findings = Vec::new();
    for surface in DATA_SURFACES {
        let dir = root.join(surface);
        if !dir.exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&dir) {
            let entry = entry?;
            if !entry.file_type().is_file() || !is_text_path(entry.path()) {
                continue;
            }
            let content = std::fs::read_to_string(entry.path())?;
            // Report paths relative to the scan root so output is stable
            // regardless of the absolute cwd (CI vs. a worktree).
            let rel = entry.path().strip_prefix(root).unwrap_or(entry.path());
            findings.extend(scan_content(rel, &content));
        }
    }
    Ok(findings)
}

/// Extract every client-data finding from one file's `content`.
fn scan_content(path: &Path, content: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let line_no = idx + 1;
        for (email, domain) in find_emails(line) {
            if !is_allowed_email_domain(&domain) {
                findings.push(Finding {
                    path: path.to_path_buf(),
                    line: line_no,
                    code: "NCD-EMAIL",
                    value: email,
                });
            }
        }
        for phone in find_phones(line) {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: line_no,
                code: "NCD-PHONE",
                value: phone,
            });
        }
    }
    findings
}

/// Whether `domain` (already lowercased) identifies the firm, its principal,
/// or one of the documented reserved synthetic domains.
fn is_allowed_email_domain(domain: &str) -> bool {
    if [".example", ".invalid", ".test"]
        .iter()
        .any(|suffix| domain.ends_with(suffix))
    {
        return true;
    }
    if domain == "example.com" {
        return true;
    }
    FIRM_DOMAINS
        .iter()
        .any(|base| domain == *base || domain.ends_with(&format!(".{base}")))
}

/// Find every email address in `line`, returning each with its lowercased
/// registrable-or-subdomain string (trailing dots trimmed). Anchored on `@`
/// so a bare word is never mistaken for an address.
fn find_emails(line: &str) -> Vec<(String, String)> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    for (at, &b) in bytes.iter().enumerate() {
        if b != b'@' {
            continue;
        }
        let mut start = at;
        while start > 0 && is_local_char(bytes[start - 1]) {
            start -= 1;
        }
        let mut end = at + 1;
        while end < bytes.len() && is_domain_char(bytes[end]) {
            end += 1;
        }
        // Empty local part or empty domain is not an address.
        if start == at || end == at + 1 {
            continue;
        }
        let domain_raw = line[at + 1..end].trim_end_matches('.');
        // Require a dotted domain ending in an alphabetic TLD of length >= 2,
        // so `@`-in-a-handle or `foo@bar` (no dot) is not a false positive.
        let tld = domain_raw.rsplit('.').next().unwrap_or("");
        if !domain_raw.contains('.')
            || tld.len() < 2
            || !tld.bytes().all(|c| c.is_ascii_alphabetic())
        {
            continue;
        }
        out.push((
            line[start..end].to_string(),
            domain_raw.to_ascii_lowercase(),
        ));
    }
    out
}

fn is_local_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'%' | b'+' | b'-')
}

fn is_domain_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-')
}

/// Find every phone number shape we enforce. Strict on purpose: domestic
/// numbers must use separated groups, and international numbers must start
/// with `+` and contain an E.164-sized digit count, so bare ids or amounts are
/// not false positives.
fn find_phones(line: &str) -> Vec<String> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if let Some(len) = match_phone(bytes, i) {
            out.push(line[i..i + len].to_string());
            i += len;
        } else {
            i += 1;
        }
    }
    out
}

/// Try to match a phone number anchored at `start`. Returns its byte length.
fn match_phone(b: &[u8], start: usize) -> Option<usize> {
    // Reject a match that begins in the middle of a longer number.
    if start > 0 && (b[start - 1].is_ascii_digit() || b[start - 1] == b'+') {
        return None;
    }
    if b[start] == b'+' {
        return match_international_phone(b, start);
    }
    let mut i = start;
    // Optional `1` country code, only when a separator follows.
    if i < b.len()
        && b[i] == b'1'
        && i + 1 < b.len()
        && (is_phone_sep(b[i + 1]) || b[i + 1] == b'(')
    {
        i += 1;
        while i < b.len() && is_phone_sep(b[i]) {
            i += 1;
        }
    }
    // Area code, optionally parenthesized.
    let paren = i < b.len() && b[i] == b'(';
    if paren {
        i += 1;
    }
    if !take_digits(b, &mut i, 3) {
        return None;
    }
    if paren {
        if i < b.len() && b[i] == b')' {
            i += 1;
        } else {
            return None;
        }
    }
    // A separator is required unless the area code was parenthesized.
    if !take_sep(b, &mut i) && !paren {
        return None;
    }
    if !take_digits(b, &mut i, 3) {
        return None;
    }
    if !take_sep(b, &mut i) {
        return None;
    }
    if !take_digits(b, &mut i, 4) {
        return None;
    }
    // Reject if a further digit follows (this was part of a longer number).
    if i < b.len() && b[i].is_ascii_digit() {
        return None;
    }
    Some(i - start)
}

fn is_phone_sep(b: u8) -> bool {
    matches!(b, b'-' | b'.' | b' ')
}

/// Match a phone anchored at a `+` at `start`. Only `+` anchors an
/// international number: it never appears in an id, UUID, or version string, so
/// it is an unambiguous signal. A bare `00` dial prefix is deliberately *not*
/// matched — it collides with the repo's zero-padded synthetic ids (a seed
/// UUID like `0000-000000000001` is not a phone number).
fn match_international_phone(b: &[u8], start: usize) -> Option<usize> {
    if start + 1 >= b.len() || !b[start + 1].is_ascii_digit() {
        return None;
    }

    let mut i = start + 1;
    let mut digits = 0;
    let mut saw_separator = false;
    let mut pending_separator = false;
    while i < b.len() {
        match b[i] {
            c if c.is_ascii_digit() => {
                if pending_separator {
                    saw_separator = true;
                    pending_separator = false;
                }
                digits += 1;
                i += 1;
            }
            b'-' | b'.' | b' ' | b'(' | b')' => {
                if digits > 0 {
                    pending_separator = true;
                }
                i += 1;
            }
            _ => break,
        }
    }

    while i > start + 1 && is_international_trailing_sep(b[i - 1]) {
        i -= 1;
    }

    // E.164 caps the total at 15 digits. A separator confirms phone formatting
    // even at the 7-digit floor (Niue/Tokelau: a 3-digit country code plus a
    // 4-digit subscriber number). Without one, only a full-length run — a
    // country code plus a complete national number, 11+ digits, the canonical
    // separator-free E.164 form like `+14155550123` — is confidently a phone
    // and not a signed integer such as `+12345678`.
    if !(7..=15).contains(&digits) {
        return None;
    }
    if !saw_separator && digits < 11 {
        return None;
    }
    if i < b.len() && (b[i].is_ascii_digit() || b[i].is_ascii_alphabetic()) {
        return None;
    }
    Some(i - start)
}

fn is_international_trailing_sep(b: u8) -> bool {
    matches!(b, b'-' | b'.' | b' ' | b'(' | b')')
}

/// Advance `i` past exactly `n` ASCII digits; return whether all `n` matched.
fn take_digits(b: &[u8], i: &mut usize, n: usize) -> bool {
    if *i + n > b.len() || !b[*i..*i + n].iter().all(u8::is_ascii_digit) {
        return false;
    }
    *i += n;
    true
}

/// Advance `i` past one phone separator; return whether one was consumed.
fn take_sep(b: &[u8], i: &mut usize) -> bool {
    if *i < b.len() && is_phone_sep(b[*i]) {
        *i += 1;
        true
    } else {
        false
    }
}

fn is_text_path(path: &Path) -> bool {
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|ext| {
            let ext = ext.to_ascii_lowercase();
            TEXT_EXTENSIONS.contains(&ext.as_str())
        })
}

/// The workspace root, resolved from this crate's manifest directory so the
/// gate scans the real tree no matter which cwd the test runner uses.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cli/ has a parent workspace root")
        .to_path_buf()
}

/// Write `<root>/<rel>` (creating parents).
fn write_file(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// Render findings one per line so a red run names every leak without the
/// operator re-running anything.
fn describe(findings: &[Finding]) -> String {
    findings
        .iter()
        .map(|f| format!("  {f}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// **The gate.** Scan the shipped data surfaces of this repository and fail on
/// any client contact data. This is the enforcement point: it runs in the
/// required workspace test job on every push.
#[test]
fn shipped_data_surfaces_hold_no_client_contact_data() {
    let root = workspace_root();
    let findings = scan(&root).expect("scan the workspace data surfaces");

    assert!(
        findings.is_empty(),
        "{} client-data finding(s) in {} — the repo must never hold a client's \
         contact information (see AGENTS.md); use a firm-owned address or a \
         reserved example domain:\n{}",
        findings.len(),
        DATA_SURFACES.join(", "),
        describe(&findings)
    );
}

#[test]
fn passes_when_data_surfaces_hold_only_firm_and_synthetic_data() {
    let work = TempDir::new().unwrap();
    write_file(
        work.path(),
        "store/seeds/people.yaml",
        "- email: nick@neonlaw.com\n- email: libra@example.com\n",
    );
    write_file(
        work.path(),
        "templates/retainer.md",
        "Questions to **support@neonlaw.com**.\n",
    );
    write_file(
        work.path(),
        "server/content/marketing/about.md",
        "Reach the firm at support@neonlaw.com.\n",
    );

    assert_eq!(scan(work.path()).unwrap(), Vec::new());
}

#[test]
fn reports_a_client_email_with_its_file_and_line() {
    let work = TempDir::new().unwrap();
    write_file(
        work.path(),
        "store/seeds/people.yaml",
        "- email: nick@neonlaw.com\n- email: jane.doe@gmail.com\n",
    );

    let findings = scan(work.path()).unwrap();
    let rendered = describe(&findings);
    assert_eq!(findings.len(), 1, "{rendered}");
    assert!(rendered.contains("jane.doe@gmail.com"), "{rendered}");
    assert!(rendered.contains("NCD-EMAIL"), "{rendered}");
    assert!(rendered.contains("people.yaml:2"), "{rendered}");
    // The allowlisted firm address on line 1 must not be reported.
    assert!(!rendered.contains("nick@neonlaw.com"), "{rendered}");
}

#[test]
fn flags_policy_banned_reserved_domains() {
    let work = TempDir::new().unwrap();
    write_file(
        work.path(),
        "store/seeds/people.yaml",
        "- email: client@example.net\n- email: admin@foo.localhost\n",
    );

    let rendered = describe(&scan(work.path()).unwrap());
    assert!(rendered.contains("client@example.net"), "{rendered}");
    assert!(rendered.contains("admin@foo.localhost"), "{rendered}");
    assert!(rendered.contains("NCD-EMAIL"), "{rendered}");
}

#[test]
fn reports_a_domestic_phone_number() {
    let work = TempDir::new().unwrap();
    write_file(
        work.path(),
        "server/content/marketing/contact.md",
        "Call us at 702-555-0134.\n",
    );

    let rendered = describe(&scan(work.path()).unwrap());
    assert!(rendered.contains("NCD-PHONE"), "{rendered}");
    assert!(rendered.contains("702-555-0134"), "{rendered}");
}

#[test]
fn reports_an_international_phone_number() {
    let work = TempDir::new().unwrap();
    write_file(
        work.path(),
        "server/content/marketing/contact.md",
        "Call us at +44 20 7946 0958.\n",
    );

    let rendered = describe(&scan(work.path()).unwrap());
    assert!(rendered.contains("NCD-PHONE"), "{rendered}");
    assert!(rendered.contains("+44 20 7946 0958"), "{rendered}");
}

#[test]
fn reports_a_short_international_phone_number() {
    let work = TempDir::new().unwrap();
    write_file(
        work.path(),
        "server/content/marketing/contact.md",
        "Reach the island office at +683 1234.\n",
    );

    let rendered = describe(&scan(work.path()).unwrap());
    assert!(rendered.contains("NCD-PHONE"), "{rendered}");
    assert!(rendered.contains("+683 1234"), "{rendered}");
}

#[test]
fn reports_a_separatorless_e164_number() {
    let work = TempDir::new().unwrap();
    // Canonical E.164 form (no separators) — how a phone lands in a data field.
    write_file(
        work.path(),
        "store/seeds/contacts.yaml",
        "- phone: \"+14155550123\"\n",
    );

    let rendered = describe(&scan(work.path()).unwrap());
    assert!(rendered.contains("NCD-PHONE"), "{rendered}");
    assert!(rendered.contains("+14155550123"), "{rendered}");
}

#[test]
fn errors_on_undecodable_data() {
    let work = TempDir::new().unwrap();
    // A text-extension file in a data surface that is not valid UTF-8 is an
    // I/O failure, not a clean scan: the scan returns `Err`, distinct from a
    // finding, so a corrupt surface can never read as "no client data".
    let path = work.path().join("store/seeds/broken.yaml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();

    assert!(scan(work.path()).is_err(), "undecodable data must error");
}

#[test]
fn scans_template_fields_sidecars() {
    let work = TempDir::new().unwrap();
    write_file(
        work.path(),
        "templates/notations/forms/example.fields",
        "Emergency contact: jane.doe@gmail.com\n",
    );

    let rendered = describe(&scan(work.path()).unwrap());
    assert!(rendered.contains("example.fields:1"), "{rendered}");
    assert!(rendered.contains("jane.doe@gmail.com"), "{rendered}");
}

#[test]
fn scans_compound_suffix_fields_toml_sidecars() {
    let work = TempDir::new().unwrap();
    // Some form sidecars are named `*.fields.toml`; `Path::extension()` returns
    // `toml` for those, which the scanner reads. A leak there must be caught.
    write_file(
        work.path(),
        "templates/notations/forms/us/nv__trust_formation.fields.toml",
        "emergency_contact = \"jane.doe@gmail.com\"\n",
    );

    let rendered = describe(&scan(work.path()).unwrap());
    assert!(
        rendered.contains("nv__trust_formation.fields.toml:1"),
        "{rendered}"
    );
    assert!(rendered.contains("jane.doe@gmail.com"), "{rendered}");
}

#[test]
fn ignores_files_outside_the_data_surfaces() {
    let work = TempDir::new().unwrap();
    // A client-shaped email in source/test code is out of scope — those carry
    // synthetic fixtures reviewed by the AGENTS.md checklist, not this gate.
    write_file(
        work.path(),
        "web/src/oauth.rs",
        "let e = \"a@gmail.com\";\n",
    );
    write_file(work.path(), "docs/example.md", "mail jane@gmail.com\n");

    assert_eq!(scan(work.path()).unwrap(), Vec::new());
}

#[test]
fn passes_on_an_empty_tree() {
    let work = TempDir::new().unwrap();
    assert_eq!(scan(work.path()).unwrap(), Vec::new());
}

#[test]
fn firm_and_principal_domains_are_allowed() {
    assert!(is_allowed_email_domain("neonlaw.com"));
    assert!(is_allowed_email_domain("mail.neonlaw.com"));
    assert!(is_allowed_email_domain("neonlaw.com"));
    assert!(is_allowed_email_domain("neonlaw.org"));
    assert!(is_allowed_email_domain("parse.neonlaw.com"));
    assert!(is_allowed_email_domain("shook.family"));
}

#[test]
fn reserved_synthetic_domains_are_allowed() {
    for d in [
        "example.com",
        "acme.example",
        "your-domain.example",
        "test.invalid",
        "evil.test",
    ] {
        assert!(is_allowed_email_domain(d), "{d} should be allowed");
    }
}

#[test]
fn a_real_third_party_domain_is_not_allowed() {
    assert!(!is_allowed_email_domain("example.org"));
    assert!(!is_allowed_email_domain("example.net"));
    assert!(!is_allowed_email_domain("localhost"));
    assert!(!is_allowed_email_domain("foo.localhost"));
    assert!(!is_allowed_email_domain("gmail.com"));
    assert!(!is_allowed_email_domain("outlook.com"));
    assert!(!is_allowed_email_domain("acme.com"));
    // A look-alike that only suffixes the firm word must not pass.
    assert!(!is_allowed_email_domain("notneonlaw.com"));
    assert!(!is_allowed_email_domain("neonlaw.com.evil.co"));
}

#[test]
fn find_emails_extracts_address_and_lowercased_domain() {
    let hits = find_emails("Contact Libra <Libra@Example.COM> today.");
    assert_eq!(
        hits,
        vec![("Libra@Example.COM".into(), "example.com".into())]
    );
}

#[test]
fn find_emails_ignores_non_addresses() {
    assert!(find_emails("a handle @nobody and foo@bar with no tld").is_empty());
    assert!(find_emails("no at sign here").is_empty());
}

#[test]
fn scan_content_flags_a_client_email_only() {
    let body = "firm: support@neonlaw.com\nclient: jane.doe@gmail.com\nexample: x@acme.example";
    let findings = scan_content(Path::new("store/seeds/people.yaml"), body);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].code, "NCD-EMAIL");
    assert_eq!(findings[0].value, "jane.doe@gmail.com");
    assert_eq!(findings[0].line, 2);
}

#[test]
fn find_phones_matches_common_us_shapes() {
    assert_eq!(find_phones("call 702-555-0134 now"), vec!["702-555-0134"]);
    assert_eq!(find_phones("(702) 555-0134"), vec!["(702) 555-0134"]);
    assert_eq!(find_phones("+1 702.555.0134"), vec!["+1 702.555.0134"]);
    assert_eq!(find_phones("1-702-555-0134"), vec!["1-702-555-0134"]);
}

#[test]
fn find_phones_matches_plus_prefixed_international_shapes() {
    assert_eq!(
        find_phones("client mobile +44 20 7946 0958"),
        vec!["+44 20 7946 0958"]
    );
    assert_eq!(
        find_phones("office +33 (1) 42 68 53 00."),
        vec!["+33 (1) 42 68 53 00"]
    );
}

#[test]
fn find_phones_ignores_zero_padded_ids() {
    // A `+`-less digit run — even a zero-padded, separated one like a seed
    // UUID — is not a phone. `+` is the only international anchor precisely
    // so these do not false-positive.
    assert!(find_phones("id 0000-000000000001").is_empty());
    assert!(find_phones("uuid 00000000-0000-0000-0000-000000000001").is_empty());
}

#[test]
fn find_phones_matches_separatorless_e164() {
    // Canonical E.164 storage form: `+` and a full country-code + national
    // number, no separators. This is how a phone lands in a data column.
    assert_eq!(find_phones("phone +14155550123 end"), vec!["+14155550123"]);
    assert_eq!(find_phones("uk +447911123456."), vec!["+447911123456"]);
}

#[test]
fn find_phones_matches_short_plus_prefixed_numbers() {
    // A 3-digit country code plus a 4-digit subscriber number (Niue): 7
    // digits total, `+`-prefixed and separated — the shortest live shape.
    assert_eq!(find_phones("client +683 1234 reachable"), vec!["+683 1234"]);
}

#[test]
fn find_phones_ignores_non_phone_digit_runs() {
    assert!(find_phones("id 12345678901234").is_empty());
    assert!(find_phones("amount 1234567890").is_empty());
    assert!(find_phones("version 1.2.3").is_empty());
    assert!(find_phones("sha 7027-555-01340").is_empty());
    assert!(find_phones("math +12345678 with no separators").is_empty());
    // Below the 7-digit floor: an amount, not a phone.
    assert!(find_phones("delta +1 234.56 today").is_empty());
    // A bare `+` with no digit after it is not a number.
    assert!(find_phones("a +/- b").is_empty());
    // Over the 15-digit E.164 ceiling.
    assert!(find_phones("+1 234 567 890 123 456").is_empty());
}

#[test]
fn find_phones_rejects_partial_domestic_shapes() {
    // Area code, then a group that is not three digits.
    assert!(find_phones("code 702-55-0134").is_empty());
    // Parenthesized area code with no closing paren.
    assert!(find_phones("(702 55-0134").is_empty());
    // A trailing extra digit means it was part of a longer run.
    assert!(find_phones("702-555-01340").is_empty());
    // A final group of fewer than four digits is not a phone.
    assert!(find_phones("702-555-013 end").is_empty());
    // An international match that runs straight into a letter is rejected.
    assert!(find_phones("+44 20 7946 0958x").is_empty());
}

#[test]
fn scan_content_flags_a_phone_number() {
    let findings = scan_content(Path::new("templates/x.md"), "reach me at 702-555-0134");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].code, "NCD-PHONE");
    assert_eq!(findings[0].value, "702-555-0134");
}

#[test]
fn is_text_path_selects_authored_copy() {
    assert!(is_text_path(Path::new("a/b.md")));
    assert!(is_text_path(Path::new("a/b.YAML")));
    assert!(is_text_path(Path::new("a/b.fields")));
    // A compound-suffix sidecar is read via its final `toml` extension.
    assert!(is_text_path(Path::new("a/b.fields.toml")));
    assert!(!is_text_path(Path::new("a/b.pdf")));
    assert!(!is_text_path(Path::new("a/b")));
}
