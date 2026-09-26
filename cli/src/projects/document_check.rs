//! The live half of `navigator project gate --check`.
//!
//! Committed pointers and the live asset rows are checked in both directions.
//! A drifted pointer, a live document with no pointer, and a missing
//! `documents/.gitignore` are rewritten in the checkout. The live site is never
//! written. A missing storage object is `navigator site document repair`. A
//! live row with no slug is `navigator site document slug`. Under `--ci` the
//! same fixes are reported and nothing is written.
//!
//! External-id checks (an invoice, a completed envelope) are the server's
//! `integrations` list on `GET /app/api/projects/{id}/documents/integrity`.
//! A pointer has no external-id field, so that list is empty.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use store::document_pointers::{DocumentPointer, PointerVersion};
use uuid::Uuid;

use crate::document_read::{discover_pointers, slug_from_pointer};
use crate::document_sync::{
    read_manifest, read_pointer, slash_path, write_pointer_atomically, DOCUMENTS_GITIGNORE,
};
use crate::remote::{DocumentClient, IntegrityAsset, RevisionSummary};

const REWRITE: &str = "rewrite the drifted pointer";
const WRITE_POINTER: &str = "write the missing pointer";
const WRITE_GITIGNORE: &str = "write the missing documents/.gitignore";

/// One live-record failure. `location` is the pointer path, or the asset id
/// when the row has no slug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckFailure {
    pub(crate) location: String,
    pub(crate) message: String,
}

/// One checkout edit the check made, or would make when it is not allowed to
/// write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckFix {
    pub(crate) path: String,
    pub(crate) action: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub(crate) errors: Vec<CheckFailure>,
    pub(crate) fixes: Vec<CheckFix>,
}

impl Outcome {
    /// Hard errors always fail. A fix fails only when the run is not allowed
    /// to write, which is `--ci`.
    #[cfg(test)]
    #[must_use]
    fn rejects(&self, ci: bool) -> bool {
        !self.errors.is_empty() || (ci && !self.fixes.is_empty())
    }
}

enum Local {
    Pointer {
        relative: String,
        absolute: PathBuf,
        pointer: Box<DocumentPointer>,
    },
    Unusable {
        relative: String,
    },
}

impl Local {
    fn relative(&self) -> &str {
        match self {
            Self::Pointer { relative, .. } | Self::Unusable { relative } => relative,
        }
    }
}

/// Read `navigator.yaml` and run the live document check.
///
/// `write` is false under `--ci`: the outcome lists the fixes and the
/// checkout is left alone. `deep` asks the server to re-hash each object.
pub(crate) async fn run(dir: &Path, ci: bool, deep: bool) -> Result<Outcome> {
    let (project, host) = read_manifest(dir)?;
    let host = host
        .filter(|host| !host.trim().is_empty())
        .ok_or_else(|| anyhow!("navigator.yaml names no host"))?;
    let client = if ci {
        let (base, token) = crate::remote::resolve_ci_document(&host).await?;
        DocumentClient::with_credential(base, token, &project).await?
    } else {
        DocumentClient::connect(Some(&host), &project).await?
    };
    check(dir, &client, !ci, deep).await
}

