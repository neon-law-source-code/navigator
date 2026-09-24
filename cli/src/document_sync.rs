//! Project document staging: upload bytes, retain only source-safe pointers.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[cfg(windows)]
use std::os::windows::fs::MetadataExt;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::remote::DocumentClient;

/// The exact `documents/.gitignore` every Project repository must carry.
///
/// Deny everything, then re-admit subdirectories, pointer files, and this
/// file. Dropping `*` leaves two negations of nothing, and the directory
/// silently ignores nothing at all. `Y014` and `scaffold` share these bytes
/// with `site sync` / `site pull` so a new repository cannot drift from the
/// guard those commands write.
///
/// Only the written spelling is admitted. [`POINTER_READ_EXTENSIONS`] keeps
/// `.yml` *readable* so a pointer committed before the LAW-25 rename still
/// resolves, and this file does not disturb one already tracked — it only
/// stops a fresh `.yml` file from being swept into Git by an unqualified
/// `git add`.
pub(crate) const DOCUMENTS_GITIGNORE: &str = "*\n!*/\n!*.yaml\n!.gitignore\n";

/// The extension Navigator writes a document pointer with.
///
/// `.yaml`, matching every other YAML file Navigator owns in a Project
/// repository — `navigator.yaml`, `seeds/*.yaml`, `.sops.yaml`. Pointers were
/// the one exception (LAW-25), and the extension is in the CLI's own help
/// text, so it taught itself to every next repository.
///
/// `.github/workflows/*.yml` is GitHub's own convention and is not ours to
/// change.
pub(crate) const POINTER_EXTENSION: &str = "yaml";

/// Every extension a document pointer may be *read* at.
///
/// Writing one spelling while reading both is what lets a fleet-wide rename
/// happen after the release rather than atomically with it. The retired
/// `.yml` stays readable until no repository carries one.
///
/// This must be the only place the set is spelled. The walkers match on
/// extension, so a repository that renames ahead of the CLI reports
/// `0 pointer(s)` and passes — indistinguishable from having no documents at
/// all. That silent-pass failure is why the contract is central and not
/// per-command.
pub(crate) const POINTER_READ_EXTENSIONS: &[&str] = &[POINTER_EXTENSION, "yml"];

/// Whether `path` names a committed document pointer, in either spelling.
///
/// An [`AUTHORITY_SIDECAR_EXTENSION`] path is excluded even though its own
/// last extension is also `yaml`: a sidecar is pre-upload input a lawyer
/// wrote by hand, not a pointer `sync` committed, and the two must never be
/// confused — `discover` sweeps the wrong one into the wrong list otherwise.
pub(crate) fn is_pointer_path(path: &Path) -> bool {
    if is_authority_sidecar_path(path) {
        return false;
    }
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| POINTER_READ_EXTENSIONS.contains(&extension))
}

/// The reserved sidecar extension for a staged `documents/cases/**` or
/// `documents/rules/**` capture: `<capture-filename>.authority.yaml`,
/// alongside the capture itself. Extends the same `<source>.<extension>`
/// naming [`is_pointer_path`] already reads pointers under, rather than
/// inventing a second scheme — the trailing `.yaml` is why [`is_pointer_path`]
/// must exclude it by name.
pub(crate) const AUTHORITY_SIDECAR_EXTENSION: &str = "authority.yaml";

/// Whether `path` names an Authority capture's sidecar.
pub(crate) fn is_authority_sidecar_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(&format!(".{AUTHORITY_SIDECAR_EXTENSION}")))
}

/// The sidecar path for a staged Authority capture at `path`, following the
/// same `<source>.<extension>` convention [`pointer_path_for_source`] writes
/// a pointer at.
fn authority_sidecar_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.{AUTHORITY_SIDECAR_EXTENSION}", path.display()))
}

/// The Authority folder `relative` (a staged path below `documents/`) is
/// captured under, or `None` when it names an ordinary Project document.
///
/// `documents/cases/**` and `documents/rules/**` are Authorities — global
/// reference data with no `project_id` (see the glossary's Authority entry)
/// — routed through `site authorities create` rather than filed as a Project
/// document. The two folders split [`rules::citation::AuthorityClass`]:
/// `cases/` holds case law, `rules/` holds everything else (statutes,
/// regulations, administrative proceedings, and secondary sources) —
/// [`authority_class_belongs_in_folder`] enforces the split against the
/// sidecar's own declared `class`.
fn authority_capture_folder(relative: &Path) -> Option<&'static str> {
    match relative
        .components()
        .next()
        .and_then(|part| part.as_os_str().to_str())
    {
        Some("cases") => Some("cases"),
        Some("rules") => Some("rules"),
        _ => None,
    }
}

/// Whether `class` (an [`AuthorityClass`](rules::citation::AuthorityClass)
/// wire value) belongs under `folder` — `"cases"` for `case_law` alone,
/// `"rules"` for every other class. A mismatch (a statute staged under
/// `cases/`, or case law staged under `rules/`) is refused by
/// [`read_authority_sidecar`] before either the network or
/// `documents/.gitignore` is touched, the same preflight discipline
/// [`validate_invoice_filename`] uses for `documents/invoices/`.
fn authority_class_belongs_in_folder(class: &str, folder: &str) -> bool {
    match folder {
        "cases" => class == "case_law",
        "rules" => class != "case_law",
        _ => false,
    }
}

/// Whether `relative` is a `documents/invoices/**` staged file.
fn is_invoice_capture(relative: &Path) -> bool {
    relative
        .components()
        .next()
        .and_then(|part| part.as_os_str().to_str())
        == Some("invoices")
}

/// The synthetic invoice-number filename shape this workspace's fixtures use:
/// `INV-` followed by one or more ASCII digits, then any extension (the
/// issue's own `INV-n.pdf` example). Real billing-system invoice numbering
/// is firm-confidential and not modeled here — this is a simple, documented
/// placeholder a synthetic fixture can satisfy, not an attempt to reproduce
/// a real numbering scheme.
fn is_invoice_filename(filename: &str) -> bool {
    let Some((stem, _extension)) = filename.rsplit_once('.') else {
        return false;
    };
    stem.strip_prefix("INV-").is_some_and(|digits| {
        !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
    })
}

