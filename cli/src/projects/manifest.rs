//! Project-repository `navigator.yaml` — one accepted key set, one reader.
//!
//! `navigator validate` and `navigator site projects drift` share this list so
//! a key one side honours cannot be a key the other refuses. Unknown keys are
//! refused by name. There is no per-repository exemption key.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The filename a Project repository declares itself in.
pub const FILE: &str = "navigator.yaml";
/// The retired `.yml` spelling. A file of this name is a rename, not a second
/// schema.
pub const RETIRED_FILE: &str = "navigator.yml";

/// `Y004` — `host` must be a hostname.
pub const HOST_CODE: &str = "Y004";
/// `Y005` — `project` must be a Navigator Project code.
pub const PROJECT_CODE: &str = "Y005";
/// `Y006` — a top-level key outside the accepted set.
pub const UNKNOWN_KEY_CODE: &str = "Y006";
/// `Y007` — `no_live_row` must be a non-empty reason string.
pub const ROWLESS_CODE: &str = "Y007";
/// `Y008` — the manifest is `navigator.yaml`, not `navigator.yml`.
pub const RENAME_CODE: &str = "Y008";
/// `Y011` — a Project manifest must not carry YAML comment tokens.
pub const COMMENT_CODE: &str = "Y011";

/// Every top-level key `navigator.yaml` may carry.
///
/// Drift's reader deserializes this same set (and only this set of *named*
/// fields). Adding a key means adding it here, in [`Manifest`], and in the
/// covering test that asserts the two agree.
pub const ACCEPTED_KEYS: &[&str] = &[
    "allowed_hosts",
    "allowed_links",
    "allowed_prefixes",
    "host",
    "no_live_row",
    "project",
];

/// A Project repository's root manifest.
///
/// Unknown keys are not stored: [`lint`] refuses them before a caller reads
/// this struct. The `no_live_row` field stays untyped so `true` cannot coerce
/// into a plausible reason.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct Manifest {
    pub host: Option<String>,
    pub project: Option<String>,
    pub no_live_row: Option<serde_yaml::Value>,
    #[serde(default)]
    pub allowed_hosts: BTreeMap<String, String>,
    #[serde(default)]
    pub allowed_links: BTreeMap<String, String>,
    #[serde(default)]
    pub allowed_prefixes: BTreeMap<String, String>,
}

/// One manifest finding, with a Y-family rule code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestFinding {
    pub path: PathBuf,
    pub line: usize,
    pub code: &'static str,
    pub message: String,
}

impl ManifestFinding {
    pub(crate) fn at(
        path: &Path,
        line: usize,
        code: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            path: path.to_path_buf(),
            line,
            code,
            message: message.into(),
        }
    }
}

/// Lint the Project manifest at `root`, if one is present.
///
/// A missing `navigator.yaml` is not a finding here — layout reports that
/// when the tree is a Project repository. A root `navigator.yml` is always
/// a rename finding.
#[must_use]
pub fn lint(root: &Path) -> Vec<ManifestFinding> {
    let mut findings = Vec::new();
    let retired = root.join(RETIRED_FILE);
    if retired.is_file() {
        findings.push(ManifestFinding::at(
            &retired,
            1,
            RENAME_CODE,
            "the manifest is navigator.yaml, rename it",
        ));
    }
    let path = root.join(FILE);
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return findings;
    };
    findings.extend(lint_contents(&path, &contents));
    findings
}

/// YAML comment tokens, not a `#` grep: quoted and block scalars keep their
/// hashes as content. Reasons belong on the pull request and in the contract.
fn lint_comments(path: &Path, contents: &str) -> Vec<ManifestFinding> {
    yaml_comment_lines(contents)
        .into_iter()
        .map(|line| {
            ManifestFinding::at(
                path,
                line,
                COMMENT_CODE,
                "navigator.yaml must not contain comments; record the reason \
                 in the pull request that adds the entry and in the repository \
                 contract",
            )
        })
        .collect()
}

