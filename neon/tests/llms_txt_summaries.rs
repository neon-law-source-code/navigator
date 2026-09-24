//! Every live house brand's `/llms.txt` must give a crawler more than its own
//! name: a plain-text understanding of what the practice actually does,
//! not just a title it can already read from the `Host:` header.

use views::brand::BrandKey;

#[tokio::test]
async fn every_live_brands_llms_txt_summarizes_what_the_practice_does() {
    let state = portal::test_support::app_state(store::test_support::mem_surreal().await).await;

    for key in BrandKey::LIVE {
        let document = neon::llms_txt(&state, *key);
        let mark = key
            .resolve_branding(&views::brand::DEFAULT_BRANDING)
            .firm
            .site_name;

        let summary = document.summary.trim();
        assert!(
            !summary.is_empty(),
            "{} publishes no llms.txt summary",
            key.as_str()
        );
        assert_ne!(
            summary,
            mark,
            "{} llms.txt summary is just its own name, not a description of what it does",
            key.as_str()
        );
        assert!(
            summary.split_whitespace().count() >= 8,
            "{} llms.txt summary is too short to explain what the practice does: {summary:?}",
            key.as_str()
        );

        let home = document
            .pages
            .first()
            .unwrap_or_else(|| panic!("{} llms.txt has no pages", key.as_str()));
        assert!(
            !home.description.trim().is_empty(),
            "{} llms.txt home entry has no description",
            key.as_str()
        );
    }
}
