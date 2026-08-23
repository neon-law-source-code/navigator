//! Inline-styled HTML wrapper for outbound email.
//!
//! Email clients (Gmail, Outlook) strip `<style>` blocks and external
//! stylesheets and never load SVG `<img>` sources, so this layout uses
//! **inline** styles on a table skeleton and references the shared NL
//! mark as a hosted **PNG** (`/logo-neon.png`, served by `web`'s static
//! asset route). It deliberately shares nothing with the `views`
//! page shell — an email is not a web page, and we don't want the
//! site nav/footer landing in someone's inbox.
//!
//! ## Typeface
//!
//! The body is set in **GORP Serif**, the firm web typeface, declared via
//! a `@font-face` in a `<head>` `<style>` block pointing at the same
//! WOFF2 files served from the deployment asset origin. Be honest about reach: web fonts in
//! email are best-effort — Apple Mail honors `@font-face`, but Gmail
//! and Outlook strip it. Every cell therefore also carries an
//! **inline** `font-family` that leads with `GORP Serif` and falls
//! back to a serif stack (Georgia), so a client that ignores the
//! webfont still renders a serif close in feel, not the default
//! sans-serif. The webfont URL is absolute (from `NAVIGATOR_ASSET_BASE_URL`,
//! or `base_url` for local development) because an inbox has no notion of our origin.
//!
//! Callers render their markdown body (the same source as the
//! plain-text part) through [`render_email_html`] so the two stay in
//! lockstep; the markdown is the single source of truth.

use pulldown_cmark::{html, Options, Parser};

/// Env var carrying the public origin the logo PNG is served from.
/// Mirrors `portal::openapi`'s base-URL knob so a single value drives
/// both the OpenAPI doc and email assets.
const BASE_URL_ENV: &str = "NAV_BASE_URL";
const ASSET_BASE_URL_ENV: &str = "NAVIGATOR_ASSET_BASE_URL";

/// OSS placeholder origin. Real deploys set [`BASE_URL_ENV`]; this
/// default matches the one in `portal::openapi` so the repo ships no
/// hard-coded NeonLaw hostname.
const DEFAULT_BASE_URL: &str = "https://www.your-domain.example";

/// Resolve the public origin for email assets from [`BASE_URL_ENV`],
/// falling back to [`DEFAULT_BASE_URL`]. Any trailing slash is left
/// for [`render_email_html`] to trim.
#[must_use]
pub fn base_url_from_env() -> String {
    if !views::brand::base_url().is_empty() {
        return views::brand::base_url().to_string();
    }
    std::env::var(BASE_URL_ENV).unwrap_or_else(|_| DEFAULT_BASE_URL.to_string())
}

/// Resolve the absolute origin for licensed email webfonts. Production serves
/// them from the public assets bucket. Relative asset paths work for pages,
/// but email clients need a network origin, so they fall back to the site URL.
fn font_base_url(base_url: &str, asset_base_url: Option<&str>) -> String {
    asset_base_url
        .map(str::trim)
        .filter(|base| base.starts_with("https://") || base.starts_with("http://"))
        .unwrap_or(base_url.trim())
        .trim_end_matches('/')
        .to_string()
}

/// The wordmark every email signs with, from the mounted brand bundle.
///
/// A rebranded fork greets under its own name without touching a template: the
/// subject lines, the salutations, and the footer all read this.
#[must_use]
pub fn brand_name() -> &'static str {
    views::brand::FIRM_BRAND.site_name
}

/// The inbound address every email's footer invites a reply to, from the
/// mounted bundle.
///
/// Deliberately not the envelope `From`, which is
/// [`crate::email::DEFAULT_FROM_EMAIL`]: what the site publishes and what the
/// pipeline sends from are allowed to differ.
#[must_use]
pub fn support_email() -> &'static str {
    views::brand::firm_email()
}

