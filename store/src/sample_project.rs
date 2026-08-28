//! Publishing a built **sample project** bundle into the applications bucket.
//!
//! Each sample matter carries a client portal at
//! `/app/projects/{code}/portal/`. Local development refreshes the real Vite
//! build of that matter's own repository — `neon-law-staging/sample-litigation`,
//! `-transactional`, and `-estate` — stages each `dist/` beside the
//! `navigator.yaml` that names its Project, and points [`STAGE_ENV`] at the
//! directory holding all three through generated `.devx/env`.
//!
//! Boot re-reads the manifest rather than trusting the staging path, and
//! refuses a bundle naming a different Project — publishing one would put a
//! matter's application on another matter's portal. Everything that decides
//! *what* gets published is pure and unit-tested here; the seed owns only the
//! `await`s.
//!
//! ## Why the ordering is load-bearing
//!
//! `index.html` publishes **last**. Until it lands, the previous `index.html`
//! is still live and still references the previous hashed assets, so a reader
//! mid-publish sees a whole old bundle rather than a new document pointing at
//! assets that do not exist yet. Nothing is ever deleted for the same reason.

use std::path::{Path, PathBuf};

/// Names the directory the staged projects sit under — one subdirectory per
/// Project code, each a `navigator.yaml` beside a built `dist/`. Generated
/// local development environments set it before `web` starts.
pub const STAGE_ENV: &str = "NAVIGATOR_SAMPLE_PROJECTS_DIR";

/// The manifest a project application carries at its root. It names the
/// Project the bundle belongs to, so the publish prefix is declared by the
/// application rather than hardcoded here.
///
/// The same filename and `project:` key a Project repository declares itself
/// in (`cli::projects::repository::PROJECT_MANIFEST`) — a bundle's staged
/// manifest is a copy of that same file, not a distinct schema. Unknown keys,
/// such as `host:`, are ignored rather than refused: a repository's own
/// manifest may carry them, and this reader only needs `project:`.
pub const MANIFEST_FILE: &str = "navigator.yaml";

/// The built-bundle directory inside the staged project.
pub const DIST_DIR: &str = "dist";

/// The application segment of the mount. `portal` is a literal segment of
/// Navigator's route — `/app/projects/{code}/portal` — not a name looked up
/// per Project, so it is a constant here rather than a manifest field.
pub const PORTAL_APPLICATION: &str = "portal";

/// The document a browser lands on, and the last object published.
pub const ENTRY_DOCUMENT: &str = "index.html";

/// `index.html` must never be cached: it is the pointer to the current hashed
/// assets, so a cached copy pins a reader to a bundle that may be gone.
pub const ENTRY_CACHE_CONTROL: &str = "no-store";

/// Hashed assets are immutable by construction — the hash changes when the
/// bytes do — and `private` because this bucket is participation-gated and
/// must not land in a shared cache.
pub const ASSET_CACHE_CONTROL: &str = "private, max-age=31536000, immutable";

/// A project application's `navigator.yaml`.
///
/// One field read here. It is a manifest rather than a convention over the
/// repository name because the repository name is the application's to choose:
/// a repository may be named for something other than the Project it mounts on,
/// and no rule can recover the code from a name that never encoded it. The
/// sample repositories happen to be named for their codes, which makes the two
/// agree — by convention, not because anything derives one from the other.
///
/// Unknown keys are ignored rather than refused (`#[derive(Deserialize)]`
/// carries no `deny_unknown_fields`): a Project repository's own manifest also
/// carries `host:`, and this reader has no business rejecting a key it does
/// not need.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Manifest {
    /// The Project code this bundle belongs to.
    pub project: String,
}

/// Why a manifest could not be turned into a publish prefix.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("navigator.yaml is not valid YAML: {0}")]
    Unparsable(String),
    #[error("navigator.yaml names `{0}`, which is not a valid Project code")]
    InvalidCode(String),
    #[error("navigator.yaml names Project `{found}`, but this bundle mounts on `{expected}`")]
    WrongProject { expected: String, found: String },
}

