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

/// GORP Serif, Plus Jakarta Sans, Tinos, one system serif, and one system
/// sans. GORP is the only face whose licence is the Firm's rather than a
/// redistributable one, so it alone sets `operator_licence_required`. Plus
/// Jakarta Sans is OFL-1.1 and rides GORP's bucket lane anyway, so a fresh
/// clone carries no font bytes for either.
pub const TYPEFACES: &[Typeface] = &[
    Typeface {
        id: "gorp-serif",
        label: "GORP Serif",
        stack: "\"GORP Serif\", Georgia, serif",
        operator_licence_required: true,
    },
    Typeface {
        id: "plus-jakarta-sans",
        label: "Plus Jakarta Sans",
        stack: "\"Plus Jakarta Sans\", ui-sans-serif, system-ui, sans-serif",
        operator_licence_required: false,
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
    // The five practice-brand faces. All OFL-1.1, so none sets
    // `operator_licence_required`: a fork may redistribute them, unlike GORP.
    // Their bytes ride the same bucket lane as every other web font here, so
    // a fresh clone carries none of them and no request ever leaves for a
    // font CDN — which on an immigration or collection-defence site is a
    // visitor-privacy property, not a performance one.
    Typeface {
        id: "eb-garamond",
        label: "EB Garamond",
        stack: "\"EB Garamond\", Garamond, Georgia, serif",
        operator_licence_required: false,
    },
    Typeface {
        id: "source-sans-3",
        label: "Source Sans 3",
        stack: "\"Source Sans 3\", ui-sans-serif, system-ui, sans-serif",
        operator_licence_required: false,
    },
    Typeface {
        id: "source-serif-4",
        label: "Source Serif 4",
        stack: "\"Source Serif 4\", Georgia, \"Times New Roman\", serif",
        operator_licence_required: false,
    },
    // Devanagari and Latin in one family, so an Abhaya page translated into
    // Hindi keeps its face instead of dropping to a system fallback.
    Typeface {
        id: "mukta",
        label: "Mukta",
        stack: "\"Mukta\", ui-sans-serif, system-ui, sans-serif",
        operator_licence_required: false,
    },
    Typeface {
        id: "public-sans",
        label: "Public Sans",
        stack: "\"Public Sans\", ui-sans-serif, system-ui, sans-serif",
        operator_licence_required: false,
    },
    // The NYC summons practice reads as government-adjacent on purpose.
    Typeface {
        id: "libre-franklin",
        label: "Libre Franklin",
        stack: "\"Libre Franklin\", ui-sans-serif, system-ui, sans-serif",
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
    // --- The five practice brands ------------------------------------
    // Every primary below is a light-mode colour that clears WCAG AA on
    // white; each also carries a dark-mode primary raised to clear 7:1 on
    // the dark canvas, because the specified colours fail AA outright
    // against a dark surface. `brand_presentation` tests assert both.
    // Warm bronze — hearth and continuity, the Vesta reading. Not a
    // grief palette: the audience is planning ahead, not bereaved.
    Palette {
        id: "vesta",
        label: "Vesta bronze",
        light: PaletteScheme {
            primary: "#8A5A2B",
            primary_hover: "#774d25",
            primary_active: "#63411f",
            on_primary: "#ffffff",
            on_brand: "#ffffff",
            link: "#8A5A2B",
            link_hover: "#63411f",
            surface_subtle: "#f9f3ee",
            bg: None,
            surface: None,
            surface_raised: None,
            text: None,
            text_muted: None,
            border: None,
        },
        dark: PaletteScheme {
            primary: "#cb925a",
            primary_hover: "#d2a171",
            primary_active: "#dab189",
            on_primary: "#0d1117",
            on_brand: "#0d1117",
            link: "#cb925a",
            link_hover: "#d2a171",
            surface_subtle: "#302112",
            bg: None,
            surface: None,
            surface_raised: None,
            text: None,
            text_muted: None,
            border: None,
        },
    },
    // Deep claret. Gravity without alarm, and deliberately not a red
    // cross in any form — that emblem is protected by 18 U.S.C. § 706 and
    // appears nowhere in this brand's colour, mark, or iconography.
    Palette {
        id: "misericordia",
        label: "Misericordia claret",
        light: PaletteScheme {
            primary: "#7A1F2B",
            primary_hover: "#661a24",
            primary_active: "#51151d",
            on_primary: "#ffffff",
            on_brand: "#ffffff",
            link: "#7A1F2B",
            link_hover: "#51151d",
            surface_subtle: "#faedef",
            bg: None,
            surface: None,
            surface_raised: None,
            text: None,
            text_muted: None,
            border: None,
        },
        dark: PaletteScheme {
            primary: "#df838f",
            primary_hover: "#e59ba5",
            primary_active: "#ecb4bb",
            on_primary: "#0d1117",
            on_brand: "#0d1117",
            link: "#df838f",
            link_hover: "#e59ba5",
            surface_subtle: "#321015",
            bg: None,
            surface: None,
            surface_raised: None,
            text: None,
            text_muted: None,
            border: None,
        },
    },
    // Deep indigo — steadiness. Chosen over the saffron an immigration
    // brand borrowing a Sanskrit name would otherwise drift toward.
    Palette {
        id: "abhaya",
        label: "Abhaya indigo",
        light: PaletteScheme {
            primary: "#1F4E79",
            primary_hover: "#1a4165",
            primary_active: "#153450",
            on_primary: "#ffffff",
            on_brand: "#ffffff",
            link: "#1F4E79",
            link_hover: "#153450",
            surface_subtle: "#edf4fa",
            bg: None,
            surface: None,
            surface_raised: None,
            text: None,
            text_muted: None,
            border: None,
        },
        dark: PaletteScheme {
            primary: "#68a3d8",
            primary_hover: "#80b2de",
            primary_active: "#99c1e5",
            on_primary: "#0d1117",
            on_brand: "#0d1117",
            link: "#68a3d8",
            link_hover: "#80b2de",
            surface_subtle: "#102232",
            bg: None,
            surface: None,
            surface_raised: None,
            text: None,
            text_muted: None,
            border: None,
        },
    },
    // Green for relief. Collection defence, not settlement: the palette
    // carries no urgency or money-back signalling.
    Palette {
        id: "delete-your-debt",
        label: "DeleteYourDebt green",
        light: PaletteScheme {
            primary: "#1F6F4A",
            primary_hover: "#195b3d",
            primary_active: "#14472f",
            on_primary: "#ffffff",
            on_brand: "#ffffff",
            link: "#1F6F4A",
            link_hover: "#14472f",
            surface_subtle: "#eef9f4",
            bg: None,
            surface: None,
            surface_raised: None,
            text: None,
            text_muted: None,
            border: None,
        },
        dark: PaletteScheme {
            primary: "#32b377",
            primary_hover: "#3bc887",
            primary_active: "#53cf95",
            on_primary: "#0d1117",
            on_brand: "#0d1117",
            link: "#32b377",
            link_hover: "#3bc887",
            surface_subtle: "#113122",
            bg: None,
            surface: None,
            surface_raised: None,
            text: None,
            text_muted: None,
            border: None,
        },
    },
    // Plum-slate. The least branded face in the family on purpose — the
    // NYC summons audience is buying competence, not warmth.
    Palette {
        id: "oath",
        label: "Summons plum",
        light: PaletteScheme {
            primary: "#4A2545",
            primary_hover: "#391c35",
            primary_active: "#281425",
            on_primary: "#ffffff",
            on_brand: "#ffffff",
            link: "#4A2545",
            link_hover: "#281425",
            surface_subtle: "#f7f0f6",
            bg: None,
            surface: None,
            surface_raised: None,
            text: None,
            text_muted: None,
            border: None,
        },
        dark: PaletteScheme {
            primary: "#c58bbd",
            primary_hover: "#cf9fc9",
            primary_active: "#d9b4d4",
            on_primary: "#0d1117",
            on_brand: "#0d1117",
            link: "#c58bbd",
            link_hover: "#cf9fc9",
            surface_subtle: "#2b1828",
            bg: None,
            surface: None,
            surface_raised: None,
            text: None,
            text_muted: None,
            border: None,
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
                typeface_by_id("plus-jakarta-sans").expect("plus-jakarta-sans is catalogued")
            }
            Self::LawyerShook => typeface_by_id("tinos").expect("tinos is catalogued"),
            Self::Vesta => typeface_by_id("eb-garamond").expect("eb-garamond is catalogued"),
            Self::Misericordia => {
                typeface_by_id("source-sans-3").expect("source-sans-3 is catalogued")
            }
            Self::Abhaya => typeface_by_id("mukta").expect("mukta is catalogued"),
            Self::DeleteYourDebt => {
                typeface_by_id("public-sans").expect("public-sans is catalogued")
            }
            Self::Summons => {
                typeface_by_id("libre-franklin").expect("libre-franklin is catalogued")
            }
        }
    }

    /// The display face this key sets over [`Self::default_typeface`], when
    /// it wears two.
    ///
    /// `None` means headings and body share one face, which is true of every
    /// brand the firm shipped before the practice brands: a single
    /// `--nav-font-family` was the whole typographic contract. Misericordia
    /// is the reason it is no longer enough — it pairs a serif display face
    /// with a separate sans body face — so this returns the *display* half
    /// and `default_typeface` keeps meaning body.
    #[must_use]
    pub fn display_typeface(self) -> Option<&'static Typeface> {
        match self {
            // Misericordia pairs serif headings with a sans body.
            Self::Misericordia => {
                Some(typeface_by_id("source-serif-4").expect("source-serif-4 is catalogued"))
            }
            // One face throughout. For most of these that is simply how the
            // brand was drawn; for Abhaya it is the point of the choice,
            // since Mukta carries Devanagari as well as Latin and a second
            // display face would not.
            Self::Neon
            | Self::DeleteYourData
            | Self::LawyerShook
            | Self::Vesta
            | Self::Abhaya
            | Self::DeleteYourDebt
            | Self::Summons => None,
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
            Self::Vesta => palette_by_id("vesta").expect("vesta is catalogued"),
            Self::Misericordia => {
                palette_by_id("misericordia").expect("misericordia is catalogued")
            }
            Self::Abhaya => palette_by_id("abhaya").expect("abhaya is catalogued"),
            Self::DeleteYourDebt => {
                palette_by_id("delete-your-debt").expect("delete-your-debt is catalogued")
            }
            Self::Summons => palette_by_id("oath").expect("oath is catalogued"),
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
pub fn tokens_stylesheet(face: &Typeface, display: Option<&Typeface>, palette: &Palette) -> String {
    let mut css = String::new();
    if let Some(faces) = webfont_css(face) {
        css.push_str(&faces);
        css.push('\n');
    }
    // A brand whose display face differs from its body face carries both
    // sets of `@font-face` rules, as Misericordia does. A brand that
    // sets one face for everything emits one set.
    if let Some(display) = display.filter(|d| d.id != face.id) {
        if let Some(faces) = webfont_css(display) {
            css.push_str(&faces);
            css.push('\n');
        }
    }
    css.push_str(":root {\n");
    emit_scheme(&mut css, face, display, &palette.light);
    css.push_str("}\n\n@media (prefers-color-scheme: dark) {\n  :root {\n");
    emit_scheme(&mut css, face, display, &palette.dark);
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
        "plus-jakarta-sans" => Some(font_face_css(
            "Plus Jakarta Sans",
            &crate::assets::asset_url("fonts/plus-jakarta-sans/PlusJakartaSans-Regular.woff2"),
            &crate::assets::asset_url("fonts/plus-jakarta-sans/PlusJakartaSans-Bold.woff2"),
        )),
        "tinos" => Some(font_face_css(
            "Tinos",
            "/public/fonts/tinos/Tinos-Regular.woff2",
            "/public/fonts/tinos/Tinos-Bold.woff2",
        )),
        "eb-garamond" => Some(bucket_face("EB Garamond", "eb-garamond", "EBGaramond")),
        "source-sans-3" => Some(bucket_face("Source Sans 3", "source-sans-3", "SourceSans3")),
        "source-serif-4" => Some(bucket_face(
            "Source Serif 4",
            "source-serif-4",
            "SourceSerif4",
        )),
        "mukta" => Some(bucket_face("Mukta", "mukta", "Mukta")),
        "public-sans" => Some(bucket_face("Public Sans", "public-sans", "PublicSans")),
        "libre-franklin" => Some(bucket_face(
            "Libre Franklin",
            "libre-franklin",
            "LibreFranklin",
        )),
        _ => None,
    }
}

/// `@font-face` for an OFL family served from the deployment's asset bucket,
/// under the `fonts/<dir>/<Stem>-{Regular,Bold}.woff2` layout every web font
/// in this repository already uses.
///
/// Self-hosting is the requirement these faces exist to satisfy: the families
/// are all available from Google Fonts, and linking them there would leak a
/// request — carrying the visitor's IP and the referring page — to a third
/// party on every pageview. On an immigration or debt-collection-defence
/// site that is a disclosure about the reader, so the bytes come from our
/// own origin and no brand stylesheet ever names a font CDN.
fn bucket_face(family: &str, dir: &str, stem: &str) -> String {
    font_face_css(
        family,
        &crate::assets::asset_url(&format!("fonts/{dir}/{stem}-Regular.woff2")),
        &crate::assets::asset_url(&format!("fonts/{dir}/{stem}-Bold.woff2")),
    )
}

fn emit_scheme(
    css: &mut String,
    face: &Typeface,
    display: Option<&Typeface>,
    scheme: &PaletteScheme,
) {
    css.push_str("  --nav-font-family: ");
    css.push_str(face.stack);
    css.push_str(";\n");
    css.push_str("  --font-body: var(--nav-font-family);\n");
    // The brand contract a new site is written against. It is emitted from
    // the same `Palette`/`Typeface` values the `--nav-*` tokens below use,
    // so the two can never drift: this is one generator with two
    // vocabularies, not a hand-maintained alias list. `--nav-*` is what the
    // shared layout consumes; `--brand-*` is what a brand's own stylesheet
    // reads, and is the whole surface a new brand needs to supply.
    css.push_str("  --brand-font-body: var(--nav-font-family);\n");
    css.push_str("  --brand-font-display: ");
    css.push_str(display.unwrap_or(face).stack);
    css.push_str(";\n");
    push_token(css, "--brand-primary", scheme.primary);
    // The accessible text colour *on* the primary, not beside it. Every
    // catalogued pairing is asserted at WCAG AA by `brand_presentation`
    // tests, in both schemes.
    push_token(css, "--brand-primary-ink", scheme.on_primary);
    push_token(css, "--brand-surface", scheme.surface_subtle);
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
        (f64::from(channel) * (1.0 - factor))
            .round()
            .clamp(0.0, 255.0) as u8
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

    /// The list stays closed, and only GORP is licence-encumbered: Plus
    /// Jakarta Sans is OFL-1.1, so it is bucket-served like GORP without
    /// carrying GORP's `operator_licence_required` flag.
    #[test]
    fn the_typeface_list_is_closed_and_only_gorp_needs_a_licence() {
        let ids: Vec<_> = TYPEFACES.iter().map(|face| face.id).collect();
        assert_eq!(
            ids,
            [
                "gorp-serif",
                "plus-jakarta-sans",
                "tinos",
                "system-serif",
                "system-sans",
                // The practice brands. All OFL, all bucket-served.
                "eb-garamond",
                "source-sans-3",
                "source-serif-4",
                "mukta",
                "public-sans",
                "libre-franklin"
            ]
        );
        assert!(
            typeface_by_id("gorp-serif")
                .unwrap()
                .operator_licence_required
        );
        assert!(!typeface_by_id("tinos").unwrap().operator_licence_required);
        assert!(
            !typeface_by_id("plus-jakarta-sans")
                .unwrap()
                .operator_licence_required
        );
    }

    /// The DeleteYourData.com brand renders Plus Jakarta Sans from the assets
    /// bucket, the same operator-upload lane GORP rides: both faces resolve
    /// through `assets::asset_url`, so they follow the deployment's asset
    /// origin. Tinos, by contrast, hard-codes the `/public/fonts/...` static
    /// mount. With `NAVIGATOR_ASSET_BASE_URL` unset — as under test —
    /// `asset_url` falls back to that same `/public` mount, so comparing
    /// against it is what distinguishes the two lanes; a bare `/public`
    /// substring check cannot.
    #[test]
    fn plus_jakarta_sans_serves_its_faces_through_the_asset_origin() {
        let css = tokens_stylesheet(
            typeface_by_id("plus-jakarta-sans").unwrap(),
            None,
            palette_by_id("delete-your-data").unwrap(),
        );
        assert!(css.contains("font-family:'Plus Jakarta Sans'"), "{css}");
        for face in [
            "PlusJakartaSans-Regular.woff2",
            "PlusJakartaSans-Bold.woff2",
        ] {
            let url = crate::assets::asset_url(&format!("fonts/plus-jakarta-sans/{face}"));
            assert!(css.contains(&format!("url('{url}')")), "{face}: {css}");
        }
        assert!(
            css.contains("--nav-font-family: \"Plus Jakarta Sans\""),
            "{css}"
        );
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
            "plus-jakarta-sans"
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
            None,
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
            None,
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

    // --- WCAG AA, computed rather than asserted by eye --------------------

    /// Relative luminance, WCAG 2.x. Kept in the tests rather than shipped:
    /// nothing at runtime needs it, and a palette is only ever checked when
    /// someone edits the catalog.
    fn luminance(hex: &str) -> f64 {
        let hex = hex.trim_start_matches('#');
        assert_eq!(hex.len(), 6, "not a 6-digit hex: {hex}");
        let channel = |offset: usize| {
            let raw = u8::from_str_radix(&hex[offset..offset + 2], 16)
                .unwrap_or_else(|_| panic!("not hex: {hex}"));
            let c = f64::from(raw) / 255.0;
            if c <= 0.039_28 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(0) + 0.7152 * channel(2) + 0.0722 * channel(4)
    }

    fn contrast(a: &str, b: &str) -> f64 {
        let (x, y) = (luminance(a), luminance(b));
        let (hi, lo) = if x > y { (x, y) } else { (y, x) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// WCAG AA body text. Large text is 3.0; we hold every pairing to the
    /// stricter number so a token is never only conditionally safe.
    const AA_BODY: f64 = 4.5;

    /// The canvas a dark-scheme token is read against when the palette does
    /// not override `bg` — the value the shipped dark palettes use.
    const DARK_CANVAS: &str = "#0d1117";

    /// The contract ENG-740 is really about: text drawn *on* the primary has
    /// to be readable, in both schemes, for every brand in the catalog. A
    /// new brand that lands a pretty primary with unreadable ink fails here
    /// rather than in front of a client.
    #[test]
    fn every_catalogued_palette_puts_readable_ink_on_its_primary() {
        for palette in PALETTE {
            for (scheme_name, scheme) in [("light", &palette.light), ("dark", &palette.dark)] {
                let ratio = contrast(scheme.primary, scheme.on_primary);
                assert!(
                    ratio >= AA_BODY,
                    "{} {scheme_name}: ink {} on primary {} is {ratio:.2}:1, below AA {AA_BODY}",
                    palette.id,
                    scheme.on_primary,
                    scheme.primary,
                );
            }
        }
    }

    /// Every practice-brand primary is specified as a light-mode colour, and
    /// each fails AA outright against a dark canvas — Misericordia's claret
    /// is 2.06:1 on `#0d1117`. That is why each carries a separate dark
    /// primary, and this asserts the pair actually works on the surface it
    /// is drawn against, which single-scheme checking misses entirely.
    #[test]
    fn practice_brand_primaries_clear_aa_against_their_own_canvas() {
        for id in [
            "vesta",
            "misericordia",
            "abhaya",
            "delete-your-debt",
            "oath",
        ] {
            let palette = palette_by_id(id).expect("practice brand is catalogued");

            let light = contrast(palette.light.primary, "#ffffff");
            assert!(
                light >= AA_BODY,
                "{id} light primary {} is {light:.2}:1 on white",
                palette.light.primary,
            );

            let dark = contrast(palette.dark.primary, DARK_CANVAS);
            assert!(
                dark >= AA_BODY,
                "{id} dark primary {} is {dark:.2}:1 on {DARK_CANVAS}",
                palette.dark.primary,
            );

            // The link colours are read as text on the same canvas, so they
            // carry the same floor.
            let link_light = contrast(palette.light.link, "#ffffff");
            assert!(
                link_light >= AA_BODY,
                "{id} light link is {link_light:.2}:1"
            );
            let link_dark = contrast(palette.dark.link, DARK_CANVAS);
            assert!(link_dark >= AA_BODY, "{id} dark link is {link_dark:.2}:1");
        }
    }

    // --- The brand token contract ----------------------------------------

    /// The five names ENG-740 defines as the seam a new brand is written
    /// against, in both schemes.
    #[test]
    fn the_brand_token_contract_is_emitted_in_both_schemes() {
        let body = typeface_by_id("source-sans-3").expect("catalogued");
        let display = typeface_by_id("eb-garamond").expect("catalogued");
        let palette = palette_by_id("vesta").expect("catalogued");
        let css = tokens_stylesheet(body, Some(display), palette);

        for token in [
            "--brand-primary:",
            "--brand-primary-ink:",
            "--brand-surface:",
            "--brand-font-display:",
            "--brand-font-body:",
        ] {
            assert_eq!(
                css.matches(token).count(),
                2,
                "{token} should appear once per scheme in:\n{css}"
            );
        }

        // The display face reaches the contract, not just the body face.
        assert!(
            css.contains("--brand-font-display: \"EB Garamond\""),
            "{css}"
        );
        assert!(css.contains("--brand-primary: #8A5A2B"), "{css}");
    }

    /// A two-face brand self-hosts both faces. Missing the display face here
    /// is the bug that silently falls back to Georgia.
    #[test]
    fn a_two_face_brand_emits_font_faces_for_both() {
        let body = typeface_by_id("source-sans-3").expect("catalogued");
        let display = typeface_by_id("source-serif-4").expect("catalogued");
        let palette = palette_by_id("misericordia").expect("catalogued");
        let css = tokens_stylesheet(body, Some(display), palette);

        assert!(css.contains("font-family:'Source Sans 3'"), "{css}");
        assert!(css.contains("font-family:'Source Serif 4'"), "{css}");
    }

    /// A brand that sets one face for everything does not emit it twice.
    #[test]
    fn a_one_face_brand_emits_that_face_once() {
        let face = typeface_by_id("public-sans").expect("catalogued");
        let palette = palette_by_id("delete-your-debt").expect("catalogued");
        let css = tokens_stylesheet(face, Some(face), palette);

        assert_eq!(css.matches("font-family:'Public Sans'").count(), 2, "{css}");
        assert!(
            css.contains("--brand-font-display: \"Public Sans\""),
            "{css}"
        );
    }

    /// No brand stylesheet may reach a font CDN. This is a privacy property
    /// for the immigration and collection-defence audiences, not a
    /// performance one: a Google Fonts link discloses the visitor's IP and
    /// the page they are reading to a third party on every pageview.
    #[test]
    fn no_catalogued_face_is_served_from_a_font_cdn() {
        for face in TYPEFACES {
            let Some(css) = webfont_css(face) else {
                continue;
            };
            for host in [
                "fonts.googleapis.com",
                "fonts.gstatic.com",
                "use.typekit.net",
                "cdn.jsdelivr.net",
                "cdnjs.cloudflare.com",
            ] {
                assert!(
                    !css.contains(host),
                    "{} is served from {host}: {css}",
                    face.id,
                );
            }
        }
    }

    /// The practice faces are all OFL, so none is licence-encumbered the way
    /// GORP is — a fork may redistribute them.
    #[test]
    fn the_practice_faces_are_redistributable() {
        for id in [
            "eb-garamond",
            "source-sans-3",
            "source-serif-4",
            "mukta",
            "public-sans",
            "libre-franklin",
        ] {
            let face = typeface_by_id(id).expect("practice face is catalogued");
            assert!(
                !face.operator_licence_required,
                "{id} is OFL and should not require an operator licence",
            );
            assert!(
                webfont_css(face).is_some(),
                "{id} must be self-hosted, not left to a system fallback",
            );
        }
    }
}
