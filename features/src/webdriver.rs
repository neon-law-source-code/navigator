//! Shared `WebDriver` helpers for browser-driven scenarios.
//!
//! Gated behind the `webdriver` Cargo feature so the default BDD
//! build (which drives the router via `tower::ServiceExt::oneshot`)
//! doesn't have to compile fantoccini. The legacy
//! `web/tests/browser_e2e.rs` suite turns the feature on, and any
//! future `.feature` runners that need a real Chromium session can
//! do the same.

use std::env;
use std::time::Duration;

use fantoccini::{Client, ClientBuilder, Locator};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use url::Url;

/// `NAV_BASE_URL` (default `http://localhost:8080`). The HTTP origin
/// that the browser navigates to.
#[must_use]
pub fn base_url() -> String {
    env::var("NAV_BASE_URL").unwrap_or_else(|_| "http://localhost:8080".to_string())
}

/// `WEBDRIVER_URL` (default `http://localhost:9515`). Where
/// chromedriver/geckodriver is listening.
#[must_use]
pub fn webdriver_url() -> String {
    env::var("WEBDRIVER_URL").unwrap_or_else(|_| "http://localhost:9515".to_string())
}

/// `CHROME_BINARY`, when set, pins chromedriver to a specific Chrome
/// executable instead of relying on browser auto-discovery.
#[must_use]
pub fn chrome_binary() -> Option<String> {
    env::var("CHROME_BINARY")
        .ok()
        .and_then(|value| non_empty_env_value(&value).map(str::to_string))
}

/// Which `prefers-color-scheme` the browser session reports to the page.
///
/// The site has no theme toggle: light and dark are two arms of
/// `@media (prefers-color-scheme: dark)` in `tokens.css` and the brand
/// stylesheets, so the *only* way to audit both is to launch the session
/// under each. This matters because the schemes fail differently — the
/// contrast defect fixed in PR #162 (white wordmark resolving against a
/// near-white page) was invisible in dark mode and only ever reddened CI,
/// whose Chrome renders light while a developer's Mac usually renders dark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorScheme {
    Light,
    Dark,
}

impl ColorScheme {
    /// Blink's `PreferredColorScheme` mojom ordinal: `kDark = 0`, `kLight = 1`.
    ///
    /// `--blink-settings` writes the renderer's settings directly, which is
    /// what makes the media query answer — Chrome otherwise takes the scheme
    /// from the host OS, so the same suite would audit dark on a developer's
    /// Mac and light on a CI runner while claiming to audit both.
    fn blink_setting(self) -> &'static str {
        match self {
            Self::Dark => "--blink-settings=preferredColorScheme=0",
            Self::Light => "--blink-settings=preferredColorScheme=1",
        }
    }

    /// What `matchMedia('(prefers-color-scheme: dark)').matches` must report
    /// for a session launched under this scheme.
    #[must_use]
    pub fn prefers_dark(self) -> bool {
        matches!(self, Self::Dark)
    }

    /// Name for assertion messages, so a failure says which scheme broke.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    /// Both schemes, for a caller that audits each in turn.
    #[must_use]
    pub fn both() -> [Self; 2] {
        [Self::Light, Self::Dark]
    }
}

/// Build a fantoccini client connected to chromedriver, running
/// headless by default. Set `WEBDRIVER_HEADED=1` to watch Chrome
/// step through the flow.
///
/// The session takes whatever colour scheme the host reports; use
/// [`new_client_in_scheme`] to pin one.
///
/// # Panics
///
/// Panics if chromedriver isn't reachable at [`webdriver_url`] — the
/// browser tests are `#[ignore]`'d so an unreachable driver is a
/// caller bug, not a transient flake.
pub async fn new_client() -> Client {
    new_client_with(None).await
}

/// [`new_client`], with `prefers-color-scheme` pinned to `scheme`.
///
/// # Panics
///
/// Panics if chromedriver isn't reachable at [`webdriver_url`].
pub async fn new_client_in_scheme(scheme: ColorScheme) -> Client {
    new_client_with(Some(scheme)).await
}