/// Refuse a `documents/invoices/**` file whose name does not match
/// [`is_invoice_filename`], per the issue's optional (but in-scope) folder
/// rule.
fn validate_invoice_filename(path: &Path) -> Result<()> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("{} has no filename", path.display()))?;
    if is_invoice_filename(filename) {
        Ok(())
    } else {
        Err(anyhow!(
            "{} does not match the invoice filename pattern `INV-<digits>.<ext>` (e.g. INV-1.pdf)",
            path.display()
        ))
    }
}

/// `slug` without its pointer extension, in either spelling, or `None` when
/// it names no pointer at all.
pub(crate) fn strip_pointer_extension(slug: &str) -> Option<&str> {
    POINTER_READ_EXTENSIONS
        .iter()
        .find_map(|extension| slug.strip_suffix(&format!(".{extension}")))
}

/// Read and validate `<root>/navigator.yaml` — the one manifest every
/// document command (`sync`, and the read verbs under `navigator site document`)
/// resolves its Project and login host from. Returns `(project, host)`.
///
/// Routes through [`crate::projects::manifest::parse`], the one reader that
/// accepts both the current `project: {host, name}` shape and the deprecated
/// flat `project:`/`host:` shape, so a v2 manifest never fails with a
/// `serde_yaml` type-mismatch instead of this command's own error.
pub(crate) fn read_manifest(root: &Path) -> Result<(String, Option<String>)> {
    let manifest_path = root.join("navigator.yaml");
    let contents = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("read {}", manifest_path.display()))?;
    let manifest = crate::projects::manifest::parse(&contents).map_err(|error| anyhow!(error))?;
    let project = manifest
        .project
        .filter(|project| !project.trim().is_empty())
        .ok_or_else(|| anyhow!("navigator.yaml must name a Project"))?;
    Ok((project, manifest.host))
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

/// The `--dry-run` report: one line per staged binary naming the route it
/// would take (an Authority upload, or the `kind` it would infer), never
/// touching the network or writing `documents/.gitignore`.
fn report_dry_run(root: &Path, documents: &Path, binaries: &[PathBuf]) -> Result<()> {
    for path in binaries {
        let relative = path
            .strip_prefix(documents)
            .map_err(|_| anyhow!("{} is outside documents/", path.display()))?;
        let route = if authority_capture_folder(relative).is_some() {
            "an Authority upload".to_string()
        } else {
            format!("kind `{}`", inferred_kind(relative))
        };
        println!("would upload {} as {route}", display_relative(root, path));
    }
    println!("{} upload planned", binaries.len());
    Ok(())
}

/// Reconcile every already-committed pointer's desired `visibility` against
/// the live record. A committed edit is desired state, and replaying it is
/// safe: the server's ordinary API audit records every reconciliation.
async fn reconcile_pointer_visibility(
    client: &DocumentClient,
    root: &Path,
    pointers: Vec<PathBuf>,
) -> Result<()> {
    for path in pointers {
        ensure_document_path_is_safe(root, &path, false, true)?;
        let raw =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let pointer = store::document_pointers::DocumentPointer::from_yaml(&raw)
            .with_context(|| format!("validate {}", path.display()))?;
        client
            .set_visibility(pointer.current_version.asset_id, &pointer.visibility)
            .await?;
    }
    Ok(())
}

/// Resolve the pointer for one staged binary: the Authority route for
/// `documents/cases/**` and `documents/rules/**`, or the ordinary per-Project
/// document upload for everything else — `preflight_sync_paths` already
/// validated any invoice filename before either the network or
/// `documents/.gitignore` was touched, so the ordinary route never sees a
/// rejected one.
#[allow(clippy::too_many_arguments)]
async fn resolve_pointer_for_binary(
    client: &DocumentClient,
    host: Option<&str>,
    path: &Path,
    bytes: &[u8],
    relative: &Path,
    slug: &str,
    existing_pointer: Option<&store::document_pointers::DocumentPointer>,
) -> Result<store::document_pointers::DocumentPointer> {
    if let Some(folder) = authority_capture_folder(relative) {
        return sync_authority_capture(host, path, bytes, existing_pointer, folder).await;
    }
    let kind =
        existing_pointer.map_or_else(|| inferred_kind(relative), |pointer| pointer.kind.as_str());
    let desired_visibility = existing_pointer.map_or_else(
        || "internal".to_string(),
        |pointer| pointer.visibility.clone(),
    );
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| anyhow!("file path has no filename"))?;
    let uploaded_pointer = client
        .upload_bytes(
            filename,
            bytes,
            kind,
            Some(&desired_visibility),
            None,
            Some(content_type(path)),
            Some(slug),
            None,
        )
        .await?;
    validate_upload_receipt(&uploaded_pointer, bytes)?;
    Ok(uploaded_pointer)
}

/// Commit one resolved pointer and remove the staged bytes it replaces —
/// the Authority route's sidecar too — restoring the previous pointer if the
/// source changed underneath the upload.
#[allow(clippy::too_many_arguments)]
fn finalize_uploaded_document(
    root: &Path,
    path: &Path,
    pointer_path: &Path,
    relative: &Path,
    pointer: &store::document_pointers::DocumentPointer,
    bytes: &[u8],
    existing_pointer_raw: Option<&str>,
    has_existing_pointer: bool,
) -> Result<()> {
    ensure_source_is_unchanged(path, bytes)?;
    let pointer_yaml = pointer.to_yaml()?;
    ensure_document_path_is_safe(root, pointer_path, !has_existing_pointer, false)?;
    write_pointer_atomically(pointer_path, &pointer_yaml)?;
    if let Err(error) = ensure_source_is_unchanged(path, bytes) {
        restore_pointer_after_source_change(
            pointer_path,
            existing_pointer_raw,
            has_existing_pointer,
        )
        .map_err(|restore| anyhow!("{error:#}; restore pointer: {restore:#}"))?;
        return Err(error);
    }
    std::fs::remove_file(path).with_context(|| format!("remove staged {}", path.display()))?;
    if authority_capture_folder(relative).is_some() {
        let sidecar_path = authority_sidecar_path(path);
        match std::fs::remove_file(&sidecar_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("remove {}", sidecar_path.display()))
            }
        }
    }
    Ok(())
}

