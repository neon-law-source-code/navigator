//! ENG-587: `project.brand` became a required `string` (from `option<string>`
//! with a closed `ASSERT`), backfilled by an idempotent `UPDATE` the schema
//! file itself runs before the tightened `DEFINE FIELD`. This proves a row
//! written with no `brand` value survives re-applying the schema — the same
//! "apply is run on every boot" guarantee every deployment relies on — and
//! that the row is still writable afterwards.

use store::projects::{self, UpdateProjectCommand};
use store::schema;
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
}
