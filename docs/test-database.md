# Test database — one engine per test, inside the test process

> **`cargo test` needs no external database or container.** Each test opens its own embedded, memory-backed SurrealDB
> and applies the schema to it. The checked-in `.cargo/config.toml` caps ordinary libtest at four workers because the
> embedded client is not safe to tear down at higher concurrency.

The local KIND lifecycle resets staging by deleting its guarded namespace, never by truncating tables; old workflow
journals cannot survive.

## The pattern

`store::test_support::mem_surreal()` starts an embedded `kv-mem` engine **inside the test process** with the `DEFINE`
schema applied. No container, no port, no shared server, nothing to reclaim: the database dies with the test's own
memory, so two tests cannot collide and the whole suite runs in parallel with no isolation machinery.

That is the whole contract. There is no external environment variable to set, no server to start, and nothing left
behind on disk when a run ends.

## The one exception — a test that spawns `navigator`

A subprocess cannot reach an in-process engine, so a test that runs the binary needs a real server instead.
`store::test_support::server_surreal(database)` connects to one and hands back the coordinates to point that process at,
on a database of the caller's own naming so two such tests cannot collide.

- **`NAVIGATOR_SURREAL_ENDPOINT` set** → connect, apply the schema, run.
- **unset** → return `None`, and the test skips.

`NAV_REQUIRE_SURREAL=1` turns that skip into a failure, which is what keeps CI honest. A half-configured environment (an
endpoint with no namespace) always panics: that is a mistake, not an opt-out.

## Test tiers

- **Tier 1 — `cargo test` and BDD:** the store is embedded; embedded Rego, Rauthy, Restate, storage, vendors, and agent
  routing use their in-process test seams.
- **Tier 2 — KIND e2e:** Rauthy, embedded Rego, Restate, Garage, browser, and accessibility use the real topology.

Do not pull sidecars into tier 1. Its trait seams are the contract boundary, and tier 1 has no external dependency at
all — that is the contributor floor, and it should stay there.

## How a contributor runs tests

```sh
cargo nextest run --workspace && cargo test -p features
```

Nothing to install, nothing to reclaim. The cucumber BDD suites in `features` keep `cargo test`; their custom harness
does not speak nextest's protocol, so the workspace nextest profile excludes that package.

To also exercise the server-mode lane — the WebSocket protocol, root `signin`, and namespace/database selection over a
wire, which an in-process engine cannot cover — point the harness at a running engine:

```sh
set -a; source .devx/env; set +a
NAV_REQUIRE_SURREAL=1 cargo nextest run --workspace
```

## How CI runs them

The test job starts one SurrealDB container for the server-mode lane and sets `NAV_REQUIRE_SURREAL=1` so a broken engine
is a red build rather than a silently absent test. Every other test runs against its own embedded engine. KIND e2e
covers the real stack, browser, and accessibility.
