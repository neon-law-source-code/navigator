//! Guard: nothing `web` serves may pull a subresource from a CDN.
//!
//! Every CSS/JS/font/image the lawyer's or applicant's browser loads must
//! come from our own same-origin `/public` mount (axum `ServeDir`), never an
//! unpinned third party. The runtime backstop is the CSP (`script-src 'self'`
//! in `portal::csp_value`; the Dioxus render route carries its own per-response
//! policy, which nonces the hydration scripts and admits the deployment asset
//! origin for passive images and the licensed webfonts only — code never comes
//! from off origin), but a blocked request still means a broken page —
//! this test catches the offending byte at build time, before it ships.
//!
//! Companion to `vendor_assets.rs`: that one proves the vendored blobs match
//! their pinned hashes; this one proves nothing we serve reaches *off* origin
//! to load code or styling in the first place. Provenance comments and the
//! `VENDOR.toml` `upstream_url` records (where a blob was *downloaded* from,
//! once, by a human) are not runtime loads and are out of scope.

use std::path::{Path, PathBuf};

fn public_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("public")
}

/// Recursively collect every file under `dir` whose extension is in `exts`.
fn collect(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect(&path, exts, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| exts.contains(&e))
        {
            out.push(path);
        }
    }
}

/// Tags whose `src`/`href` cause the browser to fetch a subresource. A bare
/// `<a href>` is a hyperlink, not a load, so it is intentionally absent —
/// linking out to (say) the MCP spec is fine; loading a stylesheet from a CDN
/// is not.
const SUBRESOURCE_TAGS: &[&str] = &["script", "link", "img", "source", "iframe", "embed"];

/// Hostnames that mean "this is a CDN load." Used for the first-party JS scan,
/// where a raw `http` substring (e.g. an SVG `xmlns`) would false-positive.
const CDN_HOSTS: &[&str] = &[
    "cdn.jsdelivr.net",
    "unpkg.com",
    "cdnjs.cloudflare.com",
    "fonts.googleapis.com",
    "fonts.gstatic.com",
    "stackpath.bootstrapcdn.com",
    "maxcdn.bootstrapcdn.com",
    "code.jquery.com",
    "ajax.googleapis.com",
];

#[test]
fn served_html_loads_no_subresource_from_an_external_origin() {
    let public = public_dir();
    let mut files = Vec::new();
    collect(&public, &["html"], &mut files);
    assert!(
        !files.is_empty(),
        "expected at least the Swagger UI index.html under {}",
        public.display()
    );

    for path in &files {
        let html = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        // Walk every `<tag …>` opening. For a subresource tag, a `src=` or
        // `href=` whose value starts with `http://` or `https://` is an
        // off-origin load — the exact thing we forbid.
        for chunk in html.split('<').skip(1) {
            let tag = chunk
                .split([' ', '\t', '\n', '>', '/'])
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            if !SUBRESOURCE_TAGS.contains(&tag.as_str()) {
                continue;
            }
            let attrs = chunk.split('>').next().unwrap_or("");
            for needle in ["src", "href"] {
                if let Some(val) = attr_value(attrs, needle) {
                    assert!(
                        !(val.starts_with("http://") || val.starts_with("https://")),
                        "{}: <{tag}> loads a subresource from an external origin: {val}\n\
                         Vendor it under web/public/ and serve it from /public instead.",
                        path.display(),
                    );
                }
            }
        }
    }
}

#[test]
fn served_css_imports_nothing_from_an_external_origin() {
    let public = public_dir();
    let mut files = Vec::new();
    collect(&public, &["css"], &mut files);

    for path in &files {
        let css = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        // `url(http…)` (a font/image fetch) and `@import "http…"` are the two
        // ways CSS reaches off origin. Same-origin `url(./fonts/…)` /
        // `url(data:…)` are fine.
        for marker in ["url(http", "url( http", "url(\"http", "url('http"] {
            assert!(
                !css.contains(marker),
                "{}: CSS fetches `{marker}…` from an external origin — \
                 vendor the resource under web/public/ instead.",
                path.display(),
            );
        }
        for import in [
            "@import \"http",
            "@import 'http",
            "@import url(http",
            "@import http",
        ] {
            assert!(
                !css.contains(import),
                "{}: CSS `@import`s `{import}…` from an external origin — \
                 vendor it under web/public/ instead.",
                path.display(),
            );
        }
    }
}

/// Hand-authored first-party scripts — the ones we control. Vendored minified
/// bundles (Bootstrap, HTMX, Alpine, the Swagger UI bundles) are locked by
/// `vendor_assets.rs` / their own provenance and may legitimately carry `http`
/// namespace URIs inside string literals, so they are not line-scanned here.
const FIRST_PARTY_JS: &[&str] = &[
    "js/northstar-review.js",
    "js/collage-lightbox.js",
    "js/workshop-progress.js",
    "js/upload-progress.js",
    "swagger-ui/init.js",
];

#[test]
fn first_party_scripts_name_no_cdn_host() {
    let public = public_dir();
    for rel in FIRST_PARTY_JS {
        let path = public.join(rel);
        let js = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for host in CDN_HOSTS {
            assert!(
                !js.contains(host),
                "{rel}: first-party script references CDN host `{host}` — \
                 load the asset from the same-origin /public mount instead.",
            );
        }
    }
}

/// Pull the value of `name="…"` (single or double quoted) out of a tag's
/// attribute span. Minimal — enough for the well-formed HTML we author.
fn attr_value<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = attrs;
    loop {
        let idx = rest.find(name)?;
        let after = &rest[idx + name.len()..];
        let trimmed = after.trim_start();
        // Guard against matching `href` inside another attribute name: the
        // char before `name` must be a boundary, and the char after must be
        // `=` (optionally spaced).
        let boundary = idx == 0
            || rest[..idx]
                .chars()
                .next_back()
                .is_some_and(|c| c == ' ' || c == '\t' || c == '\n');
        if boundary {
            if let Some(eq) = trimmed.strip_prefix('=') {
                let v = eq.trim_start();
                let quote = v.chars().next()?;
                if quote == '"' || quote == '\'' {
                    return v[1..].split(quote).next();
                }
            }
        }
        rest = after;
    }
}
