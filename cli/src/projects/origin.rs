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
    let links = merge_links(manifest);
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
            findings.extend(check_hosts(&path, &text, &hosts, &prefixes, &links));
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

fn merge_links(manifest: &Manifest) -> BTreeMap<String, String> {
    manifest.allowed_links.clone()
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
    links: &BTreeMap<String, String>,
) -> Vec<ManifestFinding> {
    let text = strip_base64(text);
    let anchors = find_anchors(&text);
    let mut covered = vec![false; text.len().saturating_add(1)];
    let mut findings = Vec::new();
    for anchor in &anchors {
        mark_covered(&mut covered, anchor.href_start, anchor.href_end);
        if skip_host(&anchor.host) || hosts.contains_key(&anchor.host) {
            continue;
        }
        if prefixes
            .keys()
            .any(|prefix| anchor.href.starts_with(prefix.as_str()))
        {
            continue;
        }
        if links.contains_key(&anchor.host) {
            if has_noreferrer(&anchor.rel) {
                continue;
            }
            findings.push(ManifestFinding::at(
                path,
                1,
                ORIGIN_CODE,
                format!(
                    "off-origin link to `{}` is listed in `allowed_links` but is missing \
                     rel=\"noreferrer\" — {}",
                    anchor.host,
                    truncate(&anchor.href, 120)
                ),
            ));
            continue;
        }
        findings.push(off_origin_finding(path, &anchor.host, &anchor.href));
    }
    for url in find_url_spans(&text) {
        if range_covered(&covered, url.start, url.end) {
            continue;
        }
        if skip_host(url.host) || hosts.contains_key(url.host) {
            continue;
        }
        if prefixes.keys().any(|prefix| url.full.starts_with(prefix)) {
            continue;
        }
        findings.push(off_origin_finding(path, url.host, &url.full));
    }
    findings
}

fn off_origin_finding(path: &Path, host: &str, full: &str) -> ManifestFinding {
    ManifestFinding::at(
        path,
        1,
        ORIGIN_CODE,
        format!(
            "off-origin reference to `{host}` — {}. A portal reads only Navigator's API under its own origin. If this is a dependency's doing, pin or patch it; if it is genuinely not a request, add it to `allowed_hosts` or `allowed_prefixes` in navigator.yaml with the reason. Citation hrefs that must not send Referer belong in `allowed_links` and must carry rel=\"noreferrer\".",
            truncate(full, 120)
        ),
    )
}

fn mark_covered(covered: &mut [bool], start: usize, end: usize) {
    let end = end.min(covered.len());
    let start = start.min(end);
    for slot in covered.iter_mut().take(end).skip(start) {
        *slot = true;
    }
}

fn range_covered(covered: &[bool], start: usize, end: usize) -> bool {
    let end = end.min(covered.len());
    let start = start.min(end);
    (start..end).any(|i| covered[i])
}

fn has_noreferrer(rel: &str) -> bool {
    rel.split_whitespace()
        .any(|token| token.eq_ignore_ascii_case("noreferrer"))
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

#[derive(Debug)]
struct FoundUrl<'a> {
    full: String,
    host: &'a str,
    start: usize,
    end: usize,
}

struct FoundAnchor {
    href: String,
    host: String,
    rel: String,
    href_start: usize,
    href_end: usize,
}

fn find_url_spans(text: &str) -> Vec<FoundUrl<'_>> {
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
                found.push(FoundUrl {
                    full: text[scheme..full_end].to_string(),
                    host,
                    start: scheme,
                    end: full_end,
                });
                i = host_end;
                continue;
            }
        }
        i += 1;
    }
    found
}

fn find_anchors(text: &str) -> Vec<FoundAnchor> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    while i + 2 < bytes.len() {
        if bytes[i] == b'<' && is_ascii_a(bytes[i + 1]) && is_a_tag_boundary(bytes[i + 2]) {
            if let Some(anchor) = parse_anchor(text, i) {
                let next = anchor.href_end;
                found.push(anchor);
                i = next.max(i + 1);
                continue;
            }
        }
        i += 1;
    }
    found
}

fn is_ascii_a(b: u8) -> bool {
    b == b'a' || b == b'A'
}

fn is_a_tag_boundary(b: u8) -> bool {
    b.is_ascii_whitespace() || b == b'/' || b == b'>'
}

