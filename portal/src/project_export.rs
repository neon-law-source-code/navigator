//! Client "download all my documents" — a ZIP of the matter's current
//! files.
//!
//! `GET /app/projects/{project_code}/documents.zip` streams a plain ZIP built
//! from the durable system of record: the matter's document `assets`
//! rows and their bytes in [`cloud::StorageService`] — the same source
//! the per-document download reads, GCS in prod and Garage locally. It
//! is **not** built from a git volume the deployed `web` tier may not
//! mount (see #542). This is the client council's "get my files out
//! cleanly": never a git packfile or bundle, and **no git jargon
//! reaches the client** — the URL and the archive are about *documents*,
//! not repositories.
//!
//! # Authorization
//!
//! Row-scoped by the caller's tier and their participation row, never by
//! the URL: a firm tier archives every asset, a client only the ones marked
//! client-visible. A non-participant gets `404` — the matter "doesn't
//! exist" for them — never `403`.

use std::collections::HashSet;
use std::io::Write;

use axum::body::Body;
use axum::extract::{Extension, Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use store::persons::Role;
use zip::write::SimpleFileOptions;

use crate::admin::AdminState;
use crate::session::SessionData;
use store::access::ProjectLens;

/// `GET /app/projects/{project_code}/documents.zip`.
pub async fn download_all(
    State(state): State<AdminState>,
    Path(project_code): Path<String>,
    session: Option<Extension<SessionData>>,
) -> Response {
    let Some(project_id) = store::projects::id_for_code(&state.surreal, &project_code).await else {
        return not_found();
    };
    let (person_id, role) = match session.as_deref() {
        Some(s) => (s.person_id, s.role),
        None => (None, Role::Client),
    };
    // Gate on tier + participation; the tier then picks which bytes ship.
    let lens = ProjectLens::for_role(role);
    match store::access::can_see_project(&state.surreal, person_id, role, project_id).await {
        Ok(true) => {}
        Ok(false) => return not_found(),
        Err(e) => {
            tracing::error!(error = %e, %project_id, "documents.zip: can_see_project failed");
            return internal_error();
        }
    }
    let Ok(Some(proj)) = store::projects::find_by_id(&state.surreal, project_id).await else {
        return not_found();
    };

    // The durable index: every document filed to this matter. The store is
    // the authorization and provenance source even though document keys are
    // Project-scoped: it resolves the exact asset revisions and applies the
    // client visibility lens. A bare content asset has `project_id IS NULL`
    // and never matches this filter.
    //
    // The client lens additionally gates on `visibility` — this archive
    // hands back full bytes, so it carries the same #782 exposure the
    // portal matter-detail listing did if left ungated (internal work
    // product, `unclassified` lawyer/email uploads). The lawyer lens is
    // unfiltered.
    let assets = match store::assets::for_project(&state.surreal, project_id).await {
        // `for_project` reads newest-first; the archive lists oldest-first,
        // which is insertion order reversed.
        Ok(rows) => {
            let mut rows: Vec<_> = rows
                .into_iter()
                .filter(|a| {
                    lens != ProjectLens::Client
                        || a.visibility == store::documents::visibility::CLIENT
                })
                .collect();
            rows.reverse();
            rows
        }
        Err(e) => {
            tracing::error!(error = %e, %project_id, "documents.zip: asset query failed");
            return internal_error();
        }
    };

    // Fetch each blob and pair it with a safe, unique entry name.
    let mut used: HashSet<String> = HashSet::new();
    let mut files: Vec<(String, Vec<u8>)> = Vec::with_capacity(assets.len());
    for a in &assets {
        let bytes = match state.storage.get(&a.storage_key).await {
            Ok(obj) => obj.bytes,
            // A governed expunge deletes a document's bytes but keeps its
            // `assets` row for audit (`portal::expunge`). A gone blob is that
            // one document's own absence, not a broken export — skip it so
            // the client still receives their remaining documents (the
            // per-document download likewise 404s a single expunged doc).
            Err(cloud::StorageError::NotFound(_)) => {
                tracing::info!(
                    %project_id, asset_id = %a.id,
                    "documents.zip: blob absent (expunged?), skipping entry",
                );
                continue;
            }
            // Any other storage error is a real fault — surface it as a 500,
            // never mask a misconfiguration as an empty archive.
            Err(e) => {
                tracing::error!(
                    error = %e, %project_id, asset_id = %a.id, storage_key = %a.storage_key,
                    "documents.zip: storage fetch failed",
                );
                return internal_error();
            }
        };
        files.push((unique_entry_name(a.filename.as_deref(), &mut used), bytes));
    }

    let zip_bytes = match build_zip(&files) {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::error!(error = %e, %project_id, "documents.zip: zip build failed");
            return internal_error();
        }
    };

    let download_name = format!("{}-documents.zip", filename_slug(&proj.name));
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/zip")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{download_name}\""),
        )
        .body(Body::from(zip_bytes))
        .unwrap_or_else(|_| internal_error())
}

/// Package `(path, bytes)` pairs into an in-memory ZIP, deflate-compressed.
fn build_zip(files: &[(String, Vec<u8>)]) -> zip::result::ZipResult<Vec<u8>> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut cursor);
        let opts =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (path, bytes) in files {
            zip.start_file(path, opts)?;
            zip.write_all(bytes)?;
        }
        zip.finish()?;
    }
    Ok(cursor.into_inner())
}

