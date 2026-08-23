//! The Dioxus design gallery renders server-side, readable before hydration.
//!
//! `webapp::design::DesignGallery` renders the Dioxus Components — the brand
//! tokens, the inline SVG icons, the cards, and the toasts — into the
//! server-rendered HTML, so a contributor sees the real components with no
//! client bundle required. This mirrors the `dioxus_people_ssr` harness: mount
//! the component through `render_handler` and assert on the SSR body.
//!
//! Dioxus fullstack SSR wraps text nodes in hydration comments
//! (`<h1><!--node-1-->Design system<!--/-->`), so these assertions match the
//! text token or the `>Text<` seam rather than `class="x">Text`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::Router;
use dioxus_server::{render_handler, FullstackState, ServeConfig};
use http_body_util::BodyExt;
use tower::ServiceExt;

/// A minimal, CDN-free bundle `index.html` with the `main` mount point.
const INDEX_HTML: &str = "<!DOCTYPE html>\n\
<html lang=\"en\"><head><meta charset=\"UTF-8\" />\
<title>Neon Law Navigator</title></head>\
<body><div id=\"main\"></div></body></html>\n";

/// Render `DesignGallery` at `/design`, returning the SSR HTML body. Owns the
/// process-global `DIOXUS_PUBLIC_PATH` (safe under nextest's process-per-test
/// isolation).
async fn render_design() -> (StatusCode, String) {
    render_design_at("/design").await
}

/// Render `DesignGallery` at `uri` (so the demo table's `?sort=` / `?page=`
/// query is exercised through the server function during SSR).
async fn render_design_at(uri: &str) -> (StatusCode, String) {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("index.html"), INDEX_HTML).expect("write index.html");
    std::env::set_var("DIOXUS_PUBLIC_PATH", dir.path());

    let router: Router = Router::<FullstackState>::new()
        .route("/design", get(render_handler))
        .with_state(FullstackState::new(
            ServeConfig::new(),
            webapp::design::DesignGallery,
        ));

    let response = router
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("router response");
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    std::env::remove_var("DIOXUS_PUBLIC_PATH");
    (status, String::from_utf8_lossy(&body).to_string())
}

#[tokio::test]
async fn design_gallery_renders_the_heading_and_theme_shell() {
    let (status, html) = render_design().await;
    assert_eq!(status, StatusCode::OK, "{html}");
    // The theme container and heading are in the server HTML before any JS.
    assert!(html.contains("nav-theme design-gallery"), "{html}");
    assert!(html.contains("Design system"), "{html}");
    // The theme stylesheet hoists into the head, same-origin.
    assert!(html.contains("/public/css/theme.css"), "{html}");
}

#[tokio::test]
async fn gallery_states_the_three_theme_contracts() {
    // The gallery is where a contributor reads the boundary before adding a
    // component, so the leaf rule, the injected-link seam, and the brand-token
    // rule are on the page — each naming the test that enforces it.
    let (status, html) = render_design().await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains("The three contracts"), "{html}");
    assert!(html.contains("components_import_no_app_crate"), "{html}");
    assert!(
        html.contains("components_declare_no_literal_colors"),
        "{html}"
    );
    assert!(html.contains("Injected links"), "{html}");
}

#[tokio::test]
async fn brand_swatches_resolve_through_nav_tokens_not_literal_colors() {
    // Contract 3: the palette previews the running deploy's brand because each
    // chip paints `var(--nav-…)`. A literal hex here would pin one of three
    // brands into the page.
    let (status, html) = render_design().await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains("design-swatch__chip"), "{html}");
    assert!(
        html.contains("var(--nav-color-primary)"),
        "swatch resolves its token: {html}"
    );
    assert!(
        html.contains(">--nav-color-danger<"),
        "the token name is the caption: {html}"
    );
}

#[tokio::test]
async fn gallery_explains_the_brand_specific_primary() {
    let (status, html) = render_design().await;
    assert_eq!(status, StatusCode::OK, "{html}");
    // The gallery reads anonymously now, so the copy no longer calls itself the
    // signed-in reference; what it still owes the reader is which primary the
    // brand wears.
    assert!(!html.contains("signed-in design reference"), "{html}");
    assert!(html.contains("Neon Law uses copper"), "{html}");
    assert!(
        !html.contains("Neon Law Foundation"),
        "the retired brand has no swatch to explain: {html}"
    );
}

