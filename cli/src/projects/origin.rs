//! Origin scan over a built Project portal.
//!
//! A client portal talks to Navigator under the same origin. Nothing it ships
//! may name another host. The scan reads the *built* files under each
//! application's `dist/`, not source: a dependency five levels down that
//! appends a beacon URL is invisible in review and visible here.
//!
//! Ported from the Project-repository Python gate, including the two host
//! guards that keep a regex-literal `//.test(` from reading as protocol-
//! relative: an empty first label, and a "host" with no letter or digit.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::manifest::{self, Manifest, ManifestFinding};

/// `Y009` — a built file names a host this portal must not fetch.
pub const ORIGIN_CODE: &str = "Y009";

const SCANNED_SUFFIXES: &[&str] = &[".js", ".mjs", ".cjs", ".css", ".html"];

/// Hosts every portal may name because every portal is built the same way.
const BUILTIN_HOSTS: &[(&str, &str)] = &[(
    "www.w3.org",
    "XML namespace identifiers — compared as strings, never fetched",
)];

/// URL prefixes every portal may name, when the host itself is not blanket-allowed.
const BUILTIN_PREFIXES: &[(&str, &str)] = &[(
    "https://react.dev/errors/",
    "React minified-error explainer, in a console string",
)];

/// Scan each application's `dist/` for off-origin host references.
///
/// A repository with no application is a no-op. An application whose `dist/`
/// is missing, or holds no scannable files, is a finding: the gate describes
/// what a client receives, so it must read the real build.
#[must_use]
pub fn lint(_root: &Path, applications: &[PathBuf], manifest: &Manifest) -> Vec<ManifestFinding> {
    if applications.is_empty() {
        return Vec::new();
    }
    let hosts = merge_hosts(manifest);
    let prefixes = merge_prefixes(manifest);
    let mut findings = Vec::new();
    for application in applications {
        let dist = application.join("dist");
        if !dist.is_dir() {
            continue;
        }
        let files = scanned_files(&dist);
        if files.is_empty() {
            findings.push(ManifestFinding::at(
                &dist,
                1,
                ORIGIN_CODE,
                "dist/ holds no .js/.mjs/.cjs/.css/.html files to check",
            ));
            continue;
        }
        for path in files {
            let Ok(text) = std::fs::read_to_string(&path) else {
                findings.push(ManifestFinding::at(
                    &path,
                    1,
                    ORIGIN_CODE,
                    "could not read built file",
                ));
                continue;
            };
            findings.extend(check_hosts(&path, &text, &hosts, &prefixes));
            findings.extend(check_minified(&path, &text));
            findings.extend(check_sourcemap(&path, &text));
        }
    }
    findings
}

fn merge_hosts(manifest: &Manifest) -> BTreeMap<String, String> {
    let mut hosts = BTreeMap::new();
    for (host, reason) in BUILTIN_HOSTS {
        hosts.insert((*host).to_string(), (*reason).to_string());
    }
    if let Some(host) = manifest.host.as_deref() {
        hosts
            .entry(host.to_string())
            .or_insert_with(|| "this Project's Navigator host".to_string());
    }
    for (host, reason) in &manifest.allowed_hosts {
        hosts.insert(host.clone(), reason.clone());
    }
    hosts
}

fn merge_prefixes(manifest: &Manifest) -> BTreeMap<String, String> {
    let mut prefixes = BTreeMap::new();
    for (prefix, reason) in BUILTIN_PREFIXES {
        prefixes.insert((*prefix).to_string(), (*reason).to_string());
    }
    for (prefix, reason) in &manifest.allowed_prefixes {
        prefixes.insert(prefix.clone(), reason.clone());
    }
    prefixes
}

fn scanned_files(dist: &Path) -> Vec<PathBuf> {
    walkdir::WalkDir::new(dist)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(walkdir::DirEntry::into_path)
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| {
                    SCANNED_SUFFIXES
                        .iter()
                        .any(|suffix| suffix.trim_start_matches('.') == ext)
                })
        })
        .collect()
}

fn check_hosts(
    path: &Path,
    text: &str,
    hosts: &BTreeMap<String, String>,
    prefixes: &BTreeMap<String, String>,
) -> Vec<ManifestFinding> {
    let text = strip_base64(text);
    let mut findings = Vec::new();
    for (full, host) in find_urls(&text) {
        if skip_host(host) || hosts.contains_key(host) {
            continue;
        }
        if prefixes.keys().any(|prefix| full.starts_with(prefix)) {
            continue;
        }
        findings.push(ManifestFinding::at(
            path,
            1,
            ORIGIN_CODE,
            format!(
                "off-origin reference to `{host}` — {}. A portal reads only Navigator's API under its own origin. If this is a dependency's doing, pin or patch it; if it is genuinely not a request, add it to `allowed_hosts` or `allowed_prefixes` in navigator.yaml with the reason.",
                truncate(&full, 120)
            ),
        ));
    }
    findings
}

/// Empty first label (`.test`) and dots/slashes-only are not hosts.
pub fn skip_host(host: &str) -> bool {
    host.starts_with('.') || !host.chars().any(|c| c.is_ascii_alphanumeric())
}

