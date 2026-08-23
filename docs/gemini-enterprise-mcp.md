# Gemini Enterprise — Neon Law Navigator MCP server

How to expose Neon Law Navigator's `/mcp` endpoint to **Gemini Enterprise** so the Workspace's LLMs can call its tool
catalog (today: `aida_create_person`, `aida_show_person`, `aida_list_jurisdictions`) during chat sessions, with no new
identity provider to operate. All tool names are namespaced under the `aida_` prefix.

The endpoint serves a **narrower catalog than the firm has**. Three tools — `aida_create_notation`,
`aida_answer_notation`, and `aida_send_welcome_email` — are supervised acts: they email a client, or create or answer a
Notation, which is a binding legal artifact. `mcp::tools::requires_confirmation` classifies them, and MCP has no
`input-required` state to pause in, so `/mcp` withholds them from `tools/list` and refuses one named anyway rather than
simulating an approval it cannot collect. Those acts are performed in `/app`, where a human approves them and the
approval is recorded against the matter. Reading the catalog and finding three tools missing is the design, not a
registration fault.

This doc is the **setup** story — agent card, OAuth, registration. For the **runtime** behavior once a request lands —
where AIDA pauses for a yes/no authorization and how a tool failure's reason reaches the user — see
[`aida-a2a-interaction.md`](aida-a2a-interaction.md).

The auth boundary is **in-app Google OAuth token validation** in the `web` pod
(`portal::google_oauth::require_google_oauth`). Gemini Enterprise sends a standard OAuth 2.0 access token; the pod calls
Google's tokeninfo endpoint to validate the `aud`, `email`, and `email_verified` claims. Same Workspace identity that
signs into `/app`. No new tokens to rotate, no extra IdP.

## Architecture

```text
Gemini Enterprise (OAuth client registered with Workspace)
   │   Authorization: Bearer <Google opaque access token, ya29.*>
   ▼
Global External HTTPS LB (www.your-domain.example)
   │   path-routed: /mcp → navigator-web-mcp Service
   │                /*   → navigator-web Service (public)
   ▼
web Pod  (same pods, two Services pointing at them)
   ▼
portal::google_oauth::require_google_oauth
   │   GET https://oauth2.googleapis.com/tokeninfo?access_token=…
   │   validates aud / azp ∈ GOOGLE_OAUTH_CLIENT_IDS allowlist
   │   validates email_verified == true
   │   validates email ends with @GOOGLE_OAUTH_REQUIRED_HD
   │   populates AuthClaims { sub: email, roles: ["lawyer"] }
   ▼
require_policy (embedded Rego)  →  /mcp handler  →  tools/call
```

In KIND / local dev `GOOGLE_OAUTH_CLIENT_IDS` is unset, so `require_google_oauth` is a pass-through and `require_auth`
handles the Bearer-JWT path — the existing test harness keeps working.

That pass-through is a local-dev shape only. `GOOGLE_OAUTH_CLIENT_IDS` is a boot invariant on every deployed environment
(`store::deployment::WEB_REQUIREMENTS`), so a staging or production `web` refuses to start without it rather than serve
`/mcp` with no token validation — and, because no `Principal` reaches the tools, with AIDA's per-Project scope checks
silently skipped.

**Why this rather than Identity-Aware Proxy?** IAP requires JWT-shaped ID tokens (`eyJ...`), but Gemini Enterprise's
Custom MCP Server data store sends opaque OAuth 2.0 access tokens (`ya29....`) that IAP rejects with the message
`"Invalid IAP credentials: Unable to parse JWT"`. Validation runs in-process instead; the BackendConfig keeps
`iap.enabled: false` as scaffolding.

## Source documentation

