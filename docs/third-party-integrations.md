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

Normal staging requires real non-production SendGrid and DocuSign demo configuration. Each cloud deployment uses the
matching attachment row described in [`provider-environment-parity.md`](provider-environment-parity.md). Only the
explicit `NAVIGATOR_CI_HARNESS=1` staging test surface may use in-process fakes; production rejects that flag.

## Current integrations

| Service | Purpose | Kind | Env prefix |
| --- | --- | --- | --- |
| DocuSign | E-signature | binding | `DOCUSIGN_*` |
| Xero | Accounting / billing (`ACCREC` invoices) | binding | `XERO_*` |
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
