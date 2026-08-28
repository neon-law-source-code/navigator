//! The public repository Neon Law Navigator is developed in, and how many
//! people have starred it.
//!
//! Navigator is open source, so the footer says so by linking the repository
//! and printing its star count beside it. The link is a constant: a white-label
//! deployment runs *this* platform whatever wordmark it wears, exactly as the
//! "Powered by Neon Law Navigator" line above it already assumes.
//!
//! # The count is read from a cache, never fetched on the render path
//!
//! [`star_count`] is a lock-free read of a process-wide cache and makes no
//! network call. Nothing about a marketing page's time-to-first-byte may depend
//! on `api.github.com` being reachable, and unauthenticated GitHub allows 60
//! requests an hour per IP — a fetch per render would exhaust that in a minute
//! of ordinary traffic and stall every page behind a third party's latency.
//!
//! So the two halves are separate. [`spawn_refresh`] starts one background task
//! at boot that refreshes the cache every [`REFRESH_INTERVAL`]; every render
//! reads whatever that task last published. The cache starts empty and stays
//! empty when the fetch fails, GitHub is unreachable, or nothing spawned the
//! refresh at all — a test router, for instance, which is why the suite makes
//! no outbound call. An empty cache is not an error state: the footer renders
//! the repository link with no count, which is the honest thing to publish when
//! the number is unknown.

/// The repository, as GitHub addresses it. Rendered as the link's text, so a
/// reader sees the owner and name rather than a bare URL.
pub const REPOSITORY_SLUG: &str = "neon-law-source-code/navigator";

/// The repository's public web address.
pub const REPOSITORY_HREF: &str = "https://github.com/neon-law-source-code/navigator";

#[cfg(feature = "server")]
pub use live::{
    refresh, spawn_refresh, star_count, DEFAULT_API_BASE, GITHUB_API_BASE_ENV, REFRESH_INTERVAL,
};

/// The cache, the fetch, and the background refresh that connects them. Server
/// only: `reqwest` does not compile to `wasm32-unknown-unknown`, and the wasm
/// client reads the count out of the server-resolved chrome rather than
/// fetching it for itself.
#[cfg(feature = "server")]
mod live {
    use std::sync::{PoisonError, RwLock};
    use std::time::Duration;

    use super::REPOSITORY_SLUG;

    /// Env var overriding the GitHub REST base, for GitHub Enterprise or a test
    /// double. Spelled the same as `workflows::github`'s, so one deployment
    /// setting redirects every GitHub caller in the tree.
    ///
    /// Naming GitHub Enterprise is a **feature, not stale narration.**
    /// Navigator runs on github.com; this override is how somebody running
    /// their own instance points it at their own tenant, which the licence
    /// invites and the deployment workshop is written for. Deleting it would
    /// remove the self-hosting path, not a false claim about us.
    pub const GITHUB_API_BASE_ENV: &str = "NAVIGATOR_GITHUB_API_BASE";

    /// Public GitHub's REST API base.
    pub const DEFAULT_API_BASE: &str = "https://api.github.com";

    /// How long the published count is allowed to stand before the background
    /// task fetches a fresh one.
    ///
    /// An hour. A star count is a soft social signal in fine print — nobody
    /// refreshes a law firm's footer to watch it tick — and one request an hour
    /// per pod sits far inside the 60/hour unauthenticated budget even with the
    /// token absent and several pods sharing an egress IP.
    pub const REFRESH_INTERVAL: Duration = Duration::from_hours(1);

    /// Bound on the fetch itself. The refresh runs off the render path, so a
    /// hang costs no request — but a task blocked forever on a half-open socket
    /// stops refreshing, which is the failure this timeout actually prevents.
    const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

    /// The REST API version this client pins. GitHub dates its breaking
    /// changes, so pinning means a future default cannot silently reshape the
    /// response this parses.
    const API_VERSION: &str = "2022-11-28";

    /// GitHub rejects an API request that sends no `User-Agent`.
    const USER_AGENT: &str = concat!("navigator/", env!("CARGO_PKG_VERSION"));

    /// The last count fetched, or `None` before the first successful fetch.
    ///
    /// A plain `static` rather than a `OnceLock`: the value is replaced on
    /// every refresh, not initialized once.
    static STARS: RwLock<Option<u64>> = RwLock::new(None);

