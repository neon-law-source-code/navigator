//! Where the public reads the `navigator` CLI from, and at which version.
//!
//! Every release attaches its archives to a **public GitHub Release**, so
//! `/navigator` links those bytes directly. `BUSL-1.1` restricts production
//! *use* rather than distribution, so the archives stay downloadable by anyone.
//!
//! # The version is the deployment's own release tag
//!
//! `deploy.yml`'s `release-version` job refuses any tag whose value differs from
//! `[workspace.package].version`, so three things that could drift are the same
//! string by gate: the pushed Git tag, the manifest, and `NAVIGATOR_RELEASE_TAG`
//! as `images/Containerfile.neon` bakes it into the image. [`release_version`]
//! therefore prefers the environment — that is the *deployed* answer, and the
//! only one that stays true when a container outlives its source tree — and
//! falls back to the manifest version compiled in, which is what makes a local
//! `cargo run` link real archives instead of rendering a dead `unknown` URL.
//!
//! # What is published, and what is not
//!
//! Three archives per release, one per platform in [`PLATFORMS`]. macOS is
//! **arm64 only** and Linux is **`x86_64` glibc only** — `deploy.yml` builds no
//! Intel Mac and no arm64 Linux archive, so each box says which machine it is
//! for rather than letting a reader discover it from a binary that will not run.
//!
//! Homebrew is the recommended macOS route, and not merely as a convenience.
//! The released Mach-O is unsigned and unnotarized: a browser download carries
//! `com.apple.quarantine`, which Gatekeeper refuses outright, while `brew`
//! fetches with `curl` and sets no such attribute — the same bytes run. Until
//! Developer ID signing lands, the macOS box has to name the tap.

use crate::components::PlatformMark;

/// The release download root of the public repository.
///
/// A GitHub Release asset URL is `…/releases/download/{tag}/{filename}`, and the
/// tag is the version — so a reader who mistrusts the page can check the link
/// against the Releases page and see the same two strings.
pub const RELEASE_DOWNLOAD_BASE: &str =
    "https://github.com/neon-law-source-code/navigator/releases/download";

/// The Releases index, for a reader who wants an older version or the notes.
pub const RELEASES_HREF: &str = "https://github.com/neon-law-source-code/navigator/releases";

/// The tap-qualified formula. `neon-law-source-code/navigator` is a **separate
/// repository** (`homebrew-navigator`), which is why the formula is named with
/// its tap rather than bare: `brew` resolves the tap on first install.
pub const HOMEBREW_FORMULA: &str = "neon-law-source-code/navigator/navigator";

/// The one command that installs the CLI on macOS.
pub const HOMEBREW_INSTALL_COMMAND: &str = "brew install neon-law-source-code/navigator/navigator";

/// The command that moves an installed CLI to the current release.
pub const HOMEBREW_UPGRADE_COMMAND: &str = "brew upgrade neon-law-source-code/navigator/navigator";

/// One platform a release publishes an archive for.
pub struct PublicPlatform {
    /// The word that appears in the archive filename.
    pub slug: &'static str,
    /// What the box calls it.
    pub label: &'static str,
    /// Which machine the archive actually runs on. On the page, not in a
    /// footnote: the release builds one architecture per platform.
    pub detail: &'static str,
    /// The archive's extension, which differs by platform.
    pub extension: &'static str,
    /// The line mark the box opens on.
    pub mark: PlatformMark,
}

/// The three archives a release publishes, **in the order the page shows them**:
/// Linux, then macOS in the middle, then Windows.
///
/// The order is the layout. `home.css` lays the boxes out in three explicit
/// columns, so this slice is what puts macOS between the other two rather than a
/// separate index — and macOS earns the middle: it is the platform most of the
/// firm's readers are on, and the one whose box carries the Homebrew route.
///
/// Every field pairs with something in `deploy.yml`: `slug` and `extension`
/// compose the filename its `release-cli-build-*` jobs write, and `detail`
/// states the architecture each job's own comment records.
pub const PLATFORMS: &[PublicPlatform] = &[
    PublicPlatform {
        slug: "linux",
        label: "Linux",
        detail: "x86_64 · glibc · tar.gz",
        extension: "tar.gz",
        mark: PlatformMark::Terminal,
    },
    PublicPlatform {
        slug: "macos",
        label: "macOS",
        detail: "Apple silicon · tar.gz",
        extension: "tar.gz",
        mark: PlatformMark::Laptop,
    },
    PublicPlatform {
        slug: "windows",
        label: "Windows",
        detail: "x86_64 · zip",
        extension: "zip",
        mark: PlatformMark::Window,
    },
];

/// The release this deployment runs, and the version its download links name.
///
/// `NAVIGATOR_RELEASE_TAG` first: it is what the image was built with, so it is
/// the honest answer for a running container. `unknown` is
/// `images/Containerfile.neon`'s own default for a build that was handed no
/// `--build-arg`, so it is treated as absent rather than published as a version.
///
/// The fallback is the compiled-in manifest version, which the release gate pins
/// equal to the tag. It cannot be wrong in a way the environment would have
/// fixed: a source tree at `26.8.20` has no other release to point at.
#[cfg(feature = "server")]
#[must_use]
pub fn release_version() -> String {
    std::env::var("NAVIGATOR_RELEASE_TAG")
        .ok()
        .filter(|tag| !tag.is_empty() && tag != "unknown")
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
}

