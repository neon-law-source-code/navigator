//! ENG-435's own floor: a house brand's primary colour must clear WCAG AA
//! (4.5:1) against `--nav-color-bg` in light mode. `navigator dev
//! browser-e2e`'s axe-core audit is the full runtime gate across every
//! rendered pairing; this pins the one number the issue names explicitly so a
//! future hex edit fails a fast, KIND-free `cargo nextest` before it ever
//! reaches that browser.

use std::fs;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("canonicalize workspace root")
}

/// Extract the hex value of a `--custom-property: #rrggbb;` declaration.
fn extract_hex(css: &str, property: &str) -> [u8; 3] {
    let needle = format!("{property}:");
    let start = css.find(&needle).unwrap_or_else(|| {
        panic!("`{property}` not declared in the stylesheet");
    });
    let after = &css[start + needle.len()..];
    let hash = after.find('#').expect("declaration is a hex colour");
    let hex = &after[hash + 1..hash + 7];
    let byte = |offset: usize| u8::from_str_radix(&hex[offset..offset + 2], 16).unwrap();
    [byte(0), byte(2), byte(4)]
}

/// WCAG 2.x relative luminance of one sRGB channel (0-255).
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

/// WCAG contrast ratio between two colours, always >= 1.0.
fn contrast_ratio(a: [u8; 3], b: [u8; 3]) -> f64 {
    let (l1, l2) = (relative_luminance(a), relative_luminance(b));
    let (lighter, darker) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
    (lighter + 0.05) / (darker + 0.05)
}

const WHITE: [u8; 3] = [0xff, 0xff, 0xff];

/// The floor ENG-435's own text states: "The 500 stop must clear 4.5:1 on
/// white." Reading the live stylesheet rather than a copied literal means an
/// edit to the actual shipped hex is what this test checks, not a constant
/// that could drift from it.
#[test]
fn delete_your_datas_primary_clears_the_accessible_contrast_floor() {
    let path = workspace_root().join("server/public/css/brand-delete-your-data-tokens.css");
    let css = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

    let primary = extract_hex(&css, "--nav-red-500");
    let ratio = contrast_ratio(primary, WHITE);

    assert!(
        ratio >= 4.5,
        "--nav-red-500 must clear WCAG AA (4.5:1) on white for normal text; got {ratio:.2}:1"
    );
}

#[test]
fn contrast_ratio_matches_known_wcag_reference_pairs() {
    // Black on white is the maximum possible ratio, 21:1.
    assert!((contrast_ratio([0, 0, 0], WHITE) - 21.0).abs() < 0.01);
    // A colour against itself is always 1:1.
    assert!((contrast_ratio(WHITE, WHITE) - 1.0).abs() < 0.01);
}
