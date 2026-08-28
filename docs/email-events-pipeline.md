# Email events pipeline — SendGrid delivery stream to BigQuery

Two halves of the outbound-email audit picture, joined in BigQuery.

- **Request side** — `portal::email`'s `LoggingEmail` writes one `sent_emails` row per attempt. A SendGrid `202` proves
  the message was *accepted*, not *delivered*. Each row now carries `sg_message_id` (SendGrid's `X-Message-Id`), the
  join key to the delivery stream.
- **Delivery side** — SendGrid's Event Webhook POSTs lifecycle events (`processed`, `delivered`, `open`, `click`,
  `bounce`, `dropped`, `deferred`, `spam_report`, `unsubscribe`). The `portal::email_events` handler lands each POST as
  one Snappy Parquet object on GCS — the same Parquet-on-GCS shape the [`archives`](../cloud/README.md) snapshot uses,
  so BigQuery reads it through an external table.

Inbound Parse is a separate callback adapter. In raw mode, `portal::inbound_email` extracts MIME attachments from the
RFC 5322 `email` field, scans each exactly once through the configured cluster-private ClamAV service, and only then
permits clean bytes into lawyer notifications or matter documents. SendGrid spam scoring is not used as a malware
verdict.

## How the join works

At send time `SendGridEmail::build_request_body` stamps top-level `custom_args` — `template_slug` and `person_id` (see
`OutboundEmail::with_person`). SendGrid echoes those keys on every lifecycle event for that message, so the delivery
rows carry `template_slug` / `person_id` directly and the analytics join needs no address parsing:

```text
sent_emails.sg_message_id ── 1:N ── email_events.sg_message_id
email_events.person_id    ─────────  persons.id
```

The admin "Send welcome" path sets `person_id`; workflow-driven sends carry `template_slug` only until `EmailPayload`
threads a person id (a follow-up).

## Object layout

One POST becomes one object:

```text
email-events/data/dt=<YYYY-MM-DD>/<sha256(body)>.parquet
```

- `dt=<date>` is a Hive-style partition (date of the first event in the batch) so a BigQuery external table prunes by
  day.
- The filename is the SHA-256 of the raw request body, so SendGrid's at-least-once retries (it re-POSTs the identical
  body on any non-2xx for 24h) overwrite the same object — file-level idempotency with no dedupe table. `sg_event_id` is
  unique per event for row-level dedupe at query time.

Columns (all nullable `Utf8` except `event_unix_ts` which is `Int64`): `sg_event_id`, `sg_message_id`, `event`, `email`,
`template_slug`, `person_id`, `url`, `reason`, `status`, `timestamp_utc`, `event_unix_ts`, `raw_json`. `raw_json` keeps
the whole event so a field we don't model yet is never lost.

The events land in `web`'s configured storage bucket under the `email-events/` prefix. A deployer who wants delivery
analytics on a separate lifecycle (and IAM) can point a dedicated bucket — e.g. `YOUR_PROJECT_ID-events` — at the prefix
instead; nothing in the handler hard-codes a bucket.

## Operator setup (one-time, machine-bound)

These run on the operator's machine against the live project — `web` issues no BigQuery DDL itself.

### 1. Configure the SendGrid Event Webhook

Point SendGrid's Event Webhook at the path-secret URL and store the secret so prod boot accepts it:

```bash
# Pick a long random token; SendGrid posts to this exact URL.
kubectl -n navigator create secret generic navigator-web-secrets \
  --from-literal=SENDGRID_EVENTS_SECRET="$(openssl rand -hex 24)" \
  --dry-run=client -o yaml | kubectl apply -f -
# In the SendGrid console, set the Event Webhook POST URL to:
#   https://www.your-domain.example/webhook/email-events/<that-token>
```

`SENDGRID_EVENTS_SECRET` is a deployment boot invariant (`portal::config::enforce_deployment_invariants`), so a deploy
without it fails fast rather than serving an unauthenticated endpoint. The ECDSA "Signed Event Webhook" is stronger: it
authenticates the payload, not just the URL.

### 2. Create the BigQuery external table

Reuse the `navigator_bi` dataset and connection from the archives bootstrap (see
[`cloud/README.md`](../cloud/README.md)), then:

```sql
CREATE EXTERNAL TABLE `YOUR_PROJECT_ID.navigator_bi.email_events`
WITH CONNECTION `us-west4.exports`
OPTIONS (
  format = 'PARQUET',
  uris = ['gs://YOUR_PROJECT_ID-assets/email-events/data/*'],
  hive_partition_uri_prefix = 'gs://YOUR_PROJECT_ID-assets/email-events/data',
  require_hive_partition_filter = false
);
```

Substitute the bucket `web` actually writes to (its `NAVIGATOR_GCS_BUCKET` / storage backend). New partitions show up on
the next query — BigLake external tables re-scan their `uris` glob, so there is no refresh step.

## Updating the pinned SendGrid OpenAPI adapter

The vendored SendGrid contract and its adapter template are pinned together with the generated adapter. After
intentionally updating either vendor input, run the regeneration command from a source checkout:

```text
cargo run -p cli -- dev sendgrid-openapi --regenerate
```

The first run calculates the specification and deterministic rendered-adapter hashes before it writes anything. If
either pin is stale, it leaves `workflows/src/email/sendgrid_openapi.rs` unchanged and prints the exact
`EXPECTED_SHA256` and `EXPECTED_ADAPTER_SHA256` replacements for `cli/src/sendgrid_openapi.rs`. Apply both replacements,
then rerun the same command; it writes the adapter and verifies the complete pinned set. Commit the updated vendor
input, both pin constants, and generated adapter together.

## Analytics

Per-template delivery funnel, joined to the request side:

```sql
SELECT
  e.template_slug,
  COUNT(DISTINCT e.sg_message_id)                          AS messages,
  COUNTIF(e.event = 'delivered')                           AS delivered,
  COUNTIF(e.event = 'open')                                AS opened,
  COUNTIF(e.event = 'click')                               AS clicked,
  COUNTIF(e.event IN ('bounce', 'dropped', 'spam_report')) AS problems
FROM `YOUR_PROJECT_ID.navigator_bi.email_events` AS e
WHERE e.dt >= '2026-05-01'
GROUP BY e.template_slug
ORDER BY messages DESC;
```

## Non-goals

- **Live dashboards.** The lake is for analytics; the `sent_emails` table (admin `/app/admin/email-log`) stays the
  operational request-side view.
- **Replacing `sent_emails`.** The store holds the request side; Parquet-on-GCS holds the delivery side; BigQuery joins
  them.
- **Iceberg-managed metadata.** Same deferral as `archives` — `format = 'ICEBERG'` with authored
  `metadata/v<n>.metadata.json` is a later follow-up; the Parquet external table is the v1.