async fn sync(root: &Path, dry_run: bool) -> Result<()> {
    let (project, host) = read_manifest(root)?;
    let documents = root.join("documents");
    let (binaries, pointers) = discover(&documents)?;
    let pointer_paths = preflight_sync_paths(root, &documents, &binaries, &pointers)?;
    if dry_run {
        return report_dry_run(root, &documents, &binaries);
    }

    std::fs::create_dir_all(&documents)
        .with_context(|| format!("create {}", documents.display()))?;
    ensure_documents_tree_is_safe(&documents)?;
    let ignore = documents.join(".gitignore");
    ensure_document_path_is_safe(root, &ignore, true, false)?;
    if !ignore.exists() {
        std::fs::write(&ignore, DOCUMENTS_GITIGNORE)
            .with_context(|| format!("write {}", ignore.display()))?;
    }

    let client = DocumentClient::connect(host.as_deref(), &project).await?;
    reconcile_pointer_visibility(&client, root, pointers).await?;

    let mut uploaded = 0usize;
    for (path, pointer_path, has_existing_pointer) in pointer_paths {
        ensure_document_path_is_safe(root, &path, false, true)?;
        let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let relative = path
            .strip_prefix(&documents)
            .map_err(|_| anyhow!("{} is outside documents/", path.display()))?;
        let slug = slash_path(relative)?;
        let existing_pointer_raw = if has_existing_pointer {
            Some(
                std::fs::read_to_string(&pointer_path)
                    .with_context(|| format!("read {}", pointer_path.display()))?,
            )
        } else {
            None
        };
        let existing_pointer = existing_pointer_raw
            .as_deref()
            .map(store::document_pointers::DocumentPointer::from_yaml)
            .transpose()
            .with_context(|| format!("validate {}", pointer_path.display()))?;

        let pointer = resolve_pointer_for_binary(
            &client,
            host.as_deref(),
            &path,
            &bytes,
            relative,
            &slug,
            existing_pointer.as_ref(),
        )
        .await?;
        finalize_uploaded_document(
            root,
            &path,
            &pointer_path,
            relative,
            &pointer,
            &bytes,
            existing_pointer_raw.as_deref(),
            has_existing_pointer,
        )?;
        uploaded += 1;
    }
    println!("{uploaded} uploaded");
    Ok(())
}

/// The Authority metadata a staged `documents/cases/**` or
/// `documents/rules/**` capture must supply via its sidecar
/// (`<capture>.authority.yaml`) — exactly the fields `site authorities
/// create` needs and cannot reliably scrape from arbitrary HTML `<meta>`
/// tags (settled in the issue's own triage comment).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthoritySidecar {
    class: String,
    citation: String,
    title: String,
    #[serde(default)]
    short_cite: Option<String>,
    #[serde(default)]
    publisher: Option<String>,
    #[serde(default)]
    issued_on: Option<String>,
    #[serde(default)]
    canonical_url: Option<String>,
    #[serde(default)]
    checked_on: Option<String>,
}

/// Read and parse one staged Authority capture's sidecar
/// (`<capture>.authority.yaml`), and enforce that its declared `class`
/// belongs under `folder` ([`authority_class_belongs_in_folder`]) — shared by
/// [`preflight_sync_paths`], which calls this to fail fast on a missing,
/// malformed, or misfiled sidecar before either the network or
/// `documents/.gitignore` is touched, and by [`sync_authority_capture`],
/// which reads the same fields it validated.
fn read_authority_sidecar(path: &Path, folder: &str) -> Result<AuthoritySidecar> {
    let sidecar_path = authority_sidecar_path(path);
    let sidecar_raw = std::fs::read_to_string(&sidecar_path).with_context(|| {
        format!(
            "read {} (a documents/{folder}/ capture needs a sidecar carrying citation/class/title)",
            sidecar_path.display()
        )
    })?;
    let sidecar: AuthoritySidecar = serde_yaml::from_str(&sidecar_raw)
        .with_context(|| format!("parse {}", sidecar_path.display()))?;
    if !authority_class_belongs_in_folder(&sidecar.class, folder) {
        let other = if folder == "cases" { "rules" } else { "cases" };
        return Err(anyhow!(
            "{} declares class `{}`, which belongs under documents/{other}/, not documents/{folder}/",
            sidecar_path.display(),
            sidecar.class
        ));
    }
    Ok(sidecar)
}

