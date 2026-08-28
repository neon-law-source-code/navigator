//! The top-level `/portal` namespace is retired; this stops a route coming back.
//!
//! ENG-81 collapsed the old per-tier namespaces, `/portal` among them, into one
//! `/app` namespace, and the paths that used to hang off `/portal` now live under
//! `/app` — the government-forms index at `/app/forms`, the impersonation stop
//! at `/app/impersonation/stop`, a matter's notation documents under
//! `/app/projects/{code}/...`. Nothing serves a top-level `/portal` any more,
//! deliberately without a redirect shim, and
//! `server/tests/routes.rs::the_retired_project_prefixes_are_not_served` pins
//! that for the handful of paths that once existed.
//!
//! That test is not this one. It walks a fixed list of yesterday's paths and
//! asserts each answers `404`, so a route registered tomorrow at
//! `/portal/anything` is not in the list and sails straight past it. This guard
//! asserts over the *registrations* instead: every route path named anywhere in
//! the workspace, whether written as a literal at the registration site or as
//! the `&str` constant one passes. A path cannot be registered without its
//! literal existing somewhere, so covering both forms covers the namespace
//! rather than a snapshot of it.
//!
//! Asserting over the registrations rather than the live router is forced, not
//! preferred: `portal::mount` makes a brand host declare its `host_paths`
//! explicitly precisely because, as `portal/src/lib.rs` records, Axum's
//! `Router` "does not expose registered paths for inspection". There is no
//! assembled router to interrogate.
//!
//! **`portal` is three different words and only one of them is retired.** What
//! survives, and must keep working:
//!
//! * A **Project's client portal** at `/app/projects/{code}/portal` — one per
//!   matter, the client-facing React bundle. Live, current vocabulary, and the
//!   reason this guard matches a leading `/portal` rather than the word: a path
//!   that merely *ends* in `portal` is the thing we ship.
//! * The **`portal` Rust crate**, Navigator's Axum crate. Renaming it is a far
//!   larger blast radius and is not what this guard is for.
//! * A stale top-level `/portal` deep link arriving as `return_to` on the login
//!   door, which `portal::oauth::post_login_landing` folds into the caller's
//!   tier landing. That is input sanitizing on a link already sitting in sent
//!   email, not a route, so this guard does not see it — and should not.

use std::fs;
use std::path::{Path, PathBuf};

/// The Axum methods that register a path. `.nest(` currently appears nowhere in
/// the workspace and is listed anyway: a nest is how a whole retired subtree
/// would come back in one line.
const REGISTRATION_METHODS: &[&str] = &[".route(", ".route_service(", ".nest(", ".nest_service("];

/// Directories the walk never descends into: build output, VCS metadata, and
/// the local dev-environment scratch dir.
///
/// `worktrees` (no dot) is the directory the agent harness creates linked
/// checkouts under, and `.claude/worktrees/<branch>/` is a whole second copy of
/// the tree. Without it the walk reads another branch's files and reports them
/// against this one. CI has no such directory, which is the worst shape for a
/// guard: it would cry wolf exactly where someone is verifying a change and
/// never where it would catch one.
const SKIPPED_DIRS: &[&str] = &[
    "target",
    ".git",
    "node_modules",
    ".worktrees",
    "worktrees",
    ".devx",
];

/// Files exempt by provenance rather than by name: only this test, whose own
/// unit tests below are written in the thing it forbids.
const SKIPPED_FILES: &[&str] = &["portal_namespace_retired.rs"];

/// The workspace root (this test crate is `cli`).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// How a path reached the router.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SiteKind {
    /// A string literal passed straight to a registration method.
    Registration,
    /// A registration whose first argument is a named constant. The name is
    /// resolved against [`PathSite::Constant`] values collected tree-wide.
    RegistrationVia,
    /// A `const NAME: &str = "/…";` path constant, which exists to be
    /// registered even if this walk never sees the call site.
    Constant,
}

