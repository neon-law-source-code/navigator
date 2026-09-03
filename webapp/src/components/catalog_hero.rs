//! The catalog hero — the animated deep-space banner the firm's sharing
//! surface leads with (#956 Phase 4).
//!
//! One component for the scene and the header markup its callers would
//! otherwise repeat. Every layer is decorative and
//! `aria-hidden`; the CSS lives in [`CATALOG_STYLESHEET_HREF`] and freezes
//! wholesale under `prefers-reduced-motion`.

use dioxus::prelude::*;

/// The stylesheet that draws the hero scene and material cards.
/// Hoisted by every page that renders a [`CatalogHero`]; without it the scene is
/// markup with no styling at all. The package version keeps a browser from
/// reusing an older presentation layout after a deployment.
pub const CATALOG_STYLESHEET_HREF: &str = concat!(
    "/public/css/catalog.css?v=",
    env!("CARGO_PKG_VERSION"),
    "-7"
);

/// The animated scene: two drifting starfields, a cloud of color, and a
/// star being born (a pulsing core inside an expanding shockwave).
#[component]
fn CatalogHeroScene() -> Element {
    rsx! {
        div { class: "catalog-hero-media", "aria-hidden": "true",
            div { class: "catalog-hero__stars" }
            div { class: "catalog-hero__stars catalog-hero__stars--far" }
            div { class: "catalog-hero__cloud" }
            div { class: "catalog-hero__burst" }
            div { class: "catalog-hero__core" }
        }
    }
}

/// The hero banner: the animated scene behind an eyebrow, the page `<h1>`, and
/// a lede. The heading is the page's single `<h1>`, so a page renders exactly
/// one of these.
#[component]
pub fn CatalogHero(eyebrow: String, title: String, lede: String) -> Element {
    rsx! {
        header { class: "catalog-hero",
            CatalogHeroScene {}
            div { class: "catalog-hero-copy",
                p { class: "catalog-hero__eyebrow", "{eyebrow}" }
                h1 { "{title}" }
                p { class: "lede", "{lede}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssr(app: fn() -> Element) -> String {
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    fn html() -> String {
        fn app() -> Element {
            rsx! {
                CatalogHero {
                    eyebrow: "Neon Law".to_string(),
                    title: "Presentations".to_string(),
                    lede: "Talks and teaching material.".to_string(),
                }
            }
        }
        ssr(app)
    }

    #[test]
    fn renders_every_layer_of_the_animated_scene() {
        // Each layer is a separate element the stylesheet animates; a missing
        // one is a scene that renders but does not move, which no class-name
        // assertion elsewhere would catch.
        let out = html();
        for layer in [
            "catalog-hero__stars",
            "catalog-hero__stars catalog-hero__stars--far",
            "catalog-hero__cloud",
            "catalog-hero__burst",
            "catalog-hero__core",
        ] {
            assert!(out.contains(layer), "scene should render {layer}: {out}");
        }
    }

    #[test]
    fn the_scene_is_hidden_from_the_accessibility_tree() {
        // The whole banner is decoration; a screen reader should hear the
        // copy, never the starfield.
        let out = html();
        assert!(
            out.contains(r#"class="catalog-hero-media" aria-hidden="true""#),
            "the scene container is aria-hidden: {out}"
        );
    }

    #[test]
    fn the_copy_is_the_eyebrow_the_h1_and_the_lede() {
        let out = html();
        assert!(out.contains("Neon Law"), "eyebrow: {out}");
        assert!(out.contains(">Presentations<"), "title text: {out}");
        assert!(out.contains("Talks and teaching material."), "lede: {out}");
        assert_eq!(out.matches("<h1").count(), 1, "exactly one h1: {out}");
    }
}
