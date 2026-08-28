# Iceberg archive — design

Status: **writer landed** (`archives::iceberg::author_snapshot`) and already wired for the telemetry tables — the
nightly `Archives` workflow's `iceberg_telemetry` phase (`archives/src/workflow.rs`) promotes `otel_logs`,
`otel_traces`, and `otel_metrics` to Iceberg tables every run. Wiring the primary SurrealDB entity-table snapshot writer
and BigLake registration are the remaining steps. This doc names every table, its partitioning, the GCS layout, the
BigQuery wiring, and the retention policy.

## Evaluation outcome (recorded 2026-06-14)

The writer uses the **`iceberg` crate (0.10)** for format correctness — a subtly-wrong hand-rolled Avro manifest is a
silently unreadable archive, and this lake holds binding legal records. Two frictions shaped the integration:

- **Arrow major mismatch.** `iceberg` 0.10 targets **arrow 58**; the workspace is on **arrow 59**. We never hand the
  crate an arrow value — `arrow_schema_to_iceberg` derives the Iceberg schema from the arrow-59 field names/types
  directly, so the two arrow majors never meet at a type boundary.
- **Storage rule.** Bytes must flow through `cloud::StorageService`, never a GCS SDK (CLAUDE.md). `iceberg`'s manifest
  writers only write to their own `FileIO`, so the writer points them at an **in-memory** `FileIO`, passing the final
  `gs://` object URIs as the output paths (the memory backend treats them as opaque keys, so the paths embedded in the
  manifest list and snapshot are the real ones), then reads the bytes back and returns them for the caller to persist.