/// Compare `dir` with the live records `client` can see.
pub(crate) async fn check(
    dir: &Path,
    client: &DocumentClient,
    write: bool,
    deep: bool,
) -> Result<Outcome> {
    let integrity = client.integrity(deep).await?;
    let mut errors = Vec::new();
    let mut locals = index_pointers(dir, &mut errors)?;
    let live_slugs = live_slugs(&integrity.assets);

    for asset in &integrity.assets {
        for message in row_problems(asset, deep) {
            errors.push(CheckFailure {
                location: location_for(asset, &locals),
                message,
            });
        }
        if let Some(message) = transcript_source_problem(asset, &integrity.assets) {
            errors.push(CheckFailure {
                location: location_for(asset, &locals),
                message,
            });
        }
    }
    for finding in &integrity.integrations {
        errors.push(CheckFailure {
            location: finding.asset_id.to_string(),
            message: format!(
                "{}: {}: {}",
                finding.integration, finding.outcome, finding.detail
            ),
        });
    }

    let mut fixes = Vec::new();
    let mut claimed: BTreeSet<String> = BTreeSet::new();
    for slug in &live_slugs {
        reconcile_slug(
            dir,
            client,
            slug,
            &integrity.assets,
            &mut locals,
            write,
            deep,
            &mut errors,
            &mut fixes,
            &mut claimed,
        )
        .await?;
    }
    for (slug, local) in &locals {
        if claimed.contains(slug) {
            continue;
        }
        if !matches!(local, Local::Pointer { .. }) {
            continue;
        }
        let message = match client.list_revisions(slug).await {
            Ok(live) if live.revisions.iter().any(|revision| revision.operative) => {
                "the live chain is missing from the integrity report".to_string()
            }
            _ => "no live document for this pointer; run `navigator site sync` to upload it or remove the pointer".to_string(),
        };
        errors.push(CheckFailure {
            location: local.relative().to_string(),
            message,
        });
    }
    reconcile_gitignore(dir, write, &mut fixes)?;
    Ok(Outcome { errors, fixes })
}

fn transcript_source_problem(asset: &IntegrityAsset, assets: &[IntegrityAsset]) -> Option<String> {
    if asset.kind.as_deref() != Some("transcript") {
        return None;
    }
    let Some(source) = asset.derived_from.as_ref() else {
        return None;
    };
    let document_id = source
        .get("document_id")
        .and_then(serde_json::Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok());
    let version = source
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .and_then(|version| usize::try_from(version).ok());
    let sha256 = source.get("sha256").and_then(serde_json::Value::as_str);
    let (Some(document_id), Some(version), Some(sha256)) = (document_id, version, sha256) else {
        return Some(
            "transcript source linkage is incomplete; re-transcribe from the source pointer".into(),
        );
    };
    let Some(source_asset) = assets
        .iter()
        .find(|candidate| candidate.asset_id == document_id)
    else {
        return Some("transcript source revision is missing from the live matter".into());
    };
    if !source_asset.operative
        || source_asset.version != Some(version)
        || source_asset.sha256 != sha256
    {
        return Some(
            "transcript source revision is stale; re-transcribe from the current source pointer"
                .into(),
        );
    }
    None
}

fn live_slugs(assets: &[IntegrityAsset]) -> BTreeSet<String> {
    assets
        .iter()
        .filter_map(|asset| slug_of(asset).map(str::to_string))
        .collect()
}

fn slug_of(asset: &IntegrityAsset) -> Option<&str> {
    asset.slug.as_deref().filter(|slug| !slug.is_empty())
}

fn location_for(asset: &IntegrityAsset, locals: &BTreeMap<String, Local>) -> String {
    match slug_of(asset) {
        Some(slug) => located(locals, slug),
        None => asset.asset_id.to_string(),
    }
}

fn located(locals: &BTreeMap<String, Local>, slug: &str) -> String {
    locals.get(slug).map_or_else(
        || pointer_relative(slug),
        |local| local.relative().to_string(),
    )
}

fn pointer_relative(slug: &str) -> String {
    format!("documents/{slug}.yaml")
}

fn filename_extension(filename: &str) -> Option<&str> {
    Path::new(filename)
        .extension()
        .and_then(std::ffi::OsStr::to_str)
}

/// The local pointer key for a live `slug`. A document filed before #786 can
/// carry a `slug` with no extension of its own (LAW-62): the committed
/// pointer must still retain the document extension ahead of its `.yaml`
/// suffix (`Y003`), so this takes the extension from the operative
/// revision's stored `filename` and appends it, unless `slug` already ends
/// with it.
fn local_key(slug: &str, filename: &str) -> String {
    match filename_extension(filename) {
        Some(extension) if !slug.ends_with(&format!(".{extension}")) => {
            format!("{slug}.{extension}")
        }
        _ => slug.to_string(),
    }
}

