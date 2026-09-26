//! One embedded SurrealDB per test.
//!
//! [`mem`] starts an engine inside the test process: no container, no
//! port, no shared server, and nothing to reclaim afterwards, because the
//! whole database is dropped with the test's own memory. Two tests cannot
//! collide, so this module carries no isolation machinery at all — no
//! labelled container to reuse, no schema names stamped with a creation
//! time, no age sweep.
//!
//! The one lane that needs a real server is a test that spawns the
//! `navigator` binary, because a subprocess cannot reach an in-process
//! engine; [`crate::test_support::server_surreal`] owns that.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::surreal::{connect, SurrealConfig, SurrealDb};

/// Namespace every embedded test engine selects. Matches the local
/// dependency tier's namespace so a statement is never written against
/// coordinates that only exist under test.
pub const TEST_NAMESPACE: &str = "navigator";

/// Distinguishes the databases handed out inside one process. Engines
/// are already separate address spaces, so this is for legibility in a
/// failure message, not isolation.
static NEXT_DATABASE: AtomicU64 = AtomicU64::new(0);

/// A private in-memory SurrealDB with the schema applied.
///
/// Every call gets its own engine, so tests stay parallel by
/// construction. The returned handle has namespace and database
/// selected, exactly as a deployment's connection does.
///
/// ```no_run
/// # async fn example() {
/// let db = store::surreal::test_support::mem().await;
/// db.query("CREATE person:alice SET name = 'Alice', email = 'alice@example.com'")
///     .await
///     .unwrap();
/// # }
/// ```
pub async fn mem() -> SurrealDb {
    let db = unmigrated().await;
    crate::schema::apply(&db)
        .await
        .expect("apply the Surreal schema to a fresh embedded engine");
    seed_compiled_brands(&db).await;
    db
}

/// A firm id no real `firm` row ever resolves to — mirrors the reasoning
/// behind [`crate::test_support::SEED_ENTITY_JURISDICTION_ID`]: a
/// `record<firm>` link is a type constraint, not a foreign key, so a raw
/// write naming this id satisfies the schema without minting a real Firm
/// (and the Entity and Admin DRI Person a real one would need), which would
/// otherwise pollute every other test's own listing or counting assertions
/// over those shared tables.
const COMPILED_BRAND_FIXTURE_FIRM_ID: uuid::Uuid =
    uuid::Uuid::from_u128(0x0199_0000_0000_7000_8000_0000_0000_0090);

/// Register every compiled house-brand key as a `brand` row — the same
/// state `store::seed::seed_practice` reaches in a real deployment before
/// any traffic ever arrives (ENG-587: `store::projects::create` and
/// `open_matter` validate `brand` against live rows, not a compiled closed
/// list, so a test engine that never reaches this state could not open a
/// matter under any compiled key, unlike a real deployment).
///
/// ENG-659 made `firm_id` required, but a raw write here — rather than the
/// authorized `store::brands::create` — is deliberate: this fixture must
/// stay a pure `brand`-table seed, exactly as it was before every row
/// needed a Firm. Going through `create` would mean minting a real Firm (and
/// the Entity and Person it needs), and every one of those rows would then
/// show up in any other test's own unscoped listing or count over the
/// `entity`/`person`/`firm` tables. [`COMPILED_BRAND_FIXTURE_FIRM_ID`] is
/// never a real Firm, so a test's own `practice()`-style Firm still attaches
/// any of these keys through `firm_brand` exactly as before — that relation
/// carries no constraint back to `brand.firm_id`.
///
/// A test that needs to observe an empty `brand` table uses [`unmigrated`]
/// directly instead of [`mem`].
async fn seed_compiled_brands(db: &SurrealDb) {
    let now = chrono::Utc::now().to_rfc3339();
    for key in crate::firms::CLOSED_BRAND_KEYS {
        // An explicit UUID id, minted exactly like every other writer in
        // this crate (`record_id(TABLE, Uuid::now_v7())`) rather than
        // SurrealDB's own auto-generated key: `store::surreal::record_uuid`
        // parses a record's id as a UUID, and every reader of this table —
        // `store::brands::find_by_key` included — silently drops a row
        // whose id does not parse, which would make this fixture invisible
        // to the very code it exists to satisfy.
        db.query(
            "CREATE $id SET name = $name, brand_key = $key, firm_id = $firm_id, \
             inserted_at = $now, updated_at = $now",
        )
        .bind((
            "id",
            crate::surreal::record_id("brand", uuid::Uuid::now_v7()),
        ))
        .bind(("name", (*key).to_string()))
        .bind(("key", (*key).to_string()))
        .bind((
            "firm_id",
            crate::surreal::record_id("firm", COMPILED_BRAND_FIXTURE_FIRM_ID),
        ))
        .bind(("now", now.clone()))
        .await
        .and_then(surrealdb::IndexedResults::check)
        .unwrap_or_else(|error| panic!("seed the compiled brand {key} for a test engine: {error}"));
    }
}

/// [`mem`] without the schema — for tests that exercise the schema
/// apply itself and need to observe an empty database first.
pub async fn unmigrated() -> SurrealDb {
    let database = format!("test_{}", NEXT_DATABASE.fetch_add(1, Ordering::Relaxed));
    connect(&SurrealConfig {
        // Named, never defaulted: `store::surreal` refuses to guess an
        // endpoint, and a test is just another caller that says which
        // engine it means.
        endpoint: "mem://".into(),
        namespace: TEST_NAMESPACE.into(),
        database,
        auth: crate::surreal::SurrealAuth::Anonymous,
    })
    .await
    .expect("start an embedded SurrealDB engine")
}

/// A handle no engine answers, for exercising a caller's error branch.
///
/// There is no close on a Surreal handle, so a handle that was never
/// connected stands in for one that stopped working: every
/// statement fails at `Router::extract` with `Connection uninitialised`,
/// before anything is sent. What the caller does with the error is the
/// subject under test; which failure produced it is not.
#[must_use]
pub fn unreachable() -> SurrealDb {
    SurrealDb::uninitialized()
}

#[cfg(test)]
mod tests {
    use super::{mem, unmigrated, unreachable};
    use crate::schema::{self, SchemaState};

    #[tokio::test]
    async fn each_engine_is_private_to_its_caller() {
        let first = mem().await;
        let second = mem().await;

        first
            .query("CREATE person:alice SET name = 'Alice', email = 'alice@example.com'")
            .await
            .unwrap()
            .check()
            .unwrap();

        let leaked: Option<String> = second
            .query("SELECT VALUE name FROM person:alice")
            .await
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(leaked, None, "a second engine saw the first engine's row");
    }

    #[tokio::test]
    async fn an_unreachable_handle_fails_every_statement() {
        let error = unreachable()
            .query("SELECT * FROM person")
            .await
            .expect_err("a handle with no engine behind it cannot answer");
        assert!(
            error.to_string().contains("uninitialised"),
            "an unreachable handle must fail as a connection error; got: {error}",
        );
    }

    #[tokio::test]
    async fn mem_arrives_with_the_schema_applied_and_unmigrated_does_not() {
        assert_eq!(
            schema::state(&mem().await).await.unwrap(),
            SchemaState::InSync
        );
        assert_eq!(
            schema::state(&unmigrated().await).await.unwrap(),
            SchemaState::Absent
        );
    }
}
