//! `navigator template forms sync` / `navigator template forms fields` — vendor, pin,
//! and inspect the blank government forms in the public assets bucket.
//!
//! The bucket is the only home of the blank bytes; the repo keeps the
//! diffable text, including a `.sha256` pin per form. `sync` closes the
//! loop in both directions:
//!
//! - a **local working copy** at `templates/notations/<object_path>` (untracked —
//!   the human downloads or re-authors it there) is uploaded and its
//!   repo pin rewritten to match;
//! - **without** a working copy, the bucket object is pulled and
//!   verified against the pin — a missing object or a mismatch is a
//!   loud non-zero exit, because the fill path would refuse the same
//!   bytes.
//!
//! `fields` prints a blank's `AcroForm` `/T` names (pulled + pin-verified
//! first), the ground truth for authoring its `.fields.toml` or
//! re-authoring the field layer.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use std::sync::Arc;

use cloud::StorageService;

/// Long-lived cache: a pinned form object is immutable in practice (a
/// re-vendor rewrites the pin in the same PR), so downstream caches may
/// hold it.
const FORM_CACHE_CONTROL: &str = "public, max-age=604800";

/// One registry form resolved onto the local checkout: where its
/// working copy would live and where its pin file lives.
struct SyncItem {
    code: String,
    object_path: String,
    /// `templates/notations/<object_path>` — the untracked working copy.
    local_blank: PathBuf,
    /// The tracked sibling `.sha256` pin.
    pin_file: PathBuf,
    /// The pin as compiled into the registry — the fallback when the
    /// pin file cannot be read; the file wins so a pin rewritten by a
    /// previous `sync` in this checkout verifies without a rebuild.
    pinned: String,
}

/// What `sync` did for one form.
#[derive(Debug, PartialEq, Eq)]
enum SyncOutcome {
    /// Local working copy uploaded where needed; pin file (re)written
    /// when it didn't match the working copy.
    Vendored { pin_rewritten: bool },
    /// No working copy; the bucket object matches the pin.
    Verified,
}

fn items_from_registry(workspace_root: &Path) -> anyhow::Result<Vec<SyncItem>> {
    Ok(forms::registry()?
        .into_iter()
        .map(|form| {
            let local_blank = workspace_root
                .join("templates")
                .join("notations")
                .join(form.object_path);
            let pin_file = local_blank.with_extension("sha256");
            SyncItem {
                code: form.code.to_string(),
                object_path: form.object_path.to_string(),
                local_blank,
                pin_file,
                pinned: form.pinned_sha256().to_string(),
            }
        })
        .collect())
}

/// The workspace root: `sync` runs from the checkout (it rewrites pin
/// files), so walk up from the current directory to the first ancestor
/// carrying `templates/notations/forms`.
fn workspace_root() -> anyhow::Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    cwd.ancestors()
        .find(|p| p.join("templates/notations/forms").is_dir())
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no `templates/notations/forms` directory above {} — run from the workspace \
                 checkout",
                cwd.display()
            )
        })
}

/// Entry point for `cli forms sync`. `bucket` defaults to the
/// `NAVIGATOR_ASSETS_BUCKET` env var — the public `<project>-assets`
/// bucket, deliberately distinct from the documents bucket so blanks
/// never land in the confidential lane (and vice versa).
pub fn run_sync(bucket: Option<&str>) -> ExitCode {
    with_assets_storage("forms sync", bucket, |storage| async move {
        let items = items_from_registry(&workspace_root()?)?;
        let mut vendored = 0usize;
        let mut verified = 0usize;
        for item in &items {
            match sync_one(storage.as_ref(), item).await? {
                SyncOutcome::Vendored { pin_rewritten } => {
                    vendored += 1;
                    if pin_rewritten {
                        println!(
                            "  {}: uploaded working copy, pin rewritten at {} \
                             (rebuild to bake the new pin into the binaries)",
                            item.code,
                            item.pin_file.display()
                        );
                    } else {
                        println!("  {}: working copy in sync, pin unchanged", item.code);
                    }
                }
                SyncOutcome::Verified => {
                    verified += 1;
                    println!("  {}: bucket object matches its pin", item.code);
                }
            }
        }
        println!("navigator: forms sync: {vendored} vendored, {verified} verified");
        Ok(())
    })
}