    /// How many people have starred the repository, or `None` when no fetch has
    /// succeeded yet.
    ///
    /// A cache read. It never blocks on the network and never fails: a poisoned
    /// lock yields the value the panicking writer left behind, because a stale
    /// star count is not worth propagating a panic into a page render.
    #[must_use]
    pub fn star_count() -> Option<u64> {
        *STARS.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// Publish a freshly fetched count for every subsequent render to read.
    fn publish(count: u64) {
        *STARS.write().unwrap_or_else(PoisonError::into_inner) = Some(count);
    }

    /// The REST base to call, given whatever [`GITHUB_API_BASE_ENV`] holds.
    ///
    /// Takes the configured value rather than reading the environment itself so
    /// the precedence is testable without mutating process state, which under a
    /// parallel test runner is shared. An unset *or blank* override falls back:
    /// a deployment that renders an empty value into its environment means
    /// "unset", not "call the empty host".
    fn api_base_from(configured: Option<String>) -> String {
        configured
            .map(|base| base.trim().trim_end_matches('/').to_string())
            .filter(|base| !base.is_empty())
            .unwrap_or_else(|| DEFAULT_API_BASE.to_string())
    }

    /// The repository endpoint to fetch, resolved against the configured base.
    fn repository_url() -> String {
        let base = api_base_from(std::env::var(GITHUB_API_BASE_ENV).ok());
        format!("{base}/repos/{REPOSITORY_SLUG}")
    }

    /// The star count carried by a `GET /repos/{owner}/{repo}` response body.
    ///
    /// `None` for anything that is not a JSON object carrying a numeric
    /// `stargazers_count` — a rate-limit message, an error document, or a
    /// future response that renamed the field. The caller publishes nothing in
    /// that case, so a shape change costs the count rather than the page.
    fn parse_star_count(body: &str) -> Option<u64> {
        serde_json::from_str::<serde_json::Value>(body)
            .ok()?
            .get("stargazers_count")?
            .as_u64()
    }

    /// Fetch the current star count from GitHub and publish it to the cache.
    ///
    /// Returns the count on success and `None` on any failure, having logged
    /// the reason. It is deliberately total: this runs unsupervised in a
    /// background task, and a repository that has been renamed, an egress
    /// policy that blocks GitHub, or a spent rate-limit budget must cost the
    /// footer its number and nothing else.
    ///
    /// The request is unauthenticated. The repository is public, so a token
    /// buys only a larger rate-limit budget — and one request an hour needs
    /// none. Sending firm credentials to a third party to read a public number
    /// is a worse trade than occasionally rendering no number.
    pub async fn refresh() -> Option<u64> {
        let url = repository_url();
        let response = reqwest::Client::new()
            .get(&url)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header("X-GitHub-Api-Version", API_VERSION)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await;

        let response = match response {
            Ok(response) => response,
            Err(error) => {
                tracing::debug!(%url, %error, "github star count unreachable");
                return None;
            }
        };

        let status = response.status();
        if !status.is_success() {
            tracing::debug!(%url, %status, "github refused the star count");
            return None;
        }

        let body = match response.text().await {
            Ok(body) => body,
            Err(error) => {
                tracing::debug!(%url, %error, "github star count body unreadable");
                return None;
            }
        };

        let Some(count) = parse_star_count(&body) else {
            // The body is a third party's document, so it is not logged — only
            // that it did not carry the field this looks for.
            tracing::debug!(%url, "github star count response carried no stargazers_count");
            return None;
        };
        publish(count);
        tracing::debug!(repository = REPOSITORY_SLUG, count, "github star count");
        Some(count)
    }

    /// Start the background task that keeps [`star_count`] current, called once
    /// per process at boot.
    ///
    /// The first fetch runs immediately, so the count appears within a second
    /// of the server accepting traffic rather than an hour later. Every render
    /// before it lands reads an empty cache and publishes the repository link
    /// with no number, which is why nothing here needs to be awaited.
    ///
    /// Nothing calls this from a test, which is what keeps the suite off the
    /// network: a router built directly in a test spawns no refresh, so its
    /// footers render the link alone.
    pub fn spawn_refresh() {
        tokio::spawn(async {
            loop {
                refresh().await;
                tokio::time::sleep(REFRESH_INTERVAL).await;
            }
        });
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The count comes out of the one field GitHub publishes it in.
        #[test]
        fn reads_the_star_count_from_the_repository_document() {
            let body = r#"{"full_name":"neon-law-source-code/navigator","stargazers_count":1234}"#;
            assert_eq!(parse_star_count(body), Some(1234));
        }

        /// Every shape that is not a repository document costs the count and
        /// nothing else — the caller publishes nothing and the footer renders
        /// the link alone.
        #[test]
        fn publishes_nothing_for_a_response_that_is_not_a_repository() {
            // A rate-limit or not-found document: valid JSON, no such field.
            assert_eq!(
                parse_star_count(r#"{"message":"API rate limit exceeded"}"#),
                None
            );
            // A future response that renamed or retyped the field.
            assert_eq!(parse_star_count(r#"{"stargazers_count":"1234"}"#), None);
            assert_eq!(parse_star_count(r#"{"stargazers_count":null}"#), None);
            // Not JSON at all — an HTML error page from a proxy in the way.
            assert_eq!(parse_star_count("<html>502</html>"), None);
            assert_eq!(parse_star_count(""), None);
        }

        /// The default base is public GitHub, an override replaces it, and a
        /// blank override means "unset" rather than "call the empty host".
        #[test]
        fn resolves_the_api_base_from_the_configured_override() {
            assert_eq!(api_base_from(None), DEFAULT_API_BASE);
            assert_eq!(
                api_base_from(Some("https://github.example.com/api/v3".to_string())),
                "https://github.example.com/api/v3"
            );
            // A trailing slash would produce `//repos/…`, which some proxies
            // do not treat as the same path.
            assert_eq!(
                api_base_from(Some("https://github.example.com/api/v3/".to_string())),
                "https://github.example.com/api/v3"
            );
            assert_eq!(api_base_from(Some(String::new())), DEFAULT_API_BASE);
            assert_eq!(api_base_from(Some("   ".to_string())), DEFAULT_API_BASE);
        }

        /// A process that has never refreshed reports no count, and the footer
        /// that reads it renders the link alone.
        ///
        /// The cache is process-wide, so this holds only while nothing in this
        /// binary has published — which is the property the suite depends on:
        /// no test spawns the refresh, so no test reaches the network.
        #[test]
        fn reports_no_count_before_the_first_refresh() {
            assert_eq!(star_count(), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{REPOSITORY_HREF, REPOSITORY_SLUG};

    /// The link and the text under it name the same repository. They are two
    /// constants, so nothing but a test stops one being edited without the
    /// other — and a footer whose text and destination disagree is worse than
    /// one carrying neither.
    #[test]
    fn the_link_and_its_text_name_the_same_repository() {
        assert_eq!(
            REPOSITORY_HREF,
            format!("https://github.com/{REPOSITORY_SLUG}")
        );
    }
}
