//! Project document staging: upload bytes, retain only source-safe pointers.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use crate::remote::DocumentClient;

const GITIGNORE: &str = "*\n!*/\n!*.yml\n!.gitignore\n";

#[derive(Deserialize)]
struct ProjectManifest {
    project: String,
    host: Option<String>,
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
    let manifest_path = root.join("navigator.yaml");
    let manifest: ProjectManifest = serde_yaml::from_str(
        &std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("read {}", manifest_path.display()))?,
    )
    .context("parse navigator.yaml")?;
    if manifest.project.trim().is_empty() {
        return Err(anyhow!("navigator.yaml must name a Project"));
    }

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

fn slash_path(path: &Path) -> Result<String> {
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

fn write_pointer_atomically(path: &Path, yaml: &str) -> Result<()> {
    let temp = PathBuf::from(format!("{}.tmp-{}", path.display(), uuid::Uuid::now_v7()));
    std::fs::write(&temp, yaml).with_context(|| format!("write {}", temp.display()))?;
    std::fs::rename(&temp, path).with_context(|| format!("publish {}", path.display()))
}

fn read_pointer(path: &Path) -> Result<Option<store::document_pointers::DocumentPointer>> {
    match std::fs::read_to_string(path) {
        Ok(raw) => store::document_pointers::DocumentPointer::from_yaml(&raw)
            .with_context(|| format!("validate {}", path.display()))
            .map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}