/// Entry point for `cli forms fields <code>`: pull the blank, verify
/// its pin, and print its `AcroForm` `/T` names one per line.
pub fn run_fields(code: &str, bucket: Option<&str>) -> ExitCode {
    let code = code.to_string();
    with_assets_storage("forms fields", bucket, |storage| async move {
        let form = forms::get(&code)?
            .ok_or_else(|| anyhow::anyhow!("`{code}` is not in the vendored forms registry"))?;
        let blank = storage.get(form.object_path).await.map_err(|e| {
            anyhow::anyhow!(
                "pull `{}`: {e} — vendor the blank with `navigator template forms sync`",
                form.object_path
            )
        })?;
        form.verify(&blank.bytes)?;
        for name in pdf::field_names(&blank.bytes)? {
            println!("{name}");
        }
        Ok(())
    })
}

/// Entry point for `cli forms re-author <code>` (#256 item 1): pull the
/// blank, verify its pin, and transform its field layer so the `AcroForm`
/// `/T` names *are* questionnaire state paths — the recorded judgment in
/// the form's `.fields.toml` drives every rename, radio merge, and
/// pre-printed literal, and every unmapped field lands in the
/// `unmapped__` namespace. Writes the re-authored working copy to
/// `templates/notations/<object_path>` plus its diffable `.fields` manifest, then
/// prints the human steps that remain: visual QA of the filled blank,
/// `navigator template forms sync` to vendor + re-pin, and deleting the
/// `.fields.toml` the transform just consumed.
pub fn run_reauthor(code: &str, bucket: Option<&str>) -> ExitCode {
    let code = code.to_string();
    with_assets_storage("forms re-author", bucket, |storage| async move {
        let root = workspace_root()?;
        let form = forms::get(&code)?
            .ok_or_else(|| anyhow::anyhow!("`{code}` is not in the vendored forms registry"))?;
        let map = load_reauthor_map(&root, form.object_path)?;

        let blank = storage.get(form.object_path).await.map_err(|e| {
            anyhow::anyhow!(
                "pull `{}`: {e} — vendor the blank with `navigator template forms sync`",
                form.object_path
            )
        })?;
        // The pin file wins over the compiled-in pin, exactly like
        // `sync`, so a re-vendor earlier in this checkout verifies
        // without a rebuild.
        let local_blank = root
            .join("templates")
            .join("notations")
            .join(form.object_path);
        let pin_file = local_blank.with_extension("sha256");
        let pinned = std::fs::read_to_string(&pin_file).map_or_else(
            |_| form.pinned_sha256().to_string(),
            |s| s.trim().to_string(),
        );
        forms::verify_sha256(&pinned, &blank.bytes)?;

        let states = questionnaire_states(&root, form.object_path)?;
        let reauthored = reauthor_bytes(&map, &blank.bytes, &states)?;

        if let Some(parent) = local_blank.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&local_blank, &reauthored)?;
        let mut manifest = pdf::field_names(&reauthored)?;
        manifest.sort();
        let manifest_path = local_blank.with_extension("fields");
        std::fs::write(&manifest_path, manifest.join("\n") + "\n")?;

        println!(
            "navigator: forms re-author: `{code}` re-authored ({} fields)",
            manifest.len()
        );
        println!("  working copy: {}", local_blank.display());
        println!("  manifest:     {}", manifest_path.display());
        println!("  next: fill the working copy with sample answers and visually QA it,");
        println!("        then `navigator template forms sync` to vendor + re-pin, and delete the");
        println!("        `.fields.toml` this transform consumed.");
        Ok(())
    })
}

/// Load the `<code>.fields.toml` map beside a form's blank — the
/// transient re-author input, read from the working tree (not compiled
/// in: no form fills through a map, so the map lives only until the
/// blank is re-authored). A missing file means the form is already
/// re-authored, or was never mapped.
fn load_reauthor_map(root: &Path, object_path: &str) -> anyhow::Result<forms::FieldMap> {
    let map_path = root
        .join("templates")
        .join("notations")
        .join(object_path)
        .with_extension("fields.toml");
    let raw = std::fs::read_to_string(&map_path).map_err(|_| {
        anyhow::anyhow!(
            "no `.fields.toml` at {} — its judgment layer is the transform's input, so a \
             form without one is already re-authored (or never mapped)",
            map_path.display()
        )
    })?;
    Ok(forms::parse_field_map(&raw)?)
}

