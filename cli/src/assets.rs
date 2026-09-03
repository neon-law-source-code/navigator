//! `cli assets build` — transcode curated source photos into the
//! responsive web variants consumed by the browser surface.
//!
//! The manifest ([`views::assets::GALLERY`]) and the width set
//! ([`views::assets::WIDTHS`]) are the single source of truth, shared
//! with the view layer — so adding a photo is a manifest edit, never a
//! change here. For each photo we decode the source JPEG once, then
//! emit every width as AVIF, lossy WebP, and JPEG (the three formats
//! the `<picture>` element negotiates, smallest first). Output lands
//! under `<out>/img/<slug>/<slug>-<width>w.<ext>`, which is exactly
//! what the `/public` mount (and, in production, the CDN bucket) serves.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Context;
use cloud::{GcsStorage, GcsStorageConfig, StorageService};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{ExtendedColorType, ImageEncoder};
use include_dir::{include_dir, Dir};
use rgb::FromSlice;
use views::assets::{GALLERY, WIDTHS};
use workflows::notify::{Notifier, SlackNotifier};

/// `Cache-Control` stamped on every uploaded variant: cacheable by any
/// shared cache for ~1 week. Crucially **not** `immutable` — the
/// variant URLs carry no cache-bust token (`views::assets` dropped
/// `?v=`), so `immutable` would turn "stale for a week" into "stale
/// forever." A bounded max-age means a re-`build` + re-`upload` is
/// picked up once the week elapses.
const ASSET_CACHE_CONTROL: &str = "public, max-age=604800";

/// The initial GORP Serif faces the web design system serves. The licensed
/// WOFF2 bytes are operator assets, uploaded to the public assets bucket and
/// deliberately never committed to this repository. That mattered when the tree
/// was private and matters more now that it is public: the Firm's GORP licence
/// covers the Firm's deployments, not redistribution to everyone who clones.
const GORP_FONT_FILES: [&str; 2] = ["GORPSerif-Regular.woff2", "GORPSerif-Bold.woff2"];
const GORP_FONT_PREFIX: &str = "fonts/gorp-serif";

/// Slide markdown is embedded in the release binary so `ops ship` can discover
/// every presentation/workshop `](img/…)` key without depending on an operator
/// checkout. The standalone CLI archive contains only the binary and licence
/// files.
static SLIDE_CONTENT: Dir<'static> =
    include_dir!("$CARGO_MANIFEST_DIR/../server/content/workshops");

/// The full GORP Serif desktop family (the licensed `.otf` faces from the
/// `TrashType` delivery), packaged as one ZIP. Firm workers download it from
/// `/app/team/fonts/gorp-serif.zip`. Unlike the public WOFF2 web faces, this key
/// lives in the *private* documents bucket so the authorization gate cannot be bypassed
/// by a direct object URL; like the WOFF2 bytes it is an operator asset and
/// never committed to this repository, whose licence does not extend to the
/// font.
const GORP_OTF_ZIP_KEY: &str = "fonts/gorp-serif/gorp-serif-otf.zip";

/// The six `.otf` weights the licensed GORP Serif desktop delivery ships — the
/// full installable family, not the two-weight WOFF2 web subset. The upload
/// refuses a directory missing any of these rather than overwriting the
/// canonical bundle with a partial family that a firm worker would download
/// expecting every weight.
const GORP_OTF_FILES: [&str; 6] = [
    "GORPSerif-Bold.otf",
    "GORPSerif-ExtraLight.otf",
    "GORPSerif-Light.otf",
    "GORPSerif-Medium.otf",
    "GORPSerif-Regular.otf",
    "GORPSerif-Semibold.otf",
];

/// JPEG quality (0–100). 82 is a good photographic sweet spot —
/// visually lossless at typical viewing sizes without bloating bytes.
const JPEG_QUALITY: u8 = 82;

/// WebP quality (0–100). WebP at 80 typically lands ~30% under the
/// equivalent JPEG with no visible difference.
const WEBP_QUALITY: f32 = 80.0;

/// AVIF quality (0–100). 70 is a sound web default — AVIF at this
/// quality typically lands ~20–30% under the equivalent WebP.
const AVIF_QUALITY: f32 = 70.0;

/// AVIF encoder speed (0 slowest/smallest – 10 fastest/largest). 6
/// keeps the whole gallery encode under a minute while staying near
/// the small-file end of the curve.
const AVIF_SPEED: u8 = 6;

/// Entry point for `cli assets build`. `only` narrows the run to the
/// named manifest slugs.
pub fn run_build(src: &Path, out: &Path, only: &[String]) -> ExitCode {
    let selected = match select(only) {
        Ok(selected) => selected,
        Err(e) => {
            eprintln!("navigator: assets build: {e:#}");
            return ExitCode::from(2);
        }
    };
    let count = selected.len();
    match build(src, out, &selected) {
        Ok(variants) => {
            println!(
                "navigator: built {variants} variant(s) for {count} photo(s) under {}",
                out.join("img").display(),
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("navigator: assets build: {e:#}");
            ExitCode::from(2)
        }
    }
}

/// Resolve `--only` slugs against the manifest.
///
/// Empty means the whole gallery. Naming a slug that is not in the
/// manifest is an error rather than an empty run: a typo'd `--only` that
/// silently built nothing would look exactly like a successful build,
/// and the missing variants would not surface until the page 404s.
fn select(only: &[String]) -> anyhow::Result<Vec<&'static views::assets::GalleryImage>> {
    if only.is_empty() {
        return Ok(GALLERY.iter().collect());
    }
    only.iter()
        .map(|slug| {
            GALLERY
                .iter()
                .find(|image| image.slug == slug)
                .with_context(|| {
                    let known = GALLERY
                        .iter()
                        .map(|image| image.slug)
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("`{slug}` is not in the manifest — known slugs: {known}")
                })
        })
        .collect()
}

fn build(
    src: &Path,
    out: &Path,
    selected: &[&'static views::assets::GalleryImage],
) -> anyhow::Result<usize> {
    let img_root = out.join("img");
    let mut variants = 0usize;
    for g in selected {
        let src_path = src.join(g.source);
        let decoded = image::open(&src_path)
            .with_context(|| format!("open source `{}`", src_path.display()))?;
        let (ow, oh) = (decoded.width(), decoded.height());
        anyhow::ensure!(
            ow > 0 && oh > 0,
            "source `{}` has zero dimension",
            src_path.display()
        );

        let dir = img_root.join(g.slug);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("create output dir `{}`", dir.display()))?;

        for &w in &WIDTHS {
            // Preserve the photo's native aspect; CSS `object-fit:cover`
            // crops to the display ratio box, so the stored variant is
            // never letterboxed or distorted here.
            let h = u32::try_from(u64::from(w) * u64::from(oh) / u64::from(ow))
                .unwrap_or(u32::MAX)
                .max(1);
            let rgb = decoded.resize_exact(w, h, FilterType::Lanczos3).to_rgb8();

            // ── JPEG (universal fallback) ──
            let jpg = dir.join(format!("{}-{w}w.jpg", g.slug));
            let file = std::fs::File::create(&jpg)
                .with_context(|| format!("create `{}`", jpg.display()))?;
            JpegEncoder::new_with_quality(std::io::BufWriter::new(file), JPEG_QUALITY)
                .write_image(rgb.as_raw(), w, h, ExtendedColorType::Rgb8)
                .with_context(|| format!("encode `{}`", jpg.display()))?;

            // ── WebP (smaller, modern browsers) ──
            let webp_path = dir.join(format!("{}-{w}w.webp", g.slug));
            let encoded = webp::Encoder::from_rgb(rgb.as_raw(), w, h).encode(WEBP_QUALITY);
            std::fs::write(&webp_path, &*encoded)
                .with_context(|| format!("write `{}`", webp_path.display()))?;

            // ── AVIF (smallest; the negotiated first choice) ──
            let avif_path = dir.join(format!("{}-{w}w.avif", g.slug));
            let avif = ravif::Encoder::new()
                .with_quality(AVIF_QUALITY)
                .with_speed(AVIF_SPEED)
                .encode_rgb(ravif::Img::new(
                    rgb.as_raw().as_rgb(),
                    w as usize,
                    h as usize,
                ))
                .with_context(|| format!("encode `{}`", avif_path.display()))?;
            std::fs::write(&avif_path, &avif.avif_file)
                .with_context(|| format!("write `{}`", avif_path.display()))?;

            variants += 3;
        }
        println!(
            "  {:<24} {ow}x{oh} → {} widths × (avif, webp, jpg)",
            g.slug,
            WIDTHS.len()
        );
    }
    Ok(variants)
}

/// Entry point for `cli assets upload`. `bucket` defaults to the
/// `NAVIGATOR_ASSETS_BUCKET` env var (the public `<project>-assets`
/// bucket, distinct from the app's documents bucket
/// `NAVIGATOR_DOCUMENTS_BUCKET`) so an upload can never accidentally
/// write photos into the documents bucket.
pub fn run_upload(dir: &Path, bucket: Option<String>) -> ExitCode {
    let bucket = match bucket.or_else(|| std::env::var("NAVIGATOR_ASSETS_BUCKET").ok()) {
        Some(b) if !b.trim().is_empty() => b,
        _ => {
            eprintln!(
                "navigator: assets upload: no bucket — pass --bucket or set NAVIGATOR_ASSETS_BUCKET"
            );
            return ExitCode::from(2);
        }
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("navigator: assets upload: tokio runtime: {e}");
            return ExitCode::from(2);
        }
    };
    runtime.block_on(async move {
        // This operator command targets real GCS through ADC.
        let cfg = GcsStorageConfig {
            bucket: bucket.clone(),
            endpoint: None,
        };
        let storage = match GcsStorage::new_from_config(cfg).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("navigator: assets upload: open bucket `{bucket}`: {e}");
                return ExitCode::from(2);
            }
        };
        match upload(&storage, dir).await {
            Ok(n) => {
                println!("navigator: uploaded {n} variant(s) to gs://{bucket}/img");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("navigator: assets upload: {e:#}");
                ExitCode::from(2)
            }
        }
    })
}

/// Entry point for `cli assets fonts upload`. Licensed font files use the
/// same public assets bucket as images, but a separate `fonts/gorp-serif/`
/// prefix so the private tree never carries proprietary WOFF2 bytes.
pub fn run_upload_fonts(dir: &Path, bucket: Option<String>) -> ExitCode {
    let bucket = match bucket.or_else(|| std::env::var("NAVIGATOR_ASSETS_BUCKET").ok()) {
        Some(b) if !b.trim().is_empty() => b,
        _ => {
            eprintln!(
                "navigator: assets fonts upload: no bucket — pass --bucket or set NAVIGATOR_ASSETS_BUCKET"
            );
            return ExitCode::from(2);
        }
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("navigator: assets fonts upload: tokio runtime: {e}");
            return ExitCode::from(2);
        }
    };
    runtime.block_on(async move {
        let storage = match GcsStorage::new_from_config(GcsStorageConfig {
            bucket: bucket.clone(),
            endpoint: None,
        })
        .await
        {
            Ok(storage) => storage,
            Err(e) => {
                eprintln!("navigator: assets fonts upload: open bucket `{bucket}`: {e}");
                return ExitCode::from(2);
            }
        };
        match upload_gorp_fonts(&storage, dir).await {
            Ok(n) => {
                println!(
                    "navigator: uploaded {n} GORP face(s) to gs://{bucket}/{GORP_FONT_PREFIX}"
                );
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("navigator: assets fonts upload: {e:#}");
                ExitCode::from(2)
            }
        }
    })
}

/// Entry point for `cli assets fonts upload-desktop`. Packages the licensed
/// GORP Serif `.otf` desktop family in `dir` into a single ZIP and uploads it
/// to `fonts/gorp-serif/gorp-serif-otf.zip`.
///
/// Unlike the WOFF2 web faces (`upload`), which live in the *public* assets
/// bucket because browsers fetch them auth-free, the installable desktop
/// family is a restricted download: it goes to the *private* documents bucket
/// (`NAVIGATOR_DOCUMENTS_BUCKET`), so the only way to the bytes is through the
/// policy-gated `/app/team/fonts/gorp-serif.zip` route — a predictable public URL
/// can never bypass authorization. ADC auth; the `.otf` source is never
/// committed.
pub fn run_upload_desktop_fonts(dir: &Path, bucket: Option<String>) -> ExitCode {
    let bucket = match bucket.or_else(|| std::env::var("NAVIGATOR_DOCUMENTS_BUCKET").ok()) {
        Some(b) if !b.trim().is_empty() => b,
        _ => {
            eprintln!(
                "navigator: assets fonts upload-desktop: no bucket — pass --bucket or set NAVIGATOR_DOCUMENTS_BUCKET"
            );
            return ExitCode::from(2);
        }
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("navigator: assets fonts upload-desktop: tokio runtime: {e}");
            return ExitCode::from(2);
        }
    };
    runtime.block_on(async move {
        let storage = match GcsStorage::new_from_config(GcsStorageConfig {
            bucket: bucket.clone(),
            endpoint: None,
        })
        .await
        {
            Ok(storage) => storage,
            Err(e) => {
                eprintln!("navigator: assets fonts upload-desktop: open bucket `{bucket}`: {e}");
                return ExitCode::from(2);
            }
        };
        match upload_gorp_otf_zip(&storage, dir).await {
            Ok(n) => {
                println!("navigator: packaged {n} OTF face(s) → gs://{bucket}/{GORP_OTF_ZIP_KEY}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("navigator: assets fonts upload-desktop: {e:#}");
                ExitCode::from(2)
            }
        }
    })
}

/// Entry point for `cli assets pull` — the inverse of `upload`, for
/// local development. `server/public/img/` is gitignored (photos live only
/// in the public assets bucket, never in git, never baked into the
/// image), so a fresh clone serves empty photo slots. This downloads
/// every built variant under the bucket's `img/` prefix into `out`
/// (default `server/public/img`) so the `/public` mount has the photos
/// again — no source JPEGs or a re-`build` required. Read-only against
/// the bucket; `bucket` defaults to `NAVIGATOR_ASSETS_BUCKET`.
pub fn run_pull(out: &Path, bucket: Option<String>) -> ExitCode {
    let bucket = match bucket.or_else(|| std::env::var("NAVIGATOR_ASSETS_BUCKET").ok()) {
        Some(b) if !b.trim().is_empty() => b,
        _ => {
            eprintln!(
                "navigator: assets pull: no bucket — pass --bucket or set NAVIGATOR_ASSETS_BUCKET"
            );
            return ExitCode::from(2);
        }
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("navigator: assets pull: tokio runtime: {e}");
            return ExitCode::from(2);
        }
    };
    runtime.block_on(async move {
        // Same endpoint override as `upload` (emulator support), pointed
        // at the assets bucket; ADC auth otherwise.
        let cfg = GcsStorageConfig {
            bucket: bucket.clone(),
            endpoint: std::env::var("NAVIGATOR_STORAGE_ENDPOINT")
                .ok()
                .filter(|s| !s.trim().is_empty()),
        };
        let storage = match GcsStorage::new_from_config(cfg).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("navigator: assets pull: open bucket `{bucket}`: {e}");
                return ExitCode::from(2);
            }
        };
        match download(&storage, out).await {
            Ok(n) => {
                println!(
                    "navigator: pulled {n} variant(s) from gs://{bucket}/img into {}",
                    out.display()
                );
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("navigator: assets pull: {e:#}");
                ExitCode::from(2)
            }
        }
    })
}

