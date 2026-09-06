//! `navigator document log`, `get`, and `diff` — the offline-checkout read
//! verbs for a matter document's revision chain (#485).
//!
//! All three take a pointer path below `documents/` (the committed `.yml`,
//! or the staged binary it names) and resolve the Project and the document
//! slug from the checkout itself: `navigator.yaml` at `.` names the Project,
//! and the path relative to `documents/` — the same rule
//! `navigator site sync` derives a slug from — names the document. A lawyer
//! at a checkout therefore names a file, never an id.
//!
//! Reads go through the existing Project-scoped surfaces under the caller's
//! own lens: `GET /app/api/projects/{id}/documents/revisions` for the chain
//! (added alongside this command; see `portal::api`), and the existing
//! `GET /app/projects/{code}/documents/{id}/download` route — the same one
//! the browser's Download link uses — for bytes.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use comfy_table::{presets::UTF8_FULL, Cell, ContentArrangement, Table};
use store::document_pointers::DocumentPointer;

use crate::document_sync::{read_manifest, read_pointer, slash_path};
use crate::remote::{DocumentClient, RevisionSummary, RevisionsResponse};

async fn run<F>(fut: F) -> ExitCode
where
    F: std::future::Future<Output = Result<()>>,
{
    match fut.await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("navigator: {error:#}");
            ExitCode::from(2)
        }
    }
}

/// The document slug named by a pointer path — the same path-below-`documents/`
/// rule `navigator site sync` derives a slug from, with a trailing `.yml`
/// stripped so either the committed pointer or the staged binary names the
/// same document.
fn slug_from_pointer(root: &Path, pointer: &Path) -> Result<String> {
    let documents_dir = root.join("documents");
    let absolute = if pointer.is_absolute() {
        pointer.to_path_buf()
    } else {
        root.join(pointer)
    };
    let relative = absolute.strip_prefix(&documents_dir).map_err(|_| {
        anyhow!(
            "{} is not below {}",
            pointer.display(),
            documents_dir.display()
        )
    })?;
    let slashed = slash_path(relative)?;
    Ok(slashed.strip_suffix(".yml").unwrap_or(&slashed).to_string())
}

/// The committed pointer's path for a document named by `pointer` — itself if
/// it already names the `.yml`, else that path with `.yml` appended.
fn pointer_yaml_path(pointer: &Path) -> PathBuf {
    if pointer.extension().and_then(|ext| ext.to_str()) == Some("yml") {
        pointer.to_path_buf()
    } else {
        let mut named = pointer.as_os_str().to_os_string();
        named.push(".yml");
        PathBuf::from(named)
    }
}

/// Resolve `(Project code, login host)` from `<root>/navigator.yaml`.
fn manifest_at(root: &Path) -> Result<(String, Option<String>)> {
    let manifest = read_manifest(root)?;
    Ok((manifest.project, manifest.host))
}

/// The check `log`, `get`, and the ENG-486 `navigator document verify` gate
/// step all reuse: a committed pointer's `current_version.asset_id` must
/// still be a row in the live chain. A pointer surviving a governed expunge
/// of exactly that revision, or hand-edited to name a foreign id, is drift —
/// named explicitly rather than silently falling back to whatever the live
/// chain's own newest row is.
pub(crate) fn check_pointer_drift(
    local: Option<&DocumentPointer>,
    live: &[RevisionSummary],
) -> Result<()> {
    let Some(pointer) = local else {
        return Ok(());
    };
    if live
        .iter()
        .any(|revision| revision.asset_id == pointer.current_version.asset_id)
    {
        return Ok(());
    }
    let live_ids: Vec<String> = live.iter().map(|r| r.asset_id.to_string()).collect();
    Err(anyhow!(
        "drift: the committed pointer names current revision {} (version {}), which is not in \
         the live chain (live revisions: {})",
        pointer.current_version.asset_id,
        pointer.current_version.version,
        if live_ids.is_empty() {
            "none".to_string()
        } else {
            live_ids.join(", ")
        }
    ))
}