#[tokio::test]
async fn gallery_renders_the_authenticated_navigator_chrome() {
    let (status, html) = render_design().await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains("navigator-shell"), "{html}");
    assert!(html.contains("navigator-navbar"), "{html}");
    assert!(html.contains("navigator-footer"), "{html}");
    assert!(html.contains(r#"aria-label="Navigator""#), "{html}");
    assert!(html.contains(r#"aria-current="page""#), "{html}");
    assert!(
        html.contains("Legal services rendered by Example Firm PLLC."),
        "{html}"
    );
    // The chrome preview must not introduce a second `<main>` landmark. The
    // gallery owns the page's single `<main>`; the embedded shell renders its
    // content region as a plain element instead of nesting another main.
    assert_eq!(html.matches("<main").count(), 1, "{html}");
}

#[tokio::test]
async fn design_gallery_renders_inline_svg_icons_not_a_font() {
    let (_status, html) = render_design().await;
    // Icons are inline `<svg>`, not `<i class="bi …">` webfont glyphs.
    assert!(html.contains("<svg"), "{html}");
    assert!(html.contains("nav-icon"), "{html}");
    assert!(!html.contains("class=\"bi bi-"), "{html}");
    // A meaningful icon carries its accessible name inside a `<title>`. Fullstack
    // SSR wraps text nodes in hydration comments (`<title …><!--id-->Litigation
    // <!--#--></title>`), so match the surviving `<title` seam and the name
    // token rather than a verbatim `<title>Litigation</title>` substring.
    assert!(html.contains("<title"), "{html}");
    assert!(html.contains("Litigation"), "{html}");
}

#[tokio::test]
async fn design_gallery_renders_dioxus_cards_and_toasts() {
    let (_status, html) = render_design().await;
    // The Dioxus components, styled by the theme — no Bootstrap classes.
    assert!(html.contains("nav-card"), "{html}");
    assert!(html.contains("nav-card--highlighted"), "{html}");
    assert!(html.contains("nav-toast--danger"), "{html}");
    assert!(html.contains("nav-toast--success"), "{html}");
    assert!(!html.contains("text-bg-danger"), "{html}");
}

#[tokio::test]
async fn design_gallery_renders_the_filterable_person_picker() {
    let (status, html) = render_design().await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains("Person foreign key"), "{html}");
    assert!(html.contains(r#"name="design_person_id_search""#), "{html}");
    assert!(html.contains("Filter people"), "{html}");
    assert!(html.contains(r#"name="design_person_id""#), "{html}");
    assert!(
        html.contains("Ada Lovelace &#60;ada@example.com&#62;"),
        "{html}"
    );
}

#[tokio::test]
async fn design_gallery_filters_person_picker_by_email() {
    let (status, html) =
        render_design_at("/design?design_person_id_search=linus%40example.com").await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains("1 person match"), "{html}");
    assert!(
        html.contains("Linus Torvalds &#60;linus@example.com&#62;"),
        "{html}"
    );
    assert!(
        !html.contains("Ada Lovelace &#60;ada@example.com&#62;"),
        "{html}"
    );
}

#[tokio::test]
async fn demo_table_ssrs_its_rows_with_sort_and_pagination_anchors() {
    // The demo table's server function reads the query and SSRs the first page
    // of the synthetic rows — in the HTML before any JS runs.
    let (status, html) = render_design_at("/design").await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains("nav-table"), "renders the data table: {html}");
    // Sortable headers are real anchors that toggle `?sort=` (ascending when
    // inactive).
    assert!(
        html.contains(r#"href="/design?sort=name""#),
        "sort anchor: {html}"
    );
    assert!(
        html.contains(r#"href="/design?sort=role""#),
        "sort anchor: {html}"
    );
    // Pagination renders as `?page=` anchors (8 synthetic rows, 4 per page → 2
    // pages), so page 2 is reachable.
    assert!(html.contains(r#"page=2""#), "pagination anchor: {html}");
    // Default (unsorted) order keeps the source order — Aquarius is on page 1.
    assert!(html.contains("Aquarius"), "{html}");
}

#[tokio::test]
async fn demo_table_survives_a_nonnumeric_page() {
    // A stray, non-numeric `?page=` must not blank the table: the lenient page
    // parse falls back to page 1 and the rows still SSR (rather than failing the
    // query extraction and rendering the error state).
    let (status, html) = render_design_at("/design?page=notanumber").await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains("nav-table"), "renders the data table: {html}");
    assert!(!html.contains("Failed to load the demo table"), "{html}");
    // Page 1 leads with the source order's first persona.
    assert!(html.contains("Aquarius"), "{html}");
}

#[tokio::test]
async fn demo_table_accepts_a_multi_field_sort() {
    // Both fields are advertised, so `?sort=role,name` is a 200 (not a 400) and
    // the table renders — every field is applied, not just the first.
    let (status, html) = render_design_at("/design?sort=role,name").await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains("nav-table"), "renders the data table: {html}");
    assert!(!html.contains("Failed to load the demo table"), "{html}");
}

#[tokio::test]
async fn demo_table_reorders_under_descending_sort() {
    // `?sort=-name` sorts the rows descending server-side, so page 1 now leads
    // with the last name alphabetically (Virgo) instead of Aquarius, and the
    // header anchor flips to ascending.
    let (status, html) = render_design_at("/design?sort=-name").await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(
        html.contains("Virgo"),
        "descending page 1 includes Virgo: {html}"
    );
    // Active-descending header now links back to ascending.
    assert!(html.contains(r#"href="/design?sort=name""#), "{html}");
    // The active-direction arrow renders.
    assert!(html.contains('\u{2193}'), "descending arrow: {html}");
}

#[tokio::test]
async fn gallery_renders_pricing_testimonial_and_disclaimer_sections() {
    // The marketing card cluster — pricing cards, testimonials, and the legal
    // disclaimer — render as Dioxus components, server-side and theme-styled
    // (no Bootstrap classes).
    let (status, html) = render_design_at("/design").await;
    assert_eq!(status, StatusCode::OK, "{html}");
    // Pricing cards: the brand band label, a solid CTA, and an inline-SVG check.
    assert!(
        html.contains("pricing-card"),
        "renders pricing cards: {html}"
    );
    assert!(html.contains("$3,500, once"), "band label: {html}");
    assert!(
        html.contains("nav-btn nav-btn--primary"),
        "solid CTA: {html}"
    );
    // An off-site CTA opens a new tab with the OWASP rel pair.
    assert!(html.contains(r#"rel="noopener noreferrer""#), "{html}");
    // Testimonials: a themed quote card with the generated-initials avatar.
    assert!(
        html.contains("testimonial-card"),
        "renders testimonials: {html}"
    );
    assert!(
        html.contains("testimonial-card__avatar--initials"),
        "initials avatar: {html}"
    );
    // The legal disclaimer partial, with its three load-bearing UPL points.
    assert!(
        html.contains("template-disclaimer"),
        "renders the disclaimer: {html}"
    );
    assert!(html.contains("not legal advice"), "{html}");
    assert!(html.contains("does not create an attorney"), "{html}");
    // No Bootstrap classes leaked in.
    assert!(!html.contains("text-bg-"), "{html}");
    assert!(!html.contains("btn btn-primary"), "{html}");
}

#[tokio::test]
async fn gallery_renders_navigation_links_as_plain_anchors() {
    // The injected-link contract, rendered: the breadcrumbs and the off-site
    // link are plain anchors in the pre-hydration HTML, so they work on a page
    // that ships no client bundle.
    let (status, html) = render_design_at("/design").await;
    assert_eq!(status, StatusCode::OK, "{html}");
    // Breadcrumb: an accessible landmark with the back arrow.
    assert!(
        html.contains("nav-breadcrumb"),
        "renders the breadcrumb: {html}"
    );
    assert!(html.contains(r#"aria-label="Breadcrumb""#), "{html}");
    // The lawyer variant is the second breadcrumb preview.
    assert!(html.contains(r#"href="/lawyer""#), "{html}");
    assert!(html.contains("Lawyer portal"), "{html}");
    // Off-site link: new tab + OWASP rel + the box-arrow-up-right glyph.
    assert!(html.contains(r#"target="_blank""#), "{html}");
    assert!(html.contains(r#"rel="noopener noreferrer""#), "{html}");
    // No last-edited stamp anywhere: the gallery renders components, and a
    // git-derived edit date is not one of them any more.
    assert!(!html.contains("nav-freshness"), "{html}");
    assert!(!html.contains("Last edited in main"), "{html}");
    // No Bootstrap icon webfont.
    assert!(!html.contains("class=\"bi bi-"), "{html}");
}

#[tokio::test]
async fn gallery_renders_the_form_card_with_a_plain_textarea() {
    // The create/edit form card renders as a native form (works pre-hydration),
    // theme-styled, with labeled fields — no Bootstrap, and no rich-text
    // editor: the long-form composer is a plain `<textarea>`.
    let (status, html) = render_design_at("/design").await;
    assert_eq!(status, StatusCode::OK, "{html}");
    // The form carries `admin-form` alongside `nav-form` as the stable e2e
    // selector hook (`accessibility_e2e.rs` waits for `form.admin-form`).
    assert!(
        html.contains(r#"class="nav-form admin-form""#),
        "renders the form: {html}"
    );
    // `/design` is GET-only, so the demo submits with GET (navigating back to
    // the gallery) rather than POST into a 405.
    assert!(
        html.contains(r#"method="get""#),
        "native form submit: {html}"
    );
    assert!(
        html.contains(r#"class="nav-label""#),
        "labeled fields: {html}"
    );
    assert!(
        html.contains(r#"class="nav-select""#),
        "renders a select: {html}"
    );
    assert!(html.contains("<textarea"), "renders a textarea: {html}");
    // The textarea value renders as element content — not a leaked hydration
    // comment. A `<textarea>` is RCDATA, so a stray `<!--node-…-->` marker would
    // show as literal text in the box; the demo seeds a value to prove it.
    assert!(
        html.contains("I&#39;d like to plan my estate.")
            || html.contains("I'd like to plan my estate."),
        "textarea value renders as content: {html}"
    );
    assert!(
        !html.contains("form-control"),
        "no Bootstrap form classes: {html}"
    );
    assert!(
        !html.contains("tiptap"),
        "the theme ships no rich-text editor: {html}"
    );
}

#[tokio::test]
async fn gallery_renders_people_list_and_social_meta() {
    // The people-list widget renders its fieldsets (pre-filled from a prior
    // answer), and the SocialMeta component hoists its og/twitter tags into
    // <head>.
    let (status, html) = render_design_at("/design").await;
    assert_eq!(status, StatusCode::OK, "{html}");
    // People-list: fieldsets with `p{row}_{part}` inputs, pre-filled.
    assert!(
        html.contains("nav-fieldset"),
        "renders the people list: {html}"
    );
    assert!(html.contains(r#"name="p0_name""#), "named inputs: {html}");
    assert!(
        html.contains(r#"value="Aries Ram""#),
        "prefills prior answer: {html}"
    );
    // SocialMeta: og + twitter tags in the head.
    assert!(html.contains(r#"property="og:title""#), "og:title: {html}");
    assert!(
        html.contains(r#"name="twitter:card""#),
        "twitter:card: {html}"
    );
    // The share tags carry the deploy's brand identity (default: Neon Law),
    // resolved from branding rather than a hard-coded neonlaw.com asset URL, so
    // a white-label install emits its own site name and logo.
    assert!(
        html.contains(r#"property="og:site_name""#),
        "og:site_name present: {html}"
    );
    assert!(
        !html.contains("www.neonlaw.com/img/logo.png"),
        "social image must come from branding, not a hard-coded URL: {html}"
    );
    assert!(
        html.contains("/public/logo-neon.png"),
        "og:image resolves from the firm brand's raster mark: {html}"
    );
    assert!(
        !html.contains("class=\"form-control"),
        "no Bootstrap: {html}"
    );
}

#[tokio::test]
async fn gallery_syntax_highlights_snippets_server_side() {
    // The component-source snippets are syntect-highlighted server-side (the
    // CodeBlock server function resolves during SSR into inline-styled token
    // spans).
    let (status, html) = render_design_at("/design").await;
    assert_eq!(status, StatusCode::OK, "{html}");
    // syntect emits inline-styled spans; a coloured token is in the pre-hydration
    // HTML with no client highlighter.
    assert!(html.contains("nav-code"), "renders a code block: {html}");
    assert!(
        html.contains("style=\"color:"),
        "syntect-highlighted tokens: {html}"
    );
    assert!(
        !html.contains("highlight.min.js"),
        "no vendored client highlighter: {html}"
    );
}

/// Every anchor in the gallery has somewhere to go.
///
/// The gallery renders each component with demo props, so a component whose
/// optional `href` defaults to empty emits `<a href="">` with no text — a WCAG
/// A `link-name` violation that failed the KIND `browser + accessibility e2e`
/// lane on `/design` rather than on any page that ships. Asserting it here
/// catches the next one in the workspace suite, before a cluster is involved.
#[tokio::test]
async fn no_gallery_component_renders_a_link_with_no_destination() {
    let (status, html) = render_design().await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(
        !html.contains(r#"href="""#),
        "a component rendered an unlabelled link with no destination: {html}"
    );
}

/// The gallery's two site headers hold two separate disclosure states.
///
/// `/design` is the only page that renders `SiteHeader` twice — once bare, once
/// inside the public-shell showcase. Both used to emit `id="site-menu"`, which
/// axe reports as `duplicate-id-aria` and which pointed the second burger label
/// at the first header's checkbox.
#[tokio::test]
async fn the_two_headers_in_the_gallery_carry_distinct_menu_ids() {
    let (status, html) = render_design().await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert_eq!(
        html.matches(r#"id="site-menu""#).count(),
        1,
        "the default menu id appears once: {html}"
    );
    assert_eq!(
        html.matches(r#"id="design-shell-menu""#).count(),
        1,
        "the shell showcase's header holds its own state: {html}"
    );
}
