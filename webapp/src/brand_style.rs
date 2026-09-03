//! The firm's brand style layer — the one stylesheet the public marketing
//! pages hoist on top of `theme.css`.
//!
//! It carries what the firm's public site owns and the shared Navigator theme
//! does not: the serif display face, the section rhythm (eyebrow, heading,
//! card, chip), and the two motion primitives. Page-specific layout stays in
//! the page's own stylesheet (`home.css`, `team.css`), which loads after this
//! one.
//!
//! The palette is *not* here — it lives in `tokens.css` as `--nav-color-*`, so
//! the marketing pages and the product chrome cannot drift to two different
//! browns. This layer only aliases it.
//!
//! It is a brand layer, not a theme fork: every rule is additive over the
//! `--nav-*` tokens, so a page that does not hoist it renders unchanged.

/// The brand stylesheet, hoisted after `theme.css` and before the page's own.
pub const BRAND_STYLESHEET_HREF: &str = "/public/css/brand-firm.css";

/// The firm's colour layer: teal, unchanged from the shared tokens.
///
/// Separate from [`BRAND_STYLESHEET_HREF`] because the two have different
/// audiences. That one carries the firm's marketing rhythm and is hoisted by
/// the four pages that use it; this one carries the palette and is hoisted by
/// `webapp::public_chrome::PublicFooter` on the firm branch, so *every* firm
/// page wears it — including the shared surfaces (`/contact`, `/blog`,
/// `/privacy`) that never hoist a marketing layer. Folding the colour into the
/// marketing file would have rebranded four pages and left the rest teal.
///
/// Fixed to the `neon` registry key rather than resolved per request: the
/// callers that link it (this workshop-slide catalog, and any surface with no
/// live request to resolve) render the firm's own identity regardless of
/// which host serves them. A request-scoped page links [`brand_tokens_href`]
/// instead.
pub const BRAND_TOKENS_HREF: &str = "/public/css/brand-neon-tokens.css";

/// The tokens stylesheet path for registry key `key` (`BrandKey::as_str()`),
/// e.g. `"delete-your-data"` → `/public/css/brand-delete-your-data-tokens.css`.
///
/// A plain string function rather than one taking `views::brand::BrandKey`
/// directly: `views::brand` does not compile to wasm, and this module ships
/// unconditionally (no `#[cfg(feature = "server")]`) because
/// [`BRAND_STYLESHEET_HREF`] and [`BRAND_TOKENS_HREF`] above are plain
/// constants a hydrated client also resolves. The server-only call sites
/// (`app_chrome`, `public_chrome`) convert the resolved `BrandKey` to its
/// string form before calling this.
#[must_use]
pub fn brand_tokens_href(key: &str) -> String {
    format!("/public/css/brand-{key}-tokens.css")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_brand_stylesheet_is_served_from_the_public_mount() {
        assert!(
            BRAND_STYLESHEET_HREF.starts_with("/public/css/"),
            "served from the public mount: {BRAND_STYLESHEET_HREF}"
        );
    }

    /// Every registry key resolves to a distinct file under `/public/css/`,
    /// so a request's resolved brand links its own palette rather than
    /// another brand's.
    #[test]
    fn brand_tokens_href_resolves_per_registry_key() {
        assert_eq!(
            brand_tokens_href("neon"),
            "/public/css/brand-neon-tokens.css"
        );
        assert_eq!(
            brand_tokens_href("delete-your-data"),
            "/public/css/brand-delete-your-data-tokens.css"
        );
        assert_ne!(
            brand_tokens_href("neon"),
            brand_tokens_href("delete-your-data")
        );
    }
}
