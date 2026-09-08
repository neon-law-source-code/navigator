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

/// Every top-level key `navigator.yaml` may carry.
///
/// Drift's reader deserializes this same set (and only this set of *named*
/// fields). Adding a key means adding it here, in [`Manifest`], and in the
/// covering test that asserts the two agree.
pub const ACCEPTED_KEYS: &[&str] = &[
    "allowed_hosts",
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

/// Lint already-read YAML. Used by tests and by [`lint`].
#[must_use]
pub fn lint_contents(path: &Path, contents: &str) -> Vec<ManifestFinding> {
    let document: serde_yaml::Value = match serde_yaml::from_str(contents) {
        Ok(document) => document,
        Err(_) => return Vec::new(),
    };
    let Some(mapping) = document.as_mapping() else {
        return vec![ManifestFinding::at(
            path,
            1,
            UNKNOWN_KEY_CODE,
            format!(
                "navigator.yaml must be a mapping of {}; got a non-mapping document",
                accepted_keys_phrase()
            ),
        )];
    };
    let mut findings = lint_keys(path, mapping);
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
            "allowed_prefixes:\n",
            "  \"https://react.dev/errors/\": React minified errors\n",
        );
        let parsed = parse(yaml).expect("accepted keys deserialize");
        assert_eq!(parsed.host.as_deref(), Some("staging.neonlaw.com"));
        assert_eq!(parsed.project.as_deref(), Some("acme"));
        assert_eq!(parsed.allowed_hosts.len(), 1);
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
