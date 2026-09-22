//! Inline-SVG icons for the Dioxus components (issue #641, Phase 2).
//!
//! The shared inline-SVG icon set. Each glyph is rendered by this Dioxus
//! component, so pages never need an icon font. The path data is inlined at a
//! 16×16 viewBox and drawn at `1em` with `currentColor`, so an icon inherits the
//! surrounding text's size and color. Litigation's scales-of-justice mark
//! (Libra) is carried here too.
//!
//! An [`Icon`] with a `label` is a meaningful image (`role="img"` + a `<title>`
//! accessible name); without one it is decorative (`aria-hidden`), mirroring the
//! component's `aria-hidden` icons.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Icons used by Navigator components and public product pages.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum IconName {
    StarFill,
    BuildingFill,
    ShieldFillCheck,
    ShieldLock,
    Eyeglasses,
    PencilSquare,
    Trash3Fill,
    Eye,
    Github,
    Diagram3Fill,
    CheckLg,
    XLg,
    HouseDoorFill,
    HddNetworkFill,
    TreeFill,
    AwardFill,
    VinylFill,
    Bank2,
    HeartFill,
    CloudFill,
    Incognito,
    /// The upper-right "opens off-site" arrow, decorating anchors that leave
    /// our domains (matches the `ExternalLink` glyph).
    BoxArrowUpRight,
    /// The left-pointing arrow on a "back to parent" breadcrumb.
    ArrowLeft,
    /// The scales of justice used for litigation.
    LibraScales,
    /// A closed envelope, for an email contact channel.
    EnvelopeFill,
    /// A handset, for a phone contact channel.
    TelephoneFill,
    /// The X mark, for the firm's profile on X.
    X,
    /// The `LinkedIn` mark.
    LinkedIn,
    /// The `YouTube` mark.
    YouTube,
}

/// The catalog spelling for the inline scales icon.
pub const LIBRA_SCALES: &str = "libra-scales";

impl IconName {
    /// Stable catalog name for this icon. It is presentation data, not a CSS
    /// class, so callers never couple to an icon font.
    #[must_use]
    pub const fn glyph(self) -> &'static str {
        match self {
            Self::StarFill => "star-fill",
            Self::BuildingFill => "building-fill",
            Self::ShieldFillCheck => "shield-fill-check",
            Self::ShieldLock => "shield-lock",
            Self::Eyeglasses => "eyeglasses",
            Self::PencilSquare => "pencil-square",
            Self::Trash3Fill => "trash3-fill",
            Self::Eye => "eye",
            Self::Github => "github",
            Self::Diagram3Fill => "diagram-3-fill",
            Self::CheckLg => "check-lg",
            Self::XLg => "x-lg",
            Self::HouseDoorFill => "house-door-fill",
            Self::HddNetworkFill => "hdd-network-fill",
            Self::TreeFill => "tree-fill",
            Self::AwardFill => "award-fill",
            Self::VinylFill => "vinyl-fill",
            Self::Bank2 => "bank2",
            Self::HeartFill => "heart-fill",
            Self::CloudFill => "cloud-fill",
            Self::Incognito => "incognito",
            Self::BoxArrowUpRight => "box-arrow-up-right",
            Self::ArrowLeft => "arrow-left",
            Self::LibraScales => LIBRA_SCALES,
            Self::EnvelopeFill => "envelope-fill",
            Self::TelephoneFill => "telephone-fill",
            Self::X => "x",
            Self::LinkedIn => "linkedin",
            Self::YouTube => "youtube",
        }
    }

    /// Resolve a legacy catalog icon spelling to its typed inline-SVG icon.
    #[must_use]
    pub fn from_catalog_name(name: &str) -> Option<Self> {
        match name {
            "star-fill" => Some(Self::StarFill),
            "building-fill" => Some(Self::BuildingFill),
            "shield-fill-check" => Some(Self::ShieldFillCheck),
            "shield-lock" => Some(Self::ShieldLock),
            "eyeglasses" => Some(Self::Eyeglasses),
            "pencil-square" => Some(Self::PencilSquare),
            "trash3-fill" => Some(Self::Trash3Fill),
            "eye" => Some(Self::Eye),
            "github" => Some(Self::Github),
            "diagram-3-fill" => Some(Self::Diagram3Fill),
            "check-lg" => Some(Self::CheckLg),
            "x-lg" => Some(Self::XLg),
            "house-door-fill" => Some(Self::HouseDoorFill),
            "hdd-network-fill" => Some(Self::HddNetworkFill),
            "tree-fill" => Some(Self::TreeFill),
            "award-fill" => Some(Self::AwardFill),
            "vinyl-fill" => Some(Self::VinylFill),
            "bank2" => Some(Self::Bank2),
            "heart-fill" => Some(Self::HeartFill),
            "cloud-fill" => Some(Self::CloudFill),
            "incognito" => Some(Self::Incognito),
            LIBRA_SCALES => Some(Self::LibraScales),
            "envelope-fill" => Some(Self::EnvelopeFill),
            "telephone-fill" => Some(Self::TelephoneFill),
            "x" => Some(Self::X),
            "linkedin" => Some(Self::LinkedIn),
            "youtube" => Some(Self::YouTube),
            _ => None,
        }
    }
}