/// Problems with one row. A missing slug and a bad object are both reported.
/// Size and digest are only meaningful once the object is there.
fn row_problems(asset: &IntegrityAsset, deep: bool) -> Vec<String> {
    let mut problems = Vec::new();
    if slug_of(asset).is_none() {
        problems.push("live row has no slug".to_string());
    }
    if let Some(problem) = storage_problem(asset, deep) {
        problems.push(problem);
    }
    problems
}

fn storage_problem(asset: &IntegrityAsset, deep: bool) -> Option<String> {
    if !asset.exists {
        return Some("storage object missing".to_string());
    }
    if asset.size_bytes != Some(asset.recorded_size) {
        let got = asset
            .size_bytes
            .map_or_else(|| "none".to_string(), |size| size.to_string());
        return Some(format!(
            "stored size {got} does not match the recorded size {}",
            asset.recorded_size
        ));
    }
    if deep && asset.sha256_matches != Some(true) {
        return Some("stored bytes do not hash to the recorded sha256".to_string());
    }
    None
}

fn index_pointers(dir: &Path, errors: &mut Vec<CheckFailure>) -> Result<BTreeMap<String, Local>> {
    let mut locals: BTreeMap<String, Local> = BTreeMap::new();
    for path in discover_pointers(dir)? {
        let slug = slug_from_pointer(dir, &path)?;
        let relative = slash_path(&path)?;
        let absolute = dir.join(&path);
        if let Some(existing) = locals.get_mut(&slug) {
            errors.push(CheckFailure {
                location: relative,
                message: format!("two committed pointers name `{slug}`"),
            });
            *existing = Local::Unusable {
                relative: existing.relative().to_string(),
            };
            continue;
        }
        let local = match read_pointer(&absolute) {
            Ok(Some(pointer)) => Local::Pointer {
                relative,
                absolute,
                pointer: Box::new(pointer),
            },
            Ok(None) => {
                errors.push(CheckFailure {
                    location: relative.clone(),
                    message: "pointer file disappeared".to_string(),
                });
                Local::Unusable { relative }
            }
            Err(error) => {
                errors.push(CheckFailure {
                    location: relative.clone(),
                    message: format!("{error:#}"),
                });
                Local::Unusable { relative }
            }
        };
        locals.insert(slug, local);
    }
    Ok(locals)
}

#[allow(clippy::too_many_arguments)]
async fn reconcile_slug(
    dir: &Path,
    client: &DocumentClient,
    slug: &str,
    assets: &[IntegrityAsset],
    locals: &mut BTreeMap<String, Local>,
    write: bool,
    deep: bool,
    errors: &mut Vec<CheckFailure>,
    fixes: &mut Vec<CheckFix>,
    claimed: &mut BTreeSet<String>,
) -> Result<()> {
    let live = match client.list_revisions(slug).await {
        Ok(live) => live,
        Err(error) => {
            errors.push(CheckFailure {
                location: located(locals, slug),
                message: format!("{error:#}"),
            });
            return Ok(());
        }
    };
    // A legacy extensionless slug (LAW-62) is keyed in `locals` under the
    // pointer's own filename-derived key, not the bare slug, because the
    // pointer must retain the document extension (`Y003`).
    let key = live
        .revisions
        .iter()
        .find(|revision| revision.operative)
        .map_or_else(
            || slug.to_string(),
            |revision| local_key(slug, &revision.filename),
        );
    let desired =
        match desired_pointer(local_pointer(locals.get(&key)), &live.kind, &live.revisions) {
            Ok(pointer) => pointer,
            Err(message) => {
                errors.push(CheckFailure {
                    location: located(locals, slug),
                    message,
                });
                return Ok(());
            }
        };
    let storage_ok = operative_storage_ok(assets, desired.current_version.asset_id, deep, errors);
    match locals.get(&key) {
        Some(Local::Pointer {
            relative,
            absolute,
            pointer,
        }) => {
            if pointer.as_ref() == &desired {
                claimed.insert(key);
                return Ok(());
            }
            let relative = relative.clone();
            let absolute = absolute.clone();
            apply_fix(write, &absolute, &relative, REWRITE, &desired, fixes)?;
            claimed.insert(key);
        }
        None if storage_ok => {
            let relative = pointer_relative(&key);
            let absolute = dir.join(&relative);
            apply_fix(write, &absolute, &relative, WRITE_POINTER, &desired, fixes)?;
            claimed.insert(key.clone());
            if write {
                if let Ok(Some(pointer)) = read_pointer(&absolute) {
                    locals.insert(
                        key,
                        Local::Pointer {
                            relative,
                            absolute,
                            pointer: Box::new(pointer),
                        },
                    );
                }
            }
        }
        Some(Local::Unusable { .. }) | None => {}
    }
    Ok(())
}