Copy/paste these URLs (long, fenced so the markdown linter doesn't chase the line-length rule):

```text
Gemini Enterprise — custom MCP server data store (canonical setup):
  https://docs.cloud.google.com/gemini/enterprise/docs/connectors/custom-mcp-server/set-up-custom-mcp-server
Gemini Enterprise — write effective MCP server descriptions:
  https://docs.cloud.google.com/gemini/enterprise/docs/connectors/custom-mcp-server/writing-mcp-server-descriptions
Gemini Enterprise Agent Platform — remote MCP server:
  https://docs.cloud.google.com/gemini-enterprise-agent-platform/reference/use-agent-platform-mcp
Gemini Enterprise Agent Platform — Agent Gateway overview:
  https://docs.cloud.google.com/gemini-enterprise-agent-platform/govern/gateways/agent-gateway-overview
IAP — for agents overview:
  https://docs.cloud.google.com/iap/docs/agent-overview
IAP — enabling on GKE:
  https://docs.cloud.google.com/iap/docs/enabling-kubernetes-howto
IAP — signed headers howto:
  https://docs.cloud.google.com/iap/docs/signed-headers-howto
MCP — authorization specification (draft):
  https://modelcontextprotocol.io/specification/draft/basic/authorization
```

## One-time setup

Run as a project owner of `YOUR_PROJECT_ID`. Steps 1, 4, and 6 are automated by the `navigator` CLI; the others are
deliberately manual because they're either Cloud Console clicks (OAuth consent screen) or write-then-`kubectl-apply`
cycles where the operator should review the YAML diff.

### 1. Enable the IAP API

```bash
gcloud services enable iap.googleapis.com --project=YOUR_PROJECT_ID
```

(Already done if `navigator ops gcp setup` ran on this project — the
[`services::enable_services`](../cli/src/devx/gcp/services.rs) pipeline enables ~30 APIs, IAP among them.)

### 2. OAuth consent screen

Cloud Console → APIs & Services → OAuth consent screen. User type **Internal** (Workspace org only). Application name
`Neon Law Navigator`, support email `support@neonlaw.com`. This is one click in the UI; there's no REST endpoint that
creates the consent screen itself.

The BackendConfig under `examples/deploy/k8s/gke/iap/backendconfig.yaml` uses the **Google-managed** OAuth client mode
(no `oauthclientCredentials` stanza), so IAP auto-provisions the client when the consent screen exists. No
client_id/secret to store anywhere.

### 3. Apply the overlay

```bash
kubectl kustomize --load-restrictor=LoadRestrictionsNone examples/deploy/k8s/gke \
    | kubectl apply -f -
```

(The `--load-restrictor=LoadRestrictionsNone` flag is needed because `examples/deploy/k8s/gke/kustomization.yaml`
references the shared base under `k8s/base/`, which trips Kustomize's default security boundary.)

Wait for the GKE Ingress controller to provision the global HTTPS LB (1–5 minutes). You can watch it with:

```bash
kubectl -n navigator describe ingress navigator-web-gke
```

If you see `Translation failed: ... could not find port "9080"`, the Ingress is pinned to a stale `workflows-service`
port. Fix in `examples/deploy/k8s/gke/ingress/ingress.yaml` (current correct port is `9081`) and re-apply.

### 4. Add the OAuth client to the allowlist

Whatever OAuth client the Gemini Enterprise data store registers against (or has Google auto-mint), add its full ID to
the `GOOGLE_OAUTH_CLIENT_IDS` env in `examples/deploy/k8s/gke/patches/web-env.yaml`. The value is a comma-separated
list. Re-apply and roll:

```bash
kubectl kustomize --load-restrictor=LoadRestrictionsNone examples/deploy/k8s/gke \
    | kubectl apply -f -
kubectl -n navigator rollout status deployment/navigator-web
```

The pinned list for this project is in `cloud/README.md`.

### 5. Access control: the `hd` claim is the gate

`GOOGLE_OAUTH_REQUIRED_HD=neonlaw.com` on the pod means every `/mcp` call must come from a token whose email ends with
`@neonlaw.com` AND has `email_verified: true`. That's the lawyer allowlist — no per-user IAM binding needed. To add a
new lawyer, all they need is a Workspace account in the org; once they OAuth-consent inside Gemini Enterprise, their
access token's email matches and the call succeeds.

Equivalent gcloud (for reference):

```text
gcloud iap web add-iam-policy-binding \
    --resource-type=backend-services \
    --service=navigator-web \
    --member="group:lawyer@neonlaw.com" \
    --role=roles/iap.httpsResourceAccessor \
    --project=YOUR_PROJECT_ID
```

The Gemini Enterprise client_id binding goes through the same command, just with a different `--member`. It's added in
step 8 below, after the Gemini console hands you that client_id.

## Register the MCP server in Gemini Enterprise

Now the user-facing part. The canonical Google docs are `set-up-custom-mcp-server` and `writing-mcp-server-descriptions`
(linked in the Source documentation section above).

### 7. Create the data store

In the Cloud Console: **Gemini Enterprise → Data stores → Create data store**. Search for **Custom MCP Server** in the
source picker (it's marked "Preview"). Click **Add MCP server**.

Fill the form:

- **MCP Server URL**: `https://www.your-domain.example/mcp` **Authorization URL**:
  `https://accounts.google.com/o/oauth2/v2/auth` **Token URL**: `https://oauth2.googleapis.com/token` **Client ID** /
  **Client Secret**: the Gemini Enterprise UI walks you through provisioning these against the same Google Workspace
  org. The OAuth consent screen from step 2 is what Gemini Enterprise's flow consents against.
- **Scopes**: `openid email` **Data connector location**: `us-west4` (matches the rest of the stack) **Data connector
  name**: `navigator-crm` **MCP Server Description**: paste the block in step 9 below.

Click **Login** and complete the Google sign-in. Gemini Enterprise performs the authorization-code exchange, lands an ID
token, and stores the credentials.

### 8. Allowlist Gemini Enterprise's OAuth client in the pod

After step 7, Gemini Enterprise will use either an OAuth client you specified in the form or one auto-minted in your
project. Whichever it is, append the client ID to the `GOOGLE_OAUTH_CLIENT_IDS` env in
`examples/deploy/k8s/gke/patches/web-env.yaml` (comma-separated list), then re-apply and roll the deployment:

```bash
kubectl kustomize --load-restrictor=LoadRestrictionsNone examples/deploy/k8s/gke \
    | kubectl apply -f -
kubectl -n navigator rollout status deployment/navigator-web
```

The currently-accepted clients are listed in `cloud/README.md` under "Live: in-app Google OAuth validation on `/mcp`".

(IAP-style IAM bindings are NOT used here — `portal::google_oauth` validates tokens directly via Google's tokeninfo
endpoint. The `navigator-web-mcp` BackendConfig is kept with `iap.enabled: false` as scaffolding; if a future caller
sends ID tokens, flip that flag and start using `navigator ops gcp iap grant` again.)

### 9. The MCP Server Description

This text is the only thing Gemini Enterprise's planner sees about the server. Be specific about what to call, when, and
what NOT to do. Markdown supported.

```markdown
## Neon Law Navigator CRM (Neon Law / Neon Law Foundation)

The Neon Law Navigator CRM is the firm's customer-relationship system.
It is the source of truth for **Person** records — clients,
prospects, opposing counsel contacts, and Foundation correspondents.

### When to call

- The user mentions a new person by name and at least one
  contact channel (email or phone) and intent to "add", "create",
  "record", "save", "intake", or "log" them.
- A chat extracts a person from an inbound email or note and the
  user confirms the firm should track them.

Ambiguous-but-yes examples:

- "Let's get Maya Patel into our records, her email is
  maya@example.com" → call `aida_create_person`.
- "I just met Diego Romero, diego@example.com, after the
  Foundation workshop" → call `aida_create_person`.

For read-back: "show me Maya's record" or "look up
maya@example.com" → call `aida_show_person` with that email (or
any case-insensitive substring of name and/or email — partial
fragments work, the tool returns up to 50 matches sorted by name).

For listing valid jurisdictions: "what states can we organize an
entity in?" or "give me the code for Nevada" → call
`aida_list_jurisdictions` (no arguments — returns every
jurisdiction in one shot).

### When NOT to call

- The user has not provided an email address for a *create*. Every
  Person needs one; without an email, ask the user for it before
  calling `aida_create_person`.
- The user wants to track a company / Entity / trust. Those are
  separate records and this server does not yet expose them.

### Behavior

- Confirm the name and email back to the user verbatim before
  calling. Misspelled emails are the #1 cause of orphan records.
- After a successful call, surface the returned `id` so the user
  can reference the new Person in follow-up messages.
- On error, do not retry automatically. Show the user the error
  and ask whether to retry with corrected inputs.
```

### 10. Verify in the default Gemini chat (no custom agent needed)

The data store's actions are automatically available to the default Gemini Enterprise chat — **you do not need to build
a custom Agent Designer / Agent Engine / Dialogflow / A2A agent**. Confirmed live on 2026-05-23: a prompt to the default
chat ran `aida_create_person` end-to-end and the row landed in the store.

1. In the data store's **Tools / Actions** tab, click **Reload custom actions**. `aida_create_person`,
   `aida_show_person`, and `aida_list_jurisdictions` should appear. Toggle them on (Google ships custom actions disabled
   by default).
2. Open the Gemini Enterprise web app (`vertexaisearch.cloud.google.com/.../r`). Pick the default chat or any of the
   pre-built agents.
3. Prompt:

   > Add a person to the CRM: Test User, test+verify@neonlaw.com.

4. Refresh `https://www.your-domain.example/admin/people` — the new row should appear with the timestamp
   matching the chat call. That surface is Owner/Admin-only, so sign in as one to read it back.

If you want a *purpose-built* agent (custom instructions, model, or workflow) on top of the data store, your Gemini
Enterprise SKU must include Agent Designer (no-code). If only Agent Engine, Dialogflow CX, A2A, or Marketplace appear in
the create-agent dialog, the no-code path isn't licensed for your tenant — the default chat is the working substitute.

## Operational notes

- **Token expiry**: Gemini caches the user's OAuth access token for ~1 hour. If chats start failing for a
  previously-working user, they probably need to re-consent (open the data store config and click **Login** again to
  refresh tokens).
- **Audit**: every Gemini-initiated call lands in Cloud Logging with `resource.type=http_load_balancer` and a request
  URL matching `/mcp`. Filter by user agent `python-httpx` to isolate Gemini's calls from manual curl tests.
- **Pod-side diagnostics**: failures emit a `portal::google_oauth: tokeninfo rejected token` warn line with the
  specific reason. Possible reasons: `aud not in allowlist` means the OAuth client needs to be added to
  `GOOGLE_OAUTH_CLIENT_IDS`, and `email-domain mismatch` means the Workspace user is outside the value of
  `GOOGLE_OAUTH_REQUIRED_HD`.
- **Local KIND**: `GOOGLE_OAUTH_CLIENT_IDS` stays unset, the middleware is a pass-through, and the Bearer-JWT path
  through `require_auth` remains the gate. Gemini Enterprise can only reach the prod-hosted endpoint — there is no local
  equivalent.

## Common pitfalls

### Stale hostname in the data store URL

If "Refresh tools" in the Gemini Enterprise Console fails with "can't load tool calls" — and the user reports OAuth
itself succeeded — **check the LB access log first**, not the pod log:

```bash
gcloud logging read \
  'resource.type="http_load_balancer" AND httpRequest.requestUrl=~"/mcp"' \
  --project YOUR_PROJECT_ID --freshness=15m \
  --format='value(timestamp,httpRequest.requestMethod,httpRequest.status,httpRequest.requestUrl,httpRequest.userAgent)'
```

**Zero hits** during the failed refresh window means the request never left Google's network — almost always because the
**MCP Server URL** field in the data store config points at a hostname that no longer resolves. Gemini Enterprise caches
the URL, so a stale hostname fails DNS client-side: the Console reports "successfully authenticated" (the OAuth dance
runs against Google's own servers) but the subsequent `tools/list` call never reaches us.

**Fix**: open the data store config, edit the URL to `https://www.your-domain.example/mcp`, save, click **Refresh
tools** again. The next LB log entry should be a `POST 401` from `python-httpx/<version>` — the 401 is expected on the
first call because OAuth scopes get re-acquired; subsequent calls land at 200 once a valid token is cached.

### Hits at the LB, 401 from the pod

If LB logs *do* show traffic but every request returns 401:

- Check the warn line on the pod
  (`kubectl -n navigator dev logs -l app=navigator-web -c web --tail=200 | grep google_oauth`).
- `aud=… azp=… not in allowlist` — copy the `aud` value and append it to `GOOGLE_OAUTH_CLIENT_IDS` in
  `examples/deploy/k8s/gke/patches/web-env.yaml`. Re-apply and roll.
- `email-domain mismatch` — the Workspace user is outside `@neonlaw.com`. Either add them to the org or change
  `GOOGLE_OAUTH_REQUIRED_HD` (the former is almost always correct).

### OPTIONS preflight 401 (browser-direct callers only)

`OPTIONS /mcp` returns 401 with no CORS headers because `require_google_oauth` runs ahead of any CORS layer. This only
matters if a future caller invokes `/mcp` directly from a browser (Gemini Enterprise is server-to-server, so it
doesn't); if you add such a caller, add a `tower_http::cors::CorsLayer` ahead of the auth middleware and short-circuit
`OPTIONS` in `require_google_oauth`.

## What this is NOT

- Not the full MCP Authorization spec (RFC 9728 Protected Resource Metadata + RFC 8414 Authorization Server Metadata).
  Gemini Enterprise's custom-MCP-server connector relies on the registered OAuth fields, not on spec-driven discovery.
- Not IAP-gated at the LB. We tried that; IAP rejects Gemini's opaque `ya29.*` access tokens. See the architecture
  section for the pivot story.
- Not a public API. The `GOOGLE_OAUTH_REQUIRED_HD=neonlaw.com` enforcement and embedded Rego `lawyer`-role rule mean
  only Workspace users in the org can invoke tools.
- Not self-service A2A onboarding. The agent card at `/app/api/aida.json` sits under the private `/app/api` prefix and
  requires a session, so a client cannot read it to discover the security schemes. That costs nothing here — the
  registered OAuth fields above are what this connector uses — but a standards-based A2A client needs those details
  handed over out of band. See [`access-model.md`](access-model.md) for the anonymous allowlist the card is absent from.
