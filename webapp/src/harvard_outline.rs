//! Harvard-outline narration assets shared by [`crate::notation_outline`]
//! (`/app/projects/{code}/{notation_id}/outline`) and
//! [`crate::notation_preview`]. Keyboard and click handling live in
//! `harvard-outline-narrate.js`, so the stage works without the wasm
//! hydration bundle.
//!
//! The bundled-catalog recording stage that once lived at `/app/outline` was
//! retired: it read no store and served no matter, so its narration surface
//! now lives only bound to a real notation.

/// Stylesheet for the stage highlight fills.
pub const HARVARD_OUTLINE_STYLESHEET_HREF: &str = "/public/css/harvard-outline.css";

/// First-party script that steps the current unit.
pub const HARVARD_OUTLINE_SCRIPT_HREF: &str = "/public/js/harvard-outline-narrate.js";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stage_assets_match_the_public_files() {
        assert_eq!(
            HARVARD_OUTLINE_SCRIPT_HREF,
            "/public/js/harvard-outline-narrate.js"
        );
        assert_eq!(
            HARVARD_OUTLINE_STYLESHEET_HREF,
            "/public/css/harvard-outline.css"
        );
        let js = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../server/public/js/harvard-outline-narrate.js"
        ));
        assert!(js.contains("data-harvard-outline"), "{js}");
        let css = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../server/public/css/harvard-outline.css"
        ));
        assert!(css.contains(".harvard-unit.is-current"), "{css}");
        assert!(css.contains(".harvard-doc-switcher"), "{css}");
    }
}
