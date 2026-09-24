//! `navigator lsp` — download the platform-matching `navigator-lsp` release
//! archive for this binary's own version and write a working
//! `navigator-lsp` executable into the caller's Downloads directory.
//!
//! Distinct from `navigator ops lsp publish` (`cli/src/lsp_publish.rs`),
//! which is the operator upload side that mirrors a "latest" key to the
//! public assets bucket for the site's own use — not a resolvable target
//! for a human who just wants the binary. This command instead resolves the
//! exact `navigator-lsp-<tag>-<platform>` archive
//! `.github/workflows/deploy.yml` attaches to the GitHub Release for the
//! *running CLI's own tag* (never "latest"), via
//! [`views::lsp::LSP_RELEASE_ARCHIVES`] — the same metadata an editor
//! extension (ENG-447) resolves against at runtime, so the mapping cannot
//! drift between the two.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use views::lsp::{lsp_release_archive_for_host, lsp_release_asset_name, LspReleaseArchive};

/// The repository release archives are attached to.
const RELEASES_REPO: &str = "neon-law-source-code/navigator";

/// `api.github.com`'s own base. A test overrides this with a `wiremock`
/// server's address so no test makes a live network call.
const GITHUB_API_BASE: &str = "https://api.github.com";

/// Entry point for the top-level `navigator lsp` command. `tag` is the
/// running binary's own version (`crate::cli_version()`); `dir` overrides
/// the resolved Downloads directory.
pub fn run_download(tag: &str, dir: Option<PathBuf>) -> ExitCode {
    let Some(archive) = lsp_release_archive_for_host(std::env::consts::OS, std::env::consts::ARCH)
    else {
        eprintln!(
            "navigator: lsp: no navigator-lsp release archive is built for this platform \
             ({os}/{arch}) — see docs/lsp/README.md",
            os = std::env::consts::OS,
            arch = std::env::consts::ARCH,
        );
        return ExitCode::from(2);
    };
    let dest_dir = match dir.map_or_else(
        || {
            default_downloads_dir(
                std::env::var("HOME").ok(),
                std::env::var("USERPROFILE").ok(),
            )
        },
        Ok,
    ) {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("navigator: lsp: {e:#}");
            return ExitCode::from(2);
        }
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("navigator: lsp: tokio runtime: {e}");
            return ExitCode::from(2);
        }
    };
    let client = reqwest::Client::new();
    match runtime.block_on(download(
        &client,
        GITHUB_API_BASE,
        RELEASES_REPO,
        tag,
        archive,
        &dest_dir,
    )) {
        Ok(path) => {
            println!("navigator: lsp: wrote {}", path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("navigator: lsp: {e:#}");
            ExitCode::from(2)
        }
    }
}

/// Resolve the Downloads directory: `$HOME/Downloads`, or
/// `%USERPROFILE%\Downloads` when `$HOME` is unset (Windows). Pure, taking
/// the two env values as arguments, so the precedence is unit-testable
/// without mutating process-global env — the same pattern
/// `credentials::resolve_credentials_path` uses.
fn default_downloads_dir(home: Option<String>, userprofile: Option<String>) -> Result<PathBuf> {
    let base = home
        .filter(|h| !h.is_empty())
        .or_else(|| userprofile.filter(|p| !p.is_empty()))
        .ok_or_else(|| {
            anyhow!("cannot resolve a home directory ($HOME/%USERPROFILE% are both unset) to find Downloads")
        })?;
    Ok(PathBuf::from(base).join("Downloads"))
}

#[derive(Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Deserialize)]
struct Release {
    assets: Vec<ReleaseAsset>,
}

/// Look up the exact asset download URL for `tag`. Every failure names the
/// tag and asset it looked for — never a silent fallback to another
/// release, per ENG-867's acceptance criteria.
async fn find_asset_url(
    client: &reqwest::Client,
    api_base: &str,
    repo: &str,
    tag: &str,
    asset_name: &str,
) -> Result<String> {
    let url = format!("{api_base}/repos/{repo}/releases/tags/{tag}");
    let response = client
        .get(&url)
        .header("User-Agent", "navigator-cli")
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(anyhow!("no GitHub Release found for tag `{tag}` in {repo}"));
    }
    let response = response
        .error_for_status()
        .with_context(|| format!("GET {url}"))?;
    let release: Release = response
        .json()
        .await
        .with_context(|| format!("parse the `{tag}` release from {repo}"))?;
    release
        .assets
        .into_iter()
        .find(|asset| asset.name == asset_name)
        .map(|asset| asset.browser_download_url)
        .ok_or_else(|| anyhow!("release `{tag}` in {repo} has no asset named `{asset_name}` — looked for {asset_name}"))
}

