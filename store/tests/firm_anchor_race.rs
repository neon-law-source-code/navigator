//! The firm anchor under concurrency (ENG-272).
//!
//! The invariant is that exactly one Entity row may hold
//! `firm_anchor_key`. The UNIQUE `entity_firm_anchor` index reads like
//! what enforces it and does not: racers writing distinct entity rows
//! collide on no shared record key, so the engine's optimistic layer has
//! nothing to conflict on and admits a second anchor. That is reproduced
//! below against a deliberately unguarded write, so the reason the
//! `firm_anchor` claim table exists cannot be refactored away as
//! redundant.
//!
//! The guarded path is then raced the same way and must land exactly one
//! row. These race outward from a store-level API on purpose: the HTTP
//! surface adds a redirect heuristic between the write and the assertion,
//! which cannot tell a refused create from one that failed for another
//! reason.

use std::sync::Arc;
use store::entities::{self, EntityError, NewEntity};
use store::surreal::{record_id, SurrealDb};
use uuid::Uuid;

/// Enough racers to overlap, few enough to stay quick under a loaded CI
/// box. The fork this guards against reproduces from two.
const RACERS: usize = 8;

/// The unguarded write the claim replaced: an `UPSERT` under a fresh id,
/// leaning on the UNIQUE index alone. Kept here as the control — it is
/// what the schema used to claim was sufficient.
async fn unguarded_create(db: &SurrealDb, key: &str) -> Result<(), surrealdb::Error> {
    db.query(
        "UPSERT $id SET name = $name, entity_type_id = $t, jurisdiction_id = $j, \
         firm_anchor_key = $key, inserted_at = time::now(), updated_at = time::now()",
    )
    .bind(("id", record_id("entity", Uuid::now_v7())))
    .bind(("name", "Shook Law PLLC".to_string()))
    .bind(("key", key.to_string()))
    .bind((
        "t",
        record_id("entity_type", store::test_support::SEED_ENTITY_TYPE_ID),
    ))
    .bind((
        "j",
        record_id(
            "jurisdiction",
            store::test_support::SEED_ENTITY_JURISDICTION_ID,
        ),
    ))
    .await
    .and_then(surrealdb::IndexedResults::check)
    .map(|_| ())
}

fn anchor_input(key: &str) -> NewEntity {
    NewEntity {
        name: "Shook Law PLLC".to_string(),
        entity_type_id: store::test_support::SEED_ENTITY_TYPE_ID,
        jurisdiction_id: store::test_support::SEED_ENTITY_JURISDICTION_ID,
        phone: None,
        url: None,
        firm_anchor_key: Some(key.to_string()),
    }
}

async fn anchor_rows(db: &SurrealDb) -> usize {
    entities::all(db)
        .await
        .expect("read the entities back")
        .into_iter()
        .filter(entities::Entity::is_firm_anchor)
        .count()
}

/// The whole create door, raced — `entity_commands::create_entity`, which
/// is what the `/app/admin/entities` form and `POST /app/api/entities` both
/// call.
///
/// This one is not redundant with the store-level race below it. The fork
/// survived a first fix that claimed the anchor *inside* the transaction
/// carrying the entity write: the extra reads this door performs widen the
/// transaction, and a claim read from the snapshot taken at `BEGIN` sees
/// the anchor free for every racer. Racing the narrow write alone did not
/// reproduce that; racing this door did.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_creates_through_the_command_door_land_exactly_one_anchor() {
    for round in 0..12 {
        let db = Arc::new(store::test_support::mem_surreal().await);
        let jurisdiction = store::jurisdictions::create(
            &db,
            &store::jurisdictions::NewJurisdiction::new("Nevada", "NV", "state"),
        )
        .await
        .expect("seed the jurisdiction the command reads back");
        let entity_type = store::entity_types::create(&db, "PLLC")
            .await
            .expect("seed the entity type the command reads back");

        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..RACERS {
            let db = Arc::clone(&db);
            let (entity_type_id, jurisdiction_id) = (entity_type.id, jurisdiction.id);
            tasks.spawn(async move {
                store::entity_commands::create_entity(
                    &db,
                    "Shook Law PLLC",
                    &store::entity_commands::CreateEntityCommand {
                        name: "Shook Law PLLC".to_string(),
                        entity_type_id,
                        jurisdiction_id,
                    },
                )
                .await
                .map(|_| ())
            });
        }

        let (mut created, mut refused) = (0, 0);
        while let Some(outcome) = tasks.join_next().await {
            match outcome.expect("a racer must not panic") {
                Ok(()) => created += 1,
                Err(store::entity_commands::EntityCommandError::FirmAnchorExists) => refused += 1,
                Err(other) => panic!("round {round}: a racer failed unrecognisably: {other:?}"),
            }
        }

        assert_eq!(created, 1, "round {round}: exactly one racer may mint");
        assert_eq!(refused, RACERS - 1, "round {round}: the rest are refused");
        assert_eq!(
            anchor_rows(&db).await,
            1,
            "round {round}: the anchor must not fork",
        );
    }
}