/// The pure transform core of `run_reauthor`: strip a static XFA packet
/// when present, plan from the blank's own `/T` names and questionnaire
/// states, then apply the pdf-side field-layer transformation.
/// Byte-deterministic — same inputs, same bytes — so the `.sha256` pin
/// is reproducible from the original blank
/// (`reauthoring_the_same_input_twice_is_byte_identical` pins this).
fn reauthor_bytes(
    map: &forms::FieldMap,
    blank: &[u8],
    states: &[String],
) -> anyhow::Result<Vec<u8>> {
    let blank = pdf::strip_static_xfa(blank)?;
    let names = pdf::field_names(&blank)?;
    let plan = forms::reauthor::plan(map, &names, states)?;
    let spec = pdf::ReauthorSpec {
        renames: plan.renames,
        radios: plan
            .radios
            .into_iter()
            .map(|(name, members)| pdf::RadioMergeSpec {
                name,
                members: members
                    .into_iter()
                    .map(|member| pdf::RadioMergeMember {
                        field: member.field,
                        source_export: member.source_export,
                        final_export: member.final_export,
                    })
                    .collect(),
            })
            .collect(),
        literals: plan.literals,
    };
    Ok(pdf::reauthor(&blank, &spec)?)
}

/// The sibling notation's declared questionnaire states — the resolution
/// target for every `.fields.toml` question reference (the same read the
/// `question_code_contract` guard performs).
fn questionnaire_states(root: &Path, object_path: &str) -> anyhow::Result<Vec<String>> {
    #[derive(serde::Deserialize)]
    struct Notation {
        questionnaire: std::collections::BTreeMap<String, serde_yaml::Value>,
    }
    let md = root
        .join("templates")
        .join("notations")
        .join(object_path.replace(".pdf", ".md"));
    let contents = std::fs::read_to_string(&md)
        .map_err(|e| anyhow::anyhow!("read notation {}: {e}", md.display()))?;
    let fm = contents
        .strip_prefix("---\n")
        .and_then(|rest| rest.find("\n---").map(|end| &rest[..end]))
        .ok_or_else(|| anyhow::anyhow!("{}: no `---` frontmatter block", md.display()))?;
    let notation: Notation = serde_yaml::from_str(fm)
        .map_err(|e| anyhow::anyhow!("{}: parse frontmatter: {e}", md.display()))?;
    Ok(notation
        .questionnaire
        .into_keys()
        .filter(|s| s != "BEGIN" && s != "END")
        .collect())
}

/// The assets-lane env lookup for the `forms` subcommands: a `--bucket`
/// flag overrides `NAVIGATOR_ASSETS_BUCKET`; every other key defers to
/// `get` (the process environment in production, an injected map in
/// tests). Feeds [`cloud::assets_from_lookup`], which requires the
/// backend (`NAVIGATOR_STORAGE_BACKEND`) and resolves the lane's own
/// bucket fallback exactly as `web` does.
fn assets_lookup<G: Fn(&str) -> Option<String>>(
    bucket: Option<&str>,
    get: &G,
    key: &str,
) -> Option<String> {
    if key == "NAVIGATOR_ASSETS_BUCKET" {
        if let Some(b) = bucket.map(str::trim).filter(|b| !b.is_empty()) {
            return Some(b.to_string());
        }
    }
    get(key)
}

/// Whether a `--bucket` override was supplied against a backend that has
/// no notion of named buckets. `--bucket` names an object-store bucket,
/// but the `fs` backend writes to a local root, so the pairing is a
/// contradiction the `forms` commands refuse rather than silently write to
/// `./var/storage`. `backend` is the raw `NAVIGATOR_STORAGE_BACKEND` value;
/// an unset one is not this function's business — `cloud`'s selector fails
/// closed on it and names the variable itself.
fn bucket_without_object_store(bucket: Option<&str>, backend: Option<String>) -> bool {
    bucket.is_some() && backend.is_some_and(|b| b == "fs")
}

