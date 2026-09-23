//! The prebuilt-`navigator-lsp` target registry — the single source of
//! truth for the publisher (`cli lsp publish`) and the object keys the
//! published binaries land at in the public assets bucket.
//!
//! A target is a Rust triple plus the reader-facing executable name and
//! platform label. The object key a binary lands at in the public assets bucket
//! ([`lsp_binary_key`]) is derived from the triple, so the upload path
//! and the download link can never drift — exactly the
//! [`GALLERY`](crate::assets) / [`WIDTHS`](crate::assets) pattern, one
//! tier down. The download URL itself is
//! `asset_url(&lsp_binary_key(target))`, which resolves to the public
//! `<project>-assets` bucket in production and to `/public` in dev.

/// One distributable platform for the `navigator-lsp` binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LspTarget {
    /// Rust target triple — `cargo build --release --target <triple>`
    /// emits the matching binary, and it forms the object key.
    pub triple: &'static str,
    /// The compiled executable name for this platform.
    pub binary_name: &'static str,
    /// Reader-facing platform label for the download button.
    pub label: &'static str,
}

/// The platforms an LSP-aware editor's user runs on. A
/// `navigator lsp publish` pushes whichever of these it finds built; the
/// page renders a download button for each.
pub const LSP_TARGETS: &[LspTarget] = &[
    LspTarget {
        triple: "aarch64-apple-darwin",
        binary_name: "navigator-lsp",
        label: "macOS · Apple Silicon",
    },
    LspTarget {
        triple: "x86_64-apple-darwin",
        binary_name: "navigator-lsp",
        label: "macOS · Intel",
    },
    LspTarget {
        triple: "x86_64-unknown-linux-gnu",
        binary_name: "navigator-lsp",
        label: "Linux · x86-64",
    },
    LspTarget {
        triple: "aarch64-unknown-linux-gnu",
        binary_name: "navigator-lsp",
        label: "Linux · ARM64",
    },
    LspTarget {
        triple: "x86_64-pc-windows-msvc",
        binary_name: "navigator-lsp.exe",
        label: "Windows · x86-64",
    },
];

/// The object key (and `/public`-relative asset path) a target's binary
/// lives at: `lsp/<triple>/<binary_name>`. Stable "latest" path — a
/// re-publish overwrites it, so the publisher stamps a *bounded*
/// `Cache-Control`, never `immutable`.
#[must_use]
pub fn lsp_binary_key(target: LspTarget) -> String {
    format!("lsp/{}/{}", target.triple, target.binary_name)
}

/// One platform the tagged-release archives cover.
///
/// A strict subset of [`LSP_TARGETS`]: `.github/workflows/deploy.yml` builds
/// only three `navigator-lsp-<tag>-<archive_suffix>` archives per release
/// (one Windows, one glibc Linux, one Apple-Silicon macOS runner), each
/// attached to the GitHub Release — distinct from the five-triple "latest"
/// mirror [`lsp_binary_key`] addresses in the public assets bucket. `os` and
/// `arch` are spelled the way `std::env::consts::OS` / `std::env::consts::ARCH`
/// spell them, so a caller can match the running host directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LspReleaseArchive {
    /// `std::env::consts::OS` spelling, e.g. `"macos"`.
    pub os: &'static str,
    /// `std::env::consts::ARCH` spelling, e.g. `"aarch64"`.
    pub arch: &'static str,
    /// The matching [`LspTarget::triple`], for cross-reference.
    pub triple: &'static str,
    /// The compiled executable name inside the archive.
    pub binary_name: &'static str,
    /// The tail of the archive's filename, after `navigator-lsp-<tag>-`:
    /// `"windows.zip"`, `"linux.tar.gz"`, or `"macos.tar.gz"`.
    pub archive_suffix: &'static str,
}

/// The three platforms `deploy.yml` actually attaches a
/// `navigator-lsp-<tag>-<archive_suffix>` archive for. Naming here and in
/// `deploy.yml` must agree; [`lsp_release_asset_name`] is the one place that
/// composes the two into a filename.
pub const LSP_RELEASE_ARCHIVES: &[LspReleaseArchive] = &[
    LspReleaseArchive {
        os: "windows",
        arch: "x86_64",
        triple: "x86_64-pc-windows-msvc",
        binary_name: "navigator-lsp.exe",
        archive_suffix: "windows.zip",
    },
    LspReleaseArchive {
        os: "linux",
        arch: "x86_64",
        triple: "x86_64-unknown-linux-gnu",
        binary_name: "navigator-lsp",
        archive_suffix: "linux.tar.gz",
    },
    LspReleaseArchive {
        os: "macos",
        arch: "aarch64",
        triple: "aarch64-apple-darwin",
        binary_name: "navigator-lsp",
        archive_suffix: "macos.tar.gz",
    },
];

