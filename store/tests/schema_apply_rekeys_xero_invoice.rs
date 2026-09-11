//! ENG-588: `xero_invoice` re-keys from one row per Project (record id =
//! `project_id`) to one row per raised Xero invoice (record id =
//! `xero_invoice_id`). The schema file itself carries the migration — a
//! `LET`-bound `SELECT` feeding a `FOR` loop that re-creates any row still
//! living under its legacy `project_id`-shaped id and drops the original —
//! so this proves a database holding an old-shape row survives re-applying
//! the schema and reads back correctly under its new key.

use store::schema;
use store::test_support::{mem_surreal, seed_entity};
use store::xero_invoices;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn a_legacy_project_keyed_row_is_rekeyed_to_its_xero_invoice_id_and_stays_readable() {
    let db = mem_surreal().await;
    let entity_id = seed_entity(&db).await;
    let project = store::projects::create(
        &db,
        &store::projects::NewProject {
            code: "legacy-invoice-matter".to_string(),
            name: "Legacy Invoice Matter".to_string(),
            status: "open".to_string(),
            entity_id,
            ..Default::default()
        },
    )
    .await
    .expect("create the matter the legacy mirror row bills");

    // `issued_at` predates ENG-588 as nonexistent, so reproduce that shape by
    // loosening the field back to optional before writing the historical row
    // — matching how the field is genuinely absent on a row this old, rather
    // than defining a value the migration's own backfill is supposed to
    // supply.
    db.query("DEFINE FIELD OVERWRITE issued_at ON xero_invoice TYPE option<datetime>")
        .await
        .and_then(surrealdb::IndexedResults::check)
        .expect("reproduce issued_at's pre-ENG-588 absence");

    // The faithful reproduction of a pre-ENG-588 row: created directly under
    // a record id equal to its own `project_id` — both minted the same way
    // `store::xero_invoices::create` used to (a native UUID key, not a
    // string that merely spells one, per `store::surreal::record_id`'s own
    // docs on the two being different records) — exactly what that function
    // wrote before the mirror was re-keyed on the Xero invoice id.
    db.query(
        "CREATE $id SET \
         project_id = $project_id, \
         xero_invoice_id = 'legacy-xero-id', reference = 'Matter legacy', \
         status = 'AUTHORISED', amount_cents = 250000, amount_paid_cents = 0, \
         currency = 'USD', inserted_at = <datetime>'2026-01-15T00:00:00Z', \
         updated_at = <datetime>'2026-01-15T00:00:00Z'",
    )
    .bind(("id", store::surreal::record_id("xero_invoice", project.id)))
    .bind((
        "project_id",
        store::surreal::record_id("project", project.id),
    ))
    .await
    .and_then(surrealdb::IndexedResults::check)
    .expect("create a legacy project_id-keyed mirror row");

    // Confirm the row genuinely predates the re-key, before re-applying the
    // schema — otherwise this proves nothing about the migration itself.
    let legacy_still_project_keyed: Option<bool> = db
        .query("SELECT VALUE meta::id(id) = meta::id(project_id) FROM ONLY xero_invoice WHERE xero_invoice_id = 'legacy-xero-id' LIMIT 1")
        .await
        .and_then(surrealdb::IndexedResults::check)
        .unwrap()
        .take(0)
        .unwrap();
    assert_eq!(
        legacy_still_project_keyed,
        Some(true),
        "the row must start keyed on its project_id"
    );

    // Re-applying the schema is the same idempotent step every process runs
    // on every boot — this is what runs the re-key migration.
    schema::apply(&db)
        .await
        .expect("re-apply the schema over the historical row");

    let rows = xero_invoices::for_projects(&db, &[project.id])
        .await
        .expect("read the mirror after the re-key");
    assert_eq!(
        rows.len(),
        1,
        "the re-key must not duplicate or drop the row"
    );
    let row = &rows[0];
    assert_eq!(row.xero_invoice_id, "legacy-xero-id");
    assert_eq!(row.project_id, project.id);
    assert_eq!(row.amount_cents, 250_000, "amounts survive the re-key");
    assert_eq!(
        row.issued_at.to_rfc3339(),
        "2026-01-15T00:00:00+00:00",
        "issued_at is backfilled from the legacy row's inserted_at"
    );

    // The row must stay keyed on the Xero invoice id, not merely readable by
    // project — otherwise a second invoice on the same matter would still
    // collide the way it did before ENG-588.
    let now_xero_keyed: Option<bool> = db
        .query(
            "SELECT VALUE meta::id(id) = xero_invoice_id FROM ONLY xero_invoice \
             WHERE xero_invoice_id = 'legacy-xero-id' LIMIT 1",
        )
        .await
        .and_then(surrealdb::IndexedResults::check)
        .unwrap()
        .take(0)
        .unwrap();
    assert_eq!(now_xero_keyed, Some(true));

    // A second invoice for the same matter must now land as its own row,
    // proving the re-key actually freed the concurrency boundary.
    xero_invoices::upsert(
        &db,
        &xero_invoices::UpsertXeroInvoice {
            project_id: project.id,
            xero_invoice_id: "second-invoice".to_string(),
            reference: "Matter second".to_string(),
            status: "AUTHORISED".to_string(),
            amount_cents: 75_000,
            currency: "USD".to_string(),
            issued_at: chrono::Utc::now(),
            due_at: None,
        },
    )
    .await
    .expect("a second invoice on the same matter must succeed after the re-key");
    assert_eq!(
        xero_invoices::for_projects(&db, &[project.id])
            .await
            .unwrap()
            .len(),
        2
    );

    // Re-applying again must be a genuine no-op: nothing left to migrate.
    schema::apply(&db)
        .await
        .expect("a second re-apply must succeed with nothing left to migrate");
    assert_eq!(
        xero_invoices::for_projects(&db, &[project.id])
            .await
            .unwrap()
            .len(),
        2,
        "a repeat apply must not duplicate or drop either row"
    );
}
