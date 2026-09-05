//! Governed expunge — the admin-only primitive that lawfully removes a
//! document from a matter (design §9).
//!
//! This is the one operation that rewrites a matter repo's history. It
//! exists for a **privilege clawback**, a **sealing order**, or a
//! client's **lawful deletion** request — never as routine editing. It
//! ties the three pieces together in order:
//!
//! 1. Verify the authorizer is an **admin** (the gate is baked into the
//!    primitive, not left to the caller).
//! 2. Rewrite legacy repo history when that Project already has a repository
//!    containing the path ([`repos::RepoStore::expunge_path_code`]). Current
//!    document filing never writes raw bytes to Git.
//! 3. Delete the file's bytes from object storage — **every** key holding
//!    them (`blobs/<sha>`, `lfs/<oid>`, and any secondary notation key a
//!    dual-write left, e.g. `notations/<id>/document.pdf`) so no copy of the
//!    content survives in the data lake.
//! 4. Record the expunge itself — who, when, category — but **not** the
//!    content, so the redaction stays auditable
//!    ([`store::expunge_records`]).
//!
//! Rewriting history invalidates existing clones; that is an accepted,
//! documented consequence of a lawful expunge.

use std::sync::Arc;

use cloud::StorageService;
use store::expunge_records;
use uuid::Uuid;