/// Render an icon as inline SVG. Pass `label` for a meaningful icon (it becomes
/// the accessible name); omit it for a decorative one.
#[component]
pub fn Icon(name: IconName, #[props(default)] label: Option<String>) -> Element {
    let decorative = label.is_none();
    rsx! {
        svg {
            class: "nav-icon",
            xmlns: "http://www.w3.org/2000/svg",
            "viewBox": "0 0 16 16",
            width: "1em",
            height: "1em",
            fill: "currentColor",
            role: "img",
            "aria-hidden": if decorative { "true" } else { "false" },
            if let Some(label) = label {
                title { "{label}" }
            }
            {icon_body(name)}
        }
    }
}

/// The path/shape elements for one glyph, verbatim Bootstrap Icons (MIT) at a
/// 16×16 viewBox — except [`IconName::LibraScales`], the inline scales drawing
/// carried over from the `product_icon`.
fn icon_body(name: IconName) -> Element {
    match name {
        IconName::StarFill => rsx! {
            path { d: "M3.612 15.443c-.386.198-.824-.149-.746-.592l.83-4.73L.173 6.765c-.329-.314-.158-.888.283-.95l4.898-.696L7.538.792c.197-.39.73-.39.927 0l2.184 4.327 4.898.696c.441.062.612.636.282.95l-3.522 3.356.83 4.73c.078.443-.36.79-.746.592L8 13.187l-4.389 2.256z" }
        },
        IconName::BuildingFill => rsx! {
            path { d: "M3 0a1 1 0 0 0-1 1v14a1 1 0 0 0 1 1h3v-3.5a.5.5 0 0 1 .5-.5h3a.5.5 0 0 1 .5.5V16h3a1 1 0 0 0 1-1V1a1 1 0 0 0-1-1zm1 2.5a.5.5 0 0 1 .5-.5h1a.5.5 0 0 1 .5.5v1a.5.5 0 0 1-.5.5h-1a.5.5 0 0 1-.5-.5zm3 0a.5.5 0 0 1 .5-.5h1a.5.5 0 0 1 .5.5v1a.5.5 0 0 1-.5.5h-1a.5.5 0 0 1-.5-.5zm3.5-.5h1a.5.5 0 0 1 .5.5v1a.5.5 0 0 1-.5.5h-1a.5.5 0 0 1-.5-.5v-1a.5.5 0 0 1 .5-.5M4 5.5a.5.5 0 0 1 .5-.5h1a.5.5 0 0 1 .5.5v1a.5.5 0 0 1-.5.5h-1a.5.5 0 0 1-.5-.5zM7.5 5h1a.5.5 0 0 1 .5.5v1a.5.5 0 0 1-.5.5h-1a.5.5 0 0 1-.5-.5v-1a.5.5 0 0 1 .5-.5m2.5.5a.5.5 0 0 1 .5-.5h1a.5.5 0 0 1 .5.5v1a.5.5 0 0 1-.5.5h-1a.5.5 0 0 1-.5-.5zM4.5 8h1a.5.5 0 0 1 .5.5v1a.5.5 0 0 1-.5.5h-1a.5.5 0 0 1-.5-.5v-1a.5.5 0 0 1 .5-.5m2.5.5a.5.5 0 0 1 .5-.5h1a.5.5 0 0 1 .5.5v1a.5.5 0 0 1-.5.5h-1a.5.5 0 0 1-.5-.5zm3.5-.5h1a.5.5 0 0 1 .5.5v1a.5.5 0 0 1-.5.5h-1a.5.5 0 0 1-.5-.5v-1a.5.5 0 0 1 .5-.5" }
        },
        IconName::ShieldFillCheck => rsx! {
            path { "fill-rule": "evenodd", d: "M8 0c-.69 0-1.843.265-2.928.56-1.11.3-2.229.655-2.887.87a1.54 1.54 0 0 0-1.044 1.262c-.596 4.477.787 7.795 2.465 9.99a11.8 11.8 0 0 0 2.517 2.453c.386.273.744.482 1.048.625.28.132.581.24.829.24s.548-.108.829-.24a7 7 0 0 0 1.048-.625 11.8 11.8 0 0 0 2.517-2.453c1.678-2.195 3.061-5.513 2.465-9.99a1.54 1.54 0 0 0-1.044-1.263 63 63 0 0 0-2.887-.87C9.843.266 8.69 0 8 0m2.146 5.146a.5.5 0 0 1 .708.708l-3 3a.5.5 0 0 1-.708 0l-1.5-1.5a.5.5 0 1 1 .708-.708L7.5 7.793z" }
        },
        IconName::ShieldLock => rsx! {
            path { d: "M5.338 1.59a61 61 0 0 0-2.837.856.48.48 0 0 0-.328.39c-.554 4.157.726 7.19 2.253 9.188a10.7 10.7 0 0 0 2.287 2.233c.346.244.652.42.893.533q.18.085.293.118a1 1 0 0 0 .101.025 1 1 0 0 0 .1-.025q.114-.034.294-.118c.24-.113.547-.29.893-.533a10.7 10.7 0 0 0 2.287-2.233c1.527-1.997 2.807-5.031 2.253-9.188a.48.48 0 0 0-.328-.39c-.651-.213-1.75-.56-2.837-.855C9.552 1.29 8.531 1.067 8 1.067c-.53 0-1.552.223-2.662.524zM5.072.56C6.157.265 7.31 0 8 0s1.843.265 2.928.56c1.11.3 2.229.655 2.887.87a1.54 1.54 0 0 1 1.044 1.262c.596 4.477-.787 7.795-2.465 9.99a11.8 11.8 0 0 1-2.517 2.453 7 7 0 0 1-1.048.625c-.28.132-.581.24-.829.24s-.548-.108-.829-.24a7 7 0 0 1-1.048-.625 11.8 11.8 0 0 1-2.517-2.453C1.928 10.487.545 7.169 1.141 2.692A1.54 1.54 0 0 1 2.185 1.43 63 63 0 0 1 5.072.56" }
            path { d: "M9.5 6.5a1.5 1.5 0 0 1-1 1.415l.385 1.99a.5.5 0 0 1-.491.595h-.788a.5.5 0 0 1-.49-.595l.384-1.99a1.5 1.5 0 1 1 2-1.415" }
        },
        IconName::Eyeglasses => rsx! {
            path { d: "M4 6a2 2 0 1 1 0 4 2 2 0 0 1 0-4m2.625.547a3 3 0 0 0-5.584.953H.5a.5.5 0 0 0 0 1h.541A3 3 0 0 0 7 8a1 1 0 0 1 2 0 3 3 0 0 0 5.959.5h.541a.5.5 0 0 0 0-1h-.541a3 3 0 0 0-5.584-.953A2 2 0 0 0 8 6c-.532 0-1.016.208-1.375.547M14 8a2 2 0 1 1-4 0 2 2 0 0 1 4 0" }
        },
        IconName::PencilSquare => rsx! {
            path { d: "M15.502 1.94a.5.5 0 0 1 0 .706L14.459 3.69l-2-2L13.502.646a.5.5 0 0 1 .707 0l1.293 1.293zm-1.75 2.456-2-2L4.939 9.21a.5.5 0 0 0-.121.196l-.805 2.414a.25.25 0 0 0 .316.316l2.414-.805a.5.5 0 0 0 .196-.12l6.813-6.814z" }
            path { "fill-rule": "evenodd", d: "M1 13.5A1.5 1.5 0 0 0 2.5 15h11a1.5 1.5 0 0 0 1.5-1.5v-6a.5.5 0 0 0-1 0v6a.5.5 0 0 1-.5.5h-11a.5.5 0 0 1-.5-.5v-11a.5.5 0 0 1 .5-.5H9a.5.5 0 0 0 0-1H2.5A1.5 1.5 0 0 0 1 2.5z" }
        },
        IconName::Trash3Fill => rsx! {
            path { d: "M11 1.5v1h3.5a.5.5 0 0 1 0 1h-.538l-.853 10.66A2 2 0 0 1 11.115 16h-6.23a2 2 0 0 1-1.994-1.84L2.038 3.5H1.5a.5.5 0 0 1 0-1H5v-1A1.5 1.5 0 0 1 6.5 0h3A1.5 1.5 0 0 1 11 1.5m-5 0v1h4v-1a.5.5 0 0 0-.5-.5h-3a.5.5 0 0 0-.5.5M4.5 5.029l.5 8.5a.5.5 0 1 0 .998-.06l-.5-8.5a.5.5 0 1 0-.998.06m6.53-.528a.5.5 0 0 0-.528.47l-.5 8.5a.5.5 0 0 0 .998.058l.5-8.5a.5.5 0 0 0-.47-.528M8 4.5a.5.5 0 0 0-.5.5v8.5a.5.5 0 0 0 1 0V5a.5.5 0 0 0-.5-.5" }
        },
        IconName::Eye => rsx! {
            path { d: "M16 8s-3-5.5-8-5.5S0 8 0 8s3 5.5 8 5.5S16 8 16 8M1.173 8a13 13 0 0 1 1.66-2.043C4.12 4.668 5.88 3.5 8 3.5s3.879 1.168 5.168 2.457A13 13 0 0 1 14.828 8q-.086.13-.195.288c-.335.48-.83 1.12-1.465 1.755C11.879 11.332 10.119 12.5 8 12.5s-3.879-1.168-5.168-2.457A13 13 0 0 1 1.172 8z" }
            path { d: "M8 5.5a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5M4.5 8a3.5 3.5 0 1 1 7 0 3.5 3.5 0 0 1-7 0" }
        },
        IconName::Github => rsx! {
            path { d: "M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27s1.36.09 2 .27c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0 0 16 8c0-4.42-3.58-8-8-8" }
        },
        IconName::Diagram3Fill => rsx! {
            path { "fill-rule": "evenodd", d: "M6 3.5A1.5 1.5 0 0 1 7.5 2h1A1.5 1.5 0 0 1 10 3.5v1A1.5 1.5 0 0 1 8.5 6v1H14a.5.5 0 0 1 .5.5v1a.5.5 0 0 1-1 0V8h-5v.5a.5.5 0 0 1-1 0V8h-5v.5a.5.5 0 0 1-1 0v-1A.5.5 0 0 1 2 7h5.5V6A1.5 1.5 0 0 1 6 4.5zm-6 8A1.5 1.5 0 0 1 1.5 10h1A1.5 1.5 0 0 1 4 11.5v1A1.5 1.5 0 0 1 2.5 14h-1A1.5 1.5 0 0 1 0 12.5zm6 0A1.5 1.5 0 0 1 7.5 10h1a1.5 1.5 0 0 1 1.5 1.5v1A1.5 1.5 0 0 1 8.5 14h-1A1.5 1.5 0 0 1 6 12.5zm6 0a1.5 1.5 0 0 1 1.5-1.5h1a1.5 1.5 0 0 1 1.5 1.5v1a1.5 1.5 0 0 1-1.5 1.5h-1a1.5 1.5 0 0 1-1.5-1.5z" }
        },
        IconName::CheckLg => rsx! {
            path { d: "M12.736 3.97a.733.733 0 0 1 1.047 0c.286.289.29.756.01 1.05L7.88 12.01a.733.733 0 0 1-1.065.02L3.217 8.384a.757.757 0 0 1 0-1.06.733.733 0 0 1 1.047 0l3.052 3.093 5.4-6.425z" }
        },
        IconName::XLg => rsx! {
            path { d: "M2.146 2.854a.5.5 0 1 1 .708-.708L8 7.293l5.146-5.147a.5.5 0 0 1 .708.708L8.707 8l5.147 5.146a.5.5 0 0 1-.708.708L8 8.707l-5.146 5.147a.5.5 0 0 1-.708-.708L7.293 8z" }
        },
        IconName::BoxArrowUpRight => rsx! {
            path { "fill-rule": "evenodd", d: "M8.636 3.5a.5.5 0 0 0-.5-.5H1.5A1.5 1.5 0 0 0 0 4.5v10A1.5 1.5 0 0 0 1.5 16h10a1.5 1.5 0 0 0 1.5-1.5V7.864a.5.5 0 0 0-1 0V14.5a.5.5 0 0 1-.5.5h-10a.5.5 0 0 1-.5-.5v-10a.5.5 0 0 1 .5-.5h6.636a.5.5 0 0 0 .5-.5" }
            path { "fill-rule": "evenodd", d: "M16 .5a.5.5 0 0 0-.5-.5h-5a.5.5 0 0 0 0 1h3.793L6.146 9.146a.5.5 0 1 0 .708.708L15 1.707V5.5a.5.5 0 0 0 1 0z" }
        },
        IconName::ArrowLeft => rsx! {
            path { "fill-rule": "evenodd", d: "M15 8a.5.5 0 0 0-.5-.5H2.707l3.147-3.146a.5.5 0 1 0-.708-.708l-4 4a.5.5 0 0 0 0 .708l4 4a.5.5 0 0 0 .708-.708L2.707 8.5H14.5A.5.5 0 0 0 15 8" }
        },
        IconName::HouseDoorFill => rsx! {
            path { d: "M6.5 14.5v-3.505c0-.245.25-.495.5-.495h2c.25 0 .5.25.5.5v3.5a.5.5 0 0 0 .5.5h4a.5.5 0 0 0 .5-.5v-7a.5.5 0 0 0-.146-.354L13 5.793V2.5a.5.5 0 0 0-.5-.5h-1a.5.5 0 0 0-.5.5v1.293L8.354 1.146a.5.5 0 0 0-.708 0l-6 6A.5.5 0 0 0 1.5 7.5v7a.5.5 0 0 0 .5.5h4a.5.5 0 0 0 .5-.5" }
        },
        IconName::HddNetworkFill => rsx! {
            path { d: "M2 2a2 2 0 0 0-2 2v1a2 2 0 0 0 2 2h5.5v3A1.5 1.5 0 0 0 6 11.5H.5a.5.5 0 0 0 0 1H6A1.5 1.5 0 0 0 7.5 14h1a1.5 1.5 0 0 0 1.5-1.5h5.5a.5.5 0 0 0 0-1H10A1.5 1.5 0 0 0 8.5 10V7H14a2 2 0 0 0 2-2V4a2 2 0 0 0-2-2zm.5 3a.5.5 0 1 1 0-1 .5.5 0 0 1 0 1m2 0a.5.5 0 1 1 0-1 .5.5 0 0 1 0 1" }
        },
        IconName::TreeFill => rsx! {
            path { d: "M8.416.223a.5.5 0 0 0-.832 0l-3 4.5A.5.5 0 0 0 5 5.5h.098L3.076 8.735A.5.5 0 0 0 3.5 9.5h.191l-1.638 3.276a.5.5 0 0 0 .447.724H7V16h2v-2.5h4.5a.5.5 0 0 0 .447-.724L12.31 9.5h.191a.5.5 0 0 0 .424-.765L10.902 5.5H11a.5.5 0 0 0 .416-.777z" }
        },
        IconName::AwardFill => rsx! {
            path { d: "m8 0 1.669.864 1.858.282.842 1.68 1.337 1.32L13.4 6l.306 1.854-1.337 1.32-.842 1.68-1.858.282L8 12l-1.669-.864-1.858-.282-.842-1.68-1.337-1.32L2.6 6l-.306-1.854 1.337-1.32.842-1.68L6.331.864z" }
            path { d: "M4 11.794V16l4-1 4 1v-4.206l-2.018.306L8 13.126 6.018 12.1z" }
        },
        IconName::VinylFill => rsx! {
            path { d: "M8 6a2 2 0 1 0 0 4 2 2 0 0 0 0-4m0 3a1 1 0 1 1 0-2 1 1 0 0 1 0 2" }
            path { d: "M16 8A8 8 0 1 1 0 8a8 8 0 0 1 16 0M4 8a4 4 0 1 0 8 0 4 4 0 0 0-8 0" }
        },
        IconName::Bank2 => rsx! {
            path { d: "M8.277.084a.5.5 0 0 0-.554 0l-7.5 5A.5.5 0 0 0 .5 6h1.875v7H1.5a.5.5 0 0 0 0 1h13a.5.5 0 1 0 0-1h-.875V6H15.5a.5.5 0 0 0 .277-.916zM12.375 6v7h-1.25V6zm-2.5 0v7h-1.25V6zm-2.5 0v7h-1.25V6zm-2.5 0v7h-1.25V6zM8 4a1 1 0 1 1 0-2 1 1 0 0 1 0 2M.5 15a.5.5 0 0 0 0 1h15a.5.5 0 1 0 0-1z" }
        },
        IconName::HeartFill => rsx! {
            path { "fill-rule": "evenodd", d: "M8 1.314C12.438-3.248 23.534 4.735 8 15-7.534 4.736 3.562-3.248 8 1.314" }
        },
        IconName::CloudFill => rsx! {
            path { d: "M4.406 3.342A5.53 5.53 0 0 1 8 2c2.69 0 4.923 2 5.166 4.579C14.758 6.804 16 8.137 16 9.773 16 11.569 14.502 13 12.687 13H3.781C1.708 13 0 11.366 0 9.318c0-1.763 1.266-3.223 2.942-3.593.143-.863.698-1.723 1.464-2.383" }
        },
        IconName::Incognito => rsx! {
            path { "fill-rule": "evenodd", d: "m4.736 1.968-.892 3.269-.014.058C2.113 5.568 1 6.006 1 6.5 1 7.328 4.134 8 8 8s7-.672 7-1.5c0-.494-1.113-.932-2.83-1.205l-.014-.058-.892-3.27c-.146-.533-.698-.849-1.239-.734C9.411 1.363 8.62 1.5 8 1.5s-1.411-.136-2.025-.267c-.541-.115-1.093.2-1.239.735m.015 3.867a.25.25 0 0 1 .274-.224c.9.092 1.91.143 2.975.143a30 30 0 0 0 2.975-.143.25.25 0 0 1 .05.498c-.918.093-1.944.145-3.025.145s-2.107-.052-3.025-.145a.25.25 0 0 1-.224-.274M3.5 10h2a.5.5 0 0 1 .5.5v1a1.5 1.5 0 0 1-3 0v-1a.5.5 0 0 1 .5-.5m-1.5.5q.001-.264.085-.5H2a.5.5 0 0 1 0-1h3.5a1.5 1.5 0 0 1 1.488 1.312 3.5 3.5 0 0 1 2.024 0A1.5 1.5 0 0 1 10.5 9H14a.5.5 0 0 1 0 1h-.085q.084.236.085.5v1a2.5 2.5 0 0 1-5 0v-.14l-.21-.07a2.5 2.5 0 0 0-1.58 0l-.21.07v.14a2.5 2.5 0 0 1-5 0zm8.5-.5h2a.5.5 0 0 1 .5.5v1a1.5 1.5 0 0 1-3 0v-1a.5.5 0 0 1 .5-.5" }
        },
        IconName::LibraScales => rsx! {
            circle { cx: "8", cy: "1.9", r: "0.95" }
            rect { x: "7.55", y: "2.4", width: "0.9", height: "10.4" }
            rect { x: "6.7", y: "12.5", width: "2.6", height: "0.9" }
            rect { x: "5.3", y: "13.4", width: "5.4", height: "1", rx: "0.5" }
            rect { x: "2", y: "3.45", width: "12", height: "0.95", rx: "0.47" }
            path {
                d: "M2.5 4.2 0.9 7.4M2.5 4.2 4.1 7.4M13.5 4.2 11.9 7.4M13.5 4.2 15.1 7.4",
                stroke: "currentColor",
                "stroke-width": "0.5",
                fill: "none",
                "stroke-linecap": "round",
            }
            path { d: "M0.6 7.3a1.9 1.9 0 0 0 3.8 0z" }
            path { d: "M11.6 7.3a1.9 1.9 0 0 0 3.8 0z" }
        },
        IconName::EnvelopeFill => rsx! {
            path { d: "M.05 3.555A2 2 0 0 1 2 2h12a2 2 0 0 1 1.95 1.555L8 8.414zm.05 1.143v7.104l5.803-3.558zM6.761 8.83l-6.57 4.027A2 2 0 0 0 2 14h12a2 2 0 0 0 1.808-1.144l-6.57-4.027L8 9.586zm3.436-.586L16 11.801V4.697z" }
        },
        IconName::TelephoneFill => rsx! {
            path { d: "M1.885.511a1.745 1.745 0 0 1 2.61.163L6.29 2.98c.329.423.445.974.315 1.494l-.547 2.19a.68.68 0 0 0 .178.643l2.457 2.457a.68.68 0 0 0 .644.178l2.189-.547a1.75 1.75 0 0 1 1.494.315l2.306 1.795c.834.65.905 1.87.163 2.611l-1.034 1.034c-.74.74-1.846 1.065-2.877.702a18.6 18.6 0 0 1-7.01-4.42 18.6 18.6 0 0 1-4.42-7.009c-.362-1.03-.037-2.137.703-2.877z" }
        },
        IconName::X | IconName::LinkedIn | IconName::YouTube => social_mark(name),
    }
}