/// One path named in source, with where it was written.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PathSite {
    file: String,
    line: usize,
    /// The path literal, or for [`SiteKind::RegistrationVia`] the constant name.
    value: String,
    /// The declared name, for [`SiteKind::Constant`] only. Kept beside the
    /// value so a registration passing the constant can be matched to it by
    /// name — without this the two halves never meet and the
    /// constant-indirection check is vacuous.
    name: Option<String>,
    kind: SiteKind,
}

impl PathSite {
    fn describe(&self) -> String {
        let what = match self.kind {
            SiteKind::Registration => "registers",
            SiteKind::RegistrationVia => "registers the constant",
            SiteKind::Constant => "defines the route constant",
        };
        match (&self.name, self.kind) {
            (Some(name), SiteKind::Constant) => {
                format!(
                    "{}:{}: {what} `{name}` = `{}`",
                    self.file, self.line, self.value
                )
            }
            _ => format!("{}:{}: {what} `{}`", self.file, self.line, self.value),
        }
    }
}

/// Is `path` a top-level `/portal` route?
///
/// `/portal` itself and anything below it. Deliberately anchored at the front:
/// `/app/projects/{code}/portal` is a Project's client portal, which is live,
/// and `/portalish` is a different word.
fn is_top_level_portal(path: &str) -> bool {
    path == "/portal" || path.starts_with("/portal/")
}

/// Whether the byte at `offset` in `text` opens a real string literal, i.e. is
/// not itself escaped.
fn is_escaped(text: &str, offset: usize) -> bool {
    let mut backslashes = 0;
    for byte in text.as_bytes()[..offset].iter().rev() {
        if *byte == b'\\' {
            backslashes += 1;
        } else {
            break;
        }
    }
    backslashes % 2 == 1
}

/// The first argument of a call whose opening paren sits at `open`, as written.
///
/// Stops at the first comma outside any nesting or string, so a handler's own
/// literals — `get(|| async { "hi" })` — can never be mistaken for the path.
fn first_argument(text: &str, open: usize) -> Option<&str> {
    let bytes = text.as_bytes();
    let start = open + 1;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut i = start;
    while i < bytes.len() {
        let byte = bytes[i];
        if in_string {
            if byte == b'"' && !is_escaped(text, i) {
                in_string = false;
            }
            i += 1;
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                if depth == 0 {
                    return Some(&text[start..i]);
                }
                depth -= 1;
            }
            b',' if depth == 0 => return Some(&text[start..i]),
            _ => {}
        }
        i += 1;
    }
    None
}

/// The first string literal in `argument`, unescaped only enough to compare a
/// path: this walks route paths, which carry no escapes.
fn string_literal(argument: &str) -> Option<String> {
    let open = argument.find('"')?;
    let rest = &argument[open + 1..];
    let close = rest.find('"')?;
    Some(rest[..close].to_string())
}

/// The `SCREAMING_SNAKE` constant name an argument consists of, if that is all
/// it is. A qualified path keeps only its last segment, so
/// `dioxus_app::PROJECTS_PATH` resolves against the constant table.
fn constant_name(argument: &str) -> Option<String> {
    let trimmed = argument.trim();
    let last = trimmed.rsplit("::").next()?.trim();
    if last.is_empty() {
        return None;
    }
    let shaped = last
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && last.starts_with(|c: char| c.is_ascii_uppercase());
    shaped.then(|| last.to_string())
}

/// The 1-based line number of a byte offset.
fn line_of(text: &str, offset: usize) -> usize {
    text[..offset].bytes().filter(|b| *b == b'\n').count() + 1
}

