//! Host-keyed credential store for `navigator site login`.
//!
//! The CLI persists one short-lived bearer token per host at
//! `~/.navigator.json` — a single dotfile in the home directory, the way
//! `gcloud` and `kubectl` keep a top-level credential file. The file is
//! written with mode `0o600` — owner read+write only — and holds, per
//! host, the opaque session token plus the identity + expiry the server
//! reported at login. The token is the lawyer's authority; it is never
//! logged and never placed in argv/env.
//!
//! Two env overrides, highest precedence first:
//!   - `NAVIGATOR_CREDENTIALS_FILE` names the file directly (tests, CI).
//!   - `NAVIGATOR_CONFIG_DIR` (legacy) places `credentials.json` in that
//!     directory, mirroring the Drive token store.
//!
//! Keyed by the normalized base URL (`https://<host>`) so one file can
//! hold tokens for prod, staging, and a local KIND cluster side by side
//! and `--host` selects between them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// The whole credential file: a map of base URL → credential.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Credentials {
    #[serde(default)]
    pub hosts: BTreeMap<String, HostCredential>,
}

/// One host's stored login: the bearer token and the identity + expiry
/// the server reported via `/auth/cli/whoami` at login time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostCredential {
    /// The HMAC-signed `SessionData` blob, presented as a bearer.
    pub token: String,
    /// Email of the authenticated person (informational / `whoami`).
    #[serde(default)]
    pub person_email: Option<String>,
    /// System-wide role (`admin` / `lawyer` / `client`).
    #[serde(default)]
    pub role: Option<String>,
    /// Unix epoch seconds the token expires — the authoritative value
    /// the server stamped into the session, so `whoami` does its expiry
    /// math without a network round-trip.
    pub expires_at: i64,
}

impl HostCredential {
    /// Seconds until expiry (negative once expired).
    #[must_use]
    pub fn seconds_remaining(&self, now: i64) -> i64 {
        self.expires_at - now
    }

    /// True once `expires_at` has passed.
    #[must_use]
    pub fn is_expired(&self, now: i64) -> bool {
        self.seconds_remaining(now) <= 0
    }
}

impl Credentials {
    /// Look up the stored credential for a base URL.
    #[must_use]
    pub fn get(&self, base_url: &str) -> Option<&HostCredential> {
        self.hosts.get(base_url)
    }

    /// Insert or replace the credential for a base URL.
    pub fn set(&mut self, base_url: impl Into<String>, cred: HostCredential) {
        self.hosts.insert(base_url.into(), cred);
    }

    /// Remove a host; returns the removed credential if present.
    pub fn remove(&mut self, base_url: &str) -> Option<HostCredential> {
        self.hosts.remove(base_url)
    }

    /// The single stored base URL when exactly one host is logged in —
    /// what lets `--host` be optional after a single `login`.
    #[must_use]
    pub fn sole_host(&self) -> Option<&str> {
        if self.hosts.len() == 1 {
            self.hosts.keys().next().map(String::as_str)
        } else {
            None
        }
    }
}

/// Normalize a `--host` value into a base URL. A bare hostname gets
/// `https://`. An explicit `http://` is honored ONLY for a loopback host
/// (the KIND / local-dev case); for any other host it is upgraded to
/// `https://` so the 8h bearer token is never sent in the clear to a
/// remote origin (a typo'd or malicious `--host` can't downgrade us). A
/// trailing slash is trimmed so the key is stable.
#[must_use]
pub fn base_url(host: &str) -> String {
    let h = host.trim().trim_end_matches('/');
    if let Some(authority) = h.strip_prefix("http://") {
        if is_loopback_host(authority) {
            return h.to_string();
        }
        // Refuse plaintext to a non-loopback host — upgrade to TLS.
        return format!("https://{authority}");
    }
    if h.contains("://") {
        h.to_string()
    } else {
        format!("https://{h}")
    }
}

/// Whether the authority of a URL (`host`, `host:port`, or `[::1]:port`)
/// is a loopback address — the only place plaintext `http://` is allowed.
/// IPs are parsed (not string-matched) so a DNS name like
/// `127.0.0.1.evil.com` is correctly NOT treated as loopback.
fn is_loopback_host(authority: &str) -> bool {
    use std::net::{Ipv4Addr, Ipv6Addr};
    let authority = authority.split('/').next().unwrap_or(authority);
    let host = if let Some(inside) = authority
        .strip_prefix('[')
        .and_then(|a| a.split_once(']').map(|(h, _)| h))
    {
        // Bracketed IPv6 literal: `[::1]:port` → `::1`.
        inside
    } else {
        // `host:port` → `host` (IPv4 / DNS name).
        authority.rsplit_once(':').map_or(authority, |(h, _)| h)
    };
    if host == "localhost" {
        return true;
    }
    if let Ok(v4) = host.parse::<Ipv4Addr>() {
        return v4.is_loopback();
    }
    host.parse::<Ipv6Addr>().is_ok_and(|v6| v6.is_loopback())
}

