# SurrealDB operational archives

The scheduled `surreal-archive` Kubernetes CronJob writes a recoverable SurrealQL artifact. It is separate from the
analytical Parquet/Iceberg `archives` lane.

## Storage and retention

Cloud deployments set `NAVIGATOR_SURREAL_ARCHIVES_BUCKET=neon-law-archives-staging`. Keys are point-in-time selectable:

```text
surreal-backups/<namespace>/<database>/<utc-timestamp>-<uuid>.surql
```

The bucket carries no lifecycle rule — objects are kept indefinitely — and stays private. The ten-year retention and
Coldline transition belong to the separate analytical Iceberg bucket instead; see
[`iceberg-archive.md`](iceberg-archive.md). The `workflows-service` workload identity needs `roles/storage.objectUser`
on the archives bucket; that IAM binding is an operator action.

## Restore drill

```bash
cargo run -p cli -- ops surreal-archive restore-drill \
    --key surreal-backups/navigator/navigator/<timestamp>-<uuid>.surql
```

The drill imports into a fresh `restore_<uuid>` namespace, applies `store/src/schema/navigator.surql`, and verifies
`schema_version:current`. It then reconciles every table's row count to the source and removes the scratch namespace. A
failure exits non-zero so Kubernetes records a failed Job. The GKE `SurrealArchiveFailed` managed-Prometheus rule alerts
firm ops on a failed Job or a missing successful run for 26 hours; configure its critical notification channel in the
deployment project.

For local verification, source `.devx/env`; the archive lane falls back to the worktree's Garage exports bucket:

```bash
set -a; source .devx/env; set +a
cargo run -p cli -- ops surreal-archive export
cargo run -p cli -- ops surreal-archive restore-drill --key <printed-key>
```
