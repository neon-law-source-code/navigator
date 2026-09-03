//! Responsive photography: the `asset_url` seam and the curated image
//! manifest.
//!
//! Photos are delivered as multi-resolution `<picture>` elements —
//! AVIF → WebP → JPEG, three width variants each — so phones download
//! the smallest file that fits their viewport. The browser picks the
//! first `<source>` whose `type` it supports, so the formats are
//! emitted smallest-first. The byte-generating
//! half (transcoding the `/tmp` sources into those variants and
//! uploading them to the `<project>-assets` bucket) lives in the
//! `cli assets build` subcommand.
//!
//! ## The `asset_url` seam
//!
//! Every photo path is resolved against [`asset_url`], which prefixes
//! `NAVIGATOR_ASSET_BASE_URL`. It defaults to `/public` so the KIND
//! dev loop, `cargo test`, and OSS forks serve the crate-bundled
//! assets with zero configuration; production points it at the Cloud
//! CDN host (e.g. `https://cdn.your-domain.example`). Nothing here is
//! hard-coded to one deployment.

use std::fmt::Write;
use std::sync::LazyLock;

#[must_use]
pub fn css_single_quoted(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '\'' => escaped.push_str("\\27 "),
            '<' => escaped.push_str("\\3C "),
            '>' => escaped.push_str("\\3E "),
            '\n' => escaped.push_str("\\A "),
            '\r' => escaped.push_str("\\D "),
            '\u{000C}' => escaped.push_str("\\C "),
            control if control.is_control() => {
                write!(escaped, "\\{:X} ", u32::from(control)).expect("write to String");
            }
            _ => escaped.push(character),
        }
    }
    escaped
}

/// Two `@font-face` declarations (Regular 400, Bold 700) for `family`,
/// resolved against `regular_url`/`bold_url` — the bucket-served shape every
/// licensed web font family in this repository shares (GORP Serif, Plus
/// Jakarta Sans). A brand's own font is a new call with its own family name
/// and URLs, never a copy of this builder.
#[must_use]
pub fn font_face_css(family: &str, regular_url: &str, bold_url: &str) -> String {
    let family = css_single_quoted(family);
    let regular_url = css_single_quoted(regular_url);
    let bold_url = css_single_quoted(bold_url);
    format!("@font-face{{font-family:'{family}';font-style:normal;font-weight:400;font-display:swap;src:url('{regular_url}') format('woff2')}}\n@font-face{{font-family:'{family}';font-style:normal;font-weight:700;font-display:swap;src:url('{bold_url}') format('woff2')}}")
}

#[must_use]
pub fn gorp_font_face_css(regular_url: &str, bold_url: &str) -> String {
    font_face_css("GORP Serif", regular_url, bold_url)
}

/// Width variants emitted for every photo, in ascending order. The
/// `<img>` fallback `src` uses [`FALLBACK_WIDTH`]. 1200 is the cap:
/// the source photos are ~2048px on the long edge, a full-width hero
/// reads crisp at 1200, and a 400px tile at 3× retina is exactly
/// 1200 — anything larger is bytes phones download but never show.
pub const WIDTHS: [u32; 3] = [400, 800, 1200];

/// Width of the plain `<img>` `src` fallback (browsers without
/// `srcset` support, and the resource the preload scanner fetches).
pub const FALLBACK_WIDTH: u32 = 1200;