/// Report bucket objects nothing on the site reaches, optionally posting
/// the result to the ops Slack channel.
///
/// Deliberately **report-only**: it never deletes. The reachable set is a
/// union across two crates, and if it is ever wrong the failure mode of a
/// pruning tool is deleting live production photographs. A human reads
/// this and removes what they recognize.
pub fn run_orphans(content_root: &Path, bucket: Option<String>, slack: bool) -> ExitCode {
    let Some(bucket) = bucket
        .or_else(|| std::env::var("NAVIGATOR_ASSETS_BUCKET").ok())
        .filter(|b| !b.trim().is_empty())
    else {
        eprintln!(
            "navigator: assets orphans: no bucket — pass --bucket or set NAVIGATOR_ASSETS_BUCKET"
        );
        return ExitCode::from(2);
    };
    let reachable = match reachable_image_keys(content_root) {
        Ok(keys) => keys,
        Err(e) => {
            eprintln!("navigator: assets orphans: {e:#}");
            return ExitCode::from(2);
        }
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("navigator: assets orphans: tokio runtime: {e}");
            return ExitCode::from(2);
        }
    };
    runtime.block_on(async move {
        let cfg = GcsStorageConfig {
            bucket: bucket.clone(),
            endpoint: std::env::var("NAVIGATOR_STORAGE_ENDPOINT")
                .ok()
                .filter(|s| !s.trim().is_empty()),
        };
        let storage = match GcsStorage::new_from_config(cfg).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("navigator: assets orphans: open bucket `{bucket}`: {e}");
                return ExitCode::from(2);
            }
        };
        let listings = match storage.list("img/").await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("navigator: assets orphans: list `img/` in `{bucket}`: {e}");
                return ExitCode::from(2);
            }
        };
        let listed: Vec<String> = listings.into_iter().map(|l| l.key).collect();
        let orphans = orphan_keys(&listed, &reachable);
        let held = listed.len().saturating_sub(orphans.len());
        let report = orphan_report(&bucket, &orphans, held);
        println!("{report}");

        if slack {
            let Some(url) = std::env::var("SLACK_WEBHOOK_URL")
                .ok()
                .filter(|u| !u.trim().is_empty())
            else {
                eprintln!(
                    "navigator: assets orphans: --slack given but SLACK_WEBHOOK_URL is unset"
                );
                return ExitCode::from(2);
            };
            let notifier = SlackNotifier::new(url);
            if let Err(e) = notifier.notify(report).await {
                eprintln!("navigator: assets orphans: Slack delivery failed: {e}");
                return ExitCode::from(2);
            }
            println!("navigator: posted the report to Slack");
        }
        // Orphans are a finding to act on, never a build failure: the tree
        // is legitimately dirty between a deck edit and its cleanup.
        ExitCode::SUCCESS
    })
}

/// Every markdown image target under `content_root` that points at a
/// public image asset (`![alt](img/<slug>/<file>)`). These are the keys
/// a browser fetches from the public origin (`server/public/img/` locally,
/// `NAVIGATOR_ASSET_BASE_URL` in production) — and, because
/// `server/public/img/` is gitignored, the ones that silently 404 if the
/// object was never uploaded. Returned deduped and sorted so the
/// verify report is stable.
fn content_image_refs(content_root: &Path) -> anyhow::Result<BTreeSet<String>> {
    anyhow::ensure!(
        content_root.is_dir(),
        "content directory `{}` does not exist",
        content_root.display()
    );
    let mut refs = BTreeSet::new();
    for entry in walkdir::WalkDir::new(content_root).follow_links(false) {
        let entry = entry.with_context(|| format!("walk `{}`", content_root.display()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read `{}`", path.display()))?;
        for r in parse_image_refs(&text) {
            refs.insert(r);
        }
    }
    Ok(refs)
}

fn embedded_content_image_refs(dir: &Dir<'_>, refs: &mut BTreeSet<String>) {
    for file in dir.files() {
        if file.path().extension().and_then(|ext| ext.to_str()) == Some("md") {
            if let Some(markdown) = file.contents_utf8() {
                refs.extend(parse_image_refs(markdown));
            }
        }
    }
    for child in dir.dirs() {
        embedded_content_image_refs(child, refs);
    }
}

/// Every bucket-lane image or video a released presentation/workshop can
/// request, carried inside the standalone CLI that performs a ship.
fn bundled_slide_asset_keys() -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    embedded_content_image_refs(&SLIDE_CONTENT, &mut keys);
    keys
}

/// Every bucket key under `img/` the site can actually reach.
///
/// Two independent sources, and the union matters more than either half.
/// Markdown content contributes `](img/…)` references. The
/// `views::assets::GALLERY` manifest contributes the responsive variants
/// its photos are served as — those are referenced from **Rust views**
/// through `responsive_picture`, never from markdown, so a sweep of the
/// content tree alone sees none of them. Reporting on the markdown half
/// by itself would name every production photograph as unreferenced.
fn reachable_image_keys(content_root: &Path) -> anyhow::Result<BTreeSet<String>> {
    let mut keys = content_image_refs(content_root)?;
    keys.extend(gallery_variant_keys());
    Ok(keys)
}

/// Every public asset key the site loads and therefore every key `verify`
/// must find published: the reachable `img/` set plus the licensed faces.
///
/// Built on [`reachable_image_keys`] rather than on [`content_image_refs`]
/// directly, so `verify` and `orphan` cannot disagree about what the site
/// reaches. They are the two halves of one reconciliation — `orphan` names
/// keys the site does not use, `verify` names keys the site uses and the
/// origin does not serve — and a photo that enters one definition without
/// the other goes unwatched by exactly one of them.
///
/// # Errors
///
/// Propagates a missing or unwalkable `content_root` from
/// [`content_image_refs`].
fn published_asset_refs(content_root: &Path) -> anyhow::Result<BTreeSet<String>> {
    let mut refs = reachable_image_keys(content_root)?;
    // The licensed faces are published by `assets fonts upload`, never by a
    // build step, so they are exactly as droppable as an unuploaded hero.
    refs.extend(gorp_font_refs());
    Ok(refs)
}

/// Every key `assets build` emits for the manifest: one per photo, per
/// width, per format. Mirrors `upload`'s keying so the strings compare
/// equal to what the bucket actually holds.
fn gallery_variant_keys() -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for image in GALLERY {
        for width in WIDTHS {
            for ext in ["avif", "webp", "jpg"] {
                keys.insert(format!(
                    "img/{slug}/{slug}-{width}w.{ext}",
                    slug = image.slug
                ));
            }
        }
    }
    keys
}

/// Bucket keys that nothing on the site reaches, given everything it does.
///
/// Scoped to the `img/` prefix — the only lane `upload` writes — so the
/// separately managed `fonts/` objects are never reported and never at
/// risk. Sorted for a stable report.
fn orphan_keys(listed: &[String], reachable: &BTreeSet<String>) -> Vec<String> {
    let mut orphans: Vec<String> = listed
        .iter()
        .filter(|key| key.starts_with("img/"))
        .filter(|key| !reachable.contains(key.as_str()))
        .cloned()
        .collect();
    orphans.sort();
    orphans.dedup();
    orphans
}

/// The report body, as posted to Slack and printed to stdout.
///
/// Names only bucket object keys, which are already public URLs on the
/// asset origin — so this crosses no trust boundary the bucket itself did
/// not already cross. It carries no client, matter, or personal data,
/// which is what lets an ops channel receive it at all.
fn orphan_report(bucket: &str, orphans: &[String], held: usize) -> String {
    if orphans.is_empty() {
        return format!(
            "Assets: no orphans in `{bucket}` — every object under `img/` is reachable \
             from content or the gallery manifest ({held} held)."
        );
    }
    let mut out = format!(
        "Assets: {} unreachable object(s) in `{bucket}` — published but referenced by \
         nothing on the site ({held} held):\n",
        orphans.len()
    );
    for key in orphans {
        let _ = writeln!(out, "• {key}");
    }
    out.push_str(
        "`upload` never deletes, so these stay publicly fetchable until removed by hand. \
         Review before deleting: this reports, it does not prune.",
    );
    out
}

/// Pull every `img/…` target out of a markdown body. Matches the
/// `CommonMark` image tail `](img/…)` — the same `img/<slug>/<file>` form
/// the blog/marketing loaders route through `views::assets::asset_url`
/// — and stops at the first `)`, whitespace, or title quote so an
/// optional `"title"` or trailing prose never leaks into the key. Split
/// from [`content_image_refs`] so the parsing is unit-tested without a
/// filesystem.
fn parse_image_refs(markdown: &str) -> Vec<String> {
    const NEEDLE: &str = "](img/";
    let mut refs = Vec::new();
    let mut cursor = 0;
    while let Some(hit) = markdown[cursor..].find(NEEDLE) {
        // Target starts right after the `](`, i.e. at the `img/`.
        let start = cursor + hit + "](".len();
        let rest = &markdown[start..];
        let end = rest
            .find(|c: char| c == ')' || c == '"' || c.is_whitespace())
            .unwrap_or(rest.len());
        if end > 0 {
            refs.push(rest[..end].to_string());
        }
        cursor = start + end;
    }
    refs
}

/// The public asset keys the design system loads from Rust rather than from
/// markdown: the licensed GORP faces `views::layout` preloads on every page.
/// [`parse_image_refs`] only ever sees `](img/…)` in content, so without these
/// the gate reports success while every page silently falls back to Georgia —
/// `font-display: swap` means a missing face degrades quietly rather than
/// erroring, so nothing else catches it.
fn gorp_font_refs() -> impl Iterator<Item = String> {
    GORP_FONT_FILES
        .into_iter()
        .map(|file| format!("{GORP_FONT_PREFIX}/{file}"))
}

/// Join a public asset base URL with a repo-relative `img/…` key, the
/// same way [`views::assets::asset_url`] does (trim one interior slash),
/// so `verify` fetches exactly the URL the rendered page would.
fn join_public_url(base: &str, rel: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        rel.trim_start_matches('/')
    )
}

/// Existence probe for one public asset URL. `Ok(true)` = served,
/// `Ok(false)` = a definitive `404` (the object was never uploaded),
/// `Err` = the answer is unknown (transport failure, or a status that is
/// neither success nor `404`) — kept distinct so a flaky network reads
/// as "couldn't verify," never as "missing." Uses `HEAD`, falling back
/// to a single-byte ranged `GET` for an origin that rejects `HEAD`.
async fn asset_exists(client: &reqwest::Client, url: &str) -> anyhow::Result<bool> {
    let head = client
        .head(url)
        .send()
        .await
        .with_context(|| format!("HEAD {url}"))?;
    let status = head.status();
    if status.is_success() {
        return Ok(true);
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    }
    if status == reqwest::StatusCode::METHOD_NOT_ALLOWED {
        let got = client
            .get(url)
            .header(reqwest::header::RANGE, "bytes=0-0")
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        if got.status().is_success() {
            return Ok(true);
        }
        if got.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        anyhow::bail!("unexpected status {} for GET {url}", got.status());
    }
    anyhow::bail!("unexpected status {status} for HEAD {url}");
}

/// "Does this public URL serve a live object?" behind a trait so the
/// reconcile loop is exercised in unit tests against a fake, while the
/// real run drives [`HttpProbe`] (an auth-free HTTP `HEAD`).
#[async_trait::async_trait]
trait AssetProbe: Sync {
    async fn exists(&self, url: &str) -> anyhow::Result<bool>;
}

/// The production probe: a live `HEAD`/`GET` via [`asset_exists`].
struct HttpProbe {
    client: reqwest::Client,
}

#[async_trait::async_trait]
impl AssetProbe for HttpProbe {
    async fn exists(&self, url: &str) -> anyhow::Result<bool> {
        asset_exists(&self.client, url).await
    }
}

/// Outcome of reconciling the content references against the origin:
/// how many were checked, which are definitively missing (`404`), and
/// which could not be checked (transport failure / unexpected status).
struct VerifyReport {
    checked: usize,
    missing: Vec<String>,
    unknown: Vec<String>,
}

/// Probe every reference against `base_url` and bucket the results.
/// Decoupled from the HTTP client and the process so a fake `probe`
/// drives it deterministically in tests; the join is applied here so a
/// test also proves the URL the origin is asked for.
async fn verify_refs(
    probe: &dyn AssetProbe,
    base_url: &str,
    refs: &BTreeSet<String>,
) -> VerifyReport {
    let mut missing = Vec::new();
    let mut unknown = Vec::new();
    for rel in refs {
        let url = join_public_url(base_url, rel);
        match probe.exists(&url).await {
            Ok(true) => {}
            Ok(false) => missing.push(rel.clone()),
            Err(e) => unknown.push(format!("{rel}: {e:#}")),
        }
    }
    VerifyReport {
        checked: refs.len(),
        missing,
        unknown,
    }
}

/// Reconcile the complete slide-media key set against a storage backend.
/// Direct bucket metadata is the deploy preflight: it checks the selected
/// deployment's destination before any rollout mutation and does not depend
/// on which release the public hostname currently serves.
async fn verify_storage_refs(
    storage: &dyn StorageService,
    refs: &BTreeSet<String>,
) -> VerifyReport {
    let mut missing = Vec::new();
    let mut unknown = Vec::new();
    for rel in refs {
        match storage.exists(rel).await {
            Ok(true) => {}
            Ok(false) => missing.push(rel.clone()),
            Err(e) => unknown.push(format!("{rel}: {e}")),
        }
    }
    VerifyReport {
        checked: refs.len(),
        missing,
        unknown,
    }
}

fn storage_report_result(report: &VerifyReport, bucket: &str) -> anyhow::Result<()> {
    if report.missing.is_empty() && report.unknown.is_empty() {
        eprintln!(
            "==> asset preflight: all {} public asset(s) exist in gs://{bucket}",
            report.checked
        );
        return Ok(());
    }

    let mut detail = String::new();
    for key in &report.missing {
        let _ = writeln!(detail, "\n  missing: gs://{bucket}/{key}");
    }
    for line in &report.unknown {
        let _ = writeln!(detail, "\n  unknown: gs://{bucket}/{line}");
    }
    anyhow::bail!(
        "slide asset preflight failed for gs://{bucket}: {} missing, {} could not be checked.\
         {detail}\nPublish every presentation/workshop asset to this deployment's bucket before shipping.",
        report.missing.len(),
        report.unknown.len()
    )
}

/// The preflight decision, against any storage backend. Split from the GCS
/// client construction below so the whole judgement — a pass, a named
/// missing key, an unreadable bucket — is proven against a filesystem
/// backend and a failing fake instead of a real deployment's bucket.
async fn verify_bundled_slide_assets(
    storage: &dyn StorageService,
    bucket: &str,
) -> anyhow::Result<()> {
    let refs = bundled_slide_asset_keys();
    let report = verify_storage_refs(storage, &refs).await;
    storage_report_result(&report, bucket)
}

/// Refuse an image or full roll unless every presentation/workshop object
/// bundled into this CLI exists in the selected deployment's GCS assets
/// bucket. Restart-only ships do not call this because they change no content
/// or image version.
pub(crate) fn verify_bundled_slide_assets_bucket(bucket: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!bucket.trim().is_empty(), "assets bucket is blank");
    let runtime = tokio::runtime::Runtime::new().context("create asset preflight runtime")?;
    runtime.block_on(async {
        let storage = GcsStorage::new_from_config(GcsStorageConfig {
            bucket: bucket.to_string(),
            endpoint: None,
        })
        .await
        .with_context(|| format!("open public assets bucket `gs://{bucket}`"))?;
        verify_bundled_slide_assets(&storage, bucket).await
    })
}

/// Render a [`VerifyReport`] and return its process exit code: `0` only
/// when nothing is missing and nothing was unknown, else `2` with the
/// offending URLs printed. Returns the raw code (not `ExitCode`, which is
/// opaque) so the decision is unit-tested without a network.
fn report_exit(report: &VerifyReport, base_url: &str) -> u8 {
    if report.missing.is_empty() && report.unknown.is_empty() {
        println!(
            "navigator: assets verify: all {} public asset(s) are published at {base_url}",
            report.checked
        );
        return 0;
    }
    if !report.missing.is_empty() {
        eprintln!(
            "navigator: assets verify: {} public asset(s) are NOT published at {base_url} \
             (run `cli assets upload` for an image, `cli assets fonts upload` for a font):",
            report.missing.len()
        );
        for rel in &report.missing {
            eprintln!("  ✗ {}", join_public_url(base_url, rel));
        }
    }
    if !report.unknown.is_empty() {
        eprintln!(
            "navigator: assets verify: {} public asset(s) could not be checked:",
            report.unknown.len()
        );
        for line in &report.unknown {
            eprintln!("  ? {line}");
        }
    }
    2
}

