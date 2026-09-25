//! Project-repository `navigator.yaml` — one accepted key set, one reader.
//!
//! `navigator project gate` and `navigator project drift` share this list so
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
/// `Y012` — `version` must be an exact release tag.
pub const VERSION_CODE: &str = "Y012";
/// `Y013` — the flat manifest shape is deprecated.
pub const DEPRECATED_CODE: &str = "Y013";
/// `Y015` — a `skills:` entry must carry `jurisdiction`, `practice_area`, and `version`.
pub const SKILLS_CODE: &str = "Y015";

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
    "skills",
    "version",
];

const PROJECT_KEYS: &[&str] = &[
    "client_dri",
    "host",
    "lawyer_dri",
    "name",
    "private_notion_page",
    "private_slack_channel",
    "shared_notion_page",
    "shared_slack_channel",
    "xero_customer",
];

const HANDLE_KEYS: &[&str] = &[
    "lawyer_dri",
    "client_dri",
    "private_slack_channel",
    "private_notion_page",
    "shared_slack_channel",
    "shared_notion_page",
    "xero_customer",
];

/// One `skills:` entry — a Project Skill catalog `(jurisdiction,
/// practice_area)` pinned at a specific `version`, recorded by `navigator
/// project skill use` and read back by `navigator project skill status` and
/// `navigator project gate --check`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillPin {
    pub jurisdiction: String,
    pub practice_area: String,
    pub version: String,
}

/// A Project repository's root manifest.
///
/// Unknown keys are not stored: [`lint`] refuses them before a caller reads
/// this struct. The `no_live_row` field stays untyped so `true` cannot coerce
/// into a plausible reason.
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct Manifest {
    pub version: Option<String>,
    pub host: Option<String>,
    pub project: Option<String>,
    pub lawyer_dri: Option<String>,
    pub client_dri: Option<String>,
    pub private_slack_channel: Option<String>,
    pub private_notion_page: Option<String>,
    pub shared_slack_channel: Option<String>,
    pub shared_notion_page: Option<String>,
    pub xero_customer: Option<String>,
    pub no_live_row: Option<serde_yaml::Value>,
    pub allowed_hosts: BTreeMap<String, String>,
    pub allowed_links: BTreeMap<String, String>,
    pub allowed_prefixes: BTreeMap<String, String>,
    pub skills: Vec<SkillPin>,
}

/// One manifest finding, with a Y-family rule code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestFinding {
    pub path: PathBuf,
    pub line: usize,
    pub code: &'static str,
    pub message: String,
    pub warning: bool,
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
            warning: false,
        }
    }

    fn warning(path: &Path, line: usize, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            path: path.to_path_buf(),
            line,
            code,
            message: message.into(),
            warning: true,
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
        Err(error) => {
            findings.push(ManifestFinding::at(
                path,
                1,
                UNKNOWN_KEY_CODE,
                format!("navigator.yaml is not valid YAML: {error}"),
            ));
            return findings;
        }
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
    findings.extend(lint_version(path, mapping));
    match mapping.get("project") {
        Some(serde_yaml::Value::Mapping(project)) => {
            findings.extend(lint_nested_project(path, project));
        }
        Some(serde_yaml::Value::String(_)) => {
            if mapping.contains_key("version") {
                findings.push(ManifestFinding::at(
                    path,
                    1,
                    PROJECT_CODE,
                    "`project` must be a map with `host` and `name` when `version` is present",
                ));
            } else {
                findings.push(ManifestFinding::warning(
                    path,
                    1,
                    DEPRECATED_CODE,
                    "flat `host:`/`project:` is deprecated; use `version:` and `project.host`/`project.name`",
                ));
                findings.extend(lint_flat_host(path, mapping));
                findings.extend(lint_flat_project(path, mapping));
            }
        }
        Some(_) => findings.push(ManifestFinding::at(
            path,
            1,
            PROJECT_CODE,
            "`project` must be a Project code or a map with `host` and `name`",
        )),
        None => {
            findings.extend(lint_flat_host(path, mapping));
            findings.extend(lint_flat_project(path, mapping));
        }
    }
    findings.extend(lint_rowless(path, mapping));
    findings.extend(lint_skills(path, mapping));
    if let Ok(manifest) = parse(contents) {
        findings.extend(lint_allowlists(path, &manifest));
    }
    findings
}

