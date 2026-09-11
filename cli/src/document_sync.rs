//! Project document staging: upload bytes, retain only source-safe pointers.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use crate::remote::DocumentClient;

const GITIGNORE: &str = "*\n!*/\n!*.yml\n!.gitignore\n";

#[derive(Deserialize)]
pub(crate) struct ProjectManifest {
    pub(crate) project: String,
    pub(crate) host: Option<String>,
}

/// Read and validate `<root>/navigator.yaml` — the one manifest every
/// document command (`sync`, and the read verbs under `navigator site document`)
/// resolves its Project and login host from.
pub(crate) fn read_manifest(root: &Path) -> Result<ProjectManifest> {
    let manifest_path = root.join("navigator.yaml");
    let manifest: ProjectManifest = serde_yaml::from_str(
        &std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("read {}", manifest_path.display()))?,
    )
    .context("parse navigator.yaml")?;
    if manifest.project.trim().is_empty() {
        return Err(anyhow!("navigator.yaml must name a Project"));
    }
    Ok(manifest)
}

/// Synchronize the current Project repository's staged documents.
pub(crate) async fn run(root: &Path, dry_run: bool) -> ExitCode {
    match sync(root, dry_run).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("navigator: {error:#}");
            ExitCode::from(2)
        }
    }
}

async fn sync(root: &Path, dry_run: bool) -> Result<()> {
    let manifest = read_manifest(root)?;
    let documents = root.join("documents");
    let (binaries, pointers) = discover(&documents)?;
    if dry_run {
        for path in &binaries {
            println!("would upload {}", display_relative(root, path));
        }
        println!("{} upload planned", binaries.len());
        return Ok(());
    }

    std::fs::create_dir_all(&documents)
        .with_context(|| format!("create {}", documents.display()))?;
    let ignore = documents.join(".gitignore");
    if !ignore.exists() {
        std::fs::write(&ignore, GITIGNORE)
            .with_context(|| format!("write {}", ignore.display()))?;
    }

    let client = DocumentClient::connect(manifest.host.as_deref(), manifest.project.trim()).await?;

    // A committed visibility edit is desired state. Replaying it is safe and
    // lets the server's ordinary API audit record every reconciliation.
    for path in pointers {
        let raw =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let pointer = store::document_pointers::DocumentPointer::from_yaml(&raw)
            .with_context(|| format!("validate {}", path.display()))?;
        client
            .set_visibility(pointer.current_version.asset_id, &pointer.visibility)
            .await?;
    }

    let mut uploaded = 0usize;
    for path in binaries {
        let relative = path
            .strip_prefix(&documents)
            .map_err(|_| anyhow!("{} is outside documents/", path.display()))?;
        let slug = slash_path(relative)?;
        let kind = inferred_kind(relative);
        let existing_pointer = PathBuf::from(format!("{}.yml", path.display()));
        let desired_visibility = read_pointer(&existing_pointer)?
            .map_or_else(|| "internal".to_string(), |pointer| pointer.visibility);
        let pointer = client
            .upload(
                &path,
                kind,
                Some(&desired_visibility),
                None,
                Some(content_type(&path)),
                Some(&slug),
            )
            .await?;
        let pointer_path = PathBuf::from(format!("{}.yml", path.display()));
        write_pointer_atomically(&pointer_path, &pointer.to_yaml()?)?;
        std::fs::remove_file(&path).with_context(|| format!("remove staged {}", path.display()))?;
        uploaded += 1;
    }
    println!("{uploaded} uploaded");
    Ok(())
}

/// Synchronize the current Project repository's staged documents in the other
/// direction: download each committed pointer's own revision into the local
/// staging path it names. Hydrate-only — a live document the checkout carries
/// no pointer for is not imported; that remains `site sync`'s and a browser
/// filing's own lane.
pub(crate) async fn run_pull(root: &Path, dry_run: bool) -> ExitCode {
    match pull(root, dry_run).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("navigator: {error:#}");
            ExitCode::from(2)
        }
    }
}