/// The narrow write, raced: exactly one racer creates the anchor, every
/// other is refused as [`EntityError::FirmAnchorTaken`], and exactly one
/// row carries the key when the dust settles.
///
/// No racer may come back as [`EntityError::Db`]. That is the outcome the
/// HTTP-level version of this test could not see — a create that failed
/// for an unrecognised reason is not a refusal, and counting it as one is
/// how a broken guard looks green.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_creates_land_exactly_one_anchor() {
    let key = "shook law pllc";
    for round in 0..8 {
        let db = Arc::new(store::test_support::mem_surreal().await);
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..RACERS {
            let db = Arc::clone(&db);
            tasks.spawn(async move { entities::create(&db, &anchor_input(key)).await });
        }

        let (mut created, mut refused) = (0, 0);
        // ENG-312: which rows won, so a fork can be correlated with the
        // `NAV_ANCHOR_TRACE` lines naming the branch each racer took.
        let mut minted = Vec::new();
        while let Some(outcome) = tasks.join_next().await {
            match outcome.expect("a racer must not panic") {
                Ok(entity) => {
                    created += 1;
                    minted.push(entity.id);
                }
                Err(EntityError::FirmAnchorTaken) => refused += 1,
                Err(other) => panic!("round {round}: a racer failed unrecognisably: {other}"),
            }
        }

        if created != 1 {
            eprintln!("ANCHOR FORK round={round} minted={minted:?}");
            eprintln!(
                "ANCHOR FORK round={round} claim_holder={:?}",
                entities::firm_anchor_holder(&db, key).await,
            );
        }
        assert_eq!(created, 1, "round {round}: exactly one racer may mint");
        assert_eq!(refused, RACERS - 1, "round {round}: the rest are refused");
        assert_eq!(
            anchor_rows(&db).await,
            1,
            "round {round}: the anchor must not fork",
        );
    }
}

/// The control, and the reason the claim table exists.
///
/// Racing the *unguarded* write is allowed to fork — the assertion is not
/// that it does (it needs a loaded machine to lose reliably), but that
/// nothing about the unguarded shape refuses the second row on its own
/// merits. If this ever starts refusing every fork, the engine gained
/// concurrent UNIQUE-index enforcement and the claim can be reconsidered.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_unique_index_alone_does_not_serialize_racers() {
    let db = Arc::new(store::test_support::mem_surreal().await);
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..RACERS {
        let db = Arc::clone(&db);
        tasks.spawn(async move { unguarded_create(&db, "shook law pllc").await });
    }
    let mut landed = 0;
    while let Some(outcome) = tasks.join_next().await {
        if outcome.expect("a racer must not panic").is_ok() {
            landed += 1;
        }
    }
    assert!(landed >= 1, "at least one unguarded write must land");
    assert_eq!(
        anchor_rows(&db).await,
        landed,
        "every unguarded write that reported success must be a row — if this \
         fails the index is silently discarding writes, which is a different \
         defect again",
    );
}

/// A second entity may not take a claim its holder still owns, and the
/// refusal is the typed one — not a `Db` fault wearing a redirect.
#[tokio::test]
async fn a_second_entity_is_refused_the_held_anchor() {
    let db = store::test_support::mem_surreal().await;
    let key = "shook law pllc";
    let first = entities::create(&db, &anchor_input(key))
        .await
        .expect("mint");

    let second = entities::create(&db, &anchor_input(key)).await;
    assert!(
        matches!(second, Err(EntityError::FirmAnchorTaken)),
        "a second anchor is refused by name, got {second:?}",
    );
    assert_eq!(
        entities::firm_anchor_holder(&db, key).await.unwrap(),
        Some(first.id),
        "the original holder keeps the claim",
    );
    assert_eq!(anchor_rows(&db).await, 1);
}

/// Re-writing the anchor row it already holds must not self-refuse: a
/// re-run seed and an operator correcting the anchor's entity type both
/// take this path.
#[tokio::test]
async fn the_holder_may_rewrite_its_own_anchor() {
    let db = store::test_support::mem_surreal().await;
    let key = "shook law pllc";
    let anchor = entities::create(&db, &anchor_input(key))
        .await
        .expect("mint");

    entities::upsert_with_id(&db, anchor.id, &anchor_input(key))
        .await
        .expect("a re-run seed reconciles its own row");
    let updated = entities::update(&db, anchor.id, &anchor_input(key))
        .await
        .expect("an in-place edit of the anchor is allowed")
        .expect("the row is written");

    assert_eq!(updated.id, anchor.id);
    assert_eq!(
        entities::firm_anchor_holder(&db, key).await.unwrap(),
        Some(anchor.id),
    );
    assert_eq!(anchor_rows(&db).await, 1);
}

/// Surrendering the key surrenders the claim, so the anchor can be minted
/// again. Clearing only the column would shut the white-label window for
/// good — the next mint would collide with a claim nothing owns.
#[tokio::test]
async fn releasing_the_key_frees_the_claim_for_the_next_holder() {
    let db = store::test_support::mem_surreal().await;
    let key = "shook law pllc";
    let outgoing = entities::create(&db, &anchor_input(key))
        .await
        .expect("mint");

    entities::set_firm_anchor_key(&db, outgoing.id, None)
        .await
        .expect("release");
    assert_eq!(
        entities::firm_anchor_holder(&db, key).await.unwrap(),
        None,
        "the claim goes with the column",
    );

    let incoming = entities::create(&db, &anchor_input(key))
        .await
        .expect("the freed anchor may be minted again");
    assert_ne!(incoming.id, outgoing.id);
    assert_eq!(
        entities::firm_anchor_holder(&db, key).await.unwrap(),
        Some(incoming.id),
    );
    assert_eq!(anchor_rows(&db).await, 1);
}
