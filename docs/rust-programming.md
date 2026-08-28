# Rust programming

Use these canonical language references when behavior matters:

- [The Rust Programming Language](https://doc.rust-lang.org/book/) [Rust by
  Example](https://doc.rust-lang.org/rust-by-example/) [Rust API
  Guidelines](https://rust-lang.github.io/api-guidelines/) [Rust async book](https://rust-lang.github.io/async-book/)
  [Rust edition guide](https://doc.rust-lang.org/edition-guide/) [rustfmt
  reference](https://rust-lang.github.io/rustfmt/) [Clippy lint index](https://rust-lang.github.io/rust-clippy/master/)

## Workspace defaults

- `rust-toolchain.toml` and workspace `Cargo.toml` define the toolchain, edition, lints, and formatting.
- `unsafe_code = "forbid"`; clippy pedantic warnings run with `-D warnings`.
- Tokio, Axum, and SurrealDB are the runtime stack. `workflows-service`, `archives`, `billing-workflows`, and
  `github_webhooks` each consume Restate SDK directly; `workflows` carries no `restate-sdk` dependency itself.

## Error handling

- Libraries use typed `thiserror` enums. Binaries use contextual `anyhow::Result<T>` at the boundary. HTTP handlers each
  define their own typed error implementing `IntoResponse` (e.g. `ApiError` in `portal/src/api.rs`, `WebhookError` in
  `portal/src/esignature_webhook.rs`). Avoid `Box<dyn Error>` in public signatures and `unwrap`/`expect` outside tests
  or `main` unless a one-line message proves a local invariant.

## Types and modules

- Prefer newtypes for cross-module ids, enums for meaningful state, `Option<T>` over sentinels, and borrowed inputs when
  ownership is unnecessary. Keep one concept per file. Public re-exports use `mod` plus `pub use`.

## Comments describe the present

- Comments, docs, and tests state the current contract, not chronology. Git records past decisions. Remove old paths,
  flags, aliases, examples, README rows, and tests with the feature they served.
- Keep rationale for live invariants and migrations, plus guards for current behavior such as required redirects,
  `404`s, or removed columns.

## Async and concurrency

- Prefer `async fn` and structured concurrency. A bare `tokio::spawn` needs ownership and cancellation. Bound channels;
  reserve `unbounded_channel` for controlled intra-process control planes. Do not hold a mutex guard across `.await`
  without audit. Use `spawn_blocking` for blocking work and timeouts around external calls.

Inside Restate handlers, the rules are stricter: do not use native concurrency for journaled work. See
[`agent-workflows.md`](agent-workflows.md#author-a-restate-handler) and [`durable-workflows.md`](durable-workflows.md).

## Axum

- Extend the existing router with typed extractors and explicit state. Keep authorization beside existing helpers. New
  `/app/...` routes follow [`access-model.md`](access-model.md) and embedded Rego; hidden lawyer-only routes return
  `404`.
- Body/consuming extractors (`Json`, `Form`, multipart) go last in a handler's argument list — the body can only be
  consumed once. `5xx` responses log via `tracing` before returning; `4xx` responses do not.

## The store

- SurrealDB is the only database. Its schema is a statement of the present, not a history: edit
  `store/src/schema/navigator.surql` and bump `SCHEMA_VERSION` in the same commit. One table is one top-level
  `store::<table>` module. Every write maintains `inserted_at` / `updated_at`. Re-seeding inserts missing rows
  idempotently and never updates live production rows.

## Service lifecycle

- Long-running binaries initialize dependencies before serving, hold the telemetry guard through `main`, and use the
  workspace shutdown helper. Liveness never probes downstreams; readiness performs required dependency round-trips.

## Testing

- Tests ship with implementation. Unit tests live beside code; integration tests under `<crate>/tests/`. Use
  `#[tokio::test]`, `assert_cmd`/`predicates` for CLI smoke tests, and snapshots for stable HTML/JSON shapes. Restate
  changes require replay-aware coverage.

## Dependencies and assets

- Use `cargo update` for compatible lockfile refreshes; `cargo upgrade` changes requirements and needs explicit review.
  Separate crate and vendored-asset updates. Serve web assets same-origin; never use runtime CDNs.

## Before committing Rust

```bash
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
cargo test -p features
```

Run narrow tests while iterating and report the exact gate. Prefer failure-only output:

- `cargo nextest run`; `.config/nextest.toml` prints failures only. `features` uses its custom harness and must run with
  `cargo test -p features`.
- `cargo build -q --message-format short`; use full diagnostics only when their rendered context is needed.