/// Resolve the public origin for verify/fetch: explicit flag/env, or
/// fail with a command-specific message when unset/blank.
fn resolve_public_origin(base_url: Option<String>, command: &str) -> Result<String, u8> {
    match base_url.or_else(|| std::env::var("NAVIGATOR_ASSET_BASE_URL").ok()) {
        Some(b) if !b.trim().is_empty() => Ok(b),
        _ => {
            eprintln!(
                "navigator: assets {command}: no public origin — pass --base-url or set \
                 NAVIGATOR_ASSET_BASE_URL"
            );
            Err(2)
        }
    }
}

/// The whole verify flow as an async function returning the raw exit
/// code (`0`/`2`): resolve the origin, gather the content references,
/// then probe each over HTTP. Split from [`run_verify`] (which only owns
/// the tokio runtime) so every branch — no origin, walk error, empty
/// tree, and the live probe against a real origin — is unit-tested with
/// a `wiremock` server, no network needed.
async fn verify_content(content_dir: &Path, base_url: Option<String>) -> u8 {
    let base_url = match resolve_public_origin(base_url, "verify") {
        Ok(b) => b,
        Err(code) => return code,
    };
    let refs = match published_asset_refs(content_dir) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("navigator: assets verify: {e:#}");
            return 2;
        }
    };
    let client = match reqwest::Client::builder().build() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("navigator: assets verify: http client: {e}");
            return 2;
        }
    };
    let probe = HttpProbe { client };
    let report = verify_refs(&probe, &base_url, &refs).await;
    report_exit(&report, &base_url)
}

/// Entry point for `cli assets verify` — reconcile everything the site
/// loads from the public origin against what is actually published there.
/// That is every `![](img/…)` reference under `content_dir`, every
/// `views::assets::GALLERY` variant the Rust views render, and the
/// licensed GORP faces, each of which must resolve to a live object at
/// `base_url` (default `NAVIGATOR_ASSET_BASE_URL`). Neither is built or
/// uploaded by CI — `server/public/img/` is gitignored and the WOFF2 bytes are
/// operator assets — so a deploy can otherwise ship a hero or a typeface
/// that 404s; this is the gate that catches it. Auth-free — it fetches the
/// public URL exactly as a browser would, so it works against any CDN
/// origin, not just GCS. Exit `2` if any reference is missing or could not
/// be checked.
pub fn run_verify(content_dir: &Path, base_url: Option<String>) -> ExitCode {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("navigator: assets verify: tokio runtime: {e}");
            return ExitCode::from(2);
        }
    };
    ExitCode::from(runtime.block_on(verify_content(content_dir, base_url)))
}

/// Outcome of downloading every content-referenced `img/…` object from a
/// public HTTP origin into a local tree (for baking into the KIND image).
struct FetchReport {
    fetched: usize,
    missing: Vec<String>,
    errored: Vec<String>,
}

/// Render a [`FetchReport`] and return its process exit code.
fn fetch_report_exit(report: &FetchReport, base_url: &str, out: &Path) -> u8 {
    if report.missing.is_empty() && report.errored.is_empty() {
        println!(
            "navigator: assets fetch-referenced: fetched {} content image(s) from {base_url} into {}",
            report.fetched,
            out.display()
        );
        return 0;
    }
    if !report.missing.is_empty() {
        eprintln!(
            "navigator: assets fetch-referenced: {} content image(s) are NOT published at \
             {base_url} (run `cli assets upload`):",
            report.missing.len()
        );
        for rel in &report.missing {
            eprintln!("  ✗ {}", join_public_url(base_url, rel));
        }
    }
    if !report.errored.is_empty() {
        eprintln!(
            "navigator: assets fetch-referenced: {} content image(s) could not be downloaded:",
            report.errored.len()
        );
        for line in &report.errored {
            eprintln!("  ? {line}");
        }
    }
    2
}

/// Download one public asset URL into `dest`. `Ok(true)` when bytes were
/// written, `Ok(false)` on a definitive `404`, `Err` on transport or
/// unexpected status.
async fn fetch_asset(client: &reqwest::Client, url: &str, dest: &Path) -> anyhow::Result<bool> {
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    }
    if !status.is_success() {
        anyhow::bail!("unexpected status {status} for GET {url}");
    }
    let bytes = resp
        .bytes()
        .await
        .with_context(|| format!("read body from GET {url}"))?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create `{}`", parent.display()))?;
    }
    std::fs::write(dest, &bytes).with_context(|| format!("write `{}`", dest.display()))?;
    Ok(true)
}

/// Rebuild a destination from a public `img/…` reference without letting
/// malformed content escape the chosen output root.
fn destination_for_ref(out: &Path, rel: &str) -> anyhow::Result<PathBuf> {
    let mut dest = out.to_path_buf();
    for seg in rel.split('/') {
        anyhow::ensure!(
            !seg.is_empty() && seg != "." && seg != "..",
            "refusing unsafe asset reference `{rel}`"
        );
        dest.push(seg);
    }
    Ok(dest)
}

/// Download every content-referenced `img/…` object from `base_url` into
/// `out` (e.g. `server/public` so `img/slug/file.png` lands at
/// `server/public/img/slug/file.png`). Auth-free HTTP — the same public
/// origin `verify` probes, but this writes the bytes for the KIND image
/// bake instead of only checking existence.
async fn fetch_referenced_content(content_dir: &Path, base_url: Option<String>, out: &Path) -> u8 {
    let base_url = match resolve_public_origin(base_url, "fetch-referenced") {
        Ok(b) => b,
        Err(code) => return code,
    };
    let refs = match content_image_refs(content_dir) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("navigator: assets fetch-referenced: {e:#}");
            return 2;
        }
    };
    if refs.is_empty() {
        println!(
            "navigator: assets fetch-referenced: no content image references under {}",
            content_dir.display()
        );
        return 0;
    }
    let client = match reqwest::Client::builder().build() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("navigator: assets fetch-referenced: http client: {e}");
            return 2;
        }
    };
    let mut report = FetchReport {
        fetched: 0,
        missing: Vec::new(),
        errored: Vec::new(),
    };
    for rel in &refs {
        let url = join_public_url(&base_url, rel);
        let dest = match destination_for_ref(out, rel) {
            Ok(path) => path,
            Err(e) => {
                report.errored.push(format!("{rel}: {e:#}"));
                continue;
            }
        };
        match fetch_asset(&client, &url, &dest).await {
            Ok(true) => report.fetched += 1,
            Ok(false) => report.missing.push(rel.clone()),
            Err(e) => report.errored.push(format!("{rel}: {e:#}")),
        }
    }
    fetch_report_exit(&report, &base_url, out)
}

/// Entry point for `cli assets fetch-referenced` — hydrate the gitignored
/// `server/public/img/` tree from a public HTTP origin so the `navigator-web`
/// Docker image can serve content heroes in KIND. No GCP ADC: the origin
/// is whatever `verify` would probe (`NAVIGATOR_ASSET_BASE_URL` or
/// `--base-url`). Exit `2` if any reference is missing or could not be
/// downloaded.
pub fn run_fetch_referenced(content_dir: &Path, out: &Path, base_url: Option<String>) -> ExitCode {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("navigator: assets fetch-referenced: tokio runtime: {e}");
            return ExitCode::from(2);
        }
    };
    ExitCode::from(runtime.block_on(fetch_referenced_content(content_dir, base_url, out)))
}

const STUB_PNG: &[u8] = &[
    0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, b'I', b'H', b'D', b'R',
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0a, b'I', b'D', b'A', b'T', 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, b'I', b'E', b'N', b'D', 0xae,
    0x42, 0x60, 0x82,
];

/// A minimal but real WOFF2: one empty `.notdef` glyph and only the tables
/// a browser requires to parse the face. Mirrors [`STUB_PNG`] — the KIND
/// `/public` mount must serve decodable bytes at the verified key, not a
/// placeholder that merely answers a HEAD.
const STUB_WOFF2: &[u8] = &[
    b'w', b'O', b'F', b'2', 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x01, 0x2c, 0x00, 0x0a, 0x00, 0x00,
    0x00, 0x00, 0x02, 0xac, 0x00, 0x00, 0x00, 0xe3, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x06, 0x60, 0x00, 0x2c, 0x0a, 0x01, 0x2a, 0x01, 0x36, 0x02, 0x24, 0x03, 0x04, 0x0b, 0x04, 0x00,
    0x04, 0x20, 0x05, 0x81, 0x4c, 0x07, 0x20, 0x1b, 0x1f, 0x02, 0x00, 0x1e, 0x85, 0x71, 0xc3, 0xdd,
    0x62, 0x7d, 0x19, 0x3f, 0x44, 0x39, 0x1b, 0x7e, 0x89, 0x21, 0x82, 0xaf, 0xbd, 0x6e, 0xdf, 0xdd,
    0x4f, 0x69, 0xc5, 0x26, 0xc6, 0xe0, 0x90, 0x74, 0x27, 0x31, 0x02, 0x2b, 0x19, 0x94, 0x60, 0xe2,
    0xe8, 0x96, 0x53, 0xb7, 0x5d, 0xc4, 0xd2, 0x0d, 0xe5, 0xa9, 0x1a, 0x94, 0x2e, 0x3b, 0xc8, 0x11,
    0x04, 0x41, 0x76, 0x1b, 0x01, 0x87, 0x47, 0x95, 0xe5, 0x8e, 0x7f, 0xb7, 0xfd, 0x83, 0xbc, 0x85,
    0x2d, 0xe0, 0xd8, 0x93, 0x87, 0x96, 0x48, 0x5b, 0x9c, 0x48, 0x14, 0xa7, 0x1c, 0x58, 0xa6, 0x91,
    0x24, 0x94, 0xde, 0xb0, 0x95, 0xf0, 0x28, 0xa5, 0x39, 0x76, 0x3b, 0xdc, 0x52, 0x8e, 0xdf, 0x78,
    0x51, 0x54, 0x92, 0xae, 0xa0, 0xe6, 0x63, 0xb1, 0x22, 0xaf, 0x00, 0x8e, 0xbf, 0x49, 0xc8, 0x98,
    0xca, 0x02, 0x53, 0x95, 0x25, 0x92, 0x04, 0xa5, 0x50, 0x8a, 0x00, 0x04, 0x48, 0x00, 0x00, 0xa8,
    0x00, 0x10, 0x08, 0x4e, 0xff, 0xbf, 0xcd, 0x41, 0x7f, 0xe4, 0x7c, 0x81, 0xf7, 0xe7, 0xcb, 0x47,
    0x6c, 0x01, 0x59, 0x02, 0xa1, 0xd7, 0xc5, 0x2f, 0x12, 0x90, 0x83, 0xe5, 0x18, 0xd3, 0xe3, 0xeb,
    0x6e, 0x24, 0x0d, 0xfd, 0x63, 0x24, 0x0d, 0x7d, 0x01, 0xea, 0x66, 0x66, 0xb2, 0xa8, 0x5a, 0xc2,
    0xd6, 0x79, 0x0e, 0x49, 0xdf, 0x75, 0xaf, 0x3b, 0x7b, 0xba, 0xc3, 0xb5, 0xc1, 0xb0, 0xdf, 0x51,
    0x46, 0x24, 0x2b, 0x8a, 0x77, 0x49, 0x03, 0xdf, 0xe6, 0x80, 0xae, 0xa7, 0x7a, 0x44, 0xc1, 0x74,
    0x8b, 0x0b, 0x07, 0x6c, 0x9b, 0xcf, 0x14, 0xeb, 0x59, 0x03, 0x00, 0x00,
];

