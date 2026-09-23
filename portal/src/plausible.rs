//! Plausible page analytics — a per-brand injection into the public page
//! shell, on for the real production deployment only.
//!
//! Each brand face is its own Plausible site with its own script, so the
//! script id is compiled in per [`BrandKey`] by [`script_id`].
//!
//! The ids are the firm's, so the gate is the deployment: [`enabled_from`]
//! admits only an explicit `NAVIGATOR_ENVIRONMENT=production` whose matters
//! are not simulated. That keeps local runs, tests, KIND, and the staging ring
//! (a production profile carrying simulated matters) from counting visits
//! against the live dashboards. An unset `NAVIGATOR_ENVIRONMENT` resolves to
//! the production profile elsewhere, but not here: a bare `cargo run` is not
//! the firm's website.
//!
//! The snippet Plausible publishes is an async vendor script plus an inline
//! stub that queues calls and runs `plausible.init()`. The stub is served
//! instead as the first-party [`PLAUSIBLE_LOADER_HREF`], so it needs no nonce;
//! see [`PlausibleSite::script_tags`] for the ordering that makes that work.

use views::brand::BrandKey;

/// Plausible Cloud, which serves the script and receives the events.
pub const PLAUSIBLE_ORIGIN: &str = "https://plausible.io";

/// The first-party stub that queues calls until the vendor script arrives.
pub const PLAUSIBLE_LOADER_HREF: &str = "/public/js/plausible.js";

/// The Plausible script id (the `pa-…` stem of the snippet's script URL) for
/// `key`'s public site.
///
/// Not a secret: it ships in the HTML every visitor receives.
#[must_use]
pub const fn script_id(key: BrandKey) -> &'static str {
    match key {
        BrandKey::Neon => "pa-dktgfAn-5R5ufpXARu6zb",
        BrandKey::Vesta => "pa-0NXcwgtQALjMsqFs8YbfN",
        BrandKey::DeleteYourData => "pa-h5nRTkQB8L0g9LJDlCctG",
        BrandKey::LawyerShook => "pa-tN2nM3ILjPwZ3kHAqRT2j",
        BrandKey::Abhaya => "pa-VFh7XqWwYjyX-p1k26flH",
        BrandKey::Misericordia => "pa-FRRt7d62fXf8Ixsc6ZmSh",
        BrandKey::DeleteYourDebt => "pa-S3SbSActjysFwN64GUe6D",
        BrandKey::Daybridge => "pa-j2JWmqoBTNJPRb64xIZ2o",
        BrandKey::Summons => "pa-WUB2cKHxULoouY7ALHjwH",
    }
}

/// Whether this deployment counts visits: an explicit production profile with
/// real (not simulated) matters. `get` reads the environment, so every shape
/// is unit-testable without mutating process state.
pub fn enabled_from<F: Fn(&str) -> Option<String>>(get: F) -> bool {
    let explicit_production =
        get(store::config::NAVIGATOR_ENVIRONMENT).as_deref() == Some("production");
    explicit_production
        && store::sample_matters_from(store::DeploymentEnvironment::Production, get) == Ok(false)
}

/// One brand's Plausible site, named by its script id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlausibleSite {
    script_id: &'static str,
}

impl PlausibleSite {
    /// `key`'s compiled site.
    #[must_use]
    pub const fn for_brand(key: BrandKey) -> Self {
        Self::new(script_id(key))
    }

    /// A site with `script_id`, for tests that inject one on the request.
    #[must_use]
    pub const fn new(script_id: &'static str) -> Self {
        Self { script_id }
    }

    /// The two `<script>` elements that start analytics, for injection at the
    /// end of a public page's `<body>`: the first-party stub, then the vendor
    /// script.
    ///
    /// Both carry `defer`, and the order is load-bearing. Deferred classic
    /// scripts execute in document order, so the stub has defined
    /// `window.plausible` and queued `init()` before the vendor script runs
    /// and drains that queue — the guarantee Plausible's own snippet gets from
    /// an inline block after an `async` tag.
    #[must_use]
    pub fn script_tags(&self) -> String {
        format!(
            "<script src=\"{PLAUSIBLE_LOADER_HREF}\" defer></script>\
             <script src=\"{PLAUSIBLE_ORIGIN}/js/{id}.js\" defer></script>",
            id = webapp::html_escape::escape_attr(self.script_id),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{enabled_from, script_id, PlausibleSite, PLAUSIBLE_LOADER_HREF};
    use views::brand::BrandKey;

    /// `enabled_from` end to end, over every shape the deployment environment
    /// can take. One injected lookup per row, rather than one test per case.
    #[test]
    fn only_explicit_production_without_simulated_matters_is_enabled() {
        let env = "NAVIGATOR_ENVIRONMENT";
        let sim = "NAVIGATOR_SIMULATED_MATTERS";
        let cases: &[(&[(&str, &str)], bool)] = &[
            (&[(env, "production"), (sim, "false")], true),
            (&[(env, "production")], true),
            // Staging: the production profile over simulated matters.
            (&[(env, "production"), (sim, "true")], false),
            (&[(env, "dev")], false),
            (&[(env, "dev"), (sim, "false")], false),
            // Unset is the local default and the test process, not the website.
            (&[], false),
            (&[(env, "")], false),
            (&[(env, "production"), (sim, "yes")], false),
        ];
        for (pairs, want) in cases {
            let got = enabled_from(|key| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| (*v).to_string())
            });
            assert_eq!(got, *want, "{pairs:?}");
        }
    }

    /// Every brand resolves a distinct, well-shaped script id — the one thing
    /// worth asserting about a table of compiled constants.
    #[test]
    fn every_brand_resolves_a_distinct_plausible_script_id() {
        let ids: Vec<_> = BrandKey::ALL.iter().map(|key| script_id(*key)).collect();
        for id in &ids {
            assert!(id.starts_with("pa-"), "{id}");
            assert!(
                id.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
                "{id}"
            );
        }
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(ids.len(), unique.len(), "{ids:?}");
    }

    /// Stub first, vendor second, both deferred, neither inline.
    #[test]
    fn the_script_tags_are_the_stub_then_the_vendor_script() {
        let tags = PlausibleSite::new("pa-dktgfAn-5R5ufpXARu6zb").script_tags();
        assert_eq!(
            tags,
            "<script src=\"/public/js/plausible.js\" defer></script>\
             <script src=\"https://plausible.io/js/pa-dktgfAn-5R5ufpXARu6zb.js\" defer></script>"
        );
        assert!(tags.find(PLAUSIBLE_LOADER_HREF).unwrap() < tags.find("plausible.io/js").unwrap());
    }
}
