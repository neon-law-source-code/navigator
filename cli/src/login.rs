//! `navigator login` — the browser-loopback OAuth
//! that lands a short-lived (1h) bearer token on disk, after which the
//! CLI drives the firm's matter flow against the live site.
//!
//! Mirrors `gcloud` / `restate`: `login` opens a one-shot loopback
//! listener, sends the browser to `/auth/cli/start`, and receives the
//! minted token back on the loopback. The token is the SAME signed
//! `SessionData` blob the browser cookie carries — the server resolves it
//! back via `portal::auth::inject_bearer_session`. We never build a parallel
//! auth system, and the token never touches argv, env, or the logs.

use std::process::ExitCode;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::credentials::{
    self, base_url, default_credentials_path, humanize_remaining, HostCredential,
};
use crate::palette;

/// Identity echoed by `GET /auth/cli/whoami`.
#[derive(Debug, serde::Deserialize)]
struct WhoAmI {
    #[serde(default)]
    email: Option<String>,
    role: String,
    exp: i64,
}

/// `navigator login --host <h>` — run the loopback browser flow and
/// persist the resulting token, host-keyed, at `0o600`.
pub async fn run_login(host: &str, no_browser: bool) -> ExitCode {
    match login_inner(host, no_browser).await {
        Ok(summary) => {
            println!("{summary}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("navigator login: {e:#}");
            ExitCode::from(2)
        }
    }
}

async fn login_inner(host: &str, no_browser: bool) -> Result<String> {
    let base = base_url(host);

    // Bind the loopback FIRST so we know which port to send the browser
    // back to. Port 0 → the OS picks a free port.
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("bind loopback listener")?;
    let port = listener.local_addr().context("read loopback port")?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/cb");
    let state = random_state();

    let start_url = format!(
        "{base}/auth/cli/start?redirect={}&state={}",
        urlencode(&redirect_uri),
        urlencode(&state),
    );

    println!("{}", palette::dim("Opening your browser to sign in…"));
    println!();
    println!("    {}", palette::highlight(&start_url));
    println!();
    println!(
        "{}",
        palette::dim(format!(
            "Waiting for the redirect on {redirect_uri} (Ctrl-C to cancel)…"
        ))
    );
    if !no_browser {
        open_in_browser(&start_url);
    }

    let token = wait_for_token(listener, &state, &base).await?;

    // Verify the token actually works AND learn the identity + expiry in
    // one call. Only after this succeeds do we touch the credential file,
    // so `login` is atomic — a bad token never lands on disk.
    let who = fetch_whoami(&base, &token).await?;

    let path = default_credentials_path();
    let mut creds = credentials::load(&path)?;
    creds.set(
        &base,
        HostCredential {
            token,
            person_email: who.email.clone(),
            role: Some(who.role.clone()),
            expires_at: who.exp,
        },
    );
    credentials::save(&path, &creds)?;

    let remaining = humanize_remaining(who.exp - now_secs());
    Ok(format!(
        "{} {} ({}) — token expires in {}",
        palette::dim(format!("logged in to {base} as")),
        palette::highlight(who.email.as_deref().unwrap_or("(unknown email)")),
        who.role,
        remaining,
    ))
}

/// `navigator site logout [--host <h>]` — drop the stored token for a host (or
/// the sole logged-in host). Local only; the stateless server token can't
/// be revoked, so this just forgets it.
pub fn run_logout(host: Option<&str>) -> ExitCode {
    let path = default_credentials_path();
    let mut creds = match credentials::load(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("navigator site logout: {e:#}");
            return ExitCode::from(2);
        }
    };
    let base = match resolve_base(host, &creds) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("navigator site logout: {e:#}");
            return ExitCode::from(2);
        }
    };
    if creds.remove(&base).is_some() {
        if let Err(e) = credentials::save(&path, &creds) {
            eprintln!("navigator site logout: {e:#}");
            return ExitCode::from(2);
        }
        println!("{}", palette::dim(format!("logged out of {base}")));
    } else {
        println!("{}", palette::dim(format!("no stored login for {base}")));
    }
    ExitCode::SUCCESS
}

/// `navigator site whoami [--host <h>]` — print the stored identity + how long
/// the token has left, computed locally from the recorded expiry.
pub fn run_whoami(host: Option<&str>) -> ExitCode {
    let path = default_credentials_path();
    let creds = match credentials::load(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("navigator site whoami: {e:#}");
            return ExitCode::from(2);
        }
    };
    let base = match resolve_base(host, &creds) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("navigator site whoami: {e:#}");
            return ExitCode::from(2);
        }
    };
    let Some(cred) = creds.get(&base) else {
        eprintln!(
            "navigator site whoami: not logged in to {base} — run `navigator site login --host …`"
        );
        return ExitCode::from(1);
    };
    let remaining = cred.seconds_remaining(now_secs());
    let email = cred.person_email.as_deref().unwrap_or("(unknown email)");
    let role = cred.role.as_deref().unwrap_or("(unknown role)");
    if remaining <= 0 {
        println!(
            "{} ({role}) — {} (run `navigator site login --host {base}`)",
            palette::highlight(email),
            palette::dim("token expired"),
        );
        return ExitCode::from(1);
    }
    println!(
        "{} ({role}) — expires in {}",
        palette::highlight(email),
        humanize_remaining(remaining),
    );
    ExitCode::SUCCESS
}