/// Route one staged `documents/cases/**` or `documents/rules/**` capture
/// through `site authorities create` instead of the ordinary per-Project
/// document upload — Authorities are global reference data with no
/// `project_id` (see the glossary's Authority entry), so filing one as a
/// Project document is the misclassification this routing exists to avoid.
///
/// Reads the capture's sidecar, archives the bytes through the same
/// authenticated door `navigator site authorities create` uses, and returns
/// a [`store::document_pointers::DocumentPointer`] carrying the resulting
/// `authority_id` alongside `sha256`/`canonical_url`/`checked_on`/`created_at`
/// — reusing the existing pointer struct rather than a parallel shape.
/// `kind` stays `exhibit`: the capture is still evidence or a rule filed on
/// the matter, only archived through the Authority door rather than the
/// document one.
async fn sync_authority_capture(
    host: Option<&str>,
    path: &Path,
    bytes: &[u8],
    existing_pointer: Option<&store::document_pointers::DocumentPointer>,
    folder: &str,
) -> Result<store::document_pointers::DocumentPointer> {
    let sidecar = read_authority_sidecar(path, folder)?;
    let authority = crate::authorities::create_authority(
        host,
        &crate::authorities::NewAuthorityArgs {
            class: &sidecar.class,
            citation: &sidecar.citation,
            title: &sidecar.title,
            short_cite: sidecar.short_cite.as_deref(),
            publisher: sidecar.publisher.as_deref(),
            issued_on: sidecar.issued_on.as_deref(),
            canonical_url: sidecar.canonical_url.as_deref(),
            checked_on: sidecar.checked_on.as_deref(),
            file: path,
            content_type: Some(content_type(path)),
        },
    )
    .await
    .with_context(|| format!("archive {} as an Authority", path.display()))?;
    let asset_id = authority
        .archived_asset_id
        .ok_or_else(|| anyhow!("authority create archived no asset for {}", path.display()))?;
    let version = existing_pointer.map_or(1, |pointer| pointer.current_version.version + 1);
    let previous_version = existing_pointer.map(|pointer| pointer.current_version.asset_id);
    let pointer = store::document_pointers::DocumentPointer {
        kind: "exhibit".to_string(),
        visibility: "internal".to_string(),
        current_version: store::document_pointers::PointerVersion {
            version,
            asset_id,
            created_at: chrono::Utc::now().to_rfc3339(),
            sha256: store::documents::sha256_hex(bytes),
            size_bytes: i64::try_from(bytes.len())
                .context("authority capture byte count does not fit in pointer")?,
            canonical_url: sidecar.canonical_url,
            checked_on: sidecar.checked_on,
        },
        previous_version,
        authority_id: Some(authority.id),
    };
    pointer
        .validate()
        .map_err(|error| anyhow!("invalid authority pointer: {error}"))?;
    Ok(pointer)
}

/// Synchronize the current Project repository's staged documents in the other
/// direction: download each committed pointer's own revision into the local
/// staging path it names. Hydrate-only — a live document the checkout carries
/// no pointer for is not imported; `project sync` owns complete discovery.
pub(crate) async fn run_pull(root: &Path, dry_run: bool) -> ExitCode {
    match pull(root, dry_run).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("navigator: {error:#}");
            ExitCode::from(2)
        }
    }
}

/// Reconcile the complete caller-visible live document inventory into the
/// current Project checkout. Unlike `site pull`, this discovers documents
/// whose pointer has never existed locally.
pub(crate) async fn run_project_sync(root: &Path, dry_run: bool) -> ExitCode {
    match project_sync(root, dry_run).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("navigator: {error:#}");
            ExitCode::from(2)
        }
    }
}

#[derive(Debug)]
struct ProjectSyncPlan {
    label: String,
    pointer: store::document_pointers::DocumentPointer,
    pointer_target: PathBuf,
    document_target: PathBuf,
    pointer_changed: bool,
    document_changed: bool,
}

fn inventory_filename(document: &crate::remote::LiveDocumentSummary) -> Result<String> {
    let filename = document
        .filename
        .as_deref()
        .filter(|name| {
            let path = Path::new(name);
            path.components().count() == 1 && path.file_name().is_some()
        })
        .ok_or_else(|| anyhow!("document {} has no safe filename", document.id))?;
    Ok(filename.to_string())
}

fn inventory_relative_path(document: &crate::remote::LiveDocumentSummary) -> Result<PathBuf> {
    if let Some(slug) = document.slug.as_deref() {
        let relative = PathBuf::from(slug);
        if relative.as_os_str().is_empty()
            || relative.is_absolute()
            || relative
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
            || relative.file_name().is_none()
        {
            return Err(anyhow!("document {} has unsafe slug `{slug}`", document.id));
        }
        return Ok(relative);
    }
    Ok(PathBuf::from("unclassified")
        .join(document.id.to_string())
        .join(inventory_filename(document)?))
}

fn pointer_target_for_document(root: &Path, relative: &Path) -> Result<PathBuf> {
    let source = root.join("documents").join(relative);
    let mut retired = source.as_os_str().to_os_string();
    retired.push(".yml");
    let retired = PathBuf::from(retired);
    if retired.exists() {
        return Err(anyhow!(
            "{} uses a retired pointer suffix; rename it to {}.yaml before syncing",
            retired.display(),
            source.display()
        ));
    }
    let mut target = source.into_os_string();
    target.push(format!(".{POINTER_EXTENSION}"));
    Ok(PathBuf::from(target))
}

fn raw_document_is_git_ignored(root: &Path, target: &Path) -> Result<()> {
    let relative = target
        .strip_prefix(root)
        .map_err(|_| anyhow!("{} is outside {}", target.display(), root.display()))?;
    let status = std::process::Command::new("git")
        .args(["check-ignore", "--quiet", "--"])
        .arg(relative)
        .current_dir(root)
        .status()
        .with_context(|| format!("verify Git ignores {}", relative.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "refusing to download {} because Git does not ignore it",
            relative.display()
        ))
    }
}