/// Fetch the live chain and check it against the local pointer, if one is
/// committed. Shared by `log` and `get` so both fail identically on drift.
async fn revisions_for(
    root: &Path,
    pointer: &Path,
) -> Result<(String, RevisionsResponse, Option<DocumentPointer>)> {
    let (project_code, host) = manifest_at(root)?;
    let slug = slug_from_pointer(root, pointer)?;
    let client = DocumentClient::connect(host.as_deref(), &project_code).await?;
    let live = client.list_revisions(&slug).await?;
    let local = read_pointer(&pointer_yaml_path(pointer))?;
    check_pointer_drift(local.as_ref(), &live.revisions)?;
    Ok((slug, live, local))
}

/// `navigator document log <pointer>`.
pub(crate) async fn log(pointer: &Path) -> ExitCode {
    run(async {
        let root = Path::new(".");
        let (slug, live, _local) = revisions_for(root, pointer).await?;
        if live.revisions.is_empty() {
            println!("no revision of `{slug}` is visible under your lens");
            return Ok(());
        }
        let mut table = Table::new();
        table.load_style(UTF8_FULL);
        table
            .set_content_arrangement(ContentArrangement::Dynamic)
            .set_header(vec![
                "Version",
                "Operative",
                "Filename",
                "Created",
                "Size",
                "SHA-256",
                "Asset id",
            ]);
        for revision in &live.revisions {
            table.add_row(vec![
                Cell::new(revision.version),
                Cell::new(if revision.operative { "*" } else { "" }),
                Cell::new(&revision.filename),
                Cell::new(&revision.created_at),
                Cell::new(revision.size_bytes),
                Cell::new(&revision.sha256),
                Cell::new(revision.asset_id),
            ]);
        }
        println!("{slug} ({})", live.kind);
        println!("{table}");
        Ok(())
    })
    .await
}

/// Whichever revision `--version` names, or the operative one when it is
/// absent. `Err` names the available range.
fn select_version(live: &RevisionsResponse, version: Option<usize>) -> Result<&RevisionSummary> {
    match version {
        Some(wanted) => live
            .revisions
            .iter()
            .find(|revision| revision.version == wanted)
            .ok_or_else(|| {
                let available = live.revisions.iter().map(|r| r.version);
                let (min, max) = available
                    .clone()
                    .fold((usize::MAX, 0), |(lo, hi), v| (lo.min(v), hi.max(v)));
                anyhow!("version {wanted} is not visible under your lens (available: {min}..={max})")
            }),
        None => live
            .revisions
            .iter()
            .find(|revision| revision.operative)
            .ok_or_else(|| anyhow!("no revision is visible under your lens")),
    }
}