fn parse_anchor(text: &str, start: usize) -> Option<FoundAnchor> {
    let bytes = text.as_bytes();
    let mut i = start + 2;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = quote {
            if b == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        if b == b'"' || b == b'\'' {
            quote = Some(b);
            i += 1;
            continue;
        }
        if b == b'>' {
            break;
        }
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'>' {
        return None;
    }
    let inner = &text[start + 2..i];
    let attrs = parse_attrs(inner);
    let href = attrs.get("href")?;
    if href.is_empty() {
        return None;
    }
    let href_start = text[start..=i].find(href.as_str())? + start;
    let href_end = href_start + href.len();
    let host = find_url_spans(href)
        .into_iter()
        .next()
        .map(|url| url.host.to_string())?;
    let rel = attrs.get("rel").cloned().unwrap_or_default();
    Some(FoundAnchor {
        href: href.clone(),
        host,
        rel,
        href_start,
        href_end,
    })
}

fn parse_attrs(inner: &str) -> BTreeMap<String, String> {
    let bytes = inner.as_bytes();
    let mut i = 0;
    let mut attrs = BTreeMap::new();
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] == b'/' {
            break;
        }
        let name_start = i;
        while i < bytes.len() && is_attr_name_byte(bytes[i]) {
            i += 1;
        }
        if i == name_start {
            i += 1;
            continue;
        }
        let name = inner[name_start..i].to_ascii_lowercase();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            attrs.insert(name, String::new());
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let value = if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
            let q = bytes[i];
            i += 1;
            let v_start = i;
            while i < bytes.len() && bytes[i] != q {
                i += 1;
            }
            let value = inner[v_start..i].to_string();
            if i < bytes.len() {
                i += 1;
            }
            value
        } else {
            let v_start = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'/' {
                i += 1;
            }
            inner[v_start..i].to_string()
        };
        attrs.insert(name, value);
    }
    attrs
}

fn is_attr_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b':' || b == b'_'
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
        let urls = find_url_spans(js);
        assert!(
            urls.iter()
                .all(|url| skip_host(url.host) || url.host != ".test"),
            "{urls:?}"
        );
        let with_real = r#"let t=/^\s*<\//.test(e.textAfter);fetch("https://cdn.example/x.js")"#;
        let found = find_url_spans(with_real);
        assert!(
            found.iter().any(|url| url.host == "cdn.example"),
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

    fn lint_snippet(name: &str, body: &str, links: &[(&str, &str)]) -> Vec<String> {
        let dir = tempfile::tempdir().unwrap();
        let dist = dir.path().join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        let contents = if name.ends_with(".js") {
            format!("{body}{}", "x".repeat(200))
        } else {
            body.to_string()
        };
        std::fs::write(dist.join(name), contents).unwrap();
        let allowed_links = links
            .iter()
            .map(|(host, reason)| ((*host).to_string(), (*reason).to_string()))
            .collect();
        let manifest = Manifest {
            host: Some("staging.neonlaw.com".into()),
            project: Some("acme".into()),
            allowed_links,
            ..Manifest::default()
        };
        lint(dir.path(), &[dir.path().to_path_buf()], &manifest)
            .into_iter()
            .map(|finding| finding.message)
            .collect()
    }

    #[test]
    fn allowed_links_table() {
        struct Case {
            name: &'static str,
            body: &'static str,
            links: &'static [(&'static str, &'static str)],
            expect: &'static str,
        }
        let cases = [
            Case {
                name: "index.html",
                body: r#"<a href="https://courts.example/r33" rel="noreferrer">rule</a>"#,
                links: &[("courts.example", "civil procedure")],
                expect: "ok",
            },
            Case {
                name: "index.html",
                body: r#"<a href="https://courts.example/r33" rel="noopener noreferrer">rule</a>"#,
                links: &[("courts.example", "civil procedure")],
                expect: "ok",
            },
            Case {
                name: "index.html",
                body: r#"<a href="https://courts.example/r33">rule</a>"#,
                links: &[("courts.example", "civil procedure")],
                expect: "missing-rel",
            },
            Case {
                name: "index.html",
                body: r#"<a href="https://caselaw.example/x">case</a>"#,
                links: &[("courts.example", "civil procedure")],
                expect: "unlisted",
            },
            Case {
                name: "app.js",
                body: r#"let html='<a href="https://courts.example/r33" rel="noreferrer">rule</a>';"#,
                links: &[("courts.example", "civil procedure")],
                expect: "ok",
            },
            Case {
                name: "app.js",
                body: r#"let html='<a href="https://courts.example/r33">rule</a>';"#,
                links: &[("courts.example", "civil procedure")],
                expect: "missing-rel",
            },
            Case {
                name: "app.js",
                body: r#"fetch("https://courts.example/x.js");"#,
                links: &[("courts.example", "civil procedure")],
                expect: "fetch",
            },
        ];
        for case in cases {
            let messages = lint_snippet(case.name, case.body, case.links);
            match case.expect {
                "ok" => assert!(
                    messages.is_empty(),
                    "{} {} -> {messages:?}",
                    case.name,
                    case.body
                ),
                "missing-rel" => assert!(
                    messages
                        .iter()
                        .any(|m| m.contains("allowed_links") && m.contains("noreferrer")),
                    "{} {} -> {messages:?}",
                    case.name,
                    case.body
                ),
                "unlisted" => assert!(
                    messages.iter().any(|m| m.contains("caselaw.example")
                        && !m.contains("missing rel=\"noreferrer\"")),
                    "{} {} -> {messages:?}",
                    case.name,
                    case.body
                ),
                "fetch" => assert!(
                    messages.iter().any(|m| m.contains("courts.example")
                        && !m.contains("missing rel=\"noreferrer\"")),
                    "{} {} -> {messages:?}",
                    case.name,
                    case.body
                ),
                other => panic!("unknown expect {other}"),
            }
        }
    }
}
