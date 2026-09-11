//! Closed typeface and palette catalogs a brand row may wear.
//!
//! The store holds ids from these lists, never free CSS. [`tokens_stylesheet`]
//! is the only renderer for `/public/css/brand-{key}-tokens.css`.

use crate::assets::font_face_css;
use crate::brand::BrandKey;

/// One licensed or system face a brand row may name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Typeface {
    pub id: &'static str,
    pub label: &'static str,
    /// CSS `font-family` stack, including fallbacks.
    pub stack: &'static str,
    pub operator_licence_required: bool,
}

/// Light or dark token values for one [`Palette`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaletteScheme {
    pub primary: &'static str,
    pub primary_hover: &'static str,
    pub primary_active: &'static str,
    pub on_primary: &'static str,
    pub on_brand: &'static str,
    pub link: &'static str,
    pub link_hover: &'static str,
    pub surface_subtle: &'static str,
    pub bg: Option<&'static str>,
    pub surface: Option<&'static str>,
    pub surface_raised: Option<&'static str>,
    pub text: Option<&'static str>,
    pub text_muted: Option<&'static str>,
    pub border: Option<&'static str>,
}

/// One named colour scheme a brand row may name. Ids are the `primary_color`
/// values the store keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub id: &'static str,
    pub label: &'static str,
    pub light: PaletteScheme,
    pub dark: PaletteScheme,
}

/// GORP Serif, Tinos, one system serif, and one system sans. Plus Jakarta Sans
/// is not on this list: a brand that used it seeds `system-sans` instead.
pub const TYPEFACES: &[Typeface] = &[
    Typeface {
        id: "gorp-serif",
        label: "GORP Serif",
        stack: "\"GORP Serif\", Georgia, serif",
        operator_licence_required: true,
    },
    Typeface {
        id: "tinos",
        label: "Tinos",
        stack: "\"Tinos\", \"Times New Roman\", serif",
        operator_licence_required: false,
    },
    Typeface {
        id: "system-serif",
        label: "System serif",
        stack: "Georgia, \"Times New Roman\", serif",
        operator_licence_required: false,
    },
    Typeface {
        id: "system-sans",
        label: "System sans",
        stack: "ui-sans-serif, system-ui, sans-serif",
        operator_licence_required: false,
    },
];

/// Primaries and accents taken from the three compiled house-brand token
/// sheets, named so a select can refuse free text.
pub const PALETTE: &[Palette] = &[
    Palette {
        id: "neon-teal",
        label: "Neon teal",
        light: PaletteScheme {
            primary: "#007c91",
            primary_hover: "#006b7d",
            primary_active: "#005a69",
            on_primary: "#ffffff",
            on_brand: "#ffffff",
            link: "#007c91",
            link_hover: "#005a69",
            surface_subtle: "#e3f7fa",
            bg: None,
            surface: None,
            surface_raised: None,
            text: None,
            text_muted: None,
            border: None,
        },
        dark: PaletteScheme {
            primary: "#4dd4e6",
            primary_hover: "#86e4ee",
            primary_active: "#b1f0f5",
            on_primary: "#002c34",
            on_brand: "#002c34",
            link: "#4dd4e6",
            link_hover: "#86e4ee",
            surface_subtle: "#082c33",
            bg: Some("#0d1117"),
            surface: Some("#161b22"),
            surface_raised: Some("#1c2128"),
            text: None,
            text_muted: None,
            border: None,
        },
    },
    Palette {
        id: "delete-your-data",
        label: "DeleteYourData red",
        light: PaletteScheme {
            primary: "#b91c1c",
            primary_hover: "#991b1b",
            primary_active: "#7f1d1d",
            on_primary: "#ffffff",
            on_brand: "#ffffff",
            link: "#b91c1c",
            link_hover: "#7f1d1d",
            surface_subtle: "#fdecec",
            bg: None,
            surface: None,
            surface_raised: None,
            text: None,
            text_muted: None,
            border: None,
        },
        dark: PaletteScheme {
            primary: "#ffa3a3",
            primary_hover: "#ffb8b8",
            primary_active: "#ffd0d0",
            on_primary: "#2c0606",
            on_brand: "#2c0606",
            link: "#ffa3a3",
            link_hover: "#ffb8b8",
            surface_subtle: "#2a0f0f",
            bg: None,
            surface: None,
            surface_raised: None,
            text: None,
            text_muted: None,
            border: None,
        },
    },
    Palette {
        id: "lawyer-shook",
        label: "Legal pad",
        light: PaletteScheme {
            primary: "#5c5100",
            primary_hover: "#443c00",
            primary_active: "#2e2900",
            on_primary: "#fff9a8",
            on_brand: "#272300",
            link: "#443c00",
            link_hover: "#2e2900",
            surface_subtle: "#fffbd1",
            bg: Some("#fff9a8"),
            surface: Some("#fff9a8"),
            surface_raised: Some("#fffbd1"),
            text: Some("#272300"),
            text_muted: Some("#5b5528"),
            border: Some("#c9bd55"),
        },
        dark: PaletteScheme {
            primary: "#fff176",
            primary_hover: "#fff7a8",
            primary_active: "#fffbd1",
            on_primary: "#272300",
            on_brand: "#272300",
            link: "#fff176",
            link_hover: "#fff7a8",
            surface_subtle: "#403900",
            bg: Some("#272300"),
            surface: Some("#332e00"),
            surface_raised: Some("#403900"),
            text: Some("#fffbd1"),
            text_muted: Some("#e5dc8a"),
            border: Some("#8f842d"),
        },
    },
];