**v1 simplifications** (documented so a reader isn't surprised): the table is **unpartitioned** — point-in-time queries
come from the snapshot log (each run appends a snapshot), which is the design's stated mechanism; `dt=` identity
partitioning and per-file column bounds are deferred refinements. The append-only snapshot log is preserved across runs
by chaining from the prior `v<N>.metadata.json` (`TableMetadataBuilder::new_from_metadata`).

## Why

The nightly [`Archives`](durable-workflows.md) Restate workflow snapshots every table declared in Navigator's SurrealDB
schema to **Parquet** on GCS (`archives/src/snapshot.rs`, `archives/src/parquet_io.rs`). The objects already live under
an Iceberg-shaped prefix:

```text
gs://<project>-exports/iceberg/<table>/data/<yyyy-mm-dd>/part-<uuid_v7>.parquet
```

But they are **loose Parquet files**, not a table: there is no Iceberg metadata, so a reader has to glob the prefix and
guess the schema, schema evolution is invisible, and "the documents table as of last Tuesday" is not a query — it is a
file hunt. Promoting these snapshots to **Apache Iceberg table format** (table metadata + manifest lists + manifest
files alongside the existing `data/` Parquet) turns each prefix into an evolving, time-travelable, engine-portable
table. "Archive everything to Iceberg" then has a precise meaning: SurrealDB table snapshots are one queryable lake,
readable from BigQuery (and Spark, Trino, DuckDB) without a copy.

## What "promote to Iceberg" means concretely

For each table prefix we additionally write, every nightly run:

- `metadata/v<N>.metadata.json` — the table metadata: schema (with column ids), partition spec, current snapshot id,
  and the snapshot log. Version `N` increments each run; `metadata/version-hint.text` points at the current `N`.
- `metadata/snap-<snapshot-id>-<uuid>.avro` — the **manifest list** for that snapshot (one row per manifest).
- `metadata/<uuid>-m0.avro` — the **manifest file** listing the data files added that run, with per-file row counts and
  column bounds.
- `data/dt=<yyyy-mm-dd>/part-<uuid_v7>.parquet` — unchanged from today, except the partition directory is the
  Iceberg-standard `dt=<date>` form (see Partitioning).

The data files we already produce are reused as-is; the new bytes are metadata. The snapshot is **append-only**: each
run adds a new snapshot pointing at that night's data files, and prior snapshots stay valid — so the lake mirrors the
append-only [per-Project git repos](project-repositories.md) and never rewrites history.

## Tables

### SurrealDB table snapshots (full-table, nightly)

`archives::tables::ALL_TABLES` is generated from every `DEFINE TABLE` in the shipped `store/src/schema/navigator.surql`.
Adding a table to that schema adds it to the analytical lake in the same build; a read failure remains visible in the
nightly summary rather than becoming an unregistered-table gap.

Each nightly run writes a **full snapshot** of the current SurrealDB table. Record links land as stable `table:key`
strings and datetimes as RFC 3339 strings, because the Parquet contract is nullable UTF-8 columns. Point-in-time queries
come from Iceberg's snapshot log, not from diffing.

### Telemetry table snapshots (already promoted, nightly)

`otel_logs`, `otel_traces`, and `otel_metrics` (`archives/src/workflow.rs`'s `TELEMETRY_TABLES`) are promoted to Iceberg
tables every nightly run by the `iceberg_telemetry` phase, reusing the same `author_snapshot` /
`author_iceberg_for_prefix` writer described above. `otel_logs` keeps its full snapshot log; `otel_traces` and
`otel_metrics` prune snapshots older than 30 days to match their GCS lifecycle window.

## Partitioning

- **SurrealDB table snapshots:** partition by `dt` = the run date (`SnapshotConfig.run_date`, UTC), identity transform.
  One partition per nightly run; a full snapshot lands wholly in its `dt=<date>` partition. This matches the existing
  `data/<yyyy-mm-dd>/` directory — the only change is the `dt=` column prefix so engines recognize the partition.

Partition spec is recorded in the table metadata, so a reader prunes by `dt` without scanning.

## GCS layout

One bucket, `gs://<project>-exports` (the existing `NAVIGATOR_EXPORTS_BUCKET`; `cloud::exports_from_env`). Per table:

```text
gs://<project>-exports/iceberg/<table>/
  metadata/
    version-hint.text
    v<N>.metadata.json
    snap-<snapshot-id>-<uuid>.avro      # manifest list
    <uuid>-m0.avro                       # manifest file(s)
  data/
    dt=2026-06-10/part-<uuid_v7>.parquet
    dt=2026-06-11/part-<uuid_v7>.parquet
```

Bytes stay in `cloud::StorageService` (GCS in prod, `FsStorage` in dev) — the writer goes through the trait, never the
GCS SDK directly, per [CLAUDE.md](../CLAUDE.md).

## Catalog + BigQuery wiring — **operator decision required**

Two ways to make BigQuery read the lake; pick one before building the writer:

1. **BigLake / BigQuery managed Iceberg tables (recommended).** Register each Iceberg table once via a BigLake
   connection; BigQuery reads the Iceberg metadata directly, so schema evolution and snapshot adds show up without a
   per-run DDL. This is the closest to "it's just a table." Cost: a one-time BigLake connection + IAM on the bucket.
2. **BigQuery external tables over the Parquet (status quo, simplest).** A per-table `CREATE EXTERNAL TABLE` with
   `OPTIONS(format = 'PARQUET', uris = ['gs://…/iceberg/<table>/data/*'])` — the pattern already used for the
   email-events stream (see [the email-events pipeline](email-events-pipeline.md)). No Iceberg metadata needed, but no
   time-travel and no schema evolution: it globs the data files. Use this only if BigLake is unavailable.

**Recommendation:** BigLake managed Iceberg, because the whole point of promoting to Iceberg is time-travel + schema
evolution; option 2 throws both away. The writer should emit standards-compliant Iceberg metadata regardless, so the
catalog choice does not change the bytes on GCS — only how BigQuery is pointed at them.

> **Flag for the operator:** confirm BigLake-managed-Iceberg vs external-tables-over-Parquet before the writer commit.
> This is the one externally-constrained decision; the rest of this doc holds either way.

## Retention

- **Data + manifests:** keep **10 years**, matching the matter-file retention the client consents to in the retainer
  (`projects.closed_at + 10y`, see <surreal-archives.md>). A lifecycle rule transitions `iceberg/**` to Coldline at 365
  days (the existing GCS lifecycle, see the GCP cost-cleanup notes) and deletes at 10 years.
- **Snapshot expiry:** Iceberg snapshot-expiry (dropping old manifest entries) is **not** run — the snapshot log is the
  point-in-time index we want, and 10 years of nightly full snapshots is small relative to the data. Revisit only if the
  metadata grows unwieldy.

## Where it runs

Inside the existing nightly `Archives` Restate workflow on `workflows-service` — **no new worker pod** ([all workflows
live in workflows-service](durable-workflows.md)). The Iceberg metadata write is one additional journaled `ctx.run` step
per table, after the Parquet write it already does. The diagnostic email gains an "Iceberg tables" line per table
(snapshot id + metadata version).

## Council consensus (folded in)

Run inline; the deliberation is not kept, only the decisions:

- **Aquarius (systems):** reuse the data files — the promotion is a metadata-layer add, not a re-snapshot. Adopted (see
  "What promote means").
- **Scorpio (trust):** the lake carries privileged content (`documents`, `notations`, `relationship_logs`). The
  `<project>-exports` bucket stays private (Workload Identity, no public binding) exactly as today; BigLake access is
  IAM-gated to the BI service account, never `allUsers`. Adopted — no public exposure in any option.
- **Capricorn (maintainability):** do **not** hand-roll Avro manifest writing if a maintained Rust Iceberg crate
  (`iceberg-rust`) covers the metadata spec; the writer commit evaluates it before writing manifest serialization by
  hand. Recorded as the first task of the follow-up.
- **Cancer (beginner's mind):** the `dt=<date>` rename is the only change visible to a reader; call it out so a future
  reader isn't surprised the directory shape shifted. Done (Partitioning).
- **Sagittarius (big picture):** deriving the lake from the shipped Surreal schema closes the archive-coverage gap
  without another table registry. Adopted (Tables).

## Writer follow-up — status

1. ~~Evaluate `iceberg-rust`~~ — **done**: chosen (see Evaluation outcome above); not hand-rolled.
2. ~~Restore the SurrealDB analytical snapshot source.~~ **Done**: every `DEFINE TABLE` is discovered from the shipped
   schema and read through one generic query path.
3. ~~Emit `metadata/` (table metadata JSON + manifest list + manifest), append-only~~ — **done** as a reusable
   writer (`archives::iceberg::author_snapshot`, unit-tested for metadata round-trip + snapshot-log chaining), and
   **already wired** for the telemetry tables via the `iceberg_telemetry` workflow phase. Calling it from the primary
   SurrealDB entity-table snapshot phase is the remaining wiring step.
4. Rename the data partition dir to `dt=<date>`. **Pending** (the writer is unpartitioned in v1, so this is cosmetic
   until identity-partitioning lands).
5. Wire BigLake (or external tables, per the operator's call) and add a smoke query. **Pending** (machine-bound; the
   one offline-unverifiable part — needs a live BigQuery to confirm the authored metadata is BigLake-readable).
6. Surface per-table Iceberg snapshot ids in the nightly diagnostic email. **Pending** (with the wiring in step 3).