/// Read the Project code a bundle declares, and prove it is one.
///
/// The code is validated with the same [`crate::projects::is_valid_code`] the
/// store uses, so a manifest cannot introduce a code the rest of Navigator
/// would reject — and cannot smuggle path segments into a bucket key.
pub fn project_code_from_manifest(yaml: &str) -> Result<String, ManifestError> {
    let manifest: Manifest =
        serde_yaml::from_str(yaml).map_err(|e| ManifestError::Unparsable(e.to_string()))?;
    let name = manifest.project.trim().to_string();
    if !crate::projects::is_valid_code(&name) {
        return Err(ManifestError::InvalidCode(name));
    }
    Ok(name)
}

/// [`project_code_from_manifest`], and additionally that the bundle declares
/// the Project it is about to be published under.
///
/// Publishing a bundle that names a different Project would put one matter's
/// application on another matter's portal, so a mismatch is refused rather
/// than reconciled.
pub fn project_code_for(yaml: &str, expected: &str) -> Result<String, ManifestError> {
    let found = project_code_from_manifest(yaml)?;
    if found == expected {
        Ok(found)
    } else {
        Err(ManifestError::WrongProject {
            expected: expected.to_string(),
            found,
        })
    }
}

/// The bucket prefix a Project's portal streams from —
/// `{project_code}/{application}`. Flat, latest-only: no `<sha>/` segment and
/// no pointer object.
#[must_use]
pub fn portal_prefix(project_code: &str) -> String {
    format!("{project_code}/{PORTAL_APPLICATION}")
}

/// One object to publish, fully resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalObject {
    /// Bucket key, always under [`portal_prefix`].
    pub key: String,
    /// Path on disk to read the bytes from.
    pub source: PathBuf,
    pub content_type: &'static str,
    pub cache_control: &'static str,
}

/// Map an extension to a content type. Unknown extensions fall back to
/// `application/octet-stream` rather than guessing: a wrong `text/*` would be
/// sniffed and rendered, which is worse than an honest download.
pub fn content_type_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        // A sourcemap is JSON; browsers fetch it by that content type.
        Some("json" | "map") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Whether `relative` is the bundle's entry document — the root `index.html`,
/// not a nested one. A nested `index.html` is an ordinary asset.
fn is_entry_document(relative: &Path) -> bool {
    relative == Path::new(ENTRY_DOCUMENT)
}

/// Build the ordered publish plan for a built `dist/` directory.
///
/// Every file under `dist` is included, recursively, keyed by its path
/// relative to `dist`. The root `index.html` sorts last; everything else keeps
/// a stable lexical order so a publish is reproducible and a diff of two plans
/// is readable.
///
/// Returns an empty plan for a directory with no entry document — a `dist`
/// without `index.html` is a failed build, and publishing its assets alone
/// would strand the live bundle.
///
/// `project_code` comes from the bundle's own `navigator.yaml`, already
/// validated by [`project_code_from_manifest`].
pub fn publish_plan(dist: &Path, project_code: &str) -> std::io::Result<Vec<PortalObject>> {
    let prefix = portal_prefix(project_code);
    let mut files = Vec::new();
    collect_files(dist, dist, &mut files)?;
    files.sort();

    if !files.iter().any(|relative| is_entry_document(relative)) {
        return Ok(Vec::new());
    }

    let mut plan: Vec<PortalObject> = Vec::with_capacity(files.len());
    for relative in files {
        let entry = is_entry_document(&relative);
        // Bucket keys are `/`-joined regardless of host separator.
        let joined = relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        plan.push(PortalObject {
            key: format!("{prefix}/{joined}"),
            source: dist.join(&relative),
            content_type: content_type_for(&relative),
            cache_control: if entry {
                ENTRY_CACHE_CONTROL
            } else {
                ASSET_CACHE_CONTROL
            },
        });
    }
    // The entry document last, everything it references already in place.
    plan.sort_by_key(|object| object.key.ends_with(ENTRY_DOCUMENT));
    Ok(plan)
}

