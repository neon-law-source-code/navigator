//! Seeded `brand` rows wear the compiled presentation catalog.
//!
//! `store` cannot depend on `views`, so [`store::seed`] copies each
//! compiled key's typeface id and light-mode primary hex into
//! `COMPILED_BRANDS`. This crate sees both halves. The wordmark copy is
//! held in `webapp::firm_footer`; this binary holds the two presentation
//! columns against [`views::brand::BrandKey::default_typeface`] and
//! [`views::brand::BrandKey::default_palette`].
//!
//! Starts from the schema alone. [`store::test_support::mem_surreal`]
//! pre-registers every closed key as a `brand` row with neither typeface
//! nor primary, which is the state *after* a boot — and `seed_brands` is
//! find-or-create, so those placeholder rows would hide a stale copy.

use std::sync::Arc;

use store::DeploymentEnvironment;
use views::brand::BrandKey;

async fn storage() -> Arc<dyn cloud::StorageService> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "navigator-brand-seed-presentation-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed),
    ));
    Arc::new(cloud::FsStorage::new(dir).await.unwrap())
}

#[tokio::test]
async fn every_compiled_brand_seeds_its_typeface_and_primary() {
    let surreal = store::surreal::test_support::unmigrated().await;
    store::schema::apply(&surreal).await.unwrap();
    let storage = storage().await;

    store::seed::seed_environment_with(
        &surreal,
        &storage,
        DeploymentEnvironment::Production,
        store::seed::BrandSeed::Neon,
    )
    .await
    .expect("a first boot seeds every compiled brand row");

    for key in BrandKey::ALL {
        let row = store::brands::find_by_key(&surreal, key.as_str())
            .await
            .unwrap()
            .unwrap_or_else(|| {
                panic!(
                    "{} is seeded as a brand row, including keys that are not yet live",
                    key.as_str()
                )
            });
        let typeface = row.typeface.as_deref().unwrap_or_else(|| {
            panic!("{} seeds a typeface id", key.as_str());
        });
        let primary = row.primary_color.as_deref().unwrap_or_else(|| {
            panic!("{} seeds a light-mode primary hex", key.as_str());
        });
        assert_eq!(
            typeface,
            key.default_typeface().id,
            "{} seeds BrandKey::default_typeface().id",
            key.as_str()
        );
        assert_eq!(
            primary,
            key.default_palette().light.primary,
            "{} seeds BrandKey::default_palette().light.primary",
            key.as_str()
        );
    }
}