/// A path lexically resolved against `root` without touching the filesystem
/// (the destination need not exist yet), so `.` and `..` collapse the same
/// way a canonicalized path would.
fn lexical(root: &Path, path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Refuse a `get` destination inside this checkout's `documents/` tree: raw
/// legal-document bytes must never be staged for a commit by a fetch that
/// only meant to read them.
fn refuse_destination_in_documents(root: &Path, out: &Path) -> Result<()> {
    let documents_dir = lexical(root, Path::new("documents"));
    if lexical(root, out).starts_with(&documents_dir) {
        return Err(anyhow!(
            "refusing to write into {} — fetched bytes must never enter a Project repository",
            documents_dir.display()
        ));
    }
    Ok(())
}

/// `navigator document get <pointer> [--version N] --out <path>`.
pub(crate) async fn get(pointer: &Path, version: Option<usize>, out: &Path) -> ExitCode {
    run(async {
        let root = Path::new(".");
        refuse_destination_in_documents(root, out)?;
        let (_slug, live, local) = revisions_for(root, pointer).await?;
        let target = select_version(&live, version)?.clone();

        let (project_code, host) = manifest_at(root)?;
        let client = DocumentClient::connect(host.as_deref(), &project_code).await?;
        let bytes = client.download_revision(target.asset_id).await?;

        // The pointer's own `sha256` is the one guarantee that does not depend
        // on the deployment's own claim about the bytes — use it whenever the
        // fetched revision is the pointer's committed current version. Any
        // other revision has no locally trusted hash, so the metadata
        // endpoint's own claim is the best available check (it still catches
        // a truncated or corrupted download).
        let expected_sha = local
            .as_ref()
            .filter(|pointer| pointer.current_version.asset_id == target.asset_id)
            .map_or_else(
                || target.sha256.clone(),
                |pointer| pointer.current_version.sha256.clone(),
            );
        let actual_sha = store::documents::sha256_hex(&bytes);
        if actual_sha != expected_sha {
            return Err(anyhow!(
                "sha256 mismatch on revision {}: expected {expected_sha}, got {actual_sha}",
                target.asset_id
            ));
        }
        let actual_size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
        if actual_size != target.size_bytes {
            return Err(anyhow!(
                "size mismatch on revision {}: expected {} bytes, got {actual_size}",
                target.asset_id,
                target.size_bytes
            ));
        }

        std::fs::write(out, &bytes).with_context(|| format!("write {}", out.display()))?;
        println!(
            "wrote revision {} ({} bytes, sha256 {actual_sha}) to {}",
            target.version,
            bytes.len(),
            out.display()
        );
        Ok(())
    })
    .await
}

/// Extensions `diff` can extract readable text from directly (UTF-8 plain
/// text). A PDF is handled separately, via `pdf-extract`; anything else is
/// "unsupported" rather than a binary diff.
const PLAIN_TEXT_EXTENSIONS: &[&str] = &["txt", "md", "markdown", "yml", "yaml", "json", "csv"];

/// Extract readable text from one revision's bytes, or say why not.
fn extract_text(filename: &str, bytes: &[u8]) -> Result<String> {
    let extension = Path::new(filename)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_lowercase);
    match extension.as_deref() {
        Some("pdf") => pdf_extract::extract_text_from_mem(bytes).map_err(|error| {
            anyhow!("`{filename}` is a PDF `diff` could not read text from: {error}")
        }),
        Some(ext) if PLAIN_TEXT_EXTENSIONS.contains(&ext) => String::from_utf8(bytes.to_vec())
            .with_context(|| format!("`{filename}` is not valid UTF-8")),
        Some(ext) => Err(anyhow!(
            "`.{ext}` is unsupported for `diff` — only PDF and plain text are readable as a redline"
        )),
        None => Err(anyhow!(
            "`{filename}` has no extension `diff` can classify as PDF or plain text"
        )),
    }
}

/// Most lines either side of a redline may carry before `diff` refuses rather
/// than building an O(lines²) comparison table. Legal documents run to a few
/// thousand lines even at PDF-extracted granularity; this is headroom above
/// that, not a tuned ceiling.
const MAX_DIFF_LINES: usize = 20_000;

/// A minimal unified-style line diff (`-`/`+`/` ` prefix), via a textbook LCS
/// table. No workspace crate already does line diffing, and this task blessed
/// exactly one new dependency (`pdf-extract`), so the comparison itself stays
/// hand-written rather than reaching for a second.
fn line_diff(left: &str, right: &str) -> Result<String> {
    let left_lines: Vec<&str> = left.lines().collect();
    let right_lines: Vec<&str> = right.lines().collect();
    if left_lines.len() > MAX_DIFF_LINES || right_lines.len() > MAX_DIFF_LINES {
        return Err(anyhow!(
            "one side has more than {MAX_DIFF_LINES} lines; `diff` refuses rather than building \
             an oversized comparison table"
        ));
    }
    let (left_len, right_len) = (left_lines.len(), right_lines.len());
    let mut lcs = vec![vec![0usize; right_len + 1]; left_len + 1];
    for left_idx in (0..left_len).rev() {
        for right_idx in (0..right_len).rev() {
            lcs[left_idx][right_idx] = if left_lines[left_idx] == right_lines[right_idx] {
                lcs[left_idx + 1][right_idx + 1] + 1
            } else {
                lcs[left_idx + 1][right_idx].max(lcs[left_idx][right_idx + 1])
            };
        }
    }
    let mut out = String::new();
    let (mut left_idx, mut right_idx) = (0, 0);
    while left_idx < left_len && right_idx < right_len {
        if left_lines[left_idx] == right_lines[right_idx] {
            out.push_str("  ");
            out.push_str(left_lines[left_idx]);
            out.push('\n');
            left_idx += 1;
            right_idx += 1;
        } else if lcs[left_idx + 1][right_idx] >= lcs[left_idx][right_idx + 1] {
            out.push_str("- ");
            out.push_str(left_lines[left_idx]);
            out.push('\n');
            left_idx += 1;
        } else {
            out.push_str("+ ");
            out.push_str(right_lines[right_idx]);
            out.push('\n');
            right_idx += 1;
        }
    }
    for line in &left_lines[left_idx..] {
        out.push_str("- ");
        out.push_str(line);
        out.push('\n');
    }
    for line in &right_lines[right_idx..] {
        out.push_str("+ ");
        out.push_str(line);
        out.push('\n');
    }
    Ok(out)
}