/// Download `tag`'s platform-matching archive, extract the `navigator-lsp`
/// executable from it, and write it into `dest_dir`. Returns the written
/// path.
async fn download(
    client: &reqwest::Client,
    api_base: &str,
    repo: &str,
    tag: &str,
    archive: &LspReleaseArchive,
    dest_dir: &Path,
) -> Result<PathBuf> {
    let asset_name = lsp_release_asset_name(tag, archive);
    let download_url = find_asset_url(client, api_base, repo, tag, &asset_name).await?;
    let bytes = client
        .get(&download_url)
        .header("User-Agent", "navigator-cli")
        .send()
        .await
        .with_context(|| format!("GET {download_url}"))?
        .error_for_status()
        .with_context(|| format!("GET {download_url}"))?
        .bytes()
        .await
        .with_context(|| format!("read body of {download_url}"))?;
    std::fs::create_dir_all(dest_dir).with_context(|| format!("create {}", dest_dir.display()))?;
    let dest_path = dest_dir.join(archive.binary_name);
    let executable = extract_binary(&bytes, archive)?;
    write_executable(&dest_path, &executable)?;
    Ok(dest_path)
}

/// Extract `archive.binary_name`'s bytes out of the downloaded archive —
/// `.zip` for Windows, `.tar.gz` for Linux/macOS.
fn extract_binary(bytes: &[u8], archive: &LspReleaseArchive) -> Result<Vec<u8>> {
    // `archive_suffix` is one of the three fixed strings in
    // `LSP_RELEASE_ARCHIVES`, not a filesystem path, so matching it exactly
    // is correct here.
    match archive.archive_suffix {
        "windows.zip" => extract_from_zip(bytes, archive.binary_name),
        _ => extract_from_tar_gz(bytes, archive.binary_name),
    }
}

fn extract_from_zip(bytes: &[u8], member: &str) -> Result<Vec<u8>> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .context("open the downloaded archive as a zip")?;
    let mut file = zip
        .by_name(member)
        .with_context(|| format!("`{member}` is not in the downloaded archive"))?;
    let mut out = Vec::new();
    std::io::copy(&mut file, &mut out).context("read the archived executable")?;
    Ok(out)
}

fn extract_from_tar_gz(bytes: &[u8], member: &str) -> Result<Vec<u8>> {
    let decoder = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    for entry in archive
        .entries()
        .context("read the downloaded tar.gz archive")?
    {
        let mut entry = entry.context("read a tar.gz archive entry")?;
        if entry.path().context("read a tar.gz entry path")?.to_str() == Some(member) {
            let mut out = Vec::new();
            std::io::copy(&mut entry, &mut out).context("read the archived executable")?;
            return Ok(out);
        }
    }
    Err(anyhow!("`{member}` is not in the downloaded archive"))
}

