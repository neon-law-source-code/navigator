//! The SurrealDB connection — the store the workspace is porting onto
//! (#1093).
//!
//! This module is the whole of Navigator's knowledge of how to reach
//! Surreal: one [`SurrealConfig`] resolved from `NAVIGATOR_SURREAL_*`,
//! one [`connect`] that selects namespace and database, and nothing
//! else. Query modules land in the later slices ([#1143] is the
//! foundation).
//!
//! # One engine
//!
//! This is the only store Navigator opens. A binary with no
//! `NAVIGATOR_SURREAL_*` coordinates fails loudly at boot rather than
//! falling back to anything.
//!
//! # One endpoint, three topologies
//!
//! [`surrealdb::engine::any::connect`] resolves the scheme at runtime,
//! so the same call site serves every environment:
//!
//! - `ws://localhost:<port>` — the KIND dependency tier, port-forwarded
//!   to the host. `dev up` and `dev worktree-env up` write this into
//!   `.devx/env`.
//! - `wss://…` — Surreal Cloud, from phase 5.
//! - `mem://` — an engine inside the calling process: one per test in
//!   [`test_support`](crate::surreal::test_support), and one per
//!   pre-matter conflict check in [`crate::conflicts`], which builds
//!   its graph there and drops it with the check. Reachable only
//!   because it is *named*; it is never a default, so nothing reaches
//!   an in-process engine by failing to configure a real one.
//!
//! [#1143]: https://github.com/neon-law-source-code/navigator/issues/1143

mod config;
mod record;
pub mod retry;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use config::{
    AuthScope, SurrealAuth, SurrealConfig, SurrealConfigError, AUTH_SCOPE_ENV, DATABASE_ENV,
    ENDPOINT_ENV, NAMESPACE_ENV, PASSWORD_ENV, TOKEN_ENV, USER_ENV,
};
pub use record::{record_id, record_uuid};

use std::ops::Deref;
use std::sync::{Arc, Once};

use surrealdb::engine::any::Any;
use surrealdb::opt::auth::{Database, Namespace, Root};
use surrealdb::Surreal;
use thiserror::Error;

/// A connected SurrealDB client with its namespace and database already
/// selected.
///
/// The SDK gives every clone of [`Surreal`] a separate remote session.  Web
/// state is cloned by Axum extractors and Dioxus context providers, so exposing
/// the SDK handle directly would create sessions while requests are being
/// assembled.  Keep one SDK handle in an [`Arc`] instead: Navigator clones are
/// aliases to the session that completed connection setup.
#[derive(Clone)]
pub struct SurrealDb(Arc<Surreal<Any>>);

impl std::fmt::Debug for SurrealDb {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_tuple("SurrealDb").finish()
    }
}

impl SurrealDb {
    /// A handle with no engine behind it, for error-path tests.
    #[must_use]
    pub fn uninitialized() -> Self {
        Self(Arc::new(Surreal::init()))
    }
}

impl Deref for SurrealDb {
    type Target = Surreal<Any>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Error)]