/// Recursively collect files under `dir`, as paths relative to `root`.
/// Symlinks are followed only as far as `metadata` reports a file, so a link
/// pointing outside the build cannot smuggle a directory walk elsewhere.
fn collect_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = std::fs::metadata(&path)?;
        if metadata.is_dir() {
            collect_files(root, &path, out)?;
        } else if metadata.is_file() {
            if let Ok(relative) = path.strip_prefix(root) {
                out.push(relative.to_path_buf());
            }
        }
    }
    Ok(())
}

/// A staged project on disk: the manifest that names its Project, and the
/// built bundle to publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedProject {
    /// The staged project root, holding `navigator.yaml` and `dist/`.
    pub root: PathBuf,
    /// The built bundle inside it.
    pub dist: PathBuf,
}

impl StagedProject {
    /// The manifest path, whether or not it exists.
    #[must_use]
    pub fn manifest(&self) -> PathBuf {
        self.root.join(MANIFEST_FILE)
    }
}

/// Resolve one matter's staged project from the environment, if it is both
/// configured and present. A configured-but-missing directory is *not* an
/// error: a worktree whose `.devx` was torn down should keep the deterministic
/// portal document rather than fail boot.
///
/// [`STAGE_ENV`] names the directory the staged projects sit *under*, one
/// per Project code, because the fixture carries several matters and each has
/// its own repository and its own build. The code selects the subdirectory;
/// the manifest inside it still decides which Project the bundle publishes
/// under, so a directory named after one matter cannot publish another's
/// application by sitting in the wrong folder.
#[must_use]
pub fn staged_for(project_code: &str) -> Option<StagedProject> {
    staged_from(project_code, |key| std::env::var(key).ok())
}