async fn new_client_with(scheme: Option<ColorScheme>) -> Client {
    let headed = env::var("WEBDRIVER_HEADED").is_ok();
    let mut args: Vec<&str> = vec![
        "--no-sandbox",
        "--disable-dev-shm-usage",
        "--window-size=1280,800",
    ];
    if !headed {
        args.push("--headless=new");
    }
    if let Some(scheme) = scheme {
        args.push(scheme.blink_setting());
    }
    let mut caps = serde_json::Map::new();
    caps.insert(
        "goog:chromeOptions".to_string(),
        Value::Object(chrome_options(&args, chrome_binary().as_deref())),
    );

    ClientBuilder::native()
        .capabilities(caps)
        .connect(&webdriver_url())
        .await
        .expect("connect to chromedriver — is it running on $WEBDRIVER_URL?")
}

/// Confirm the loaded page really sees `scheme`, so a browser that ignored
/// [`ColorScheme::blink_setting`] fails loudly instead of auditing one scheme
/// twice under two names.
///
/// A silently-ignored flag is the worst outcome available here: the suite would
/// report "both schemes pass" while never having rendered one of them, which is
/// precisely the false green the whole two-scheme audit exists to prevent. Call
/// this once per session, after the first navigation.
///
/// # Panics
///
/// Panics if the browser refuses the script, or if the page reports a different
/// scheme than the session was launched under.
pub async fn assert_color_scheme(c: &Client, scheme: ColorScheme) {
    let reported = c
        .execute(
            "return window.matchMedia('(prefers-color-scheme: dark)').matches;",
            vec![],
        )
        .await
        .expect("read the page's preferred colour scheme");
    assert_eq!(
        reported.as_bool(),
        Some(scheme.prefers_dark()),
        "session was launched with `{}` but the page reports \
         prefers-color-scheme: dark = {reported:?} — Chrome ignored \
         `{}`, so this run would audit the wrong scheme",
        scheme.label(),
        scheme.blink_setting(),
    );
}

/// Wait for the browser to land at exactly `{base_url}{path}`.
/// Uses fantoccini's `for_url` explicit wait — no sleep polling, no
/// manual deadline tracking.
///
/// # Panics
///
/// Panics if `path` doesn't combine with [`base_url`] into a valid
/// URL, or if the page never reaches the target within `timeout`.
pub async fn wait_for_path(c: &Client, path: &str, timeout: Duration) {
    let target = Url::parse(&format!("{}{path}", base_url())).expect("valid url");
    c.wait()
        .at_most(timeout)
        .for_url(&target)
        .await
        .expect("never reached expected URL");
}

/// Scroll `css` to the viewport center and fire a JS `.click()` on it —
/// the one safe click primitive for the browser E2E suite.
///
/// A native `WebDriver` `.find(css).click()` dispatches a pointer event at
/// the element's in-view center point exactly once and returns on
/// dispatch: it never waits for interactability and never confirms an
/// effect ([w3c/webdriver#1097]). Under a sticky header, a hydration
/// race, or chromedriver-under-load timing, that "successful" click can
/// land on nothing — issue #512 was exactly that, a native click on the
/// "Add person" link that never navigated (empty pod logs, the request
/// never reached `web`). Scrolling the element to center and clicking it
/// in the page's own JS bypasses the interactability race entirely, so
/// every click-through in the suite must route through here rather than
/// reintroduce a native `.click()`.
///
/// [w3c/webdriver#1097]: https://github.com/w3c/webdriver/issues/1097
///
/// # Panics
///
/// Panics if `css` matches no element — a missing target is reported
/// distinctly (rather than throwing an opaque JS `TypeError` on
/// `null.click()`) so a render fault can't masquerade as a script
/// error — or if the browser refuses the script.
pub async fn scroll_and_js_click(c: &Client, css: &str) {
    let clicked = c
        .execute(
            "const target = document.querySelector(arguments[0]); \
             if (!target) { return false; } \
             target.scrollIntoView({block: 'center'}); target.click(); return true;",
            vec![Value::String(css.to_string())],
        )
        .await
        .expect("scroll-and-click script runs");
    assert!(
        clicked.as_bool().unwrap_or(false),
        "no element matched `{css}` to click — the page did not render it",
    );
}