/// Find the release archive matching a host's `std::env::consts::{OS,ARCH}`
/// pair, or `None` when the release workflow builds nothing for it (e.g.
/// Intel macOS or ARM Linux — see [`LSP_RELEASE_ARCHIVES`]'s doc comment).
#[must_use]
pub fn lsp_release_archive_for_host(os: &str, arch: &str) -> Option<&'static LspReleaseArchive> {
    LSP_RELEASE_ARCHIVES
        .iter()
        .find(|archive| archive.os == os && archive.arch == arch)
}

/// The exact GitHub Release asset filename for `tag`:
/// `navigator-lsp-<tag>-<archive_suffix>`, matching `deploy.yml`'s
/// `gh release upload` naming.
#[must_use]
pub fn lsp_release_asset_name(tag: &str, archive: &LspReleaseArchive) -> String {
    format!("navigator-lsp-{tag}-{}", archive.archive_suffix)
}

#[cfg(test)]
mod tests {
    use super::{
        lsp_binary_key, lsp_release_archive_for_host, lsp_release_asset_name, LSP_RELEASE_ARCHIVES,
        LSP_TARGETS,
    };

    #[test]
    fn release_archive_matches_known_hosts() {
        assert_eq!(
            lsp_release_archive_for_host("macos", "aarch64")
                .expect("macos aarch64 is released")
                .archive_suffix,
            "macos.tar.gz"
        );
        assert_eq!(
            lsp_release_archive_for_host("linux", "x86_64")
                .expect("linux x86_64 is released")
                .archive_suffix,
            "linux.tar.gz"
        );
        assert_eq!(
            lsp_release_archive_for_host("windows", "x86_64")
                .expect("windows x86_64 is released")
                .archive_suffix,
            "windows.zip"
        );
    }

    #[test]
    fn release_archive_is_none_for_unbuilt_host_combinations() {
        // The release workflow builds no Intel-macOS or ARM-Linux archive
        // (see `LSP_RELEASE_ARCHIVES`'s doc comment) — an unsupported
        // combination must not fall back to a near-miss.
        assert!(lsp_release_archive_for_host("macos", "x86_64").is_none());
        assert!(lsp_release_archive_for_host("linux", "aarch64").is_none());
        assert!(lsp_release_archive_for_host("windows", "aarch64").is_none());
        assert!(lsp_release_archive_for_host("freebsd", "x86_64").is_none());
    }

    #[test]
    fn release_asset_name_matches_deploy_workflow_naming() {
        let linux = lsp_release_archive_for_host("linux", "x86_64").expect("linux archive");
        assert_eq!(
            lsp_release_asset_name("26.9.23", linux),
            "navigator-lsp-26.9.23-linux.tar.gz"
        );
        let windows = lsp_release_archive_for_host("windows", "x86_64").expect("windows archive");
        assert_eq!(
            lsp_release_asset_name("26.9.23", windows),
            "navigator-lsp-26.9.23-windows.zip"
        );
    }

    #[test]
    fn every_release_archive_triple_is_a_known_lsp_target() {
        let triples: Vec<_> = LSP_TARGETS.iter().map(|t| t.triple).collect();
        for archive in LSP_RELEASE_ARCHIVES {
            assert!(
                triples.contains(&archive.triple),
                "release archive triple {} is not in LSP_TARGETS",
                archive.triple
            );
        }
    }

    #[test]
    fn key_is_triple_scoped_under_lsp() {
        let mac = LSP_TARGETS
            .iter()
            .find(|target| target.triple == "aarch64-apple-darwin")
            .copied()
            .expect("mac target");
        assert_eq!(
            lsp_binary_key(mac),
            "lsp/aarch64-apple-darwin/navigator-lsp"
        );
    }

    #[test]
    fn windows_key_uses_exe_suffix() {
        let windows = LSP_TARGETS
            .iter()
            .find(|target| target.triple == "x86_64-pc-windows-msvc")
            .copied()
            .expect("windows target");
        assert_eq!(
            lsp_binary_key(windows),
            "lsp/x86_64-pc-windows-msvc/navigator-lsp.exe"
        );
    }

    #[test]
    fn registry_covers_mac_linux_and_windows() {
        let triples: Vec<_> = LSP_TARGETS.iter().map(|t| t.triple).collect();
        for expected in [
            "aarch64-apple-darwin",
            "x86_64-apple-darwin",
            "x86_64-unknown-linux-gnu",
            "aarch64-unknown-linux-gnu",
            "x86_64-pc-windows-msvc",
        ] {
            assert!(triples.contains(&expected), "missing target {expected}");
        }
    }

    #[test]
    fn triples_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for t in LSP_TARGETS {
            assert!(seen.insert(t.triple), "duplicate triple {}", t.triple);
        }
    }
}