/// Every registration and path constant in one file's source.
fn sites_in(source: &str, displayed: &str) -> Vec<PathSite> {
    let mut out = Vec::new();

    for method in REGISTRATION_METHODS {
        let mut from = 0;
        while let Some(found) = source[from..].find(method) {
            let at = from + found;
            let open = at + method.len() - 1;
            from = open + 1;
            let Some(argument) = first_argument(source, open) else {
                continue;
            };
            let line = line_of(source, at);
            if let Some(literal) = string_literal(argument) {
                out.push(PathSite {
                    file: displayed.to_string(),
                    line,
                    value: literal,
                    name: None,
                    kind: SiteKind::Registration,
                });
            } else if let Some(name) = constant_name(argument) {
                out.push(PathSite {
                    file: displayed.to_string(),
                    line,
                    value: name,
                    name: None,
                    kind: SiteKind::RegistrationVia,
                });
            }
        }
    }

    // `const NAME: &str = "/…";`, with or without `pub`, which is how 118 of
    // this workspace's route paths are written.
    for (index, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        let declaration = trimmed
            .strip_prefix("pub const ")
            .or_else(|| trimmed.strip_prefix("const "));
        let Some(declaration) = declaration else {
            continue;
        };
        let Some((name, value)) = declaration.split_once(": &str =") else {
            continue;
        };
        let Some(literal) = string_literal(value) else {
            continue;
        };
        if !literal.starts_with('/') {
            continue;
        }
        out.push(PathSite {
            file: displayed.to_string(),
            line: index + 1,
            value: literal,
            name: Some(name.trim().to_string()),
            kind: SiteKind::Constant,
        });
    }

    out
}

fn walk(dir: &Path, out: &mut Vec<PathSite>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // `.git` is a directory in a normal checkout and a *file* holding a
        // `gitdir:` pointer inside a worktree, so it is skipped by name before
        // the directory test.
        if SKIPPED_DIRS.contains(&name.as_ref()) {
            continue;
        }
        if path.is_dir() {
            walk(&path, out);
            continue;
        }
        if SKIPPED_FILES.contains(&name.as_ref()) {
            continue;
        }
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        let Ok(body) = fs::read_to_string(&path) else {
            continue;
        };
        let relative = path.strip_prefix(repo_root()).unwrap_or(&path);
        let displayed = relative.to_string_lossy().to_string();
        out.extend(sites_in(&body, &displayed));
    }
}

fn workspace_sites() -> Vec<PathSite> {
    let mut sites = Vec::new();
    walk(&repo_root(), &mut sites);
    sites
}

#[test]
fn no_route_registration_names_a_top_level_portal_path() {
    let sites = workspace_sites();

    // A constant holding a retired path is a registration waiting to happen,
    // so it fails on its own — and naming it here is what lets the
    // `RegistrationVia` arm below report the call site too.
    let retired_constants: Vec<&PathSite> = sites
        .iter()
        .filter(|site| site.kind == SiteKind::Constant && is_top_level_portal(&site.value))
        .collect();

    let mut hits: Vec<String> = sites
        .iter()
        .filter(|site| site.kind == SiteKind::Registration && is_top_level_portal(&site.value))
        .map(PathSite::describe)
        .collect();
    hits.extend(retired_constants.iter().map(|site| site.describe()));

    assert!(
        hits.is_empty(),
        "the top-level `/portal` namespace is retired — its paths live under \
         `/app` now, and nothing serves `/portal` (see \
         `server/tests/routes.rs::the_retired_project_prefixes_are_not_served`). \
         A Project's client portal at `/app/projects/{{code}}/portal` is a \
         different thing and is allowed. Found {} site(s) naming the retired \
         namespace:\n  {}",
        hits.len(),
        hits.join("\n  ")
    );
}

/// A registration whose constant argument resolves to a retired path.
///
/// Separate from the test above because it needs the constant table built
/// first: `.route(PORTAL_FORMS_PATH, …)` says nothing on its own line.
#[test]
fn no_registration_reaches_a_top_level_portal_path_through_a_constant() {
    let sites = workspace_sites();
    // Constant *names* whose value is a retired path — matched against what a
    // registration actually writes, which is the name and never the value.
    let retired: Vec<&str> = sites
        .iter()
        .filter(|site| site.kind == SiteKind::Constant && is_top_level_portal(&site.value))
        .filter_map(|site| site.name.as_deref())
        .collect();

    let hits: Vec<String> = sites
        .iter()
        .filter(|site| site.kind == SiteKind::RegistrationVia)
        .filter(|site| retired.contains(&site.value.as_str()))
        .map(PathSite::describe)
        .collect();

    assert!(
        hits.is_empty(),
        "these registrations pass a constant holding a retired top-level \
         `/portal` path:\n  {}",
        hits.join("\n  ")
    );
}

