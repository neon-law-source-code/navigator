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

use std::future::Future;
use std::sync::{Arc, Mutex};

use store::schema::{self, SchemaState};
use store::surreal::{connect, ping, SurrealConfig, SurrealConfigError};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;

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

/// Introspection against a real server rather than an in-process
/// engine.
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
    // a reader recovers the foreign keys straight out of their types.
    for end in ["in", "out"] {
        assert!(
            relationship.fields[end].contains("record<person | entity>"),
            "{}",
            relationship.fields[end]
        );
    }
}

/// A `MakeWriter` that appends every write to a shared buffer, for asserting
/// on rendered log output. Mirrors the pattern `telemetry::tests` uses for
/// the same reason: `tracing_subscriber::fmt` only writes to something
/// implementing `Write`, and a test wants that output back as a string.
#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<String>>);

struct CapturedLogsWriter(Arc<Mutex<String>>);

impl<'a> MakeWriter<'a> for CapturedLogs {
    type Writer = CapturedLogsWriter;

    fn make_writer(&'a self) -> Self::Writer {
        CapturedLogsWriter(self.0.clone())
    }
}

impl std::io::Write for CapturedLogsWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("captured-log buffer is not poisoned")
            .push_str(&String::from_utf8_lossy(bytes));
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// ENG-709: `readyz`'s kubelet probe has its own short timeout, and firing
/// before `store::surreal::ping` answers drops the axum handler future that
/// was awaiting it. Reproduce exactly that race — cancel the caller before
/// the query returns — and prove the remote WS engine never logs "Failed to
/// send query results to channel", the ERROR line that was ~85% of
/// staging's log volume before `ping` started running its query on a task
/// decoupled from the caller's lifetime.
///
/// `current_thread` matters here: it is what makes the spawned query task
/// share this test's OS thread, so the thread-local subscriber this test
/// installs sees every event the query emits, on whichever task it fires
/// from.
#[tokio::test(flavor = "current_thread")]
async fn cancelling_the_caller_never_orphans_the_query_on_the_wire() {
    let Some(config) = config("test_server_ping_cancellation") else {
        return;
    };
    let db = connect(&config).await.expect("connect to the engine");

    let logs = CapturedLogs::default();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_writer(logs.clone())
            .with_ansi(false),
    );
    let _guard = tracing::subscriber::set_default(subscriber);

    // Poll `ping` exactly once, with a no-op waker, then drop it without
    // polling again — the readiness-probe race, made deterministic. A
    // `tokio::time::timeout` racing a real clock against a loopback engine
    // is not reliable for this: the query can complete before the timer is
    // even checked, so the "cancel" never actually happens and the test
    // proves nothing. Polling once and dropping cancels unconditionally,
    // after the query has genuinely gone out over the wire (its first poll
    // cannot resolve synchronously — the response has to come back from the
    // engine's own router task) but before any response arrives.
    let mut fut = Box::pin(ping(&db));
    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);
    assert!(
        fut.as_mut().poll(&mut cx).is_pending(),
        "ping must not resolve on its very first poll for this test to prove anything"
    );
    drop(fut);

    // Give the runtime a few turns to drive the detached query task (and the
    // engine's own router task) to completion on this thread before reading
    // the captured output.
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }

    let rendered = logs
        .0
        .lock()
        .expect("captured-log buffer is not poisoned")
        .clone();
    assert!(
        !rendered.contains("Failed to send query results to channel"),
        "cancelling the caller must not orphan the in-flight query: {rendered}"
    );

    // The connection itself must still be healthy for the next probe.
    ping(&db).await.expect("a later ping still succeeds");
}
