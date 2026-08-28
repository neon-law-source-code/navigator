# Xero billing — setup, invoice flow, and production cutover

**All accounting originates in Xero.** Navigator does not compute, discount, or raise money: lawyers agree a matter's
price with the client and raise the invoice in Xero directly. This page covers how Navigator reads that ledger — how a
Xero invoice's paid-status is reconciled back into the portal, and how **one custom connection per organisation** keeps
test activity off the live ledger. Local sandbox values use a gitignored `.env`; production values live in that
deployment's `secrets.enc.yaml` per [`deployment-secrets.md`](deployment-secrets.md). The environment-variable
convention is in [`third-party-integrations.md`](third-party-integrations.md). This page is the Xero specifics.

The billing seam lives in [`billing/src/lib.rs`](../billing/src/lib.rs) (the `BillingProvider` trait + the
`XeroBillingProvider` and `StubBillingProvider` impls), client-credentials auth in
[`billing/src/xero_auth.rs`](../billing/src/xero_auth.rs), and the worker-side nightly reconcile in
[`billing-workflows`](../billing-workflows/). The crate is re-exported as `portal::billing` so the app and tests share
one trait. An unconfigured vendor falls back to the in-process stub, so a fresh checkout boots and self-tests without a
Xero account.

## One connection per organisation (the contrast with DocuSign)

DocuSign promotes **one app** across environments — at Go-Live the integration key is *copied*, so demo and production
share a GUID. **Xero is the opposite.** A Xero *custom connection* is a machine-to-machine app bound to exactly **one
organisation**, so each environment needs its **own** connection:

| Environment | Connection | Organisation | Ledger weight | Cost |
| --- | --- | --- | --- | --- |
| dev / CI | sandbox custom connection | the free **demo company** | none — resets periodically | free |
| production | live custom connection | the firm's real **organisation** | real receivables | $5/mo USD |

Both use the same `XERO_*` variable names — the env *file* is the namespace (`.env` = sandbox, `.env.production` =
live), so no code branches on environment. The connected org is fixed per connection and sent as the `Xero-Tenant-Id`
header on every Accounting API call.

### Testing never touches the live ledger

The demo company is the one free, non-binding target: invoices and contacts created there carry no receivable weight and
are self-cleaning (the demo org resets periodically). That is why dev, CI, and the live grounding test all point at the
demo company — a leaked sandbox secret cannot raise a real invoice against a real client.

## Who raises the invoice: lawyers, in Xero

A matter is a relationship the firm opens, priced bespoke per client. Lawyers agree the price with the client and raise
the **`ACCREC`** (accounts-receivable) invoice **in Xero directly**. No Navigator handler and no Restate workflow raises
one, and no Navigator code applies a discount or otherwise computes what a client owes. Navigator's job is to record the
legal work.

What Navigator does hold is a **read-only mirror**: the `xero_invoice` table carries the Xero `InvoiceID`, reference,
amount, and paid-status for at most one invoice per matter, keyed on `project_id`. That mirror backs the per-project
invoice card in the portal and the "View in Xero" link on the lawyer matter page, so nobody has to open Xero to see
whether a matter is paid. Navigator **never holds client funds, card data, or bank credentials** — Xero reconciles
against the firm's bank (Mercury) itself. The integration boundary is the Xero Accounting API and nothing beyond it.

## Where the price comes from: the matter, agreed per client

There is no catalog and no published price. Every engagement is bespoke: lawyers agree the figure with the client for
that matter and raise the invoice in Xero directly. What was actually billed is the Xero invoice, mirrored read-only
into `xero_invoice` — nothing in Navigator computes, quotes, or anchors a price.

An invoice line that would once have carried a catalog field now takes the firm-wide default. Tagging invoices to a
project and a jurisdiction is a separate, later model; it is deliberately not part of removing the catalog.

## Authentication: client-credentials grant (preferred)

A custom connection authenticates with the OAuth 2.0 **client-credentials** grant — no user, no redirect, no consent
ceremony. [`XeroClientCredentials`](../billing/src/xero_auth.rs) mints a short-lived Accounting API token and refreshes
it itself, so there is no 30-minute token to rotate by hand. Set the client-credentials pair to activate it:

- `XERO_CLIENT_ID`, `XERO_CLIENT_SECRET` — the custom connection's credentials (the secret is shown **once** at
  creation).
- `XERO_TENANT_ID` — the connected org's GUID (`Xero-Tenant-Id` header). Optional for the live test, which can
  auto-discover it from the `/connections` endpoint since a custom connection binds to one org.
