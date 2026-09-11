//! ENG-587: `project.brand` became a required `string` (from `option<string>`
//! with a closed `ASSERT`), backfilled by a guarded idempotent `UPDATE` after
//! the schema definitions restore the tightened `DEFINE FIELD`. This proves a row
//! written with no `brand` value survives re-applying the schema — the same
//! "apply is run on every boot" guarantee every deployment relies on — and
//! that the row is still writable afterwards.

use store::projects::{self, UpdateProjectCommand};
use store::schema;
use store::surreal::test_support::unmigrated;
use store::test_support::{mem_surreal, seed_entity};

#[tokio::test]
async fn a_project_with_no_brand_value_is_backfilled_and_stays_writable() {
    let db = mem_surreal().await;
    let entity_id = seed_entity(&db).await;

    // `brand` predates ENG-587 as `option<string>`; reproduce that
    // permissive definition first so the row below can be written with no
    // `brand` value at all, exactly what a Project opened before this field
    // was required left behind. `store::projects::create`/`open_matter`
    // always bind a `brand` value explicitly, so a historical row can only
    // be reproduced with a query that bypasses them. Leaving the field fully
    // undefined instead (`REMOVE FIELD`) would also let this `CREATE`
    // succeed, but then the backfill `UPDATE` below — which runs before the
    // schema's tightened `DEFINE FIELD`, against a `SCHEMAFULL` table — would
    // itself be refused as referencing a field that does not exist, which
    // does not reproduce the real historical shape of this field.
    db.query("DEFINE FIELD OVERWRITE brand ON project TYPE option<string>")
        .await
        .and_then(surrealdb::IndexedResults::check)
        .expect("reproduce brand's pre-ENG-587 permissive definition");
    let now = chrono::Utc::now().to_rfc3339();
    let project_id = uuid::Uuid::now_v7();
    db.query(
        "CREATE $id SET code = 'brand-backfill-test', name = 'Backfill Test', \
         status = 'open', entity_id = $entity_id, inserted_at = $now, updated_at = $now",
    )
    .bind(("id", store::surreal::record_id("project", project_id)))
    .bind(("entity_id", store::surreal::record_id("entity", entity_id)))
    .bind(("now", now))
    .await
    .and_then(surrealdb::IndexedResults::check)
    .expect("create a project row with no brand value");

    // Confirm the row genuinely has no brand yet, before the backfill runs
    // again — otherwise this proves nothing about the backfill itself.
    let before: Option<String> = db
        .query("SELECT VALUE brand FROM ONLY project WHERE code = 'brand-backfill-test' LIMIT 1")
        .await
        .and_then(surrealdb::IndexedResults::check)
        .unwrap()
        .take(0)
        .unwrap();
    assert_eq!(before, None, "the row must start with no brand value");

    // Re-applying the schema is the same idempotent step every process runs
    // on every boot — this is what runs the backfill `UPDATE` a second time,
    // now that this row exists to backfill.
    schema::apply(&db)
        .await
        .expect("re-apply the schema over the historical row");

    let project = projects::find_by_code(&db, "brand-backfill-test")
        .await
        .unwrap()
        .expect("the backfilled project must still be found by its code");
    assert_eq!(
        project.brand, "neon",
        "the backfill must set the deployment default"
    );

    // The row must stay writable afterwards: a partial update touching a
    // field unrelated to `brand` must not hit a coercion error against it.
    let renamed = projects::update_project(
        &db,
        project.id,
        &UpdateProjectCommand {
            name: Some("Backfill Test Renamed".to_string()),
            ..Default::default()
        },
    )
    .await
    .expect("an unrelated partial update must succeed against the backfilled row");
    assert_eq!(renamed.name, "Backfill Test Renamed");
    assert_eq!(renamed.brand, "neon");
    let project = schema::introspect(&db)
        .await
        .expect("read the schema after a successful backfill")
        .remove("project")
        .expect("the project table remains defined");
    let brand_field = project
        .fields
        .get("brand")
        .expect("the brand field remains defined after a successful backfill");
    assert!(
        brand_field.contains("TYPE string") && !brand_field.contains("none | string"),
        "a successful migration must restore the required brand field: {brand_field}"
    );
}