/// Shared storage + runtime scaffolding for the `forms` subcommands. The
/// handle is backend-agnostic — GCS in prod, S3/Garage in the local KIND
/// deps, `fs` in tests — so `forms sync` vendors to whatever object store
/// the deps actually run, not GCS alone.
fn with_assets_storage<F, Fut>(what: &str, bucket: Option<&str>, run: F) -> ExitCode
where
    F: FnOnce(Arc<dyn StorageService>) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("navigator: {what}: tokio runtime: {e}");
            return ExitCode::from(2);
        }
    };
    runtime.block_on(async move {
        let env = |key: &str| std::env::var(key).ok();
        // A `--bucket` names an object-store bucket, but the `fs` backend
        // writes to a local root and ignores bucket names — so honoring the
        // flag against `fs` would silently write to `./var/storage` and
        // report success while the intended remote bucket stays untouched.
        // Refuse that contradiction loudly. An unset backend needs no guard
        // here: `assets_from_lookup` below rejects it by name.
        if bucket_without_object_store(bucket, env("NAVIGATOR_STORAGE_BACKEND")) {
            eprintln!(
                "navigator: {what}: --bucket names an object store, but \
                 NAVIGATOR_STORAGE_BACKEND is `fs` (which writes to a local \
                 root, not a named bucket) — set it to `gcs` or `s3`"
            );
            return ExitCode::from(2);
        }
        let storage = match cloud::assets_from_lookup(|key| assets_lookup(bucket, &env, key)).await
        {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "navigator: {what}: open assets storage: {e} — pass --bucket or set \
                     NAVIGATOR_ASSETS_BUCKET (or NAVIGATOR_STORAGE_BUCKET)"
                );
                return ExitCode::from(2);
            }
        };
        match run(storage).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("navigator: {what}: {e:#}");
                ExitCode::from(2)
            }
        }
    })
}

/// The tracked `.fields` manifest for a re-authored form: the sibling
/// file beside the working-copy path when readable (it wins for the
/// same reason the pin file does), else the manifest compiled into
/// `forms`. `None` for a form that still fills through a
/// `.fields.toml`.
fn tracked_manifest(item: &SyncItem) -> Option<Vec<String>> {
    match std::fs::read_to_string(item.local_blank.with_extension("fields")) {
        Ok(raw) => Some(
            raw.lines()
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect(),
        ),
        Err(_) => {
            forms::manifest(&item.code).map(|names| names.into_iter().map(str::to_string).collect())
        }
    }
}

/// For a re-authored form, the blank's own `/T` names must equal the
/// tracked `.fields` manifest — the fill path resolves fields from the
/// manifest, so a divergence (bytes regressed to a pre-re-author blank,
/// a coordinated re-vendor that forgot the manifest, a hand-corrupted
/// manifest line) means names that silently never fill. The pin alone
/// cannot catch the cases where it was rewritten alongside the bytes.
/// Both sides compare sorted: the manifest is a name set, so line order
/// carries no meaning.
fn verify_manifest(item: &SyncItem, bytes: &[u8]) -> anyhow::Result<()> {
    let Some(mut tracked) = tracked_manifest(item) else {
        return Ok(());
    };
    tracked.sort();
    let mut derived = pdf::field_names(bytes)?;
    derived.sort();
    if derived != tracked {
        let missing: Vec<&String> = tracked.iter().filter(|n| !derived.contains(n)).collect();
        let unexpected: Vec<&String> = derived.iter().filter(|n| !tracked.contains(n)).collect();
        anyhow::bail!(
            "{}: the blank's own `/T` names diverge from the tracked `.fields` manifest \
             ({} in the bytes vs {} tracked; first missing from the bytes: {:?}; first \
             absent from the manifest: {:?}) — re-run `navigator template forms re-author {}` so \
             the manifest mirrors the exact bytes",
            item.code,
            derived.len(),
            tracked.len(),
            missing.iter().take(5).collect::<Vec<_>>(),
            unexpected.iter().take(5).collect::<Vec<_>>(),
            item.code,
        );
    }
    Ok(())
}