/// Render `content_markdown` into a self-contained, inline-styled HTML email
/// document headed by the firm's logo. `base_url` is the public origin where
/// the brand PNG (`/logo-neon.png`) is served (e.g. from
/// [`base_url_from_env`]); a trailing slash is tolerated.
///
/// One brand, resolved from the mounted bundle. Every email this workspace
/// sends is the firm's, so there is no per-message brand to pass and nothing to
/// misattribute — a white-label deploy renames the sender by mounting its own
/// manifest, not by picking a variant at the call site.
///
/// PNG, never SVG — email clients won't render an SVG `<img>` src. The logo is
/// served under `/public/` (the `ServeDir` static mount) rather than at the
/// site root: `/public` is the path that exists as a route, so an email client
/// fetching it unauthenticated gets the PNG (200). A root `/logo-*.png` is
/// unrouted, and every email's logo silently broke on it.
#[must_use]
pub fn render_email_html(content_markdown: &str, base_url: &str) -> String {
    let parser = Parser::new_ext(content_markdown, Options::empty());
    let mut content_html = String::new();
    html::push_html(&mut content_html, parser);

    let base = base_url.trim_end_matches('/');
    // The font origin lands inside a single-quoted CSS `url('…')` in a raw
    // `<style>` block, so it needs the same CSS-string + style-terminator
    // escaping the page layout applies — a `'` or `</style>` in the deployment
    // asset base must not break out of the rule or the style element.
    let font_base = views::assets::css_single_quoted(&font_base_url(
        base,
        std::env::var(ASSET_BASE_URL_ENV).ok().as_deref(),
    ));
    let logo = views::brand::FIRM_BRAND.social_image;
    let alt = brand_name();
    let support = support_email();
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>
@font-face {{
  font-family: 'GORP Serif';
  font-style: normal;
  font-weight: 400;
  src: url('{font_base}/fonts/gorp-serif/GORPSerif-Regular.woff2') format('woff2');
}}
@font-face {{
  font-family: 'GORP Serif';
  font-style: normal;
  font-weight: 700;
  src: url('{font_base}/fonts/gorp-serif/GORPSerif-Bold.woff2') format('woff2');
}}
</style>
</head>
<body style="margin:0;padding:0;background:#f4f4f5;">
<table role="presentation" width="100%" cellpadding="0" cellspacing="0" style="background:#f4f4f5;">
<tr><td align="center" style="padding:24px 12px;">
<table role="presentation" width="600" cellpadding="0" cellspacing="0" style="width:100%;max-width:600px;background:#ffffff;border-radius:8px;">
<tr><td style="padding:32px 32px 0;">
<img src="{base}{logo}" alt="{alt}" width="120" style="display:block;width:120px;height:auto;border:0;">
</td></tr>
<tr><td style="padding:8px 32px 24px;font-family:'GORP Serif',Georgia,'Times New Roman',serif;font-size:16px;line-height:1.5;color:#18181b;">
{content_html}</td></tr>
<tr><td style="padding:16px 32px 28px;border-top:1px solid #e4e4e7;font-family:'GORP Serif',Georgia,'Times New Roman',serif;font-size:13px;line-height:1.5;color:#71717a;">
{alt} · Reach us any time at <a href="mailto:{support}" style="color:#71717a;">{support}</a>.
</td></tr>
</table>
</td></tr>
</table>
</body>
</html>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::{base_url_from_env, font_base_url, render_email_html};

    #[test]
    fn renders_markdown_body_into_html() {
        let html = render_email_html("Hi **Aries**", "https://example.test");
        assert!(
            html.contains("<strong>Aries</strong>"),
            "markdown is rendered"
        );
        assert!(html.starts_with("<!doctype html>"), "full document");
    }

    #[test]
    fn embeds_logo_png_at_base_url_and_trims_trailing_slash() {
        let html = render_email_html("body", "https://example.test/");
        // Served from the exempt `/public` mount, not a gated site root.
        assert!(html.contains(r#"src="https://example.test/public/logo-neon.png""#));
        assert!(html.contains(r#"alt="Neon Law""#));
        // No double slash from a trailing-slash base.
        assert!(!html.contains("example.test//public/logo-neon.png"));
        // Never reference the SVG — clients won't render it.
        assert!(!html.contains("logo-neon.svg"));
    }

    /// Every email signs with the firm's wordmark, and with no other.
    ///
    /// A second brand used to sign some of these — a nonprofit sharing the
    /// firm's family name, which is exactly the case where a wrong wordmark
    /// misattributes the sender. That brand is retired, so this asserts the one
    /// that remains rather than the distinction between two.
    #[test]
    fn every_email_signs_with_the_firms_wordmark() {
        let html = render_email_html("body", "https://example.test");
        assert!(html.contains(r#"src="https://example.test/public/logo-neon.png""#));
        assert!(html.contains(r#"alt="Neon Law""#));
        assert!(
            !html.contains("Foundation"),
            "no email signs under the retired nonprofit's wordmark: {html}"
        );
    }

    #[test]
    fn body_is_set_in_gorp_serif_with_serif_fallback() {
        let html = render_email_html("body", "https://mail.test");
        // @font-face falls back to the local site's static mount when the
        // deployment asset origin is not configured…
        assert!(
            html.contains(
                "src: url('https://mail.test/fonts/gorp-serif/\
                 GORPSerif-Regular.woff2') format('woff2')"
            ),
            "expected absolute @font-face src for GORP Serif regular: {html}"
        );
        // …and the body cell leads with GORP Serif, serif fallback for
        // clients (Gmail/Outlook) that strip the webfont.
        assert!(
            html.contains("font-family:'GORP Serif',Georgia,'Times New Roman',serif;"),
            "expected GORP-Serif-first serif font stack on the body cell: {html}"
        );
        assert!(
            !html.contains("Noto Serif"),
            "superseded email font: {html}"
        );
    }

    #[test]
    fn font_base_prefers_the_deployment_asset_origin() {
        assert_eq!(
            font_base_url(
                "https://app.example.test/",
                Some("https://storage.example.test/navigator-assets/"),
            ),
            "https://storage.example.test/navigator-assets"
        );
        assert_eq!(
            font_base_url("https://app.example.test/", None),
            "https://app.example.test"
        );
        assert_eq!(
            font_base_url("https://app.example.test/", Some("/public")),
            "https://app.example.test"
        );
    }

    #[test]
    fn footer_carries_the_brand_support_address() {
        // The address a recipient is invited to write back to is the one the
        // public site publishes (`views::brand`'s `firm_email`), not the envelope
        // `From` — that is `DEFAULT_FROM_EMAIL`, still `support@`, and the two are
        // deliberately allowed to differ.
        let firm = render_email_html("body", "https://b.test");
        assert!(
            firm.contains("mailto:contact@neonlaw.com"),
            "the footer should carry the firm's published address: {firm}"
        );
    }

    #[test]
    fn font_origin_cannot_break_out_of_the_email_style_block() {
        // The font origin reaches a single-quoted CSS `url('…')` inside a raw
        // `<style>` block. A hostile asset base carrying a quote or `</style>`
        // (here via the site-URL fallback) must be CSS-escaped so it can
        // neither terminate the rule nor close the style element.
        let html = render_email_html(
            "body",
            "https://evil.test/x'</style><script>alert(1)</script>",
        );
        // Inspect the raw `<style>` element in isolation: nothing between its
        // open tag and the first `</style>` may carry markup, or the injected
        // `</style>` would have closed the element early.
        let style_block = html
            .split_once("<style>")
            .and_then(|(_, rest)| rest.split_once("</style>"))
            .map(|(inner, _)| inner)
            .expect("email head must contain a <style> block");
        assert!(
            !style_block.contains('<'),
            "no raw markup may reach the email <style> block: {style_block}"
        );
        assert!(
            style_block.contains(r"\3C /style\3E \3C script\3E "),
            "angle brackets must be CSS-escaped inside the email font url: {style_block}"
        );
    }

    #[test]
    fn autolinks_in_angle_brackets_become_anchors() {
        let html = render_email_html("Visit <https://neonlaw.example> today", "https://b.test");
        assert!(html.contains(r#"href="https://neonlaw.example""#));
    }

    #[test]
    fn base_url_from_env_defaults_to_oss_placeholder_when_unset() {
        // The real value is injected via NAV_BASE_URL in deploys; the
        // default must stay a generic placeholder (no NeonLaw host).
        if std::env::var("NAV_BASE_URL").is_err() {
            assert_eq!(base_url_from_env(), "https://www.your-domain.example");
        }
    }
}
