# DocuSign e-signature — setup, signing flow, and production cutover

How Neon Law Navigator sends a retainer for signature, how a client signs it inside the portal, and how each cloud
deployment receives an isolated DocuSign attachment. Local sandbox values use a gitignored `.env`; cloud values live in
that deployment's `deployments/<name>/secrets.enc.yaml` per [`deployment-secrets.md`](deployment-secrets.md). The
environment-variable convention is in [`third-party-integrations.md`](third-party-integrations.md). This page is the
DocuSign specifics.

The signature seam lives under `portal/src/`:
[`signature.rs`](https://github.com/neon-law-source-code/navigator/blob/main/portal/src/signature.rs) (the
`SignatureProvider` trait + the DocuSign and stub impls), JWT-grant auth in
[`docusign_auth.rs`](https://github.com/neon-law-source-code/navigator/blob/main/portal/src/docusign_auth.rs), the
embedded signing route in
[`esign_view.rs`](https://github.com/neon-law-source-code/navigator/blob/main/portal/src/esign_view.rs), and the
completion [webhook
source](https://github.com/neon-law-source-code/navigator/blob/main/portal/src/esignature_webhook.rs). An unconfigured
vendor falls back to the in-process stub, so a fresh checkout boots and self-tests without a DocuSign account.

## One attachment per deployment

The four-deployment topology uses four separately revocable attachments: one demo attachment for staging and three
production attachments for live sites. An attachment consists of the DocuSign account, app/integration key, impersonated
user, RSA keypair, consent grant, REST and OAuth hosts, Connect configuration, HMAC key, and Navigator webhook path
secret.

DocuSign Go-Live copies an approved integration key into production rather than moving it. That permits an app identity
to exist in both tiers, but it does not make the two tiers one credential boundary. Configuration is not copied
automatically.

— [Docusign Go-Live](https://developers.docusign.com/platform/go-live/)

The safe boundary is:

| May be promoted from demo to production | Unique to every deployment attachment |
| --- | --- |
| integration-key app identity | account, base, OAuth, user, RSA, consent, HMAC, webhook |

The RSA keypair does not carry over. The OAuth host also differs — `account-d.docusign.com` for demo and
`account.docusign.com` for production. Do not copy an entire DocuSign block between deployments, even when two
attachments ultimately use a promoted integration-key GUID. See the complete signup matrix in
[`provider-environment-parity.md`](provider-environment-parity.md).

### Testing never burns the production envelope quota

The environment is selected by **which account, base, and OAuth host the credentials point at — not by the app.** So
`cargo test`, CI, and the `#[ignore]` live test use credentials for the **demo environment**, which DocuSign documents
as a free sandbox; see its [Developer account](https://developers.docusign.com/platform/account/) page:

> A Docusign developer account (sometimes referred to as a demo account) enables you to develop and test your app in the
  developer environment … which is isolated from the production environment. … a developer account … provides a free
  sandbox environment … any documents sent are purely for testing and are not legally binding.

So a full end-to-end integration test — render, create the envelope, sign it embedded, reach `completed`, download the
executed documents, and receive the completion webhook — runs entirely against a demo attachment and consumes no
production envelopes. Production traffic and deliberate production smoke tests use only the matching live attachment.

**The rule that protects this:** `cargo test`, CI, and the live test use demo credentials from the local or CI
environment. Never wire production DocuSign credentials into a test or CI path. The production allowance is reserved for
real client retainers and any deliberate, manually run production smoke test.

If a promoted integration-key GUID exists in demo and production, production remains gated by its unique account, API
user, RSA keypair, consent, and HMAC key. Treat the shared GUID as an identifier, never as proof of shared credentials.

## Which templates are signed (template-agnostic send path)

The send path is keyed off the notation's **template code**, not the retainer. `drive_post_questionnaire_workflow`
([`portal::retainer_walk`](https://github.com/neon-law-source-code/navigator/blob/main/portal/src/retainer_walk.rs))
resolves the workflow spec via `workflows::bundled_spec_yaml(code)`, rendering to the generic per-notation storage keys
(`notations/{id}/document.pdf`, `signed-document.pdf`, `certificate-of-completion.pdf`), and resolves the captive signer
from the questionnaire answers when present and otherwise from the notation's bound Person row. Adding a signed template
is a template + spec, not a new handler — the spec just needs the retainer's shape: an `intake_persisted__*` →
`lawyer_review` → `generate_pdf__*_pdf` → `sent_for_signature__pending` chain.

Signed templates today:

- **`onboarding__retainer`** — the firm's engagement agreement; client signs, firm countersigns.
- **`nv__trust`** — the Nevada revocable trust instrument; the settlor signs as `client`, the attorney countersigns as
  `firm`. The trust instrument is valid e-signed (NRS 163.008 — no witnesses or notary required), but any deed funding
  **real property** into the trust must be notarized and recorded as a separate step; the template states this caveat
  and the deed is **not** e-signed here.

Deliberately **not** e-signed: `will__simple` (Nevada wills need two attesting witnesses + a notarized self-proving
affidavit, NRS 133.040/133.050, or the NRS 133.085 qualified-custodian path) keeps its in-person `testator_signature` →
`witnesses` → `notarization` flow; `offboarding__letter` is firm correspondence (firm signature only).

## Authentication: JWT grant

The provider authenticates with **JWT grant** — it signs a short-lived RSA assertion with the firm's integration key and
impersonated user, exchanges it for an access token, and caches that token (re-minting 300 s before expiry). A static
`DOCUSIGN_ACCESS_TOKEN` is kept only as a local/demo fallback. The integration key is the JWT `iss`; the OAuth secret is
*not used* by JWT grant — supplying the secret where the integration key belongs yields `issuer_not_found`.

**Why JWT grant, not Authorization Code / PKCE.** PKCE is an extension of the *interactive* Authorization Code flow: it
needs a human at a browser to log in, uses a one-time `code_verifier`/`code_challenge` (random strings, not the RSA
keys), and still performs a token exchange. For a server that sends envelopes unattended that is strictly worse. JWT
grant is the server-to-server flow built for exactly this case, and it is already minimal: one cached token exchange,
re-minted before expiry, no human in the loop after the one-time consent. There is no DocuSign mode that signs each REST
call with the RSA key directly — every call needs a Bearer token, so the private key's only job is to mint that token.
The integration key + user id + account id are required regardless of grant type: every call is scoped to an account and
user.

Required env (canonical `DOCUSIGN_*` names — same names in `.env` for sandbox and `.env.production` for prod):

- `DOCUSIGN_INTEGRATION_KEY` — the app's Integration Key / OAuth client id (the JWT `iss`). **Not** the OAuth secret.
  `DOCUSIGN_USER_ID` — the impersonated API user GUID (the JWT `sub`); the "API Username", not the email.
  `DOCUSIGN_ACCOUNT_ID` — the API Account ID GUID. `DOCUSIGN_PRIVATE_KEY` — the RSA private-key PEM whose public half is
  registered on the app. `DOCUSIGN_BASE_URL` — the eSignature REST base; `https://demo.docusign.net/restapi` for the
  sandbox. `DOCUSIGN_OAUTH_BASE` — the OAuth host. A sandbox boot may omit it and take the demo default
  `https://account-d.docusign.com`; a production boot may not. Once `DOCUSIGN_BASE_URL` declares the integration,
  `store::deployment::WEB_REQUIREMENTS` demands it, and `portal::config::enforce_deployment_invariants` separately
  rejects both an empty value alongside a JWT integration key and any value naming the demo host. Production is
  `https://account.docusign.com`. `DOCUSIGN_SIGNER_EMAIL` — the firm countersignature inbox and the live test's signer.
  `DOCUSIGN_HMAC_KEY` — the DocuSign Connect HMAC key used to verify completion webhooks (see below).

## Sandbox setup (one-time)

The sandbox/developer account is **permanent and free** — watermarked, non-binding envelopes that consume no allowance.
It is the only environment used for dev, CI, and the `#[ignore]` live test. Navigate (deep-links redirect): sign in at
`https://account-d.docusign.com`, then **Settings (gear) → Integrations → Apps and Keys**. That page shows the User ID,
the API Account ID, the Account Base URI, and your apps with their Integration Keys + RSA keypair management.

1. **Create the app** (`Add App and Integration Key`) → copy the **Integration Key** → `DOCUSIGN_INTEGRATION_KEY`.
2. **Add an RSA keypair** under the app's Authentication. DocuSign shows the private key once — copy it into
`DOCUSIGN_PRIVATE_KEY` (or register your existing public key so the key already in `.env` matches). 3. Add a **Redirect
URI** to the app — needed only to land the one-time consent click. Use a dedicated, app-controlled path,
`https://www.neonlaw.com/docusign/consent-callback`, kept **distinct from** the OIDC `/auth/callback`
([`portal::oauth`](https://github.com/neon-law-source-code/navigator/blob/main/portal/src/oauth.rs)): JWT grant never
sends an auth code back, so this URI is ceremonial and must not collide with the Google-login callback. `web` serves it
as a small "Consent recorded" confirmation page (exempt from the private-mode gate) so the operator lands on a
confirmation rather than a 404. 4. From **My Account Information** copy the **API Account ID** → `DOCUSIGN_ACCOUNT_ID`
and the **User ID** GUID → `DOCUSIGN_USER_ID`. 5. **Grant one-time consent** — open this in a browser logged into the
sandbox and click **Allow** (substitute the integration key + a registered redirect):

   ```text
   https://account-d.docusign.com/oauth/auth?response_type=code&scope=signature%20impersonation&client_id=KEY&redirect_uri=REDIRECT
   ```

   JWT grant returns `consent_required` until this is done. Consent is scoped to the **(integration key × impersonated
   user)** pair, so sign in as the *same* user whose GUID is in `DOCUSIGN_USER_ID`. If `consent_required` persists
   *after* a successful Allow, the cause is almost always a user mismatch: one email can have multiple DocuSign
   memberships (e.g. a demo and a production account, or two demo accounts), and consent was recorded for the wrong user
   id. Confirm the **User ID** at the top of Apps and Keys equals `DOCUSIGN_USER_ID`, and that the consent browser is
   logged into the sandbox account that owns the app — not the production account.

## Running the live test (Phase 0 grounding)

The `#[ignore]` [sandbox
test](https://github.com/neon-law-source-code/navigator/blob/main/server/tests/docusign_sandbox.rs) mints a JWT token,
creates a real sandbox envelope with an anchored signature tab, and requests an embedded recipient-view URL. It
self-skips when the env is absent, so it is safe in the default suite.

```bash
set -a && source .env && set +a
cargo test -p server --test docusign_sandbox -- --ignored --nocapture
```

Success prints `created sandbox envelope <id>` and `embedded signing URL: https://…`. Common errors map directly:

- `issuer_not_found` — wrong integration key (likely the OAuth secret). `consent_required` — redo the consent grant.
  `invalid_grant` / `no_valid_keys` — the private key is not the one registered on the app.
  `USER_DOES_NOT_BELONG_TO_ACCOUNT` — the user / account pair does not match.

What the live run still cannot automate (no API auto-signs an envelope): driving the ceremony to `completed`,
downloading the executed documents, and capturing a real Connect completion/decline payload plus the
`X-DocuSign-Signature-1` header. Those steps ground the webhook + HMAC and must be run by hand against the Connect log.

## Client delivery: captive vs emailed

Each notation carries a `delivery` column (`m20260708_add_delivery_to_notations`) that selects, per matter, how the
client recipient is addressed when the single send path builds the signature manifest. The firm always countersigns
second (`routingOrder` 2) as a non-captive recipient — it receives the usual emailed link — regardless of `delivery`.

- **`embedded`** (the default; the standalone retainer walk) — the client is a **captive** recipient: the manifest sets
  `client_user_id` (derived from the notation), so DocuSign suppresses the signing email. Because no email goes out, a
  recipient-view URL is the only door. `GET /lawyer/notations/:id/sign`
  ([`portal::esign_view`](https://github.com/neon-law-source-code/navigator/blob/main/portal/src/esign_view.rs)) mints
  one via `SignatureProvider::create_recipient_view`, which POSTs `envelopes/{id}/views/recipient` and matches the
  recipient on the email, userName, and clientUserId triple. It **redirects the browser to it**. The ceremony runs on
  DocuSign's own site; Navigator does not frame it. The URL expires in minutes, so it is minted fresh per request. The
  stub returns a deterministic URL in dev/KIND. This fits an in-office signing or a logged-in portal session.

  The signer may never come back — they close the tab, or finish on their phone — so **nothing depends on the return
  trip**. The completion webhook below is the authoritative path; the redirect's `return_url` only decides where a
  signer who does return lands. The column keeps the name `embedded` because it selects the *captive recipient* model,
  which is what DocuSign calls it, not a rendering choice on our side.
- **`emailed`** (the matter-open form) — the client is **non-captive**: the manifest omits `client_user_id`, so DocuSign
  emails the client a signing link they open from their own inbox. This is the right experience for a client whose
  matter an admin opens from the "new project" page (`POST /app/projects` with "Send retainer for signature"): that
  client is not in the room and has no portal session yet, so a captive embedded recipient would leave them with nothing
  to sign. Same send path, same `send_for_signature` call — only the recipient's captive flag differs.

## Completion webhook + HMAC

DocuSign Connect POSTs to `/webhook/esignature/:secret`. The handler verifies the raw-body HMAC
(`X-DocuSign-Signature-1`) **before** parsing, classifies the event (`completed` → `signature_received`;
`declined`/`voided` → `signature_declined`; everything else → 200 no-op), signals the workflow, and on completion
archives the signed PDF + Certificate of Completion to object storage (best-effort).

> **Production readiness gate.** The prod `DOCUSIGN_HMAC_KEY` is currently a generated placeholder (a boot invariant in
  [`portal/src/config.rs`](https://github.com/neon-law-source-code/navigator/blob/main/portal/src/config.rs)). E-sign is
  safe to run, but **not client-ready** until DocuSign Connect on the production account is configured with an HMAC key
  and the matching value is set in the prod Secret.

## Production cutover (Phase 2)

Each production deployment needs a production-capable eSignature account and an integration approved or promoted through
Go-Live. Configuration does not copy; set up and prove each production attachment separately:

1. **Promote + prod auth.** Complete Go-Live, then on `account.docusign.com` add a **production RSA keypair**
   and **grant consent** for that attachment's production user. The OAuth host becomes `account.docusign.com`; discover
   the account's assigned REST base from `/oauth/userinfo`.
2. **Production secrets (`deployments/<name>/secrets.enc.yaml` → Secret Manager → projected Secret).** The identifiers
   (`DOCUSIGN_INTEGRATION_KEY`, `DOCUSIGN_USER_ID`, `DOCUSIGN_ACCOUNT_ID`, `DOCUSIGN_BASE_URL`, `DOCUSIGN_OAUTH_BASE`)
   accompany that row's `DOCUSIGN_PRIVATE_KEY` and `DOCUSIGN_HMAC_KEY`. Use the `ship` pre-deploy Secret check.
3. **DocuSign Connect (production account).** Configure
   `https://<deployment-host>/webhook/esignature/<secret>`, subscribe to envelope **completed**, **declined**, and
   **voided**, and enable HMAC with that attachment's key.
4. **Deploy + verify.** `ship` rolls the service images at one published tag, confirms the boot invariant passes, and
   round-trips a real envelope through the prod webhook.

## Related

- [`third-party-integrations.md`](third-party-integrations.md) — the per-deployment provider convention this follows.
  [`.env`](https://github.com/neon-law-source-code/navigator/blob/main/.env.example) is the canonical per-variable
  reference (JWT-grant preferred, static fallback).