/// 1-based lines that a YAML scanner treats as comments.
fn yaml_comment_lines(contents: &str) -> Vec<usize> {
    let contents = contents.strip_prefix('\u{feff}').unwrap_or(contents);
    let bytes = contents.as_bytes();
    let mut i = 0;
    let mut line = 1;
    let mut lines = Vec::new();
    while i < bytes.len() {
        match bytes[i] {
            b'\'' if is_quote_start(bytes, i) => skip_single_quoted(bytes, &mut i, &mut line),
            b'"' if is_quote_start(bytes, i) => skip_double_quoted(bytes, &mut i, &mut line),
            b'|' | b'>' if is_block_scalar_header(bytes, i) => {
                if let Some(comment_line) = comment_on_rest_of_line(bytes, i, line) {
                    lines.push(comment_line);
                }
                skip_block_scalar(bytes, &mut i, &mut line);
            }
            b'#' if is_comment_start(bytes, i) => {
                lines.push(line);
                skip_to_eol(bytes, &mut i, &mut line);
            }
            b'\n' => {
                line += 1;
                i += 1;
            }
            b'\r' => {
                i += 1;
                if i < bytes.len() && bytes[i] == b'\n' {
                    i += 1;
                }
                line += 1;
            }
            _ => i += 1,
        }
    }
    lines
}

fn is_comment_start(bytes: &[u8], i: usize) -> bool {
    i == 0
        || bytes[i - 1] == b' '
        || bytes[i - 1] == b'\t'
        || bytes[i - 1] == b'\n'
        || bytes[i - 1] == b'\r'
}

/// A quote opens a scalar only at a token boundary. An apostrophe inside a
/// plain value (`the author's closed file`) is content; treating it as a
/// quote start would skip the rest of the document and hide a later comment.
fn is_quote_start(bytes: &[u8], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    matches!(
        bytes[i - 1],
        b':' | b'[' | b'{' | b',' | b' ' | b'\t' | b'\n' | b'\r'
    )
}

fn comment_on_rest_of_line(bytes: &[u8], mut i: usize, line: usize) -> Option<usize> {
    while i < bytes.len() {
        match bytes[i] {
            b'\n' | b'\r' => return None,
            b'#' if is_comment_start(bytes, i) => return Some(line),
            _ => i += 1,
        }
    }
    None
}

fn is_block_scalar_header(bytes: &[u8], i: usize) -> bool {
    let Ok(rest) = std::str::from_utf8(&bytes[i..]) else {
        return false;
    };
    let line = rest
        .split('\n')
        .next()
        .unwrap_or(rest)
        .trim_end_matches('\r');
    let mut chars = line.chars();
    match chars.next() {
        Some('|' | '>') => {}
        _ => return false,
    }
    for ch in chars.by_ref() {
        match ch {
            '0'..='9' | '+' | '-' | ' ' | '\t' => {}
            '#' => return true,
            _ => return false,
        }
    }
    true
}

fn skip_to_eol(bytes: &[u8], i: &mut usize, line: &mut usize) {
    while *i < bytes.len() {
        match bytes[*i] {
            b'\n' => {
                *line += 1;
                *i += 1;
                return;
            }
            b'\r' => {
                *i += 1;
                if *i < bytes.len() && bytes[*i] == b'\n' {
                    *i += 1;
                }
                *line += 1;
                return;
            }
            _ => *i += 1,
        }
    }
}

fn skip_single_quoted(bytes: &[u8], i: &mut usize, line: &mut usize) {
    *i += 1;
    while *i < bytes.len() {
        match bytes[*i] {
            b'\'' => {
                *i += 1;
                if *i < bytes.len() && bytes[*i] == b'\'' {
                    *i += 1;
                    continue;
                }
                return;
            }
            b'\n' => {
                *line += 1;
                *i += 1;
            }
            b'\r' => {
                *i += 1;
                if *i < bytes.len() && bytes[*i] == b'\n' {
                    *i += 1;
                }
                *line += 1;
            }
            _ => *i += 1,
        }
    }
}