/// Default on-disk location for the bearer-token store: `~/.navigator.json`
/// — a single dotfile in the home directory, the way `gcloud`/`kubectl`
/// keep a top-level credential file. Overridable, highest precedence
/// first: `NAVIGATOR_CREDENTIALS_FILE` (a file path) then
/// `NAVIGATOR_CONFIG_DIR` (a directory holding `credentials.json`, the
/// legacy `~/.config/navigator` convention).
#[must_use]
pub fn default_credentials_path() -> PathBuf {
    resolve_credentials_path(
        std::env::var("NAVIGATOR_CREDENTIALS_FILE").ok(),
        std::env::var("NAVIGATOR_CONFIG_DIR").ok(),
        home_dir_from_env(),
    )
}

/// Resolve the operator's home directory using the platform's native
/// variable, while keeping the other spelling as a fallback for unusual
/// environments (including tests and Unix shells that set both).
fn home_dir_from_env() -> Option<String> {
    let home = std::env::var("HOME").ok().filter(|value| !value.is_empty());
    let userprofile = std::env::var("USERPROFILE")
        .ok()
        .filter(|value| !value.is_empty());

    home_dir_from_values(home, userprofile)
}

fn home_dir_from_values(home: Option<String>, userprofile: Option<String>) -> Option<String> {
    #[cfg(windows)]
    {
        userprofile.or(home)
    }
    #[cfg(not(windows))]
    {
        home.or(userprofile)
    }
}

/// Pure resolver behind [`default_credentials_path`], taking the three
/// env values as arguments so the precedence is unit-testable without
/// mutating process-global env vars (which races under parallel tests).
fn resolve_credentials_path(
    credentials_file: Option<String>,
    config_dir: Option<String>,
    home: Option<String>,
) -> PathBuf {
    if let Some(file) = credentials_file.filter(|f| !f.is_empty()) {
        return PathBuf::from(file);
    }
    if let Some(dir) = config_dir.filter(|d| !d.is_empty()) {
        return PathBuf::from(dir).join("credentials.json");
    }
    PathBuf::from(home.unwrap_or_else(|| ".".into())).join(".navigator.json")
}

/// Load the credential file, returning an empty store when it doesn't
/// exist yet (a fresh machine has no logins).
pub fn load(path: &Path) -> Result<Credentials> {
    match std::fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Credentials::default()),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

/// Persist the credential file with `0o600` permissions, creating the
/// parent directory if needed. Written via a temp file + rename so a
/// crashed write never leaves a half-written (or world-readable)
/// credential file in place — login is all-or-nothing.
pub fn save(path: &Path, creds: &Credentials) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(creds).context("serialize credentials")?;
    let tmp = path.with_extension("json.tmp");
    write_with_mode_0600(&tmp, &bytes).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

#[cfg(unix)]
fn write_with_mode_0600(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    std::io::Write::write_all(&mut file, bytes)
}

