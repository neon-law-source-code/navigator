//! Guard the three `navigator-lsp` release archives, attached to the SAME
//! GitHub Release the CLI archives already publish to.
//!
//! An editor extension (Zed's official marketplace, first) needs a stable,
//! versioned asset to fetch `navigator-lsp` from. The public assets bucket
//! `cli lsp publish` targets carries only a "latest" key with no version in
//! its path — fine for the site's own use, but not what a `gh release`-based
//! extension resolves against. Reusing the CLI's existing per-platform build
//! jobs and shared publish job keeps one release meaning one version across
//! every surface, rather than adding a fourth pipeline that could drift from
//! it.

use std::fs;
use std::path::PathBuf;

fn deploy_workflow() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(".github")
        .join("workflows")
        .join("deploy.yml");
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn deploy_job(name: &str) -> serde_yaml::Value {
    let workflow: serde_yaml::Value =
        serde_yaml::from_str(&deploy_workflow()).expect("deploy.yml parses as YAML");
    workflow
        .get("jobs")
        .and_then(|jobs| jobs.get(name))
        .cloned()
        .unwrap_or_else(|| panic!("deploy.yml must define the `{name}` job"))
}

/// Every platform's build job compiles `navigator-lsp` alongside the CLI, in
/// the SAME `cargo build` invocation — one compile of the shared dependency
/// graph rather than a second cold build per platform.
#[test]
fn every_platform_build_compiles_the_lsp_binary_too() {
    let workflow = deploy_workflow();
    assert!(
        workflow.contains("cargo build --locked --release -p cli -p lsp"),
        "every CLI release build must also compile `lsp`, so one command produces both binaries"
    );
}

#[test]
fn releases_build_and_attach_a_linux_lsp_archive() {
    let workflow = deploy_workflow();

    for required in [
        "install -m 0755 target/release/navigator-lsp dist/navigator-lsp-linux/navigator-lsp",
        "install -m 0644 LICENSE dist/navigator-lsp-linux/LICENSE",
        "-C dist/navigator-lsp-linux navigator-lsp LICENSE",
        "name: navigator-linux-lsp",
        "path: dist/navigator-lsp-${{ needs.release-version.outputs.tag }}-linux.tar.gz",
        "gh release upload \"${TAG}\" dist/navigator-lsp-${TAG}-linux.tar.gz",
    ] {
        assert!(
            workflow.contains(required),
            "deploy.yml must carry the Linux `navigator-lsp` release contract `{required}`"
        );
    }
}

#[test]
fn releases_build_and_attach_a_macos_lsp_archive() {
    let workflow = deploy_workflow();

    for required in [
        "install -m 0755 target/release/navigator-lsp dist/navigator-lsp-macos/navigator-lsp",
        "install -m 0644 LICENSE dist/navigator-lsp-macos/LICENSE",
        "-C dist/navigator-lsp-macos navigator-lsp LICENSE",
        "name: navigator-macos-lsp",
        "path: dist/navigator-lsp-${{ needs.release-version.outputs.tag }}-macos.tar.gz",
        "gh release upload \"${TAG}\" dist/navigator-lsp-${TAG}-macos.tar.gz",
    ] {
        assert!(
            workflow.contains(required),
            "deploy.yml must carry the macOS `navigator-lsp` release contract `{required}`"
        );
    }
}

#[test]
fn releases_build_and_attach_a_windows_lsp_archive() {
    let workflow = deploy_workflow();

    for required in [
        "Copy-Item \"target/release/navigator-lsp.exe\"",
        "dist/navigator-lsp-windows/navigator-lsp.exe",
        "Compress-Archive -Path \"dist/navigator-lsp-windows/*\"",
        "name: navigator-windows-lsp",
        "path: dist/navigator-lsp-${{ needs.release-version.outputs.tag }}-windows.zip",
        "gh release upload \"${TAG}\" dist/navigator-lsp-${TAG}-windows.zip",
    ] {
        assert!(
            workflow.contains(required),
            "deploy.yml must carry the Windows `navigator-lsp` release contract `{required}`"
        );
    }
}