/// A minimal but real MP4: a single 16x16 black H.264 frame, `yuv420p`, with
/// `faststart` applied. Mirrors [`STUB_PNG`] and [`STUB_WOFF2`] — the KIND
/// `/public` mount must serve bytes a browser can actually decode at the
/// verified key, because a slide renders a `<video>` there and a file that
/// merely answers a HEAD would still show a broken player.
const STUB_MP4: &[u8] = &[
    0x00, 0x00, 0x00, 0x20, 0x66, 0x74, 0x79, 0x70, 0x69, 0x73, 0x6f, 0x6d, 0x00, 0x00, 0x02, 0x00,
    0x69, 0x73, 0x6f, 0x6d, 0x69, 0x73, 0x6f, 0x32, 0x61, 0x76, 0x63, 0x31, 0x6d, 0x70, 0x34, 0x31,
    0x00, 0x00, 0x03, 0x15, 0x6d, 0x6f, 0x6f, 0x76, 0x00, 0x00, 0x00, 0x6c, 0x6d, 0x76, 0x68, 0x64,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0xe8,
    0x00, 0x00, 0x03, 0xe8, 0x00, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x02, 0x3f, 0x74, 0x72, 0x61, 0x6b, 0x00, 0x00, 0x00, 0x5c,
    0x74, 0x6b, 0x68, 0x64, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0xe8, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00,
    0x00, 0x10, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x24, 0x65, 0x64, 0x74, 0x73,
    0x00, 0x00, 0x00, 0x1c, 0x65, 0x6c, 0x73, 0x74, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
    0x00, 0x00, 0x03, 0xe8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x01, 0xb7,
    0x6d, 0x64, 0x69, 0x61, 0x00, 0x00, 0x00, 0x20, 0x6d, 0x64, 0x68, 0x64, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x40, 0x00,
    0x55, 0xc4, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2d, 0x68, 0x64, 0x6c, 0x72, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x76, 0x69, 0x64, 0x65, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x56, 0x69, 0x64, 0x65, 0x6f, 0x48, 0x61, 0x6e, 0x64, 0x6c, 0x65, 0x72,
    0x00, 0x00, 0x00, 0x01, 0x62, 0x6d, 0x69, 0x6e, 0x66, 0x00, 0x00, 0x00, 0x14, 0x76, 0x6d, 0x68,
    0x64, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x24, 0x64, 0x69, 0x6e, 0x66, 0x00, 0x00, 0x00, 0x1c, 0x64, 0x72, 0x65, 0x66, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x0c, 0x75, 0x72, 0x6c, 0x20, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x01, 0x22, 0x73, 0x74, 0x62, 0x6c, 0x00, 0x00, 0x00, 0xbe, 0x73, 0x74, 0x73,
    0x64, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0xae, 0x61, 0x76, 0x63,
    0x31, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x10, 0x00, 0x48, 0x00,
    0x00, 0x00, 0x48, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x15, 0x4c, 0x61, 0x76, 0x63,
    0x36, 0x32, 0x2e, 0x32, 0x38, 0x2e, 0x31, 0x30, 0x32, 0x20, 0x6c, 0x69, 0x62, 0x78, 0x32, 0x36,
    0x34, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0xff, 0xff, 0x00,
    0x00, 0x00, 0x34, 0x61, 0x76, 0x63, 0x43, 0x01, 0x64, 0x00, 0x0a, 0xff, 0xe1, 0x00, 0x17, 0x67,
    0x64, 0x00, 0x0a, 0xac, 0xd9, 0x5e, 0xc0, 0x44, 0x00, 0x00, 0x03, 0x00, 0x04, 0x00, 0x00, 0x03,
    0x00, 0x08, 0x3c, 0x48, 0x96, 0x58, 0x01, 0x00, 0x06, 0x68, 0xeb, 0xe3, 0xcb, 0x22, 0xc0, 0xfd,
    0xf8, 0xf8, 0x00, 0x00, 0x00, 0x00, 0x10, 0x70, 0x61, 0x73, 0x70, 0x00, 0x00, 0x00, 0x01, 0x00,
    0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x14, 0x62, 0x74, 0x72, 0x74, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x16, 0x28, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x73, 0x74, 0x74, 0x73, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x40, 0x00, 0x00,
    0x00, 0x00, 0x1c, 0x73, 0x74, 0x73, 0x63, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00,
    0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x14, 0x73,
    0x74, 0x73, 0x7a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xc5, 0x00, 0x00, 0x00, 0x01, 0x00,
    0x00, 0x00, 0x14, 0x73, 0x74, 0x63, 0x6f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00,
    0x00, 0x03, 0x45, 0x00, 0x00, 0x00, 0x62, 0x75, 0x64, 0x74, 0x61, 0x00, 0x00, 0x00, 0x5a, 0x6d,
    0x65, 0x74, 0x61, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x21, 0x68, 0x64, 0x6c, 0x72, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x6d, 0x64, 0x69, 0x72, 0x61, 0x70, 0x70, 0x6c, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2d, 0x69, 0x6c, 0x73, 0x74,
    0x00, 0x00, 0x00, 0x25, 0xa9, 0x74, 0x6f, 0x6f, 0x00, 0x00, 0x00, 0x1d, 0x64, 0x61, 0x74, 0x61,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x4c, 0x61, 0x76, 0x66, 0x36, 0x32, 0x2e, 0x31,
    0x32, 0x2e, 0x31, 0x30, 0x32, 0x00, 0x00, 0x00, 0x08, 0x66, 0x72, 0x65, 0x65, 0x00, 0x00, 0x02,
    0xcd, 0x6d, 0x64, 0x61, 0x74, 0x00, 0x00, 0x02, 0xad, 0x06, 0x05, 0xff, 0xff, 0xa9, 0xdc, 0x45,
    0xe9, 0xbd, 0xe6, 0xd9, 0x48, 0xb7, 0x96, 0x2c, 0xd8, 0x20, 0xd9, 0x23, 0xee, 0xef, 0x78, 0x32,
    0x36, 0x34, 0x20, 0x2d, 0x20, 0x63, 0x6f, 0x72, 0x65, 0x20, 0x31, 0x36, 0x35, 0x20, 0x72, 0x33,
    0x32, 0x32, 0x32, 0x20, 0x62, 0x33, 0x35, 0x36, 0x30, 0x35, 0x61, 0x20, 0x2d, 0x20, 0x48, 0x2e,
    0x32, 0x36, 0x34, 0x2f, 0x4d, 0x50, 0x45, 0x47, 0x2d, 0x34, 0x20, 0x41, 0x56, 0x43, 0x20, 0x63,
    0x6f, 0x64, 0x65, 0x63, 0x20, 0x2d, 0x20, 0x43, 0x6f, 0x70, 0x79, 0x6c, 0x65, 0x66, 0x74, 0x20,
    0x32, 0x30, 0x30, 0x33, 0x2d, 0x32, 0x30, 0x32, 0x35, 0x20, 0x2d, 0x20, 0x68, 0x74, 0x74, 0x70,
    0x3a, 0x2f, 0x2f, 0x77, 0x77, 0x77, 0x2e, 0x76, 0x69, 0x64, 0x65, 0x6f, 0x6c, 0x61, 0x6e, 0x2e,
    0x6f, 0x72, 0x67, 0x2f, 0x78, 0x32, 0x36, 0x34, 0x2e, 0x68, 0x74, 0x6d, 0x6c, 0x20, 0x2d, 0x20,
    0x6f, 0x70, 0x74, 0x69, 0x6f, 0x6e, 0x73, 0x3a, 0x20, 0x63, 0x61, 0x62, 0x61, 0x63, 0x3d, 0x31,
    0x20, 0x72, 0x65, 0x66, 0x3d, 0x33, 0x20, 0x64, 0x65, 0x62, 0x6c, 0x6f, 0x63, 0x6b, 0x3d, 0x31,
    0x3a, 0x30, 0x3a, 0x30, 0x20, 0x61, 0x6e, 0x61, 0x6c, 0x79, 0x73, 0x65, 0x3d, 0x30, 0x78, 0x33,
    0x3a, 0x30, 0x78, 0x31, 0x31, 0x33, 0x20, 0x6d, 0x65, 0x3d, 0x68, 0x65, 0x78, 0x20, 0x73, 0x75,
    0x62, 0x6d, 0x65, 0x3d, 0x37, 0x20, 0x70, 0x73, 0x79, 0x3d, 0x31, 0x20, 0x70, 0x73, 0x79, 0x5f,
    0x72, 0x64, 0x3d, 0x31, 0x2e, 0x30, 0x30, 0x3a, 0x30, 0x2e, 0x30, 0x30, 0x20, 0x6d, 0x69, 0x78,
    0x65, 0x64, 0x5f, 0x72, 0x65, 0x66, 0x3d, 0x31, 0x20, 0x6d, 0x65, 0x5f, 0x72, 0x61, 0x6e, 0x67,
    0x65, 0x3d, 0x31, 0x36, 0x20, 0x63, 0x68, 0x72, 0x6f, 0x6d, 0x61, 0x5f, 0x6d, 0x65, 0x3d, 0x31,
    0x20, 0x74, 0x72, 0x65, 0x6c, 0x6c, 0x69, 0x73, 0x3d, 0x31, 0x20, 0x38, 0x78, 0x38, 0x64, 0x63,
    0x74, 0x3d, 0x31, 0x20, 0x63, 0x71, 0x6d, 0x3d, 0x30, 0x20, 0x64, 0x65, 0x61, 0x64, 0x7a, 0x6f,
    0x6e, 0x65, 0x3d, 0x32, 0x31, 0x2c, 0x31, 0x31, 0x20, 0x66, 0x61, 0x73, 0x74, 0x5f, 0x70, 0x73,
    0x6b, 0x69, 0x70, 0x3d, 0x31, 0x20, 0x63, 0x68, 0x72, 0x6f, 0x6d, 0x61, 0x5f, 0x71, 0x70, 0x5f,
    0x6f, 0x66, 0x66, 0x73, 0x65, 0x74, 0x3d, 0x2d, 0x32, 0x20, 0x74, 0x68, 0x72, 0x65, 0x61, 0x64,
    0x73, 0x3d, 0x31, 0x20, 0x6c, 0x6f, 0x6f, 0x6b, 0x61, 0x68, 0x65, 0x61, 0x64, 0x5f, 0x74, 0x68,
    0x72, 0x65, 0x61, 0x64, 0x73, 0x3d, 0x31, 0x20, 0x73, 0x6c, 0x69, 0x63, 0x65, 0x64, 0x5f, 0x74,
    0x68, 0x72, 0x65, 0x61, 0x64, 0x73, 0x3d, 0x30, 0x20, 0x6e, 0x72, 0x3d, 0x30, 0x20, 0x64, 0x65,
    0x63, 0x69, 0x6d, 0x61, 0x74, 0x65, 0x3d, 0x31, 0x20, 0x69, 0x6e, 0x74, 0x65, 0x72, 0x6c, 0x61,
    0x63, 0x65, 0x64, 0x3d, 0x30, 0x20, 0x62, 0x6c, 0x75, 0x72, 0x61, 0x79, 0x5f, 0x63, 0x6f, 0x6d,
    0x70, 0x61, 0x74, 0x3d, 0x30, 0x20, 0x63, 0x6f, 0x6e, 0x73, 0x74, 0x72, 0x61, 0x69, 0x6e, 0x65,
    0x64, 0x5f, 0x69, 0x6e, 0x74, 0x72, 0x61, 0x3d, 0x30, 0x20, 0x62, 0x66, 0x72, 0x61, 0x6d, 0x65,
    0x73, 0x3d, 0x33, 0x20, 0x62, 0x5f, 0x70, 0x79, 0x72, 0x61, 0x6d, 0x69, 0x64, 0x3d, 0x32, 0x20,
    0x62, 0x5f, 0x61, 0x64, 0x61, 0x70, 0x74, 0x3d, 0x31, 0x20, 0x62, 0x5f, 0x62, 0x69, 0x61, 0x73,
    0x3d, 0x30, 0x20, 0x64, 0x69, 0x72, 0x65, 0x63, 0x74, 0x3d, 0x31, 0x20, 0x77, 0x65, 0x69, 0x67,
    0x68, 0x74, 0x62, 0x3d, 0x31, 0x20, 0x6f, 0x70, 0x65, 0x6e, 0x5f, 0x67, 0x6f, 0x70, 0x3d, 0x30,
    0x20, 0x77, 0x65, 0x69, 0x67, 0x68, 0x74, 0x70, 0x3d, 0x32, 0x20, 0x6b, 0x65, 0x79, 0x69, 0x6e,
    0x74, 0x3d, 0x32, 0x35, 0x30, 0x20, 0x6b, 0x65, 0x79, 0x69, 0x6e, 0x74, 0x5f, 0x6d, 0x69, 0x6e,
    0x3d, 0x31, 0x20, 0x73, 0x63, 0x65, 0x6e, 0x65, 0x63, 0x75, 0x74, 0x3d, 0x34, 0x30, 0x20, 0x69,
    0x6e, 0x74, 0x72, 0x61, 0x5f, 0x72, 0x65, 0x66, 0x72, 0x65, 0x73, 0x68, 0x3d, 0x30, 0x20, 0x72,
    0x63, 0x5f, 0x6c, 0x6f, 0x6f, 0x6b, 0x61, 0x68, 0x65, 0x61, 0x64, 0x3d, 0x34, 0x30, 0x20, 0x72,
    0x63, 0x3d, 0x63, 0x72, 0x66, 0x20, 0x6d, 0x62, 0x74, 0x72, 0x65, 0x65, 0x3d, 0x31, 0x20, 0x63,
    0x72, 0x66, 0x3d, 0x32, 0x33, 0x2e, 0x30, 0x20, 0x71, 0x63, 0x6f, 0x6d, 0x70, 0x3d, 0x30, 0x2e,
    0x36, 0x30, 0x20, 0x71, 0x70, 0x6d, 0x69, 0x6e, 0x3d, 0x30, 0x20, 0x71, 0x70, 0x6d, 0x61, 0x78,
    0x3d, 0x36, 0x39, 0x20, 0x71, 0x70, 0x73, 0x74, 0x65, 0x70, 0x3d, 0x34, 0x20, 0x69, 0x70, 0x5f,
    0x72, 0x61, 0x74, 0x69, 0x6f, 0x3d, 0x31, 0x2e, 0x34, 0x30, 0x20, 0x61, 0x71, 0x3d, 0x31, 0x3a,
    0x31, 0x2e, 0x30, 0x30, 0x00, 0x80, 0x00, 0x00, 0x00, 0x10, 0x65, 0x88, 0x84, 0x00, 0x15, 0xff,
    0xfe, 0xf7, 0xc9, 0xef, 0xc0, 0xa6, 0xeb, 0xdb, 0xdf, 0x81,
];

/// Tiny valid bytes for a referenced asset's extension. CI stubs only need
/// browser-decodable bytes at the same path as the verified public object.
fn placeholder_bytes_for(rel: &str) -> anyhow::Result<Vec<u8>> {
    let ext = Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => {
            let rgb = [0xee, 0xee, 0xee];
            let mut bytes = Vec::new();
            JpegEncoder::new_with_quality(&mut bytes, JPEG_QUALITY)
                .write_image(&rgb, 1, 1, ExtendedColorType::Rgb8)
                .context("encode jpeg placeholder")?;
            Ok(bytes)
        }
        "png" => Ok(STUB_PNG.to_vec()),
        "webp" => {
            let encoded = webp::Encoder::from_rgb(&[0xee, 0xee, 0xee], 1, 1).encode(WEBP_QUALITY);
            Ok(encoded.to_vec())
        }
        "avif" => {
            let rgb = [0xee, 0xee, 0xee];
            let avif = ravif::Encoder::new()
                .with_quality(AVIF_QUALITY)
                .with_speed(AVIF_SPEED)
                .encode_rgb(ravif::Img::new(rgb.as_rgb(), 1, 1))
                .context("encode avif placeholder")?;
            Ok(avif.avif_file)
        }
        "woff2" => Ok(STUB_WOFF2.to_vec()),
        // A slide can reference a clip, and the deploy lane stubs every
        // content reference before verifying it. Without this arm a deck
        // carrying a video fails `stub-referenced` outright.
        "mp4" => Ok(STUB_MP4.to_vec()),
        _ => anyhow::bail!("unsupported asset extension for `{rel}`"),
    }
}

fn write_stub(dest: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create `{}`", parent.display()))?;
    }
    std::fs::write(dest, bytes).with_context(|| format!("write `{}`", dest.display()))
}

/// Write placeholder bytes for every asset [`published_asset_refs`] verifies
/// into `out`. The public-origin verifier owns publication correctness; this
/// only materializes local bytes for an ephemeral KIND image. Neither the
/// photos nor the WOFF2 bytes are in the tree — both are gitignored and
/// excluded from a normal image build — so both are stubbed here and
/// un-ignored by the deploy workflow before the bake, which is what lets the
/// KIND origin serve every verified key.
fn stub_referenced_content(content_dir: &Path, out: &Path) -> u8 {
    let refs = match published_asset_refs(content_dir) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("navigator: assets stub-referenced: {e:#}");
            return 2;
        }
    };

    let mut written = 0usize;
    let mut errored = Vec::new();
    for rel in &refs {
        match (
            destination_for_ref(out, rel),
            placeholder_bytes_for(rel).with_context(|| format!("build placeholder for `{rel}`")),
        ) {
            (Ok(dest), Ok(bytes)) => match write_stub(&dest, &bytes) {
                Ok(()) => written += 1,
                Err(e) => errored.push(format!("{rel}: {e:#}")),
            },
            (Err(e), _) | (_, Err(e)) => errored.push(format!("{rel}: {e:#}")),
        }
    }

    if errored.is_empty() {
        println!(
            "navigator: assets stub-referenced: wrote {written} placeholder asset(s) into {}",
            out.display()
        );
        0
    } else {
        eprintln!(
            "navigator: assets stub-referenced: {} public asset(s) could not be stubbed:",
            errored.len()
        );
        for line in errored {
            eprintln!("  ? {line}");
        }
        2
    }
}

/// Entry point for `cli assets stub-referenced`.
pub fn run_stub_referenced(content_dir: &Path, out: &Path) -> ExitCode {
    ExitCode::from(stub_referenced_content(content_dir, out))
}

/// The content type for an asset under `server/public/img/`, keyed off its
/// extension. The three formats `cli assets build` emits (AVIF/WebP/JPEG)
/// plus `png` for hand-authored blog/illustration heroes are carried;
/// anything else under `dir` (a stray `.DS_Store`, an editor temp file)
/// is skipped rather than pushed with a wrong type.
fn content_type_for(ext: &str) -> Option<&'static str> {
    match ext {
        "avif" => Some("image/avif"),
        "webp" => Some("image/webp"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        // Not a `build` re-encode variant — `png` carries hand-authored
        // blog/illustration assets dropped straight under
        // `server/public/img/<slug>/` (e.g. a painted hero), where JPEG's
        // ringing on sharp edges would show. `upload`/`pull` carry the
        // bytes through untouched.
        "png" => Some("image/png"),
        // Video for a slide or a post, dropped under the same prefix and
        // carried through untouched — nothing here transcodes. A markdown
        // `![caption](img/<slug>/clip.mp4)` renders as `<video>`. MP4 is the
        // only accepted format, matching `views::markdown::VIDEO_EXTENSIONS`;
        // `every_renderable_video_extension_is_uploadable` fails the build if
        // the two ever diverge, because an extension the renderer accepts but
        // this does not uploads as nothing and 404s in every deployment.
        "mp4" => Some("video/mp4"),
        _ => None,
    }
}