/// The local staging path a pointer's own bytes belong at — its committed
/// path with the trailing `.yml` stripped. Refuses to resolve outside
/// `<root>/documents/`, the same lexical guard `document get` uses in the
/// other direction (`document_read::refuse_destination_in_documents`).
fn pull_target(root: &Path, pointer_relative: &Path) -> Result<PathBuf> {
    let stem = pointer_relative
        .to_str()
        .and_then(|path| path.strip_suffix(".yml"))
        .ok_or_else(|| {
            anyhow!(
                "{} does not name a `.yml` pointer",
                pointer_relative.display()
            )
        })?;
    let documents_dir = crate::document_read::lexical(root, Path::new("documents"));
    let resolved = crate::document_read::lexical(root, Path::new(stem));
    if !resolved.starts_with(&documents_dir) {
        return Err(anyhow!(
            "refusing to write {} outside {}",
            resolved.display(),
            documents_dir.display()
        ));
    }
    Ok(resolved)
}

/// Whether the file at `path` already carries `expected_sha256` — a missing
/// file never matches, so a fresh clone always pulls.
fn matches_digest(path: &Path, expected_sha256: &str) -> Result<bool> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(store::documents::sha256_hex(&bytes) == expected_sha256),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

async fn pull(root: &Path, dry_run: bool) -> Result<()> {
    let manifest = read_manifest(root)?;
    let pointers = crate::document_read::discover_pointers(root)?;
    if pointers.is_empty() {
        println!("no documents/ pointers to pull");
        return Ok(());
    }

    if dry_run {
        let mut planned = 0usize;
        for relative in &pointers {
            let pointer_path = root.join(relative);
            let pointer = read_pointer(&pointer_path)?
                .ok_or_else(|| anyhow!("{} vanished mid-scan", pointer_path.display()))?;
            let target = pull_target(root, relative)?;
            if !matches_digest(&target, &pointer.current_version.sha256)? {
                println!("would pull {}", display_relative(root, &target));
                planned += 1;
            }
        }
        println!("{planned} pull(s) planned");
        return Ok(());
    }

    let documents = root.join("documents");
    std::fs::create_dir_all(&documents)
        .with_context(|| format!("create {}", documents.display()))?;
    let ignore = documents.join(".gitignore");
    if !ignore.exists() {
        std::fs::write(&ignore, GITIGNORE)
            .with_context(|| format!("write {}", ignore.display()))?;
    }

    let client = DocumentClient::connect(manifest.host.as_deref(), manifest.project.trim()).await?;
    let staging = tempfile::Builder::new()
        .prefix(".navigator-pull-")
        .tempdir_in(root)
        .with_context(|| format!("create pull staging area in {}", root.display()))?;
    let mut pulled = 0usize;
    let mut failures = Vec::new();
    let mut staged = Vec::new();
    for relative in &pointers {
        let pointer_path = root.join(relative);
        let Some(pointer) = read_pointer(&pointer_path)? else {
            failures.push(format!("{} vanished mid-scan", pointer_path.display()));
            continue;
        };
        let target = match pull_target(root, relative) {
            Ok(target) => target,
            Err(error) => {
                failures.push(format!("{}: {error}", pointer_path.display()));
                continue;
            }
        };
        if matches_digest(&target, &pointer.current_version.sha256)? {
            continue;
        }
        match client
            .download_revision(pointer.current_version.asset_id)
            .await
        {
            Ok(bytes) => {
                let actual = store::documents::sha256_hex(&bytes);
                if actual != pointer.current_version.sha256 {
                    failures.push(format!(
                        "{}: sha256 mismatch: pointer says {}, download says {actual}",
                        pointer_path.display(),
                        pointer.current_version.sha256
                    ));
                    continue;
                }
                let staged_path = staging.path().join(staged.len().to_string());
                std::fs::write(&staged_path, &bytes)
                    .with_context(|| format!("stage {}", target.display()))?;
                staged.push((target, staged_path));
                pulled += 1;
            }
            Err(error) => {
                failures.push(format!("{}: {error}", pointer_path.display()));
            }
        }
    }
    if !failures.is_empty() {
        for failure in &failures {
            eprintln!("{failure}");
        }
        return Err(anyhow!(
            "{} of {} pointer(s) failed to pull",
            failures.len(),
            pointers.len()
        ));
    }
    for (target, staged_path) in staged {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        std::fs::copy(&staged_path, &target)
            .with_context(|| format!("publish {}", target.display()))?;
    }
    println!("{pulled} pulled");
    Ok(())
}