- `XERO_SCOPE` — optional; defaults to `accounting.contacts accounting.invoices`.

A static `XERO_ACCESS_TOKEN` is accepted as a fallback for a quick local smoke test, but Xero expires it in ~30 minutes
and it is ignored when the client-credentials pair is set. The real provider activates when `XERO_TENANT_ID` is present
together with **either** the client-credentials pair **or** a static access token; otherwise `web` uses the stub.

## Sandbox setup (one-time) — sign up and create the custom connection

1. **Create a Xero developer account.** Sign up free at [developer.xero.com](https://developer.xero.com/) and sign in to
   **My Apps**.
2. **Have a demo company.** From your Xero account, enable the **Demo Company** (My Xero → "Try the demo company"). It
   is free, pre-populated, and resets periodically — the right target for dev and CI.
3. **Create a custom connection.** In My Apps → **New app** → choose **Custom connection** (the machine-to-machine,
   client-credentials app type). Name it (e.g. `Neon Law Navigator (demo)`), and add the **integrator** email that will
   authorise it.
4. **Select scopes.** Grant exactly `accounting.contacts` and `accounting.invoices`. A custom connection offers only
   granular scopes — the legacy parent `accounting.transactions` is **not** offered, and requesting it fails token
   minting with `invalid_scope`.
5. **Authorise the connection against the demo company.** The integrator opens the authorisation link Xero generates and
   connects it to the **demo company** org. This binds the connection to that one organisation.
6. **Copy the credentials.** From the connection's Configuration, copy the **Client ID** → `XERO_CLIENT_ID` and generate
   the **Client Secret** (shown once) → `XERO_CLIENT_SECRET`. Set `XERO_TENANT_ID` to the demo org's GUID (or leave it
   unset locally and let the live test discover it).

For production, repeat steps 3–6 with a **separate** custom connection authorised against the firm's **live**
organisation (a paid $5/mo single-org app), and put those credentials in the production deployment's `secrets.enc.yaml`.

## Running the live test (grounding)

The live test in [`server/tests/xero_sandbox.rs`](../server/tests/xero_sandbox.rs) mints a real client-credentials token
against the demo-company connection and drives `ensure_contact` twice with the same unique name — the first call
**creates** the contact, the second must **find** it and return the *same* `ContactID`. This is the only test that
catches a regression in our understanding of Xero's API (a wrong `where` predicate, a bad scope, a rejected payload). It
self-skips green when no creds are present and runs only under the explicit `NAVIGATOR_RUN_LIVE_SANDBOX=1` opt-in, so it
never fires on an ambient-credentials `cargo test`:

```bash
set -a; source .env; set +a
NAVIGATOR_RUN_LIVE_SANDBOX=1 cargo test -p server --test xero_sandbox -- --nocapture
```

It reads the CI `XERO_SANDBOX_*` names first, each falling back to the canonical `XERO_*` name, so a local `.env` drives
it without separate sandbox vars.

## Paid-status reconciliation

The invoice is raised in Xero, and its payment status has to come back. The nightly `ReconcileInvoices` workflow
(worker-side, in [`billing-workflows`](../billing-workflows/)) calls `get_invoice` for each mirrored invoice and folds
Xero's paid-status into the `xero_invoice` table, so the per-project invoice card in the portal flips to **Paid**
without anyone re-keying it. This is the only writer of the mirror in normal operation, and it only ever *reads* from
Xero. Like every workflow, it is hosted by `workflows-service` — no per-workflow worker pod.

## Production cutover

1. Create the **live** custom connection (separate from the demo one) authorised against the firm's real organisation.
2. Put its `XERO_CLIENT_ID` / `XERO_CLIENT_SECRET` / `XERO_TENANT_ID` in the production deployment's
   `secrets.enc.yaml` (applied to Secret Manager and projected into the `navigator-web-secrets` Secret), never in
   plaintext source.
3. Confirm the live org grants the same `accounting.contacts accounting.invoices` scopes.
4. Verify with one real invoice raised in the live org that `ReconcileInvoices` mirrors its paid-status onto the
   portal card.

## Related

- [`third-party-integrations.md`](third-party-integrations.md) — the per-environment vendor-account convention and the
  full integration catalog.
- [`docusign-esignature.md`](docusign-esignature.md) — the sibling e-signature integration (one app, two environments).
  [`deployment-secrets.md`](deployment-secrets.md) — how production secrets are rendered.