fn skip_double_quoted(bytes: &[u8], i: &mut usize, line: &mut usize) {
    *i += 1;
    while *i < bytes.len() {
        match bytes[*i] {
            b'\\' => {
                *i += 1;
                if *i < bytes.len() {
                    if bytes[*i] == b'\n' {
                        *line += 1;
                    } else if bytes[*i] == b'\r' {
                        *i += 1;
                        if *i < bytes.len() && bytes[*i] == b'\n' {
                            *i += 1;
                        }
                        *line += 1;
                        continue;
                    }
                    *i += 1;
                }
            }
            b'"' => {
                *i += 1;
                return;
            }
            b'\n' => {
                *line += 1;
                *i += 1;
            }
            b'\r' => {
                *i += 1;
                if *i < bytes.len() && bytes[*i] == b'\n' {
                    *i += 1;
                }
                *line += 1;
            }
            _ => *i += 1,
        }
    }
}

fn skip_block_scalar(bytes: &[u8], i: &mut usize, line: &mut usize) {
    skip_to_eol(bytes, i, line);
    let mut content_indent: Option<usize> = None;
    while *i < bytes.len() {
        let start = *i;
        let indent = leading_indent(bytes, *i);
        if !line_is_blank(bytes, *i) {
            match content_indent {
                None => content_indent = Some(indent),
                Some(min) if indent < min => {
                    *i = start;
                    return;
                }
                Some(_) => {}
            }
        }
        skip_to_eol(bytes, i, line);
    }
}

fn leading_indent(bytes: &[u8], mut i: usize) -> usize {
    let mut n = 0;
    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' => {
                n += 1;
                i += 1;
            }
            _ => break,
        }
    }
    n
}

fn line_is_blank(bytes: &[u8], mut i: usize) -> bool {
    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' => i += 1,
            b'\n' | b'\r' => return true,
            _ => return false,
        }
    }
    true
}

/// Lint already-read YAML. Used by tests and by [`lint`].
#[must_use]
pub fn lint_contents(path: &Path, contents: &str) -> Vec<ManifestFinding> {
    let mut findings = lint_comments(path, contents);
    let document: serde_yaml::Value = match serde_yaml::from_str(contents) {
        Ok(document) => document,
        Err(_) => return findings,
    };
    let Some(mapping) = document.as_mapping() else {
        findings.push(ManifestFinding::at(
            path,
            1,
            UNKNOWN_KEY_CODE,
            format!(
                "navigator.yaml must be a mapping of {}; got a non-mapping document",
                accepted_keys_phrase()
            ),
        ));
        return findings;
    };
    findings.extend(lint_keys(path, mapping));
    findings.extend(lint_host(path, mapping));
    findings.extend(lint_project(path, mapping));
    findings.extend(lint_rowless(path, mapping));
    if let Ok(manifest) = parse(contents) {
        findings.extend(lint_allowlists(path, &manifest));
    }
    findings
}

/// Allowlist map values are claims: each host or prefix needs a non-empty
/// reason. The maps themselves are what the origin scanner reads.
fn lint_allowlists(path: &Path, manifest: &Manifest) -> Vec<ManifestFinding> {
    let mut findings = Vec::new();
    for (host, reason) in &manifest.allowed_hosts {
        if reason.trim().is_empty() {
            findings.push(ManifestFinding::at(
                path,
                1,
                UNKNOWN_KEY_CODE,
                format!("`allowed_hosts` entry `{host}` must give a reason"),
            ));
        }
    }
    for (prefix, reason) in &manifest.allowed_prefixes {
        if reason.trim().is_empty() {
            findings.push(ManifestFinding::at(
                path,
                1,
                UNKNOWN_KEY_CODE,
                format!("`allowed_prefixes` entry `{prefix}` must give a reason"),
            ));
        }
    }
    for (host, reason) in &manifest.allowed_links {
        if reason.trim().is_empty() {
            findings.push(ManifestFinding::at(
                path,
                1,
                UNKNOWN_KEY_CODE,
                format!("`allowed_links` entry `{host}` must give a reason"),
            ));
        }
    }
    let _ = manifest.host.as_deref();
    findings
}