/// Base URL every photo path resolves against. Read once: production
/// sets it via env, dev/test/OSS fall back to the crate-bundled
/// `/public` mount. Only responsive photos route through this seam;
/// vendored JS/CSS (Bootstrap, htmx, Alpine) is linked from the literal
/// same-origin `/public` mount in [`crate::layout`], so it never follows
/// the photo CDN cross-origin.
static ASSET_BASE_URL: LazyLock<String> = LazyLock::new(|| {
    std::env::var("NAVIGATOR_ASSET_BASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "/public".to_string())
});

/// Resolve a repo-relative asset path (e.g. `img/lake-tahoe/...`)
/// against the configured base URL.
#[must_use]
pub fn asset_url(rel: &str) -> String {
    join_base(&ASSET_BASE_URL, rel)
}

/// Resolve a markdown image's `src` against the asset seam. A
/// repo-relative path (`img/thanks-apple/foo.jpg`) routes through
/// [`asset_url`], so its bytes can live in the deployment's assets
/// bucket rather than the repository. An already-absolute source
/// (`http(s)://`, `data:`, or a root-relative `/path`) passes through
/// untouched — that is how the tracked lane (`/public/workshops/...`,
/// shipped inside the container image) keeps working.
///
/// Shared by every markdown surface that renders author-written images,
/// so a slide and a blog post resolve a picture the same way.
#[must_use]
pub fn rewrite_image_src(dest: &str) -> String {
    if dest.starts_with("http://")
        || dest.starts_with("https://")
        || dest.starts_with("data:")
        || dest.starts_with('/')
    {
        return dest.to_string();
    }
    asset_url(dest)
}

/// A configured `NAVIGATOR_ASSET_BASE_URL` that could break out of a
/// context it is interpolated into. Carries the offending value so the
/// boot-time deployment check can name it in the crash message.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error(
    "asset base URL `{0}` is invalid: expected `/public` or an absolute \
     http(s):// origin (host, optional port and path — no query or fragment) \
     containing no markup, quote, backslash, whitespace, or control characters"
)]
pub struct AssetBaseUrlError(pub String);

/// Validate `NAVIGATOR_ASSET_BASE_URL` **once**, at process startup, so
/// every downstream interpolation (the GORP `@font-face` `url('…')` in
/// [`crate::layout`], the email `@font-face` in `workflows`, and the CSP
/// `img-src`/`font-src` origin in `web`) can treat the value as a
/// known-good origin instead of re-escaping it per call site.
///
/// Accepts exactly three shapes:
/// - blank / unset — callers fall back to the same-origin `/public` mount;
/// - the literal `/public` — the KIND / same-origin dev default;
/// - an absolute `http(s)://host[:port][/path]` origin whose every
///   character is a safe ASCII graphic — no markup (`<`/`>`), CSS-string
///   terminators (`'`/`"`/`\`), whitespace, control, or non-ASCII
///   character reaches any interpolation site, and no query/fragment
///   (`?`/`#`) that would swallow the asset path [`asset_url`] appends.
///
/// Everything else — a bare relative path other than `/public`, a
/// non-`http(s)` scheme, an origin with no authority, or any value
/// carrying an unsafe character — is rejected, naming the value.
///
/// # Errors
///
/// Returns [`AssetBaseUrlError`] carrying `value` when it is neither the
/// `/public` default nor a markup-free absolute `http(s)` origin.
pub fn validate_asset_base_url(value: &str) -> Result<(), AssetBaseUrlError> {
    // Blank is "unset": callers fall back to the `/public` default.
    if value.trim().is_empty() {
        return Ok(());
    }
    // The same-origin dev/KIND/test default — matched exactly, so a
    // whitespace-padded or otherwise-decorated relative path is rejected
    // as the config error it is.
    if value == "/public" {
        return Ok(());
    }
    // Otherwise the value must be an absolute `http(s)` origin whose
    // authority is non-empty and whose every character is safe to
    // interpolate into a `<style>` `url('…')` and a CSP host-source.
    let authority = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
        .map(|rest| rest.split(['/', '?', '#']).next().unwrap_or(""));
    if authority.is_some_and(|authority| !authority.is_empty())
        && value.chars().all(is_safe_asset_url_char)
    {
        Ok(())
    } else {
        Err(AssetBaseUrlError(value.to_string()))
    }
}

/// A character safe to carry in an asset base URL: an ASCII graphic that
/// is neither a markup angle bracket, a CSS-string terminator, a
/// backslash, nor a query/fragment delimiter. The first three classes
/// (`<`/`>`, `'`/`"`, `\`) keep the value from breaking out of a
/// single-quoted CSS `url('…')` string or a CSP host-source token; `?`
/// and `#` are rejected because [`asset_url`] joins the asset path onto
/// the base with [`join_base`], and a base carrying a query or fragment
/// would swallow that path (`https://cdn.example#f` + `img/a.jpg` →
/// `https://cdn.example#f/img/a.jpg`, a fragment the browser never
/// fetches). Whitespace and control/non-ASCII characters need no explicit
/// arm because [`char::is_ascii_graphic`] is false for all of them.
fn is_safe_asset_url_char(character: char) -> bool {
    character.is_ascii_graphic() && !matches!(character, '<' | '>' | '\'' | '"' | '\\' | '?' | '#')
}