/// [`staged_for`] with the environment read through `get`, so the
/// decision is testable without mutating process state.
pub fn staged_from(
    project_code: &str,
    get: impl Fn(&str) -> Option<String>,
) -> Option<StagedProject> {
    let configured = get(STAGE_ENV)?;
    let trimmed = configured.trim();
    if trimmed.is_empty() {
        return None;
    }
    // A code that is not a valid Project code could otherwise walk out of the
    // staging root with `..`; every caller passes a compiled-in constant, and
    // this keeps that true of any future one.
    if !crate::projects::is_valid_code(project_code) {
        return None;
    }
    let root = PathBuf::from(trimmed).join(project_code);
    let dist = root.join(DIST_DIR);
    // Both halves must be there: a root without `dist/` is a checkout nobody
    // built, and publishing from it would strand the live bundle.
    (root.is_dir() && dist.is_dir()).then_some(StagedProject { root, dist })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a file, creating parents, so a test can describe a `dist` tree
    /// by its leaves.
    fn write(root: &Path, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("create dirs");
        std::fs::write(path, bytes).expect("write");
    }

    #[test]
    fn plan_publishes_the_entry_document_last() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write(root, "index.html", b"<!doctype html>");
        write(root, "assets/app-abc123.js", b"console.log(1)");
        write(root, "assets/app-abc123.css", b"body{}");

        let plan = publish_plan(root, "sample-litigation").expect("plan");

        assert_eq!(plan.len(), 3);
        assert_eq!(
            plan.last().expect("a last object").key,
            "sample-litigation/portal/index.html",
            "index.html must land after the assets it references"
        );
    }

    #[test]
    fn plan_is_empty_without_an_entry_document() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write(root, "assets/app-abc123.js", b"console.log(1)");

        assert!(
            publish_plan(root, "sample-litigation")
                .expect("plan")
                .is_empty(),
            "a dist with no index.html is a failed build, not a partial publish"
        );
    }

    #[test]
    fn plan_keys_are_prefixed_and_slash_joined() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write(root, "index.html", b"x");
        write(root, "assets/fonts/gorp.woff2", b"x");

        let keys: Vec<String> = publish_plan(root, "sample-litigation")
            .expect("plan")
            .into_iter()
            .map(|o| o.key)
            .collect();

        assert!(keys.contains(&"sample-litigation/portal/assets/fonts/gorp.woff2".to_string()));
        assert!(keys
            .iter()
            .all(|k| k.starts_with("sample-litigation/portal/")));
    }

    #[test]
    fn entry_document_is_never_cached_and_assets_are_immutable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write(root, "index.html", b"x");
        write(root, "assets/app-abc123.js", b"x");

        let plan = publish_plan(root, "sample-litigation").expect("plan");
        let find = |key: &str| {
            plan.iter()
                .find(|o| o.key == key)
                .unwrap_or_else(|| panic!("{key} in the plan"))
        };
        let entry = find("sample-litigation/portal/index.html");
        let asset = find("sample-litigation/portal/assets/app-abc123.js");

        assert_eq!(entry.cache_control, "no-store");
        assert_eq!(asset.cache_control, "private, max-age=31536000, immutable");
    }

    #[test]
    fn a_nested_index_html_is_an_ordinary_asset() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write(root, "index.html", b"x");
        write(root, "docs/index.html", b"x");

        let plan = publish_plan(root, "sample-litigation").expect("plan");
        let nested = plan
            .iter()
            .find(|o| o.key == "sample-litigation/portal/docs/index.html")
            .expect("the nested document");

        assert_eq!(
            nested.cache_control, ASSET_CACHE_CONTROL,
            "only the root document is the bundle's pointer"
        );
        assert_eq!(
            plan.last().expect("a last object").key,
            "sample-litigation/portal/index.html"
        );
    }

    #[test]
    fn content_types_cover_the_shapes_a_vite_build_emits() {
        assert_eq!(
            content_type_for(Path::new("index.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            content_type_for(Path::new("app.js")),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(content_type_for(Path::new("a.woff2")), "font/woff2");
        assert_eq!(content_type_for(Path::new("a.svg")), "image/svg+xml");
        assert_eq!(
            content_type_for(Path::new("noextension")),
            "application/octet-stream",
            "guessing a text type would get the bytes sniffed and rendered"
        );
    }

    #[test]
    fn staged_ignores_unset_blank_missing_and_unbuilt_directories() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let configured = root.to_string_lossy().into_owned();

        assert_eq!(staged_from("sample-litigation", |_| None), None);
        assert_eq!(
            staged_from("sample-litigation", |_| Some("   ".to_string())),
            None
        );
        assert_eq!(
            staged_from("sample-litigation", |_| Some(
                "/nonexistent/navigator/sample".to_string()
            )),
            None,
            "a missing staged bundle is not treated as a built application"
        );

        // Present, but nobody built it.
        let unbuilt = configured.clone();
        assert_eq!(
            staged_from("sample-litigation", move |_| Some(unbuilt.clone())),
            None,
            "a checkout with no dist/ is not something to publish"
        );

        let staged_root = root.join("sample-litigation");
        std::fs::create_dir_all(staged_root.join(DIST_DIR)).expect("mkdir");
        let built = configured.clone();
        let staged = staged_from("sample-litigation", move |_| Some(built.clone()))
            .expect("a staged project");
        assert_eq!(staged.dist, staged_root.join("dist"));
        assert_eq!(staged.manifest(), staged_root.join("navigator.yaml"));
    }

    /// Each matter resolves its own subdirectory under the one staging root,
    /// and a matter nobody staged resolves to nothing rather than to a
    /// sibling's bundle. Publishing one client's application on another
    /// client's portal is the failure this shape exists to prevent.
    #[test]
    fn each_matter_stages_under_its_own_code() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let configured = root.to_string_lossy().into_owned();
        std::fs::create_dir_all(root.join("sample-transactional").join(DIST_DIR)).expect("mkdir");

        let get = |_: &str| Some(configured.clone());
        let staged = staged_from("sample-transactional", get).expect("the staged matter");
        assert_eq!(staged.root, root.join("sample-transactional"));
        assert_eq!(
            staged_from("sample-estate", get),
            None,
            "an unstaged matter resolves to nothing, never to a sibling's bundle"
        );
    }

    /// A code that is not a valid Project code never reaches the filesystem,
    /// so no caller can walk out of the staging root through the segment.
    #[test]
    fn an_invalid_project_code_stages_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let configured = dir.path().to_string_lossy().into_owned();
        std::fs::create_dir_all(dir.path().join(DIST_DIR)).expect("mkdir");

        assert_eq!(
            staged_from("../..", |_| Some(configured.clone())),
            None,
            "a traversal segment is refused before it is joined"
        );
    }

    #[test]
    fn the_manifest_names_the_project_the_bundle_mounts_on() {
        assert_eq!(
            project_code_from_manifest("project: sample-litigation\n").expect("a code"),
            "sample-litigation"
        );
    }

    /// The retired `name:` key is not a quiet alias for `project:` — there is
    /// no serde alias, so a manifest still spelled the old way is exactly as
    /// unreadable as one carrying neither key. Deserializing into a required
    /// `project: String` field with nothing to fill it is a missing-field
    /// parse failure, which `project_code_from_manifest` reports as
    /// `Unparsable`.
    #[test]
    fn the_retired_name_key_is_rejected_rather_than_read() {
        assert!(matches!(
            project_code_from_manifest("name: sample-litigation\n"),
            Err(ManifestError::Unparsable(_))
        ));
    }

    /// A manifest may carry keys this reader does not need — `host:`, the one
    /// a Project repository's own manifest adds — without failing to parse.
    /// This is what keeps the downstream `DEPLOYMENTS` table's own additions
    /// from breaking this gate.
    #[test]
    fn an_unknown_key_alongside_project_is_ignored() {
        assert_eq!(
            project_code_from_manifest("host: www.example.com\nproject: sample-litigation\n")
                .expect("a code"),
            "sample-litigation"
        );
    }

    #[test]
    fn a_manifest_cannot_introduce_a_code_the_store_would_reject() {
        // Path segments are the dangerous case: a bucket key is built from
        // this, so `../` or a slash must never survive validation. `new` is
        // reserved; uppercase is refused because a code is also a
        // case-insensitive drive folder name.
        for bad in [
            "../etc",
            "Donut-Litigation",
            "sample-litigation/portal",
            "",
            "new",
            "-x",
            "a--b",
        ] {
            assert!(
                matches!(
                    project_code_from_manifest(&format!("project: \"{bad}\"\n")),
                    Err(ManifestError::InvalidCode(_))
                ),
                "`{bad}` must not validate as a Project code"
            );
        }
    }

    #[test]
    fn unparsable_yaml_is_an_error_rather_than_a_default() {
        assert!(matches!(
            project_code_from_manifest("project: [this is not a string]\n"),
            Err(ManifestError::Unparsable(_))
        ));
    }

    #[test]
    fn a_bundle_declaring_another_project_is_refused() {
        assert_eq!(
            project_code_for("project: henderson\n", "sample-litigation"),
            Err(ManifestError::WrongProject {
                expected: "sample-litigation".to_string(),
                found: "henderson".to_string(),
            }),
            "one matter's application must not land on another matter's portal"
        );
        assert_eq!(
            project_code_for("project: sample-litigation\n", "sample-litigation").expect("a code"),
            "sample-litigation"
        );
    }

    #[test]
    fn the_prefix_is_derived_from_the_declared_project() {
        assert_eq!(
            portal_prefix("sample-litigation"),
            "sample-litigation/portal"
        );
        assert_eq!(portal_prefix("henderson"), "henderson/portal");
    }
}