/// Write `bytes` to `path`, marking it executable on Unix (archives carry no
/// permission bits worth trusting across the zip/tar split above).
fn write_executable(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file =
        std::fs::File::create(path).with_context(|| format!("create {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("write {}", path.display()))?;
    set_executable(path)
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)
        .with_context(|| format!("stat {}", path.display()))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).with_context(|| format!("chmod {}", path.display()))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn downloads_dir_prefers_home_over_userprofile() {
        assert_eq!(
            default_downloads_dir(Some("/home/nick".into()), Some(r"C:\Users\nick".into()))
                .expect("home resolves"),
            PathBuf::from("/home/nick/Downloads"),
        );
    }

    #[test]
    fn downloads_dir_falls_back_to_userprofile_when_home_is_unset() {
        assert_eq!(
            default_downloads_dir(None, Some(r"C:\Users\nick".into()))
                .expect("userprofile resolves"),
            PathBuf::from(r"C:\Users\nick").join("Downloads"),
        );
    }

    #[test]
    fn downloads_dir_treats_blank_env_values_as_unset() {
        assert_eq!(
            default_downloads_dir(Some(String::new()), Some("/home/nick".into()))
                .expect("blank HOME falls through to USERPROFILE"),
            PathBuf::from("/home/nick/Downloads"),
        );
    }

    #[test]
    fn downloads_dir_refuses_when_neither_is_set() {
        let err = default_downloads_dir(None, None).expect_err("no home to resolve");
        assert!(err.to_string().contains("cannot resolve a home directory"));
    }

    #[tokio::test]
    async fn find_asset_url_returns_the_matching_asset() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/neon-law-source-code/navigator/releases/tags/26.9.23"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "assets": [
                    {
                        "name": "navigator-lsp-26.9.23-linux.tar.gz",
                        "browser_download_url": "https://example.test/navigator-lsp-26.9.23-linux.tar.gz",
                    },
                    {
                        "name": "navigator-26.9.23-linux.tar.gz",
                        "browser_download_url": "https://example.test/navigator-26.9.23-linux.tar.gz",
                    },
                ],
            })))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = find_asset_url(
            &client,
            &server.uri(),
            "neon-law-source-code/navigator",
            "26.9.23",
            "navigator-lsp-26.9.23-linux.tar.gz",
        )
        .await
        .expect("asset is found");
        assert_eq!(
            url,
            "https://example.test/navigator-lsp-26.9.23-linux.tar.gz"
        );
    }

    #[tokio::test]
    async fn find_asset_url_names_the_tag_when_no_release_exists() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(
                "/repos/neon-law-source-code/navigator/releases/tags/99.9.99",
            ))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let err = find_asset_url(
            &client,
            &server.uri(),
            "neon-law-source-code/navigator",
            "99.9.99",
            "navigator-lsp-99.9.99-linux.tar.gz",
        )
        .await
        .expect_err("no release for this tag");
        assert!(
            err.to_string().contains("99.9.99"),
            "error names the tag it looked for: {err}"
        );
    }

    #[tokio::test]
    async fn find_asset_url_names_the_asset_when_the_release_lacks_it() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(
                "/repos/neon-law-source-code/navigator/releases/tags/26.9.23",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "assets": [
                    {
                        "name": "navigator-lsp-26.9.23-macos.tar.gz",
                        "browser_download_url": "https://example.test/navigator-lsp-26.9.23-macos.tar.gz",
                    },
                ],
            })))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let err = find_asset_url(
            &client,
            &server.uri(),
            "neon-law-source-code/navigator",
            "26.9.23",
            "navigator-lsp-26.9.23-linux.tar.gz",
        )
        .await
        .expect_err("the release exists but carries no matching asset");
        assert!(
            err.to_string()
                .contains("navigator-lsp-26.9.23-linux.tar.gz"),
            "error names the asset it looked for: {err}"
        );
    }

    #[test]
    fn extracts_the_named_member_from_a_zip_archive() {
        let mut buf = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(Cursor::new(&mut buf));
            writer
                .start_file::<_, ()>("navigator-lsp.exe", zip::write::FileOptions::default())
                .expect("start entry");
            std::io::Write::write_all(&mut writer, b"fake windows binary").expect("write entry");
            writer.finish().expect("finish zip");
        }
        let extracted = extract_from_zip(&buf, "navigator-lsp.exe").expect("extract");
        assert_eq!(extracted, b"fake windows binary");
    }

    #[test]
    fn extracts_the_named_member_from_a_tar_gz_archive() {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let data = b"fake unix binary";
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, "navigator-lsp", &data[..])
                .expect("append tar entry");
            builder.finish().expect("finish tar");
        }
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&tar_bytes).expect("gzip tar");
        let gz_bytes = encoder.finish().expect("finish gzip");

        let extracted = extract_from_tar_gz(&gz_bytes, "navigator-lsp").expect("extract");
        assert_eq!(extracted, b"fake unix binary");
    }

    #[test]
    fn tar_gz_extraction_names_the_missing_member() {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let data = b"unrelated";
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "LICENSE", &data[..])
                .expect("append tar entry");
            builder.finish().expect("finish tar");
        }
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&tar_bytes).expect("gzip tar");
        let gz_bytes = encoder.finish().expect("finish gzip");

        let err = extract_from_tar_gz(&gz_bytes, "navigator-lsp").expect_err("member absent");
        assert!(err.to_string().contains("navigator-lsp"));
    }
}