/// Pure join used by [`asset_url`]; split out so tests can exercise
/// every base form without stomping the process-wide env var (which
/// would race the parallel test runner).
fn join_base(base: &str, rel: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        rel.trim_start_matches('/')
    )
}

/// Canonical public origin of the running site (scheme + host, no
/// trailing slash), read once from `NAV_BASE_URL`. Distinct from
/// [`ASSET_BASE_URL`]: that points at the photo CDN, whereas this is
/// the app's own origin where `/public/...` is served.
///
/// Open Graph / Twitter Card scrapers (Facebook, X, Slack, iMessage,
/// `LinkedIn`, Discord) require **absolute** URLs for `og:image`;
/// relative paths are silently dropped. Empty when unset (KIND,
/// tests, an OSS fork that hasn't configured a hostname) — callers
/// then fall back to the relative path, which is fine in dev where
/// links aren't scraped by external clients.
static SITE_BASE_URL: LazyLock<String> = LazyLock::new(|| {
    std::env::var("NAV_BASE_URL")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
});

/// Resolve a root-relative path (`/public/logo.png`) to an
/// absolute URL against [`SITE_BASE_URL`] for use in social-share
/// meta tags. Returns `rel` unchanged when it is already absolute or
/// when no base URL is configured.
#[must_use]
pub fn absolute_url(rel: &str) -> String {
    join_site(&SITE_BASE_URL, rel)
}

/// Pure join used by [`absolute_url`]; split out so tests can cover
/// every base/`rel` shape without touching the process-wide env var.
fn join_site(base: &str, rel: &str) -> String {
    if base.is_empty() || rel.starts_with("http://") || rel.starts_with("https://") {
        return rel.to_string();
    }
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        rel.trim_start_matches('/')
    )
}

/// Which of the four brand stories a photo tells. Drives nothing in
/// the markup — it is the editorial axis the page authors curate by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    /// Nevada-first: Tahoe, the Mojave, the Las Vegas Strip.
    Nevada,
    /// The globally distributed team: India, Japan.
    Global,
    /// The beautiful things in life: blossoms, birds, gardens.
    Beauty,
    /// The firm's own surface — the photography `www.neonlaw.com`
    /// leads with, curated for the law firm host.
    Firm,
}

/// Editorial aspect classification for a curated photo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aspect {
    /// 21:9 cinematic letterbox — the home hero.
    Hero,
    /// 16:9 — gallery tiles, section banners.
    Wide,
    /// 4:3 — classic landscape.
    Landscape,
    /// 1:1 — square accents.
    Square,
    /// 3:4 — portrait (custom ratio; Bootstrap ships no `ratio-3x4`).
    Portrait,
}

/// One curated photo. `slug` is the URL + directory stem; `source` is
/// the original filename the `cli assets build` step transcodes from.
#[derive(Debug, Clone, Copy)]
pub struct GalleryImage {
    pub slug: &'static str,
    pub theme: Theme,
    pub aspect: Aspect,
    pub alt: &'static str,
    /// Source filename under the build's input directory (`/tmp/...`).
    pub source: &'static str,
}