#[cfg(not(unix))]
fn write_with_mode_0600(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

/// Render a remaining-seconds duration as `7h52m`, `8m`, or `expired`.
#[must_use]
pub fn humanize_remaining(seconds: i64) -> String {
    if seconds <= 0 {
        return "expired".to_string();
    }
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cred(token: &str, exp: i64) -> HostCredential {
        HostCredential {
            token: token.into(),
            person_email: Some("nick@neonlaw.com".into()),
            role: Some("admin".into()),
            expires_at: exp,
        }
    }

    #[test]
    fn credentials_path_defaults_to_home_dotfile_and_honors_overrides() {
        // Default: a single `~/.navigator.json` dotfile (gcloud-style).
        assert_eq!(
            resolve_credentials_path(None, None, Some("/home/nick".into())),
            PathBuf::from("/home/nick/.navigator.json"),
        );
        // Legacy `NAVIGATOR_CONFIG_DIR` → `<dir>/credentials.json`.
        assert_eq!(
            resolve_credentials_path(
                None,
                Some("/cfg/navigator".into()),
                Some("/home/nick".into())
            ),
            PathBuf::from("/cfg/navigator/credentials.json"),
        );
        // `NAVIGATOR_CREDENTIALS_FILE` wins outright and names the file.
        assert_eq!(
            resolve_credentials_path(
                Some("/tmp/creds.json".into()),
                Some("/cfg/navigator".into()),
                Some("/home/nick".into()),
            ),
            PathBuf::from("/tmp/creds.json"),
        );
        // Empty env values are ignored (treated as unset).
        assert_eq!(
            resolve_credentials_path(Some(String::new()), Some(String::new()), Some("/h".into())),
            PathBuf::from("/h/.navigator.json"),
        );
    }

    #[test]
    fn home_directory_prefers_userprofile_on_windows_and_home_elsewhere() {
        #[cfg(windows)]
        assert_eq!(
            home_dir_from_values(None, Some(r"C:\Users\nick".into())),
            Some(r"C:\Users\nick".into())
        );
        #[cfg(not(windows))]
        assert_eq!(
            home_dir_from_values(Some("/home/nick".into()), Some(r"C:\Users\nick".into())),
            Some("/home/nick".into())
        );
    }

    #[test]
    fn home_directory_falls_back_to_userprofile_when_home_is_unset() {
        assert_eq!(
            home_dir_from_values(None, Some(r"C:\Users\nick".into())),
            Some(r"C:\Users\nick".into())
        );
    }

    #[test]
    fn base_url_adds_https_to_bare_host_and_honors_explicit_scheme() {
        assert_eq!(base_url("www.neonlaw.com"), "https://www.neonlaw.com");
        assert_eq!(base_url("www.neonlaw.com/"), "https://www.neonlaw.com");
        assert_eq!(base_url("http://localhost:8080"), "http://localhost:8080");
        assert_eq!(base_url("http://localhost:8080/"), "http://localhost:8080");
    }

    #[test]
    fn base_url_allows_plaintext_only_for_loopback() {
        // Loopback http is kept (KIND / local dev).
        assert_eq!(base_url("http://127.0.0.1:3001"), "http://127.0.0.1:3001");
        assert_eq!(base_url("http://localhost"), "http://localhost");
        assert_eq!(base_url("http://[::1]:8080"), "http://[::1]:8080");
        // Plaintext to ANY other host is upgraded to TLS so the bearer
        // token is never sent in the clear off-box.
        assert_eq!(base_url("http://prod.example"), "https://prod.example");
        assert_eq!(base_url("http://10.0.0.5:3001"), "https://10.0.0.5:3001");
        assert_eq!(
            base_url("http://127.0.0.1.evil.com"),
            "https://127.0.0.1.evil.com",
            "a host that merely starts with a loopback-looking label is not loopback",
        );
        // https is always preserved as-is.
        assert_eq!(
            base_url("https://www.neonlaw.com"),
            "https://www.neonlaw.com"
        );
    }

    #[test]
    fn round_trips_host_keyed_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        let mut creds = Credentials::default();
        creds.set("https://www.neonlaw.com", cred("tok-prod", 100));
        creds.set("http://localhost:8080", cred("tok-local", 200));
        save(&path, &creds).unwrap();

        let back = load(&path).unwrap();
        assert_eq!(
            back.get("https://www.neonlaw.com").unwrap().token,
            "tok-prod"
        );
        assert_eq!(
            back.get("http://localhost:8080").unwrap().token,
            "tok-local"
        );
        // Host-keyed: prod and local don't collide.
        assert_eq!(back.hosts.len(), 2);
    }

    #[test]
    fn loading_a_missing_file_yields_an_empty_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.json");
        let creds = load(&path).unwrap();
        assert!(creds.hosts.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn save_sets_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        let mut creds = Credentials::default();
        creds.set("https://x", cred("t", 1));
        save(&path, &creds).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {mode:o}");
        // The temp file must not survive the atomic rename.
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn sole_host_is_some_only_with_exactly_one_login() {
        let mut creds = Credentials::default();
        assert!(creds.sole_host().is_none());
        creds.set("https://only", cred("t", 1));
        assert_eq!(creds.sole_host(), Some("https://only"));
        creds.set("https://second", cred("t", 1));
        assert!(creds.sole_host().is_none());
    }

    #[test]
    fn remove_drops_just_that_host() {
        let mut creds = Credentials::default();
        creds.set("https://a", cred("ta", 1));
        creds.set("https://b", cred("tb", 1));
        assert!(creds.remove("https://a").is_some());
        assert!(creds.get("https://a").is_none());
        assert!(creds.get("https://b").is_some());
        // Removing an absent host is a clean None.
        assert!(creds.remove("https://a").is_none());
    }

    #[test]
    fn expiry_math() {
        let c = cred("t", 1_000);
        assert_eq!(c.seconds_remaining(400), 600);
        assert!(!c.is_expired(400));
        assert!(c.is_expired(1_000));
        assert!(c.is_expired(2_000));
    }

    #[test]
    fn humanize_remaining_formats_hours_minutes_and_expiry() {
        assert_eq!(humanize_remaining(7 * 3600 + 52 * 60), "7h52m");
        assert_eq!(humanize_remaining(8 * 60), "8m");
        assert_eq!(humanize_remaining(45), "45s");
        assert_eq!(humanize_remaining(0), "expired");
        assert_eq!(humanize_remaining(-30), "expired");
        // Single-digit minutes are zero-padded inside an hour.
        assert_eq!(humanize_remaining(3600 + 5 * 60), "1h05m");
    }
}