/// Look up a typeface by the id stored on a brand row.
#[must_use]
pub fn typeface_by_id(id: &str) -> Option<&'static Typeface> {
    TYPEFACES.iter().find(|face| face.id == id)
}

/// Look up a palette by the id stored in `brand.primary_color`.
#[must_use]
pub fn palette_by_id(id: &str) -> Option<&'static Palette> {
    PALETTE.iter().find(|palette| palette.id == id)
}

impl BrandKey {
    /// The typeface this compiled key seeds and falls back to.
    #[must_use]
    pub fn default_typeface(self) -> &'static Typeface {
        match self {
            Self::Neon => typeface_by_id("gorp-serif").expect("gorp-serif is catalogued"),
            Self::DeleteYourData => {
                typeface_by_id("system-sans").expect("system-sans is catalogued")
            }
            Self::LawyerShook => typeface_by_id("tinos").expect("tinos is catalogued"),
        }
    }

    /// The palette this compiled key seeds and falls back to.
    #[must_use]
    pub fn default_palette(self) -> &'static Palette {
        match self {
            Self::Neon => palette_by_id("neon-teal").expect("neon-teal is catalogued"),
            Self::DeleteYourData => {
                palette_by_id("delete-your-data").expect("delete-your-data is catalogued")
            }
            Self::LawyerShook => palette_by_id("lawyer-shook").expect("lawyer-shook is catalogued"),
        }
    }
}

/// Resolve a row's presentation, falling back to a compiled key when the
/// stored ids are missing or unknown (legacy hex, empty seed).
#[must_use]
pub fn resolve_presentation(
    typeface_id: Option<&str>,
    palette_id: Option<&str>,
    fallback: Option<BrandKey>,
) -> Option<(&'static Typeface, &'static Palette)> {
    let fallback_face = fallback.map(BrandKey::default_typeface);
    let fallback_palette = fallback.map(BrandKey::default_palette);
    let face = typeface_id.and_then(typeface_by_id).or(fallback_face)?;
    let palette = palette_id.and_then(palette_by_id).or(fallback_palette)?;
    Some((face, palette))
}

/// Resolve the CSS `font-family` stack for a brand row's `typeface`
/// (ENG-586): `"uploaded"` reads `uploaded_family` (the row's own
/// `font_family`); any other value is a catalog id, falling back to a
/// compiled key's own default when the row names none. `None` only when
/// neither a catalog id nor an uploaded family can be resolved — a row that
/// predates every typeface value.
#[must_use]
pub fn font_stack_for(
    typeface_id: Option<&str>,
    uploaded_family: Option<&str>,
    fallback: Option<crate::brand::BrandKey>,
) -> Option<String> {
    if typeface_id == Some("uploaded") {
        return uploaded_family.map(|family| format!("'{family}', sans-serif"));
    }
    typeface_id
        .and_then(typeface_by_id)
        .map(|face| face.stack.to_string())
        .or_else(|| fallback.map(|key| key.default_typeface().stack.to_string()))
}