fn local_pointer(local: Option<&Local>) -> Option<&DocumentPointer> {
    if let Some(Local::Pointer { pointer, .. }) = local {
        Some(pointer.as_ref())
    } else {
        None
    }
}

/// Whether the operative object's storage is healthy. A missing integrity row
/// is its own error, and it blocks creating a pointer for a document the
/// checkout does not already name.
fn operative_storage_ok(
    assets: &[IntegrityAsset],
    asset_id: Uuid,
    deep: bool,
    errors: &mut Vec<CheckFailure>,
) -> bool {
    if let Some(asset) = assets.iter().find(|asset| asset.asset_id == asset_id) {
        return storage_problem(asset, deep).is_none();
    }
    errors.push(CheckFailure {
        location: asset_id.to_string(),
        message: "operative revision is absent from the integrity report".to_string(),
    });
    false
}

fn desired_pointer(
    local: Option<&DocumentPointer>,
    kind: &str,
    revisions: &[RevisionSummary],
) -> Result<DocumentPointer, String> {
    let operative = revisions
        .iter()
        .find(|revision| revision.operative)
        .ok_or_else(|| "the live chain has no operative revision".to_string())?;
    if !matches!(operative.visibility.as_str(), "internal" | "client") {
        return Err("the operative revision has no visibility".to_string());
    }
    let previous_version = if operative.version == 1 {
        None
    } else {
        Some(
            revisions
                .iter()
                .find(|revision| revision.version + 1 == operative.version)
                .map(|revision| revision.asset_id)
                .ok_or_else(|| {
                    format!(
                        "the live chain has no revision {} to name as previous_version",
                        operative.version - 1
                    )
                })?,
        )
    };
    // ENG-859: `docusign_envelope_id` and `xero_invoice_id` are, like
    // `authority_id`, facts this offline reconciliation never resolves for
    // itself — it only carries a local value forward when the operative
    // revision it names is unchanged. Live resolution against DocuSign/Xero
    // (confirming the id it names still exists) is `project gate --check`'s
    // separate live-check pass (ENG-863), not this function.
    let (authority_id, canonical_url, checked_on, docusign_envelope_id, xero_invoice_id) =
        match local {
            Some(pointer) if pointer.current_version.asset_id == operative.asset_id => (
                pointer.authority_id,
                pointer.current_version.canonical_url.clone(),
                pointer.current_version.checked_on.clone(),
                pointer.docusign_envelope_id,
                pointer.xero_invoice_id,
            ),
            _ => (None, None, None, None, None),
        };
    let pointer = DocumentPointer {
        kind: kind.to_string(),
        visibility: operative.visibility.clone(),
        current_version: PointerVersion {
            version: operative.version,
            asset_id: operative.asset_id,
            created_at: operative.created_at.clone(),
            sha256: operative.sha256.clone(),
            size_bytes: operative.size_bytes,
            canonical_url,
            checked_on,
        },
        previous_version,
        authority_id,
        docusign_envelope_id,
        xero_invoice_id,
    };
    pointer.validate().map_err(|error| error.to_string())?;
    Ok(pointer)
}

