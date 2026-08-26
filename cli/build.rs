//! Bake the published release tag into the `navigator` binary.
//!
//! `deploy.yml` builds the downloadable CLI from an immutable release Git tag and
//! exposes it to `cargo build` as `NAVIGATOR_RELEASE_TAG`. We capture that at
//! build time and re-export it as `NAVIGATOR_CLI_VERSION`, which `main.rs`
//! reads with `env!`. This is what makes a *downloaded* release binary report
//! its release with no environment set — the runtime `NAVIGATOR_RELEASE_TAG`
//! override in `main.rs` still wins when present. On a plain local build the
//! tag is unset and we fall back to the workspace crate version: `0.1.0`
//! between releases, or the release tag `navigator ops release-version` stamped
//! into the tagged commit, so a build of released source reports its release
//! even with no env var set.
//!
//! That fallback is a fine answer to "what version is this?", but it is not
//! proof this repository ever published it — `[workspace.package].version` is
//! bumped on `main` before `deploy.yml`'s tag job creates the matching Git tag,
//! so a plain local build taken in that window reports a version that names no
//! release yet. We also bake `NAVIGATOR_CLI_VERSION_IS_RELEASE`, set only when
//! `NAVIGATOR_RELEASE_TAG` was actually present at build time, so callers that
//! need a *trustworthy* release version (not merely a version-shaped one) — see
//! `cli::main::published_cli_version`, which `projects repository scaffold`
//! pins its generated gate to — can tell the two cases apart.

use std::env;

fn main() {
    // Rebuild when the release tag changes so a re-tag re-bakes the version.
    println!("cargo:rerun-if-env-changed=NAVIGATOR_RELEASE_TAG");
    // Emitting any rerun-if directive opts out of Cargo's package-wide file
    // scan, so also watch the workspace manifest — that is where the fallback
    // `version` lives (`version.workspace = true`), and a bump there must
    // re-bake the baked `CARGO_PKG_VERSION` instead of leaving it stale.
    println!("cargo:rerun-if-changed=../Cargo.toml");

    let release_tag = env::var("NAVIGATOR_RELEASE_TAG")
        .ok()
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty());

    let version = release_tag.clone().unwrap_or_else(|| {
        // CARGO_PKG_VERSION is always set for a build script.
        env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION is set by cargo")
    });
    println!("cargo:rustc-env=NAVIGATOR_CLI_VERSION={version}");

    if release_tag.is_some() {
        println!("cargo:rustc-env=NAVIGATOR_CLI_VERSION_IS_RELEASE=1");
    }
}
