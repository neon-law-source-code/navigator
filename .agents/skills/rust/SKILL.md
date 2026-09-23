---
name: rust
description: >
  Workspace Rust guardrails. Trigger on the sharpest moments: a change that adds `unsafe`, `unwrap`, `expect`, or
  `panic!` outside `main()`/tests; introducing a new public API, error type, or module; reaching for a different web
  framework, store client, or async runtime (we standardize on Axum + SurrealDB + Tokio); or wiring a new binary's
  `main()`. Read [`docs/rust-programming.md`](../../../docs/rust-programming.md) before acting — it is the authoritative
  reference.
---

# Rust guardrails

- **No `unwrap`/`expect`/`panic!` outside `main()` and tests.** Use `?` with `anyhow` (binaries) or `thiserror`
  (libraries); `expect("invariant: …")` only when the invariant is provable in one line for a future reader.
- **`unsafe_code = "forbid"`** at the workspace level — never reach for `unsafe`.
- **Standardize on Axum + SurrealDB + Tokio.** Don't add a second web framework, store client, or async runtime; extend
  the existing router, entity, and runtime instead.
- **One canonical shutdown-signal helper** for service lifecycle (SIGTERM + SIGINT). No ad-hoc `ctrl_c().await.unwrap()`
  inline in `main`.
- **Axum body/consuming extractors go LAST** in handler argument order — the body can only be consumed once.
- **Iterate on failure-only output.** When installed, use RTK for agent-facing `cargo build`, `check`, `clippy`,
  `test`, and `nextest` commands; it collapses repetitive success output while preserving failures, warnings, and exit
  codes. RTK reduces context usage, not Rust compile time. Keep `cargo fmt`, coverage, project gates, machine-readable
  `--message-format` consumers, and raw diagnostics on ordinary Cargo commands. The one exception is the cucumber
  `features` package: nextest cannot drive its custom harness, so those suites run with `rtk cargo test -p features`.
- **Comments and tests describe the present.** No "we used to…"/"legacy" narration and no deprecated-but-kept flags or
  aliases — delete the old path; git history holds the past. Keep the *why* behind a live invariant, nothing else.
- **A removal removes the whole seam.** Deleting the caller is half the job. In the same change, delete the types,
  catalog and manifest entries, fixtures, CSS, tests, and docs that existed only for it, and any enum variant or
  constant left with no member. Then sweep: `git grep -n '<removed-name>'` returns nothing but the guard test asserting
  it is gone. What survives a half-removal is dead weight a reader trusts, or an entry some later verify step fails on.
- **Opportunistic file pass.** [`random-refactor`](../random-refactor/SKILL.md) picks one tracked `.rs` file and
  compares it to this skill, The Rust Book, a `/tmp` standard-library clone, and similar patterns in the repository.

Everything else — conventions, async, Axum, the store, service lifecycle, testing — is in
[`docs/rust-programming.md`](../../../docs/rust-programming.md).