/// The curated set. Held back by editorial/brand rules and therefore
/// absent: the Raiders mural and Golden Knights billboard (third-party
/// trademarks in commercial advertising) and the Hiroshima A-Bomb Dome
/// (too somber for the firm's bold brand voice).
pub static GALLERY: &[GalleryImage] = &[
    // ── Nevada-first ───────────────────────────────────────────────
    GalleryImage {
        slug: "lake-tahoe",
        theme: Theme::Nevada,
        aspect: Aspect::Hero,
        alt: "Lake Tahoe ringed by snow-capped peaks and pine forest under a clear blue sky",
        source: "photo_00.jpg",
    },
    GalleryImage {
        slug: "mojave-yucca",
        theme: Theme::Nevada,
        aspect: Aspect::Portrait,
        alt: "A yucca and creosote bush on open Mojave Desert gravel under deep blue sky",
        source: "photo_16.jpg",
    },
    GalleryImage {
        slug: "desert-lizard",
        theme: Theme::Nevada,
        aspect: Aspect::Square,
        alt: "A desert lizard sunning on a pale rock with a mountain ridge behind",
        source: "photo_17.jpg",
    },
    GalleryImage {
        slug: "bellagio-horses",
        theme: Theme::Nevada,
        aspect: Aspect::Landscape,
        alt: "Glass mosaic horses leaping above red flowers in the Bellagio Conservatory",
        source: "photo_05.jpg",
    },
    GalleryImage {
        slug: "bellagio-atrium",
        theme: Theme::Nevada,
        aspect: Aspect::Portrait,
        alt: "Butterfly sculptures over a lush flower garden in a glass-domed conservatory",
        source: "photo_15.jpg",
    },
    // ── Globally distributed (India, Japan) ───────────────────────
    GalleryImage {
        slug: "bengaluru-skyline",
        theme: Theme::Global,
        aspect: Aspect::Wide,
        alt: "The green tree canopy and towers of Bengaluru under a wide cloudy sky",
        source: "photo_13.jpg",
    },
    GalleryImage {
        slug: "falaknuma-palace",
        theme: Theme::Global,
        aspect: Aspect::Wide,
        alt: "Manicured palace gardens overlooking the Hyderabad cityscape from Falaknuma Palace",
        source: "photo_10.jpg",
    },
    GalleryImage {
        slug: "india-tricolor-rangoli",
        theme: Theme::Global,
        aspect: Aspect::Portrait,
        alt: "A flower rangoli in the saffron, white and green of the Indian flag with brass bowls",
        source: "photo_12.jpg",
    },
    GalleryImage {
        slug: "kyoto-blossoms",
        theme: Theme::Global,
        aspect: Aspect::Portrait,
        alt: "White cherry blossoms in front of a Kyoto temple gate under blue sky",
        source: "photo_01.jpg",
    },
    // ── Beautiful things in life ──────────────────────────────────
    GalleryImage {
        slug: "migrating-birds",
        theme: Theme::Beauty,
        aspect: Aspect::Wide,
        alt: "A loose V of migrating birds crossing a deep blue sky",
        source: "photo_06.jpg",
    },
    GalleryImage {
        slug: "yellow-rose",
        theme: Theme::Beauty,
        aspect: Aspect::Portrait,
        alt: "A single full-bloom yellow rose against dark green leaves",
        source: "photo_09.jpg",
    },
    GalleryImage {
        slug: "lantana",
        theme: Theme::Beauty,
        aspect: Aspect::Wide,
        alt: "Clusters of pink, orange and yellow lantana flowers among green leaves",
        source: "photo_11.jpg",
    },
    GalleryImage {
        slug: "wa-capitol-blossoms",
        theme: Theme::Beauty,
        aspect: Aspect::Portrait,
        alt: "The Washington State Capitol dome framed by pink cherry blossoms at golden hour",
        source: "photo_14.jpg",
    },
    // ── The firm's own surface ────────────────────────────────────
    GalleryImage {
        slug: "berkeley-bay",
        theme: Theme::Firm,
        aspect: Aspect::Hero,
        // The firm's own words for its own front page. Short, and a description
        // rather than an inventory: a reader who cannot see the photo learns
        // where the firm is from, which is what the picture is doing there.
        alt: "Berkeley, CA; Go Bears!",
        source: "berkeley-bay.jpg",
    },
];

/// Look up a curated photo by slug.
#[must_use]
pub fn find(slug: &str) -> Option<&'static GalleryImage> {
    GALLERY.iter().find(|img| img.slug == slug)
}

/// URL for a single variant. The path is stable across builds, so the
/// `/public` mount (and, in production, the CDN) caches it under a
/// bounded TTL and re-fetches when it expires — no cache-bust token.
fn variant_url(slug: &str, width: u32, ext: &str) -> String {
    asset_url(&format!("img/{slug}/{slug}-{width}w.{ext}"))
}

/// Preload `<link>` href for a hero photo's `<img>` fallback (the
/// resource the browser's preload scanner fetches). Pages pass this to
/// Dioxus so the Largest Contentful Paint image starts downloading
/// before the body parses.
/// `None` for an unknown slug.
#[must_use]
pub fn preload_href(slug: &str) -> Option<String> {
    find(slug).map(|img| variant_url(img.slug, FALLBACK_WIDTH, "jpg"))
}