/// What can go wrong during an expunge.
#[derive(Debug, thiserror::Error)]
pub enum ExpungeError {
    /// The authorizing person is not an `admin`.
    #[error("not authorized: only an admin may expunge a matter document")]
    NotAdmin,
    /// Unknown expunge category.
    #[error("unknown expunge category `{0}` (expected privilege | sealing | client_request)")]
    BadCategory(String),
    /// The repo-history rewrite failed.
    #[error("repo: {0}")]
    Repo(#[from] repos::RepoError),
    /// Deleting the object bytes failed.
    #[error("storage: {0}")]
    Storage(String),
    /// Reading or deleting a document asset failed.
    #[error("asset: {0}")]
    Asset(#[from] store::assets::AssetError),
    /// Writing the audit row failed.
    #[error("audit trail: {0}")]
    Record(#[from] store::expunge_records::ExpungeRecordError),
    /// Resolving the matter failed.
    #[error("database: {0}")]
    Project(#[from] store::projects::ProjectStoreError),
    /// The matter named by the request does not exist, so there is no repo
    /// to rewrite.
    #[error("no matter with id {0}")]
    ProjectNotFound(Uuid),
    /// Resolving the authorizing person failed.
    #[error(transparent)]
    Person(#[from] store::persons::PersonError),
    /// The blocking git task panicked.
    #[error("expunge task: {0}")]
    Join(String),
}

/// One governed-expunge request.
pub struct ExpungeRequest<'a> {
    /// The matter whose repo holds the document.
    pub project_id: Uuid,
    /// The repo path to remove from all history (e.g. `notice.pdf`).
    pub path: &'a str,
    /// One of the [`expunge_records`] `CATEGORY_*` constants.
    pub category: &'a str,
    /// The admin authorizing the expunge.
    pub authorized_by: Uuid,
    /// Every `StorageService` key holding the file's bytes — each is
    /// deleted so no copy survives. Typically the asset's content-addressed
    /// `blobs/<sha>` plus any secondary key a dual-write left (a generated
    /// PDF's `notations/<id>/document.pdf`); may instead be an `lfs/<oid>`
    /// object or a fixed notation key. Use [`storage_keys_for_asset`] to
    /// derive the full set from an asset row. Empty when there is nothing to
    /// remove from object storage.
    pub storage_keys: Vec<String>,
    /// Optional non-content note (e.g. a docket reference).
    pub note: Option<&'a str>,
}

/// Every object-storage key holding an asset's bytes: its canonical
/// content-addressed `storage_key` plus any `secondary_storage_key` a
/// dual-write left (a generated PDF's notation key). A governed expunge must
/// delete all of them, or a copy of the bytes survives outside the asset
/// lifecycle (#470).
#[must_use]
pub fn storage_keys_for_asset(asset: &store::assets::Asset) -> Vec<String> {
    let mut keys = vec![asset.storage_key.clone()];
    if let Some(secondary) = &asset.secondary_storage_key {
        if secondary != &asset.storage_key {
            keys.push(secondary.clone());
        }
    }
    keys
}

/// Run a governed expunge. Returns the id of the audit row.
///
/// # Errors
/// [`ExpungeError::NotAdmin`] if the authorizer isn't an admin,
/// [`ExpungeError::BadCategory`] for an unknown category, or the
/// underlying repo / storage / database error.
pub async fn expunge(
    surreal: &store::surreal::SurrealDb,
    storage: &Arc<dyn StorageService>,
    req: ExpungeRequest<'_>,
) -> Result<Uuid, ExpungeError> {
    // (1) Admin-only — the gate lives in the primitive itself.
    match store::persons::find_by_id(surreal, req.authorized_by).await? {
        Some(p) if p.role.is_admin_tier() => {}
        _ => return Err(ExpungeError::NotAdmin),
    }
    if ![
        expunge_records::CATEGORY_PRIVILEGE,
        expunge_records::CATEGORY_SEALING,
        expunge_records::CATEGORY_CLIENT_REQUEST,
    ]
    .contains(&req.category)
    {
        return Err(ExpungeError::BadCategory(req.category.to_string()));
    }

    // (2) Rewrite history — shells git, so off the async pool.
    let project_code = store::projects::find_by_id(surreal, req.project_id)
        .await?
        .ok_or(ExpungeError::ProjectNotFound(req.project_id))?
        .code;
    let path = req.path.to_string();
    let outcome = match repos::RepoStore::from_env() {
        Ok(repo_store) if repo_store.path_for_code(&project_code).exists() => {
            tokio::task::spawn_blocking(move || repo_store.expunge_path_code(&project_code, &path))
                .await
                .map_err(|e| ExpungeError::Join(e.to_string()))??
        }
        Ok(_) | Err(repos::RepoError::RootUnset) => repos::ExpungeOutcome {
            head_before: None,
            head_after: None,
            path,
        },
        Err(error) => return Err(error.into()),
    };

    // (3) Delete the bytes from object storage — every key holding them, so
    //     no copy of a dual-written document survives. A missing object is
    //     fine (already gone); anything else is a hard error.
    //
    //     An object that an asset row on **another matter** still points at is
    //     retained. Content-addressed keys were deduped workspace-wide before
    //     dedup was scoped to a matter (`store::documents::ingest_bytes_as`),
    //     so the existing corpus still has rows on different matters sharing
    //     one `blobs/<sha>`. Deleting it here would empty an unrelated
    //     matter's document on the authority of an order that never named it,
    //     and if that matter is under a preservation duty, that is spoliation
    //     caused by a case with no connection to it.
    //
    //     A row with no `project_id` does **not** block: it is unattached, not
    //     another matter, and treating it as a referent would let a stray row
    //     defeat a privilege clawback — the failure this primitive exists to
    //     prevent. SQL gives that for free, since `project_id <> $1` is NULL,
    //     not true, for an unattached row.
    for key in &req.storage_keys {
        let referenced_by_another_matter =
            store::assets::referenced_by_another_project(surreal, req.project_id, key).await?;
        if referenced_by_another_matter {
            // Loud, not silent: an order that requires the bytes actually
            // destroyed needs a human to reach the other matter.
            tracing::warn!(
                project_id = %req.project_id,
                storage_key = %key,
                "expunge retained an object another matter still references"
            );
            continue;
        }
        match storage.delete(key).await {
            Ok(()) | Err(cloud::StorageError::NotFound(_)) => {}
            Err(e) => return Err(ExpungeError::Storage(e.to_string())),
        }
    }

    // (4) Record the expunge — who / when / category, not content.
    let id = expunge_records::record(
        surreal,
        &expunge_records::NewExpunge {
            project_id: req.project_id,
            path: req.path,
            category: req.category,
            authorized_by_person_id: req.authorized_by,
            head_before: outcome.head_before.as_deref(),
            head_after: outcome.head_after.as_deref(),
            note: req.note,
        },
    )
    .await?;

    tracing::warn!(
        project_id = %req.project_id,
        category = req.category,
        authorized_by = %req.authorized_by,
        "governed expunge completed"
    );
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::storage_keys_for_asset;

    fn asset_with(storage_key: &str, secondary: Option<&str>) -> store::assets::Asset {
        store::assets::Asset {
            id: uuid::Uuid::now_v7(),
            storage_key: storage_key.into(),
            secondary_storage_key: secondary.map(Into::into),
            content_type: "application/pdf".into(),
            byte_size: 1,
            sha256_hex: "deadbeef".into(),
            project_id: None,
            filename: None,
            kind: None,
            slug: None,
            published_at: None,
            metadata: None,
            source: None,
            received_at: None,
            description: None,
            visibility: store::documents::visibility::INTERNAL.to_string(),
            inserted_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn single_copy_asset_yields_just_its_canonical_key() {
        let a = asset_with("blobs/abc", None);
        assert_eq!(storage_keys_for_asset(&a), vec!["blobs/abc".to_string()]);
    }

    #[test]
    fn dual_written_asset_yields_both_keys() {
        // Regression guard for #470: a generated PDF's notation-key copy
        // must be returned alongside the content-addressed key so expunge
        // deletes every copy of the bytes.
        let a = asset_with("blobs/abc", Some("notations/n/document.pdf"));
        assert_eq!(
            storage_keys_for_asset(&a),
            vec![
                "blobs/abc".to_string(),
                "notations/n/document.pdf".to_string(),
            ]
        );
    }

    #[test]
    fn a_secondary_equal_to_the_canonical_key_is_not_duplicated() {
        let a = asset_with("blobs/abc", Some("blobs/abc"));
        assert_eq!(storage_keys_for_asset(&a), vec!["blobs/abc".to_string()]);
    }
}