/// Walk `dir` and `put_cached` every recognized image variant under the
/// key `img/<path-relative-to-dir>` (e.g. `img/lake-tahoe/lake-tahoe-400w.avif`).
/// Decoupled from backend construction so tests drive it against the
/// `Fs` backend. Returns the count of objects uploaded.
async fn upload(storage: &dyn StorageService, dir: &Path) -> anyhow::Result<usize> {
    anyhow::ensure!(
        dir.is_dir(),
        "asset directory `{}` does not exist — run `cli assets build` first",
        dir.display()
    );
    let mut uploaded = 0usize;
    for entry in walkdir::WalkDir::new(dir).follow_links(false) {
        let entry = entry.with_context(|| format!("walk `{}`", dir.display()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        let Some(content_type) = content_type_for(&ext) else {
            continue;
        };
        let rel = path
            .strip_prefix(dir)
            .with_context(|| format!("`{}` not under `{}`", path.display(), dir.display()))?;
        // Keys always use `/`; build from components so a Windows host
        // doesn't emit backslash-separated keys.
        let rel_key = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let key = format!("img/{rel_key}");
        let bytes = std::fs::read(path).with_context(|| format!("read `{}`", path.display()))?;
        storage
            .put_cached(&key, &bytes, content_type, ASSET_CACHE_CONTROL)
            .await
            .with_context(|| format!("upload `{key}`"))?;
        println!("  → {key} ({content_type}, {} bytes)", bytes.len());
        uploaded += 1;
    }
    // A publish that published nothing is a failure, not a success. The tree
    // is gitignored, so a fresh checkout has an empty (or image-free) one, and
    // reporting `uploaded 0 variant(s)` with exit 0 reads to an operator as
    // "the photos are live" while the bucket stays empty.
    anyhow::ensure!(
        uploaded > 0,
        "no image variants under `{}` — run `cli assets build` first",
        dir.display()
    );
    Ok(uploaded)
}

/// Upload precisely the licensed GORP faces the current web design uses.
/// Refusing a partial directory prevents a deploy that claims Bold support
/// while serving a synthetic browser-generated weight instead.
async fn upload_gorp_fonts(storage: &dyn StorageService, dir: &Path) -> anyhow::Result<usize> {
    anyhow::ensure!(
        dir.is_dir(),
        "GORP font directory `{}` does not exist",
        dir.display()
    );
    for file in GORP_FONT_FILES {
        let path = dir.join(file);
        anyhow::ensure!(
            path.is_file(),
            "required GORP font `{}` is missing",
            path.display()
        );
    }
    for file in GORP_FONT_FILES {
        let path = dir.join(file);
        let bytes = std::fs::read(&path).with_context(|| format!("read `{}`", path.display()))?;
        let key = format!("{GORP_FONT_PREFIX}/{file}");
        storage
            .put_cached(&key, &bytes, "font/woff2", ASSET_CACHE_CONTROL)
            .await
            .with_context(|| format!("upload `{key}`"))?;
        println!("  → {key} (font/woff2, {} bytes)", bytes.len());
    }
    Ok(GORP_FONT_FILES.len())
}

/// Package every `.otf` face in `dir` into one deflate-compressed ZIP.
///
/// Entries are sorted by filename and stamped with a fixed timestamp so the
/// archive is byte-stable across runs and hosts — a re-run only re-uploads
/// when the fonts themselves change. Directory-name components are stripped so
/// the archive is flat (`GORPSerif-Bold.otf`, not `GORP Serif/…`). Returns the
/// face count. A directory missing any expected face (see [`GORP_OTF_FILES`])
/// is refused, so a re-run cannot overwrite the canonical bundle with a
/// partial family a firm worker would download expecting every weight.
fn build_gorp_otf_zip(dir: &Path) -> anyhow::Result<(Vec<u8>, usize)> {
    use std::io::Write as _;

    anyhow::ensure!(
        dir.is_dir(),
        "OTF font directory `{}` does not exist",
        dir.display()
    );
    // Propagate a per-entry read error rather than dropping it: silently
    // skipping an unreadable entry could package a partial family and
    // overwrite the canonical bundle with fewer faces than the delivery.
    let mut faces: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("read `{}`", dir.display()))? {
        let entry = entry.with_context(|| format!("read an entry in `{}`", dir.display()))?;
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("otf"))
        {
            faces.push(path);
        }
    }
    faces.sort();
    // Reject a partial delivery: the family ships a fixed set of faces, so a
    // directory missing any expected one would otherwise overwrite the
    // canonical bundle with fewer faces than the family. This also covers the
    // degenerate empty directory.
    for file in GORP_OTF_FILES {
        anyhow::ensure!(
            dir.join(file).is_file(),
            "required GORP OTF face `{}` is missing",
            dir.join(file).display()
        );
    }

    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut cursor);
        // A fixed 1980-01-01 timestamp (the ZIP epoch) keeps the archive
        // deterministic — the face bytes, not the packaging run, decide it.
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .last_modified_time(zip::DateTime::default());
        for path in &faces {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .with_context(|| format!("non-UTF-8 font name `{}`", path.display()))?;
            let bytes =
                std::fs::read(path).with_context(|| format!("read `{}`", path.display()))?;
            zip.start_file(name, opts)
                .with_context(|| format!("zip entry `{name}`"))?;
            zip.write_all(&bytes)
                .with_context(|| format!("write `{name}`"))?;
        }
        zip.finish().context("finish ZIP")?;
    }
    Ok((cursor.into_inner(), faces.len()))
}

/// Build the GORP Serif desktop ZIP from `dir` and upload it to the private
/// documents bucket. Decoupled from backend construction so tests drive it
/// against the `Fs` backend. Returns the number of faces packaged. A plain
/// `put` (no `Cache-Control`) is deliberate: the object is private and only
/// ever streamed through the gated route, so no shared cache should hold it.
async fn upload_gorp_otf_zip(storage: &dyn StorageService, dir: &Path) -> anyhow::Result<usize> {
    let (zip_bytes, faces) = build_gorp_otf_zip(dir)?;
    storage
        .put(GORP_OTF_ZIP_KEY, &zip_bytes, "application/zip")
        .await
        .with_context(|| format!("upload `{GORP_OTF_ZIP_KEY}`"))?;
    println!(
        "  → {GORP_OTF_ZIP_KEY} (application/zip, {} bytes)",
        zip_bytes.len()
    );
    Ok(faces)
}

/// List the bucket's `img/` prefix and write each built variant to
/// `out/<key-without-"img/">` — the inverse of [`upload`]'s keying, so a
/// pulled tree is byte-identical to what `build` would produce. Skips
/// any object that isn't one of the three built formats (defensive: the
/// bucket's `img/` lane only ever holds variants). Decoupled from
/// backend construction so tests drive it against the `Fs` backend.
/// Returns the count of variants written.
async fn download(storage: &dyn StorageService, out: &Path) -> anyhow::Result<usize> {
    let listings = storage
        .list("img/")
        .await
        .context("list the bucket's `img/` prefix")?;
    let mut listed_under_img = 0usize;
    let mut pulled = 0usize;
    for listing in listings {
        let key = listing.key;
        let Some(rel) = key.strip_prefix("img/").filter(|r| !r.is_empty()) else {
            continue;
        };
        listed_under_img += 1;
        let ext = Path::new(rel)
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        if content_type_for(&ext).is_none() {
            continue;
        }
        // Rebuild the destination from `/`-separated key segments,
        // refusing empty/`.`/`..` so a malformed key can't escape `out`.
        let mut dest = out.to_path_buf();
        for seg in rel.split('/') {
            anyhow::ensure!(
                !seg.is_empty() && seg != "." && seg != "..",
                "refusing unsafe object key `{key}`"
            );
            dest.push(seg);
        }
        let obj = storage
            .get(&key)
            .await
            .with_context(|| format!("download `{key}`"))?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create `{}`", parent.display()))?;
        }
        std::fs::write(&dest, &obj.bytes).with_context(|| format!("write `{}`", dest.display()))?;
        println!("  ← {key} → {} ({} bytes)", dest.display(), obj.bytes.len());
        pulled += 1;
    }
    anyhow::ensure!(
        pulled > 0,
        "{}",
        if listed_under_img == 0 {
            "no objects under `img/` in the bucket — populate it first with \
             `cli assets build` + `cli assets upload`"
        } else {
            "objects exist under `img/`, but none are supported image variants \
             (.avif, .webp, .jpg, .jpeg, .png)"
        }
    );
    Ok(pulled)
}

#[cfg(test)]
mod tests {
    use super::{
        asset_exists, build_gorp_otf_zip, bundled_slide_asset_keys, content_image_refs,
        content_type_for, destination_for_ref, download, fetch_asset, fetch_referenced_content,
        fetch_report_exit, gallery_variant_keys, gorp_font_refs, join_public_url, orphan_keys,
        orphan_report, parse_image_refs, placeholder_bytes_for, published_asset_refs,
        reachable_image_keys, report_exit, resolve_public_origin, run_fetch_referenced,
        run_orphans, run_pull, run_upload, run_upload_desktop_fonts, run_upload_fonts, select,
        storage_report_result, stub_referenced_content, upload, upload_gorp_fonts,
        upload_gorp_otf_zip, verify_bundled_slide_assets, verify_bundled_slide_assets_bucket,
        verify_content, verify_refs, verify_storage_refs, AssetProbe, FetchReport, VerifyReport,
        ASSET_CACHE_CONTROL, GORP_OTF_ZIP_KEY,
    };
    use cloud::{FsStorage, ObjectListing, StorageError, StorageService, StoredObject};
    use std::collections::BTreeSet;
    use std::fs;
    use std::process::ExitCode;
    use std::time::Duration;
    use tempfile::TempDir;
    use views::assets::{GALLERY, WIDTHS};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    struct ListingOnlyStorage {
        keys: Vec<String>,
    }

    #[async_trait::async_trait]
    impl StorageService for ListingOnlyStorage {
        async fn put(
            &self,
            _key: &str,
            _bytes: &[u8],
            _content_type: &str,
        ) -> Result<(), StorageError> {
            Err(StorageError::Unsupported("ListingOnlyStorage put"))
        }

        async fn get(&self, key: &str) -> Result<StoredObject, StorageError> {
            Ok(StoredObject {
                key: key.to_string(),
                bytes: b"bytes".to_vec(),
                content_type: "image/avif".to_string(),
            })
        }

        async fn delete(&self, _key: &str) -> Result<(), StorageError> {
            Err(StorageError::Unsupported("ListingOnlyStorage delete"))
        }

        async fn list(&self, prefix: &str) -> Result<Vec<ObjectListing>, StorageError> {
            Ok(self
                .keys
                .iter()
                .filter(|key| key.starts_with(prefix))
                .map(|key| ObjectListing {
                    key: key.clone(),
                    size_bytes: 1,
                })
                .collect())
        }

        async fn signed_url(
            &self,
            _key: &str,
            _expires_in: Duration,
        ) -> Result<String, StorageError> {
            Err(StorageError::Unsupported(
                "ListingOnlyStorage has no signed URL",
            ))
        }
    }

    #[test]
    fn parse_image_refs_extracts_img_targets_and_ignores_other_links() {
        let md = "\
![Ferris](img/going-all-in-on-rust/ferris-rust-logo-nlf-20260705.png)\n\
Some prose with a [doc link](/docs/assets) and an external \
[site](https://example.com/img/not-an-asset.png).\n\
![Tahoe](img/lake-tahoe/lake-tahoe-800w.avif \"a title\")\n\
Inline raw-HTML tile: <div>![Team](img/thanks-apple/team-lunch.jpg)</div>\n";
        let refs = parse_image_refs(md);
        assert_eq!(
            refs,
            vec![
                "img/going-all-in-on-rust/ferris-rust-logo-nlf-20260705.png".to_string(),
                // The `"a title"` suffix is dropped at the space.
                "img/lake-tahoe/lake-tahoe-800w.avif".to_string(),
                "img/thanks-apple/team-lunch.jpg".to_string(),
            ],
            "only `](img/…)` targets are keys; site links and doc links are not"
        );
    }

    #[test]
    fn content_image_refs_walks_the_tree_dedupes_and_sorts() {
        let dir = TempDir::new().unwrap();
        let blog = dir.path().join("blog");
        fs::create_dir_all(&blog).unwrap();
        // Two posts, one image repeated across them plus a unique one, a
        // video written with the same image syntax (the extractor is
        // extension-agnostic — it must discover the mp4 lane too), and a
        // non-markdown file that must be ignored.
        fs::write(
            blog.join("a.md"),
            "![hero](img/slug-a/hero.png)\n![clip](img/slug-a/demo.mp4)\n![shared](img/common/logo.png)\n",
        )
        .unwrap();
        fs::write(
            blog.join("b.md"),
            "![shared](img/common/logo.png)\ntext ![two](img/slug-b/two.jpg)\n",
        )
        .unwrap();
        fs::write(blog.join("notes.txt"), "![nope](img/ignored/x.png)").unwrap();

        let refs = content_image_refs(dir.path()).unwrap();
        assert_eq!(
            refs.into_iter().collect::<Vec<_>>(),
            vec![
                "img/common/logo.png".to_string(),
                "img/slug-a/demo.mp4".to_string(),
                "img/slug-a/hero.png".to_string(),
                "img/slug-b/two.jpg".to_string(),
            ],
            "deduped across files, sorted, images and video, and only from `.md`"
        );
    }

    #[test]
    fn content_image_refs_errors_when_the_dir_is_missing() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("no-such-content");
        let err = content_image_refs(&missing).unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn content_image_refs_reads_the_real_content_tree() {
        // Guards that the extractor stays wired to the actual repo layout:
        // the workspace `server/content` tree is the single source of truth
        // for which media the deployments must publish, so it must yield
        // every published medium `verify` is meant to gate. If the loaders'
        // image syntax ever changes, this fails instead of verify silently
        // checking nothing.
        let content = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../server/content");
        let refs = content_image_refs(&content).unwrap();
        assert!(
            refs.contains("img/going-all-in-on-rust/ferris-training-hero.png"),
            "the Rust post hero must be discovered, got: {refs:?}"
        );
        // The Rust in Peace slide media the publish gate must know to check:
        // the deck's cover image and the Las Vegas Ruby Group logo, each a
        // slide image on the bucket lane. Pinning them here means a deck can
        // never again reference media the reference set — and so the publish
        // gate — does not know to check for. (The mp4 placeholder clip was
        // retired with the Delete Your Data talk; the extractor's
        // extension-agnostic discovery — videos included — is pinned by
        // `content_image_refs_walks_the_tree_dedupes_and_sorts`.)
        assert!(
            refs.contains("img/rust-in-peace/cover.png"),
            "the Rust in Peace cover image must be discovered, got: {refs:?}"
        );
        assert!(
            refs.contains("img/lvrug/lvrug.png"),
            "the Rust in Peace slide image must be discovered, got: {refs:?}"
        );
        assert!(
            refs.iter().all(|r| r.starts_with("img/")),
            "every discovered reference is an `img/` key, got: {refs:?}"
        );
    }

