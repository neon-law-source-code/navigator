# Third-party integrations — one provider attachment per deployment

Neon Law Navigator talks to a handful of external services. They fall into two kinds:

- **Binding vendors** perform real, billable, or legally binding actions on the firm's behalf — DocuSign for
  e-signature, Xero for accounting and billing. Every cloud deployment receives its own provider attachment and
  credentials. Staging attachments point only at vendor sandboxes; production attachments point only at live accounts. A
  vendor tenant may contain more than one attachment when that is the provider's native isolation seam, but keys, users,
  webhooks, and data partitions are never copied between deployments.
- **Platform services** are the cloud infrastructure the app runs on — durable execution, object storage, the database,
  identity, the agent-router LLM, and outbound/inbound email. They follow the same deployment boundary: one database,
  bucket set, OAuth client pair, Restate environment, mail credential, and webhook configuration per deployment.

The [full catalog](#current-integrations) below lists every external service the application code itself dials. Purely
operational layers that sit *above* the env-var interface — the SOPS-encrypted `deployments/` tree (secret values),
DNSimple (DNS) — are deliberately out of scope here: they are not code dependencies, and a fork can swap them freely.

## Why one attachment per deployment

- **No legal or financial weight in dev.** A test envelope or a draft invoice created against the sandbox account is not
  a binding signature or a real ledger entry. A leaked dev key cannot mint a production signature request.
- **Clean books and clean signers.** Test data stays out of the real accounting ledger and off real signers' inboxes.
- **Fault isolation.** Rotating or revoking one deployment's credential cannot take down another site.
- **Self-testable forks.** An OSS adopter can stand up their own sandbox account and exercise the full flow without
  touching a real account or paying for live API calls.

## Deployment selector and credential separation

The narrow deployment selector is exact: `NAVIGATOR_ENVIRONMENT=dev` selects the one development profile, which local
KIND uses, while exact `production`, empty, or unset selects production, which every hosted deployment uses. `staging`,
`test`, mixed case, whitespace, and every other nonempty value fail configuration parsing. It controls infrastructure
safety checks only.

- `.env` holds the **sandbox** credentials and is auto-loaded on startup, so local dev and `cargo test` run against the
  vendor sandbox by default.
- `.env.production` holds the **production** credentials. It is gitignored by the `.env.*` rule and never committed. To
  run against production locally, source it over the defaults before launching the binary:

  ```bash
  set -a; source .env.production; set +a
  ```

Both sources use the same application variable names (`DOCUSIGN_*`, `XERO_*`, …). The operator also sets
`NAVIGATOR_CREDENTIAL_ENVIRONMENT` to `dev` or `production`; startup rejects a mismatch. Each deployment receives its
own set from its own namespaced Kubernetes Secret; no two deployments share one.

## Firm-owned integration credentials

Notion and other Firm integrations are not deployment-wide credentials. A Firm's Admin DRI writes a typed provider
secret through the Navigator secret boundary; the value is envelope-encrypted with the dedicated runtime KMS key and is
never returned in JSON, logs, traces, or durable payloads. The resolver selects the credential from the Project's
`firm_id`, so a Project cannot fall back to another Firm or to a deployment environment variable. Owner governance may
inspect metadata and appoint the DRI, but Owner is not a secret writer.

The runtime KMS coordinate is `NAVIGATOR_RUNTIME_KMS_KEY`. It must name a dedicated runtime key, never the deployment
configuration key. Staging manifests and operator documentation may describe the workload permission; this repository
does not apply IAM or write cloud state. Provider clients receive a resolved credential through an injected trait and
tests use fakes, so local verification needs no live provider account.

Notion reconciliation uses the explicitly selected `NAVIGATOR_NOTION_DATABASE_ID`. A missing, moved, deleted, duplicate,
or unshared page is an operator-visible repair outcome; the reconciler never silently creates a second page. It writes
the canonical Project code and stable Person IDs while preserving manual Notion fields.

### The Firm integration doors

Four admin-tier operations act on a Firm's own provider resources, and the `navigator` CLI is their only client today:

| Command | Door |
| --- | --- |
| `navigator site projects notion ensure <code>` / `--all` | `POST /app/api/integrations/notion/ensure` |
| `navigator site projects notion reconcile <code>` / `--all` | `POST /app/api/integrations/notion/reconcile` |
| `navigator site projects slack ensure <code>` | `POST /app/api/integrations/slack/ensure` |
| `navigator site projects slack notify <code> --event <kind>` | `POST /app/api/integrations/slack/notify` |

They carry their own noun rather than nesting under `projects`, for the reason `project-surfaces` does: the `projects`
policy rule admits any authenticated caller several segments deep, so a provisioning path nested there would be
policy-reachable by a client even though the handler refuses one. The tier matches `project-surfaces` too — creating or
adopting a Project's external resources is one kind of act — and `--all` sweeps only the Projects visible to the calling
login, never every row in the deployment.

Each response is one outcome slug per Project and nothing else. No page id, no channel id, no provider URL: those are
the Firm-private coordinates this boundary exists to keep on the firm side, and they are recorded on the Project row for
the surfaces entitled to read them. `ensure` reports `created` or `adopted`, so an operator can tell a first
provisioning from a re-run; `reconcile` reports `unchanged`, `repaired`, `missing`, `duplicate`, `conflict`, or
`unavailable`, and only a repair writes. A deployment with no runtime KMS key reports `runtime_not_configured` rather
than falling back to a deployment-wide token — a Project must never reach a credential its Firm did not write.

`slack notify` accepts only the closed event vocabulary (`project_opened`, `project_closed`, `project_reconciled`,
`integration_unavailable`) and derives the message from the kind, so no caller-supplied prose reaches a channel. It
never provisions: a Project with no channel reports `no_channel` so `ensure` stays a deliberate act. `slack ensure`
invites nobody — the adapter takes only provider-issued member ids, Navigator stores none, and it will not turn a
participation row or an email address into an invite, so the Firm's own Slack membership governs who joins.

Normal staging requires real non-production SendGrid and DocuSign demo configuration. Each cloud deployment uses the
matching attachment row described in [`provider-environment-parity.md`](provider-environment-parity.md). Only the
explicit `NAVIGATOR_CI_HARNESS=1` staging test surface may use in-process fakes; production rejects that flag.

## Current integrations

| Service | Purpose | Kind | Env prefix |
| --- | --- | --- | --- |
| DocuSign | E-signature | binding | `DOCUSIGN_*` |
| Xero | Accounting / billing (`ACCREC` invoices) | binding | `XERO_*` |
| Notion | Firm-private Project workspaces | firm-owned | resolved by `firm_id` |
| Slack | Firm-private Project channels and mechanism notices | firm-owned | resolved by `firm_id` |
| Restate Cloud | Durable workflow execution (`workflows-service`) | platform | `RESTATE_*` |
| Google Cloud | Storage, OIDC, archive | platform | `NAVIGATOR_*`, `GOOGLE_OAUTH_*` |
| Vertex AI | A2A agent-router LLM (Gemini Flash in prod) | platform | `NAVIGATOR_GCP_*` |
| SendGrid | Outbound + inbound email | platform | `SENDGRID_*` |

Notes:

- **Xero ↔ Mercury.** Xero reconciles against the firm's bank (Mercury) inside Xero itself. Neon Law Navigator never
  speaks to Mercury — our only integration boundary is the Xero API.
- **Google Cloud is several spec-compliant touchpoints, not one SDK.** Object storage goes through the `cloud`
  crate's `StorageService` trait (GCS in prod, Garage in dev); the store is SurrealDB over `NAVIGATOR_SURREAL_ENDPOINT`
  (Surreal Cloud in prod); OIDC is Google Identity validated against `GOOGLE_OAUTH_*`; Drive REST v3 is import-only,
  never a live store or archive. See [`cloud/README.md`](../cloud/README.md) for the full resource map.
- **Vertex AI is pluggable.** The router is the `portal::agent_router::AgentRouter` trait — `GeminiRouter` (Vertex AI)
  in prod, `NullRouter` in KIND. Swapping to another LLM means a new `impl`, not a new vendor account.

When you add a vendor, define its provider-native staging and production seams, give each cloud deployment a separately
revocable attachment, add a `<VENDOR>_*` block to `.env.example`, and document a real smoke test. Local development may
use a sandbox credential in `.env`; a cloud credential belongs only in its deployment's `secrets.enc.yaml`. A platform
service also needs a stub/local equivalent (Garage, the in-cluster store, the `NullRouter`) so a fresh checkout boots
and self-tests without a cloud account.

## Not in this catalog — and why

A few external-looking things are deliberately absent. They are not third-party SaaS vendors, so the per-environment
account convention does not apply to them:

- **Authorization policy** is compiled into the Navigator web process with the Rust-native Regorus interpreter. It is
  application code, not a vendor account or separately operated service. See [`rego-policy.md`](rego-policy.md).
- **OIDC identity is already the Google Cloud row.** In production, sign-in is **Google Identity** (validated against
  `GOOGLE_OAUTH_*`) — counted under Google Cloud above, not as a separate vendor. **Rauthy** is its **non-production
  stand-in** (the staging/KIND OIDC provider), exactly as Garage stands in for GCS and the in-cluster store for Surreal
  SQL. The identity provider is pluggable and spec-compliant either way.
- **The `deployments/` tree and DNSimple** sit *above* the env-var interface — the repository holds each deployment's
  coordinates and SOPS-encrypted secret *values* (the app reads plain env vars; see
  [`deployment-secrets.md`](deployment-secrets.md)), DNSimple holds DNS records. Neither is a code dependency, and a
  fork can swap both freely.

## Related

- `.env.example` — the canonical per-variable reference; this convention is stated in its top "Conventions" block.
  [`oss-install.md`](oss-install.md) — the install walkthrough's env-configuration step.
  [`env-driven-devx.md`](env-driven-devx.md) — the broader "one config surface, three audiences" env philosophy.
