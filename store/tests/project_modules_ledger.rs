//! The per-matter capability ledger: presence is the enabled state and
//! every change appends to the Relationship Log.
//!
//! Single-engine since wave four (ENG-120). The ledger row and its audit
//! entry used to land in different engines with no transaction over the
//! pair, so a toggle could be recorded without a trail or the reverse.

use store::project_modules::{disable, enable, is_enabled, list_for_project, Module};
use store::projects::{self, NewProject};
use store::relationship_logs;
use store::surreal::{record_id, SurrealDb};
use store::test_support::{dri_person, mem_surreal, seed_entity};
use uuid::Uuid;

async fn open_matter(surreal: &SurrealDb, code: &str) -> Uuid {
    projects::create(
        surreal,
        &NewProject {
            code: code.to_string(),
            name: code.to_string(),
            status: "open".to_string(),
            entity_id: seed_entity(surreal).await,
            ..Default::default()
        },
    )
    .await
    .expect("insert matter")
    .id
}

async fn toggle_log(surreal: &SurrealDb, project_id: Uuid) -> Vec<(String, String)> {
    let mut entries: Vec<(String, String)> =
        relationship_logs::for_subject(surreal, "project", project_id)
            .await
            .expect("logs")
            .into_iter()
            .filter(|log| log.action.starts_with("module_"))
            .map(|log| (log.action, log.detail))
            .collect();
    // `for_subject` reads newest-first; the ledger's assertions read as a
    // history, so put them back in the order they happened.
    entries.reverse();
    entries
}

#[tokio::test]
async fn one_matter_runs_several_modules_at_once() {
    let surreal = mem_surreal().await;
    let project_id = open_matter(&surreal, "multi").await;
    let actor = dri_person(&surreal).await;

    enable(&surreal, project_id, Module::Litigation, Some(actor))
        .await
        .expect("litigation");
    enable(&surreal, project_id, Module::CapTable, Some(actor))
        .await
        .expect("cap table");

    let enabled = list_for_project(&surreal, project_id).await.expect("list");
    assert_eq!(enabled, vec![Module::CapTable, Module::Litigation]);
}

#[tokio::test]
async fn enable_disable_list_round_trips_without_a_tombstone() {
    let surreal = mem_surreal().await;
    let project_id = open_matter(&surreal, "round").await;
    let actor = dri_person(&surreal).await;

    assert!(!is_enabled(&surreal, project_id, Module::Estate)
        .await
        .expect("q"));
    enable(&surreal, project_id, Module::Estate, Some(actor))
        .await
        .expect("enable");
    assert!(is_enabled(&surreal, project_id, Module::Estate)
        .await
        .expect("q"));
    assert!(disable(&surreal, project_id, Module::Estate, Some(actor))
        .await
        .expect("disable"));
    assert!(!is_enabled(&surreal, project_id, Module::Estate)
        .await
        .expect("q"));
    assert!(list_for_project(&surreal, project_id)
        .await
        .expect("list")
        .is_empty());
}

#[tokio::test]
async fn a_disabled_module_leaves_nothing_to_leak() {
    let surreal = mem_surreal().await;
    let project_id = open_matter(&surreal, "blind").await;

    enable(&surreal, project_id, Module::Litigation, None)
        .await
        .expect("on");
    enable(&surreal, project_id, Module::Deadlines, None)
        .await
        .expect("on");
    disable(&surreal, project_id, Module::Deadlines, None)
        .await
        .expect("off");

    assert_eq!(
        list_for_project(&surreal, project_id).await.expect("list"),
        vec![Module::Litigation]
    );
}

#[tokio::test]
async fn every_effective_toggle_writes_one_audit_entry() {
    let surreal = mem_surreal().await;
    let project_id = open_matter(&surreal, "audit").await;

    let first = enable(&surreal, project_id, Module::CapTable, None)
        .await
        .expect("on");
    let second = enable(&surreal, project_id, Module::CapTable, None)
        .await
        .expect("idempotent on");
    assert_eq!(first.id, second.id);
    disable(&surreal, project_id, Module::CapTable, None)
        .await
        .expect("off");
    assert!(!disable(&surreal, project_id, Module::CapTable, None)
        .await
        .expect("idempotent off"));
    assert_eq!(
        toggle_log(&surreal, project_id).await,
        vec![
            ("module_enabled".to_string(), "cap_table".to_string()),
            ("module_disabled".to_string(), "cap_table".to_string()),
        ]
    );
}

#[tokio::test]
async fn the_schema_refuses_an_unrecognized_module() {
    let surreal = mem_surreal().await;
    let project_id = open_matter(&surreal, "unrecognized-module").await;

    let error = surreal
        .query(
            "CREATE $id SET project_id = $project_id, module = 'contract_review', \
             enabled_at = $now, enabled_by_person_id = NONE, inserted_at = $now, updated_at = $now",
        )
        .bind(("id", record_id("project_module", Uuid::now_v7())))
        .bind(("project_id", record_id("project", project_id)))
        .bind(("now", chrono::Utc::now().to_rfc3339()))
        .await
        .and_then(surrealdb::IndexedResults::check)
        .expect_err("a module outside the closed set is not storable");
    let error = error.to_string();
    assert!(
        error.contains("module") && error.contains("contract_review"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn modules_are_scoped_to_one_matter() {
    let surreal = mem_surreal().await;
    let a = open_matter(&surreal, "matter-a").await;
    let b = open_matter(&surreal, "matter-b").await;

    enable(&surreal, a, Module::Litigation, None)
        .await
        .expect("on");
    assert_eq!(
        list_for_project(&surreal, a).await.expect("a"),
        vec![Module::Litigation]
    );
    assert!(list_for_project(&surreal, b).await.expect("b").is_empty());
}
