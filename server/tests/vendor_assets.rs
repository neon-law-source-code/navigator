//! Provenance guard for vendored front-end assets.
//!
//! `web/public/VENDOR.toml` is the single source of truth for every
//! third-party CSS/JS/font we vend. This test recomputes the SHA-256 of each
//! `served_path` and asserts
//! it equals the recorded `sha256`. If someone hand-edits a vendored blob, or
//! new bytes land without updating the manifest, this fails — so the manifest
//! can never silently drift from disk.
//!
//! Same shape as `store/tests/timestamp_convention.rs`: a convention enforced
//! by a test, not by discipline.

use std::fmt::Write as _;
use std::path::PathBuf;

use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
struct Manifest {
    asset: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    version: String,
    name: String,
    served_path: String,
    sha256: String,
}

fn public_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("public")
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

#[test]
fn vendored_assets_match_manifest() {
    let public = public_dir();
    let manifest_path = public.join("VENDOR.toml");
    let raw = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", manifest_path.display()));
    let manifest: Manifest =
        toml::from_str(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", manifest_path.display()));

    assert!(
        !manifest.asset.is_empty(),
        "VENDOR.toml lists no assets — did the [[asset]] tables get dropped?"
    );

    for asset in &manifest.asset {
        let path = public.join(&asset.served_path);
        let bytes = std::fs::read(&path).unwrap_or_else(|e| {
            panic!(
                "{} ({}): cannot read served_path {}: {e}",
                asset.name,
                asset.served_path,
                path.display()
            )
        });
        let actual = hex_lower(&Sha256::digest(&bytes));
        assert_eq!(
            actual, asset.sha256,
            "{} ({}): on-disk SHA-256 does not match VENDOR.toml.\n  \
             expected {}\n  actual   {}\n\
             Refresh the vendored asset and manifest together, or update the \
             manifest if this change is intentional.",
            asset.name, asset.served_path, asset.sha256, actual
        );
    }
}

#[test]
fn bootstrap_assets_are_not_shipped() {
    let public = public_dir();
    let raw = std::fs::read_to_string(public.join("VENDOR.toml")).expect("read VENDOR.toml");
    let manifest: Manifest = toml::from_str(&raw).expect("parse VENDOR.toml");

    assert!(
        manifest
            .asset
            .iter()
            .all(|asset| !asset.name.contains("Bootstrap")
                && !asset.served_path.contains("bootstrap")),
        "VENDOR.toml must not retain a Bootstrap distribution"
    );
    for removed in [
        "css/bootstrap.min.css",
        "js/bootstrap.bundle.min.js",
        "icons/bootstrap-icons.css",
        "icons/fonts/bootstrap-icons.woff2",
    ] {
        assert!(
            !public.join(removed).exists(),
            "removed Bootstrap asset still ships at {removed}"
        );
    }
}

/// Swagger UI is the only third-party browser distribution Navigator vendors.
///
/// Fonts licensed directly to the firm live under `public/fonts/` with their
/// own notices, while application CSS and JavaScript are first-party source.
/// A new framework therefore needs an explicit architecture decision and a
/// reviewed change to this allowlist instead of silently gaining a manifest
/// entry.
#[test]
fn only_swagger_ui_is_an_approved_vendored_browser_distribution() {
    let public = public_dir();
    let raw = std::fs::read_to_string(public.join("VENDOR.toml")).expect("read VENDOR.toml");
    let manifest: Manifest = toml::from_str(&raw).expect("parse VENDOR.toml");

    let unexpected = manifest
        .asset
        .iter()
        .filter(|asset| {
            !asset.name.starts_with("Swagger UI ") || !asset.served_path.starts_with("swagger-ui/")
        })
        .map(|asset| format!("{} ({})", asset.name, asset.served_path))
        .collect::<Vec<_>>();

    assert!(
        unexpected.is_empty(),
        "VENDOR.toml contains an unapproved browser distribution: {unexpected:?}"
    );
}

/// Files under `public/swagger-ui/` that are ours, not upstream: the Navigator
/// banner page, the same-origin bootstrap that keeps the `/docs` route's CSP at
/// `script-src 'self'`, and the `VERSION` stamp left by the vendoring.
/// Everything else in that directory ships from `swagger-ui-dist` and must be
/// pinned.
const SWAGGER_UI_FIRST_PARTY: &[&str] = &["index.html", "init.js", "VERSION"];

/// The manifest test above only proves that *listed* assets match disk — it is
/// blind to a vendored bundle nobody listed, which is exactly how the Swagger
/// UI tree shipped unpinned. This closes that hole for the one directory that
/// is wholly vendored: every file is either in `VENDOR.toml` or named as
/// first-party. An explicit waiver, never a silent pass.
#[test]
fn every_upstream_swagger_ui_asset_is_pinned() {
    let public = public_dir();
    let raw = std::fs::read_to_string(public.join("VENDOR.toml")).expect("read VENDOR.toml");
    let manifest: Manifest = toml::from_str(&raw).expect("parse VENDOR.toml");

    let dir = public.join("swagger-ui");
    let mut unpinned = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("read the swagger-ui directory") {
        let entry = entry.expect("read a swagger-ui directory entry");
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if SWAGGER_UI_FIRST_PARTY.contains(&file_name.as_str()) {
            continue;
        }
        let served = format!("swagger-ui/{file_name}");
        if !manifest.asset.iter().any(|a| a.served_path == served) {
            unpinned.push(served);
        }
    }

    assert!(
        unpinned.is_empty(),
        "vendored Swagger UI files are served but not pinned in VENDOR.toml: {unpinned:?}\n\
         Add an [[asset]] entry recording the upstream version, URL, and SHA-256 — \
         or add the file to SWAGGER_UI_FIRST_PARTY if we wrote it."
    );

    // Two records of the same fact, so a refresh cannot update one and leave
    // the other stale: the `VERSION` stamp on disk and every manifest entry
    // for this directory must name the same upstream release.
    let stamped = std::fs::read_to_string(dir.join("VERSION")).expect("read the VERSION stamp");
    let stamped = stamped.trim();
    for asset in manifest
        .asset
        .iter()
        .filter(|a| a.served_path.starts_with("swagger-ui/"))
    {
        assert_eq!(
            asset.version, stamped,
            "{} pins {} but public/swagger-ui/VERSION says {stamped} — \
             refresh the bytes, the manifest, and the stamp together",
            asset.name, asset.version,
        );
    }
}

#[test]
fn gorp_license_notice_separates_code_and_font_licenses() {
    let notice = std::fs::read_to_string(public_dir().join("fonts/gorp-serif/LICENSE.txt"))
        .expect("read the tracked GORP license notice");
    assert!(notice.contains("Neon Law IP LLC") && notice.contains("proprietary"));
    assert!(notice.contains("licensed separately from TrashType"));
    assert!(notice.contains("https://trashtype.com/legal"));
    assert!(!notice.contains("sample purposes"));
}

/// The reusable token module names the firm's typeface, while `theme.css`
/// imports it before applying shared component rules.
#[test]
fn the_dioxus_theme_is_set_in_gorp_serif() {
    let theme = std::fs::read_to_string(public_dir().join("css/theme.css"))
        .expect("read the Dioxus Components theme");
    let tokens = std::fs::read_to_string(public_dir().join("css/tokens.css"))
        .expect("read Navigator token module");

    assert!(
        tokens.contains("--nav-font-family: \"GORP Serif\", Georgia, serif;"),
        "tokens.css must name GORP Serif as the brand family"
    );
    assert!(
        theme.contains("@import url(\"/public/css/tokens.css\");"),
        "theme.css must load the reusable token module first"
    );
    assert!(
        !tokens.contains("system-ui"),
        "no surface may fall back to a system font stack instead of GORP Serif"
    );
    // The quoted CSS-string form, as `catalog.rs` checks the other stylesheets:
    // the prose above may name the fallback the browser picks, but no rule may
    // declare it.
    assert!(
        !tokens.contains("\"Times New Roman\""),
        "the firm's copy stays GORP Serif"
    );
    // The page chrome (`.lawyer-nav`, headings, flash banners) is a sibling of
    // `<main class="nav-theme">`, so the family has to be inherited from the
    // document root rather than declared on the component surface alone. The
    // root rule carries the rest of the baseline too, so read its declarations
    // instead of pinning the whole block.
    let root_rule = theme
        .split_once("html {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(declarations, _)| declarations)
        .expect("theme.css must carry a document-root rule");
    assert!(
        root_rule.contains("font-family: var(--nav-font-family);"),
        "the family must be declared on the document root so page chrome inherits it"
    );
    // Form controls take the UA font, not the inherited one, so every button in
    // a row-action cell would otherwise render in the platform sans.
    assert!(
        theme.contains("button,\ninput,\nselect,\ntextarea {\n  font: inherit;\n}"),
        "form controls must inherit the brand font instead of the UA default"
    );
}

#[test]
fn workshop_step_media_fits_the_visible_page_without_changing_display_mode() {
    let css = std::fs::read_to_string(public_dir().join("css/catalog.css"))
        .expect("read the workshop stylesheet");

    let media_frame = css
        .split_once(".workshop-step .workshop-slide .material-body p:has(> img, > video) {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(declarations, _)| declarations)
        .expect("the regular workshop page must frame image and video media together");
    assert!(media_frame.contains("align-items: center;"));
    assert!(media_frame.contains("padding-block:"));

    let media = css
        .split_once(".workshop-step .workshop-slide .material-body :is(img, video) {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(declarations, _)| declarations)
        .expect("the regular workshop page must size image and video media together");
    assert!(
        media.contains("max-height: min(100%, 45dvh);"),
        "regular workshop media must remain within the visible page"
    );
}

#[test]
fn render_demo_slides_place_the_command_beside_the_document() {
    let css = std::fs::read_to_string(public_dir().join("css/catalog.css"))
        .expect("read the workshop stylesheet");

    let layout = css
        .split_once(".workshop-slide > .material-body:has(.nav-code + p > img) {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(declarations, _)| declarations)
        .expect("render-demo slides must own a code-and-document grid");
    assert!(layout.contains("display: grid;"));
    assert!(layout.contains("grid-template-columns:"));
    assert!(layout.contains("grid-template-rows: auto minmax(0, 1fr);"));

    let document = css
        .split_once(".workshop-slide > .material-body:has(.nav-code + p > img) > p:has(> img) {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(declarations, _)| declarations)
        .expect("the rendered document must occupy the second grid column");
    assert!(document.contains("grid-column: 2;"));
    assert!(document.contains("grid-row: 2;"));
}

#[test]
fn presentation_practice_cards_hold_the_hover_treatment_without_motion() {
    let css = std::fs::read_to_string(public_dir().join("css/catalog.css"))
        .expect("read the workshop stylesheet");

    let card = css
        .split_once(".workshop-slide .workshop-product-cards > .home-practice {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(declarations, _)| declarations)
        .expect("the presentation must style its practice cards");
    assert!(card.contains("border-color: var(--firm-brand);"));
    assert!(card.contains("transform: translateY(-2px);"));
    assert!(card.contains("transition: none;"));

    let heading = css
        .split_once(".workshop-slide .workshop-product-cards .home-practice__heading {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(declarations, _)| declarations)
        .expect("the presentation must style its practice-card headings");
    assert!(heading.contains("color: var(--firm-brand-strong);"));

    let wash = css
        .split_once(".workshop-slide .workshop-product-cards > .home-practice::after {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(declarations, _)| declarations)
        .expect("the presentation must keep the practice-card hover wash visible");
    assert!(wash.contains("opacity: 0.22;"));
    assert!(wash.contains("transform: scale(1.5);"));
    assert!(wash.contains("transition: none;"));
}

#[test]
fn navigator_product_slide_keeps_its_heading_left_aligned() {
    let css = std::fs::read_to_string(public_dir().join("css/catalog.css"))
        .expect("read the workshop stylesheet");

    let heading = css
        .split_once(".workshop-navigator-slide h3 {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(declarations, _)| declarations)
        .expect("the Navigator product slide must own its heading alignment");
    assert!(heading.contains("justify-self: stretch;"));
    assert!(heading.contains("text-align: left;"));
}

#[test]
fn slide_image_authoring_keeps_the_local_staging_and_production_copies_together() {
    let guide =
        std::fs::read_to_string(repo_root().join(".agents/skills/authoring-slides/SKILL.md"))
            .expect("read the slide-authoring guide");

    let local = guide
        .find("server/public/img/<deck-slug>/<filename>")
        .expect("slide images must name their ignored local source path");
    let staging = guide
        .find("neon-law-stg-assets")
        .expect("slide images must name the staging publication target");
    let production = guide
        .find("<production>-assets")
        .expect("slide images must name the production publication target");

    assert!(
        local < staging && staging < production,
        "slide image guidance must teach local preview, staging, then production"
    );
    assert!(
        guide.contains("If production remains pending, say"),
        "an incomplete production handoff must stay explicit"
    );
}

/// A blog post's body is authored in Markdown, so every picture arrives as a
/// bare `<img>` with no width of its own. Without a cap it lays out at the
/// file's intrinsic pixel width and overruns the article's 65ch measure, so the
/// photo no longer aligns with the paragraphs around it.
#[test]
fn a_blog_post_picture_is_capped_to_the_post_measure() {
    let theme = std::fs::read_to_string(public_dir().join("css/theme.css"))
        .expect("read the Dioxus Components theme");

    let rule = theme
        .split_once(".blog-post img {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(declarations, _)| declarations)
        .expect("theme.css must cap pictures inside a blog post");
    assert!(
        rule.contains("max-width: 100%;"),
        "a picture may not exceed the post's measure: {rule}"
    );
    assert!(
        rule.contains("height: auto;"),
        "the height must follow the capped width so the picture is not squashed: {rule}"
    );
    assert!(
        rule.contains("display: block;"),
        "a block picture starts on the prose's own left edge: {rule}"
    );
}

/// The firm home page opens on its question, not on a photograph. The
/// stylesheet therefore carries no hero band at all: a hero rule left behind
/// would be dead CSS today and a temptation to put type over pixels tomorrow,
/// which is the contrast defect the public accessibility gate
/// (`server/tests/accessibility_e2e.rs`) exists to catch.
#[test]
fn the_home_stylesheet_carries_no_hero_band() {
    let home = std::fs::read_to_string(public_dir().join("css/home.css"))
        .expect("read the firm home stylesheet");
    assert!(
        !home.contains(".home-hero"),
        "home.css must not carry a hero rule: {home}"
    );
    let heading = home
        .split_once(".home-statement__heading {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(declarations, _)| declarations)
        .expect("home.css must size the question");
    assert!(
        heading.contains("font-size: clamp(2.75rem, 8vw, 5rem);"),
        "the question is the page, so it is set large: {heading}"
    );
}

#[test]
fn the_home_band_and_footer_share_the_theme_wash_and_hairline() {
    let theme = std::fs::read_to_string(public_dir().join("css/theme.css"))
        .expect("read the shared theme stylesheet");
    let home = std::fs::read_to_string(public_dir().join("css/home.css"))
        .expect("read the firm home stylesheet");

    let theme_tokens = theme
        .split_once(".nav-theme {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(declarations, _)| declarations)
        .expect("theme.css must carry the public theme rule");
    assert!(
        theme_tokens.contains("--nav-public-wash:")
            && theme_tokens.contains("radial-gradient(")
            && theme_tokens.contains("--nav-public-hairline:")
            && theme_tokens.contains("linear-gradient(\n    90deg,"),
        "theme.css must own the shared public wash and hairline values: {theme_tokens}"
    );

    let service_before = home
        .split_once(".home-service::before {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(declarations, _)| declarations)
        .expect("home.css must carry the engagements wash rule");
    assert!(
        service_before.contains("background: var(--nav-public-wash);"),
        "the engagements band must consume the theme wash: {service_before}"
    );
    assert!(
        !service_before.contains("radial-gradient("),
        "the engagements band must not duplicate the theme wash: {service_before}"
    );

    let service_after = home
        .split_once(".home-service::after {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(declarations, _)| declarations)
        .expect("home.css must carry the engagements hairline rule");
    assert!(
        service_after.contains("background: var(--nav-public-hairline);"),
        "the engagements hairline must consume the theme value: {service_after}"
    );

    let footer = theme
        .split_once(".site-footer {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(declarations, _)| declarations)
        .expect("theme.css must carry the footer rule");
    assert!(
        footer.contains("background: var(--nav-public-wash);"),
        "the footer must consume the theme wash: {footer}"
    );

    let footer_before = theme
        .split_once(".site-footer::before {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(declarations, _)| declarations)
        .expect("theme.css must carry the footer hairline rule");
    assert!(
        footer_before.contains("background: var(--nav-public-hairline);"),
        "the footer hairline must consume the theme value: {footer_before}"
    );
}

#[test]
fn the_home_service_band_contract_keeps_its_clip_bleed_and_shared_left_edge() {
    let home = std::fs::read_to_string(public_dir().join("css/home.css"))
        .expect("read the firm home stylesheet");

    let shell = home
        .split_once(".public-shell:has(.home-service) {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(declarations, _)| declarations)
        .expect("home.css must scope the service-band clip to its shell");
    assert!(
        shell.contains("overflow-x: clip;"),
        "the shell must clip the band's viewport bleed: {shell}"
    );

    let band = home
        .split_once(".home-service {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(declarations, _)| declarations)
        .expect("home.css must carry the engagements band rule");
    assert!(
        band.contains("margin: clamp(1.5rem, 4vw, 2.5rem) 0 0;"),
        "the band must keep the shell's shared horizontal edge: {band}"
    );

    let bleed = home
        .split_once(".home-service::before {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(declarations, _)| declarations)
        .expect("home.css must carry the engagements bleed rule");
    for declaration in [
        "position: absolute;",
        "inset-block: 0;",
        "inset-inline: calc(50% - 50vw);",
    ] {
        assert!(
            bleed.contains(declaration),
            "the band bleed must retain {declaration:?}: {bleed}"
        );
    }

    let hairline = home
        .split_once(".home-service::after {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(declarations, _)| declarations)
        .expect("home.css must carry the engagements hairline rule");
    for declaration in [
        "inset: 0 auto auto 50%;",
        "width: min(72rem, calc(100% + 2rem));",
        "transform: translateX(-50%);",
    ] {
        assert!(
            hairline.contains(declaration),
            "the band hairline must retain {declaration:?}: {hairline}"
        );
    }
}

/// Motion may move normal-size text into place, but it may not fade it below
/// the contrast floor while the page first renders. The browser gate caught
/// the Fractional GC virtue cards mid-fade in the release KIND image; this
/// inexpensive stylesheet check keeps that transient failure out of ordinary
/// workspace runs too.
#[test]
fn transactional_text_entrances_keep_text_opaque() {
    let css = std::fs::read_to_string(public_dir().join("css/transactional.css"))
        .expect("read the transactional stylesheet");

    for (name, expected_transform) in [
        ("speed-heading-in", "transform: translateY(0.4rem);"),
        ("speed-virtue-in", "transform: translateY(1rem);"),
        ("speed-card-in", "transform: translateY(1rem);"),
    ] {
        let keyframe = css
            .split_once(&format!("@keyframes {name} {{"))
            .and_then(|(_, rest)| rest.split_once("\n  }\n}"))
            .map_or_else(
                || panic!("transactional.css must retain the {name} keyframe"),
                |(declarations, _)| declarations,
            );
        assert!(
            keyframe.contains(expected_transform),
            "{name} remains a motion, not a static replacement: {keyframe}"
        );
        assert!(
            !keyframe.contains("opacity:"),
            "normal-size text may not fade below WCAG contrast while {name} runs: {keyframe}"
        );
    }
}

/// An inline link inside prose carries a cue that is not colour (WCAG 1.4.1).
///
/// axe reports this as `link-in-text-block`, and the public accessibility gate
/// caught it on three surfaces at once — the legal pages, the transparency
/// documents, and the 403 — because the rule that used to carry it was an
/// allow-list of four page containers that every new prose page had to be
/// remembered into. The replacement keys off the class attribute instead: a
/// bare `<a href>` inside a paragraph or list item is prose, while a control
/// carries the class that styles it.
///
/// That gate needs a live KIND cluster and a browser; this reads the rule
/// itself, so the regression is caught in the ordinary workspace run — and so
/// the *shape* of the rule is pinned, since an allow-list would pass a browser
/// audit of today's pages while still being the thing that drifts.
#[test]
fn inline_prose_links_are_underlined_without_an_allow_list_of_pages() {
    let theme = std::fs::read_to_string(public_dir().join("css/theme.css"))
        .expect("read the shared theme stylesheet");

    let selector = ".nav-theme :is(p, li) > a:not([class])";
    let rule = theme
        .split_once(&format!("{selector} {{"))
        .and_then(|(_, rest)| rest.split_once('}'))
        .map_or_else(
            || panic!("theme.css must carry the inline-prose-link rule at `{selector}`"),
            |(declarations, _)| declarations,
        );
    assert!(
        rule.contains("text-decoration: underline;"),
        "an inline prose link needs a non-colour cue: {rule}"
    );
    // Not anchored to a `main` descendant. `.nav-theme` sits on an ancestor of
    // `main` on the public shell and *on* `main` on the back-office pages, so
    // `.nav-theme main …` matches only half the surface — which is how the
    // lawyer pages kept their un-cued links through a first pass at this fix.
    assert!(
        !theme.contains(".nav-theme main :is(p, li) > a"),
        "the rule may not require `main` to be a descendant of `.nav-theme`: \
         the back-office pages put both on the same element"
    );

    // The rule this replaced. Its return would restore the drift, and it would
    // do so silently: every page it names would still pass a browser audit.
    for retired in [
        ".nav-theme .service-open-letter a,",
        ".nav-theme .blog-post a,",
        ".nav-theme .docs-article a,",
    ] {
        assert!(
            !theme.contains(retired),
            "`{retired}` is back — the WCAG 1.4.1 cue is an allow-list of page \
             containers again, which is what left the legal, transparency, and \
             error pages without it"
        );
    }
}

/// The muted verification link carries its cue on the class, not on a page.
///
/// `link-secondary` is the fine-print anchor around a trademark registration
/// or a bar licence number, and it is always read mid-sentence: "… is a
/// registered trademark of Shook Law PLLC, U.S. Reg. No. 6,325,650". It had no
/// stylesheet rule at all, so `.nav-theme a { text-decoration: none }` left it
/// distinguishable by colour alone and axe failed `link-in-text-block` on
/// `/design` — in the `/app` footer and, undecidably, in the public one.
///
/// The cue rides the class because that is what the class means. Pinning it
/// here keeps the rule from being re-expressed as the list of footers that
/// happen to use it today.
#[test]
fn the_muted_verification_link_is_not_distinguished_by_colour_alone() {
    let theme = std::fs::read_to_string(public_dir().join("css/theme.css"))
        .expect("read the shared theme stylesheet");

    let selector = ".nav-theme a.link-secondary";
    let rule = theme
        .split_once(&format!("{selector} {{"))
        .and_then(|(_, rest)| rest.split_once('}'))
        .map_or_else(
            || panic!("theme.css must carry the non-colour link rule at `{selector}`"),
            |(declarations, _)| declarations,
        );
    assert!(
        rule.contains("text-decoration: underline;"),
        "{selector} is read inside a sentence, so colour cannot be its only \
         cue: {rule}"
    );
    assert!(
        rule.contains("text-underline-offset: 0.14em;"),
        "{selector} must use the shared in-copy underline offset: {rule}"
    );
}

/// A field's help line is prose, and its block margins are the theme's.
///
/// The help text is rendered as a `<p>` so the shared
/// `.nav-theme :is(p, li) > a:not([class])` rule underlines a link inside it —
/// the contact form's "We'll only use this to reply. Privacy Policy" is the
/// case axe caught. A `<p>` brings the browser's 1em block margins with it, so
/// the rule must state its own, or every helped field grows a gap.
#[test]
fn the_field_help_paragraph_states_its_own_block_margins() {
    let theme = std::fs::read_to_string(public_dir().join("css/theme.css"))
        .expect("read the shared theme stylesheet");

    let rule = theme
        .split_once(".nav-field__help {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map_or_else(
            || panic!("theme.css must carry the `.nav-field__help` rule"),
            |(declarations, _)| declarations,
        );
    assert!(
        rule.contains("margin: 0.25rem 0 0;"),
        "`.nav-field__help` is a `<p>`: it must zero the browser's default \
         block margins rather than set `margin-top` alone: {rule}"
    );
}

#[test]
fn gallery_footer_links_keep_a_non_colour_cue() {
    let theme = std::fs::read_to_string(public_dir().join("css/theme.css"))
        .expect("read the shared theme stylesheet");

    let selector = ".nav-theme .nav-card__footer > a";
    let rule = theme
        .split_once(&format!("{selector} {{"))
        .and_then(|(_, rest)| rest.split_once('}'))
        .map_or_else(
            || panic!("theme.css must carry the non-colour link rule at `{selector}`"),
            |(declarations, _)| declarations,
        );
    assert!(
        rule.contains("text-decoration: underline;"),
        "{selector} must not rely on link colour: {rule}"
    );
    assert!(
        rule.contains("text-underline-offset: 0.14em;"),
        "{selector} must use the shared in-copy underline offset: {rule}"
    );
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// A filled brand button must out-specify the theme's link rule.
///
/// `theme.css` sets `.nav-theme a { color: var(--nav-color-link) }` at
/// specificity (0,1,1). A filled button styled by a lone class — `(0,1,0)` —
/// loses to it and renders its label in the link colour instead of its own ink.
///
/// That was invisible while link and primary were different hues, and became
/// literal when the palette collapsed to one ramp: a call to action rendered
/// teal-on-teal, a 1:1 contrast ratio. The axe gate did not catch it, so this
/// guard is what keeps it fixed.
#[test]
fn the_filled_action_button_outranks_the_theme_link_rule() {
    let css = std::fs::read_to_string(public_dir().join("css/marketing-page.css"))
        .expect("read the marketing-page stylesheet");
    assert!(
        css.contains(".nav-theme a.fm-action__link"),
        "the filled action button must be qualified so it beats `.nav-theme a`"
    );
    // The bare form is what loses. It must not come back, in either the base
    // rule or the hover pair.
    for line in css.lines() {
        let selector = line.trim();
        assert!(
            !(selector.starts_with(".fm-action__link")),
            "an unqualified `.fm-action__link` selector loses to `.nav-theme a`: {selector}"
        );
    }
}

/// The declarations of one rule in a stylesheet, or a panic naming the selector.
fn rule_body<'css>(css: &'css str, selector: &str) -> &'css str {
    css.split_once(&format!("\n{selector} {{"))
        .and_then(|(_, rest)| rest.split_once('}'))
        .map_or_else(
            || panic!("the stylesheet must carry a rule at `{selector}`"),
            |(declarations, _)| declarations,
        )
}

/// The `--nav-color-*` token a declaration reads, e.g. `primary-hover` from
/// `color: var(--nav-color-primary-hover);`.
fn token_in(declarations: &str, property: &str) -> String {
    let value = declarations
        .split(';')
        .map(str::trim)
        .find_map(|declaration| declaration.strip_prefix(&format!("{property}:")))
        .unwrap_or_else(|| panic!("no `{property}` in `{declarations}`"));
    let token = value
        .split_once("var(--nav-color-")
        .and_then(|(_, rest)| rest.split_once(')'))
        .unwrap_or_else(|| panic!("`{property}` must read a `--nav-color-*` token: `{value}`"));
    token.0.to_owned()
}

/// The `color-mix(in srgb, var(--nav-color-primary) N%, …)` share, as a percent.
fn primary_mix_percent(declarations: &str, property: &str) -> u32 {
    let value = declarations
        .split(';')
        .map(str::trim)
        .find_map(|declaration| declaration.strip_prefix(&format!("{property}:")))
        .unwrap_or_else(|| panic!("no `{property}` in `{declarations}`"));
    let percent = value
        .split_once("var(--nav-color-primary)")
        .and_then(|(_, rest)| rest.trim_start().split_once('%'))
        .unwrap_or_else(|| panic!("`{property}` must mix `--nav-color-primary`: `{value}`"));
    percent
        .0
        .trim()
        .parse::<u32>()
        .unwrap_or_else(|error| panic!("unreadable mix share in `{value}`: {error}"))
}

/// `color-mix(in srgb, …)` interpolates the gamma-encoded channels, so a share
/// of `fg` over `bg` is a plain per-channel blend of the two hex triples. Kept
/// in integer arithmetic — the share is written as a whole percent in the
/// stylesheet, and `+ 50` before the divide is the round-half-up a browser
/// does, which is what reproduces the `#ebf5f6` axe measured.
fn srgb_mix(fg: [u8; 3], bg: [u8; 3], percent: u32) -> [u8; 3] {
    let channel = |index: usize| {
        let blended =
            (u32::from(fg[index]) * percent + u32::from(bg[index]) * (100 - percent) + 50) / 100;
        u8::try_from(blended).expect("a blend of two bytes is a byte")
    };
    [channel(0), channel(1), channel(2)]
}

/// The surface a scheme paints a card on: explicit `surface`, else the
/// `tokens.css` default for the scheme (white, or `#161b22` in dark).
fn scheme_surface(scheme: &views::brand_presentation::PaletteScheme, dark: bool) -> [u8; 3] {
    scheme
        .surface
        .and_then(views::brand_presentation::parse_hex)
        .unwrap_or(if dark {
            [0x16, 0x1b, 0x22]
        } else {
            [0xff, 0xff, 0xff]
        })
}

/// `/services` prints each package's badge on a primary-tinted panel, and the
/// tint is what decides whether the badge is readable.
///
/// A brand's 500 stop is sized to clear 4.5:1 against a *plain* surface, with
/// little headroom — `tokens.css` says so, and says a surface that needs more
/// should step to 600. `.fm-services__package` mixes 8% of that same primary
/// into the surface, which lightens the ground under the ink without moving
/// the ink: the firm's teal fell to 4.41:1 there, and axe failed the
/// `26.9.17-rc.1` deploy on it at 11px.
///
/// The browser gate that caught it runs only in `deploy.yml`, behind a KIND
/// cluster, so it reports after `main` already has the regression. This reads
/// the two rules and does the arithmetic instead — every catalogued palette,
/// both schemes — so the next one fails in the ordinary workspace run.
#[test]
fn the_services_package_badge_clears_wcag_aa_on_its_tinted_panel() {
    let css = std::fs::read_to_string(public_dir().join("css/marketing-page.css"))
        .expect("read the marketing-page stylesheet");

    let percent = primary_mix_percent(rule_body(&css, ".fm-services__package"), "background");
    let ink = token_in(rule_body(&css, ".fm-services__package-badge"), "color");

    for palette in views::brand::PALETTE {
        for (scheme, dark) in [(&palette.light, false), (&palette.dark, true)] {
            let hex = match ink.as_str() {
                "primary" => scheme.primary,
                "primary-hover" => scheme.primary_hover,
                "primary-active" => scheme.primary_active,
                other => panic!("the badge reads `--nav-color-{other}`, which this gate cannot resolve to a palette stop"),
            };
            let ink_rgb = views::brand_presentation::parse_hex(hex)
                .unwrap_or_else(|| panic!("{} {hex}", palette.id));
            let primary = views::brand_presentation::parse_hex(scheme.primary)
                .unwrap_or_else(|| panic!("{} primary", palette.id));
            let panel = srgb_mix(primary, scheme_surface(scheme, dark), percent);
            let ratio = views::brand_presentation::contrast_ratio(ink_rgb, panel);
            assert!(
                ratio >= 4.5,
                "{} [{}]: the badge's `--nav-color-{ink}` is {ratio:.2}:1 on the \
                 panel's {percent}% primary tint — under the 4.5:1 floor for 11px \
                 text. Step the ink to the next stop rather than lightening the tint.",
                palette.id,
                if dark { "dark" } else { "light" },
            );
        }
    }
}

/// Shared marketing headings must follow the resolved brand, including the
/// single-face debt site and Misericordia's separate display face.
#[test]
fn marketing_headings_use_the_brands_display_face() {
    let css = std::fs::read_to_string(public_dir().join("css/brand-firm.css")).unwrap();
    assert!(css.contains("--firm-serif: var(--brand-font-display, var(--nav-font-family));"));
    assert!(!css.contains("GORP"));
    for (key, body, display) in [
        (
            views::brand::BrandKey::DeleteYourDebt,
            "Public Sans",
            "Public Sans",
        ),
        (
            views::brand::BrandKey::Misericordia,
            "Source Sans 3",
            "Source Serif 4",
        ),
    ] {
        let tokens = views::brand_presentation::tokens_stylesheet(
            key.default_typeface(),
            key.display_typeface(),
            key.default_palette(),
        );
        assert!(
            tokens.contains(&format!("--nav-font-family: \"{body}\"")),
            "{key:?}"
        );
        assert!(
            tokens.contains(&format!("--brand-font-display: \"{display}\"")),
            "{key:?}"
        );
        assert!(!tokens.contains("GORP"), "{key:?}");
        assert!(
            !portal::dioxus_app::font_head(key).contains("GORP"),
            "{key:?}"
        );
    }
}
