//! Integration coverage for `navigator ops assets stub-referenced` - the
//! CI-only path that writes tiny local placeholders after the public
//! origin has already been verified.

use assert_cmd::Command;
use std::fs;
use tempfile::TempDir;
use views::assets::{GALLERY, WIDTHS};

fn expected_gallery_placeholders() -> usize {
    GALLERY.len() * WIDTHS.len() * 3
}

#[test]
fn stub_referenced_writes_the_gallery_and_licensed_fonts_without_content_images() {
    // The gallery and faces are stubbed on every run, not only when content
    // happens to reference an image: `assets verify` probes them against the
    // KIND origin unconditionally, so the bake needs them unconditionally too.
    let content = TempDir::new().unwrap();
    fs::create_dir_all(content.path().join("blog")).unwrap();
    let out = TempDir::new().unwrap();

    Command::cargo_bin("navigator")
        .unwrap()
        .args(["ops", "assets", "stub-referenced"])
        .arg("--content")
        .arg(content.path())
        .arg("--out")
        .arg(out.path())
        .assert()
        .success()
        .stdout(predicates::str::contains(format!(
            "wrote {} placeholder asset",
            expected_gallery_placeholders() + 4
        )));

    for face in ["GORPSerif-Regular.woff2", "GORPSerif-Bold.woff2"] {
        let woff2 = fs::read(out.path().join("fonts/gorp-serif").join(face)).unwrap();
        assert_eq!(&woff2[..4], b"wOF2", "{face} must be a real WOFF2");
    }

    for face in [
        "PlusJakartaSans-Regular.woff2",
        "PlusJakartaSans-Bold.woff2",
    ] {
        let woff2 = fs::read(out.path().join("fonts/plus-jakarta-sans").join(face)).unwrap();
        assert_eq!(&woff2[..4], b"wOF2", "{face} must be a real WOFF2");
    }

    let gallery = GALLERY.first().expect("the public gallery is non-empty");
    let width = WIDTHS[0];
    assert!(out
        .path()
        .join(format!(
            "img/{slug}/{slug}-{width}w.avif",
            slug = gallery.slug
        ))
        .is_file());
}

#[test]
fn stub_referenced_writes_valid_placeholder_files_at_content_paths() {
    let content = TempDir::new().unwrap();
    let blog = content.path().join("blog");
    fs::create_dir_all(&blog).unwrap();
    // Cover every format the gallery pipeline emits (AVIF/WebP/JPEG) plus a
    // hand-authored PNG, so the placeholder writer is exercised end-to-end
    // for each extension it claims to support.
    fs::write(
        blog.join("post.md"),
        "![hero](img/demo/hero.png)\n![photo](img/demo/photo.jpg)\n\
         ![webp](img/demo/photo.webp)\n![avif](img/demo/photo.avif)\n",
    )
    .unwrap();
    let out = TempDir::new().unwrap();

    Command::cargo_bin("navigator")
        .unwrap()
        .args(["ops", "assets", "stub-referenced"])
        .arg("--content")
        .arg(content.path())
        .arg("--out")
        .arg(out.path())
        .assert()
        .success()
        // Four content images, every gallery variant, plus four licensed faces
        // (two GORP, two Plus Jakarta Sans).
        .stdout(predicates::str::contains(format!(
            "wrote {} placeholder asset",
            expected_gallery_placeholders() + 8
        )));

    let png = fs::read(out.path().join("img/demo/hero.png")).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");

    let jpg = fs::read(out.path().join("img/demo/photo.jpg")).unwrap();
    assert_eq!(&jpg[..2], &[0xff, 0xd8]);

    let webp = fs::read(out.path().join("img/demo/photo.webp")).unwrap();
    assert_eq!(&webp[..4], b"RIFF");
    assert_eq!(&webp[8..12], b"WEBP");

    let avif = fs::read(out.path().join("img/demo/photo.avif")).unwrap();
    assert_eq!(&avif[4..8], b"ftyp");
    assert_eq!(&avif[8..12], b"avif");
}

#[test]
fn stub_referenced_errors_when_the_content_root_is_missing() {
    // A mistyped or absent `--content` root is an operator error, not an
    // empty gallery: the command exits non-zero rather than silently
    // writing nothing.
    let parent = TempDir::new().unwrap();
    let missing = parent.path().join("no-such-content");
    let out = TempDir::new().unwrap();

    Command::cargo_bin("navigator")
        .unwrap()
        .args(["ops", "assets", "stub-referenced"])
        .arg("--content")
        .arg(&missing)
        .arg("--out")
        .arg(out.path())
        .assert()
        .code(2)
        .stderr(predicates::str::contains("does not exist"));
}

#[test]
fn stub_referenced_reports_a_write_failure_without_aborting_the_run() {
    // If a parent path segment already exists as a file, the placeholder
    // write can't create its directory. The command must surface that per
    // reference and exit non-zero rather than panic.
    let content = TempDir::new().unwrap();
    let blog = content.path().join("blog");
    fs::create_dir_all(&blog).unwrap();
    fs::write(blog.join("post.md"), "![hero](img/demo/hero.png)\n").unwrap();
    let out = TempDir::new().unwrap();
    // `img` is a regular file, so creating `img/demo/` for the placeholder
    // fails.
    fs::write(out.path().join("img"), b"not a directory").unwrap();

    Command::cargo_bin("navigator")
        .unwrap()
        .args(["ops", "assets", "stub-referenced"])
        .arg("--content")
        .arg(content.path())
        .arg("--out")
        .arg(out.path())
        .assert()
        .code(2)
        .stderr(predicates::str::contains("could not be stubbed"));
}

#[test]
fn stub_referenced_fails_loudly_on_an_unsupported_extension() {
    // A published `img/...` reference whose extension is outside the gallery
    // formats (here `.gif`) can pass `assets verify` against the real origin
    // but has no placeholder encoder, so the stub step exits non-zero and
    // names the offending path. This pins the fail-loud contract: a new
    // content format must be a deliberate CLI change, not a silently
    // mis-served stub.
    let content = TempDir::new().unwrap();
    let blog = content.path().join("blog");
    fs::create_dir_all(&blog).unwrap();
    fs::write(blog.join("post.md"), "![logo](img/demo/logo.gif)\n").unwrap();
    let out = TempDir::new().unwrap();

    Command::cargo_bin("navigator")
        .unwrap()
        .args(["ops", "assets", "stub-referenced"])
        .arg("--content")
        .arg(content.path())
        .arg("--out")
        .arg(out.path())
        .assert()
        .code(2)
        .stderr(predicates::str::contains("unsupported asset extension"))
        .stderr(predicates::str::contains("could not be stubbed"));
}

#[test]
fn stub_referenced_rejects_unsafe_content_paths() {
    let content = TempDir::new().unwrap();
    let blog = content.path().join("blog");
    fs::create_dir_all(&blog).unwrap();
    fs::write(blog.join("post.md"), "![hero](img/../escape.jpg)\n").unwrap();
    let out = TempDir::new().unwrap();

    Command::cargo_bin("navigator")
        .unwrap()
        .args(["ops", "assets", "stub-referenced"])
        .arg("--content")
        .arg(content.path())
        .arg("--out")
        .arg(out.path())
        .assert()
        .code(2)
        .stderr(predicates::str::contains("refusing unsafe asset reference"));
}
