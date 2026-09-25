//! `navigator ops application publish` — build a Project application from
//! its repository and upload the bundle to a deployment's applications
//! bucket.
//!
//! The clone, install, build, and manifest checks are
//! [`super::sample_project::build_from_repository`], unchanged from the local
//! `dev sample-project` loop. What differs is the destination: instead of
//! staging `dist/` under `.devx/` for a local boot, every object in
//! [`store::sample_project::publish_plan`] is written to the named bucket, in
//! plan order, through the operator's own Application Default Credentials.
//!
//! This is the operator lane. A Project repository's own CI publishes through
//! `.github/actions/application-publish`; the three public sample
//! repositories deliberately carry no such workflow, and a production-profile
//! boot never writes a portal bundle. When a bundle has to be put back — or
//! put up for the first time from a machine rather than a runner — this is
//! the command that does it.
//!
//! ## Four properties carried, not reimplemented
//!
//! * **Order.** `publish_plan` sorts the entry document last, so no
//!   `index.html` naming a new hashed asset is readable before that asset
//!   exists. The upload walks the plan as given and never reorders or
//!   parallelizes across it.
//! * **Every object, every time.** The plan enumerates from disk and each
//!   object is written unconditionally. That is what keeps the applications
//!   bucket's object-age Delete rule safe: a live asset's `updateTime` is
//!   refreshed on every publish, so the rule can only ever reach an orphan.
//!   Do not add a skip-unchanged optimization here.
//! * **Prune only what this publish's own plan already superseded.**
//!   [`store::sample_project::prune_plan`] computes what to delete from a
//!   manifest object this same code wrote on its own previous publish, never
//!   from a bucket listing — one Project's publish still never touches
//!   another's objects, because the manifest is scoped to this Project's own
//!   prefix and nothing here ever calls `list`. Deletion only ever runs after
//!   every object the new plan names is already uploaded, so nothing live is
//!   ever removed while something still points at it.
//! * **A single bad build cannot empty a bucket prefix.** A build that would
//!   prune more than `store::sample_project::MAX_PRUNE_PERCENT` of the
//!   previous publish's objects is refused outright rather than partially
//!   pruned — see [`prune_stale`].
//!
//! ## The bucket is named every time
//!
//! `--bucket` is required and has no environment fallback. A sourced
//! `.devx/env` sets `NAVIGATOR_APPLICATIONS_BUCKET` for the local `fs`
//! backend, and a deployment's runtime configuration sets it for the pod;
//! neither is ever the right target for an operator publish. Naming a bucket
//! is naming a deployment, so it is spelled out on every invocation.
//!
//! ## The Project comes from the bundle
//!
//! The Project code is read from the checkout's own `navigator.yaml` and
//! validated by [`store::sample_project::project_code_for`] against the code
//! the operator asked for. A bundle naming a different Project is refused
//! before any object is written; the repository name is never consulted.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{bail, Context, Result};
use cloud::{GcsStorage, GcsStorageConfig, StorageError, StorageService};
use store::sample_project::{project_code_for, publish_plan, PortalObject, ENTRY_DOCUMENT};

use super::sample_project::{build_from_repository, repo_basename, resolve_repo};