/// The collector has to actually find the router, or this whole file is a
/// guard that passes because it read nothing.
///
/// A broken extractor is the failure mode that matters: it would leave every
/// assertion above vacuously true while reporting green. The floors are well
/// under the current counts (176 `.route(` calls, 118 path constants) so
/// ordinary churn does not touch them, and the named anchors prove the two
/// extraction shapes — literal and constant — both still work.
#[test]
fn the_collector_finds_the_real_router() {
    let sites = workspace_sites();

    let registrations = sites
        .iter()
        .filter(|s| matches!(s.kind, SiteKind::Registration | SiteKind::RegistrationVia))
        .count();
    let constants = sites
        .iter()
        .filter(|s| s.kind == SiteKind::Constant)
        .count();

    assert!(
        registrations >= 120,
        "only {registrations} route registrations found — the extractor is \
         broken and every `/portal` assertion in this file is vacuous"
    );
    assert!(
        constants >= 90,
        "only {constants} path constants found — the constant extractor is \
         broken and a retired path could hide behind one"
    );

    let literal = sites.iter().any(|s| {
        s.kind == SiteKind::Registration && s.value == "/app/admin/people/{id}/impersonate"
    });
    assert!(
        literal,
        "the literal-argument shape stopped being extracted (expected \
         `/app/admin/people/{{id}}/impersonate` from portal/src/admin.rs)"
    );

    let via = sites
        .iter()
        .any(|s| s.kind == SiteKind::RegistrationVia && s.value == "PROJECT_PORTAL_PATH");
    assert!(
        via,
        "the constant-argument shape stopped being extracted (expected \
         `PROJECT_PORTAL_PATH` from portal/src/project_portal.rs)"
    );

    // The word this guard must *not* break: one Project's client portal is a
    // live route whose path ends in `portal`, registered through a constant.
    let project_portal = sites
        .iter()
        .any(|s| s.kind == SiteKind::Constant && s.value == "/app/projects/{project_code}/portal");
    assert!(
        project_portal,
        "a Project's client portal constant is missing — either the collector \
         broke or the route moved, and this guard's whole boundary is that it \
         permits that path"
    );
}

#[cfg(test)]
mod classifier {
    use super::*;

    #[test]
    fn a_top_level_portal_path_is_the_retired_namespace() {
        assert!(is_top_level_portal("/portal"));
        assert!(is_top_level_portal("/portal/"));
        assert!(is_top_level_portal("/portal/forms"));
        assert!(is_top_level_portal("/portal/forms/{file}"));
        assert!(is_top_level_portal("/portal/impersonation/stop"));
        assert!(is_top_level_portal(
            "/portal/notations/{id}/documents/{doc_id}"
        ));
    }

    /// The three live meanings of the word, none of which this guard may flag.
    #[test]
    fn a_project_client_portal_is_not_the_retired_namespace() {
        assert!(!is_top_level_portal("/app/projects/{project_code}/portal"));
        assert!(!is_top_level_portal("/app/projects/{project_code}/portal/"));
        assert!(!is_top_level_portal(
            "/app/projects/{project_code}/portal/{*asset}"
        ));
        assert!(!is_top_level_portal("/app/projects/some-code/portal"));
        // A different word that merely starts the same way.
        assert!(!is_top_level_portal("/portalish"));
        assert!(!is_top_level_portal("/app/forms"));
    }