/// A default only becomes safe once its corresponding brand row is live.
/// An upgrade with historical Projects but no neon row needs an explicit
/// operator repair, not a silent reference to a brand that does not exist.
#[tokio::test]
async fn a_missing_default_brand_is_reported_without_backfilling_historical_projects() {
    let db = unmigrated().await;
    schema::apply(&db)
        .await
        .expect("a fresh database with no projects still applies");
    db.query("DEFINE FIELD OVERWRITE brand ON project TYPE option<string>")
        .await
        .and_then(surrealdb::IndexedResults::check)
        .expect("reproduce the historical optional brand field");
    let entity_id = uuid::Uuid::now_v7();
    db.query(
        "CREATE $id SET code = 'missing-default-brand', name = 'Missing default brand',
         status = 'open', entity_id = $entity_id,
         inserted_at = '2026-01-15T00:00:00Z', updated_at = '2026-01-15T00:00:00Z'",
    )
    .bind((
        "id",
        store::surreal::record_id("project", uuid::Uuid::now_v7()),
    ))
    .bind(("entity_id", store::surreal::record_id("entity", entity_id)))
    .await
    .and_then(surrealdb::IndexedResults::check)
    .expect("create a pre-brand project");

    let error = schema::apply(&db)
        .await
        .expect_err("a missing live default brand must stop the backfill");
    assert!(
        error
            .to_string()
            .contains("project brand backfill missing default brand: neon"),
        "the migration error must name the missing default: {error}"
    );
    let brand: Option<String> = db
        .query("SELECT VALUE brand FROM ONLY project WHERE code = 'missing-default-brand'")
        .await
        .and_then(surrealdb::IndexedResults::check)
        .expect("read the untouched historical project")
        .take(0)
        .expect("deserialize the absent brand");
    assert_eq!(
        brand, None,
        "a failed migration must not write a dangling default"
    );
    let project = schema::introspect(&db)
        .await
        .expect("read the schema left for an explicit operator repair")
        .remove("project")
        .expect("the project table remains defined");
    let brand_field = project
        .fields
        .get("brand")
        .expect("the brand field remains defined after a failed migration");
    assert!(
        brand_field.contains("none | string"),
        "a failed migration must leave brand optional until its default exists: {brand_field}"
    );
}

/// A non-null legacy value can still point nowhere. It must be reported as
/// data to repair, never overwritten with the deployment default.
#[tokio::test]
async fn a_dangling_legacy_brand_is_reported_without_reassigning_the_project() {
    let db = unmigrated().await;
    schema::apply(&db)
        .await
        .expect("a fresh database with no projects still applies");
    let entity_id = uuid::Uuid::now_v7();
    db.query(
        "CREATE $id SET code = 'dangling-brand', name = 'Dangling brand',
         status = 'open', brand = 'retired-brand', entity_id = $entity_id,
         inserted_at = '2026-01-15T00:00:00Z', updated_at = '2026-01-15T00:00:00Z'",
    )
    .bind((
        "id",
        store::surreal::record_id("project", uuid::Uuid::now_v7()),
    ))
    .bind(("entity_id", store::surreal::record_id("entity", entity_id)))
    .await
    .and_then(surrealdb::IndexedResults::check)
    .expect("create a project carrying a retired brand key");

    let error = schema::apply(&db)
        .await
        .expect_err("a dangling legacy key must stop the migration");
    assert!(
        error
            .to_string()
            .contains("project brand backfill dangling brand: retired-brand"),
        "the migration error must name the dangling key: {error}"
    );
    let brand: Option<String> = db
        .query("SELECT VALUE brand FROM ONLY project WHERE code = 'dangling-brand'")
        .await
        .and_then(surrealdb::IndexedResults::check)
        .expect("read the untouched legacy project")
        .take(0)
        .expect("deserialize the legacy brand");
    assert_eq!(brand.as_deref(), Some("retired-brand"));
}