fn check_minified(path: &Path, text: &str) -> Vec<ManifestFinding> {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return Vec::new();
    };
    if !matches!(ext, "js" | "mjs" | "cjs") || text.is_empty() {
        return Vec::new();
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let longest = lines.iter().map(|line| line.len()).max().unwrap_or(0);
    if longest < 200 && lines.len() > 50 {
        return vec![ManifestFinding::at(
            path,
            1,
            ORIGIN_CODE,
            format!(
                "looks unminified ({} lines, longest {longest} chars). The gate describes what a client receives, so it must read the real build.",
                lines.len()
            ),
        )];
    }
    Vec::new()
}

fn check_sourcemap(path: &Path, text: &str) -> Vec<ManifestFinding> {
    let mut findings = Vec::new();
    let mut rest = text;
    while let Some(idx) = rest.find("sourceMappingURL=") {
        let after = &rest[idx + "sourceMappingURL=".len()..];
        let target = after
            .split(|c: char| c.is_ascii_whitespace())
            .next()
            .unwrap_or("");
        if target.starts_with("http") || target.starts_with("//") {
            findings.push(ManifestFinding::at(
                path,
                1,
                ORIGIN_CODE,
                format!("sourceMappingURL points off-origin: {target}"),
            ));
        }
        rest = after;
    }
    findings
}

fn strip_base64(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if is_b64(bytes[i]) {
            let start = i;
            while i < bytes.len() && is_b64(bytes[i]) {
                i += 1;
            }
            let mut pad = 0;
            while i + pad < bytes.len() && bytes[i + pad] == b'=' && pad < 2 {
                pad += 1;
            }
            if i - start >= 64 {
                out.push_str(&".".repeat(i - start + pad));
                i += pad;
                continue;
            }
            out.push_str(&text[start..i]);
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn is_b64(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'+' || b == b'/'
}

/// `(full, host)` for each absolute or protocol-relative URL.
fn find_urls(text: &str) -> Vec<(String, &str)> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'/' && bytes[i + 1] == b'/' {
            let scheme = if i >= 5 && &bytes[i - 5..i] == b"http:" {
                i - 5
            } else if i >= 6 && &bytes[i - 6..i] == b"https:" {
                i - 6
            } else {
                i
            };
            let host_start = i + 2;
            let mut host_end = host_start;
            while host_end < bytes.len() && is_host_byte(bytes[host_end]) {
                host_end += 1;
            }
            if host_end > host_start {
                let host = &text[host_start..host_end];
                let mut full_end = host_end;
                while full_end < bytes.len() && !is_url_stop(bytes[full_end]) {
                    full_end += 1;
                }
                found.push((text[scheme..full_end].to_string(), host));
                i = host_end;
                continue;
            }
        }
        i += 1;
    }
    found
}

fn is_host_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-'
}

fn is_url_stop(b: u8) -> bool {
    b.is_ascii_whitespace() || matches!(b, b'"' | b'\'' | b'`' | b')' | b'\\')
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..max]
    }
}

/// Load the root manifest, if present, for the origin scan.
#[must_use]
pub fn load_manifest(root: &Path) -> Option<Manifest> {
    let contents = std::fs::read_to_string(root.join(manifest::FILE)).ok()?;
    manifest::parse(&contents).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leading_dot_is_not_a_host() {
        assert!(skip_host(".test"));
        assert!(skip_host(".exec"));
        assert!(skip_host("..."));
        assert!(!skip_host("example.com"));
        assert!(!skip_host("localhost"));
    }

    #[test]
    fn regex_literal_slash_pair_is_not_protocol_relative() {
        let js = r"let t=/^\s*<\//.test(e.textAfter);cdn.example/x.js";
        let urls = find_urls(js);
        assert!(
            urls.iter()
                .all(|(_, host)| skip_host(host) || *host != ".test"),
            "{urls:?}"
        );
        let with_real = r#"let t=/^\s*<\//.test(e.textAfter);fetch("https://cdn.example/x.js")"#;
        let found = find_urls(with_real);
        assert!(
            found.iter().any(|(_, host)| *host == "cdn.example"),
            "{found:?}"
        );
    }

    #[test]
    fn builtin_and_declared_hosts_are_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let dist = dir.path().join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(
            dist.join("index.html"),
            r#"<svg xmlns="http://www.w3.org/2000/svg"></svg><script src="https://react.dev/errors/123"></script><link href="//staging.neonlaw.com/x">"#,
        )
        .unwrap();
        let manifest = Manifest {
            host: Some("staging.neonlaw.com".into()),
            project: Some("acme".into()),
            ..Manifest::default()
        };
        let findings = lint(dir.path(), &[dir.path().to_path_buf()], &manifest);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn an_undeclared_host_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let dist = dir.path().join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(
            dist.join("app.js"),
            format!("fetch(\"https://evil.example/x.js\");{}", "x".repeat(200)),
        )
        .unwrap();
        let manifest = Manifest {
            host: Some("staging.neonlaw.com".into()),
            project: Some("acme".into()),
            ..Manifest::default()
        };
        let findings = lint(dir.path(), &[dir.path().to_path_buf()], &manifest);
        assert!(
            findings
                .iter()
                .any(|f| f.code == ORIGIN_CODE && f.message.contains("evil.example")),
            "{findings:?}"
        );
    }
}