/// The three formats a `<picture>` negotiates, smallest first — the
/// browser takes the first `type` it supports, so the order *is* the
/// preference. Paired with the MIME type the `<source>` advertises.
const PICTURE_FORMATS: [(&str, &str); 3] = [
    ("avif", "image/avif"),
    ("webp", "image/webp"),
    ("jpg", "image/jpeg"),
];

/// One `<source>` of a responsive `<picture>`: the MIME type the browser
/// tests for support, and the width-keyed candidate set it picks from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PictureSource {
    pub mime: String,
    pub srcset: String,
}

/// Everything a view needs to render one curated photo responsively,
/// resolved to plain owned strings.
///
/// The resolution happens here, server-side, because [`asset_url`] reads
/// process environment: a wasm view cannot ask where the assets live. A
/// server router resolves this once at router-build time and injects the
/// result, exactly as it injects page copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponsivePicture {
    /// `<source>` elements in negotiation order: AVIF, WebP, JPEG.
    pub sources: Vec<PictureSource>,
    /// The `<img>` `src` every browser understands, at [`FALLBACK_WIDTH`].
    pub fallback_src: String,
    /// The manifest's own description of the photo.
    pub alt: String,
    /// `sizes`, describing the layout width so the browser can choose a
    /// candidate before stylesheets resolve.
    pub sizes: String,
}

