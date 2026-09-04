//! The server-mode `SurrealDB` lane (#1093).
//!
//! # Why this exists beside the embedded tests
//!
//! `store::surreal`'s unit tests run against `mem://`, an engine inside
//! the test process. That covers the query surface but skips everything
//! that only exists when the engine is somewhere else: the WebSocket
//! protocol, `signin` with root credentials, and namespace/database
//! selection over a wire. Every deployment uses exactly that path — the
//! KIND dependency tier locally, Surreal Cloud in production — so it
//! needs a test that speaks it.
//!
//! # The env contract
//!
//! The contract (see `docs/test-database.md`):
//!
//! - **`NAVIGATOR_SURREAL_ENDPOINT` set** → run against that engine. CI
//!   starts one for this lane; locally it is the
//!   worktree's port-forwarded tier, already in `.devx/env`.
//! - **unset** → skip, so `cargo test` stays zero-config on a laptop
//!   with no tier running.
//!
//! `NAV_REQUIRE_SURREAL=1` turns the skip into a failure, which is what
//! keeps "skips when unconfigured" from quietly becoming "never runs":
//! CI sets it, so a broken engine there is a red build, not a silent
//! pass. This lane owns that flag rather than borrowing
//! `NAV_REQUIRE_HARNESS`, which arms the browser harness
//! (`features::webdriver`) and the Restate broker fixture: those live in
//! `navigator dev e2e`, not in the workspace test job that starts this
//! engine, so one flag for all three would fail the suites whose fixture
//! that job never brings up.

use store::schema::{self, SchemaState};
use store::surreal::{connect, SurrealConfig, SurrealConfigError};

/// The configured engine, or `None` when this lane is not wired up.
///
/// Each test gets its own database on the shared engine — the same
/// isolation shape an embedded engine gives every other test, and
/// what keeps two tests in this file from colliding on one server.
fn config(database: &str) -> Option<SurrealConfig> {
    match SurrealConfig::from_env() {
        Ok(config) => Some(SurrealConfig {
            database: database.to_string(),
            ..config
        }),
        Err(SurrealConfigError::MissingEnv(name)) => {
            assert!(
                std::env::var("NAV_REQUIRE_SURREAL").as_deref() != Ok("1"),
                "NAV_REQUIRE_SURREAL=1 but {name} is unset: the server-mode SurrealDB lane cannot \
                 run. Start the dependency tier (`navigator dev up`) and source `.devx/env`.",
            );
            eprintln!("skipping the server-mode SurrealDB lane: {name} is unset");
            None
        }
        Err(err) => panic!("the SurrealDB environment is half-configured: {err}"),
    }
}

/// Connect over the wire, authenticate, select coordinates, and prove
/// the connection is real by writing and reading a row back.
#[tokio::test]
async fn connects_over_the_wire_and_round_trips_a_row() {
    let Some(config) = config("test_server_round_trip") else {
        return;
    };
    assert!(
        config.endpoint.starts_with("ws://") || config.endpoint.starts_with("wss://"),
        "this lane exists to exercise the remote protocol, but the endpoint is `{}`",
        config.endpoint
    );

    let db = connect(&config).await.expect("connect to the engine");
    schema::apply(&db).await.expect("apply the schema");

    // `email` is required on `person` and has no default, so the write
    // has to carry one — the same shape every other scratch row against
    // the deployment schema uses.
    db.query("CREATE person:wire SET name = 'Over The Wire', email = 'wire@example.com'")
        .await
        .unwrap()
        .check()
        .unwrap();
    let name: Option<String> = db
        .query("SELECT VALUE name FROM person:wire")
        .await
        .unwrap()
        .take(0)
        .unwrap();

    assert_eq!(name.as_deref(), Some("Over The Wire"));

    db.query("REMOVE TABLE person").await.unwrap().check().ok();
}

/// Router state and Dioxus context factories clone the store handle while a
/// request is being assembled.  A remote engine must accept work through each
/// of those clones: unlike `mem://`, a WebSocket client has a server session
/// to register before its first query can arrive.
///
/// Keep this in the server lane.  The embedded engine does not exercise the
/// clone registration protocol that a deployment uses.
#[tokio::test]
async fn request_owned_clones_can_query_the_remote_engine() {
    let Some(config) = config("test_server_request_clones") else {
        return;
    };
    let db = connect(&config).await.expect("connect to the engine");
    schema::apply(&db).await.expect("apply the schema");

    // This is deliberately more than the one clone a standalone query uses:
    // each mounted router and each Dioxus render context owns a clone in web.
    let mut queries = tokio::task::JoinSet::new();
    for handle in (0..32).map(|_| db.clone()) {
        queries.spawn(async move { handle.query("RETURN 1").await?.take::<Option<i64>>(0) });
    }
    while let Some(result) = queries.join_next().await {
        let value = result
            .expect("request-owned query task does not panic")
            .expect("request-owned handle queries the remote engine");
        assert_eq!(value, Some(1));
    }
}

/// The schema apply and its drift check work the same against a remote
/// engine as against an embedded one — the property the local loop and
/// every deployment both rely on at boot.
#[tokio::test]
async fn applying_the_schema_remotely_is_idempotent_and_reports_in_sync() {
    let Some(config) = config("test_server_schema") else {
        return;
    };
    let db = connect(&config).await.expect("connect to the engine");

    schema::apply(&db).await.expect("first apply");
    schema::apply(&db).await.expect("second apply");

    assert_eq!(schema::state(&db).await.unwrap(), SchemaState::InSync);
    assert_eq!(
        schema::installed_version(&db).await.unwrap(),
        Some(schema::SCHEMA_VERSION)
    );
}

/// Introspection — what `navigator erd` reads — against a real server
/// rather than an in-process engine.
#[tokio::test]
async fn the_applied_schema_introspects_back_over_the_wire() {
    let Some(config) = config("test_server_introspect") else {
        return;
    };
    let db = connect(&config).await.expect("connect to the engine");
    schema::apply(&db).await.expect("apply the schema");

    let introspection = schema::introspect(&db).await.expect("introspect");

    for table in ["person", "entity", "entity_role", "relationship"] {
        assert!(
            introspection.contains_key(table),
            "`{table}` missing from {:?}",
            introspection.keys().collect::<Vec<_>>()
        );
    }
    let relationship = &introspection["relationship"];
    assert!(
        relationship.definition.contains("TYPE RELATION"),
        "{}",
        relationship.definition
    );
    // The edge ends are the implicit link fields Surreal maintains, and
    // the ERD reads its foreign keys straight out of their types.
    for end in ["in", "out"] {
        assert!(
            relationship.fields[end].contains("record<person | entity>"),
            "{}",
            relationship.fields[end]
        );
    }
}