/// Resolve `--host` (normalized) or fall back to the sole logged-in host.
pub fn resolve_base(host: Option<&str>, creds: &credentials::Credentials) -> Result<String> {
    if let Some(h) = host {
        return Ok(base_url(h));
    }
    if let Some(sole) = creds.sole_host() {
        return Ok(sole.to_string());
    }
    if creds.hosts.is_empty() {
        Err(anyhow!(
            "no stored logins — run `navigator site login --host <host>` first"
        ))
    } else {
        Err(anyhow!(
            "multiple hosts are logged in; pass --host to choose one"
        ))
    }
}

async fn fetch_whoami(base: &str, token: &str) -> Result<WhoAmI> {
    let resp = reqwest::Client::new()
        .get(format!("{base}/auth/cli/whoami"))
        .bearer_auth(token)
        .send()
        .await
        .context("GET /auth/cli/whoami")?;
    let status = resp.status();
    if !status.is_success() {
        return Err(anyhow!(
            "whoami check failed: {status} (the minted token was not accepted)"
        ));
    }
    resp.json::<WhoAmI>()
        .await
        .context("parse /auth/cli/whoami response")
}

/// One-shot loopback listener that returns the `token` query parameter
/// from the first GET it sees, after verifying the echoed `state` matches
/// what we sent (CSRF / token-injection guard).
async fn wait_for_token(listener: TcpListener, expected_state: &str, base: &str) -> Result<String> {
    // 5-minute hard timeout so a user who closes the browser tab doesn't
    // leave `navigator site login` hanging forever.
    #[allow(clippy::duration_suboptimal_units)]
    let timeout = Duration::from_secs(5 * 60);
    let (socket, _) = tokio::time::timeout(timeout, listener.accept())
        .await
        .map_err(|_| anyhow!("timed out waiting for the browser redirect"))?
        .context("accept callback connection")?;
    handle_callback_socket(socket, expected_state, base).await
}

/// Render the tiny page the loopback callback hands back to the browser, in
/// the site's own chrome (theme tokens + the licensed GORP Serif face)
/// instead of an unstyled system-font note.
///
/// The listener serves exactly one response and closes the socket, so this
/// can't be a page on `web`'s own router — it has to carry its stylesheet
/// and font `src`s as *absolute* URLs against `base`, the host the browser
/// was just sent to sign in on, rather than the relative `/public/...`
/// paths a routed page uses (those would resolve against the loopback
/// listener itself, which serves nothing else).
fn callback_page_html(base: &str, title: &str, heading: &str, body_html: &str) -> String {
    let regular = format!("{base}/public/fonts/gorp-serif/GORPSerif-Regular.woff2");
    let bold = format!("{base}/public/fonts/gorp-serif/GORPSerif-Bold.woff2");
    let faces = views::assets::gorp_font_face_css(&regular, &bold);
    format!(
        "<!DOCTYPE html><html lang=\"en\"><head>\
         <meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"color-scheme\" content=\"light dark\">\
         <meta name=\"robots\" content=\"noindex\">\
         <title>{title}</title>\
         <link rel=\"stylesheet\" href=\"{base}/public/css/theme.css\">\
         <style>{faces}</style>\
         </head><body class=\"nav-theme\">\
         <main class=\"public-shell__main error-page\">\
         <h1>{heading}</h1>\
         {body_html}\
         </main></body></html>"
    )
}