    #[test]
    fn bundled_slide_asset_keys_carry_every_workshop_image_into_ship() {
        let refs = bundled_slide_asset_keys();
        let expected: BTreeSet<String> = [
            "img/rust-in-peace/cover.png",
            "img/lvrug/lvrug.png",
            "img/rust-in-peace/apple-teaching.jpg",
            "img/rust-in-peace/apple-team.jpg",
            "img/rust-in-peace/clippy-for-law.png",
            "img/rust-in-peace/cloud-gcp.png",
            "img/rust-in-peace/cloud-iceberg-backups.png",
            "img/rust-in-peace/cloud-saas.png",
            "img/rust-in-peace/continuous-feedback.png",
            "img/rust-in-peace/data-documents.png",
            "img/rust-in-peace/data-people.png",
            "img/rust-in-peace/data-project.png",
            "img/rust-in-peace/devx-agent-skills.png",
            "img/rust-in-peace/devx-glossary-ontology.png",
            "img/rust-in-peace/devx-monorepo.png",
            "img/rust-in-peace/devx-one-flow-worktree-pr.png",
            "img/rust-in-peace/everyone-loves-vibing.png",
            "img/rust-in-peace/ferris-access-to-justice.png",
            "img/rust-in-peace/ferris-apple-windows-linux.png",
            "img/rust-in-peace/ferris-gke-autopilot.png",
            "img/rust-in-peace/ferris-green-wood-farewell.png",
            "img/rust-in-peace/ferris-races-the-hare.png",
            "img/rust-in-peace/ferris-shared-governance.png",
            "img/rust-in-peace/ferris-signs-rust-document.png",
            "img/rust-in-peace/ferris-surrealdb-restate.png",
            "img/rust-in-peace/ferris-three-way-pointing.png",
            "img/rust-in-peace/ferris-web-cli-mcp.png",
            "img/rust-in-peace/intake-customer-conversion.png",
            "img/rust-in-peace/kiwi-rainbow.jpg",
            "img/rust-in-peace/libraries-developer-parity.png",
            "img/rust-in-peace/new-york-lawyer-decision.jpg",
            "img/rust-in-peace/project-lifecycle.png",
            "img/rust-in-peace/retainer-agreement-preview.png",
            "img/rust-in-peace/rust-community-meetup.png",
            "img/rust-in-peace/tests-checks-passed.png",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(
            refs, expected,
            "the standalone ship binary must carry the exact workshop media set"
        );
    }

    #[tokio::test]
    async fn storage_verifier_reports_every_missing_bucket_key() {
        let bucket = TempDir::new().unwrap();
        let storage = FsStorage::new(bucket.path().to_path_buf()).await.unwrap();
        storage
            .put("img/present.png", b"present", "image/png")
            .await
            .unwrap();
        let refs = ["img/missing.png", "img/present.png"]
            .into_iter()
            .map(String::from)
            .collect();

        let report = verify_storage_refs(&storage, &refs).await;

        assert_eq!(report.checked, 2);
        assert_eq!(report.missing, vec!["img/missing.png".to_string()]);
        assert!(report.unknown.is_empty(), "got: {:?}", report.unknown);
    }

    #[tokio::test]
    async fn slide_preflight_passes_when_the_bucket_holds_every_bundled_key() {
        let bucket = TempDir::new().unwrap();
        let storage = FsStorage::new(bucket.path().to_path_buf()).await.unwrap();
        for key in bundled_slide_asset_keys() {
            storage.put(&key, b"bytes", "image/png").await.unwrap();
        }

        verify_bundled_slide_assets(&storage, "example-prod-assets")
            .await
            .expect("a fully published bucket is a shippable bucket");
    }

    #[tokio::test]
    async fn slide_preflight_names_the_key_the_selected_bucket_is_missing() {
        let bucket = TempDir::new().unwrap();
        let storage = FsStorage::new(bucket.path().to_path_buf()).await.unwrap();
        let keys = bundled_slide_asset_keys();
        let withheld = keys.iter().next().expect("bundled keys").clone();
        for key in keys.iter().filter(|key| **key != withheld) {
            storage.put(key, b"bytes", "image/png").await.unwrap();
        }

        let error = verify_bundled_slide_assets(&storage, "example-prod-assets")
            .await
            .expect_err("one unpublished slide asset blocks the roll")
            .to_string();

        assert!(
            error.contains("1 missing, 0 could not be checked"),
            "got: {error}"
        );
        assert!(
            error.contains(&format!("missing: gs://example-prod-assets/{withheld}")),
            "the operator needs the exact object to publish, got: {error}"
        );
        assert!(
            error.contains("Publish every presentation/workshop asset"),
            "got: {error}"
        );
    }

    #[test]
    fn slide_preflight_fails_closed_on_a_key_it_could_not_check() {
        // A probe that errored is not a probe that passed: an unreadable
        // bucket must block the roll exactly like a missing object, and name
        // the key so the operator knows what to look at.
        let report = VerifyReport {
            checked: 2,
            missing: Vec::new(),
            unknown: vec!["img/lvrug/lvrug.png: gcs error: connection reset".to_string()],
        };

        let error = storage_report_result(&report, "example-prod-assets")
            .expect_err("an unreadable bucket is not a published bucket")
            .to_string();

        assert!(
            error.contains("0 missing, 1 could not be checked"),
            "got: {error}"
        );
        assert!(
            error.contains("unknown: gs://example-prod-assets/img/lvrug/lvrug.png"),
            "got: {error}"
        );
    }

    #[test]
    fn slide_preflight_refuses_a_blank_bucket_before_opening_a_client() {
        let error = verify_bundled_slide_assets_bucket("   ")
            .expect_err("a blank coordinate cannot be verified")
            .to_string();
        assert!(error.contains("assets bucket is blank"), "got: {error}");
    }

    /// The bucket-lane commands that reach real GCS all refuse a blank
    /// coordinate before a client is opened, the same guard the ship
    /// preflight above applies. Exit 2 with a named coordinate beats an
    /// opaque ADC failure.
    #[test]
    fn every_bucket_command_reports_a_missing_bucket_without_touching_the_network() {
        let dir = TempDir::new().unwrap();
        assert_eq!(
            run_upload(dir.path(), Some("   ".to_string())),
            ExitCode::from(2)
        );
        assert_eq!(
            run_pull(dir.path(), Some("   ".to_string())),
            ExitCode::from(2)
        );
        assert_eq!(
            run_orphans(dir.path(), Some("   ".to_string()), false),
            ExitCode::from(2)
        );
    }

    #[test]
    fn placeholder_bytes_for_emits_decodable_bytes_for_every_supported_extension() {
        // The stub only has to be a valid, browser-decodable file of the
        // referenced type so the KIND `/public` mount serves real bytes at
        // the verified key. Assert the container magic for each format the
        // gallery pipeline emits (`assets build`) plus hand-authored PNG.
        let png = placeholder_bytes_for("img/slug/hero.png").unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "png magic");

        // A deck slide can reference a clip, and the deploy lane stubs every
        // content reference before verifying it — without this the whole
        // `stub-referenced` step fails on a deck carrying a video.
        let mp4 = placeholder_bytes_for("img/slug/clip.mp4").unwrap();
        assert_eq!(&mp4[4..8], b"ftyp", "mp4 magic: an ISO base media box");
        assert!(
            mp4.windows(4).any(|w| w == b"moov") && mp4.windows(4).any(|w| w == b"mdat"),
            "the stub must be a real, decodable MP4, not an empty container"
        );

        for jpg_ref in ["img/slug/photo.jpg", "img/slug/photo.jpeg"] {
            let jpg = placeholder_bytes_for(jpg_ref).unwrap();
            assert_eq!(&jpg[..2], &[0xff, 0xd8], "jpeg SOI for {jpg_ref}");
        }

        let webp = placeholder_bytes_for("img/slug/photo.webp").unwrap();
        assert_eq!(&webp[..4], b"RIFF", "webp RIFF header");
        assert_eq!(&webp[8..12], b"WEBP", "webp form type");

        let avif = placeholder_bytes_for("img/slug/photo.avif").unwrap();
        assert_eq!(&avif[4..8], b"ftyp", "avif ftyp box");
        assert_eq!(&avif[8..12], b"avif", "avif major brand");

        // The licensed faces are stubbed on the same seam as the photos, so
        // the KIND image serves a parseable font rather than bytes that only
        // answer a HEAD.
        let woff2 = placeholder_bytes_for("fonts/gorp-serif/GORPSerif-Bold.woff2").unwrap();
        assert_eq!(&woff2[..4], b"wOF2", "woff2 signature");
        // `totalCompressedSize` (offset 20) must describe a non-empty
        // brotli stream — a truncated const would still carry the magic.
        assert!(
            u32::from_be_bytes(woff2[20..24].try_into().unwrap()) > 0,
            "woff2 must carry a compressed table stream"
        );
    }

    #[test]
    fn placeholder_bytes_for_uses_case_insensitive_extensions() {
        // Content can reference `.JPG`/`.PNG`; the match lower-cases first.
        let png = placeholder_bytes_for("img/slug/HERO.PNG").unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let jpg = placeholder_bytes_for("img/slug/PHOTO.JPG").unwrap();
        assert_eq!(&jpg[..2], &[0xff, 0xd8]);
    }

    #[test]
    fn placeholder_bytes_for_rejects_an_unsupported_extension() {
        // A format outside the gallery pipeline (or a missing extension)
        // fails loudly rather than writing a wrong-typed placeholder, so a
        // genuinely new content format is a deliberate CLI change, not a
        // silent mis-served stub.
        let err = placeholder_bytes_for("img/slug/logo.gif").unwrap_err();
        assert!(
            err.to_string().contains("unsupported asset extension"),
            "got: {err}"
        );
        let no_ext = placeholder_bytes_for("img/slug/noext").unwrap_err();
        assert!(no_ext.to_string().contains("unsupported asset extension"));
    }

    #[test]
    fn destination_for_ref_builds_nested_paths_under_the_output_root() {
        let out = TempDir::new().unwrap();
        let dest = destination_for_ref(out.path(), "img/slug/file.png").unwrap();
        assert_eq!(dest, out.path().join("img").join("slug").join("file.png"));
    }

    #[test]
    fn destination_for_ref_refuses_traversal_and_empty_segments() {
        let out = TempDir::new().unwrap();
        for unsafe_ref in ["img/../escape.png", "img/./slug/file.png", "img//file.png"] {
            let err = destination_for_ref(out.path(), unsafe_ref).unwrap_err();
            assert!(
                err.to_string().contains("refusing unsafe asset reference"),
                "{unsafe_ref} must be rejected, got: {err}"
            );
        }
    }

    #[test]
    fn join_public_url_matches_the_asset_seam_for_local_and_cdn_bases() {
        // Local `/public` mount and a production CDN origin both collapse
        // exactly one interior slash — the same join `asset_url` performs.
        assert_eq!(
            join_public_url("/public", "img/going-all-in-on-rust/ferris.png"),
            "/public/img/going-all-in-on-rust/ferris.png"
        );
        assert_eq!(
            join_public_url(
                "https://storage.googleapis.com/proj-assets/",
                "img/lake-tahoe/lake-tahoe-800w.avif"
            ),
            "https://storage.googleapis.com/proj-assets/img/lake-tahoe/lake-tahoe-800w.avif"
        );
    }

    #[test]
    fn explicit_public_origin_supports_local_and_gcp_gates() {
        assert_eq!(
            resolve_public_origin(Some("http://localhost:8080/public".to_string()), "verify")
                .unwrap(),
            "http://localhost:8080/public"
        );
        assert_eq!(
            resolve_public_origin(
                Some("https://storage.googleapis.com/proj-assets".to_string()),
                "verify"
            )
            .unwrap(),
            "https://storage.googleapis.com/proj-assets"
        );
        assert_eq!(
            resolve_public_origin(Some("   ".to_string()), "verify").unwrap_err(),
            2
        );
    }

    /// A probe backed by in-memory sets so `verify_refs` is driven
    /// without a network: a URL in `present` exists, one in `errored`
    /// fails the check, anything else is a definitive miss.
    struct FakeProbe {
        present: BTreeSet<String>,
        errored: BTreeSet<String>,
    }

    #[async_trait::async_trait]
    impl AssetProbe for FakeProbe {
        async fn exists(&self, url: &str) -> anyhow::Result<bool> {
            if self.errored.contains(url) {
                anyhow::bail!("probe boom for {url}");
            }
            Ok(self.present.contains(url))
        }
    }

    #[tokio::test]
    async fn verify_refs_buckets_present_missing_and_unknown_by_joined_url() {
        let refs: BTreeSet<String> = ["img/a/present.png", "img/b/missing.png", "img/c/boom.png"]
            .into_iter()
            .map(String::from)
            .collect();
        // The probe is keyed by the *joined* URL, so a hit here also
        // proves `verify_refs` asked the origin for the right path.
        let probe = FakeProbe {
            present: ["/public/img/a/present.png"]
                .into_iter()
                .map(String::from)
                .collect(),
            errored: ["/public/img/c/boom.png"]
                .into_iter()
                .map(String::from)
                .collect(),
        };
        let report = verify_refs(&probe, "/public", &refs).await;
        assert_eq!(report.checked, 3);
        assert_eq!(report.missing, vec!["img/b/missing.png".to_string()]);
        assert_eq!(report.unknown.len(), 1);
        assert!(
            report.unknown[0].starts_with("img/c/boom.png: "),
            "unknown entry is labelled by its reference, got: {}",
            report.unknown[0]
        );
    }

    #[test]
    fn report_exit_is_success_only_when_nothing_is_missing_or_unknown() {
        let clean = VerifyReport {
            checked: 3,
            missing: vec![],
            unknown: vec![],
        };
        assert_eq!(report_exit(&clean, "https://cdn.example/assets"), 0);

        let has_missing = VerifyReport {
            checked: 2,
            missing: vec!["img/x/hero.png".to_string()],
            unknown: vec![],
        };
        assert_eq!(report_exit(&has_missing, "https://cdn.example/assets"), 2);

        let has_unknown = VerifyReport {
            checked: 1,
            missing: vec![],
            unknown: vec!["img/y/z.png: transport error".to_string()],
        };
        assert_eq!(report_exit(&has_unknown, "https://cdn.example/assets"), 2);
    }

