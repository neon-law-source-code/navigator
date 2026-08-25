---
name: rust
description: >
  Workspace Rust guardrails. Trigger on the sharpest moments: a change that adds `unsafe`, `unwrap`, `expect`, or
  `panic!` outside `main()`/tests; introducing a new public API, error type, or module; reaching for a different web
  framework, store client, or async runtime (we standardize on Axum + SurrealDB + Tokio); or wiring a new binary's
  `main()`. Read
  [`docs/rust-programming.md`](../../../docs/rust-programming.md) before acting — it is the authoritative reference.
---

# Rust guardrails

The doc owns the conventions; this skill is the short list of guards that are easy to violate. Read
[`docs/rust-programming.md`](../../../docs/rust-programming.md) and keep it, not this skill, authoritative.

- **No `unwrap`/`expect`/`panic!` outside `main()` and tests.** Use `?` with `anyhow` (binaries) or `thiserror`
  (libraries); `expect("invariant: …")` only when the invariant is provable in one line for a future reader.
- **`unsafe_code = "forbid"`** at the workspace level — never reach for `unsafe`.
- **Standardize on Axum + SurrealDB + Tokio.** Don't add a second web framework, store client, or async runtime;
  extend the existing
  router, entity, and runtime instead.
- **One canonical shutdown-signal helper** for service lifecycle (SIGTERM + SIGINT). No ad-hoc `ctrl_c().await.unwrap()`
  inline in `main`.
- **Axum body/consuming extractors go LAST** in handler argument order — the body can only be consumed once.
- **Iterate on failure-only output.** Test with `cargo nextest run` (the workspace default profile prints failures only,
  in full) and compile-check with `cargo build -q --message-format short` — not bare `cargo test`/`cargo build`, whose
  success noise buries the diagnostic that matters. The one exception is the cucumber `features` package: nextest cannot
  drive its custom harness, so those suites run with `cargo test -p features`.
- **Comments and tests describe the present.** No "we used to…"/"no longer…"/"legacy" narration, no deprecated-but-kept
  flags or aliases — delete the old path; git history holds the past. Keep only the *why* behind a live invariant and
  guard tests that assert today's behavior.

Everything else — conventions, async, Axum, the store, service lifecycle, testing — is in
[`docs/rust-programming.md`](../../../docs/rust-programming.md).