/// Sync one form. With a local working copy: upload when the bucket
/// bytes differ (or are absent) and rewrite the pin file when it does
/// not match the working copy. Without one: pull + verify against the
/// pin, erroring loudly on a missing object or a mismatch. In both
/// directions a re-authored form's bytes must also match its tracked
/// `.fields` manifest ([`verify_manifest`]).
async fn sync_one(storage: &dyn StorageService, item: &SyncItem) -> anyhow::Result<SyncOutcome> {
    if item.local_blank.is_file() {
        let bytes = std::fs::read(&item.local_blank)?;
        verify_manifest(item, &bytes)?;
        let digest = forms::sha256_hex(&bytes);
        let bucket_matches = match storage.get(&item.object_path).await {
            Ok(existing) => forms::sha256_hex(&existing.bytes) == digest,
            Err(cloud::StorageError::NotFound(_)) => false,
            Err(e) => return Err(e.into()),
        };
        if !bucket_matches {
            storage
                .put_cached(
                    &item.object_path,
                    &bytes,
                    "application/pdf",
                    FORM_CACHE_CONTROL,
                )
                .await?;
        }
        let pin_on_disk = std::fs::read_to_string(&item.pin_file)
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let pin_rewritten = pin_on_disk != digest;
        if pin_rewritten {
            std::fs::write(&item.pin_file, format!("{digest}\n"))?;
        }
        return Ok(SyncOutcome::Vendored { pin_rewritten });
    }

    // No working copy: verify the bucket against the pin.
    let pinned = std::fs::read_to_string(&item.pin_file)
        .map_or_else(|_| item.pinned.clone(), |s| s.trim().to_string());
    let blank = storage.get(&item.object_path).await.map_err(|e| {
        anyhow::anyhow!(
            "{}: no working copy at {} and the bucket pull failed: {e} — \
             download the blank from the form's origin_url and re-run",
            item.code,
            item.local_blank.display()
        )
    })?;
    forms::verify_sha256(&pinned, &blank.bytes).map_err(|e| {
        anyhow::anyhow!(
            "{}: bucket object `{}` fails its pin: {e} — the blank was \
             re-vendored without updating {}; the fill path will refuse it",
            item.code,
            item.object_path,
            item.pin_file.display()
        )
    })?;
    verify_manifest(item, &blank.bytes)?;
    Ok(SyncOutcome::Verified)
}

#[cfg(test)]
mod tests {
    use super::{
        assets_lookup, bucket_without_object_store, load_reauthor_map, reauthor_bytes, sync_one,
        SyncItem, SyncOutcome,
    };
    use cloud::StorageService;

    fn with_static_xfa(blank: &[u8]) -> Vec<u8> {
        let mut doc = lopdf::Document::load_mem(blank).expect("fixture parses");
        let root = doc
            .trailer
            .get(b"Root")
            .expect("root")
            .as_reference()
            .expect("root reference");
        let acroform_id = doc
            .get_object(root)
            .expect("catalog")
            .as_dict()
            .expect("catalog dict")
            .get(b"AcroForm")
            .expect("acroform")
            .as_reference()
            .expect("acroform reference");
        if let Some(lopdf::Object::Dictionary(acroform)) = doc.objects.get_mut(&acroform_id) {
            acroform.set("XFA", lopdf::Object::Array(vec![]));
        }
        let mut out = Vec::new();
        doc.save_to(&mut out).expect("save static XFA fixture");
        out
    }

    #[test]
    fn reauthoring_the_same_input_twice_is_byte_identical() {
        // The `.sha256` pin is whatever bytes a re-author run produced,
        // so the transform must be reproducible from the original blank
        // — proven live on 2026-07-03 (two independent runs regenerated
        // the committed LLC pin exactly); this pins it in CI across
        // every primitive the transform composes: rename, multi-widget
        // merge, radio merge, pre-printed literal, unmapped namespace.
        let blank = with_static_xfa(&pdf::blank_acroform_with(&[
            pdf::FieldSpec::Text {
                name: "1 Name of Entity".into(),
            },
            pdf::FieldSpec::Text {
                name: "Name3".into(),
            },
            pdf::FieldSpec::Text {
                name: "organizer name".into(),
            },
            pdf::FieldSpec::Checkbox {
                name: "managers_a".into(),
                on_state: "managers".into(),
            },
            pdf::FieldSpec::Checkbox {
                name: "managers_b".into(),
                on_state: "members".into(),
            },
            pdf::FieldSpec::Text {
                name: "formation_1".into(),
            },
            pdf::FieldSpec::Text {
                name: "City".into(),
            },
        ]));
        let map: forms::FieldMap = toml::from_str(
            r#"
            form_code = "nv__test"
            [[field]]
            name = "formation_1"
            literal = "NRS 86"
            [[field]]
            name = "1 Name of Entity"
            question = "entity__company.name"
            [[field]]
            name = "managers_a"
            question = "management_structure"
            checked_when = "managers"
            on_state = "managers"
            [[field]]
            name = "managers_b"
            question = "management_structure"
            checked_when = "members"
            on_state = "members"
            [[field]]
            name = "Name3"
            question = "managing_members"
            row = 0
            part = "name"
            [[field]]
            name = "organizer name"
            question = "managing_members"
            row = 0
            part = "name"
            "#,
        )
        .expect("fixture map parses");
        let states: Vec<String> = [
            "entity__company",
            "custom_single_choice__management_structure",
            "people__managing_members",
        ]
        .iter()
        .map(ToString::to_string)
        .collect();

        let first = reauthor_bytes(&map, &blank, &states).expect("first run");
        let second = reauthor_bytes(&map, &blank, &states).expect("second run");
        assert_eq!(
            forms::sha256_hex(&first),
            forms::sha256_hex(&second),
            "the transform must be byte-deterministic: the committed pin is reproducible"
        );
        // The derived manifest — what `run_reauthor` writes as the
        // tracked `.fields` — is identical too.
        let mut manifest = pdf::field_names(&first).expect("first manifest");
        manifest.sort();
        let mut again = pdf::field_names(&second).expect("second manifest");
        again.sort();
        assert_eq!(manifest, again);
        assert!(
            manifest.contains(&"unmapped__City".to_string()),
            "{manifest:?}"
        );
    }

