---
title: "Data Export"
description: "A Data Export is a snapshot of SurrealDB tables written as Parquet and Iceberg metadata for BigQuery."
---

A snapshot of one or more SurrealDB tables, written to Parquet (and Iceberg metadata) on a dedicated GCS bucket,
consumed by BigQuery via BigLake external tables. The [`archives`](../../archives/) crate owns the writer, exposed as
the `Archives` Restate workflow hosted by the `workflows-service` worker (all workflows live there). The
[`cron-archives-trigger.yaml`](../../examples/deploy/k8s/exports/cron-archives-trigger.yaml) CronJob fires nightly at
02:00 Pacific to start one invocation; the workflow runs the snapshot (and, when configured, a GCP cost-by-service
summary written as the `gcp_cost` table) as durable steps, then posts a diagnostic summary (snapshot outcomes, cost
summary, BigQuery query template, Restate invocation link) through the worker's `SLACK_WEBHOOK_URL` notifier.

Disambiguates from the deploy **source export** — that ships git bundles of HEAD to `gs://YOUR_PROJECT_ID-source/` for
repo distribution. Two buckets, two flavors of "export," one shared word.

- Crate: [`archives/`](../../archives/) Bucket: `gs://YOUR_PROJECT_ID-exports/iceberg/<table>/`
