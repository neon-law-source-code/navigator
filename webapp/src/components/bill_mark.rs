//! A real photograph of a currency note that makes a flat day-rate visually
//! concrete.
//!
//! Public-domain U.S. currency scans from Wikimedia Commons's "PD US money"
//! collection — a work of the U.S. government is ineligible for copyright —
//! not a generated or reproduced likeness. Presentational only: the
//! deploy-specific asset URL is resolved server-side
//! (`neon::locales`, mirroring `webapp::home::HeroPicture`'s pattern) and
//! carried here as a plain string. `views::assets::asset_url` never appears
//! inside this crate's shared component tree because `webapp` also compiles
//! for the browser (`web` feature), where the server-only `views` crate is
//! not available.

use dioxus::prelude::*;

/// Draw the photograph at `src`. Empty `alt`: every mount sits beside a
/// caption that already states the amount in words, so the photo is
/// decorative rather than informative on its own.
#[component]
pub(crate) fn BillMarkGlyph(src: String, #[props(default)] class: String) -> Element {
    rsx! {
        img {
            class: "{class}",
            src: "{src}",
            alt: "",
            loading: "lazy",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BillMarkGlyph;
    use dioxus::prelude::*;

    #[test]
    fn it_renders_the_given_photo_with_no_alt_text() {
        let html = dioxus_ssr::render_element(rsx! {
            BillMarkGlyph {
                src: "/public/img/ten-dollar-bill/ten-dollar-bill.jpg".to_string(),
                class: "day-rate-mark".to_string(),
            }
        });
        assert!(html.contains("ten-dollar-bill.jpg"), "{html}");
        assert!(html.contains("day-rate-mark"), "{html}");
        assert!(html.contains(r#"alt="""#), "{html}");
    }
}