fn current_pointer_matches(
    path: &Path,
    pointer: &store::document_pointers::DocumentPointer,
) -> Result<bool> {
    match std::fs::read_to_string(path) {
        Ok(raw) => Ok(store::document_pointers::DocumentPointer::from_yaml(&raw)? == *pointer),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

fn project_sync_plans(
    root: &Path,
    documents: Vec<crate::remote::LiveDocumentSummary>,
) -> Result<Vec<ProjectSyncPlan>> {
    let mut grouped: BTreeMap<String, Vec<crate::remote::LiveDocumentSummary>> = BTreeMap::new();
    for document in documents {
        let key = document
            .slug
            .clone()
            .unwrap_or_else(|| format!("@{}", document.id));
        grouped.entry(key).or_default().push(document);
    }
    let mut plans = Vec::with_capacity(grouped.len());
    for (_, revisions) in grouped {
        let current = revisions
            .first()
            .ok_or_else(|| anyhow!("empty document revision group"))?;
        let relative = inventory_relative_path(current)?;
        let document_target = root.join("documents").join(&relative);
        let pointer_target = pointer_target_for_document(root, &relative)?;
        let kind = current
            .kind
            .clone()
            .unwrap_or_else(|| "unclassified".to_string());
        let pointer = store::document_pointers::DocumentPointer {
            kind,
            visibility: current.visibility.clone(),
            current_version: store::document_pointers::PointerVersion {
                version: revisions.len(),
                asset_id: current.id,
                created_at: current.inserted_at.clone(),
                sha256: current.sha256_hex.clone(),
                size_bytes: current.byte_size,
                canonical_url: None,
                checked_on: None,
            },
            previous_version: revisions.get(1).map(|revision| revision.id),
            authority_id: None,
        };
        pointer
            .validate()
            .map_err(|error| anyhow!("invalid live pointer for {}: {error}", relative.display()))?;
        let pointer_changed = !current_pointer_matches(&pointer_target, &pointer)?;
        let document_changed = !matches_digest(&document_target, &pointer.current_version.sha256)?;
        plans.push(ProjectSyncPlan {
            label: format!("documents/{}", slash_path(&relative)?),
            pointer,
            pointer_target,
            document_target,
            pointer_changed,
            document_changed,
        });
    }
    Ok(plans)
}

async fn stage_project_sync(
    plans: &[ProjectSyncPlan],
    client: &DocumentClient,
    staging: &tempfile::TempDir,
) -> Result<Vec<StagedPull>> {
    let downloads = staging.path().join("downloads");
    std::fs::create_dir_all(&downloads).context("create project sync downloads")?;
    let mut staged = Vec::new();
    let mut unreadable = Vec::new();
    for plan in plans {
        if plan.pointer_changed {
            let staged_path = downloads.join(staged.len().to_string());
            std::fs::write(&staged_path, plan.pointer.to_yaml()?)?;
            staged.push(StagedPull {
                target: plan.pointer_target.clone(),
                staged: staged_path,
            });
        }
        if plan.document_changed {
            match client
                .download_revision(plan.pointer.current_version.asset_id)
                .await
            {
                Ok(bytes)
                    if store::documents::sha256_hex(&bytes)
                        == plan.pointer.current_version.sha256 =>
                {
                    let staged_path = downloads.join(staged.len().to_string());
                    std::fs::write(&staged_path, bytes)?;
                    staged.push(StagedPull {
                        target: plan.document_target.clone(),
                        staged: staged_path,
                    });
                }
                Ok(bytes) => unreadable.push(format!(
                    "{}: could not read: sha256 mismatch (expected {}, received {})",
                    plan.label,
                    plan.pointer.current_version.sha256,
                    store::documents::sha256_hex(&bytes)
                )),
                Err(error) => unreadable.push(format!("{}: could not read: {error:#}", plan.label)),
            }
        }
    }
    if unreadable.is_empty() {
        return Ok(staged);
    }
    for failure in &unreadable {
        eprintln!("{failure}");
    }
    Err(anyhow!(
        "{} document(s) could not read; checkout unchanged",
        unreadable.len()
    ))
}

async fn project_sync(root: &Path, dry_run: bool) -> Result<()> {
    let root = root.canonicalize().context("resolve checkout")?;
    recover_interrupted_pull(&root)?;
    let (project, host) = read_manifest(&root)?;
    let client = DocumentClient::connect(host.as_deref(), &project).await?;
    let plans = project_sync_plans(&root, client.list_documents().await?)?;

    let mut added = 0usize;
    let mut updated = 0usize;
    let mut skipped = 0usize;
    for plan in &plans {
        if !plan.pointer_target.exists() {
            added += 1;
            if dry_run {
                println!("would add {}", plan.label);
            }
        } else if plan.pointer_changed || plan.document_changed {
            updated += 1;
            if dry_run {
                println!("would update {}", plan.label);
            }
        } else {
            skipped += 1;
            if dry_run {
                println!("skip {}", plan.label);
            }
        }
    }
    if dry_run {
        println!("{added} added, {updated} updated, {skipped} skipped");
        return Ok(());
    }

    let documents = root.join("documents");
    std::fs::create_dir_all(&documents)
        .with_context(|| format!("create {}", documents.display()))?;
    ensure_documents_tree_is_safe(&documents)?;
    let ignore = documents.join(".gitignore");
    match std::fs::read_to_string(&ignore) {
        Ok(contents) if contents == DOCUMENTS_GITIGNORE => {}
        Ok(_) => {
            return Err(anyhow!(
                "{} does not carry Navigator's document ignore rules",
                ignore.display()
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::write(&ignore, DOCUMENTS_GITIGNORE)
                .with_context(|| format!("write {}", ignore.display()))?;
        }
        Err(error) => return Err(error).with_context(|| format!("read {}", ignore.display())),
    }
    for plan in &plans {
        raw_document_is_git_ignored(&root, &plan.document_target)?;
    }

    let staging = tempfile::tempdir().context("create project sync staging area")?;
    let staged = stage_project_sync(&plans, &client, &staging).await?;
    if !staged.is_empty() {
        let mut transaction = begin_pull_transaction(&root, staging, staged)?;
        match publish_pull_transaction(&root, &mut transaction) {
            Ok(_) => {}
            Err(PublicationFailure::BeforeCommit(error)) => {
                rollback_pull_transaction(&root, &transaction.path, &transaction.state)?;
                return Err(error);
            }
            Err(PublicationFailure::AfterCommit(error)) => return Err(error),
        }
    }
    println!("{added} added, {updated} updated, {skipped} skipped");
    Ok(())
}

/// The local staging path a pointer's own bytes belong at — its committed
/// path with the trailing `.yml` stripped. Refuses to resolve outside
/// `<root>/documents/`, the same lexical guard `document get` uses in the
/// other direction (`document_read::refuse_destination_in_documents`).
fn pull_target(root: &Path, pointer_relative: &Path) -> Result<PathBuf> {
    let stem = pointer_relative
        .to_str()
        .and_then(strip_pointer_extension)
        .ok_or_else(|| {
            anyhow!(
                "{} does not name a document pointer (expected one of: {})",
                pointer_relative.display(),
                POINTER_READ_EXTENSIONS
                    .iter()
                    .map(|extension| format!(".{extension}"))
                    .collect::<Vec<_>>()
                    .join(", "),
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

/// A failure that left every document target as it found it.
///
/// It used to add "(documents/.gitignore may have been created)" to every
/// such failure, which was false for most of them: `pull` writes that
/// guard file only *after* the `--dry-run` early return, so the entire dry
/// run, the manifest read, and the pointer scan all reported a side effect
/// that path cannot perform (LAW-12). Use
/// [`document_targets_unchanged_after_guard`] for the failures that really
/// do come after the guard is written.
fn document_targets_unchanged(error: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("{error:#}; document targets are unchanged")
}

/// The same refusal, for a failure reached *after* the `documents/`
/// directory and its `.gitignore` guard have been written — the one case
/// where a new untracked file really may have appeared in the checkout.
fn document_targets_unchanged_after_guard(error: impl std::fmt::Display) -> anyhow::Error {
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
            Ok(metadata) if metadata_is_symlink_or_reparse(&metadata) => {
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
        Ok(metadata) if metadata_is_symlink_or_reparse(&metadata) => Err(anyhow!(
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
                    .map_err(document_targets_unchanged_after_guard)?;
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
        return Err(document_targets_unchanged_after_guard(anyhow!(
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
    let (project, host) = read_manifest(&root).map_err(document_targets_unchanged)?;
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
        std::fs::write(&ignore, DOCUMENTS_GITIGNORE)
            .with_context(|| format!("write {}", ignore.display()))
            .map_err(document_targets_unchanged)?;
    }

    let client = DocumentClient::connect(host.as_deref(), &project)
        .await
        .map_err(document_targets_unchanged_after_guard)?;
    let staging = tempfile::tempdir().map_err(|error| {
        document_targets_unchanged_after_guard(anyhow!(
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

fn metadata_is_symlink_or_reparse(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn ensure_documents_tree_is_safe(documents: &Path) -> Result<()> {
    match std::fs::symlink_metadata(documents) {
        Ok(metadata) if metadata_is_symlink_or_reparse(&metadata) => {
            return Err(anyhow!(
                "refusing to use symlinked documents root {}",
                documents.display()
            ));
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(anyhow!("{} is not a directory", documents.display()));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("inspect {}", documents.display()))
        }
    }
    for entry in walkdir::WalkDir::new(documents).follow_links(false) {
        let entry = entry.with_context(|| format!("walk {}", documents.display()))?;
        let metadata = std::fs::symlink_metadata(entry.path())
            .with_context(|| format!("inspect {}", entry.path().display()))?;
        if metadata_is_symlink_or_reparse(&metadata) {
            return Err(anyhow!(
                "refusing to use symlink in documents tree: {}",
                entry.path().display()
            ));
        }
        if !metadata.is_file() && !metadata.is_dir() {
            return Err(anyhow!(
                "refusing unsupported documents tree entry {}",
                entry.path().display()
            ));
        }
    }
    Ok(())
}

fn ensure_document_path_is_safe(
    root: &Path,
    path: &Path,
    leaf_may_be_missing: bool,
    leaf_must_be_file: bool,
) -> Result<()> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| anyhow!("{} is outside {}", path.display(), root.display()))?;
    let components = relative.components().collect::<Vec<_>>();
    let mut current = root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        current.push(component);
        let leaf = index + 1 == components.len();
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata_is_symlink_or_reparse(&metadata) => {
                return Err(anyhow!(
                    "refusing to use symlink path {}",
                    current.display()
                ));
            }
            Ok(metadata) if !leaf && !metadata.is_dir() => {
                return Err(anyhow!("{} is not a directory", current.display()));
            }
            Ok(metadata) if leaf && leaf_must_be_file && !metadata.is_file() => {
                return Err(anyhow!("{} is not a regular file", current.display()));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && leaf => {
                if leaf_may_be_missing {
                    return Ok(());
                }
                return Err(error).with_context(|| format!("inspect {}", current.display()));
            }
            Err(error) => {
                return Err(error).with_context(|| format!("inspect {}", current.display()));
            }
        }
    }
    Ok(())
}

fn pointer_path_for_source(path: &Path) -> Result<(PathBuf, bool)> {
    for extension in POINTER_READ_EXTENSIONS {
        let candidate = PathBuf::from(format!("{}.{extension}", path.display()));
        match std::fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata_is_symlink_or_reparse(&metadata) => {
                return Err(anyhow!(
                    "refusing to use symlink pointer path {}",
                    candidate.display()
                ));
            }
            Ok(metadata) if !metadata.is_file() => {
                return Err(anyhow!("{} is not a regular file", candidate.display()));
            }
            Ok(_) => return Ok((candidate, true)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("inspect {}", candidate.display()))
            }
        }
    }
    Ok((
        PathBuf::from(format!("{}.{POINTER_EXTENSION}", path.display())),
        false,
    ))
}

fn preflight_sync_paths(
    root: &Path,
    documents: &Path,
    binaries: &[PathBuf],
    pointers: &[PathBuf],
) -> Result<Vec<(PathBuf, PathBuf, bool)>> {
    ensure_documents_tree_is_safe(documents)?;
    for path in pointers {
        ensure_document_path_is_safe(root, path, false, true)?;
    }
    binaries
        .iter()
        .map(|path| {
            ensure_document_path_is_safe(root, path, false, true)?;
            let relative = path
                .strip_prefix(documents)
                .map_err(|_| anyhow!("{} is outside documents/", path.display()))?;
            if is_invoice_capture(relative) {
                validate_invoice_filename(path)?;
            }
            if let Some(folder) = authority_capture_folder(relative) {
                read_authority_sidecar(path, folder)?;
            }
            let (pointer_path, has_existing_pointer) = pointer_path_for_source(path)?;
            ensure_document_path_is_safe(root, &pointer_path, true, false)?;
            Ok((path.clone(), pointer_path, has_existing_pointer))
        })
        .collect()
}

fn validate_upload_receipt(
    pointer: &store::document_pointers::DocumentPointer,
    bytes: &[u8],
) -> Result<()> {
    pointer
        .validate()
        .map_err(|error| anyhow!("invalid upload pointer: {error}"))?;
    let expected_sha256 = store::documents::sha256_hex(bytes);
    if pointer.current_version.sha256 != expected_sha256 {
        return Err(anyhow!(
            "upload pointer sha256 mismatch: expected {expected_sha256}, received {}",
            pointer.current_version.sha256
        ));
    }
    let expected_size =
        i64::try_from(bytes.len()).context("uploaded byte count does not fit in pointer")?;
    if pointer.current_version.size_bytes != expected_size {
        return Err(anyhow!(
            "upload pointer byte count mismatch: expected {expected_size}, received {}",
            pointer.current_version.size_bytes
        ));
    }
    Ok(())
}

fn ensure_source_is_unchanged(path: &Path, expected: &[u8]) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("inspect source {}", path.display()))?;
    if metadata_is_symlink_or_reparse(&metadata) {
        return Err(anyhow!(
            "source changed during upload: {} is now a symlink",
            path.display()
        ));
    }
    if !metadata.is_file() {
        return Err(anyhow!(
            "source changed during upload: {} is not a regular file",
            path.display()
        ));
    }
    let actual = std::fs::read(path).with_context(|| format!("read source {}", path.display()))?;
    if actual != expected {
        return Err(anyhow!("source changed during upload: {}", path.display()));
    }
    Ok(())
}

fn restore_pointer_after_source_change(
    path: &Path,
    previous: Option<&str>,
    had_previous: bool,
) -> Result<()> {
    if had_previous {
        write_pointer_atomically(
            path,
            previous.ok_or_else(|| anyhow!("previous pointer bytes are missing"))?,
        )
    } else {
        std::fs::remove_file(path).with_context(|| format!("remove {}", path.display()))
    }
}

fn discover(documents: &Path) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    ensure_documents_tree_is_safe(documents)?;
    if !documents.is_dir() {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut binaries = Vec::new();
    let mut pointers = Vec::new();
    for entry in walkdir::WalkDir::new(documents).follow_links(false) {
        let entry = entry.with_context(|| format!("walk {}", documents.display()))?;
        let metadata = std::fs::symlink_metadata(entry.path())
            .with_context(|| format!("inspect {}", entry.path().display()))?;
        if metadata_is_symlink_or_reparse(&metadata) {
            return Err(anyhow!(
                "refusing to use symlink in documents tree: {}",
                entry.path().display()
            ));
        }
        if !entry.file_type().is_file() || entry.file_name() == ".gitignore" {
            continue;
        }
        // An Authority sidecar is metadata consulted by its known derived
        // path when the capture it describes is processed — never a pointer
        // to reconcile and never a binary to upload in its own right.
        if is_authority_sidecar_path(entry.path()) {
            continue;
        }
        if is_pointer_path(entry.path()) {
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
        Some("invoices") => "invoice",
        // LAW-60: a case-assessment memo or a scan transcript staged under
        // `documents/` had no folder that filed it as anything but
        // `unclassified`.
        Some("memos") => "memo",
        Some("transcripts") => "transcript",
        // `documents/cases/**` and `documents/rules/**` never reach this
        // default: `sync` routes them through `sync_authority_capture` before
        // `inferred_kind` is ever called for those folders, and the Authority
        // pointer it writes back hard-codes `kind: exhibit` rather than
        // asking this function.
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
        // An Authority capture under `documents/cases/` or `documents/rules/`
        // is an archived HTML page more often than any other kind this
        // function names.
        Some("html" | "htm") => "text/html",
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
        authority_capture_folder, authority_class_belongs_in_folder, content_type, inferred_kind,
        is_authority_sidecar_path, is_invoice_capture, is_invoice_filename, is_pointer_path,
        matches_digest, pull_target, pull_transaction_path, read_manifest,
        recover_interrupted_pull, strip_pointer_extension, write_pull_transaction_state,
        PullTransactionPhase, PullTransactionState, PullTransactionTarget,
        AUTHORITY_SIDECAR_EXTENSION, DOCUMENTS_GITIGNORE, POINTER_EXTENSION,
        POINTER_READ_EXTENSIONS,
    };
    use std::path::Path;

    #[test]
    fn is_pointer_path_excludes_the_authority_sidecar_extension() {
        assert_eq!(AUTHORITY_SIDECAR_EXTENSION, "authority.yaml");
        assert!(is_pointer_path(Path::new("documents/cases/roe.html.yaml")));
        assert!(!is_pointer_path(Path::new(
            "documents/cases/roe.html.authority.yaml"
        )));
        assert!(is_authority_sidecar_path(Path::new(
            "documents/cases/roe.html.authority.yaml"
        )));
        assert!(!is_authority_sidecar_path(Path::new(
            "documents/cases/roe.html.yaml"
        )));
    }

    #[test]
    fn authority_and_invoice_captures_are_recognized_by_their_top_folder() {
        assert_eq!(
            authority_capture_folder(Path::new("cases/roe.html")),
            Some("cases")
        );
        assert_eq!(
            authority_capture_folder(Path::new("rules/nrs-86-201.html")),
            Some("rules")
        );
        assert_eq!(
            authority_capture_folder(Path::new("pleadings/motion.pdf")),
            None
        );
        assert!(is_invoice_capture(Path::new("invoices/INV-1.pdf")));
        assert!(!is_invoice_capture(Path::new("exhibits/photo.png")));
    }

    #[test]
    fn authority_class_must_belong_in_its_folder() {
        assert!(authority_class_belongs_in_folder("case_law", "cases"));
        assert!(!authority_class_belongs_in_folder("statute", "cases"));
        assert!(authority_class_belongs_in_folder("statute", "rules"));
        assert!(authority_class_belongs_in_folder("regulation", "rules"));
        assert!(authority_class_belongs_in_folder("administrative", "rules"));
        assert!(authority_class_belongs_in_folder("secondary", "rules"));
        assert!(!authority_class_belongs_in_folder("case_law", "rules"));
    }

    #[test]
    fn inferred_kind_maps_every_folder_convention() {
        assert_eq!(inferred_kind(Path::new("pleadings/motion.pdf")), "filing");
        assert_eq!(inferred_kind(Path::new("exhibits/photo.png")), "exhibit");
        assert_eq!(inferred_kind(Path::new("agreements/nda.pdf")), "agreement");
        assert_eq!(inferred_kind(Path::new("invoices/INV-1.pdf")), "invoice");
        // LAW-60: a case-assessment memo or a scan transcript staged under
        // `documents/memos/` or `documents/transcripts/` now files as its
        // own kind rather than falling into `unclassified`.
        assert_eq!(inferred_kind(Path::new("memos/case-assessment.md")), "memo");
        assert_eq!(
            inferred_kind(Path::new("transcripts/deposition.md")),
            "transcript"
        );
        assert_eq!(inferred_kind(Path::new("misc/note.txt")), "unclassified");
    }

    #[test]
    fn invoice_filenames_must_match_the_synthetic_pattern() {
        assert!(is_invoice_filename("INV-1.pdf"));
        assert!(is_invoice_filename("INV-042.pdf"));
        assert!(!is_invoice_filename("invoice-1.pdf"));
        assert!(!is_invoice_filename("INV-.pdf"));
        assert!(!is_invoice_filename("INV-1a.pdf"));
        assert!(!is_invoice_filename("INV-1"));
    }

    #[test]
    fn html_authority_captures_get_a_real_content_type() {
        assert_eq!(content_type(Path::new("roe.html")), "text/html");
        assert_eq!(content_type(Path::new("roe.htm")), "text/html");
    }

    #[test]
    fn read_manifest_yields_project_and_host_from_a_v2_manifest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("navigator.yaml"),
            "version: \"1.0.0\"\nproject:\n  host: staging.neonlaw.com\n  name: acme\n",
        )
        .unwrap();

        let (project, host) = read_manifest(dir.path()).unwrap();

        assert_eq!(project, "acme");
        assert_eq!(host, Some("staging.neonlaw.com".to_string()));
    }

    #[test]
    fn read_manifest_still_yields_project_and_host_from_the_deprecated_flat_form() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("navigator.yaml"),
            "project: acme\nhost: staging.neonlaw.com\n",
        )
        .unwrap();

        let (project, host) = read_manifest(dir.path()).unwrap();

        assert_eq!(project, "acme");
        assert_eq!(host, Some("staging.neonlaw.com".to_string()));
    }

    #[test]
    fn read_manifest_reports_one_error_against_navigator_yaml_for_a_malformed_project_map() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("navigator.yaml"),
            "project:\n  name: acme\n",
        )
        .unwrap();

        let error = read_manifest(dir.path()).unwrap_err();

        let message = error.to_string();
        assert!(message.contains("navigator.yaml"), "{message}");
        assert_eq!(message.matches("navigator.yaml").count(), 1, "{message}");
    }

    #[test]
    fn pull_target_strips_either_pointer_extension_and_stays_below_documents() {
        let root = Path::new("/repo");
        // Both spellings resolve to the same staging path. `.yml` is the
        // retired one and stays readable so a fleet-wide rename does not have
        // to be atomic with the release (LAW-25).
        for pointer in [
            "documents/pleadings/motion.pdf.yaml",
            "documents/pleadings/motion.pdf.yml",
        ] {
            assert_eq!(
                pull_target(root, Path::new(pointer)).unwrap(),
                Path::new("/repo/documents/pleadings/motion.pdf"),
                "{pointer} must resolve to the document beside it"
            );
        }
    }

    #[test]
    fn pull_target_refuses_a_path_that_names_no_pointer_and_says_what_it_wanted() {
        let root = Path::new("/repo");
        let error = pull_target(root, Path::new("documents/pleadings/motion.pdf")).unwrap_err();
        let message = error.to_string();
        assert!(message.contains(".yaml"), "{message}");
        assert!(message.contains(".yml"), "{message}");
    }

    #[test]
    fn the_pointer_extension_contract_writes_yaml_and_reads_both() {
        assert_eq!(
            POINTER_EXTENSION, "yaml",
            "pointers are written at the extension every other Navigator YAML file uses"
        );
        assert_eq!(
            POINTER_READ_EXTENSIONS,
            &["yaml", "yml"],
            "the retired spelling stays readable through the rename"
        );
        assert!(is_pointer_path(Path::new("documents/a.pdf.yaml")));
        assert!(is_pointer_path(Path::new("documents/a.pdf.yml")));
        assert!(!is_pointer_path(Path::new("documents/a.pdf")));
        assert_eq!(strip_pointer_extension("a.pdf.yaml"), Some("a.pdf"));
        assert_eq!(strip_pointer_extension("a.pdf.yml"), Some("a.pdf"));
        assert_eq!(strip_pointer_extension("a.pdf"), None);
    }

    /// The gitignore admits only the written spelling. The retired `.yml`
    /// spelling stays *readable* via [`POINTER_READ_EXTENSIONS`] for pointers
    /// committed before the rename; this only keeps a fresh one from being
    /// swept into Git by an unqualified `git add`.
    #[test]
    fn the_documents_gitignore_admits_only_the_written_pointer_extension() {
        assert!(
            DOCUMENTS_GITIGNORE.contains(&format!("!*.{POINTER_EXTENSION}\n")),
            "documents/.gitignore must re-admit *.{POINTER_EXTENSION}, got {DOCUMENTS_GITIGNORE:?}"
        );
        assert!(
            !DOCUMENTS_GITIGNORE.contains("!*.yml\n"),
            "documents/.gitignore must not re-admit the retired *.yml spelling, got {DOCUMENTS_GITIGNORE:?}"
        );
        assert!(
            DOCUMENTS_GITIGNORE.starts_with("*\n"),
            "it must deny everything first, or the negations negate nothing"
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