    #[test]
    fn assets_lookup_prefers_flag_then_defers_to_env() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |key: &str| {
                vars.iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| (*v).to_string())
            }
        };
        // --bucket wins over the env value for the bucket key.
        let get = env(&[("NAVIGATOR_ASSETS_BUCKET", "assets-bucket")]);
        assert_eq!(
            assets_lookup(Some("flag-bucket"), &get, "NAVIGATOR_ASSETS_BUCKET"),
            Some("flag-bucket".to_string())
        );
        // A blank flag is not a bucket — fall through to the env value so
        // `cloud::assets_from_lookup` can still find (or miss) it.
        assert_eq!(
            assets_lookup(Some("  "), &get, "NAVIGATOR_ASSETS_BUCKET"),
            Some("assets-bucket".to_string())
        );
        // No flag: defer to the env for every key, including the backend
        // selector, so the resolved store matches whatever the deps run.
        let get = env(&[
            ("NAVIGATOR_ASSETS_BUCKET", "assets-bucket"),
            ("NAVIGATOR_STORAGE_BACKEND", "s3"),
        ]);
        assert_eq!(
            assets_lookup(None, &get, "NAVIGATOR_ASSETS_BUCKET"),
            Some("assets-bucket".to_string())
        );
        assert_eq!(
            assets_lookup(None, &get, "NAVIGATOR_STORAGE_BACKEND"),
            Some("s3".to_string())
        );
    }

    #[test]
    fn a_bucket_override_against_the_fs_backend_is_a_refused_contradiction() {
        // `--bucket` on an object store (gcs/s3) is honored.
        assert!(!bucket_without_object_store(
            Some("some-bucket"),
            Some("gcs".to_string())
        ));
        assert!(!bucket_without_object_store(
            Some("some-bucket"),
            Some("s3".to_string())
        ));
        // `--bucket` against an explicit `fs` would silently write to a local
        // root instead of the named bucket, so it is refused.
        assert!(bucket_without_object_store(
            Some("some-bucket"),
            Some("fs".to_string())
        ));
        // An unset or empty selector is not this guard's job — `cloud`'s
        // selector fails closed on it and names the variable (#618), which is
        // a better message than a `--bucket` complaint.
        assert!(!bucket_without_object_store(Some("some-bucket"), None));
        assert!(!bucket_without_object_store(
            Some("some-bucket"),
            Some(String::new())
        ));
        // No `--bucket`: the resolved backend stands, whatever it is.
        assert!(!bucket_without_object_store(None, None));
        assert!(!bucket_without_object_store(None, Some("fs".to_string())));
    }

    fn item(dir: &std::path::Path, with_blank: Option<&[u8]>, pinned: &str) -> SyncItem {
        let local_blank = dir.join("nv__test.pdf");
        if let Some(bytes) = with_blank {
            std::fs::write(&local_blank, bytes).unwrap();
        }
        SyncItem {
            code: "nv__test".into(),
            object_path: "forms/united_states/nevada/state/nv__test.pdf".into(),
            local_blank,
            pin_file: dir.join("nv__test.sha256"),
            pinned: pinned.into(),
        }
    }

    async fn fs_storage(tag: &str) -> cloud::FsStorage {
        cloud::FsStorage::new(std::env::temp_dir().join(format!(
            "navigator-forms-sync-{tag}-{}",
            uuid::Uuid::new_v4()
        )))
        .await
        .expect("temp FsStorage")
    }

    fn temp_repo(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "navigator-forms-sync-repo-{tag}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn a_working_copy_uploads_and_writes_the_pin_then_reverifies() {
        let storage = fs_storage("vendor").await;
        let dir = temp_repo("vendor");
        let blank = b"%PDF-1.5 working copy";
        let it = item(&dir, Some(blank), "");

        // First run vendors: uploads + writes the pin file.
        let outcome = sync_one(&storage, &it).await.unwrap();
        assert_eq!(
            outcome,
            SyncOutcome::Vendored {
                pin_rewritten: true
            }
        );
        let pin = std::fs::read_to_string(&it.pin_file).unwrap();
        assert_eq!(pin.trim(), forms::sha256_hex(blank));
        assert!(storage.exists(&it.object_path).await.unwrap());

        // Second run with the working copy still present: idempotent
        // (bucket bytes match, pin unchanged).
        let outcome = sync_one(&storage, &it).await.unwrap();
        assert_eq!(
            outcome,
            SyncOutcome::Vendored {
                pin_rewritten: false
            }
        );

        // Remove the working copy: the bucket verifies against the pin
        // file the first run wrote.
        std::fs::remove_file(&it.local_blank).unwrap();
        let outcome = sync_one(&storage, &it).await.unwrap();
        assert_eq!(outcome, SyncOutcome::Verified);
    }

    #[tokio::test]
    async fn a_missing_bucket_object_without_a_working_copy_fails_loudly() {
        let storage = fs_storage("missing").await;
        let dir = temp_repo("missing");
        let it = item(&dir, None, &forms::sha256_hex(b"whatever"));
        let err = sync_one(&storage, &it).await.unwrap_err();
        assert!(err.to_string().contains("no working copy"), "{err:#}");
    }

    #[tokio::test]
    async fn every_registered_production_form_requires_an_obtainable_pinned_blank() {
        // This is intentionally the real registry rather than `item()`'s
        // synthetic form. A registry entry is a production promise: without
        // a local working copy, sync must fail on both a missing object and
        // any object whose bytes do not match the committed pin.
        let storage = fs_storage("registered-production-forms").await;
        let dir = temp_repo("registered-production-forms");

        for form in forms::registry().expect("registry loads") {
            let it = SyncItem {
                code: form.code.to_string(),
                object_path: form.object_path.to_string(),
                local_blank: dir.join(format!("{}.pdf", form.code)),
                pin_file: dir.join(format!("{}.sha256", form.code)),
                pinned: form.pinned_sha256().to_string(),
            };

            let missing = sync_one(&storage, &it).await.unwrap_err();
            assert!(
                missing.to_string().contains(form.code),
                "missing registered blank must name {}: {missing:#}",
                form.code
            );

            storage
                .put(
                    &it.object_path,
                    b"not the committed blank",
                    "application/pdf",
                )
                .await
                .unwrap();
            let mismatched = sync_one(&storage, &it).await.unwrap_err();
            assert!(
                mismatched.to_string().contains("fails its pin"),
                "mismatched registered blank must fail its pin: {mismatched:#}"
            );
        }
    }

    #[tokio::test]
    async fn a_bucket_object_diverging_from_its_manifest_fails_verify_loudly() {
        // Pin verification alone cannot catch a coordinated re-vendor
        // that re-pinned pre-re-author bytes, or a hand-corrupted
        // manifest line — both leave `/T` names the fill path resolves
        // that the bytes do not carry.
        let storage = fs_storage("manifest-mismatch").await;
        let dir = temp_repo("manifest-mismatch");
        let blank = pdf::blank_acroform(&["entity__company.name", "unmapped__City"]);
        let it = item(&dir, None, &forms::sha256_hex(&blank));
        std::fs::write(
            it.local_blank.with_extension("fields"),
            "entity__company.name\nunmapped__Ghost\n",
        )
        .unwrap();
        storage
            .put(&it.object_path, &blank, "application/pdf")
            .await
            .unwrap();
        let err = sync_one(&storage, &it).await.unwrap_err();
        assert!(err.to_string().contains("manifest"), "{err:#}");
    }

    #[tokio::test]
    async fn a_bucket_object_matching_pin_and_manifest_verifies() {
        let storage = fs_storage("manifest-match").await;
        let dir = temp_repo("manifest-match");
        let blank = pdf::blank_acroform(&["entity__company.name", "unmapped__City"]);
        let it = item(&dir, None, &forms::sha256_hex(&blank));
        std::fs::write(
            it.local_blank.with_extension("fields"),
            "entity__company.name\nunmapped__City\n",
        )
        .unwrap();
        storage
            .put(&it.object_path, &blank, "application/pdf")
            .await
            .unwrap();
        let outcome = sync_one(&storage, &it).await.unwrap();
        assert_eq!(outcome, SyncOutcome::Verified);
    }

    #[tokio::test]
    async fn a_manifest_with_the_same_names_in_another_order_still_verifies() {
        // The fill path consumes the manifest as a name set; line order
        // carries no meaning, so a reordered-but-equal manifest must not
        // read as divergence.
        let storage = fs_storage("manifest-order").await;
        let dir = temp_repo("manifest-order");
        let blank = pdf::blank_acroform(&["entity__company.name", "unmapped__City"]);
        let it = item(&dir, None, &forms::sha256_hex(&blank));
        std::fs::write(
            it.local_blank.with_extension("fields"),
            "unmapped__City\nentity__company.name\n",
        )
        .unwrap();
        storage
            .put(&it.object_path, &blank, "application/pdf")
            .await
            .unwrap();
        let outcome = sync_one(&storage, &it).await.unwrap();
        assert_eq!(outcome, SyncOutcome::Verified);
    }

    #[tokio::test]
    async fn a_working_copy_diverging_from_its_manifest_is_refused_before_upload() {
        // The 2026-07-03 regression entered exactly here: a bulk
        // re-stage dropped the original (pre-re-author) blank at the
        // working-copy path while the tracked manifest still described
        // the re-authored layer.
        let storage = fs_storage("manifest-vendor").await;
        let dir = temp_repo("manifest-vendor");
        let blank = pdf::blank_acroform(&["1 Name of Entity"]);
        let it = item(&dir, Some(&blank), "");
        std::fs::write(
            it.local_blank.with_extension("fields"),
            "entity__company.name\n",
        )
        .unwrap();
        let err = sync_one(&storage, &it).await.unwrap_err();
        assert!(err.to_string().contains("manifest"), "{err:#}");
        assert!(
            !storage.exists(&it.object_path).await.unwrap(),
            "diverging bytes must never reach the bucket"
        );
        assert!(!it.pin_file.exists(), "and must never be pinned");
    }

    #[tokio::test]
    async fn a_pin_mismatch_fails_loudly_instead_of_repinning() {
        let storage = fs_storage("mismatch").await;
        let dir = temp_repo("mismatch");
        let it = item(&dir, None, &forms::sha256_hex(b"the pinned blank"));
        storage
            .put(&it.object_path, b"silently re-vendored", "application/pdf")
            .await
            .unwrap();
        let err = sync_one(&storage, &it).await.unwrap_err();
        assert!(err.to_string().contains("fails its pin"), "{err:#}");
        assert!(
            !it.pin_file.exists(),
            "verify-only mode must never rewrite the pin"
        );
    }

    #[test]
    fn load_reauthor_map_reads_and_parses_the_sibling_fields_toml() {
        let root = tempfile::tempdir().unwrap();
        let object_path = "forms/nv/state/x.pdf";
        let map_path = root
            .path()
            .join("templates")
            .join("notations")
            .join(object_path)
            .with_extension("fields.toml");
        std::fs::create_dir_all(map_path.parent().unwrap()).unwrap();
        std::fs::write(
            &map_path,
            "form_code = \"x\"\n[[field]]\nname = \"formation\"\nliteral = \"NRS 88A\"\n",
        )
        .unwrap();

        let map = load_reauthor_map(root.path(), object_path).expect("map loads");
        assert_eq!(map.form_code, "x");
        assert_eq!(map.field.len(), 1);
    }

    #[test]
    fn load_reauthor_map_missing_file_reports_already_reauthored() {
        let root = tempfile::tempdir().unwrap();
        let err = load_reauthor_map(root.path(), "forms/nv/state/x.pdf").unwrap_err();
        assert!(err.to_string().contains("already re-authored"), "{err:#}");
    }
}