/// Turn an asset's stored filename into a safe, unique ZIP entry name,
/// recording it in `used`. Two guards:
///
/// 1. **No path escape.** The filename comes from document ingestion and
///    is otherwise written verbatim into the archive, so a crafted name
///    like `../../etc/passwd` would let an unsafe extractor write outside
///    its destination (zip-slip). Strip every directory component first.
/// 2. **No collision.** Two documents can share a `filename` — and even a
///    `storage_key`, since bytes are deduped — and a de-collided name can
///    itself equal a later original (`report.pdf`, `report (2).pdf`,
///    `report.pdf`). Append ` (n)` before the extension, advancing `n`
///    until the name is unique among *all* entries already emitted, so no
///    two entries ever share a name.
fn unique_entry_name(filename: Option<&str>, used: &mut HashSet<String>) -> String {
    let base = sanitize_filename(filename);
    if used.insert(base.clone()) {
        return base;
    }
    let (stem, ext) = split_extension(&base);
    let mut n = 2u32;
    loop {
        let candidate = if ext.is_empty() {
            format!("{stem} ({n})")
        } else {
            format!("{stem} ({n}).{ext}")
        };
        if used.insert(candidate.clone()) {
            return candidate;
        }
        n += 1;
    }
}

/// Basename of a caller-supplied filename with any `/` or `\` path
/// components stripped, so it can never escape the archive root. Falls
/// back to `document` for an empty, missing, or dot-only name.
fn sanitize_filename(filename: Option<&str>) -> String {
    let raw = filename.unwrap_or("document");
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw).trim();
    if base.is_empty() || base == "." || base == ".." {
        "document".to_string()
    } else {
        base.to_string()
    }
}

/// Split a filename into `(stem, extension)` on the last `.`. The
/// extension is empty for a name with no dot or a leading-dot dotfile
/// (`.env` → (`.env`, "")), so the ` (n)` suffix appends rather than
/// splitting the dotfile.
fn split_extension(name: &str) -> (&str, &str) {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, ext),
        _ => (name, ""),
    }
}

/// Turn a matter name into a safe download-filename stem: lowercase,
/// alphanumerics kept, every run of anything else collapsed to a single
/// `-`. Empty names fall back to `matter`.
fn filename_slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "matter".to_string()
    } else {
        trimmed.to_string()
    }
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        webapp::error_pages::not_found_signed_in(),
    )
        .into_response()
}

fn internal_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        webapp::error_pages::server_error(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{build_zip, filename_slug, sanitize_filename, unique_entry_name};

    #[test]
    fn unique_entry_name_suffixes_repeats_before_the_extension() {
        let mut used: HashSet<String> = HashSet::new();
        assert_eq!(
            unique_entry_name(Some("report.pdf"), &mut used),
            "report.pdf"
        );
        assert_eq!(
            unique_entry_name(Some("report.pdf"), &mut used),
            "report (2).pdf"
        );
        // A distinct name is untouched.
        assert_eq!(unique_entry_name(Some("trust.pdf"), &mut used), "trust.pdf");
        // Extensionless and dotfile names fall back to a trailing suffix.
        assert_eq!(unique_entry_name(Some("notes"), &mut used), "notes");
        assert_eq!(unique_entry_name(Some("notes"), &mut used), "notes (2)");
        assert_eq!(unique_entry_name(Some(".env"), &mut used), ".env");
        assert_eq!(unique_entry_name(Some(".env"), &mut used), ".env (2)");
        // A missing filename is named `document`.
        assert_eq!(unique_entry_name(None, &mut used), "document");
    }

    #[test]
    fn unique_entry_name_never_collides_when_a_generated_name_reappears() {
        // A de-collided name can equal a *later* original: the third entry
        // here would generate `report (2).pdf`, which the second already
        // took, so it must advance to `report (3).pdf` — never a duplicate.
        let mut used: HashSet<String> = HashSet::new();
        assert_eq!(
            unique_entry_name(Some("report.pdf"), &mut used),
            "report.pdf"
        );
        assert_eq!(
            unique_entry_name(Some("report (2).pdf"), &mut used),
            "report (2).pdf"
        );
        assert_eq!(
            unique_entry_name(Some("report.pdf"), &mut used),
            "report (3).pdf"
        );
    }

    #[test]
    fn sanitize_filename_strips_path_components() {
        // Zip-slip guard: a crafted filename must not carry a path into the
        // archive — only the basename survives.
        assert_eq!(sanitize_filename(Some("../../etc/passwd")), "passwd");
        assert_eq!(sanitize_filename(Some("a/b/c.pdf")), "c.pdf");
        assert_eq!(sanitize_filename(Some("evil\\..\\x.pdf")), "x.pdf");
        assert_eq!(sanitize_filename(Some("..")), "document");
        assert_eq!(sanitize_filename(Some("/")), "document");
        assert_eq!(sanitize_filename(Some("")), "document");
        assert_eq!(sanitize_filename(None), "document");
        assert_eq!(sanitize_filename(Some("plain.pdf")), "plain.pdf");
    }

    #[test]
    fn slug_is_filename_safe() {
        assert_eq!(filename_slug("Libra Estate Plan"), "libra-estate-plan");
        assert_eq!(filename_slug("Acme, LLC — Formation"), "acme-llc-formation");
        assert_eq!(filename_slug("   "), "matter");
    }

    #[test]
    fn build_zip_round_trips_paths_and_bytes() {
        let files = vec![
            ("will.txt".to_string(), b"the will".to_vec()),
            ("folder/trust.pdf".to_string(), b"trust bytes".to_vec()),
        ];
        let bytes = build_zip(&files).unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(archive.len(), 2);
        let mut names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        names.sort();
        assert_eq!(names, vec!["folder/trust.pdf", "will.txt"]);
    }
}