    #[tokio::test]
    async fn asset_exists_maps_head_get_and_error_statuses() {
        let server = MockServer::start().await;
        // 200 on HEAD → present.
        Mock::given(method("HEAD"))
            .and(path("/img/present.png"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        // 404 on HEAD → definitively missing.
        Mock::given(method("HEAD"))
            .and(path("/img/missing.png"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        // 405 on HEAD, 206 on the ranged GET fallback → present.
        Mock::given(method("HEAD"))
            .and(path("/img/head-405.png"))
            .respond_with(ResponseTemplate::new(405))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/img/head-405.png"))
            .respond_with(ResponseTemplate::new(206))
            .mount(&server)
            .await;
        // 405 on HEAD, 404 on the GET fallback → missing.
        Mock::given(method("HEAD"))
            .and(path("/img/head-405-gone.png"))
            .respond_with(ResponseTemplate::new(405))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/img/head-405-gone.png"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        // 500 on HEAD → not success, not 404 → unknown (Err).
        Mock::given(method("HEAD"))
            .and(path("/img/boom.png"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let base = server.uri();

        assert!(asset_exists(&client, &format!("{base}/img/present.png"))
            .await
            .unwrap());
        assert!(!asset_exists(&client, &format!("{base}/img/missing.png"))
            .await
            .unwrap());
        assert!(asset_exists(&client, &format!("{base}/img/head-405.png"))
            .await
            .unwrap());
        assert!(
            !asset_exists(&client, &format!("{base}/img/head-405-gone.png"))
                .await
                .unwrap()
        );
        assert!(asset_exists(&client, &format!("{base}/img/boom.png"))
            .await
            .is_err());
    }

    /// Write one blog post referencing `img/<rel>` into a fresh content
    /// dir and return it, so `verify_content` has a real tree to walk.
    fn content_dir_referencing(rel: &str) -> TempDir {
        let dir = TempDir::new().unwrap();
        let blog = dir.path().join("blog");
        fs::create_dir_all(&blog).unwrap();
        fs::write(blog.join("post.md"), format!("![hero]({rel})\n")).unwrap();
        dir
    }

    #[tokio::test]
    async fn verify_content_returns_2_when_no_origin_is_configured() {
        // A blank `--base-url` resolves to the no-origin branch without
        // touching the process env.
        let dir = content_dir_referencing("img/demo/hero.png");
        assert_eq!(verify_content(dir.path(), Some("   ".to_string())).await, 2);
    }

    #[tokio::test]
    async fn verify_content_returns_2_when_the_content_dir_is_missing() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("no-such-content");
        assert_eq!(
            verify_content(&missing, Some("https://cdn.example".to_string())).await,
            2
        );
    }

    /// Publish the licensed faces on `server`. `verify_content` probes them on
    /// every run, so any case that expects success must serve them.
    async fn mount_published_fonts(server: &MockServer) {
        for rel in gorp_font_refs() {
            Mock::given(method("HEAD"))
                .and(path(format!("/{rel}")))
                .respond_with(ResponseTemplate::new(200))
                .mount(server)
                .await;
        }
    }

    /// Publish every `GALLERY` variant on `server`. Those keys are referenced
    /// from Rust views, so `verify_content` probes them on every run and any
    /// case expecting success must serve them too.
    async fn mount_published_gallery(server: &MockServer) {
        for rel in gallery_variant_keys() {
            Mock::given(method("HEAD"))
                .and(path(format!("/{rel}")))
                .respond_with(ResponseTemplate::new(200))
                .mount(server)
                .await;
        }
    }

    #[tokio::test]
    async fn verify_content_probes_the_fonts_even_when_the_tree_has_no_images() {
        let server = MockServer::start().await;
        mount_published_fonts(&server).await;
        mount_published_gallery(&server).await;
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("blog")).unwrap();
        assert_eq!(verify_content(dir.path(), Some(server.uri())).await, 0);
    }

    #[tokio::test]
    async fn verify_content_fails_when_a_gallery_photo_is_not_published() {
        // The other half of the production bug: the origin serves the licensed
        // faces and every markdown hero, but the manifest photos were never
        // uploaded. They are referenced from Rust `<picture>` elements, never
        // from markdown, so a content-tree sweep alone reports success while
        // every gallery tile on the site is a broken image.
        let server = MockServer::start().await;
        mount_published_fonts(&server).await;
        Mock::given(method("HEAD"))
            .and(path("/img/demo/hero.png"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let mut keys = gallery_variant_keys().into_iter();
        let withheld = keys.next().expect("the manifest publishes variants");
        for rel in keys {
            Mock::given(method("HEAD"))
                .and(path(format!("/{rel}")))
                .respond_with(ResponseTemplate::new(200))
                .mount(&server)
                .await;
        }
        Mock::given(method("HEAD"))
            .and(path(format!("/{withheld}")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let dir = content_dir_referencing("img/demo/hero.png");
        assert_eq!(verify_content(dir.path(), Some(server.uri())).await, 2);
    }

    #[test]
    fn verify_and_orphan_agree_on_the_reachable_image_keys() {
        // THE guard against the two halves drifting apart again. `orphan` asks
        // which published keys nothing reaches; `verify` asks which reached
        // keys are unpublished. Both must mean the same thing by "reached", so
        // a photo added to the manifest cannot enter one definition alone.
        let dir = content_dir_referencing("img/demo/hero.png");
        let refs = published_asset_refs(dir.path()).unwrap();
        let reachable = reachable_image_keys(dir.path()).unwrap();
        let image_refs: BTreeSet<String> = refs
            .iter()
            .filter(|key| key.starts_with("img/"))
            .cloned()
            .collect();
        assert_eq!(image_refs, reachable);
        for key in gallery_variant_keys() {
            assert!(refs.contains(&key), "verify must probe `{key}`");
        }
        for key in gorp_font_refs() {
            assert!(refs.contains(&key), "verify must probe `{key}`");
        }
    }

    #[test]
    fn stub_referenced_content_materializes_every_asset_the_verifier_checks() {
        let content = content_dir_referencing("img/demo/hero.png");
        let out = TempDir::new().unwrap();
        let expected = published_asset_refs(content.path()).unwrap();

        assert_eq!(stub_referenced_content(content.path(), out.path()), 0);
        for rel in expected {
            let destination = destination_for_ref(out.path(), &rel).unwrap();
            assert!(
                destination.is_file(),
                "stub-referenced must materialize verifier key `{rel}` at {}",
                destination.display()
            );
        }
    }

    #[tokio::test]
    async fn verify_content_fails_when_a_licensed_font_is_not_published() {
        // The production bug this gate exists to catch: the bucket serves every
        // content image, but `assets fonts upload` was never run, so each page
        // 404s its GORP faces and silently falls back to Georgia.
        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .and(path("/img/demo/hero.png"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("HEAD"))
            .and(path("/fonts/gorp-serif/GORPSerif-Bold.woff2"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("HEAD"))
            .and(path("/fonts/gorp-serif/GORPSerif-Regular.woff2"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let dir = content_dir_referencing("img/demo/hero.png");
        assert_eq!(verify_content(dir.path(), Some(server.uri())).await, 2);
    }

    #[tokio::test]
    async fn verify_content_passes_when_the_referenced_image_is_published() {
        let server = MockServer::start().await;
        mount_published_fonts(&server).await;
        mount_published_gallery(&server).await;
        Mock::given(method("HEAD"))
            .and(path("/img/demo/hero.png"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let dir = content_dir_referencing("img/demo/hero.png");
        assert_eq!(verify_content(dir.path(), Some(server.uri())).await, 0);
    }

    #[tokio::test]
    async fn verify_content_fails_when_the_referenced_image_404s() {
        let server = MockServer::start().await;
        mount_published_fonts(&server).await;
        Mock::given(method("HEAD"))
            .and(path("/img/demo/hero.png"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let dir = content_dir_referencing("img/demo/hero.png");
        assert_eq!(verify_content(dir.path(), Some(server.uri())).await, 2);
    }

    #[tokio::test]
    async fn fetch_referenced_content_writes_bytes_from_the_public_origin() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/img/demo/hero.png"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"png-bytes"))
            .mount(&server)
            .await;
        let content = content_dir_referencing("img/demo/hero.png");
        let out = TempDir::new().unwrap();
        assert_eq!(
            fetch_referenced_content(content.path(), Some(server.uri()), out.path()).await,
            0
        );
        assert_eq!(
            fs::read(out.path().join("img/demo/hero.png")).unwrap(),
            b"png-bytes"
        );
    }

    #[tokio::test]
    async fn fetch_referenced_content_returns_2_when_no_origin_is_configured() {
        let content = content_dir_referencing("img/demo/hero.png");
        let out = TempDir::new().unwrap();
        assert_eq!(
            fetch_referenced_content(content.path(), Some("   ".to_string()), out.path()).await,
            2
        );
    }

    #[tokio::test]
    async fn fetch_referenced_content_returns_2_when_the_content_dir_is_missing() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("no-such-content");
        let out = TempDir::new().unwrap();
        assert_eq!(
            fetch_referenced_content(
                &missing,
                Some("https://cdn.example".to_string()),
                out.path()
            )
            .await,
            2
        );
    }

    #[tokio::test]
    async fn fetch_referenced_content_succeeds_when_the_tree_has_no_image_references() {
        let content = TempDir::new().unwrap();
        fs::create_dir_all(content.path().join("blog")).unwrap();
        let out = TempDir::new().unwrap();
        assert_eq!(
            fetch_referenced_content(
                content.path(),
                Some("https://cdn.example".to_string()),
                out.path()
            )
            .await,
            0
        );
    }

    #[tokio::test]
    async fn fetch_referenced_content_records_transport_failures_as_errored() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let content = content_dir_referencing("img/demo/hero.png");
        let out = TempDir::new().unwrap();
        assert_eq!(
            fetch_referenced_content(
                content.path(),
                Some(format!("http://127.0.0.1:{port}")),
                out.path()
            )
            .await,
            2
        );
        assert!(!out.path().join("img/demo/hero.png").exists());
    }

    #[tokio::test]
    async fn fetch_referenced_content_fails_when_the_referenced_image_404s() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/img/demo/hero.png"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let content = content_dir_referencing("img/demo/hero.png");
        let out = TempDir::new().unwrap();
        assert_eq!(
            fetch_referenced_content(content.path(), Some(server.uri()), out.path()).await,
            2
        );
        assert!(!out.path().join("img/demo/hero.png").exists());
    }

    #[tokio::test]
    async fn fetch_asset_maps_get_statuses_like_verify() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/img/present.png"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"ok"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/img/missing.png"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let out = TempDir::new().unwrap();
        let base = server.uri();
        assert!(fetch_asset(
            &client,
            &format!("{base}/img/present.png"),
            &out.path().join("present.png")
        )
        .await
        .unwrap());
        assert!(!fetch_asset(
            &client,
            &format!("{base}/img/missing.png"),
            &out.path().join("missing.png")
        )
        .await
        .unwrap());
    }

    #[tokio::test]
    async fn fetch_asset_errors_on_an_unexpected_status() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/img/boom.png"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let out = TempDir::new().unwrap();
        let err = fetch_asset(
            &client,
            &format!("{}/img/boom.png", server.uri()),
            &out.path().join("boom.png"),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("unexpected status"), "got: {err}");
    }

    #[test]
    fn fetch_report_exit_is_success_only_when_nothing_is_missing_or_errored() {
        let out = TempDir::new().unwrap();
        let clean = FetchReport {
            fetched: 2,
            missing: vec![],
            errored: vec![],
        };
        assert_eq!(
            fetch_report_exit(&clean, "https://cdn.example", out.path()),
            0
        );

        let has_missing = FetchReport {
            fetched: 1,
            missing: vec!["img/x/hero.png".to_string()],
            errored: vec![],
        };
        assert_eq!(
            fetch_report_exit(&has_missing, "https://cdn.example", out.path()),
            2
        );

        let has_errored = FetchReport {
            fetched: 0,
            missing: vec![],
            errored: vec!["img/y/z.png: transport error".to_string()],
        };
        assert_eq!(
            fetch_report_exit(&has_errored, "https://cdn.example", out.path()),
            2
        );

        let has_both = FetchReport {
            fetched: 0,
            missing: vec!["img/x/hero.png".to_string()],
            errored: vec!["img/y/z.png: transport error".to_string()],
        };
        assert_eq!(
            fetch_report_exit(&has_both, "https://cdn.example", out.path()),
            2
        );
    }

    #[test]
    fn run_fetch_referenced_returns_success_exit_code_on_happy_path() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/img/demo/hero.png"))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(b"png-bytes"))
                .mount(&server)
                .await;
            let content = content_dir_referencing("img/demo/hero.png");
            let out = TempDir::new().unwrap();
            let server_uri = server.uri();
            let content_path = content.path().to_path_buf();
            let out_path = out.path().to_path_buf();
            let code = tokio::task::spawn_blocking(move || {
                run_fetch_referenced(&content_path, &out_path, Some(server_uri))
            })
            .await
            .unwrap();
            assert_eq!(code, ExitCode::from(0));
            assert_eq!(
                fs::read(out.path().join("img/demo/hero.png")).unwrap(),
                b"png-bytes"
            );
        });
    }

    #[tokio::test]
    async fn asset_exists_errors_on_a_dead_origin() {
        // Bind then drop a listener to claim a port that now refuses
        // connections — a transport failure must surface as Err (unknown),
        // never a false "missing".
        //
        // The freed ephemeral port returns to the OS pool, so a parallel test
        // (e.g. a wiremock `MockServer`) can reclaim it in the window before
        // the HEAD lands and answer with a 404, turning the expected transport
        // Err into `Ok(false)`. That is the port only, not `asset_exists`: on a
        // genuinely dead origin the request always fails to connect. Re-pick a
        // fresh port when we catch that race so the assertion stays honest.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        for attempt in 1..=16 {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            drop(listener);
            match asset_exists(&client, &format!("http://127.0.0.1:{port}/img/x.png")).await {
                Err(err) => {
                    assert!(
                        err.to_string().contains("HEAD"),
                        "transport failure is labelled with the request, got: {err}"
                    );
                    return;
                }
                // The freed port was reclaimed and answered; fall through to a
                // fresh port unless we have exhausted every attempt.
                Ok(exists) => assert!(
                    attempt < 16,
                    "asset_exists never surfaced a transport error for a dead origin \
                     after {attempt} attempts (last returned Ok({exists}))"
                ),
            }
        }
    }

    /// The renderer and the asset pipeline each keep their own list of what
    /// counts as a video, in different crates. They must agree: an extension
    /// `views::markdown` upgrades to a `<video>` element but `upload` skips
    /// would publish nothing while reporting success, leaving a player
    /// pointed at a 404 in every deployment — and `assets verify` reports
    /// that as a missing object rather than as the mismatch it is. Pin the
    /// direction that actually breaks a page.
    /// THE guard on this feature. Every `GALLERY` photograph is referenced
    /// from Rust views via `responsive_picture`, never from markdown — so a
    /// reachable set built from the content sweep alone reports all of them
    /// as orphans. A human trusting that report would delete the entire
    /// production photo library. Pin that the manifest's variants are
    /// reachable even when no markdown mentions them at all.
    #[test]
    fn a_gallery_photo_is_never_reported_as_an_orphan() {
        let dir = TempDir::new().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        // Content that references nothing — the worst case for the sweep.
        fs::write(content.join("post.md"), "# A post with no images\n").unwrap();

        let reachable = reachable_image_keys(&content).unwrap();
        assert!(
            !GALLERY.is_empty(),
            "the manifest must be non-empty for this guard to mean anything"
        );
        // Build what the bucket holds independently of the production
        // helper. Deriving it from `gallery_variant_keys` would move both
        // sides of the comparison together, so a helper that returned
        // nothing would still "pass" the orphan assertion below.
        let listed: Vec<String> = GALLERY
            .iter()
            .flat_map(|image| {
                WIDTHS.iter().flat_map(move |width| {
                    ["avif", "webp", "jpg"].into_iter().map(move |ext| {
                        format!("img/{slug}/{slug}-{width}w.{ext}", slug = image.slug)
                    })
                })
            })
            .collect();
        assert_eq!(
            listed.len(),
            GALLERY.len() * WIDTHS.len() * 3,
            "every photo contributes one object per width per format"
        );
        let orphans = orphan_keys(&listed, &reachable);
        assert!(
            orphans.is_empty(),
            "gallery variants must never be reported as orphans — reporting these \
             would invite deleting every production photograph. Got: {orphans:?}"
        );
    }

    /// The other half: an object nothing references *is* reported, and the
    /// separately managed `fonts/` lane is never in scope.
    #[test]
    fn orphan_keys_reports_the_unreferenced_and_ignores_other_prefixes() {
        let mut reachable = BTreeSet::new();
        reachable.insert("img/lvrug/lvrug.png".to_string());
        let listed = vec![
            "img/lvrug/lvrug.png".to_string(),
            "img/retired-deck/clip.mp4".to_string(),
            "fonts/gorp-serif/GORPSerif-Bold.woff2".to_string(),
        ];
        assert_eq!(
            orphan_keys(&listed, &reachable),
            vec!["img/retired-deck/clip.mp4".to_string()],
            "only unreferenced `img/` keys are orphans; `fonts/` is out of scope"
        );
    }

    /// The report is what reaches Slack, so pin that it names the finding
    /// and says plainly that nothing was deleted.
    #[test]
    fn orphan_report_states_the_finding_and_that_it_prunes_nothing() {
        let clean = orphan_report("neon-production-assets", &[], 126);
        assert!(clean.contains("no orphans"), "got: {clean}");
        assert!(clean.contains("126 held"), "got: {clean}");

        let found = orphan_report(
            "neon-production-assets",
            &["img/retired-deck/clip.mp4".to_string()],
            126,
        );
        assert!(found.contains("img/retired-deck/clip.mp4"), "got: {found}");
        assert!(
            found.contains("publicly fetchable"),
            "the report must say an unreferenced object is still reachable: {found}"
        );
        assert!(
            found.contains("does not prune"),
            "the report must be explicit that it deleted nothing: {found}"
        );
    }

    #[test]
    fn every_renderable_video_extension_is_uploadable() {
        for ext in views::markdown::VIDEO_EXTENSIONS {
            let content_type = content_type_for(ext);
            assert!(
                content_type.is_some(),
                "`views::markdown::VIDEO_EXTENSIONS` renders `.{ext}` as a video, but \
                 `content_type_for` skips it — a slide using it would upload nothing \
                 and serve a 404. Add it here or drop it there."
            );
            assert!(
                content_type.is_some_and(|ct| ct.starts_with("video/")),
                "`.{ext}` must upload with a video content type, got {content_type:?}"
            );
        }
    }

    #[test]
    fn content_type_covers_the_built_formats_plus_png_and_skips_others() {
        assert_eq!(content_type_for("avif"), Some("image/avif"));
        assert_eq!(content_type_for("webp"), Some("image/webp"));
        assert_eq!(content_type_for("jpg"), Some("image/jpeg"));
        assert_eq!(content_type_for("jpeg"), Some("image/jpeg"));
        // Video rides the same prefix, carried through untouched. This list
        // must stay in step with `views::markdown::VIDEO_EXTENSIONS`: an
        // extension the renderer upgrades to `<video>` but this skips would
        // upload as nothing and then 404 in every deployment.
        assert_eq!(content_type_for("mp4"), Some("video/mp4"));
        // Every other container is skipped — nothing here transcodes, and a
        // second format would be an alternative to choose between rather than
        // a fallback, because the rendered `<video>` carries a single `src`.
        assert_eq!(content_type_for("webm"), None);
        assert_eq!(content_type_for("mov"), None);
        assert_eq!(content_type_for("mkv"), None);
        // PNG rides the same lane for hand-authored blog/illustration heroes.
        assert_eq!(content_type_for("png"), Some("image/png"));
        assert_eq!(content_type_for("txt"), None);
        assert_eq!(content_type_for("DS_Store"), None);
    }

    #[tokio::test]
    async fn upload_keys_each_variant_under_img_and_skips_non_images() {
        // Lay out a slug directory the way `cli assets build` does,
        // plus a stray non-image file that must not be uploaded.
        let dir = TempDir::new().unwrap();
        let slug = dir.path().join("lake-tahoe");
        fs::create_dir_all(&slug).unwrap();
        fs::write(slug.join("lake-tahoe-400w.avif"), b"avif").unwrap();
        fs::write(slug.join("lake-tahoe-400w.webp"), b"webp").unwrap();
        fs::write(slug.join("lake-tahoe-400w.jpg"), b"jpg").unwrap();
        fs::write(slug.join(".DS_Store"), b"junk").unwrap();

        let store_dir = TempDir::new().unwrap();
        let storage = FsStorage::new(store_dir.path().to_path_buf())
            .await
            .unwrap();
        let n = upload(&storage, dir.path()).await.unwrap();
        assert_eq!(
            n, 3,
            "the three image variants upload, the stray file does not"
        );

        // Keys are `img/<slug>/<file>`; the default `put_cached` on the
        // Fs backend falls back to `put`, so the bytes round-trip.
        let got = storage
            .get("img/lake-tahoe/lake-tahoe-400w.avif")
            .await
            .unwrap();
        assert_eq!(got.bytes, b"avif");
        assert_eq!(got.content_type, "image/avif");
        // The non-image stray was never stored under any key.
        assert!(storage.get("img/lake-tahoe/.DS_Store").await.is_err());
    }

    #[tokio::test]
    async fn upload_errors_when_the_tree_holds_no_images() {
        // `server/public/img/` is gitignored, so this is what a fresh checkout
        // looks like. Reporting success here tells an operator the photos are
        // published when nothing was written.
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("notes.txt"), b"not an image").unwrap();
        let store_dir = TempDir::new().unwrap();
        let storage = FsStorage::new(store_dir.path().to_path_buf())
            .await
            .unwrap();
        let err = upload(&storage, dir.path()).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("assets build"),
            "must name the build step: {err:#}"
        );
    }

    #[tokio::test]
    async fn upload_errors_when_dir_is_missing() {
        let store_dir = TempDir::new().unwrap();
        let storage = FsStorage::new(store_dir.path().to_path_buf())
            .await
            .unwrap();
        let missing = store_dir.path().join("no-such-img-tree");
        let err = upload(&storage, &missing).await.unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[tokio::test]
    async fn upload_gorp_fonts_requires_both_faces_and_uses_the_font_prefix() {
        let source = TempDir::new().unwrap();
        fs::write(source.path().join("GORPSerif-Regular.woff2"), b"regular").unwrap();
        fs::write(source.path().join("GORPSerif-Bold.woff2"), b"bold").unwrap();

        let bucket = TempDir::new().unwrap();
        let storage = FsStorage::new(bucket.path().to_path_buf()).await.unwrap();
        assert_eq!(upload_gorp_fonts(&storage, source.path()).await.unwrap(), 2);

        let regular = storage
            .get("fonts/gorp-serif/GORPSerif-Regular.woff2")
            .await
            .unwrap();
        assert_eq!(regular.bytes, b"regular");
        assert_eq!(regular.content_type, "font/woff2");
        assert_eq!(
            storage
                .get("fonts/gorp-serif/GORPSerif-Bold.woff2")
                .await
                .unwrap()
                .bytes,
            b"bold"
        );
    }

    #[test]
    fn run_upload_fonts_reports_a_missing_bucket_without_touching_the_network() {
        // A blank `--bucket` fails the non-empty guard and short-circuits
        // before any tokio runtime or GCS client is constructed, so the
        // operator gets exit 2 rather than an opaque auth failure.
        let dir = TempDir::new().unwrap();
        let code = run_upload_fonts(dir.path(), Some("   ".to_string()));
        assert_eq!(code, ExitCode::from(2));
    }

    #[tokio::test]
    async fn upload_gorp_fonts_rejects_a_partial_licensed_delivery() {
        let source = TempDir::new().unwrap();
        fs::write(source.path().join("GORPSerif-Regular.woff2"), b"regular").unwrap();
        let bucket = TempDir::new().unwrap();
        let storage = FsStorage::new(bucket.path().to_path_buf()).await.unwrap();

        let err = upload_gorp_fonts(&storage, source.path())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("GORPSerif-Bold.woff2"));
        assert!(storage
            .get("fonts/gorp-serif/GORPSerif-Regular.woff2")
            .await
            .is_err());
    }

    /// Stage the full licensed family so `build`/`upload` accept the delivery.
    fn stage_full_gorp_family(dir: &std::path::Path) {
        for (i, file) in super::GORP_OTF_FILES.iter().enumerate() {
            fs::write(dir.join(file), format!("otf-{i}").as_bytes()).unwrap();
        }
    }

    #[tokio::test]
    async fn upload_gorp_otf_zip_packages_the_full_family_as_one_zip_object() {
        let source = TempDir::new().unwrap();
        stage_full_gorp_family(source.path());
        // A stray non-manifest file must be ignored, not packaged.
        fs::write(source.path().join("readme.txt"), b"license").unwrap();

        let bucket = TempDir::new().unwrap();
        let storage = FsStorage::new(bucket.path().to_path_buf()).await.unwrap();
        assert_eq!(
            upload_gorp_otf_zip(&storage, source.path()).await.unwrap(),
            6
        );

        let obj = storage.get(GORP_OTF_ZIP_KEY).await.unwrap();
        assert_eq!(obj.content_type, "application/zip");
        // The bytes are a real ZIP carrying exactly the manifest faces, flat.
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(obj.bytes)).expect("valid zip");
        assert_eq!(archive.len(), 6);
        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        assert_eq!(names, super::GORP_OTF_FILES);
    }

    #[tokio::test]
    async fn build_gorp_otf_zip_is_byte_stable_across_runs() {
        // A re-run over unchanged faces must yield identical bytes, so the
        // object only churns when the fonts themselves change.
        let source = TempDir::new().unwrap();
        stage_full_gorp_family(source.path());
        let (first, _) = build_gorp_otf_zip(source.path()).unwrap();
        let (second, _) = build_gorp_otf_zip(source.path()).unwrap();
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn build_gorp_otf_zip_refuses_a_directory_with_no_faces() {
        let source = TempDir::new().unwrap();
        fs::write(source.path().join("GORPSerif-Regular.woff2"), b"web-only").unwrap();
        let err = build_gorp_otf_zip(source.path()).unwrap_err();
        assert!(err.to_string().contains("required GORP OTF face"));
    }

    #[tokio::test]
    async fn build_gorp_otf_zip_rejects_a_partial_licensed_delivery() {
        // Every weight but one: a nonempty set must still be rejected, or the
        // upload would overwrite the canonical bundle with a partial family
        // missing a weight.
        let source = TempDir::new().unwrap();
        stage_full_gorp_family(source.path());
        fs::remove_file(source.path().join("GORPSerif-Semibold.otf")).unwrap();
        let err = build_gorp_otf_zip(source.path()).unwrap_err();
        assert!(err.to_string().contains("GORPSerif-Semibold.otf"));
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn run_upload_desktop_fonts_reports_a_missing_bucket_without_touching_the_network() {
        let dir = TempDir::new().unwrap();
        let code = run_upload_desktop_fonts(dir.path(), Some("   ".to_string()));
        assert_eq!(code, ExitCode::from(2));
    }

    #[test]
    fn cache_control_is_bounded_not_immutable() {
        // The variant URLs carry no `?v=` token, so the TTL must be
        // bounded — `immutable` would pin a stale photo forever.
        assert!(ASSET_CACHE_CONTROL.contains("max-age=604800"));
        assert!(!ASSET_CACHE_CONTROL.contains("immutable"));
    }

    #[tokio::test]
    async fn download_restores_each_variant_under_out_stripping_the_img_prefix() {
        // Seed the bucket the way `upload` keys it: `img/<slug>/<file>`,
        // plus a stray non-image key that `download` must skip.
        let store_dir = TempDir::new().unwrap();
        let storage = FsStorage::new(store_dir.path().to_path_buf())
            .await
            .unwrap();
        storage
            .put("img/lake-tahoe/lake-tahoe-400w.avif", b"avif", "image/avif")
            .await
            .unwrap();
        storage
            .put("img/lake-tahoe/lake-tahoe-400w.webp", b"webp", "image/webp")
            .await
            .unwrap();
        storage
            .put("img/lake-tahoe/lake-tahoe-400w.jpg", b"jpg", "image/jpeg")
            .await
            .unwrap();
        storage
            .put("img/lake-tahoe/notes.txt", b"junk", "text/plain")
            .await
            .unwrap();

        let out = TempDir::new().unwrap();
        let n = download(&storage, out.path()).await.unwrap();
        assert_eq!(
            n, 3,
            "the three image variants land, the stray file does not"
        );

        // The `img/` prefix is stripped; bytes round-trip under `out`.
        assert_eq!(
            fs::read(out.path().join("lake-tahoe/lake-tahoe-400w.avif")).unwrap(),
            b"avif"
        );
        assert_eq!(
            fs::read(out.path().join("lake-tahoe/lake-tahoe-400w.webp")).unwrap(),
            b"webp"
        );
        assert_eq!(
            fs::read(out.path().join("lake-tahoe/lake-tahoe-400w.jpg")).unwrap(),
            b"jpg"
        );
        // The non-image was never written.
        assert!(!out.path().join("lake-tahoe/notes.txt").exists());
    }

    #[tokio::test]
    async fn download_errors_when_the_bucket_has_no_variants() {
        let store_dir = TempDir::new().unwrap();
        let storage = FsStorage::new(store_dir.path().to_path_buf())
            .await
            .unwrap();
        let out = TempDir::new().unwrap();
        let err = download(&storage, out.path()).await.unwrap_err();
        assert!(
            err.to_string().contains("no objects under `img/`"),
            "empty bucket should guide the user, got: {err}"
        );
    }

    #[tokio::test]
    async fn download_rejects_unsafe_object_keys() {
        let storage = ListingOnlyStorage {
            keys: vec!["img/../../../etc/passwd.avif".to_string()],
        };
        let out = TempDir::new().unwrap();
        let err = download(&storage, out.path()).await.unwrap_err();
        assert!(
            err.to_string().contains("refusing unsafe object key"),
            "unsafe object key should fail before writing outside out, got: {err}"
        );
        assert!(
            !out.path().join("etc/passwd.avif").exists(),
            "unsafe key must not be written under the output directory"
        );
    }

    #[tokio::test]
    async fn download_distinguishes_non_image_objects_from_empty_bucket() {
        let store_dir = TempDir::new().unwrap();
        let storage = FsStorage::new(store_dir.path().to_path_buf())
            .await
            .unwrap();
        storage
            .put("img/lake-tahoe/notes.txt", b"junk", "text/plain")
            .await
            .unwrap();
        let out = TempDir::new().unwrap();
        let err = download(&storage, out.path()).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("none are supported image variants"),
            "non-image objects should get a precise diagnostic, got: {err}"
        );
    }

    #[tokio::test]
    async fn upload_then_download_round_trips_the_tree_byte_for_byte() {
        // Build a slug dir, upload it to an Fs-backed bucket, then pull
        // it into a fresh dir — the result is identical to the source.
        let src = TempDir::new().unwrap();
        let slug = src.path().join("lantana");
        fs::create_dir_all(&slug).unwrap();
        fs::write(slug.join("lantana-800w.avif"), b"AVIF-bytes").unwrap();
        fs::write(slug.join("lantana-800w.jpg"), b"JPEG-bytes").unwrap();

        let store_dir = TempDir::new().unwrap();
        let storage = FsStorage::new(store_dir.path().to_path_buf())
            .await
            .unwrap();
        assert_eq!(upload(&storage, src.path()).await.unwrap(), 2);

        let out = TempDir::new().unwrap();
        assert_eq!(download(&storage, out.path()).await.unwrap(), 2);
        assert_eq!(
            fs::read(out.path().join("lantana/lantana-800w.avif")).unwrap(),
            b"AVIF-bytes"
        );
        assert_eq!(
            fs::read(out.path().join("lantana/lantana-800w.jpg")).unwrap(),
            b"JPEG-bytes"
        );
    }

    #[tokio::test]
    async fn png_blog_image_uploads_and_round_trips_with_its_content_type() {
        // A hand-authored blog hero is a raw `.png` dropped straight
        // under `server/public/img/<slug>/` — not a `build` variant. It must
        // upload (keyed under `img/`, content-type `image/png`) and pull
        // back byte-for-byte so a fresh clone serves it from `/public`.
        let src = TempDir::new().unwrap();
        let slug = src.path().join("going-all-in-on-rust");
        fs::create_dir_all(&slug).unwrap();
        fs::write(slug.join("ferris.png"), b"PNG-bytes").unwrap();

        let store_dir = TempDir::new().unwrap();
        let storage = FsStorage::new(store_dir.path().to_path_buf())
            .await
            .unwrap();
        assert_eq!(upload(&storage, src.path()).await.unwrap(), 1);

        let stored = storage
            .get("img/going-all-in-on-rust/ferris.png")
            .await
            .unwrap();
        assert_eq!(stored.bytes, b"PNG-bytes");
        assert_eq!(stored.content_type, "image/png");

        let out = TempDir::new().unwrap();
        assert_eq!(download(&storage, out.path()).await.unwrap(), 1);
        assert_eq!(
            fs::read(out.path().join("going-all-in-on-rust/ferris.png")).unwrap(),
            b"PNG-bytes"
        );
    }

    #[test]
    fn no_only_flag_selects_the_whole_manifest() {
        use super::GALLERY;
        assert_eq!(select(&[]).unwrap().len(), GALLERY.len());
    }

    #[test]
    fn only_narrows_the_build_to_the_named_slugs() {
        // Adding one photo must not require every other photo's source
        // JPEG on disk, which is what the unfiltered walk demands.
        let selected = select(&["berkeley-bay".to_string()]).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].slug, "berkeley-bay");
    }

    #[test]
    fn an_unknown_only_slug_fails_and_lists_the_manifest() {
        // A typo that silently built nothing would be indistinguishable
        // from a successful build until the page 404s in production.
        let err = select(&["berkley-bay".to_string()]).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("berkley-bay"), "{message}");
        assert!(
            message.contains("berkeley-bay"),
            "names the real slug: {message}"
        );
    }
}
