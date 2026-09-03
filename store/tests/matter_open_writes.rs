//! What opening a matter writes, and nothing more.
//!
//! `open_matter` commits its rows in one explicit `SurrealDB` transaction, so
//! the set of statements in that transaction *is* the contract. Pinning it
//! here means a future statement — another journal, another projection —
//! cannot be added to the open path without this test naming it.

use store::persons::{self, NewPerson, Role};
use store::projects::{self, OpenMatterCommand};
use store::surreal::SurrealDb;
use store::test_support::{mem_surreal, seed_entity};
use uuid::Uuid;

/// Every reference `open_matter` validates, seeded: the client, the
/// attesting attorney, and the entity.
async fn references(surreal: &SurrealDb) -> (Uuid, Uuid, Uuid) {
    let client = persons::create(
        surreal,
        &NewPerson::with_role("Libra Client", "libra@example.com", Role::Client),
    )
    .await
    .expect("client");
    let attester = persons::create(
        surreal,
        &NewPerson::with_role("Virgo Attorney", "virgo@example.com", Role::Lawyer),
    )
    .await
    .expect("attester");
    let entity_id = seed_entity(surreal).await;
    (client.id, attester.id, entity_id)
}

/// Whether the applied schema defines `table` at all.
async fn table_is_defined(surreal: &SurrealDb, table: &str) -> bool {
    let mut response = surreal
        .query("INFO FOR DB")
        .await
        .expect("query")
        .check()
        .expect("check");
    let info: surrealdb::types::Value = response.take(0).expect("db info");
    // `INFO FOR DB` renders each table as its own `DEFINE TABLE` statement,
    // so the definition text is the presence check.
    format!("{info:?}").contains(&format!("DEFINE TABLE {table} "))
}

/// Opening a matter writes the project, both participations, and the
/// conflict attestation — and nothing else. The price journal that used to
/// ride along in this transaction is gone: the firm prices each matter
/// bespoke and the Xero invoice is the record of what was billed.
#[tokio::test]
async fn opening_a_matter_writes_the_project_two_participations_and_the_attestation() {
    let surreal = mem_surreal().await;
    let (client_id, acting_person_id, entity_id) = references(&surreal).await;

    let project = projects::open_matter(
        &surreal,
        &OpenMatterCommand {
            name: "LLC Formation".into(),
            code: "llc-formation".into(),
            client_id,
            entity_id,
            description: Some("Delaware LLC formation.".into()),
            brand: "neon".to_string(),
            attestation: true,
            acting_person_id,
        },
    )
    .await
    .expect("open the matter");

    // The stored code is the code the caller supplied, verbatim — see
    // `project_code_storage.rs` for the storage contract itself.
    assert_eq!(project.code, "llc-formation");

    // Both participations: the attesting attorney as lawyer DRI, the client
    // as client DRI.
    let participations = projects::participations_for_project(&surreal, project.id)
        .await
        .expect("participations");
    assert_eq!(
        participations.len(),
        2,
        "the attorney and the client, and no one else: {participations:?}"
    );

    // The attestation audit row — the record the firm's cleared-conflict
    // discipline rests on.
    let logs = store::relationship_logs::for_subject(&surreal, "project", project.id)
        .await
        .expect("logs");
    assert_eq!(
        logs.iter()
            .filter(|log| log.action == "conflict_attestation")
            .count(),
        1,
        "exactly one attestation: {logs:?}"
    );

    // Nothing else. The price journal is the statement this transaction no
    // longer carries, and the schema no longer defines the table it wrote
    // to — the firm prices each matter bespoke and the Xero invoice is the
    // record of what was billed.
    assert!(
        !table_is_defined(&surreal, "project_price_event").await,
        "the price journal is gone from the schema, not merely unwritten"
    );
    assert!(
        table_is_defined(&surreal, "project").await,
        "the control: INFO FOR DB really does name the tables that exist"
    );
}