/// Which Projects one invocation publishes, and where each repository comes
/// from.
///
/// No `--project` publishes every sample matter, the same default as
/// `dev sample-project`. Naming one publishes that Project alone — any valid
/// Project code, not only a sample matter, because the operator lane exists
/// for real Projects whose production boot never writes a bundle. The
/// repository for a named Project is [`repository_for`]'s business.
///
/// An invalid code is refused here, before anything is cloned: the code is a
/// bucket prefix, and `is_valid_code` is what keeps a path segment out of it.
fn choose_targets<'a>(project: Option<&'a str>, known: &[&'a str]) -> Result<Vec<&'a str>> {
    match project {
        None => Ok(known.to_vec()),
        Some(code) if store::projects::is_valid_code(code) => Ok(vec![code]),
        Some(code) => bail!(
            "`{code}` is not a valid Project code: lowercase letters and digits joined by single \
             hyphens"
        ),
    }
}

/// The repository to build one Project from, when no store has to be asked.
///
/// `--repo` wins outright. Otherwise a sample matter builds from its
/// compiled-in repository: the seed writes exactly that URL onto the matter's
/// row, so a store would add nothing, and a publish to a deployment must not
/// depend on a local development database being up. `None` means the code is
/// not a sample matter and the caller has to read its Project row.
fn choose_repository(explicit: Option<&str>, compiled_in: Option<&str>) -> Option<String> {
    explicit.or(compiled_in).map(str::to_string)
}

/// The repository to build `code` from: [`choose_repository`], falling back to
/// the Project row for a code that is not a sample matter — the lookup
/// `dev sample-project` makes, whose error names `--repo` as the way around a
/// store that is not reachable.
fn repository_for(code: &str, explicit: Option<&str>) -> Result<String> {
    match choose_repository(explicit, store::seed::sample_matter_repository(code)) {
        Some(url) => Ok(url),
        None => resolve_repo(code, None),
    }
}

/// What `--dry-run` prints instead of uploading.
///
/// The resolved bucket, every key in the order it would be written with its
/// content type and cache directive, and a closing line naming the object
/// count and the *last* key — so an operator can see the entry document is
/// last without reading the whole plan. That last line is the one property a
/// diff of two plans will not make obvious.
fn dry_run_report(bucket: &str, code: &str, plan: &[PortalObject]) -> String {
    let mut out =
        format!("navigator: dry run — would publish Project `{code}` to gs://{bucket}/\n");
    for object in plan {
        let _ = writeln!(
            out,
            "  {}  ({}; cache-control: {})",
            object.key, object.content_type, object.cache_control
        );
    }
    match plan.last() {
        Some(last) => {
            let _ = writeln!(
                out,
                "navigator: {} object(s); last in order: {}. Nothing was written.",
                plan.len(),
                last.key
            );
        }
        None => out.push_str("navigator: 0 objects; nothing to publish.\n"),
    }
    out
}

/// Write every object in `plan`, in plan order, and return how many landed.
///
/// One `put_cached` per object with the plan's own content type and cache
/// directive — the same call the seed makes for a staged bundle. Nothing is
/// listed, compared, skipped, or deleted.
async fn upload_plan(storage: &dyn StorageService, plan: &[PortalObject]) -> Result<usize> {
    for object in plan {
        let bytes = std::fs::read(&object.source)
            .with_context(|| format!("reading {}", object.source.display()))?;
        storage
            .put_cached(
                &object.key,
                &bytes,
                object.content_type,
                object.cache_control,
            )
            .await
            .with_context(|| format!("uploading {}", object.key))?;
    }
    Ok(plan.len())
}

/// Delete what the previous publish left behind that this one's plan no
/// longer carries, then record this plan's own key set so the *next* publish
/// has something to diff against.
///
/// Reads the manifest object the previous publish wrote — absent on a
/// Project's first publish, or one from before pruning existed — and hands it
/// to [`store::sample_project::prune_plan`] alongside `plan`. A build that
/// would prune more than `store::sample_project::MAX_PRUNE_PERCENT` of the
/// previous publish's objects is refused outright rather than partially
/// pruned: see the module doc. The caller uploads `plan` in full before
/// calling this, so every key deleted here is already confirmed absent from
/// the bundle `index.html` was just published pointing at.
async fn prune_stale(storage: &dyn StorageService, code: &str, plan: &[PortalObject]) -> Result<usize> {
    let manifest_key = store::sample_project::manifest_key(code);
    let previous = match storage.get(&manifest_key).await {
        Ok(object) => Some(store::sample_project::parse_manifest(&object.bytes)),
        Err(StorageError::NotFound(_)) => None,
        Err(error) => return Err(error).with_context(|| format!("reading {manifest_key}")),
    };

    let prefix = format!("{}/", store::sample_project::portal_prefix(code));
    let pruned = match store::sample_project::prune_plan(previous.as_deref(), plan, code) {
        store::sample_project::PrunePlan::NoPriorManifest => 0,
        store::sample_project::PrunePlan::Refused { stale, previous } => {
            bail!(
                "publish would prune {} of {previous} previously published object(s) for \
                 Project `{code}` — more than {}%, which looks like a broken build rather \
                 than an intentional removal. Nothing was deleted; fix the build and \
                 republish, or prune the bucket by hand if this really is intentional. \
                 Would-be-pruned: {stale:?}",
                stale.len(),
                store::sample_project::MAX_PRUNE_PERCENT,
            );
        }
        store::sample_project::PrunePlan::Prune(stale) => {
            for key in &stale {
                storage
                    .delete(&format!("{prefix}{key}"))
                    .await
                    .with_context(|| format!("pruning {prefix}{key}"))?;
            }
            stale.len()
        }
    };

    let manifest_bytes = store::sample_project::render_manifest(plan, code);
    storage
        .put(
            &manifest_key,
            &manifest_bytes,
            store::sample_project::MANIFEST_CONTENT_TYPE,
        )
        .await
        .with_context(|| format!("writing {manifest_key}"))?;

    Ok(pruned)
}

/// The plan for one built bundle, refused before any object is written when
/// the bundle names a Project other than `expected`.
fn plan_for(manifest: &str, expected: &str, dist: &Path, repo: &str) -> Result<Vec<PortalObject>> {
    let code = project_code_for(manifest, expected).with_context(|| {
        format!(
            "{} declares a Project this publish is not for — nothing was uploaded",
            repo_basename(repo)
        )
    })?;
    let plan = publish_plan(dist, &code)?;
    anyhow::ensure!(
        !plan.is_empty(),
        "{} has no {ENTRY_DOCUMENT} — that is a failed build, not a bundle",
        dist.display()
    );
    Ok(plan)
}

/// Entry point for `navigator ops application publish`.
pub(super) fn run(
    bucket: &str,
    project: Option<&str>,
    repo: Option<&str>,
    git_ref: Option<&str>,
    dry_run: bool,
    keep: bool,
) -> Result<()> {
    let bucket = bucket.trim();
    if bucket.is_empty() {
        bail!("--bucket names the deployment's applications bucket and cannot be empty");
    }
    let known = store::seed::sample_matter_codes();
    let targets = choose_targets(project, &known)?;
    super::require_tools(&["git", "pnpm"])?;

    // The bucket is opened before any build is spent, so a missing credential
    // is reported first. A dry run never touches the bucket at all.
    let runtime = tokio::runtime::Runtime::new().context("create tokio runtime")?;
    let storage = if dry_run {
        None
    } else {
        let name = bucket.to_string();
        let storage = runtime
            .block_on(async move {
                // This operator command targets real GCS through ADC.
                GcsStorage::new_from_config(GcsStorageConfig {
                    bucket: name,
                    endpoint: None,
                })
                .await
            })
            .with_context(|| {
                format!("open bucket `{bucket}` — is ADC configured for its project?")
            })?;
        Some(storage)
    };

    let mut published = 0;
    for expected in &targets {
        // A Project-row lookup reads the store through its own runtime, so it
        // runs here, outside `block_on`.
        let url = repository_for(expected, repo)?;
        let built = build_from_repository(&url, git_ref)?;
        let plan = plan_for(&built.manifest, expected, &built.dist, &url)?;

        match &storage {
            None => print!("{}", dry_run_report(bucket, expected, &plan)),
            Some(storage) => {
                let count = runtime.block_on(upload_plan(storage, &plan))?;
                published += count;
                // Pruning reads and deletes only after every object `plan`
                // names is already uploaded, so nothing removed here could
                // have been named by the `index.html` just published.
                let pruned = runtime.block_on(prune_stale(storage, expected, &plan))?;
                println!(
                    "navigator: published {count} object(s) for Project `{expected}` to \
                     gs://{bucket}/{}/ — {ENTRY_DOCUMENT} last{}",
                    store::sample_project::portal_prefix(expected),
                    if pruned > 0 {
                        format!(", pruned {pruned} stale object(s)")
                    } else {
                        String::new()
                    }
                );
            }
        }

        if keep {
            built.keep();
        }
    }

    if storage.is_some() {
        println!(
            "navigator: {published} object(s) across {} Project(s) published to gs://{bucket}/",
            targets.len()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::Duration;

    use cloud::{StorageError, StoredObject};

    use super::*;

    /// A storage backend that records every call so a test can assert not
    /// only what landed but the order it landed in — and what was never
    /// called.
    /// One recorded `put_cached`: key, bytes, content type, cache directive.
    type Put = (String, Vec<u8>, String, String);
    /// One recorded plain `put`: key, bytes, content type — the manifest
    /// write, which carries no cache directive of its own.
    type PlainPut = (String, Vec<u8>, String);

    #[derive(Default)]
    struct Recording {
        puts: Mutex<Vec<Put>>,
        plain_puts: Mutex<Vec<PlainPut>>,
        deletes: Mutex<Vec<String>>,
        /// Objects already "in the bucket" before the call under test — the
        /// manifest a previous publish left, for a test that exercises
        /// pruning against it.
        preexisting: Mutex<std::collections::HashMap<String, Vec<u8>>>,
    }

    #[async_trait::async_trait]
    impl StorageService for Recording {
        async fn put(
            &self,
            key: &str,
            bytes: &[u8],
            content_type: &str,
        ) -> Result<(), StorageError> {
            self.plain_puts.lock().expect("lock").push((
                key.to_string(),
                bytes.to_vec(),
                content_type.to_string(),
            ));
            Ok(())
        }

        async fn put_cached(
            &self,
            key: &str,
            bytes: &[u8],
            content_type: &str,
            cache_control: &str,
        ) -> Result<(), StorageError> {
            self.puts.lock().expect("lock").push((
                key.to_string(),
                bytes.to_vec(),
                content_type.to_string(),
                cache_control.to_string(),
            ));
            Ok(())
        }

        async fn get(&self, key: &str) -> Result<StoredObject, StorageError> {
            match self.preexisting.lock().expect("lock").get(key) {
                Some(bytes) => Ok(StoredObject {
                    key: key.to_string(),
                    bytes: bytes.clone(),
                    content_type: store::sample_project::MANIFEST_CONTENT_TYPE.to_string(),
                }),
                None => Err(StorageError::NotFound(key.to_string())),
            }
        }

        async fn delete(&self, key: &str) -> Result<(), StorageError> {
            self.deletes.lock().expect("lock").push(key.to_string());
            Ok(())
        }

        async fn signed_url(
            &self,
            _key: &str,
            _expires_in: Duration,
        ) -> Result<String, StorageError> {
            Err(StorageError::Unsupported("signed_url"))
        }
    }

    fn write(root: &Path, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, bytes).expect("write");
    }

    /// A built `dist/` with hashed assets and an entry document.
    fn sample_dist() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "index.html", b"<!doctype html>");
        write(dir.path(), "assets/app-abc123.js", b"console.log(1)");
        write(dir.path(), "assets/app-abc123.css", b"body{}");
        write(dir.path(), "favicon.svg", b"<svg/>");
        dir
    }

    const MANIFEST: &str = "project: sample-litigation\nhost: staging.neonlaw.com\n";

    #[test]
    fn no_project_publishes_every_sample_matter_and_a_named_one_publishes_it_alone() {
        let known = ["sample-litigation", "sample-transactional", "sample-estate"];
        assert_eq!(choose_targets(None, &known).expect("all"), known.to_vec());
        assert_eq!(
            choose_targets(Some("sample-estate"), &known).expect("one"),
            vec!["sample-estate"]
        );
    }

    #[test]
    fn a_named_project_need_not_be_a_sample_matter_but_must_be_a_valid_code() {
        let known = ["sample-litigation"];
        // The operator lane exists for real Projects, whose repository comes
        // from the Project row or `--repo`.
        assert_eq!(
            choose_targets(Some("acme"), &known).expect("any valid code"),
            vec!["acme"]
        );
        // A code is a bucket prefix; a path segment is refused before a clone.
        let error = choose_targets(Some("../other"), &known).expect_err("invalid code");
        assert!(
            error.to_string().contains("not a valid Project code"),
            "{error}"
        );
        let error = choose_targets(Some("Sample-Estate"), &known).expect_err("uppercase");
        assert!(
            error.to_string().contains("not a valid Project code"),
            "{error}"
        );
    }

    #[test]
    fn a_sample_matter_builds_from_its_compiled_in_repository_without_a_store() {
        let compiled_in = store::seed::sample_matter_repository("sample-litigation");
        assert!(
            compiled_in.is_some(),
            "every sample matter records a repository"
        );
        assert_eq!(
            choose_repository(None, compiled_in),
            compiled_in.map(str::to_string)
        );
        // `--repo` wins over the compiled-in URL — a fork or a local mirror.
        assert_eq!(
            choose_repository(Some("https://forge.example/o/fork"), compiled_in).as_deref(),
            Some("https://forge.example/o/fork")
        );
        // A code that is not a sample matter has no compiled-in URL; only the
        // Project row can answer, and that is the caller's lookup.
        assert_eq!(store::seed::sample_matter_repository("acme"), None);
        assert_eq!(choose_repository(None, None), None);
        assert_eq!(
            choose_repository(Some("https://forge.example/o/acme"), None).as_deref(),
            Some("https://forge.example/o/acme")
        );
    }

    #[test]
    fn the_plan_refuses_a_bundle_declaring_another_project_before_anything_is_written() {
        let dist = sample_dist();
        let error = plan_for(
            MANIFEST,
            "sample-estate",
            dist.path(),
            "https://forge.example/o/sample-estate",
        )
        .expect_err("a valid-but-different code is the dangerous case");
        let text = format!("{error:#}");
        assert!(text.contains("sample-estate"), "{text}");
        assert!(text.contains("names Project `sample-litigation`"), "{text}");
        assert!(text.contains("nothing was uploaded"), "{text}");
    }

    #[test]
    fn the_plan_refuses_a_dist_without_an_entry_document() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "assets/app-abc123.js", b"x");
        let error = plan_for(MANIFEST, "sample-litigation", dir.path(), "r")
            .expect_err("no index.html is a failed build");
        assert!(error.to_string().contains("failed build"), "{error}");
    }

    #[test]
    fn the_plan_for_a_matching_bundle_covers_every_file_under_dist() {
        let dist = sample_dist();
        let plan = plan_for(MANIFEST, "sample-litigation", dist.path(), "r").expect("plan");
        let keys: Vec<&str> = plan.iter().map(|o| o.key.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "sample-litigation/portal/assets/app-abc123.css",
                "sample-litigation/portal/assets/app-abc123.js",
                "sample-litigation/portal/favicon.svg",
                "sample-litigation/portal/index.html",
            ],
            "every file, in plan order, with the entry document last — not a diff against a bucket"
        );
    }

    #[test]
    fn the_upload_writes_every_object_in_plan_order_with_its_headers_and_deletes_nothing() {
        let dist = sample_dist();
        let plan = publish_plan(dist.path(), "sample-litigation").expect("plan");
        let storage = Recording::default();

        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        let count = runtime
            .block_on(upload_plan(&storage, &plan))
            .expect("upload");

        assert_eq!(count, plan.len());
        let puts = storage.puts.lock().expect("lock");
        let written: Vec<&str> = puts.iter().map(|(key, ..)| key.as_str()).collect();
        let planned: Vec<&str> = plan.iter().map(|o| o.key.as_str()).collect();
        assert_eq!(written, planned, "plan order is carried, never re-sorted");
        assert!(
            written.last().expect("some").ends_with(ENTRY_DOCUMENT),
            "the entry document lands last"
        );

        let (key, bytes, content_type, cache_control) = puts
            .iter()
            .find(|(key, ..)| key.ends_with("index.html"))
            .expect("entry");
        assert_eq!(key, "sample-litigation/portal/index.html");
        assert_eq!(bytes, b"<!doctype html>");
        assert_eq!(content_type, "text/html; charset=utf-8");
        assert_eq!(cache_control, store::sample_project::ENTRY_CACHE_CONTROL);

        let (_, _, content_type, cache_control) = puts
            .iter()
            .find(|(key, ..)| key.ends_with("app-abc123.js"))
            .expect("asset");
        assert_eq!(content_type, "text/javascript; charset=utf-8");
        assert_eq!(cache_control, store::sample_project::ASSET_CACHE_CONTROL);

        assert!(
            storage.deletes.lock().expect("lock").is_empty(),
            "upload_plan itself never deletes — pruning is prune_stale's own, later, separate step"
        );
        assert!(
            storage.plain_puts.lock().expect("lock").is_empty(),
            "every portal object must carry a cache directive, so upload_plan calls \
             put_cached exclusively, never the plain put the manifest write uses"
        );
    }

    #[test]
    fn a_first_publish_has_nothing_to_prune_and_writes_its_own_manifest() {
        let dist = sample_dist();
        let plan = publish_plan(dist.path(), "sample-litigation").expect("plan");
        let storage = Recording::default();

        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        let pruned = runtime
            .block_on(prune_stale(&storage, "sample-litigation", &plan))
            .expect("prune");

        assert_eq!(pruned, 0);
        assert!(storage.deletes.lock().expect("lock").is_empty());

        let manifest_key = store::sample_project::manifest_key("sample-litigation");
        let (key, bytes, content_type) = storage
            .plain_puts
            .lock()
            .expect("lock")
            .iter()
            .find(|(key, ..)| key == &manifest_key)
            .cloned()
            .expect("the manifest is written even on a first publish");
        assert_eq!(key, manifest_key);
        assert_eq!(content_type, store::sample_project::MANIFEST_CONTENT_TYPE);
        let parsed = store::sample_project::parse_manifest(&bytes);
        assert_eq!(parsed.len(), plan.len());
    }

    #[test]
    fn a_later_publish_prunes_only_what_the_new_build_dropped() {
        let dist = sample_dist();
        let plan = publish_plan(dist.path(), "sample-litigation").expect("plan");
        let storage = Recording::default();

        // The previous publish's manifest names one extra key the new build
        // no longer carries — a renamed or removed fixed-name file.
        let mut previous_keys: Vec<String> = plan
            .iter()
            .map(|o| {
                o.key
                    .strip_prefix("sample-litigation/portal/")
                    .unwrap()
                    .to_string()
            })
            .collect();
        previous_keys.push("pdf/renamed-away.pdf".to_string());
        let manifest_key = store::sample_project::manifest_key("sample-litigation");
        storage.preexisting.lock().expect("lock").insert(
            manifest_key.clone(),
            previous_keys.join("\n").into_bytes(),
        );

        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        let pruned = runtime
            .block_on(prune_stale(&storage, "sample-litigation", &plan))
            .expect("prune");

        assert_eq!(pruned, 1);
        assert_eq!(
            *storage.deletes.lock().expect("lock"),
            vec!["sample-litigation/portal/pdf/renamed-away.pdf".to_string()],
            "only the key the new build dropped is deleted, under this Project's own prefix"
        );
    }

    #[test]
    fn a_publish_that_would_prune_too_much_is_refused_and_deletes_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "index.html", b"x");
        let plan = publish_plan(dir.path(), "sample-litigation").expect("plan");
        let storage = Recording::default();

        // Three objects lived before; this build carries only index.html.
        let manifest_key = store::sample_project::manifest_key("sample-litigation");
        storage.preexisting.lock().expect("lock").insert(
            manifest_key,
            b"index.html\nassets/a.js\nassets/b.js\n".to_vec(),
        );

        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        let error = runtime
            .block_on(prune_stale(&storage, "sample-litigation", &plan))
            .expect_err("pruning two of three prior objects must be refused");

        assert!(format!("{error:#}").contains("sample-litigation"), "{error:#}");
        assert!(
            storage.deletes.lock().expect("lock").is_empty(),
            "a refused prune must delete nothing, not a partial, silently-bounded set"
        );
        assert!(
            storage.plain_puts.lock().expect("lock").is_empty(),
            "a refused prune must not advance the manifest either — the next \
             publish should see the same previous manifest and refuse again \
             until a human intervenes"
        );
    }

    #[test]
    fn a_missing_source_file_fails_the_upload_by_name() {
        let plan = vec![PortalObject {
            key: "acme/portal/index.html".to_string(),
            source: PathBuf::from("/nonexistent/index.html"),
            content_type: "text/html; charset=utf-8",
            cache_control: store::sample_project::ENTRY_CACHE_CONTROL,
        }];
        let storage = Recording::default();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        let error = runtime
            .block_on(upload_plan(&storage, &plan))
            .expect_err("unreadable source");
        assert!(format!("{error:#}").contains("index.html"), "{error:#}");
        assert!(storage.puts.lock().expect("lock").is_empty());
    }

    #[test]
    fn the_dry_run_names_the_bucket_every_key_the_count_and_the_last_key() {
        let dist = sample_dist();
        let plan = publish_plan(dist.path(), "sample-litigation").expect("plan");
        let report = dry_run_report("neon-law-stg-applications", "sample-litigation", &plan);

        assert!(
            report.contains("gs://neon-law-stg-applications/"),
            "{report}"
        );
        for object in &plan {
            assert!(report.contains(&object.key), "{report}");
        }
        assert!(
            report.contains("4 object(s); last in order: sample-litigation/portal/index.html"),
            "{report}"
        );
        assert!(report.contains("Nothing was written."), "{report}");
        assert!(
            report.contains("cache-control: no-store"),
            "the entry document's directive is visible in the rehearsal: {report}"
        );
    }

    #[test]
    fn an_empty_plan_dry_run_says_so() {
        let report = dry_run_report("b", "acme", &[]);
        assert!(report.contains("0 objects; nothing to publish"), "{report}");
    }
}
