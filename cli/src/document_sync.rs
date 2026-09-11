//! Project document staging: upload bytes, retain only source-safe pointers.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

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

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
enum PullTransactionPhase {
    Staging,
    Prepared,
    Publishing,
    Committed,
}

#[derive(Debug, Deserialize, Serialize)]
struct PullTransactionState {
    phase: PullTransactionPhase,
    targets: Vec<PullTransactionTarget>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PullTransactionTarget {
    target: PathBuf,
    backup: PathBuf,
    existed: bool,
}

struct StagedPull {
    target: PathBuf,
    staged: PathBuf,
}

struct PullTransaction {
    path: PathBuf,
    state: PullTransactionState,
    staged: Vec<StagedPull>,
}

enum PublicationFailure {
    BeforeCommit(anyhow::Error),
    AfterCommit(anyhow::Error),
}

fn document_targets_unchanged(error: impl std::fmt::Display) -> anyhow::Error {
    anyhow!(
        "{error:#}; document targets are unchanged (documents/.gitignore may have been created)"
    )
}

fn pull_transaction_path(root: &Path) -> PathBuf {
    let root_key = store::documents::sha256_hex(root.to_string_lossy().as_bytes());
    std::env::temp_dir().join("navigator-pull").join(root_key)
}

fn pull_transaction_state_path(transaction: &Path) -> PathBuf {
    transaction.join("state.json")
}

fn write_pull_transaction_state(transaction: &Path, state: &PullTransactionState) -> Result<()> {
    let path = pull_transaction_state_path(transaction);
    let temporary = transaction.join(format!("state.tmp-{}", uuid::Uuid::now_v7()));
    let result = (|| {
        let bytes = serde_json::to_vec(state).context("serialize pull transaction state")?;
        let mut file = std::fs::File::create(&temporary)
            .with_context(|| format!("create {}", temporary.display()))?;
        file.write_all(&bytes)
            .with_context(|| format!("write {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("flush {}", temporary.display()))?;
        std::fs::rename(&temporary, &path)
            .with_context(|| format!("publish {}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn ensure_pull_target_parent_is_safe(root: &Path, target: &Path) -> Result<()> {
    let documents = crate::document_read::lexical(root, Path::new("documents"));
    if !target.starts_with(&documents) {
        return Err(anyhow!(
            "refusing to write {} outside {}",
            target.display(),
            documents.display()
        ));
    }
    let relative = target
        .strip_prefix(root)
        .map_err(|_| anyhow!("{} is outside {}", target.display(), root.display()))?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        if current == target {
            break;
        }
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(anyhow!(
                    "refusing to write through symlink {}",
                    current.display()
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(anyhow!("{} is not a directory", current.display()));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(error).with_context(|| format!("inspect {}", current.display()));
            }
        }
    }
    Ok(())
}

fn ensure_pull_target_is_safe(root: &Path, target: &Path) -> Result<()> {
    ensure_pull_target_parent_is_safe(root, target)?;
    match std::fs::symlink_metadata(target) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(anyhow!(
            "refusing to write through symlink {}",
            target.display()
        )),
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => Err(anyhow!("{} is not a regular file", target.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspect {}", target.display())),
    }
}

fn remove_pull_target(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            std::fs::remove_dir(path).with_context(|| format!("remove {}", path.display()))?;
        }
        Ok(_) => {
            std::fs::remove_file(path).with_context(|| format!("remove {}", path.display()))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).with_context(|| format!("inspect {}", path.display())),
    }
    Ok(())
}

fn rollback_pull_transaction(
    root: &Path,
    transaction: &Path,
    state: &PullTransactionState,
) -> Result<()> {
    for entry in state.targets.iter().rev() {
        let target = crate::document_read::lexical(root, &entry.target);
        ensure_pull_target_parent_is_safe(root, &target)?;
        remove_pull_target(&target)?;
        if entry.existed {
            let backup = transaction.join(&entry.backup);
            let Some(parent) = target.parent() else {
                return Err(anyhow!("{} has no parent directory", target.display()));
            };
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
            std::fs::copy(&backup, &target)
                .with_context(|| format!("restore {}", target.display()))?;
        }
    }
    std::fs::remove_dir_all(transaction)
        .with_context(|| format!("remove pull transaction {}", transaction.display()))?;
    Ok(())
}

fn recover_interrupted_pull(root: &Path) -> Result<()> {
    let transaction = pull_transaction_path(root);
    if !transaction.exists() {
        return Ok(());
    }
    if !transaction.is_dir() {
        return Err(anyhow!(
            "pull recovery path {} is not a directory",
            transaction.display()
        ));
    }
    let state_path = pull_transaction_state_path(&transaction);
    let state = match std::fs::read(&state_path) {
        Ok(bytes) => serde_json::from_slice::<PullTransactionState>(&bytes)
            .with_context(|| format!("read {}", state_path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::remove_dir_all(&transaction)
                .with_context(|| format!("remove {}", transaction.display()))?;
            return Ok(());
        }
        Err(error) => return Err(error).with_context(|| format!("read {}", state_path.display())),
    };
    match state.phase {
        PullTransactionPhase::Staging | PullTransactionPhase::Committed => {
            std::fs::remove_dir_all(&transaction)
                .with_context(|| format!("remove {}", transaction.display()))?;
        }
        PullTransactionPhase::Prepared | PullTransactionPhase::Publishing => {
            rollback_pull_transaction(root, &transaction, &state)?;
        }
    }
    Ok(())
}

fn begin_pull_transaction(
    root: &Path,
    staging: tempfile::TempDir,
    staged: Vec<StagedPull>,
) -> Result<PullTransaction> {
    let transaction = pull_transaction_path(root);
    let parent = transaction
        .parent()
        .ok_or_else(|| anyhow!("pull transaction has no parent directory"))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create pull transaction parent {}", parent.display()))?;
    let staging_path = staging.keep();
    if let Err(error) = std::fs::rename(&staging_path, &transaction) {
        let _ = std::fs::remove_dir_all(&staging_path);
        return Err(error)
            .with_context(|| format!("move pull staging area {}", transaction.display()));
    }

    let state = PullTransactionState {
        phase: PullTransactionPhase::Staging,
        targets: Vec::new(),
    };
    if let Err(error) = write_pull_transaction_state(&transaction, &state) {
        let _ = std::fs::remove_dir_all(&transaction);
        return Err(error);
    }

    let backups = transaction.join("backups");
    if let Err(error) = std::fs::create_dir_all(&backups) {
        let _ = std::fs::remove_dir_all(&transaction);
        return Err(error).with_context(|| format!("create {}", backups.display()));
    }

    let mut prepared = state;
    for (index, item) in staged.iter().enumerate() {
        ensure_pull_target_is_safe(root, &item.target)?;
        let target = item
            .target
            .strip_prefix(root)
            .map_err(|_| anyhow!("{} is outside {}", item.target.display(), root.display()))?
            .to_path_buf();
        let existed = match std::fs::symlink_metadata(&item.target) {
            Ok(metadata) if metadata.file_type().is_file() => true,
            Ok(_) => {
                return Err(anyhow!("{} is not a regular file", item.target.display()));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => {
                return Err(error).with_context(|| format!("inspect {}", item.target.display()));
            }
        };
        let backup = PathBuf::from(format!("backups/{index}"));
        if existed {
            std::fs::copy(&item.target, transaction.join(&backup))
                .with_context(|| format!("backup {}", item.target.display()))?;
        }
        prepared.targets.push(PullTransactionTarget {
            target,
            backup,
            existed,
        });
    }
    prepared.phase = PullTransactionPhase::Prepared;
    write_pull_transaction_state(&transaction, &prepared)?;

    let staged = staged
        .into_iter()
        .map(|item| {
            let file_name = item
                .staged
                .file_name()
                .ok_or_else(|| anyhow!("staged pull has no file name"))?;
            Ok(StagedPull {
                staged: transaction.join("downloads").join(file_name),
                target: item.target,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(PullTransaction {
        path: transaction,
        state: prepared,
        staged,
    })
}

fn publish_pull_transaction(
    root: &Path,
    transaction: &mut PullTransaction,
) -> std::result::Result<usize, PublicationFailure> {
    transaction.state.phase = PullTransactionPhase::Publishing;
    write_pull_transaction_state(&transaction.path, &transaction.state)
        .map_err(PublicationFailure::BeforeCommit)?;

    for item in &transaction.staged {
        ensure_pull_target_is_safe(root, &item.target).map_err(PublicationFailure::BeforeCommit)?;
        let Some(parent) = item.target.parent() else {
            return Err(PublicationFailure::BeforeCommit(anyhow!(
                "{} has no parent directory",
                item.target.display()
            )));
        };
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))
            .map_err(PublicationFailure::BeforeCommit)?;
        std::fs::copy(&item.staged, &item.target)
            .with_context(|| format!("publish {}", item.target.display()))
            .map_err(PublicationFailure::BeforeCommit)?;
    }

    transaction.state.phase = PullTransactionPhase::Committed;
    write_pull_transaction_state(&transaction.path, &transaction.state)
        .map_err(PublicationFailure::BeforeCommit)?;
    let pulled = transaction.staged.len();
    if let Err(error) = std::fs::remove_dir_all(&transaction.path) {
        return Err(PublicationFailure::AfterCommit(anyhow!(
            "remove completed pull transaction {}: {error}",
            transaction.path.display()
        )));
    }
    Ok(pulled)
}

fn pull_dry_run(root: &Path, pointers: &[PathBuf]) -> Result<()> {
    let mut planned = 0usize;
    for relative in pointers {
        let pointer_path = root.join(relative);
        let pointer = read_pointer(&pointer_path)
            .map_err(document_targets_unchanged)?
            .ok_or_else(|| anyhow!("{} vanished mid-scan", pointer_path.display()))
            .map_err(document_targets_unchanged)?;
        let target = pull_target(root, relative).map_err(document_targets_unchanged)?;
        ensure_pull_target_is_safe(root, &target).map_err(document_targets_unchanged)?;
        if !matches_digest(&target, &pointer.current_version.sha256)
            .map_err(document_targets_unchanged)?
        {
            println!("would pull {}", display_relative(root, &target));
            planned += 1;
        }
    }
    println!("{planned} pull(s) planned");
    Ok(())
}

async fn stage_pull_downloads(
    root: &Path,
    pointers: &[PathBuf],
    client: &DocumentClient,
    staging: &tempfile::TempDir,
) -> Result<(usize, Vec<StagedPull>)> {
    let downloads = staging.path().join("downloads");
    std::fs::create_dir_all(&downloads)
        .with_context(|| format!("create {}", downloads.display()))
        .map_err(document_targets_unchanged)?;
    let mut pulled = 0usize;
    let mut failures = Vec::new();
    let mut staged = Vec::new();
    for relative in pointers {
        let pointer_path = root.join(relative);
        let Some(pointer) = read_pointer(&pointer_path).map_err(document_targets_unchanged)? else {
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
        if let Err(error) = ensure_pull_target_is_safe(root, &target) {
            failures.push(format!("{}: {error}", pointer_path.display()));
            continue;
        }
        if matches_digest(&target, &pointer.current_version.sha256)
            .map_err(document_targets_unchanged)?
        {
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
                let staged_path = downloads.join(staged.len().to_string());
                std::fs::write(&staged_path, &bytes)
                    .with_context(|| format!("stage {}", target.display()))
                    .map_err(document_targets_unchanged)?;
                staged.push(StagedPull {
                    target,
                    staged: staged_path,
                });
                pulled += 1;
            }
            Err(error) => {
                failures.push(format!("{}: {error}", pointer_path.display()));
            }
        }
    }
    if !failures.is_empty() {
        for failure in &failures {
            eprintln!(
                "{failure} (document targets are unchanged; documents/.gitignore may have been created)"
            );
        }
        return Err(document_targets_unchanged(anyhow!(
            "{} of {} pointer(s) failed to pull",
            failures.len(),
            pointers.len()
        )));
    }
    Ok((pulled, staged))
}

async fn pull(root: &Path, dry_run: bool) -> Result<()> {
    let root = root
        .canonicalize()
        .map_err(|error| document_targets_unchanged(anyhow!("resolve checkout: {error}")))?;
    recover_interrupted_pull(&root).map_err(|error| {
        anyhow!(
            "recover interrupted pull: {error:#}; document targets may be in an interrupted publication state; retry pull to attempt recovery"
        )
    })?;
    let manifest = read_manifest(&root).map_err(document_targets_unchanged)?;
    let pointers =
        crate::document_read::discover_pointers(&root).map_err(document_targets_unchanged)?;
    if pointers.is_empty() {
        println!("no documents/ pointers to pull");
        return Ok(());
    }

    if dry_run {
        return pull_dry_run(&root, &pointers);
    }

    let documents = root.join("documents");
    std::fs::create_dir_all(&documents)
        .with_context(|| format!("create {}", documents.display()))
        .map_err(document_targets_unchanged)?;
    let ignore = documents.join(".gitignore");
    if !ignore.exists() {
        std::fs::write(&ignore, GITIGNORE)
            .with_context(|| format!("write {}", ignore.display()))
            .map_err(document_targets_unchanged)?;
    }

    let client = DocumentClient::connect(manifest.host.as_deref(), manifest.project.trim())
        .await
        .map_err(document_targets_unchanged)?;
    let staging = tempfile::tempdir().map_err(|error| {
        document_targets_unchanged(anyhow!(
            "create pull staging area outside checkout: {error}"
        ))
    })?;
    let (pulled, staged) = stage_pull_downloads(&root, &pointers, &client, &staging).await?;
    if staged.is_empty() {
        println!("{pulled} pulled");
        return Ok(());
    }

    let mut transaction =
        begin_pull_transaction(&root, staging, staged).map_err(document_targets_unchanged)?;
    match publish_pull_transaction(&root, &mut transaction) {
        Ok(_) => {
            println!("{pulled} pulled");
            Ok(())
        }
        Err(PublicationFailure::BeforeCommit(error)) => {
            match rollback_pull_transaction(&root, &transaction.path, &transaction.state) {
                Ok(()) => Err(document_targets_unchanged(error)),
                Err(recovery) => Err(anyhow!(
                    "{error:#}; document targets may be mixed because automatic rollback failed: {recovery:#}; retry pull to attempt recovery"
                )),
            }
        }
        Err(PublicationFailure::AfterCommit(error)) => Err(anyhow!(
            "{error:#}; document targets contain the complete after-publication state; retry pull to finish cleanup"
        )),
    }
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
    use super::{
        matches_digest, pull_target, pull_transaction_path, recover_interrupted_pull,
        write_pull_transaction_state, PullTransactionPhase, PullTransactionState,
        PullTransactionTarget,
    };
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

    #[test]
    fn recovery_restores_the_before_state_after_an_interrupted_publication() {
        let root = tempfile::tempdir().unwrap();
        let transaction = pull_transaction_path(root.path());
        let backups = transaction.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        let first_target = root.path().join("documents/pleadings/a.pdf");
        let second_target = root.path().join("documents/pleadings/b.pdf");
        std::fs::create_dir_all(first_target.parent().unwrap()).unwrap();
        std::fs::write(&first_target, b"before first").unwrap();
        std::fs::write(&second_target, b"before second").unwrap();
        std::fs::copy(&first_target, backups.join("0")).unwrap();
        std::fs::copy(&second_target, backups.join("1")).unwrap();
        let state = PullTransactionState {
            phase: PullTransactionPhase::Publishing,
            targets: vec![
                PullTransactionTarget {
                    target: Path::new("documents/pleadings/a.pdf").to_path_buf(),
                    backup: Path::new("backups/0").to_path_buf(),
                    existed: true,
                },
                PullTransactionTarget {
                    target: Path::new("documents/pleadings/b.pdf").to_path_buf(),
                    backup: Path::new("backups/1").to_path_buf(),
                    existed: true,
                },
            ],
        };
        write_pull_transaction_state(&transaction, &state).unwrap();
        std::fs::write(&first_target, b"after first").unwrap();

        recover_interrupted_pull(root.path()).unwrap();

        assert_eq!(std::fs::read(&first_target).unwrap(), b"before first");
        assert_eq!(std::fs::read(&second_target).unwrap(), b"before second");
        assert!(!transaction.exists());
    }
}
