//! `navigator notations export` — write the notation catalog compiled into
//! this binary out to a directory.
//!
//! The catalog already travels inside `navigator`: `portal::template_api`
//! embeds the repository's `templates/` tree at build time, and the binary
//! links it. Until this command there was no way to get it back out, so
//! sharing the firm's templates meant sharing a checkout. Here the binary
//! itself is the distribution: a lawyer with `navigator` on their machine
//! can lay the catalog down anywhere, with no git access and no network.
//!
//! What lands is the tree as shipped — Markdown, plus the `.fields` and
//! `.sha256` manifests a vendored government form travels with — so the
//! export is a working catalog rather than a pile of prose.
//!
//! Existing files are left alone unless `--force` is passed. An export is a
//! convenience, and a convenience must not be the thing that overwrites an
//! edit someone has not committed yet.

use std::path::Path;

use anyhow::{Context, Result};

/// Write the bundled catalog under `out`, preserving its layout.
///
/// Returns the number of files written and the number left in place.
pub fn run(out: &Path, force: bool) -> Result<()> {
    let (written, skipped) = export(out, force)?;

    eprintln!("==> wrote {written} file(s)");
    if skipped > 0 {
        eprintln!("    {skipped} left in place; --force overwrites them");
    }
    eprintln!(
        "    the catalog carries the firm's confidential templates — treat what you \
         just wrote as work product"
    );
    Ok(())
}

/// The write itself, split out so a test can assert what landed without
/// reading the printed summary.
fn export(out: &Path, force: bool) -> Result<(usize, usize)> {
    if out.is_file() {
        anyhow::bail!(
            "{} is a file — pass a directory to export into",
            out.display()
        );
    }

    let mut written = 0;
    let mut skipped = 0;
    for (relative, bytes) in portal::template_api::bundled_files() {
        let dest = out.join(relative);
        if dest.exists() && !force {
            skipped += 1;
            continue;
        }
        let parent = dest
            .parent()
            .with_context(|| format!("{} has no parent directory", dest.display()))?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
        std::fs::write(&dest, bytes).with_context(|| format!("writing {}", dest.display()))?;
        written += 1;
    }
    Ok((written, skipped))
}

#[cfg(test)]
mod tests {
    use super::export;

    /// The export reproduces the catalog's layout, not a flattened dump: a
    /// template's path is how every other Navigator surface names it.
    #[test]
    fn writes_the_catalog_under_its_own_relative_layout() {
        let dir = tempfile::tempdir().expect("tempdir");

        let (written, skipped) = export(dir.path(), false).expect("export");

        assert!(written > 0);
        assert_eq!(skipped, 0);
        let letter = dir.path().join("notations/neon_law/onboarding.md");
        let body = std::fs::read_to_string(&letter).expect("the onboarding letter landed");
        assert!(body.contains("code: onboarding__letter"));
        assert!(
            dir.path()
                .join("notations/forms/united_states/federal/irs/us__form_990.md")
                .is_file(),
            "a nested government form keeps its shelf"
        );
    }

    /// A second export is not allowed to eat an uncommitted edit.
    #[test]
    fn leaves_an_existing_file_alone_until_force() {
        let dir = tempfile::tempdir().expect("tempdir");
        export(dir.path(), false).expect("first export");
        let letter = dir.path().join("notations/neon_law/onboarding.md");
        std::fs::write(&letter, "mine\n").expect("local edit");

        let (written, skipped) = export(dir.path(), false).expect("second export");
        assert_eq!(written, 0);
        assert!(skipped > 0);
        assert_eq!(std::fs::read_to_string(&letter).unwrap(), "mine\n");

        let (forced, skipped) = export(dir.path(), true).expect("forced export");
        assert!(forced > 0);
        assert_eq!(skipped, 0);
        assert!(std::fs::read_to_string(&letter)
            .unwrap()
            .contains("code: onboarding__letter"));
    }

    /// A path that is already a file is refused rather than half-written.
    #[test]
    fn refuses_a_destination_that_is_a_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("catalog");
        std::fs::write(&file, "not a directory").expect("write");

        let err = export(&file, false).expect_err("a file destination is refused");

        assert!(err.to_string().contains("pass a directory"), "{err}");
    }
}