/// The archive filename one platform publishes for `version`.
///
/// Must match what `deploy.yml` packages and `gh release upload` attaches. The
/// release workflow attaches. The tests below pin that contract.
#[must_use]
pub fn asset_filename(version: &str, platform: &PublicPlatform) -> String {
    format!(
        "navigator-{version}-{}.{}",
        platform.slug, platform.extension
    )
}

/// The public download URL for one platform's archive at `version`.
#[must_use]
pub fn asset_href(version: &str, platform: &PublicPlatform) -> String {
    format!(
        "{RELEASE_DOWNLOAD_BASE}/{version}/{}",
        asset_filename(version, platform)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three URLs the page publishes, spelled out against a fixed version.
    ///
    /// Written as literals rather than composed a second way: a test that
    /// rebuilds the URL with the same `format!` proves only that `format!` is
    /// deterministic. These strings are what a reader's browser asks GitHub for,
    /// and they have to match `deploy.yml`'s `gh release upload` globs
    /// (`navigator-*-linux.tar.gz`, `navigator-*-macos.tar.gz`,
    /// `navigator-*-windows.zip`) exactly.
    #[test]
    fn the_download_urls_match_the_names_the_release_attaches() {
        let hrefs: Vec<String> = PLATFORMS
            .iter()
            .map(|platform| asset_href("26.8.20", platform))
            .collect();
        assert_eq!(
            hrefs,
            vec![
                "https://github.com/neon-law-source-code/navigator/releases/download/26.8.20/navigator-26.8.20-linux.tar.gz",
                "https://github.com/neon-law-source-code/navigator/releases/download/26.8.20/navigator-26.8.20-macos.tar.gz",
                "https://github.com/neon-law-source-code/navigator/releases/download/26.8.20/navigator-26.8.20-windows.zip",
            ]
        );
    }

    /// A `-hotfix.N` tag is a legal release version, and it reaches the URL
    /// verbatim. The prerelease suffix contains a `.` and a `-`, so a filename
    /// composed by splitting or trimming the version would quietly produce a
    /// 404 on exactly the release someone is in a hurry to install.
    #[test]
    fn a_hotfix_version_reaches_the_url_verbatim() {
        let linux = &PLATFORMS[0];
        assert_eq!(
            asset_filename("26.8.20-hotfix.4", linux),
            "navigator-26.8.20-hotfix.4-linux.tar.gz"
        );
        assert!(asset_href("26.8.20-hotfix.4", linux)
            .ends_with("/26.8.20-hotfix.4/navigator-26.8.20-hotfix.4-linux.tar.gz"));
    }

    /// Linux, macOS, Windows — the order the boxes appear in, with macOS in the
    /// middle. The slice *is* the layout, so a reordering here silently moves
    /// the boxes on the page; this is what notices.
    #[test]
    fn macos_sits_between_linux_and_windows() {
        let labels: Vec<&str> = PLATFORMS.iter().map(|p| p.label).collect();
        assert_eq!(labels, vec!["Linux", "macOS", "Windows"]);
    }

    /// Each box states its architecture. The release builds one per platform,
    /// so a reader on an Intel Mac or an arm64 Linux box has to be able to see
    /// that this archive is not for them before they download it.
    #[test]
    fn every_platform_states_which_machine_its_archive_runs_on() {
        for platform in PLATFORMS {
            assert!(
                !platform.detail.is_empty(),
                "{} states its architecture",
                platform.label
            );
        }
        assert!(
            PLATFORMS[1].detail.contains("Apple silicon"),
            "the macOS archive is arm64 only, and says so: {}",
            PLATFORMS[1].detail
        );
    }

    /// The formula is tap-qualified. A bare `brew install navigator` resolves
    /// against homebrew-core, which has no such formula — so the tap has to be
    /// in the command a reader copies.
    #[test]
    fn the_homebrew_command_names_the_tap() {
        assert!(
            HOMEBREW_INSTALL_COMMAND.ends_with(HOMEBREW_FORMULA),
            "the install command installs the tap-qualified formula: {HOMEBREW_INSTALL_COMMAND}"
        );
        assert_eq!(HOMEBREW_FORMULA.split('/').count(), 3, "owner/tap/formula");
        for command in [HOMEBREW_INSTALL_COMMAND, HOMEBREW_UPGRADE_COMMAND] {
            assert!(
                command.starts_with("brew "),
                "a reader pastes this into a shell: {command}"
            );
        }
    }

    /// With no release stamped, the version is the manifest's — never the
    /// `unknown` the image defaults to, which would compose a URL that 404s.
    ///
    /// The variable is read at call time and this test does not set it, so it
    /// asserts what it can without racing a sibling test over process
    /// environment: whatever `release_version` returns, it is a version and not
    /// the sentinel.
    #[cfg(feature = "server")]
    #[test]
    fn the_version_is_never_the_unknown_sentinel() {
        let version = release_version();
        assert_ne!(version, "unknown");
        assert!(!version.is_empty());
        assert!(
            version.starts_with(|c: char| c.is_ascii_digit()),
            "a version opens on a digit: {version}"
        );
    }
}
