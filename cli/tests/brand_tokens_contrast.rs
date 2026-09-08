//! Each catalogued palette must clear WCAG AA. The tokens stylesheet is
//! generated from `views::brand_presentation`, so this pins the catalog rather
//! than a static CSS file.

#[test]
fn every_catalogued_palette_clears_wcag_aa() {
    for palette in views::brand::PALETTE {
        for (scheme, dark) in [(&palette.light, false), (&palette.dark, true)] {
            let primary = views::brand_presentation::parse_hex(scheme.primary)
                .unwrap_or_else(|| panic!("{} primary", palette.id));
            let on_primary = views::brand_presentation::parse_hex(scheme.on_primary)
                .unwrap_or_else(|| panic!("{} on-primary", palette.id));
            let bg = views::brand_presentation::scheme_bg(scheme, dark);
            let ratio = views::brand_presentation::contrast_ratio(primary, bg);
            assert!(
                ratio >= 4.5,
                "{} primary on surface is {ratio:.2}:1",
                palette.id
            );
            let fill = views::brand_presentation::contrast_ratio(on_primary, primary);
            assert!(
                fill >= 4.5,
                "{} on-primary on primary is {fill:.2}:1",
                palette.id
            );
        }
    }
}

#[test]
fn contrast_ratio_matches_known_wcag_reference_pairs() {
    let white = [0xff, 0xff, 0xff];
    assert!((views::brand_presentation::contrast_ratio([0, 0, 0], white) - 21.0).abs() < 0.01);
    assert!((views::brand_presentation::contrast_ratio(white, white) - 1.0).abs() < 0.01);
}