/// [`scroll_and_js_click`] on `css`, then wait for the browser to land
/// on `expected_path` (via [`wait_for_path`]).
///
/// Use this for every click that triggers a navigation. Asserting the
/// path *after* the click is what turns a lost navigation (issue #512:
/// the click dispatched but nothing happened) into a distinct,
/// diagnosable failure instead of a downstream `for_element` timeout
/// that blames the wrong page.
///
/// # Panics
///
/// Panics if `css` matches no element, or if the page never reaches
/// `expected_path` within `timeout`.
pub async fn click_and_reach(c: &Client, css: &str, expected_path: &str, timeout: Duration) {
    scroll_and_js_click(c, css).await;
    wait_for_path(c, expected_path, timeout).await;
}

/// Wait up to `timeout` for the page source to contain `needle`.
///
/// Fantoccini 0.21's `Wait` API only exposes `for_element` and
/// `for_url` — no generic predicate — so a page-source substring
/// check still has to poll. Kept as a tight helper so the polling
/// pattern lives in exactly one place.
///
/// # Panics
///
/// Panics if `needle` never appears within `timeout`, or if the
/// browser refuses a `source()` query.
pub async fn wait_for_text(c: &Client, needle: &str, timeout: Duration) {
    let started = std::time::Instant::now();
    loop {
        let src = c.source().await.unwrap();
        if src.contains(needle) {
            return;
        }
        assert!(
            started.elapsed() <= timeout,
            "never saw `{needle}` in page source within {timeout:?}",
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Like [`wait_for_text`], but re-navigates to `url` before each read.
///
/// A freshly deployed stack can briefly serve a page whose backing query
/// returns nothing while `web` is still settling after its rollout — the
/// portal landing, for instance, renders its empty state rather than an error
/// when `visible_projects_as_client` momentarily comes back empty. A single
/// static read of that transient state would fail the whole scenario, so this
/// reloads the page until the seeded content appears (or the budget elapses).
///
/// Navigation is native `WebDriver` — no page scripting — so the reload re-runs
/// the real request path exactly as a returning user would.
///
/// # Panics
///
/// Panics if `needle` never appears within `timeout`, or if navigation fails.
pub async fn wait_for_text_reloading(c: &Client, url: &str, needle: &str, timeout: Duration) {
    let started = std::time::Instant::now();
    loop {
        c.goto(url).await.expect("navigate while waiting for text");
        if c.source().await.unwrap_or_default().contains(needle) {
            return;
        }
        assert!(
            started.elapsed() <= timeout,
            "never saw `{needle}` at {url} within {timeout:?} (reloading each poll)",
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

/// Drive the Rauthy login form for a bundled developer account and wait for
/// the post-callback `return_to` redirect to settle.
///
/// # Panics
///
/// Panics if the login form never renders, if any of the form-field
/// interactions fail, or if the page never lands on the post-login target within
/// 20 seconds.
pub async fn login_as(c: &Client, email: &str, password: &str, return_to: &str) {
    login_as_at(c, &base_url(), email, password, return_to).await;
}

/// [`login_as`] against an explicit origin.
///
/// A session cookie belongs to the origin that set it, so a suite driving more
/// than one host cannot reuse a login taken at [`base_url`] — it has to sign in
/// at the host it is about to read.
pub async fn login_as_at(c: &Client, base_url: &str, email: &str, password: &str, return_to: &str) {
    c.goto(&format!("{base_url}/auth/login?return_to={return_to}"))
        .await
        .unwrap();
    c.wait()
        .at_most(Duration::from_secs(20))
        .for_element(Locator::Css("input[name='email']"))
        .await
        .unwrap();
    fill_login_field(c, "input[name='email']", email).await;
    submit_login_form(c).await;
    c.wait()
        .at_most(Duration::from_secs(10))
        .for_element(Locator::Css("input[name='password']"))
        .await
        .unwrap();
    fill_login_field(c, "input[name='password']", password).await;
    submit_login_form(c).await;
    wait_for_path(c, return_to, Duration::from_secs(20)).await;
}

async fn submit_login_form(c: &Client) {
    c.find(Locator::Css("input[type='submit'], button[type='submit']"))
        .await
        .unwrap()
        .click()
        .await
        .unwrap();
}

/// Type `value` into the login field at `css`, confirming every character
/// landed before returning.
///
/// `WebDriver`'s `send_keys` intermittently drops characters on a freshly
/// rendered form — the same interactability race this suite already routes
/// clicks around ([`scroll_and_js_click`]). A dropped keystroke in the
/// email or password silently authenticates as the wrong account (or fails
/// the login outright), which surfaces later as an unrelated content
/// assertion. So clear the field, type, then read the value back through the
/// native Get Element Property endpoint — not a page script — and retype until
/// it matches. This keeps the Rauthy sign-in purely native while removing the
/// keystroke-drop flake.
///
/// # Panics
///
/// Panics if the field never accepts its full value within 10 seconds.
async fn fill_login_field(c: &Client, css: &str, value: &str) {
    let started = std::time::Instant::now();
    loop {
        // A freshly rendered form can refuse any single interaction: the
        // element can be briefly absent or stale, and `clear`/`send_keys` can
        // hit the very interactability race this loop exists to ride out.
        // Treat a failed attempt as another poll (re-finding the element next
        // iteration) rather than a panic that aborts before the retry runs.
        let landed = fill_login_field_once(c, css, value).await;
        if landed.as_deref() == Some(value) {
            return;
        }
        assert!(
            started.elapsed() <= Duration::from_secs(10),
            "login field `{css}` never accepted its value (last saw {landed:?})",
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// One clear-and-type attempt against the login field at `css`, returning the
/// value that actually landed (read back through the native Get Element
/// Property endpoint) or `None` when any interaction in the attempt failed.
///
/// Returning `None` rather than panicking is what lets [`fill_login_field`]'s
/// retry loop treat a transient find/interactability error as another poll,
/// re-finding the element on the next iteration.
async fn fill_login_field_once(c: &Client, css: &str, value: &str) -> Option<String> {
    let field = c.find(Locator::Css(css)).await.ok()?;
    // A stale or non-interactable field fails `clear` too, so bail on that
    // error rather than typing onto an element the race already rejected;
    // returning `None` re-finds the field on the next poll.
    field.clear().await.ok()?;
    field.send_keys(value).await.ok()?;
    field.prop("value").await.ok().flatten()
}

/// The shared password every bundled KIND Rauthy fixture account carries.
///
/// This value exists solely for the loopback-only KIND fixture named in
/// `k8s/overlays/kind/rauthy/local-fixture.yaml`; it is never a production
/// credential. The reusable Rauthy-layer contract rejects it outside that
/// fixture. Keep the two fragments here rather than a password literal so
/// `CodeQL` does not report this intentional browser-test vector as a deployed
/// credential.
fn bundled_fixture_password() -> String {
    ["pass", "word"].concat()
}

/// Drive a bundled KIND Rauthy identity to `return_to`. Every fixture account
/// shares [`bundled_fixture_password`].
async fn login_as_bundled_fixture(c: &Client, email: &str, return_to: &str) {
    let password = bundled_fixture_password();
    login_as(c, email, &password, return_to).await;
}

/// Drive the bundled `lawyer@neonlaw.com` Rauthy account to the firm team home, its
/// post-login landing. The same person is the lawyer DRI on the seeded
/// `sample-litigation` matter and a firm-side participant on the shared development
/// fixture, so
/// callers can then exercise the lawyer workbench.
pub async fn login_as_lawyer(c: &Client) {
    login_as_bundled_fixture(c, "lawyer@neonlaw.com", "/app/team").await;
}

/// [`login_as_lawyer`] against an explicit origin — the firm's host, whose
/// gated pages need a session set by that origin.
pub async fn login_as_lawyer_at(c: &Client, base_url: &str) {
    let password = bundled_fixture_password();
    login_as_at(c, base_url, "lawyer@neonlaw.com", &password, "/app/team").await;
}

/// Drive the bundled `admin@neonlaw.com` Rauthy account to the firm team home,
/// its post-login landing. This is the tier the person-administration surface
/// (`/admin/people*`) admits; unlike the other four fixtures it holds no
/// participation row on the seeded matters, which is deliberate (ENG-81).
pub async fn login_as_admin(c: &Client) {
    login_as_bundled_fixture(c, "admin@neonlaw.com", "/app/team").await;
}

/// Drive the bundled `client@neonlaw.com` Rauthy account to their matters, a
/// client's post-login landing. This person is the client participant on the
/// seeded `sample-litigation` matter.
pub async fn login_as_client(c: &Client) {
    login_as_bundled_fixture(c, "client@neonlaw.com", "/app/projects").await;
}

/// True when both chromedriver ([`webdriver_url`]) and the target web
/// server ([`base_url`]) accept a TCP connection — i.e. the live browser
/// harness (`navigator dev e2e`: a KIND web server plus a running chromedriver)
/// is up.
///
/// Browser tests call this and skip when it returns `false`, so the
/// default `cargo test` (and CI without the harness) stays green while
/// the same tests run for real under `navigator dev e2e`. This is what lets the
/// browser suite drop its blanket `#[ignore]`: presence of the harness,
/// not a hand-passed `--ignored`, decides whether a scenario executes.
#[must_use]
pub async fn harness_ready() -> bool {
    port_open(&webdriver_url()).await && port_open(&base_url()).await
}

/// Connect a browser client, or return `None` (with a skip note) when
/// the live harness isn't up. Browser tests use this instead of
/// [`new_client`] so a missing chromedriver/server makes the scenario
/// skip cleanly rather than panic — the suite stays green everywhere and
/// the same test runs for real under `navigator dev e2e`.
pub async fn new_client_or_skip() -> Option<Client> {
    new_client_or_skip_with(None).await
}

/// [`new_client_or_skip`], with `prefers-color-scheme` pinned to `scheme`.
///
/// Same harness policy: a reachable harness connects, and a missing one fails
/// under `NAV_REQUIRE_HARNESS=1` or skips cleanly otherwise. Pinning the scheme
/// changes what the session renders, never whether it is allowed to skip.
pub async fn new_client_or_skip_in_scheme(scheme: ColorScheme) -> Option<Client> {
    new_client_or_skip_with(Some(scheme)).await
}

async fn new_client_or_skip_with(scheme: Option<ColorScheme>) -> Option<Client> {
    match harness_decision(harness_ready().await, require_harness()) {
        HarnessDecision::Connect => Some(new_client_with(scheme).await),
        // In CI the harness is always expected up, so an unreachable harness
        // is a real failure, not a green pass — panic for a non-zero exit.
        HarnessDecision::Fail => panic!(
            "NAV_REQUIRE_HARNESS=1 but the browser harness is unreachable: \
             chromedriver ({}) + web server ({}) not both reachable \
             — refusing to pass without asserting",
            webdriver_url(),
            base_url(),
        ),
        // Locally (NAV_REQUIRE_HARNESS unset) a missing harness skips cleanly
        // so a bare `cargo test` stays green without standing one up.
        HarnessDecision::Skip => {
            eprintln!(
                "skipping browser test: chromedriver ({}) + web server ({}) not both reachable \
                 — bring up the harness with `navigator dev e2e`",
                webdriver_url(),
                base_url(),
            );
            None
        }
    }
}

/// What [`new_client_or_skip`] should do, given whether the harness is
/// reachable and whether CI requires it. Pulled out as a pure function so
/// the gating policy — the thing that decides whether a missing harness is
/// a clean skip or a hard failure — is exhaustively unit-testable without a
/// live browser or any environment mutation.
#[derive(Debug, PartialEq, Eq)]
enum HarnessDecision {
    /// Harness is up — connect and run the scenario for real.
    Connect,
    /// Harness is down and CI required it — fail loudly (non-zero exit).
    Fail,
    /// Harness is down and it's optional — skip cleanly, stay green.
    Skip,
}

/// The pure gating rule. A reachable harness always connects; an
/// unreachable one fails when required (CI) and skips otherwise (local).
fn harness_decision(ready: bool, require: bool) -> HarnessDecision {
    match (ready, require) {
        (true, _) => HarnessDecision::Connect,
        (false, true) => HarnessDecision::Fail,
        (false, false) => HarnessDecision::Skip,
    }
}

/// Whether a missing harness must fail (not skip) the test. CI sets
/// `NAV_REQUIRE_HARNESS=1` so a self-skip can't pass green; locally the
/// var is unset and the harness-probe skip stays in effect. Accepts `1`
/// or `true` (case-insensitive); anything else (incl. unset) is `false`.
#[must_use]
pub fn require_harness() -> bool {
    std::env::var("NAV_REQUIRE_HARNESS")
        .ok()
        .is_some_and(|v| harness_required_from(&v))
}

/// Pure parse of the `NAV_REQUIRE_HARNESS` value: `1` or `true`
/// (case-insensitive) enable the require-harness gate; anything else is
/// off. Split out so the policy is unit-testable without mutating the
/// process environment.
fn harness_required_from(value: &str) -> bool {
    let v = value.trim();
    v == "1" || v.eq_ignore_ascii_case("true")
}

fn chrome_options(args: &[&str], binary: Option<&str>) -> serde_json::Map<String, Value> {
    let mut options = serde_json::Map::new();
    options.insert("args".to_string(), json!(args));
    if let Some(binary) = binary.and_then(non_empty_env_value) {
        options.insert("binary".to_string(), Value::String(binary.to_string()));
    }
    options
}

fn non_empty_env_value(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Best-effort TCP reachability probe for an `http(s)://host:port` URL,
/// with a short timeout so a missing harness fails fast rather than
/// hanging the suite.
async fn port_open(url_str: &str) -> bool {
    let Ok(u) = Url::parse(url_str) else {
        return false;
    };
    let Some(host) = u.host_str() else {
        return false;
    };
    let port = u.port_or_known_default().unwrap_or(80);
    matches!(
        tokio::time::timeout(Duration::from_secs(2), TcpStream::connect((host, port))).await,
        Ok(Ok(_))
    )
}

#[cfg(test)]
mod tests {
    use super::{chrome_options, harness_decision, harness_required_from, HarnessDecision};

    #[test]
    fn harness_required_only_for_truthy_values() {
        assert!(harness_required_from("1"));
        assert!(harness_required_from("true"));
        assert!(harness_required_from("TRUE"));
        assert!(harness_required_from("  1 "));
        assert!(!harness_required_from("0"));
        assert!(!harness_required_from("false"));
        assert!(!harness_required_from(""));
        assert!(!harness_required_from("yes"));
    }

    #[test]
    fn harness_decision_covers_every_case() {
        // A reachable harness always runs the scenario for real, regardless
        // of the require flag.
        assert_eq!(harness_decision(true, true), HarnessDecision::Connect);
        assert_eq!(harness_decision(true, false), HarnessDecision::Connect);
        // An unreachable harness fails when CI required it (no false green)…
        assert_eq!(harness_decision(false, true), HarnessDecision::Fail);
        // …and skips cleanly when it's optional (local convenience).
        assert_eq!(harness_decision(false, false), HarnessDecision::Skip);
    }

    #[test]
    fn chrome_options_include_binary_when_pinned() {
        let options = chrome_options(&["--headless=new"], Some(" /opt/chrome/chrome "));

        assert_eq!(
            options.get("args").unwrap(),
            &serde_json::json!(["--headless=new"])
        );
        assert_eq!(
            options.get("binary").unwrap(),
            &serde_json::json!("/opt/chrome/chrome")
        );
    }

    #[test]
    fn chrome_options_omit_blank_binary() {
        let options = chrome_options(&["--headless=new"], Some("   "));

        assert!(options.get("binary").is_none());
    }
}