/// Resolve the `@font-face` CSS for whichever typeface a brand row names —
/// a compiled catalog face, or an uploaded one. `uploaded` is
/// `(family, object_url)`, present only when the row's `typeface` is
/// `"uploaded"` and it has a font object to point at. `None` for a system
/// stack with no face of its own, or an uploaded typeface with no object yet.
#[must_use]
pub fn font_face_for(typeface_id: Option<&str>, uploaded: Option<(&str, &str)>) -> Option<String> {
    if typeface_id == Some("uploaded") {
        let (family, url) = uploaded?;
        return Some(crate::assets::font_face_css(family, url, url));
    }
    typeface_id.and_then(typeface_by_id).and_then(webfont_css)
}

/// Render the tokens stylesheet a request for `brand-{key}-tokens.css` serves.
#[must_use]
pub fn tokens_stylesheet(face: &Typeface, palette: &Palette) -> String {
    let mut css = String::new();
    if let Some(faces) = webfont_css(face) {
        css.push_str(&faces);
        css.push('\n');
    }
    css.push_str(":root {\n");
    emit_scheme(&mut css, face, &palette.light);
    css.push_str("}\n\n@media (prefers-color-scheme: dark) {\n  :root {\n");
    emit_scheme(&mut css, face, &palette.dark);
    css.push_str("  }\n}\n");
    css
}

fn webfont_css(face: &Typeface) -> Option<String> {
    match face.id {
        "gorp-serif" => Some(font_face_css(
            "GORP Serif",
            &crate::assets::asset_url("fonts/gorp-serif/GORPSerif-Regular.woff2"),
            &crate::assets::asset_url("fonts/gorp-serif/GORPSerif-Bold.woff2"),
        )),
        "tinos" => Some(font_face_css(
            "Tinos",
            "/public/fonts/tinos/Tinos-Regular.woff2",
            "/public/fonts/tinos/Tinos-Bold.woff2",
        )),
        _ => None,
    }
}

fn emit_scheme(css: &mut String, face: &Typeface, scheme: &PaletteScheme) {
    css.push_str("  --nav-font-family: ");
    css.push_str(face.stack);
    css.push_str(";\n");
    css.push_str("  --font-body: var(--nav-font-family);\n");
    push_token(css, "--nav-color-primary", scheme.primary);
    push_token(css, "--nav-color-primary-hover", scheme.primary_hover);
    push_token(css, "--nav-color-primary-active", scheme.primary_active);
    push_token(css, "--nav-color-on-primary", scheme.on_primary);
    push_token(css, "--nav-color-on-brand", scheme.on_brand);
    push_token(css, "--nav-color-link", scheme.link);
    push_token(css, "--nav-color-link-hover", scheme.link_hover);
    push_token(css, "--nav-color-surface-subtle", scheme.surface_subtle);
    if let Some(value) = scheme.bg {
        push_token(css, "--nav-color-bg", value);
    }
    if let Some(value) = scheme.surface {
        push_token(css, "--nav-color-surface", value);
    }
    if let Some(value) = scheme.surface_raised {
        push_token(css, "--nav-color-surface-raised", value);
    }
    if let Some(value) = scheme.text {
        push_token(css, "--nav-color-text", value);
    }
    if let Some(value) = scheme.text_muted {
        push_token(css, "--nav-color-text-muted", value);
    }
    if let Some(value) = scheme.border {
        push_token(css, "--nav-color-border", value);
    }
}

fn push_token(css: &mut String, name: &str, value: &str) {
    css.push_str("  ");
    css.push_str(name);
    css.push_str(": ");
    css.push_str(value);
    css.push_str(";\n");
}

/// A derived colour scheme for a free-hex brand primary (ENG-586): owned
/// strings, since the hex is resolved from a `brand` row at request time
/// rather than compiled like [`PaletteScheme`]. Applies to both the light
/// `:root` block and the dark-mode block a runtime brand's tokens sheet
/// emits — there is no separate compiled dark variant for a free hex, unlike
/// the three catalog [`PALETTE`] entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedScheme {
    pub primary: String,
    pub primary_hover: String,
    pub primary_active: String,
    pub on_primary: String,
    pub surface_subtle: String,
}