pub enum SurrealError {
    #[error(transparent)]
    Config(#[from] SurrealConfigError),
    #[error("connect to SurrealDB at {endpoint}")]
    Connect {
        endpoint: String,
        #[source]
        source: surrealdb::Error,
    },
    #[error("authenticate as root against SurrealDB at {endpoint}")]
    Signin {
        endpoint: String,
        #[source]
        source: surrealdb::Error,
    },
    #[error("select namespace `{namespace}` and database `{database}`")]
    Select {
        namespace: String,
        database: String,
        #[source]
        source: surrealdb::Error,
    },
}

/// Connect using the process environment.
#[allow(clippy::result_large_err)]
pub async fn connect_from_env() -> Result<SurrealDb, SurrealError> {
    connect(&SurrealConfig::from_env()?).await
}

/// Install a rustls crypto provider, once per process.
///
/// A TLS connection through rustls refuses to pick a provider on its own
/// when more than one is compiled in. This workspace links both
/// `aws-lc-rs` and `ring` — they arrive through different dependents — so
/// without this call the first handshake **panics** with "Could not
/// automatically determine the process-level `CryptoProvider`".
///
/// Nothing local catches it. The KIND tier and CI speak plaintext `ws://`
/// and reach Kubernetes through `kubectl` rather than a Rust client, so
/// the first rustls handshake in a process's life is usually one only a
/// deployed pod or a production operator command makes — a `wss://`
/// Surreal endpoint at boot, or `ops observability` reading GKE
/// endpoints through the Kubernetes API.
///
/// Public because rustls is process-global while its consumers are not
/// one module: `store::surreal` reaches the engine, and the CLI builds a
/// `kube` client for the same process. Each entry point calls this before
/// its first handshake.
///
/// `install_default` returns `Err` if a provider is already installed,
/// which is not a failure: a process that installed its own is already
/// in the state this needs. `Once` makes the call idempotent regardless.
pub fn install_tls_provider() {
    static TLS: Once = Once::new();
    TLS.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

/// One-line liveness probe for the readiness endpoint: run the cheapest
/// statement the engine has and report whether it came back.
///
/// A `SELECT 1` rather than a table read, so the probe answers "is the
/// engine reachable and answering" without depending on any row existing
/// or on the schema being applied.
///
/// # Errors
///
/// The engine's own error when the query does not complete.
pub async fn ping(db: &SurrealDb) -> Result<(), surrealdb::Error> {
    db.query("RETURN 1")
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    Ok(())
}

/// Connect to `config`'s endpoint, sign in when it carries credentials,
/// and select its namespace and database.
///
/// The namespace/database selection is part of connecting rather than a
/// step a caller may forget: every environment — each worktree's KIND
/// tier, and each deployment's Surreal Cloud namespace — is one
/// coordinate pair, and a client that skipped the selection would
/// happily run statements against no database at all.
#[allow(clippy::result_large_err)]
pub async fn connect(config: &SurrealConfig) -> Result<SurrealDb, SurrealError> {
    install_tls_provider();
    let db = surrealdb::engine::any::connect(config.endpoint.clone())
        .await
        .map_err(|source| SurrealError::Connect {
            endpoint: config.endpoint.clone(),
            source,
        })?;

    let signin = |source| SurrealError::Signin {
        endpoint: config.endpoint.clone(),
        source,
    };
    match &config.auth {
        SurrealAuth::Anonymous => {}
        SurrealAuth::Password {
            scope: AuthScope::Root,
            username,
            password,
        } => {
            db.signin(Root {
                username: username.clone(),
                password: password.clone(),
            })
            .await
            .map_err(signin)?;
        }
        SurrealAuth::Password {
            scope: AuthScope::Namespace,
            username,
            password,
        } => {
            db.signin(Namespace {
                namespace: config.namespace.clone(),
                username: username.clone(),
                password: password.clone(),
            })
            .await
            .map_err(signin)?;
        }
        SurrealAuth::Password {
            scope: AuthScope::Database,
            username,
            password,
        } => {
            db.signin(Database {
                namespace: config.namespace.clone(),
                database: config.database.clone(),
                username: username.clone(),
                password: password.clone(),
            })
            .await
            .map_err(signin)?;
        }
        SurrealAuth::Token(token) => {
            db.authenticate(token.clone()).await.map_err(signin)?;
        }
    }

    // Selecting coordinates creates the database if it is new, which is
    // a write to the engine's catalog — so two processes coming up
    // together (host `web` and the in-cluster worker, or a test binary's
    // parallel tests) can collide there. The engine reports that as a
    // retryable transaction conflict, so honor it rather than failing a
    // boot on a race.
    retry::retrying(|| {
        db.use_ns(config.namespace.clone())
            .use_db(config.database.clone())
    })
    .await
    .map_err(|source| SurrealError::Select {
        namespace: config.namespace.clone(),
        database: config.database.clone(),
        source,
    })?;

    Ok(SurrealDb(Arc::new(db)))
}

#[cfg(test)]
mod tests {
    use super::{connect, SurrealConfig, SurrealError};

    fn embedded(namespace: &str, database: &str) -> SurrealConfig {
        SurrealConfig {
            endpoint: "mem://".into(),
            namespace: namespace.into(),
            database: database.into(),
            auth: crate::surreal::SurrealAuth::Anonymous,
        }
    }

    #[tokio::test]
    async fn connects_to_a_named_embedded_engine_and_round_trips_a_row() {
        let db = connect(&embedded("navigator", "test")).await.unwrap();

        db.query("CREATE person:alice SET name = 'Alice'")
            .await
            .unwrap()
            .check()
            .unwrap();
        let name: Option<String> = db
            .query("SELECT VALUE name FROM person:alice")
            .await
            .unwrap()
            .take(0)
            .unwrap();

        assert_eq!(name.as_deref(), Some("Alice"));
    }

    /// The isolation property the local loop depends on, in the shape
    /// that could break it: two databases inside ONE engine. A
    /// worktree's rows must be invisible to another environment even
    /// when both are reachable through the same connection.
    #[tokio::test]
    async fn a_row_in_one_database_is_invisible_from_another() {
        let db = connect(&embedded("navigator", "worktree_a")).await.unwrap();
        crate::schema::apply(&db).await.unwrap();
        db.query("CREATE person:alice SET name = 'Alice', email = 'alice@example.com'")
            .await
            .unwrap()
            .check()
            .unwrap();

        // Same connection, sibling database — the coordinates `connect`
        // selects are the whole of the boundary. The schema is applied
        // here too, so the assertion below rests on an empty result
        // rather than on the sibling lacking the table.
        db.use_db("worktree_b").await.unwrap();
        crate::schema::apply(&db).await.unwrap();
        let visible: Option<String> = db
            .query("SELECT VALUE name FROM person:alice")
            .await
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(visible, None, "worktree_b saw worktree_a's row");

        db.use_db("worktree_a").await.unwrap();
        let own: Option<String> = db
            .query("SELECT VALUE name FROM person:alice")
            .await
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(own.as_deref(), Some("Alice"), "worktree_a lost its own row");
    }

    #[tokio::test]
    async fn an_unreachable_endpoint_reports_the_endpoint_it_tried() {
        // Port 1 is privileged and unbound: the connection cannot be
        // mistaken for a live engine.
        let err = connect(&SurrealConfig {
            endpoint: "ws://127.0.0.1:1".into(),
            namespace: "navigator".into(),
            database: "navigator".into(),
            auth: crate::surreal::SurrealAuth::Anonymous,
        })
        .await
        .unwrap_err();

        assert!(
            matches!(&err, SurrealError::Connect { endpoint, .. } if endpoint == "ws://127.0.0.1:1"),
            "{err:?}"
        );
        assert!(err.to_string().contains("ws://127.0.0.1:1"), "{err}");
    }
}