/// `Y015` — each `skills:` entry must be a map carrying non-empty
/// `jurisdiction`, `practice_area`, and `version` text. Whether the pair
/// still resolves against the compiled-in catalog is a separate question —
/// `navigator project gate --check` (not this offline lint) answers it,
/// because only the CLI binary carries the catalog.
fn lint_skills(path: &Path, mapping: &serde_yaml::Mapping) -> Vec<ManifestFinding> {
    let Some(value) = mapping.get("skills") else {
        return Vec::new();
    };
    let Some(items) = value.as_sequence() else {
        return vec![ManifestFinding::at(
            path,
            1,
            SKILLS_CODE,
            "`skills:` must be a list of {jurisdiction, practice_area, version} entries",
        )];
    };
    let mut findings = Vec::new();
    for item in items {
        let Some(entry) = item.as_mapping() else {
            findings.push(ManifestFinding::at(
                path,
                1,
                SKILLS_CODE,
                "each `skills:` entry must be a map of jurisdiction, practice_area, and version",
            ));
            continue;
        };
        for key in ["jurisdiction", "practice_area", "version"] {
            if entry.get(key).and_then(scalar_string).is_none() {
                findings.push(ManifestFinding::at(
                    path,
                    1,
                    SKILLS_CODE,
                    format!("each `skills:` entry must carry a non-empty `{key}`"),
                ));
            }
        }
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

fn lint_version(path: &Path, mapping: &serde_yaml::Mapping) -> Vec<ManifestFinding> {
    match mapping.get("version") {
        None => Vec::new(),
        Some(value) => match scalar_string(value) {
            Some(version) if crate::devx::registry::is_release_tag(&version) => Vec::new(),
            Some(version) => vec![ManifestFinding::at(
                path,
                1,
                VERSION_CODE,
                format!("version must be an exact release tag, not '{version}'."),
            )],
            None => vec![ManifestFinding::at(
                path,
                1,
                VERSION_CODE,
                "version must be an exact release tag, not ''.",
            )],
        },
    }
}

fn lint_flat_host(path: &Path, mapping: &serde_yaml::Mapping) -> Vec<ManifestFinding> {
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

fn lint_flat_project(path: &Path, mapping: &serde_yaml::Mapping) -> Vec<ManifestFinding> {
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

fn lint_nested_project(path: &Path, project: &serde_yaml::Mapping) -> Vec<ManifestFinding> {
    let mut findings = Vec::new();
    for key in project.keys() {
        let Some(name) = key.as_str() else {
            findings.push(ManifestFinding::at(
                path,
                1,
                UNKNOWN_KEY_CODE,
                "navigator.yaml project-map keys must be strings",
            ));
            continue;
        };
        if !PROJECT_KEYS.contains(&name) {
            findings.push(ManifestFinding::at(
                path,
                1,
                UNKNOWN_KEY_CODE,
                format!(
                    "unknown project key `{name}`; expected one of {}",
                    PROJECT_KEYS.join(", ")
                ),
            ));
        }
    }

    let Some(host) = project.get("host").and_then(scalar_string) else {
        findings.push(ManifestFinding::at(
            path,
            1,
            HOST_CODE,
            "`project.host` is required and must be a hostname",
        ));
        return findings;
    };
    if !is_hostname(&host) {
        findings.push(ManifestFinding::at(
            path,
            1,
            HOST_CODE,
            format!("`project.host: {host}` is not a hostname"),
        ));
    }

    match project.get("name").and_then(scalar_string) {
        Some(name) if store::projects::is_valid_code(&name) => {}
        Some(name) => findings.push(ManifestFinding::at(
            path,
            1,
            PROJECT_CODE,
            format!("`project.name: {name}` is not a valid Project code"),
        )),
        None => findings.push(ManifestFinding::at(
            path,
            1,
            PROJECT_CODE,
            "`project.name` is required and must be a Project code",
        )),
    }

    for key in HANDLE_KEYS {
        if let Some(value) = project.get(*key) {
            findings.extend(lint_handle(path, key, value));
        }
    }
    findings
}

fn lint_handle(path: &Path, key: &str, value: &serde_yaml::Value) -> Vec<ManifestFinding> {
    let Some(value) = scalar_string(value) else {
        return vec![ManifestFinding::at(
            path,
            1,
            PROJECT_CODE,
            format!("`project.{key}` must be non-empty text"),
        )];
    };
    let valid = match key {
        "lawyer_dri" | "client_dri" => is_email(&value),
        "private_slack_channel"
        | "private_notion_page"
        | "shared_slack_channel"
        | "shared_notion_page" => is_https_url(&value),
        "xero_customer" => true,
        _ => false,
    };
    if valid {
        Vec::new()
    } else {
        vec![ManifestFinding::at(
            path,
            1,
            PROJECT_CODE,
            format!("`project.{key}` has the wrong shape"),
        )]
    }
}

fn is_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && !value.chars().any(char::is_whitespace)
        && domain.contains('.')
}

fn is_https_url(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https" && url.host_str().is_some_and(|host| !host.is_empty())
    })
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
    let document: serde_yaml::Value = serde_yaml::from_str(contents)
        .map_err(|error| format!("navigator.yaml is not valid YAML: {error}"))?;
    let mapping = document
        .as_mapping()
        .ok_or_else(|| "navigator.yaml must be a mapping".to_string())?;
    let version = mapping.get("version").map(parse_text).transpose()?;
    let (host, project, project_map) = match mapping.get("project") {
        Some(serde_yaml::Value::Mapping(project)) => (
            Some(required_text(project, "host")?),
            Some(required_text(project, "name")?),
            Some(project),
        ),
        Some(value) => (
            mapping.get("host").map(parse_text).transpose()?,
            Some(required_text_value(value, "project")?),
            None,
        ),
        None => (mapping.get("host").map(parse_text).transpose()?, None, None),
    };
    let values = |key: &str| {
        project_map
            .and_then(|project| project.get(key))
            .map(parse_text)
            .transpose()
    };
    Ok(Manifest {
        version,
        host,
        project,
        lawyer_dri: values("lawyer_dri")?,
        client_dri: values("client_dri")?,
        private_slack_channel: values("private_slack_channel")?,
        private_notion_page: values("private_notion_page")?,
        shared_slack_channel: values("shared_slack_channel")?,
        shared_notion_page: values("shared_notion_page")?,
        xero_customer: values("xero_customer")?,
        no_live_row: mapping.get("no_live_row").cloned(),
        allowed_hosts: parse_reason_map(mapping, "allowed_hosts")?,
        allowed_links: parse_reason_map(mapping, "allowed_links")?,
        allowed_prefixes: parse_reason_map(mapping, "allowed_prefixes")?,
        skills: parse_skills(mapping)?,
    })
}

fn parse_skills(mapping: &serde_yaml::Mapping) -> Result<Vec<SkillPin>, String> {
    let Some(value) = mapping.get("skills") else {
        return Ok(Vec::new());
    };
    let items = value
        .as_sequence()
        .ok_or_else(|| "navigator.yaml skills must be a list".to_string())?;
    items
        .iter()
        .map(|item| {
            let entry = item
                .as_mapping()
                .ok_or_else(|| "navigator.yaml skills entry must be a map".to_string())?;
            Ok(SkillPin {
                jurisdiction: required_text(entry, "jurisdiction")?,
                practice_area: required_text(entry, "practice_area")?,
                version: required_text(entry, "version")?,
            })
        })
        .collect()
}

/// Pin `(jurisdiction, practice_area)` at `version` in a `navigator.yaml`'s
/// `skills:` list, matching an existing entry case-insensitively. Returns
/// the rewritten contents and whether anything changed — a caller writes
/// the file only when it did, so re-pinning the same version is a byte-for-
/// byte no-op rather than a reformat.
///
/// # Errors
///
/// A string error if `contents` is not valid YAML, is not a mapping, or
/// already carries a non-list `skills:` key.
pub fn pin_skill(
    contents: &str,
    jurisdiction: &str,
    practice_area: &str,
    version: &str,
) -> Result<(String, bool), String> {
    let mut document: serde_yaml::Value = serde_yaml::from_str(contents)
        .map_err(|error| format!("navigator.yaml is not valid YAML: {error}"))?;
    let mapping = document
        .as_mapping_mut()
        .ok_or_else(|| "navigator.yaml must be a mapping".to_string())?;

    let mut skills: Vec<serde_yaml::Value> = match mapping.get("skills") {
        Some(serde_yaml::Value::Sequence(items)) => items.clone(),
        Some(_) => return Err("navigator.yaml `skills` must be a list".to_string()),
        None => Vec::new(),
    };

    let mut changed = false;
    let mut found = false;
    for item in &mut skills {
        let Some(entry) = item.as_mapping_mut() else {
            continue;
        };
        let entry_jurisdiction = entry
            .get("jurisdiction")
            .and_then(scalar_string)
            .unwrap_or_default();
        let entry_practice_area = entry
            .get("practice_area")
            .and_then(scalar_string)
            .unwrap_or_default();
        if entry_jurisdiction.eq_ignore_ascii_case(jurisdiction)
            && entry_practice_area.eq_ignore_ascii_case(practice_area)
        {
            found = true;
            let current_version = entry
                .get("version")
                .and_then(scalar_string)
                .unwrap_or_default();
            if current_version != version {
                entry.insert(
                    serde_yaml::Value::String("version".to_string()),
                    serde_yaml::Value::String(version.to_string()),
                );
                changed = true;
            }
        }
    }
    if !found {
        let mut entry = serde_yaml::Mapping::new();
        entry.insert(
            serde_yaml::Value::String("jurisdiction".to_string()),
            serde_yaml::Value::String(jurisdiction.to_string()),
        );
        entry.insert(
            serde_yaml::Value::String("practice_area".to_string()),
            serde_yaml::Value::String(practice_area.to_string()),
        );
        entry.insert(
            serde_yaml::Value::String("version".to_string()),
            serde_yaml::Value::String(version.to_string()),
        );
        skills.push(serde_yaml::Value::Mapping(entry));
        changed = true;
    }

    if !changed {
        return Ok((contents.to_string(), false));
    }
    mapping.insert(
        serde_yaml::Value::String("skills".to_string()),
        serde_yaml::Value::Sequence(skills),
    );
    let serialized = serde_yaml::to_string(&document)
        .map_err(|error| format!("serialize navigator.yaml: {error}"))?;
    Ok((serialized, true))
}

fn parse_text(value: &serde_yaml::Value) -> Result<String, String> {
    scalar_string(value).ok_or_else(|| "value must be non-empty text".to_string())
}

fn required_text(mapping: &serde_yaml::Mapping, key: &str) -> Result<String, String> {
    mapping
        .get(key)
        .ok_or_else(|| format!("navigator.yaml project.{key} is required"))
        .and_then(parse_text)
}

fn required_text_value(value: &serde_yaml::Value, key: &str) -> Result<String, String> {
    parse_text(value).map_err(|_| format!("navigator.yaml {key} must be non-empty text"))
}

fn parse_reason_map(
    mapping: &serde_yaml::Mapping,
    key: &str,
) -> Result<BTreeMap<String, String>, String> {
    mapping.get(key).map_or_else(
        || Ok(BTreeMap::new()),
        |value| {
            value
                .as_mapping()
                .ok_or_else(|| format!("navigator.yaml {key} must be a map"))?
                .iter()
                .map(|(key, value)| {
                    let key = parse_text(key)?;
                    Ok((key, parse_text(value)?))
                })
                .collect()
        },
    )
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
            "skills",
            "version",
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
        assert_eq!(codes(yaml), vec![DEPRECATED_CODE]);
    }

    #[test]
    fn nested_manifest_reads_release_and_handles() {
        let yaml = concat!(
            "version: 26.9.14\n",
            "project:\n",
            "  host: staging.neonlaw.com\n",
            "  name: acme\n",
            "  lawyer_dri: lawyer@example.com\n",
            "  client_dri: client@example.com\n",
            "  private_slack_channel: https://slack.example.com/private\n",
            "  private_notion_page: https://notion.example.com/private\n",
            "  shared_slack_channel: https://slack.example.com/shared\n",
            "  shared_notion_page: https://notion.example.com/shared\n",
            "  xero_customer: customer-1\n",
        );
        let parsed = parse(yaml).expect("nested manifest deserializes");
        assert_eq!(parsed.version.as_deref(), Some("26.9.14"));
        assert_eq!(parsed.host.as_deref(), Some("staging.neonlaw.com"));
        assert_eq!(parsed.project.as_deref(), Some("acme"));
        assert_eq!(parsed.lawyer_dri.as_deref(), Some("lawyer@example.com"));
        assert_eq!(parsed.xero_customer.as_deref(), Some("customer-1"));
        assert!(lint_contents(Path::new(FILE), yaml).is_empty());
    }

    #[test]
    fn malformed_nested_manifest_reports_one_manifest_finding() {
        let findings = lint_contents(
            Path::new(FILE),
            "version: 26.9.14\nproject:\n  host: [staging.neonlaw.com]\n  name: acme\n",
        );
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].path, Path::new(FILE));
        assert_eq!(findings[0].code, HOST_CODE);
    }

    #[test]
    fn version_must_be_an_exact_release_tag() {
        for version in ["latest", "main", "HEAD", ""] {
            let yaml = format!(
                "version: \"{version}\"\nproject:\n  host: staging.neonlaw.com\n  name: acme\n"
            );
            let finding = lint_contents(Path::new(FILE), &yaml)
                .into_iter()
                .find(|finding| finding.code == VERSION_CODE)
                .expect("invalid version finding");
            assert!(finding
                .message
                .contains("version must be an exact release tag"));
        }
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
        assert!(lint_contents(
            Path::new("navigator.yaml"),
            "host: staging.neonlaw.com\nproject: acme\nno_live_row: the matter closed\n"
        )
        .iter()
        .all(|finding| finding.warning));
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

    #[test]
    fn skills_entries_parse_into_pins() {
        let yaml = concat!(
            "host: staging.neonlaw.com\n",
            "project: acme\n",
            "skills:\n",
            "  - jurisdiction: NV\n",
            "    practice_area: estates\n",
            "    version: \"1\"\n",
            "  - jurisdiction: TX\n",
            "    practice_area: probate\n",
            "    version: \"1\"\n",
        );
        assert!(
            lint_contents(Path::new(FILE), yaml)
                .iter()
                .all(|f| f.warning),
            "{:?}",
            lint_contents(Path::new(FILE), yaml)
        );
        let parsed = parse(yaml).expect("skills parse");
        assert_eq!(parsed.skills.len(), 2);
        assert_eq!(parsed.skills[0].jurisdiction, "NV");
        assert_eq!(parsed.skills[0].practice_area, "estates");
        assert_eq!(parsed.skills[0].version, "1");
    }

    #[test]
    fn a_skills_entry_missing_a_field_is_flagged() {
        let yaml = concat!(
            "host: staging.neonlaw.com\n",
            "project: acme\n",
            "skills:\n",
            "  - jurisdiction: NV\n",
            "    version: \"1\"\n",
        );
        let findings = lint_contents(Path::new(FILE), yaml);
        assert!(
            findings
                .iter()
                .any(|f| f.code == SKILLS_CODE && f.message.contains("practice_area")),
            "{findings:?}"
        );
    }

    #[test]
    fn a_non_list_skills_key_is_flagged() {
        let yaml = "host: staging.neonlaw.com\nproject: acme\nskills: nv/estates\n";
        let findings = lint_contents(Path::new(FILE), yaml);
        assert!(findings.iter().any(|f| f.code == SKILLS_CODE));
    }

    #[test]
    fn pin_skill_appends_a_new_entry() {
        let yaml = "host: staging.neonlaw.com\nproject: acme\n";
        let (updated, changed) = pin_skill(yaml, "NV", "estates", "1").expect("pin");
        assert!(changed);
        let parsed = parse(&updated).expect("parses");
        assert_eq!(parsed.skills.len(), 1);
        assert_eq!(parsed.skills[0].jurisdiction, "NV");
        assert_eq!(parsed.skills[0].practice_area, "estates");
        assert_eq!(parsed.skills[0].version, "1");
    }

    #[test]
    fn pin_skill_is_a_no_op_when_already_pinned_at_the_same_version() {
        let yaml = "host: staging.neonlaw.com\nproject: acme\n";
        let (once, _) = pin_skill(yaml, "NV", "estates", "1").expect("pin");
        let (twice, changed) = pin_skill(&once, "nv", "estates", "1").expect("pin again");
        assert!(!changed, "re-pinning the same version must be a no-op");
        assert_eq!(once, twice);
    }

    #[test]
    fn pin_skill_updates_the_version_on_an_existing_pin() {
        let yaml = "host: staging.neonlaw.com\nproject: acme\n";
        let (once, _) = pin_skill(yaml, "NV", "estates", "1").expect("pin");
        let (updated, changed) = pin_skill(&once, "NV", "estates", "2").expect("re-pin");
        assert!(changed);
        let parsed = parse(&updated).expect("parses");
        assert_eq!(
            parsed.skills.len(),
            1,
            "a version bump updates in place, not a duplicate"
        );
        assert_eq!(parsed.skills[0].version, "2");
    }
}