/// Resolve a manifest photo into its `<picture>` data for the given
/// `sizes` attribute. `None` for a slug that is not in [`GALLERY`] —
/// which is the manifest's whole point as the single source of truth: a
/// typo'd slug renders no image rather than a broken one.
#[must_use]
pub fn responsive_picture(slug: &str, sizes: &str) -> Option<ResponsivePicture> {
    let image = find(slug)?;
    let sources = PICTURE_FORMATS
        .iter()
        .map(|(ext, mime)| PictureSource {
            mime: (*mime).to_string(),
            srcset: WIDTHS
                .iter()
                .map(|&width| format!("{} {width}w", variant_url(image.slug, width, ext)))
                .collect::<Vec<_>>()
                .join(", "),
        })
        .collect();
    Some(ResponsivePicture {
        sources,
        fallback_src: variant_url(image.slug, FALLBACK_WIDTH, "jpg"),
        alt: image.alt.to_string(),
        sizes: sizes.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        asset_url, find, font_face_css, join_base, join_site, validate_asset_base_url, Aspect,
        Theme, GALLERY,
    };

    /// `font_face_css` emits both weights under the given family name, so a
    /// second licensed brand font (DeleteYourData.com's Plus Jakarta Sans) is
    /// a new call with its own family name and URLs, not a copy of GORP's
    /// hard-coded `@font-face` builder.
    #[test]
    fn font_face_css_declares_both_weights_under_the_given_family() {
        let css = font_face_css(
            "Plus Jakarta Sans",
            "https://assets.example.test/fonts/plus-jakarta-sans/PlusJakartaSans-Regular.woff2",
            "https://assets.example.test/fonts/plus-jakarta-sans/PlusJakartaSans-Bold.woff2",
        );
        assert!(css.contains("font-family:'Plus Jakarta Sans'"), "{css}");
        assert!(css.contains("font-weight:400"), "{css}");
        assert!(css.contains("font-weight:700"), "{css}");
        assert!(
            css.contains(
                "url('https://assets.example.test/fonts/plus-jakarta-sans/PlusJakartaSans-Regular.woff2')"
            ),
            "{css}"
        );
    }

    #[test]
    fn validate_accepts_the_public_default_and_blank() {
        // The KIND / same-origin dev shape and "unset" both pass — this is
        // the zero-config default the crate ships with.
        assert!(validate_asset_base_url("/public").is_ok());
        assert!(validate_asset_base_url("").is_ok());
        assert!(validate_asset_base_url("   ").is_ok());
    }

    #[test]
    fn validate_accepts_markup_free_absolute_origins() {
        // The real production shape (a bucket origin) plus a dev host:port.
        assert!(validate_asset_base_url("https://storage.example.test/navigator-assets").is_ok());
        assert!(validate_asset_base_url("http://localhost:8080").is_ok());
        assert!(validate_asset_base_url("https://cdn.example.com").is_ok());
    }

    #[test]
    fn validate_rejects_a_style_breakout_and_names_the_value() {
        // The exact hostile shape #493 escaped per-site: a quote + `</style>`
        // that would otherwise close the raw `<style>` element.
        let hostile = "https://evil.test/x'</style><script>";
        let err = validate_asset_base_url(hostile).unwrap_err();
        assert_eq!(err.0, hostile);
        // The message must name the offending value so an operator can fix
        // the deploy from the crash log alone.
        assert!(err.to_string().contains(hostile), "{err}");
    }

    #[test]
    fn validate_rejects_bare_relative_paths_other_than_public() {
        // Only `/public` is a legitimate relative base; anything else can
        // carry no CSP origin and is not a shape any deploy uses.
        assert!(validate_asset_base_url("/assets").is_err());
        assert!(validate_asset_base_url("public").is_err());
        assert!(validate_asset_base_url(" /public ").is_err());
        assert!(validate_asset_base_url("/public/img").is_err());
    }

    #[test]
    fn validate_rejects_non_http_schemes_and_empty_authorities() {
        assert!(validate_asset_base_url("ftp://cdn.example.test").is_err());
        // `http(s)://` with no host — an origin needs an authority.
        assert!(validate_asset_base_url("https://").is_err());
        assert!(validate_asset_base_url("https:///path").is_err());
    }

    #[test]
    fn validate_rejects_query_and_fragment_bases() {
        // `asset_url` appends the asset path onto the base, so a base
        // carrying a query or fragment would swallow it — the browser
        // would resolve `https://cdn.example.test#f/img/a.jpg` as a
        // fragment and never fetch the file. Reject both delimiters.
        assert!(validate_asset_base_url("https://cdn.example.test#fonts").is_err());
        assert!(validate_asset_base_url("https://cdn.example.test?v=1").is_err());
        // Sanity: the join a passing base would produce stays a real path.
        assert_eq!(
            asset_url("img/a.jpg"),
            "/public/img/a.jpg",
            "the default base joins cleanly"
        );
    }

    #[test]
    fn validate_rejects_unsafe_characters_in_an_absolute_origin() {
        // Backslash, an interior space, and an interior control character
        // each fail even though the value is otherwise an absolute origin —
        // none may reach a `<style>` `url('…')` or a CSP host-source.
        assert!(validate_asset_base_url("https://cdn.example.test/a\\b").is_err());
        assert!(validate_asset_base_url("https://cdn.example.test/a b").is_err());
        assert!(validate_asset_base_url("https://cdn.example.test/a\nb").is_err());
        assert!(validate_asset_base_url("https://cdn.example.test/\"q\"").is_err());
    }

    #[test]
    fn asset_url_defaults_to_public_mount() {
        // With no env override the seam must resolve to the
        // crate-bundled `/public` mount so dev/KIND/tests work.
        assert_eq!(
            asset_url("img/lake-tahoe/lake-tahoe-800w.avif"),
            "/public/img/lake-tahoe/lake-tahoe-800w.avif"
        );
    }

    #[test]
    fn join_base_normalizes_slashes_for_any_base() {
        // Trailing slash on base, leading slash on rel — exactly one
        // separator either way.
        assert_eq!(join_base("/public", "img/a.avif"), "/public/img/a.avif");
        assert_eq!(join_base("/public/", "/img/a.avif"), "/public/img/a.avif");
        assert_eq!(
            join_base("https://cdn.example.com", "img/a.avif"),
            "https://cdn.example.com/img/a.avif"
        );
    }

    #[test]
    fn join_site_makes_relative_paths_absolute_against_the_origin() {
        // With a configured origin, a `/public/...` logo path becomes
        // the absolute URL a social scraper can fetch.
        assert_eq!(
            join_site("https://www.neonlaw.com", "/public/logo.png"),
            "https://www.neonlaw.com/public/logo.png"
        );
        // Trailing slash on base, no leading slash on rel — still one
        // separator.
        assert_eq!(
            join_site("https://www.neonlaw.com/", "public/logo.png"),
            "https://www.neonlaw.com/public/logo.png"
        );
    }

    #[test]
    fn join_site_passes_through_when_unconfigured_or_already_absolute() {
        // No NAV_BASE_URL (dev/KIND/tests): keep the relative path.
        assert_eq!(join_site("", "/public/logo.png"), "/public/logo.png");
        // Already-absolute image (e.g. a CDN URL): leave it untouched
        // rather than double-prefixing.
        assert_eq!(
            join_site(
                "https://www.neonlaw.com",
                "https://cdn.example.com/logo.png"
            ),
            "https://cdn.example.com/logo.png"
        );
    }

    #[test]
    fn gallery_excludes_trademarked_and_somber_images() {
        // Editorial/brand guardrail: the held-back set must never be
        // wired into a marketing surface via the manifest.
        for banned in ["raider", "golden-knight", "cosmopolitan", "hiroshima"] {
            assert!(
                GALLERY.iter().all(|i| !i.slug.contains(banned)),
                "manifest must not carry held-back image `{banned}`"
            );
        }
    }

    #[test]
    fn gallery_covers_every_theme() {
        for theme in [Theme::Nevada, Theme::Global, Theme::Beauty, Theme::Firm] {
            assert!(
                GALLERY.iter().any(|i| i.theme == theme),
                "every brand theme needs at least one photo: {theme:?}"
            );
        }
    }

    #[test]
    fn gallery_slugs_are_unique() {
        let mut slugs: Vec<_> = GALLERY.iter().map(|i| i.slug).collect();
        slugs.sort_unstable();
        let before = slugs.len();
        slugs.dedup();
        assert_eq!(before, slugs.len(), "gallery slugs must be unique");
    }

    #[test]
    fn the_hero_is_lake_tahoe_in_nevada() {
        let hero = find("lake-tahoe").expect("lake-tahoe in manifest");
        assert_eq!(hero.theme, Theme::Nevada);
        assert_eq!(hero.aspect, Aspect::Hero);
    }

    #[test]
    fn preload_href_points_at_the_jpeg_fallback() {
        use super::{preload_href, FALLBACK_WIDTH};
        let href = preload_href("lake-tahoe").expect("hero has a preload href");
        assert!(href.contains(&format!("lake-tahoe-{FALLBACK_WIDTH}w.jpg")));
        assert!(preload_href("no-such-photo").is_none());
    }

    #[test]
    fn the_firms_hero_is_a_hero_aspect_photo_on_the_firm_theme() {
        let hero = find("berkeley-bay").expect("berkeley-bay in manifest");
        assert_eq!(hero.theme, Theme::Firm);
        assert_eq!(hero.aspect, Aspect::Hero);
        assert!(
            hero.alt.len() > 20,
            "the firm's front-page photo carries a real description: {}",
            hero.alt
        );
    }

    #[test]
    fn responsive_picture_negotiates_avif_then_webp_then_jpeg() {
        use super::{responsive_picture, FALLBACK_WIDTH, WIDTHS};
        let picture = responsive_picture("berkeley-bay", "100vw").expect("a manifest photo");

        // Smallest format first: the browser takes the first `type` it
        // supports, so this order is the preference, not decoration.
        let mimes: Vec<_> = picture.sources.iter().map(|s| s.mime.as_str()).collect();
        assert_eq!(mimes, ["image/avif", "image/webp", "image/jpeg"]);

        // Every source offers every built width, keyed by `w` so the
        // browser can choose before layout.
        for source in &picture.sources {
            for width in WIDTHS {
                assert!(
                    source.srcset.contains(&format!("-{width}w.")),
                    "{} offers the {width}w variant: {}",
                    source.mime,
                    source.srcset
                );
                assert!(
                    source.srcset.contains(&format!(" {width}w")),
                    "{} keys its candidates by width: {}",
                    source.mime,
                    source.srcset
                );
            }
        }

        assert!(
            picture
                .fallback_src
                .ends_with(&format!("berkeley-bay-{FALLBACK_WIDTH}w.jpg")),
            "the <img> fallback is the JPEG every browser reads: {}",
            picture.fallback_src
        );
        assert_eq!(picture.sizes, "100vw");
        // Carried through from the manifest rather than invented by the
        // resolver — the assertion is that the alt survives the trip, not what
        // the firm chose to say.
        assert_eq!(picture.alt, find("berkeley-bay").expect("in manifest").alt);
        assert!(!picture.alt.is_empty(), "the photo is described");
    }

    #[test]
    fn responsive_picture_is_none_for_a_slug_outside_the_manifest() {
        use super::responsive_picture;
        // The manifest is the single source of truth: a typo renders no
        // image rather than a `<picture>` of 404s.
        assert!(responsive_picture("no-such-photo", "100vw").is_none());
    }
}