    #[test]
    fn a_literal_registration_is_extracted_with_its_path() {
        let sites = sites_in(
            "let r = Router::new().route(\"/portal/forms\", get(forms));",
            "synthetic.rs",
        );
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].kind, SiteKind::Registration);
        assert_eq!(sites[0].value, "/portal/forms");
        assert!(is_top_level_portal(&sites[0].value));
    }

    /// The shape the guard exists for: a route added later, which no
    /// hardcoded list of yesterday's paths would catch.
    #[test]
    fn a_newly_added_portal_route_is_flagged() {
        let sites = sites_in(
            "        .route(\"/portal/something-nobody-has-thought-of\", post(h))",
            "synthetic.rs",
        );
        let flagged: Vec<&PathSite> = sites
            .iter()
            .filter(|s| is_top_level_portal(&s.value))
            .collect();
        assert_eq!(flagged.len(), 1, "{sites:?}");
    }

    #[test]
    fn a_registration_split_across_lines_is_extracted() {
        let sites = sites_in(
            "    .route(\n        \"/portal/forms/{file}\",\n        get(one),\n    )",
            "synthetic.rs",
        );
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].value, "/portal/forms/{file}");
    }

    #[test]
    fn a_constant_argument_is_recorded_by_name_not_mistaken_for_a_path() {
        let sites = sites_in("    .route(PORTAL_FORMS_PATH, get(forms))", "synthetic.rs");
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].kind, SiteKind::RegistrationVia);
        assert_eq!(sites[0].value, "PORTAL_FORMS_PATH");
    }

    #[test]
    fn a_qualified_constant_argument_resolves_to_its_last_segment() {
        let sites = sites_in(
            "    .route(dioxus_app::PROJECTS_PATH, page())",
            "synthetic.rs",
        );
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].value, "PROJECTS_PATH");
    }

    #[test]
    fn a_path_constant_declaration_is_collected() {
        let sites = sites_in(
            "pub const PORTAL_LANDING_PATH: &str = \"/portal\";",
            "synthetic.rs",
        );
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].kind, SiteKind::Constant);
        assert_eq!(sites[0].value, "/portal");
        assert!(is_top_level_portal(&sites[0].value));
        // The name is what a registration writes, so it has to be captured
        // beside the value or the indirection check above matches nothing.
        assert_eq!(sites[0].name.as_deref(), Some("PORTAL_LANDING_PATH"));
    }

    /// The two halves of the indirection have to meet: a constant's captured
    /// name must equal what the registration site records as its value.
    #[test]
    fn a_constants_name_matches_what_a_registration_records() {
        let sites = sites_in(
            "const PORTAL_FORMS_PATH: &str = \"/portal/forms\";\n\
             let r = Router::new().route(PORTAL_FORMS_PATH, get(forms));",
            "synthetic.rs",
        );
        let declared = sites
            .iter()
            .find(|s| s.kind == SiteKind::Constant)
            .expect("the declaration");
        let registered = sites
            .iter()
            .find(|s| s.kind == SiteKind::RegistrationVia)
            .expect("the registration");
        assert!(is_top_level_portal(&declared.value));
        assert_eq!(declared.name.as_deref(), Some(registered.value.as_str()));
    }

    /// A handler's own string literals sit inside the *second* argument, so
    /// parsing must stop at the first comma or the guard reads noise as paths.
    #[test]
    fn a_handlers_own_literal_is_not_read_as_the_path() {
        let sites = sites_in(
            "    .route(SOME_PATH, get(|| async { \"/portal/not-a-route\" }))",
            "synthetic.rs",
        );
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].kind, SiteKind::RegistrationVia);
        assert_eq!(sites[0].value, "SOME_PATH");
    }

    /// A non-path constant is not a route, and collecting it would dilute the
    /// anti-vacuity floor with unrelated strings.
    #[test]
    fn a_constant_that_is_not_a_path_is_ignored() {
        let sites = sites_in("const GREETING: &str = \"hello\";", "synthetic.rs");
        assert!(sites.is_empty(), "{sites:?}");
    }
}