/// The publish job downloads all three LSP archives, alongside the three CLI
/// ones, before it attaches anything to the Release.
#[test]
fn the_publish_job_downloads_every_lsp_archive_before_attaching_it() {
    let publish = deploy_job("release-windows-cli-publish");
    let steps = publish["steps"]
        .as_sequence()
        .expect("release-windows-cli-publish declares steps");

    let mut downloaded = Vec::new();
    for step in steps {
        if step.get("uses").and_then(serde_yaml::Value::as_str)
            == Some("actions/download-artifact@v8")
        {
            let name = step["with"]["name"]
                .as_str()
                .expect("a download-artifact step names its artifact")
                .to_string();
            downloaded.push(name);
        }
    }

    for required in [
        "navigator-linux-lsp",
        "navigator-macos-lsp",
        "navigator-windows-lsp",
    ] {
        assert!(
            downloaded.iter().any(|name| name == required),
            "release-windows-cli-publish must download `{required}` before it can attach it, \
             got: {downloaded:?}"
        );
    }
}

/// `navigator-*` and `navigator-lsp-*` are distinct strings but not distinct
/// match sets: a shell glob `navigator-*-linux.tar.gz` also matches
/// `navigator-lsp-<tag>-linux.tar.gz`, because `*` spans the `lsp-<tag>` run.
/// While the publish step used that wildcard, each LSP archive uploaded twice
/// from one step — harmlessly, only because both `gh release upload` calls
/// passed `--clobber` with identical bytes. Every archive path under `dist/`
/// therefore names its file tag-exactly: `${TAG}` inside a `run:` block, the
/// `needs.release-version.outputs.tag` expression inside a `with:` input.
///
/// The check is a match-set check, not a string-equality one: it substitutes a
/// tag into every `dist/navigator-` path and asserts the result matches the
/// CLI archive name or the LSP archive name for its platform, never both.
#[test]
fn no_cli_archive_path_can_match_an_lsp_archive() {
    const ARCHIVE_SUFFIXES: [&str; 2] = [".tar.gz", ".zip"];
    let tag = "26.9.3";
    // Resolve every spelling of the tag first, so the `with:` expression form
    // (which carries spaces) tokenizes as one path like the shell forms do.
    let resolved_workflow = deploy_workflow()
        .replace("${{ needs.release-version.outputs.tag }}", tag)
        .replace("${TAG}", tag)
        .replace("$env:TAG", tag);
    let paths: Vec<String> = resolved_workflow
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .flat_map(|line| {
            line.split_whitespace()
                .map(|word| word.trim_matches('"').to_string())
                .filter(|word| word.contains("dist/navigator-"))
                .collect::<Vec<_>>()
        })
        // Staging directories (`dist/navigator-linux/…`) hold the members an
        // archive is packed from; only the archive filenames are uploaded. The
        // suffixes are exact-case on purpose: they are the asset names a
        // consumer downloads, not a loose file-type check.
        .filter(|path| ARCHIVE_SUFFIXES.iter().any(|suffix| path.ends_with(suffix)))
        .collect();
    assert!(
        paths.len() >= 12,
        "expected at least the six artifact paths and the six upload paths, got {paths:?}"
    );

    for path in &paths {
        assert!(
            !path.contains('*') && !path.contains('?') && !path.contains('['),
            "`{path}` is a glob; every archive path must name its file tag-exactly"
        );
        let name = path
            .rsplit('/')
            .next()
            .expect("an archive path has a filename");
        let cli = format!("navigator-{tag}-");
        let lsp = format!("navigator-lsp-{tag}-");
        assert!(
            name.starts_with(&cli) ^ name.starts_with(&lsp),
            "`{path}` resolves to `{name}`, which must name the CLI archive (`{cli}<platform>`) or the LSP archive (`{lsp}<platform>`), never both"
        );
    }
}

/// Attaching an LSP archive is not a new job with its own gate to keep in
/// sync — it rides the same publish job the CLI archives already use, so one
/// `needs` list and one `if:` gate cover all six archives.
#[test]
fn the_lsp_archives_ride_the_same_publish_job_as_the_cli() {
    let workflow = deploy_workflow();
    assert!(
        !workflow.contains("release-lsp-publish"),
        "the LSP archives must attach from the existing `release-windows-cli-publish` job, not \
         a second publish job that could drift from its gate"
    );
}
