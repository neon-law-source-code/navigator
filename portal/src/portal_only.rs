//! White-label "portal-only" mode.
//!
//! When the mounted brand manifest sets `portal_only: true`, `web` mounts only
//! the *application* surface — `/app`, auth, the JSON `/api`, `/mcp`,
//! the git transport, webhooks, the health probes, and the legal pages —
//! and drops the entire public marketing surface (the firm
//! home page, `/contact`, `/team`, `/blog`, the workshops
//! and presentations). The bare host `/` 303-redirects to `/app/projects`.
//!
//! The use case is a law firm that deploys Neon Law Navigator under its own brand:
//! it already runs its own marketing website (WordPress, a marketing
//! team) and only wants Neon Law Navigator to be the client portal + workflow
//! engine, not a second public site. See `docs/oss-install.md` and the
//! "Operating Neon Law Navigator" workshop.
//!
//! Disabled by default — NeonLaw's own deploy serves the full public
//! site, so the flag ships off and the router is unchanged unless it is
//! lit. Portal-only decides *whether the public pages exist at all*.

/// Bundle-driven toggle for portal-only mode. `Copy` so [`crate::AppState`]
/// can hand it to [`crate::bootstrap`] without a clone.
#[derive(Debug, Clone, Copy, Default)]
pub struct PortalOnly(bool);

impl PortalOnly {
    /// Construct explicitly (tests build the router both ways without
    /// stomping the process env).
    #[must_use]
    pub fn new(enabled: bool) -> Self {
        Self(enabled)
    }

    /// True when the marketing surface should be suppressed.
    #[must_use]
    pub fn enabled(self) -> bool {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::PortalOnly;

    #[test]
    fn default_is_disabled() {
        assert!(!PortalOnly::default().enabled());
    }

    #[test]
    fn new_round_trips() {
        assert!(PortalOnly::new(true).enabled());
        assert!(!PortalOnly::new(false).enabled());
    }
}