fn to_hex(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

/// Multiply each channel toward black by `factor` (0.0 keeps the colour,
/// 1.0 reaches black) — a simple sRGB-space darken, good enough for a
/// derived hover/active shade.
///
/// The cast is exact: `channel` is a `u8` and `factor` is called only with
/// values in `0.0..=1.0`, so `channel * (1.0 - factor)` stays within
/// `0.0..=255.0` before rounding — the explicit `clamp` is defensive, not
/// load-bearing, and is what lets the truncating cast stay honest.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn darken(rgb: [u8; 3], factor: f64) -> [u8; 3] {
    rgb.map(|channel| {
        (f64::from(channel) * (1.0 - factor)).round().clamp(0.0, 255.0) as u8
    })
}

/// Mix `rgb` toward white by `factor` (0.0 keeps the colour, 1.0 reaches
/// white) — the light background tint behind a primary-coloured accent. See
/// [`darken`] for why the cast is safe.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn lighten(rgb: [u8; 3], factor: f64) -> [u8; 3] {
    rgb.map(|channel| {
        (f64::from(channel) + (255.0 - f64::from(channel)) * factor)
            .round()
            .clamp(0.0, 255.0) as u8
    })
}

/// Derive a full [`DerivedScheme`] from a validated primary hex. Returns
/// `None` when `hex` is malformed or its best on-primary contrast (white or
/// black) falls short of WCAG AA 4.5:1 — the same gate
/// `store::brands::create`/`update` already enforce before this is ever
/// called, so this is a defensive re-check, not the primary boundary.
#[must_use]
pub fn derive_scheme(hex: &str) -> Option<DerivedScheme> {
    let rgb = parse_hex(hex)?;
    let white = [0xff, 0xff, 0xff];
    let black = [0x00, 0x00, 0x00];
    let on_primary = if contrast_ratio(rgb, white) >= contrast_ratio(rgb, black) {
        white
    } else {
        black
    };
    if contrast_ratio(rgb, on_primary) < 4.5 {
        return None;
    }
    Some(DerivedScheme {
        primary: to_hex(rgb),
        primary_hover: to_hex(darken(rgb, 0.15)),
        primary_active: to_hex(darken(rgb, 0.30)),
        on_primary: to_hex(on_primary),
        surface_subtle: to_hex(lighten(rgb, 0.90)),
    })
}

/// Render the tokens stylesheet for a brand wearing a free hex primary
/// (ENG-586) instead of a catalog [`Palette`] — the runtime-brand twin of
/// [`tokens_stylesheet`]. `font_face` is the `@font-face` block for an
/// uploaded font (`typeface = "uploaded"`), or `None` to emit no `@font-face`
/// (a catalog typeface with its own compiled face, or a system stack with
/// none).
#[must_use]
pub fn tokens_stylesheet_from_hex(
    font_stack: &str,
    font_face: Option<&str>,
    scheme: &DerivedScheme,
) -> String {
    let mut css = String::new();
    if let Some(face) = font_face {
        css.push_str(face);
        css.push('\n');
    }
    let emit = |css: &mut String| {
        css.push_str("  --nav-font-family: ");
        css.push_str(font_stack);
        css.push_str(";\n");
        css.push_str("  --font-body: var(--nav-font-family);\n");
        push_token(css, "--nav-color-primary", &scheme.primary);
        push_token(css, "--nav-color-primary-hover", &scheme.primary_hover);
        push_token(css, "--nav-color-primary-active", &scheme.primary_active);
        push_token(css, "--nav-color-on-primary", &scheme.on_primary);
        push_token(css, "--nav-color-on-brand", &scheme.on_primary);
        push_token(css, "--nav-color-link", &scheme.primary);
        push_token(css, "--nav-color-link-hover", &scheme.primary_hover);
        push_token(css, "--nav-color-surface-subtle", &scheme.surface_subtle);
    };
    css.push_str(":root {\n");
    emit(&mut css);
    css.push_str("}\n\n@media (prefers-color-scheme: dark) {\n  :root {\n");
    emit(&mut css);
    css.push_str("  }\n}\n");
    css
}