/// `navigator document diff <pointer> <a> <b>` — a redline between two
/// revision numbers.
pub(crate) async fn diff(pointer: &Path, a: usize, b: usize) -> ExitCode {
    run(async {
        let root = Path::new(".");
        let (_slug, live, _local) = revisions_for(root, pointer).await?;
        let rev_a = select_version(&live, Some(a))?.clone();
        let rev_b = select_version(&live, Some(b))?.clone();

        let (project_code, host) = manifest_at(root)?;
        let client = DocumentClient::connect(host.as_deref(), &project_code).await?;
        let bytes_a = client.download_revision(rev_a.asset_id).await?;
        let bytes_b = client.download_revision(rev_b.asset_id).await?;
        let text_a = extract_text(&rev_a.filename, &bytes_a)?;
        let text_b = extract_text(&rev_b.filename, &bytes_b)?;

        println!("--- version {} ({})", rev_a.version, rev_a.filename);
        println!("+++ version {} ({})", rev_b.version, rev_b.filename);
        print!("{}", line_diff(&text_a, &text_b)?);
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::{
        check_pointer_drift, extract_text, lexical, line_diff, pointer_yaml_path,
        refuse_destination_in_documents, select_version, slug_from_pointer,
    };
    use crate::remote::{RevisionSummary, RevisionsResponse};
    use std::path::Path;
    use uuid::Uuid;

    fn revision(version: usize, asset_id: Uuid, operative: bool) -> RevisionSummary {
        let hex = asset_id.simple().to_string();
        RevisionSummary {
            version,
            asset_id,
            created_at: "2026-09-06T00:00:00Z".to_string(),
            sha256: format!("{hex}{hex}"),
            size_bytes: 10,
            filename: "notice.pdf".to_string(),
            operative,
        }
    }

    #[test]
    fn slug_derives_from_the_path_below_documents_and_strips_yml() {
        let root = Path::new("/repo");
        assert_eq!(
            slug_from_pointer(root, Path::new("documents/pleadings/motion.pdf.yml")).unwrap(),
            "pleadings/motion.pdf"
        );
        assert_eq!(
            slug_from_pointer(root, Path::new("documents/pleadings/motion.pdf")).unwrap(),
            "pleadings/motion.pdf"
        );
    }

    #[test]
    fn slug_refuses_a_path_outside_documents() {
        let root = Path::new("/repo");
        assert!(slug_from_pointer(root, Path::new("templates/foo.md")).is_err());
    }

    #[test]
    fn pointer_yaml_path_appends_yml_once() {
        assert_eq!(
            pointer_yaml_path(Path::new("documents/motion.pdf")),
            Path::new("documents/motion.pdf.yml")
        );
        assert_eq!(
            pointer_yaml_path(Path::new("documents/motion.pdf.yml")),
            Path::new("documents/motion.pdf.yml")
        );
    }

    #[test]
    fn select_version_finds_the_named_version() {
        let live = RevisionsResponse {
            kind: "agreement".to_string(),
            revisions: vec![
                revision(2, Uuid::nil(), true),
                revision(1, Uuid::max(), false),
            ],
        };
        assert_eq!(select_version(&live, Some(1)).unwrap().version, 1);
        assert_eq!(select_version(&live, None).unwrap().version, 2);
    }

    #[test]
    fn select_version_names_the_available_range_when_absent() {
        let live = RevisionsResponse {
            kind: "agreement".to_string(),
            revisions: vec![
                revision(2, Uuid::nil(), true),
                revision(1, Uuid::max(), false),
            ],
        };
        let error = select_version(&live, Some(5)).unwrap_err();
        assert!(error.to_string().contains("1..=2"), "{error}");
    }

    #[test]
    fn drift_is_silent_when_no_local_pointer_exists() {
        assert!(check_pointer_drift(None, &[]).is_ok());
    }

    #[test]
    fn drift_names_the_missing_asset_when_the_pointer_is_stale() {
        let pointer = store::document_pointers::DocumentPointer {
            kind: "agreement".to_string(),
            visibility: "internal".to_string(),
            current_version: store::document_pointers::PointerVersion {
                version: 1,
                asset_id: Uuid::nil(),
                created_at: "2026-09-06T00:00:00Z".to_string(),
                sha256: "a".repeat(64),
                size_bytes: 1,
            },
            previous_version: None,
        };
        let live = [revision(1, Uuid::max(), true)];
        let error = check_pointer_drift(Some(&pointer), &live).unwrap_err();
        assert!(error.to_string().contains("drift"), "{error}");
        assert!(
            error.to_string().contains(&Uuid::nil().to_string()),
            "{error}"
        );
    }

    #[test]
    fn drift_passes_when_the_pointers_asset_is_still_live() {
        let pointer = store::document_pointers::DocumentPointer {
            kind: "agreement".to_string(),
            visibility: "internal".to_string(),
            current_version: store::document_pointers::PointerVersion {
                version: 1,
                asset_id: Uuid::max(),
                created_at: "2026-09-06T00:00:00Z".to_string(),
                sha256: "a".repeat(64),
                size_bytes: 1,
            },
            previous_version: None,
        };
        let live = [revision(1, Uuid::max(), true)];
        assert!(check_pointer_drift(Some(&pointer), &live).is_ok());
    }

    #[test]
    fn destination_inside_documents_is_refused() {
        let root = Path::new("/repo");
        assert!(refuse_destination_in_documents(root, Path::new("documents/redline.pdf")).is_err());
        assert!(refuse_destination_in_documents(
            root,
            Path::new("/repo/documents/nested/redline.pdf")
        )
        .is_err());
    }

    #[test]
    fn destination_outside_documents_is_accepted() {
        let root = Path::new("/repo");
        assert!(refuse_destination_in_documents(root, Path::new("/tmp/redline.pdf")).is_ok());
    }

    #[test]
    fn lexical_resolves_dot_and_dot_dot_without_touching_the_filesystem() {
        let root = Path::new("/repo");
        assert_eq!(
            lexical(root, Path::new("documents/../documents/motion.pdf")),
            Path::new("/repo/documents/motion.pdf")
        );
    }

    #[test]
    fn plain_text_extracts_verbatim() {
        assert_eq!(extract_text("notice.txt", b"hello").unwrap(), "hello");
    }

    #[test]
    fn docx_is_unsupported_rather_than_a_binary_diff() {
        let error = extract_text("agreement.docx", b"PK\x03\x04").unwrap_err();
        assert!(error.to_string().contains("unsupported"), "{error}");
    }

    #[test]
    fn line_diff_marks_additions_removals_and_unchanged_lines() {
        let out = line_diff("a\nb\nc\n", "a\nx\nc\n").unwrap();
        assert!(out.contains("  a\n"));
        assert!(out.contains("- b\n"));
        assert!(out.contains("+ x\n"));
        assert!(out.contains("  c\n"));
    }
}