fn lint_keys(path: &Path, mapping: &serde_yaml::Mapping) -> Vec<ManifestFinding> {
    let mut findings = Vec::new();
    for key in mapping.keys() {
        let Some(name) = key.as_str() else {
            findings.push(ManifestFinding::at(
                path,
                1,
                UNKNOWN_KEY_CODE,
                format!(
                    "navigator.yaml top-level keys must be strings; expected one of {}",
                    accepted_keys_phrase()
                ),
            ));
            continue;
        };
        if !ACCEPTED_KEYS.contains(&name) {
            findings.push(ManifestFinding::at(
                path,
                1,
                UNKNOWN_KEY_CODE,
                format!(
                    "unknown key `{name}`; expected one of {}",
                    accepted_keys_phrase()
                ),
            ));
        }
    }
    findings
}

fn lint_host(path: &Path, mapping: &serde_yaml::Mapping) -> Vec<ManifestFinding> {
    match mapping.get("host") {
        Some(host) => match host_string(host) {
            None => vec![ManifestFinding::at(
                path,
                1,
                HOST_CODE,
                "`host` must be a hostname",
            )],
            Some(host) if !is_hostname(&host) => vec![ManifestFinding::at(
                path,
                1,
                HOST_CODE,
                format!("`host: {host}` is not a hostname"),
            )],
            Some(_) => Vec::new(),
        },
        None => vec![ManifestFinding::at(
            path,
            1,
            HOST_CODE,
            "`host` is required and must be a hostname",
        )],
    }
}

fn lint_project(path: &Path, mapping: &serde_yaml::Mapping) -> Vec<ManifestFinding> {
    match mapping.get("project").and_then(scalar_string) {
        Some(code) if store::projects::is_valid_code(&code) => Vec::new(),
        Some(code) => vec![ManifestFinding::at(
            path,
            1,
            PROJECT_CODE,
            format!("`project: {code}` is not a valid Project code"),
        )],
        None if mapping.contains_key("project") => vec![ManifestFinding::at(
            path,
            1,
            PROJECT_CODE,
            "`project` must be a Project code",
        )],
        None => vec![ManifestFinding::at(
            path,
            1,
            PROJECT_CODE,
            "`project` is required and must be a valid Project code",
        )],
    }
}

fn lint_rowless(path: &Path, mapping: &serde_yaml::Mapping) -> Vec<ManifestFinding> {
    match mapping.get("no_live_row") {
        None => Vec::new(),
        Some(serde_yaml::Value::String(reason)) if !reason.trim().is_empty() => Vec::new(),
        Some(serde_yaml::Value::String(_)) => vec![ManifestFinding::at(
            path,
            1,
            ROWLESS_CODE,
            "`no_live_row:` must give the reason this repository has no live row",
        )],
        Some(_) => vec![ManifestFinding::at(
            path,
            1,
            ROWLESS_CODE,
            "`no_live_row:` must be the reason this repository has no live row, written as text",
        )],
    }
}

/// Parse a manifest that has already passed [`lint_contents`] (or a fixture
/// known to be well-shaped). Unknown keys are ignored here the way serde
/// default-denies nothing: callers that need the closed set call [`lint`].
pub fn parse(contents: &str) -> Result<Manifest, String> {
    serde_yaml::from_str(contents).map_err(|error| format!("not valid YAML: {error}"))
}

fn accepted_keys_phrase() -> String {
    ACCEPTED_KEYS.join(", ")
}