/// Escape the handful of characters that would let an untrusted value (the
/// `error` query param a compromised or misbehaving identity provider could
/// echo back) break out of the HTML it is interpolated into.
fn escape_html(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

async fn handle_callback_socket(
    mut socket: tokio::net::TcpStream,
    expected_state: &str,
    base: &str,
) -> Result<String> {
    let mut buf = [0u8; 8192];
    let n = socket.read(&mut buf).await.context("read callback")?;
    let request = std::str::from_utf8(&buf[..n]).context("non-utf8 in callback request")?;
    let first_line = request
        .lines()
        .next()
        .ok_or_else(|| anyhow!("empty callback request"))?;
    let path_and_query = first_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| anyhow!("malformed request line: {first_line}"))?;
    let url = url::Url::parse(&format!("http://127.0.0.1{path_and_query}"))
        .with_context(|| format!("parse callback path `{path_and_query}`"))?;

    let mut state = None;
    let mut token = None;
    let mut error = None;
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "state" => state = Some(v.into_owned()),
            "token" => token = Some(v.into_owned()),
            "error" => error = Some(v.into_owned()),
            _ => {}
        }
    }

    if let Some(err) = error {
        let body = callback_page_html(
            base,
            "Login failed",
            "Login failed",
            &format!("<p>{}</p>", escape_html(&err)),
        );
        send_response(&mut socket, 400, "Bad Request", "text/html", &body).await;
        return Err(anyhow!("login failed: {err}"));
    }
    let state = state.ok_or_else(|| anyhow!("callback missing `state`"))?;
    if state != expected_state {
        send_response(
            &mut socket,
            400,
            "Bad Request",
            "text/plain",
            "state mismatch",
        )
        .await;
        return Err(anyhow!("state mismatch — refusing the token"));
    }
    let token = token.ok_or_else(|| anyhow!("callback missing `token`"))?;
    let body = callback_page_html(
        base,
        "Login successful",
        "Login successful",
        "<p>You can close this tab and return to the terminal.</p>",
    );
    send_response(&mut socket, 200, "OK", "text/html", &body).await;
    Ok(token)
}

async fn send_response(
    socket: &mut tokio::net::TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &str,
) {
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len(),
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;
}

/// Best-effort browser open. The URL is always printed too, so a
/// headless or SSH session can copy it manually.
fn open_in_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let opener = ("open", vec![url]);
    #[cfg(target_os = "windows")]
    let opener = ("cmd", vec!["/C", "start", "", url]);
    #[cfg(all(unix, not(target_os = "macos")))]
    let opener = ("xdg-open", vec![url]);

    let (cmd, args) = opener;
    let _ = std::process::Command::new(cmd)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn random_state() -> String {
    // 128 bits hex — enough to defeat any accidental collision between
    // concurrent `navigator site login` runs on the same host.
    let a: u64 = rand::random();
    let b: u64 = rand::random();
    format!("{a:016x}{b:016x}")
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Minimal percent-encoder for the query values we build.
fn urlencode(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    async fn drive_a_callback(addr: SocketAddr, query: &str) {
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        let line = format!("GET {query} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
        s.write_all(line.as_bytes()).await.unwrap();
        let mut buf = [0u8; 1024];
        let _ = s.read(&mut buf).await;
    }

    const TEST_BASE: &str = "https://staging.neonlaw.com";

    #[tokio::test]
    async fn callback_returns_token_on_state_match() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server =
            tokio::spawn(async move { wait_for_token(listener, "the-nonce", TEST_BASE).await });
        drive_a_callback(addr, "/cb?token=sess.blob&state=the-nonce").await;
        let token = server.await.unwrap().unwrap();
        assert_eq!(token, "sess.blob");
    }

    #[tokio::test]
    async fn callback_rejects_state_mismatch() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server =
            tokio::spawn(async move { wait_for_token(listener, "the-nonce", TEST_BASE).await });
        drive_a_callback(addr, "/cb?token=sess.blob&state=tampered").await;
        let err = server.await.unwrap().unwrap_err();
        assert!(format!("{err:#}").contains("state mismatch"));
    }

    #[tokio::test]
    async fn callback_surfaces_an_error_param() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server =
            tokio::spawn(async move { wait_for_token(listener, "the-nonce", TEST_BASE).await });
        drive_a_callback(addr, "/cb?error=access_denied&state=the-nonce").await;
        let err = server.await.unwrap().unwrap_err();
        assert!(format!("{err:#}").contains("access_denied"));
    }

    #[test]
    fn callback_page_carries_the_theme_and_gorp_face_from_base() {
        let html = callback_page_html(
            TEST_BASE,
            "Login successful",
            "Login successful",
            "<p>You can close this tab and return to the terminal.</p>",
        );
        assert!(
            html.contains(&format!(r#"href="{TEST_BASE}/public/css/theme.css""#)),
            "theme stylesheet: {html}"
        );
        assert!(html.contains("GORP Serif"), "brand face: {html}");
        assert!(
            html.contains(&format!("{TEST_BASE}/public/fonts/gorp-serif/")),
            "font src absolute against base: {html}"
        );
    }

    #[test]
    fn callback_page_escapes_an_untrusted_error_value() {
        let html = callback_page_html(
            TEST_BASE,
            "Login failed",
            "Login failed",
            &format!("<p>{}</p>", escape_html("<script>evil()</script>")),
        );
        assert!(
            !html.contains("<script>evil()</script>"),
            "raw markup leaked: {html}"
        );
        assert!(html.contains("&lt;script&gt;"), "escaped instead: {html}");
    }

    #[test]
    fn random_state_is_32_hex_chars_and_unique() {
        let a = random_state();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, random_state());
    }
}