/// Bootstrap Icons (MIT), the same set as the other glyphs.
fn social_mark(name: IconName) -> Element {
    match name {
        IconName::X => rsx! {
            path { d: "M12.6.75h2.454l-5.36 6.142L16 15.25h-4.937l-3.867-5.07-4.425 5.07H.316l5.733-6.57L0 .75h5.063l3.495 4.633L12.601.75Zm-.86 13.028h1.36L4.323 2.145H2.865z" }
        },
        IconName::LinkedIn => rsx! {
            path { d: "M0 1.146C0 .513.526 0 1.175 0h13.65C15.474 0 16 .513 16 1.146v13.708c0 .633-.526 1.146-1.175 1.146H1.175C.526 16 0 15.487 0 14.854zm4.943 12.248V6.169H2.542v7.225zm-1.2-8.212c.837 0 1.358-.554 1.358-1.248-.015-.709-.52-1.248-1.342-1.248S2.4 3.226 2.4 3.934c0 .694.521 1.248 1.327 1.248zm4.908 8.212V9.359c0-.216.016-.432.08-.586.173-.431.568-.878 1.232-.878.869 0 1.216.662 1.216 1.634v3.865h2.401V9.25c0-2.22-1.184-3.252-2.764-3.252-1.274 0-1.845.7-2.165 1.193V6.169h-2.4c.03.678 0 7.225 0 7.225z" }
        },
        IconName::YouTube => rsx! {
            path { d: "M8.051 1.999h.089c.822.003 4.987.033 6.11.335a2.01 2.01 0 0 1 1.415 1.42c.101.38.172.883.22 1.402l.01.104.022.26.008.104c.065.914.073 1.77.074 1.957v.075c-.001.194-.01 1.108-.082 2.06l-.008.105-.009.104c-.05.572-.124 1.14-.235 1.558a2.01 2.01 0 0 1-1.415 1.42c-1.16.312-5.569.334-6.18.335h-.142c-.309 0-1.587-.006-2.927-.052l-.17-.006-.087-.004-.171-.007-.171-.007c-1.11-.049-2.167-.128-2.654-.26a2.01 2.01 0 0 1-1.415-1.419c-.111-.417-.185-.986-.235-1.558L.09 9.82l-.008-.104A31 31 0 0 1 0 7.68v-.123c.002-.215.01-.958.064-1.778l.007-.103.003-.052.008-.104.022-.26.01-.104c.048-.519.119-1.023.22-1.402a2.01 2.01 0 0 1 1.415-1.42c.487-.13 1.544-.21 2.654-.26l.17-.007.172-.006.086-.003.171-.007A100 100 0 0 1 7.858 2zM6.4 5.209v4.818l4.157-2.408z" }
        },
        _ => rsx! {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(name: IconName, label: Option<&str>) -> String {
        let mut dom = VirtualDom::new_with_props(
            Icon,
            IconProps {
                name,
                label: label.map(str::to_string),
            },
        );
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    #[test]
    fn decorative_icon_is_aria_hidden_and_has_no_title() {
        let html = render(IconName::StarFill, None);
        assert!(html.contains("aria-hidden=\"true\""), "{html}");
        assert!(!html.contains("<title>"), "{html}");
    }

    #[test]
    fn labeled_icon_carries_its_accessible_name() {
        let html = render(IconName::LibraScales, Some("Litigation"));
        assert!(html.contains("<title>Litigation</title>"), "{html}");
        assert!(html.contains("aria-hidden=\"false\""), "{html}");
    }

    #[test]
    fn catalog_names_resolve_to_inline_icons() {
        assert_eq!(
            IconName::from_catalog_name("trash3-fill"),
            Some(IconName::Trash3Fill)
        );
        assert_eq!(
            IconName::from_catalog_name(LIBRA_SCALES),
            Some(IconName::LibraScales)
        );
    }
}