fn discover(documents: &Path) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    if !documents.exists() {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut binaries = Vec::new();
    let mut pointers = Vec::new();
    for entry in walkdir::WalkDir::new(documents).follow_links(false) {
        let entry = entry.with_context(|| format!("walk {}", documents.display()))?;
        if !entry.file_type().is_file() || entry.file_name() == ".gitignore" {
            continue;
        }
        if entry.path().extension().and_then(|ext| ext.to_str()) == Some("yml") {
            pointers.push(entry.into_path());
        } else {
            binaries.push(entry.into_path());
        }
    }
    binaries.sort();
    pointers.sort();
    Ok((binaries, pointers))
}

fn inferred_kind(relative: &Path) -> &'static str {
    match relative
        .components()
        .next()
        .and_then(|part| part.as_os_str().to_str())
    {
        Some("pleadings") => "filing",
        Some("exhibits") => "exhibit",
        Some("agreements") => "agreement",
        _ => "unclassified",
    }
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("pdf") => "application/pdf",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("txt") => "text/plain",
        Some("docx") => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        _ => "application/octet-stream",
    }
}

pub(crate) fn slash_path(path: &Path) -> Result<String> {
    let parts = path
        .components()
        .map(|part| {
            part.as_os_str()
                .to_str()
                .ok_or_else(|| anyhow!("document path is not valid UTF-8"))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(parts.join("/"))
}

fn display_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .ok()
        .and_then(|relative| slash_path(relative).ok())
        .unwrap_or_else(|| path.display().to_string())
}

pub(crate) fn write_pointer_atomically(path: &Path, yaml: &str) -> Result<()> {
    let temp = PathBuf::from(format!("{}.tmp-{}", path.display(), uuid::Uuid::now_v7()));
    std::fs::write(&temp, yaml).with_context(|| format!("write {}", temp.display()))?;
    std::fs::rename(&temp, path).with_context(|| format!("publish {}", path.display()))
}

pub(crate) fn read_pointer(
    path: &Path,
) -> Result<Option<store::document_pointers::DocumentPointer>> {
    match std::fs::read_to_string(path) {
        Ok(raw) => store::document_pointers::DocumentPointer::from_yaml(&raw)
            .with_context(|| format!("validate {}", path.display()))
            .map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::{matches_digest, pull_target};
    use std::path::Path;

    #[test]
    fn pull_target_strips_yml_and_stays_below_documents() {
        let root = Path::new("/repo");
        assert_eq!(
            pull_target(root, Path::new("documents/pleadings/motion.pdf.yml")).unwrap(),
            Path::new("/repo/documents/pleadings/motion.pdf")
        );
    }

    #[test]
    fn pull_target_refuses_a_pointer_that_would_escape_the_checkout() {
        let root = Path::new("/repo");
        let error = pull_target(root, Path::new("documents/../../etc/passwd.yml")).unwrap_err();
        assert!(error.to_string().contains("outside"), "{error}");
    }

    #[test]
    fn matches_digest_is_false_for_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!matches_digest(&dir.path().join("absent.pdf"), &"a".repeat(64)).unwrap());
    }

    #[test]
    fn matches_digest_compares_the_actual_sha256() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("present.pdf");
        std::fs::write(&path, b"hello").unwrap();
        let sha256 = store::documents::sha256_hex(b"hello");
        assert!(matches_digest(&path, &sha256).unwrap());
        assert!(!matches_digest(&path, &"0".repeat(64)).unwrap());
    }
}