/// Parse `#rrggbb` into sRGB bytes.
#[must_use]
pub fn parse_hex(hex: &str) -> Option<[u8; 3]> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let byte = |offset: usize| u8::from_str_radix(&hex[offset..offset + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

fn channel_luminance(value: u8) -> f64 {
    let c = f64::from(value) / 255.0;
    if c <= 0.039_28 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn relative_luminance([r, g, b]: [u8; 3]) -> f64 {
    0.2126 * channel_luminance(r) + 0.7152 * channel_luminance(g) + 0.0722 * channel_luminance(b)
}

/// WCAG contrast ratio, always >= 1.0.
#[must_use]
pub fn contrast_ratio(a: [u8; 3], b: [u8; 3]) -> f64 {
    let (l1, l2) = (relative_luminance(a), relative_luminance(b));
    let (lighter, darker) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
    (lighter + 0.05) / (darker + 0.05)
}

/// Surface a scheme uses for body text: explicit `bg`, else white / dark navy.
#[must_use]
pub fn scheme_bg(scheme: &PaletteScheme, dark: bool) -> [u8; 3] {
    scheme.bg.and_then(parse_hex).unwrap_or(if dark {
        [0x0d, 0x11, 0x17]
    } else {
        [0xff, 0xff, 0xff]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_typeface_list_is_closed_and_excludes_jakarta() {
        let ids: Vec<_> = TYPEFACES.iter().map(|face| face.id).collect();
        assert_eq!(ids, ["gorp-serif", "tinos", "system-serif", "system-sans"]);
        assert!(TYPEFACES
            .iter()
            .all(|face| !face.stack.to_lowercase().contains("jakarta")));
        assert!(
            typeface_by_id("gorp-serif")
                .unwrap()
                .operator_licence_required
        );
        assert!(!typeface_by_id("tinos").unwrap().operator_licence_required);
    }

    #[test]
    fn free_text_ids_are_refused() {
        assert!(typeface_by_id("\"Comic Sans\", cursive").is_none());
        assert!(palette_by_id("#ff00aa").is_none());
        assert!(palette_by_id("neon-teal").is_some());
    }

    #[test]
    fn every_palette_clears_wcag_aa() {
        for palette in PALETTE {
            for (scheme, dark) in [(&palette.light, false), (&palette.dark, true)] {
                let bg = scheme_bg(scheme, dark);
                let primary = parse_hex(scheme.primary).expect(palette.id);
                let on_primary = parse_hex(scheme.on_primary).expect(palette.id);
                let text = contrast_ratio(primary, bg);
                assert!(
                    text >= 4.5,
                    "{} primary {} on bg {:02x?} is {text:.2}:1",
                    palette.id,
                    scheme.primary,
                    bg
                );
                let on_fill = contrast_ratio(on_primary, primary);
                assert!(
                    on_fill >= 4.5,
                    "{} on-primary {} on {} is {on_fill:.2}:1",
                    palette.id,
                    scheme.on_primary,
                    scheme.primary
                );
                assert!(
                    text >= 3.0,
                    "{} primary vs surface must also clear UI 3:1",
                    palette.id
                );
            }
        }
    }

    /// ENG-586: `store::seed` now migrates each compiled brand's row with its
    /// real primary hex (not a palette id) in `primary_color`. This proves
    /// the three house brands still resolve their compiled `Palette`
    /// unchanged regardless — the fallback this function already carried for
    /// "legacy hex" covers it, so the seed change is not a rendering change.
    #[test]
    fn a_compiled_brand_s_own_hex_still_resolves_the_catalog_palette() {
        let with_hex =
            resolve_presentation(Some("gorp-serif"), Some("#007c91"), Some(BrandKey::Neon));
        let with_none = resolve_presentation(None, None, Some(BrandKey::Neon));
        assert_eq!(with_hex, with_none);
        let (face, palette) = with_hex.unwrap();
        assert_eq!(face.id, "gorp-serif");
        assert_eq!(palette.id, "neon-teal");
    }

    #[test]
    fn compiled_keys_seed_the_three_house_presentations() {
        assert_eq!(BrandKey::Neon.default_typeface().id, "gorp-serif");
        assert_eq!(BrandKey::Neon.default_palette().id, "neon-teal");
        assert_eq!(
            BrandKey::DeleteYourData.default_typeface().id,
            "system-sans"
        );
        assert_eq!(
            BrandKey::DeleteYourData.default_palette().id,
            "delete-your-data"
        );
        assert_eq!(BrandKey::LawyerShook.default_typeface().id, "tinos");
        assert_eq!(BrandKey::LawyerShook.default_palette().id, "lawyer-shook");
    }

    #[test]
    fn tokens_stylesheet_names_the_face_and_primary() {
        let css = tokens_stylesheet(
            typeface_by_id("tinos").unwrap(),
            palette_by_id("lawyer-shook").unwrap(),
        );
        assert!(css.contains("font-family:'Tinos'"), "{css}");
        assert!(css.contains("--nav-color-primary: #5c5100"), "{css}");
        assert!(css.contains("--nav-color-bg: #fff9a8"), "{css}");
        assert!(
            css.contains("/public/fonts/tinos/Tinos-Regular.woff2"),
            "{css}"
        );
    }

    #[test]
    fn system_sans_emits_no_font_face() {
        let css = tokens_stylesheet(
            typeface_by_id("system-sans").unwrap(),
            palette_by_id("delete-your-data").unwrap(),
        );
        assert!(!css.contains("@font-face"), "{css}");
        assert!(css.contains("ui-sans-serif"), "{css}");
        assert!(css.contains("#b91c1c"), "{css}");
    }

    #[test]
    fn contrast_ratio_matches_known_wcag_reference_pairs() {
        let white = [0xff, 0xff, 0xff];
        assert!((contrast_ratio([0, 0, 0], white) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(white, white) - 1.0).abs() < 0.01);
    }

    /// ENG-586: every compiled palette's own primary hex — the values
    /// `store::seed::compiled_brand_presentation` migrates into the three
    /// house `brand` rows — clears `derive_scheme`'s own gate, proving the
    /// seed's values are not silently incompatible with the runtime path a
    /// custom brand takes.
    #[test]
    fn derive_scheme_accepts_every_compiled_primary() {
        for hex in ["#007c91", "#b91c1c", "#5c5100"] {
            let scheme = derive_scheme(hex).unwrap_or_else(|| panic!("{hex} must clear the gate"));
            assert_eq!(scheme.primary, hex);
        }
    }

    #[test]
    fn derive_scheme_refuses_only_a_malformed_hex() {
        assert!(derive_scheme("not-a-hex").is_none());
        // `max(contrast(hex, white), contrast(hex, black))` has a
        // mathematical floor of ~4.58 for every possible RGB value (see
        // `store::brands::validate_primary_hex`'s doc comment for the
        // derivation), so a pale colour that reads as low-contrast against
        // white still clears the gate against black.
        assert!(derive_scheme("#f5f5a0").is_some());
    }

    #[test]
    fn derive_scheme_picks_the_higher_contrast_on_primary() {
        // A dark primary contrasts more with white.
        let dark = derive_scheme("#007c91").unwrap();
        assert_eq!(dark.on_primary, "#ffffff");
        // A light, saturated primary that still clears the gate against
        // black should pick black as its on-primary.
        let light = derive_scheme("#fff176").unwrap();
        assert_eq!(light.on_primary, "#000000");
    }

    #[test]
    fn tokens_stylesheet_from_hex_names_the_derived_tokens_and_an_optional_font_face() {
        let scheme = derive_scheme("#007c91").unwrap();
        let css = tokens_stylesheet_from_hex(
            "'Custom Sans', sans-serif",
            Some(
                "@font-face{font-family:'Custom Sans';src:url('/assets/brands/custom/font.woff2')}",
            ),
            &scheme,
        );
        assert!(css.contains("--nav-color-primary: #007c91"), "{css}");
        assert!(css.contains("--nav-color-on-primary: #ffffff"), "{css}");
        assert!(css.contains("font-family:'Custom Sans'"), "{css}");
        assert!(css.contains("@media (prefers-color-scheme: dark)"), "{css}");

        let without_face = tokens_stylesheet_from_hex("sans-serif", None, &scheme);
        assert!(!without_face.contains("@font-face"), "{without_face}");
    }
}