fn scalar_string(value: &serde_yaml::Value) -> Option<String> {
    value
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn host_string(value: &serde_yaml::Value) -> Option<String> {
    scalar_string(value)
}

/// A hostname: labels of letters, digits, and hyphens, joined by dots, with
/// no empty first (or any) label. No scheme, port, or path.
pub fn is_hostname(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    if host.starts_with('.') || host.ends_with('.') {
        return false;
    }
    if host.contains('/') || host.contains(':') || host.contains(' ') {
        return false;
    }
    host.split('.').all(is_dns_label)
}

fn is_dns_label(label: &str) -> bool {
    let bytes = label.as_bytes();
    if bytes.is_empty() || bytes.len() > 63 {
        return false;
    }
    let first = bytes[0];
    let last = bytes[bytes.len() - 1];
    if !first.is_ascii_alphanumeric() || !last.is_ascii_alphanumeric() {
        return false;
    }
    bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || *b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn codes(contents: &str) -> Vec<&'static str> {
        lint_contents(Path::new("navigator.yaml"), contents)
            .into_iter()
            .map(|f| f.code)
            .collect()
    }

    #[test]
    fn accepted_keys_and_drift_reader_agree() {
        // Drift deserializes these named fields on `drift::Manifest`. Keep the
        // lists identical so a new key cannot land on one side only.
        const DRIFT_READER_KEYS: &[&str] = &[
            "allowed_hosts",
            "allowed_links",
            "allowed_prefixes",
            "host",
            "no_live_row",
            "project",
        ];
        assert_eq!(
            BTreeSet::from_iter(ACCEPTED_KEYS.iter().copied()),
            BTreeSet::from_iter(DRIFT_READER_KEYS.iter().copied())
        );
        let yaml = concat!(
            "host: staging.neonlaw.com\n",
            "project: acme\n",
            "no_live_row: the matter closed\n",
            "allowed_hosts:\n",
            "  www.w3.org: XML namespace\n",
            "allowed_links:\n",
            "  courts.example: civil-procedure citation\n",
            "allowed_prefixes:\n",
            "  \"https://react.dev/errors/\": React minified errors\n",
        );
        let parsed = parse(yaml).expect("accepted keys deserialize");
        assert_eq!(parsed.host.as_deref(), Some("staging.neonlaw.com"));
        assert_eq!(parsed.project.as_deref(), Some("acme"));
        assert_eq!(parsed.allowed_hosts.len(), 1);
        assert_eq!(parsed.allowed_links.len(), 1);
        assert_eq!(parsed.allowed_prefixes.len(), 1);
        assert!(codes(yaml).is_empty());
    }

    #[test]
    fn host_must_be_a_hostname() {
        assert!(codes("host: https://staging.neonlaw.com\nproject: acme\n").contains(&HOST_CODE));
        assert!(codes("host: staging.neonlaw.com/app\nproject: acme\n").contains(&HOST_CODE));
        assert!(codes("host: .neonlaw.com\nproject: acme\n").contains(&HOST_CODE));
        assert!(codes("project: acme\n").contains(&HOST_CODE));
        assert!(!codes("host: staging.neonlaw.com\nproject: acme\n").contains(&HOST_CODE));
        assert!(is_hostname("staging.neonlaw.com"));
        assert!(is_hostname("localhost"));
        assert!(!is_hostname(".test"));
        assert!(!is_hostname("example.com:443"));
    }

    #[test]
    fn project_must_be_a_valid_code() {
        assert!(codes("host: staging.neonlaw.com\nproject: Not A Code\n").contains(&PROJECT_CODE));
        assert!(codes("host: staging.neonlaw.com\n").contains(&PROJECT_CODE));
        assert!(!codes("host: staging.neonlaw.com\nproject: acme\n").contains(&PROJECT_CODE));
    }

    #[test]
    fn unknown_key_names_the_known_set() {
        let findings = lint_contents(
            Path::new("navigator.yaml"),
            "host: staging.neonlaw.com\nproject: acme\nexempt_roots: [docs]\n",
        );
        let unknown = findings
            .iter()
            .find(|f| f.code == UNKNOWN_KEY_CODE)
            .expect("unknown key");
        assert!(unknown.message.contains("exempt_roots"));
        assert!(unknown.message.contains("host"));
        assert!(unknown.message.contains("project"));
        assert!(unknown.message.contains("no_live_row"));
        assert!(unknown.message.contains("allowed_hosts"));
        assert!(unknown.message.contains("allowed_links"));
        assert!(unknown.message.contains("allowed_prefixes"));
    }

    #[test]
    fn no_live_row_must_be_a_non_empty_reason() {
        let boolean = lint_contents(
            Path::new("navigator.yaml"),
            "host: staging.neonlaw.com\nproject: acme\nno_live_row: true\n",
        );
        assert!(boolean
            .iter()
            .any(|f| f.code == ROWLESS_CODE && f.message.contains("written as text")));
        let empty = lint_contents(
            Path::new("navigator.yaml"),
            "host: staging.neonlaw.com\nproject: acme\nno_live_row: \"\"\n",
        );
        assert!(empty
            .iter()
            .any(|f| f.code == ROWLESS_CODE && f.message.contains("must give the reason")));
        assert!(codes(
            "host: staging.neonlaw.com\nproject: acme\nno_live_row: the matter closed\n"
        )
        .is_empty());
    }

    #[test]
    fn a_comment_token_is_refused_and_a_hash_in_a_quoted_scalar_is_not() {
        let commented = lint_contents(
            Path::new("navigator.yaml"),
            "host: staging.neonlaw.com\n# record the exemption in the pull request\nproject: acme\n",
        );
        let finding = commented
            .iter()
            .find(|f| f.code == COMMENT_CODE)
            .expect("comment token");
        assert_eq!(finding.line, 2);
        assert!(
            finding.message.contains("pull request"),
            "{}",
            finding.message
        );
        assert!(
            finding.message.contains("repository contract"),
            "{}",
            finding.message
        );
        let trailing = lint_contents(
            Path::new("navigator.yaml"),
            "host: staging.neonlaw.com  # hostname of the deployment\nproject: acme\n",
        );
        assert!(
            trailing
                .iter()
                .any(|f| f.code == COMMENT_CODE && f.line == 1),
            "{trailing:?}"
        );
        let quoted = concat!(
            "host: staging.neonlaw.com\n",
            "project: acme\n",
            "allowed_prefixes:\n",
            "  \"https://react.dev/errors/#\": React minified errors\n",
        );
        assert!(
            !codes(quoted).contains(&COMMENT_CODE),
            "{:?}",
            lint_contents(Path::new("navigator.yaml"), quoted)
        );
        let quoted_value = "host: staging.neonlaw.com\nproject: \"acme # synthetic\"\n";
        assert!(!codes(quoted_value).contains(&COMMENT_CODE));
        let single_quoted = "host: staging.neonlaw.com\nproject: 'acme # synthetic'\n";
        assert!(!codes(single_quoted).contains(&COMMENT_CODE));
        let flow = concat!(
            "host: staging.neonlaw.com\n",
            "project: acme\n",
            "allowed_prefixes: [\"https://react.dev/errors/#\", 'https://doc.rust-lang.org/error_codes/#']\n",
        );
        assert!(
            !codes(flow).contains(&COMMENT_CODE),
            "{:?}",
            lint_contents(Path::new("navigator.yaml"), flow)
        );
        let apostrophe_then_comment = concat!(
            "host: staging.neonlaw.com\n",
            "project: acme\n",
            "no_live_row: the author's closed file # record the reason\n",
        );
        assert!(
            codes(apostrophe_then_comment).contains(&COMMENT_CODE),
            "{:?}",
            lint_contents(Path::new("navigator.yaml"), apostrophe_then_comment)
        );
        let bom = concat!(
            "\u{feff}",
            "# record the exemption in the pull request\n",
            "host: staging.neonlaw.com\n",
            "project: acme\n",
        );
        let bom_findings = lint_contents(Path::new("navigator.yaml"), bom);
        assert!(
            bom_findings
                .iter()
                .any(|f| f.code == COMMENT_CODE && f.line == 1),
            "{bom_findings:?}"
        );
        let block = concat!(
            "host: staging.neonlaw.com\n",
            "project: acme\n",
            "no_live_row: |\n",
            "  the matter closed; # this hash is scalar content\n",
        );
        assert!(!codes(block).contains(&COMMENT_CODE));
    }

    #[test]
    fn yml_spelling_is_a_rename() {
        let dir = std::env::temp_dir().join(format!(
            "navigator-manifest-yml-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(RETIRED_FILE),
            "host: staging.neonlaw.com\nproject: acme\n",
        )
        .unwrap();
        let findings = lint(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(findings
            .iter()
            .any(|f| f.code == RENAME_CODE
                && f.message == "the manifest is navigator.yaml, rename it"));
    }
}