fn apply_fix(
    write: bool,
    absolute: &Path,
    relative: &str,
    action: &str,
    pointer: &DocumentPointer,
    fixes: &mut Vec<CheckFix>,
) -> Result<()> {
    if write {
        if let Some(parent) = absolute.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        let yaml = pointer.to_yaml()?;
        write_pointer_atomically(absolute, &yaml)?;
    }
    fixes.push(CheckFix {
        path: relative.to_string(),
        action: action.to_string(),
    });
    Ok(())
}

fn reconcile_gitignore(dir: &Path, write: bool, fixes: &mut Vec<CheckFix>) -> Result<()> {
    let documents = dir.join("documents");
    let ignore = documents.join(".gitignore");
    let needed = documents.is_dir() || fixes.iter().any(|fix| fix.action == WRITE_POINTER);
    if !needed || ignore.is_file() {
        return Ok(());
    }
    if write {
        std::fs::create_dir_all(&documents)
            .with_context(|| format!("create {}", documents.display()))?;
        std::fs::write(&ignore, DOCUMENTS_GITIGNORE)
            .with_context(|| format!("write {}", ignore.display()))?;
    }
    fixes.push(CheckFix {
        path: "documents/.gitignore".to_string(),
        action: WRITE_GITIGNORE.to_string(),
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::DocumentClient;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const OTHER_SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn transcript_gate_rejects_a_source_revision_that_is_no_longer_operative() {
        let source_id = Uuid::now_v7();
        let transcript = IntegrityAsset {
            asset_id: Uuid::now_v7(),
            slug: Some("transcripts/order.transcript.md".into()),
            kind: Some("transcript".into()),
            sha256: OTHER_SHA.into(),
            derived_from: Some(serde_json::json!({
                "document_id": source_id,
                "version": 1,
                "sha256": SHA,
            })),
            transcript_quality: Some("machine".into()),
            operative: true,
            version: Some(1),
            exists: true,
            size_bytes: Some(100),
            recorded_size: 100,
            sha256_matches: None,
        };
        let source = IntegrityAsset {
            asset_id: source_id,
            slug: Some("pleadings/order.pdf".into()),
            kind: Some("filing".into()),
            sha256: SHA.into(),
            derived_from: None,
            transcript_quality: None,
            operative: false,
            version: Some(1),
            exists: true,
            size_bytes: Some(200),
            recorded_size: 200,
            sha256_matches: None,
        };
        let message = transcript_source_problem(&transcript, &[transcript.clone(), source])
            .expect("stale transcript is a gate failure");
        assert!(message.contains("source revision is stale"));
    }

    fn pointer_yaml(asset_id: Uuid, sha: &str) -> String {
        format!(
            "kind: filing\nvisibility: internal\ncurrent_version:\n  version: 1\n  \
             asset_id: {asset_id}\n  created_at: \"2026-09-05T12:00:00Z\"\n  sha256: {sha}\n  \
             size_bytes: 18\n"
        )
    }

    fn write_tree(dir: &Path, relative: &str, bytes: &str) {
        let path = dir.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn gitignore(dir: &Path) {
        write_tree(dir, "documents/.gitignore", DOCUMENTS_GITIGNORE);
    }

    async fn server() -> (MockServer, Uuid, Uuid) {
        (MockServer::start().await, Uuid::now_v7(), Uuid::now_v7())
    }

    fn client(server: &MockServer, project_id: Uuid) -> DocumentClient {
        DocumentClient::for_check_tests(server.uri(), project_id)
    }

    async fn mount_integrity(server: &MockServer, project_id: Uuid, body: serde_json::Value) {
        Mock::given(method("GET"))
            .and(path(format!(
                "/app/api/projects/{project_id}/documents/integrity"
            )))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(server)
            .await;
    }

    async fn mount_revisions(
        server: &MockServer,
        project_id: Uuid,
        slug: &str,
        asset_id: Uuid,
        sha: &str,
    ) {
        mount_revisions_named(server, project_id, slug, asset_id, sha, "summons.pdf").await;
    }

    async fn mount_revisions_named(
        server: &MockServer,
        project_id: Uuid,
        slug: &str,
        asset_id: Uuid,
        sha: &str,
        filename: &str,
    ) {
        Mock::given(method("GET"))
            .and(path(format!(
                "/app/api/projects/{project_id}/documents/revisions"
            )))
            .and(query_param("slug", slug))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "kind": "filing",
                "revisions": [{
                    "version": 1,
                    "asset_id": asset_id,
                    "created_at": "2026-09-05T12:00:00Z",
                    "sha256": sha,
                    "size_bytes": 18,
                    "filename": filename,
                    "visibility": "internal",
                    "operative": true
                }]
            })))
            .expect(1)
            .mount(server)
            .await;
    }

    fn asset(
        asset_id: Uuid,
        slug: Option<&str>,
        exists: bool,
        sha256_matches: Option<bool>,
    ) -> serde_json::Value {
        let mut value = serde_json::json!({
            "asset_id": asset_id,
            "slug": slug,
            "exists": exists,
            "recorded_size": 18,
        });
        if exists {
            value["size_bytes"] = serde_json::json!(18);
        }
        if let Some(matches) = sha256_matches {
            value["sha256_matches"] = serde_json::json!(matches);
        }
        value
    }

    #[tokio::test]
    async fn a_missing_object_fails_when_the_pointer_matches_the_row() {
        let (server, project_id, asset_id) = server().await;
        let dir = tempfile::tempdir().unwrap();
        let relative = "documents/pleadings/summons.pdf.yaml";
        let yaml = pointer_yaml(asset_id, SHA);
        write_tree(dir.path(), relative, &yaml);
        gitignore(dir.path());
        mount_integrity(
            &server,
            project_id,
            serde_json::json!({ "assets": [asset(asset_id, Some("pleadings/summons.pdf"), false, None)] }),
        )
        .await;
        mount_revisions(&server, project_id, "pleadings/summons.pdf", asset_id, SHA).await;

        let outcome = check(dir.path(), &client(&server, project_id), true, false)
            .await
            .unwrap();

        assert!(outcome.errors.iter().any(|error| {
            error.location == relative && error.message.contains("storage object missing")
        }));
        assert!(outcome.fixes.is_empty());
        assert!(outcome.rejects(false));
        assert_eq!(
            std::fs::read_to_string(dir.path().join(relative)).unwrap(),
            yaml
        );
    }

    #[tokio::test]
    async fn a_live_document_with_no_pointer_fails_under_ci() {
        let (server, project_id, asset_id) = server().await;
        let dir = tempfile::tempdir().unwrap();
        mount_integrity(
            &server,
            project_id,
            serde_json::json!({ "assets": [asset(asset_id, Some("pleadings/summons.pdf"), true, None)] }),
        )
        .await;
        mount_revisions(&server, project_id, "pleadings/summons.pdf", asset_id, SHA).await;

        let outcome = check(dir.path(), &client(&server, project_id), false, false)
            .await
            .unwrap();

        assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
        assert!(outcome.fixes.iter().any(|fix| {
            fix.path == "documents/pleadings/summons.pdf.yaml" && fix.action == WRITE_POINTER
        }));
        assert!(outcome
            .fixes
            .iter()
            .any(|fix| fix.path == "documents/.gitignore"));
        assert!(outcome.rejects(true));
        assert!(!dir.path().join("documents").exists());
    }

    #[tokio::test]
    async fn a_local_check_writes_the_missing_pointer_and_gitignore() {
        let (server, project_id, asset_id) = server().await;
        let dir = tempfile::tempdir().unwrap();
        mount_integrity(
            &server,
            project_id,
            serde_json::json!({ "assets": [asset(asset_id, Some("pleadings/summons.pdf"), true, None)] }),
        )
        .await;
        mount_revisions(&server, project_id, "pleadings/summons.pdf", asset_id, SHA).await;

        let outcome = check(dir.path(), &client(&server, project_id), true, false)
            .await
            .unwrap();

        assert!(!outcome.rejects(false), "{outcome:?}");
        let pointer = DocumentPointer::from_yaml(
            &std::fs::read_to_string(dir.path().join("documents/pleadings/summons.pdf.yaml"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(pointer.current_version.asset_id, asset_id);
        assert_eq!(pointer.current_version.sha256, SHA);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("documents/.gitignore")).unwrap(),
            DOCUMENTS_GITIGNORE
        );
    }

    #[tokio::test]
    async fn auto_fix_rewrites_a_drifted_pointer() {
        let (server, project_id, asset_id) = server().await;
        let dir = tempfile::tempdir().unwrap();
        let relative = "documents/pleadings/summons.pdf.yaml";
        write_tree(dir.path(), relative, &pointer_yaml(asset_id, OTHER_SHA));
        gitignore(dir.path());
        mount_integrity(
            &server,
            project_id,
            serde_json::json!({ "assets": [asset(asset_id, Some("pleadings/summons.pdf"), true, None)] }),
        )
        .await;
        mount_revisions(&server, project_id, "pleadings/summons.pdf", asset_id, SHA).await;

        let outcome = check(dir.path(), &client(&server, project_id), true, false)
            .await
            .unwrap();

        assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
        assert_eq!(
            outcome.fixes,
            vec![CheckFix {
                path: relative.to_string(),
                action: REWRITE.to_string(),
            }]
        );
        let pointer = DocumentPointer::from_yaml(
            &std::fs::read_to_string(dir.path().join(relative)).unwrap(),
        )
        .unwrap();
        assert_eq!(pointer.current_version.sha256, SHA);
        assert!(!outcome.rejects(false));
    }

    #[tokio::test]
    async fn ci_reports_a_drifted_pointer_and_does_not_write_it() {
        let (server, project_id, asset_id) = server().await;
        let dir = tempfile::tempdir().unwrap();
        let relative = "documents/pleadings/summons.pdf.yaml";
        let original = pointer_yaml(asset_id, OTHER_SHA);
        write_tree(dir.path(), relative, &original);
        gitignore(dir.path());
        mount_integrity(
            &server,
            project_id,
            serde_json::json!({ "assets": [asset(asset_id, Some("pleadings/summons.pdf"), true, None)] }),
        )
        .await;
        mount_revisions(&server, project_id, "pleadings/summons.pdf", asset_id, SHA).await;

        let outcome = check(dir.path(), &client(&server, project_id), false, false)
            .await
            .unwrap();

        assert!(outcome.rejects(true));
        assert!(outcome.fixes.iter().any(|fix| fix.action == REWRITE));
        assert_eq!(
            std::fs::read_to_string(dir.path().join(relative)).unwrap(),
            original
        );
    }

    #[tokio::test]
    async fn a_deep_hash_mismatch_fails_without_rewriting_the_pointer() {
        let (server, project_id, asset_id) = server().await;
        let dir = tempfile::tempdir().unwrap();
        let relative = "documents/pleadings/summons.pdf.yaml";
        let yaml = pointer_yaml(asset_id, SHA);
        write_tree(dir.path(), relative, &yaml);
        gitignore(dir.path());
        mount_integrity(
            &server,
            project_id,
            serde_json::json!({
                "assets": [asset(asset_id, Some("pleadings/summons.pdf"), true, Some(false))]
            }),
        )
        .await;
        mount_revisions(&server, project_id, "pleadings/summons.pdf", asset_id, SHA).await;

        let outcome = check(dir.path(), &client(&server, project_id), true, true)
            .await
            .unwrap();

        assert!(outcome
            .errors
            .iter()
            .any(|error| { error.message.contains("do not hash to the recorded sha256") }));
        assert!(outcome.fixes.is_empty());
        assert_eq!(
            std::fs::read_to_string(dir.path().join(relative)).unwrap(),
            yaml
        );
    }

    #[tokio::test]
    async fn a_slugless_row_is_an_error_named_by_asset_id() {
        let (server, project_id, asset_id) = server().await;
        let dir = tempfile::tempdir().unwrap();
        mount_integrity(
            &server,
            project_id,
            serde_json::json!({ "assets": [asset(asset_id, None, true, None)] }),
        )
        .await;

        let outcome = check(dir.path(), &client(&server, project_id), true, false)
            .await
            .unwrap();

        assert!(outcome.errors.iter().any(|error| {
            error.location == asset_id.to_string() && error.message.contains("no slug")
        }));
        assert!(!dir.path().join("documents").exists());
    }

    /// LAW-62: a document filed before #786 can carry a live `slug` with no
    /// extension of its own. Writing the pointer at `documents/<slug>.yaml`
    /// fails `Y003` (no document extension before the `.yaml` suffix), so
    /// the check must take the extension from the stored filename and write
    /// `documents/<slug>.<ext>.yaml` instead.
    #[tokio::test]
    async fn a_legacy_extensionless_slug_writes_a_pointer_with_the_stored_extension() {
        let (server, project_id, asset_id) = server().await;
        let dir = tempfile::tempdir().unwrap();
        let slug = "summons-wendell-prine";
        mount_integrity(
            &server,
            project_id,
            serde_json::json!({ "assets": [asset(asset_id, Some(slug), true, None)] }),
        )
        .await;
        mount_revisions_named(
            &server,
            project_id,
            slug,
            asset_id,
            SHA,
            "summons-wendell-prine.pdf",
        )
        .await;

        let outcome = check(dir.path(), &client(&server, project_id), true, false)
            .await
            .unwrap();

        assert!(!outcome.rejects(false), "{outcome:?}");
        assert!(
            outcome.fixes.iter().any(|fix| {
                fix.path == "documents/summons-wendell-prine.pdf.yaml"
                    && fix.action == WRITE_POINTER
            }),
            "{:?}",
            outcome.fixes
        );
        assert!(dir
            .path()
            .join("documents/summons-wendell-prine.pdf.yaml")
            .exists());
        assert!(!dir
            .path()
            .join("documents/summons-wendell-prine.yaml")
            .exists());
    }

    /// The extension-carrying pointer this fix writes must not be mistaken
    /// for an orphan (no live document matches its filename-derived local
    /// key) or rewritten on every subsequent run.
    #[tokio::test]
    async fn a_legacy_extensionless_pointer_already_on_disk_is_not_reported_as_orphaned() {
        let (server, project_id, asset_id) = server().await;
        let dir = tempfile::tempdir().unwrap();
        let slug = "summons-wendell-prine";
        let relative = "documents/summons-wendell-prine.pdf.yaml";
        write_tree(dir.path(), relative, &pointer_yaml(asset_id, SHA));
        gitignore(dir.path());
        mount_integrity(
            &server,
            project_id,
            serde_json::json!({ "assets": [asset(asset_id, Some(slug), true, None)] }),
        )
        .await;
        mount_revisions_named(
            &server,
            project_id,
            slug,
            asset_id,
            SHA,
            "summons-wendell-prine.pdf",
        )
        .await;

        let outcome = check(dir.path(), &client(&server, project_id), true, false)
            .await
            .unwrap();

        assert!(!outcome.rejects(false), "{outcome:?}");
        assert!(outcome.fixes.is_empty(), "{:?}", outcome.fixes);
    }
}
