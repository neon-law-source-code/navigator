# Signed-URL document delivery audit

A defensive audit of the path a filed Project document takes from object storage to a reader: the signed-URL handoff,
the authorization in front of it, the `navigator site pull` lane that materialises bytes on a laptop, the Project-portal
Content Security Policy, the per-Project publisher identity, the audit trail of an issued URL, and the CI session that
verifies pointers against live records. It covers code and configuration this repository owns. Every live check ran
against the staging deployment (`neon-law-stg`, `staging.neonlaw.com`) on 2026-09-12; production was not read. The
equivalent production reads are listed in the pull request that landed this note, for a human to run.

Nothing here names a client, a matter, a real Project code, or a production coordinate. Where an example is needed it
uses `<code>` or the seeded sample matters, which are invented.

## Severity scale

| Severity | Meaning |
| --- | --- |
| High | A reader without a participation row can reach privileged bytes, or one Project can write another's. |
| Medium | Exposure is bounded by a credential or a window that is wider than it needs to be. |
| Low | A hardening or a documentation correction; no exposure found. |
| Informational | A control checked and found in place. Recorded so the next audit starts from evidence. |

No finding is High. The delivery path holds; the corrections are about windows, drift, and a test that proves less than
its name says.

## Summary

| # | Finding | Severity |
| --- | --- | --- |
| 1 | The one-hour project-document TTL buys nothing, because every click mints a fresh URL. | Medium |
| 2 | The signed URL lands in the address bar and history for its whole TTL, and renders inline. | Medium |
| 3 | No bucket in the staging chain is public; unsigned reads are refused. | Informational |
| 4 | Authorization runs before the bytes on the download route; one test proves the wrong layer. | Informational |
| 5 | Committing `asset_id` and `sha256` to a private repository is acceptable, with corrections. | Low |
| 6 | `navigator site pull` writes privileged plaintext to a laptop with no warning and no way to clean up. | Medium |
| 7 | Portal CSP: proxy the bytes same-origin; do not admit the storage origin. **Landed.** | Medium |
| 8 | Publisher isolation holds, but staging runs a hand-made generation the code does not describe. | Medium |
| 9 | A successful signed-URL issuance leaves no audit event; only failures are logged. | Medium |
| 10 | The CI document session is main-only, single-use, and Project-scoped. | Informational |

## Finding 1: the project-document TTL

**Severity: Medium.**

[`portal/src/project_documents.rs:86`](../portal/src/project_documents.rs) sets `SIGNED_URL_TTL` to one hour for project
documents. [`portal/src/documents.rs:37`](../portal/src/documents.rs) sets five minutes for notation PDFs. The comment
above the one-hour constant (lines 58 to 85) is candid that the TTL is the URL's entire security lifetime, and justifies
the hour with a scenario: a lawyer opens the page, is pulled into a call, comes back, and clicks Download.

That scenario does not involve the signed URL. The page links to the Navigator route
`/app/projects/{code}/documents/{doc_id}/download` ([`portal/src/admin.rs:610`](../portal/src/admin.rs)), and the route
mints a new URL on every request ([`project_documents.rs:563`](../portal/src/project_documents.rs)). A lawyer who
returns after an hour clicks the same link, hits the same route with a live session, and receives a fresh URL. The TTL
only has to cover the redirect hop and the start of the transfer, which is the five-minute reasoning the notation route
already uses. GCS validates the signature when the request begins, so a long download does not need a long TTL.

The one case the hour does help is a URL copied out of the browser and pasted elsewhere. That is the case the comment
lists as a leak, not a feature.

**Recommendation.** Drop project documents to the notation figure, five minutes, and move both constants next to each
other so the two routes cannot drift again. Finding 7 makes the constant moot for the portal path; it still governs the
matter show page's Download link.

**Follow-up.** Align `SIGNED_URL_TTL` across the two document routes and rewrite the trade-off comment around the
per-click mint.

**Closed by Finding 7's fix (LAW-22 / ENG-651), more completely than this recommendation expected.** The
project-document route no longer signs anything, so its `SIGNED_URL_TTL` is gone rather than shortened. The
recommendation above assumed the constant would survive to govern "the matter show page's Download link" — it does not,
because that link and the portal's viewer are the same handler, and it now streams for both. Findings 1 and 2 therefore
close for `/app/projects/{code}/documents/{doc_id}/download` entirely: there is no URL to leak, to age, or to land in
history.

The line references in the finding text above are left as they were found on 2026-09-12. This is a dated audit, and
re-pointing them at today's file would make it a description of the present rather than a record of what was examined.

`portal/src/documents.rs`'s five-minute constant for notation PDFs is untouched and still signs. Aligning the two is
moot now that there is only one.

## Finding 2: Referer, history, and the redirect hop

**Severity: Medium.**

The route answers with `Redirect::temporary` ([`project_documents.rs:566`](../portal/src/project_documents.rs)), a `307`
whose `Location` is the signed URL. Three questions.

**What does `referrer-policy` do on the hop?** The site-wide layer sets `strict-origin-when-cross-origin` on every
response with `if_not_present` ([`portal/src/lib.rs:1952`](../portal/src/lib.rs)), and the covering test at
[`server/tests/routes.rs:13916`](../server/tests/routes.rs) pins it. Live on staging, the `303` a document route sends
an anonymous browser carries that header, so the redirect responses carry it too. When the browser follows the
`Location` to the storage origin, the request's referrer is the page that held the link, reduced by that policy to the
Navigator origin alone. Nothing in the path or query of the referring page reaches Google. Referer leakage on the hop is
not a risk here.

**Does the signed URL leak from the storage origin onward?** Only if the document itself navigates somewhere. A PDF
rendered by the browser's viewer does not send a `Referer` on link clicks in the major engines, but that behaviour is
the viewer's, not ours, and was not verified in this audit.

**Where does the URL persist?** Two places this code controls.

- The URL is the final hop of a top-level navigation, so the browser records it in history and shows it in the address
  bar. Chromium and Firefox both keep redirect-chain URLs. Anyone reading that history, or a screenshot of the tab,
  holds the credential until expiry.
- The signed URL carries no `response-content-disposition`, so a PDF renders inline at the storage origin, in a tab
  whose address bar is the credential. The `FsStorage` fallback sets `Content-Disposition: attachment`
  ([`project_documents.rs:665`](../portal/src/project_documents.rs)); the production branch does not. The pinned
  `google-cloud-storage` 0.24.0 `SignedURLOptions` accepts `query_parameters`, which is how a V4 URL carries
  `response-content-disposition`.

**Recommendation.** Sign `response-content-disposition=attachment; filename=...` into the URL so the browser saves the
file instead of navigating to a tab that displays the credential. Finding 7's proxy removes the URL from the browser
entirely and is the durable fix for the portal path.

**Follow-up.** Add the attachment disposition to `signed_url` callers, or accept the proxy in Finding 7 as the fix for
both routes and record the decision beside the TTL rationale.

## Finding 3: bucket public access

**Severity: Informational.** The control is in place.

Checked on 2026-09-12 with `gcloud` pinned to `neon-law-stg`:

- All seven staging buckets (`applications`, `archives`, `assets`, `documents`, `exports`, `logs`, `telemetry`) have
  uniform bucket-level access on and no `allUsers` or `allAuthenticatedUsers` member in any binding.
- `publicAccessPrevention` reads `inherited` on every bucket, and the effective organization policy for
  `constraints/storage.publicAccessPrevention` is empty. So nothing enforces it; the buckets are private because no
  binding makes them public, not because a guardrail refuses one.
- Unsigned `GET`s to the `documents`, `applications`, and `assets` bucket roots and to a well-formed object key under
  each return `403`.

The public asset lane is the app's own `/assets` proxy ([`docs/assets.md`](assets.md)), so even the marketing bucket
stays private at the bucket.

**Recommendation.** Set `publicAccessPrevention: enforced` on every bucket `navigator ops gcp setup` creates, so a
future hand-added `allUsers` binding is refused rather than honoured. Have `ops gcp verify` assert it.

**Follow-up.** Enforce public-access prevention in the bucket provisioning stage and verify it.

## Finding 4: authorization before the bytes

**Severity: Informational.** Authorization runs before the bytes, on every layer.

One correction first. The audit brief names `GET /app/api/projects/{id}/documents/{asset_id}`. That path exists only as
`PATCH` ([`portal/src/api.rs:361`](../portal/src/api.rs)), reconciling a pointer's visibility. The byte-delivery route
is `GET /app/projects/{code}/documents/{doc_id}/download` ([`portal/src/admin.rs:610`](../portal/src/admin.rs)), and it
is the route the CLI's `pull` uses too ([`cli/src/remote.rs:403`](../cli/src/remote.rs)).

The layers on that route:

1. **Session and policy.** The admin sub-router requires an authenticated session and passes the request through the
   embedded Rego policy. The matter-surface rule at [`portal/policy/navigator.rego:81`](../portal/policy/navigator.rego)
   admits every signed-in tier and leaves the participation check to the handler. An anonymous browser is sent to login,
   verified live on staging.
2. **Participation.** The handler resolves the matter by code and calls `store::access::matter_lens`
   ([`store/src/access.rs:168`](../store/src/access.rs)), which is participation-scoped for every tier with no Owner or
   Admin bypass and fails closed when the session carries no person. A caller off the matter receives `404`
   ([`project_documents.rs:535`](../portal/src/project_documents.rs)).
3. **Cross-matter guard.** `load_doc_for_project` ([`project_documents.rs:617`](../portal/src/project_documents.rs))
   loads the asset by id and refuses it when `asset.project_id` is not the matter in the URL. An `asset_id` from one
   matter cannot be fetched under another matter's code, even by a participant of both. Under the client lens it also
   refuses an `internal` asset.
4. **Signed URL.** Only after the three layers pass is a URL signed.

The `PATCH` route is scoped the same way: `can_see_project_as_lawyer` on the matter, then
`store::assets::set_visibility` filters the asset on `project_id` ([`store/src/assets.rs:851`](../store/src/assets.rs)).

**One test proves the wrong layer.** `project_document_download_404s_when_doc_belongs_to_a_different_project`
([`server/tests/routes.rs:14899`](../server/tests/routes.rs)) requests matter A's asset under matter B's code with
`admin_session_cookie()`, a session with no linked person ([`routes.rs:53`](../server/tests/routes.rs)). `matter_lens`
returns `None` on that session before the cross-matter guard runs, so the `404` the test asserts comes from layer 2, not
layer 3. The guard itself is covered directly at the unit level
([`project_documents.rs:756`](../portal/src/project_documents.rs)), and the client-lens refusal is covered end to end at
[`server/tests/project_documents_acl.rs:169`](../server/tests/project_documents_acl.rs). The route-level proof of the
cross-matter guard is what is missing.

**Follow-up.** Seed a participation row on matter B for the requesting session in that test, so the `404` can only come
from the cross-matter guard.

## Finding 5: committing `asset_id` and `sha256` to a private repository

**Severity: Low.** Sign-off with corrections.

A committed pointer ([`store/src/document_pointers.rs:14`](../store/src/document_pointers.rs)) carries `kind`,
`visibility`, `version`, `asset_id`, `created_at`, `sha256`, `size_bytes`, and an optional `previous_version`. Its path
below `documents/` is the document slug.

**The identifier alone is worthless without a session.** Every read path that accepts an `asset_id` is behind the layers
in Finding 4. The object key is `projects/<code>/documents/<sha256>`
([`store/src/documents.rs:526`](../store/src/documents.rs)), so a pointer plus the repository name is the full object
coordinate, but the bucket refuses an unsigned read (Finding 3). Holding the key gets an attacker a `403`. Committing
`asset_id` and `sha256` to a private repository is acceptable for privileged material.

Three corrections to how the contract is described and reasoned about.

- **The disclosure in a pointer is the slug, not the hash.** `docs/project-repositories.md` says a pointer never
  contains an object-storage coordinate. The `sha256` *is* the object key's last segment; the statement is true only
  because the bucket is private. The more consequential content is the file path: a descriptive slug under `documents/`
  names what was filed, and `kind`, `size_bytes`, and `created_at` say when and how large. Read access to a Project
  repository is therefore access to the privileged *index* of the matter, and should be granted on that basis.
- **A content hash is a confirmation oracle.** Anyone who holds a candidate file and repository read can prove that
  exact file was filed on the matter, by hashing it. For litigation material that is a property worth knowing, not a
  reason to stop.
- **The trust boundary is the hosting provider's private-repository control.** The repository is private on a
  third-party host; Navigator does not encrypt the pointer. That is consistent with the rest of the contract and is the
  reason the no-client-data gate scans only source.

**Follow-up.** Correct the pointer description in `docs/project-repositories.md` to say what a pointer discloses and why
that is acceptable, and add guidance on neutral slugs for privileged documents.

## Finding 6: `navigator site pull` on a laptop

**Severity: Medium.**

`pull` ([`cli/src/document_sync.rs:115`](../cli/src/document_sync.rs)) walks every committed pointer, skips one whose
local target already carries the recorded `sha256` ([`document_sync.rs:532`](../cli/src/document_sync.rs)), downloads
each missing or mismatched revision through the authorized route, verifies the digest, and publishes the plaintext bytes
at the pointer's own path under `documents/`. A repeat run over a hydrated checkout downloads nothing. The transaction
area under the system temp directory ([`document_sync.rs:204`](../cli/src/document_sync.rs)) holds the downloads and
backups only until commit or rollback, and an interrupted run is recovered on the next invocation.

**The ignore file is a convenience, not the control.** `pull` and `sync` write `documents/.gitignore` (`*`, `!*/`,
`!*.yml`, `!.gitignore`) when it is absent. An ignore rule only keeps *untracked* paths out of `git add`'s default
sweep: `git add -f`, a `git mv` onto a path below `documents/`, or any other index entry stages the plaintext anyway,
and a commit of it lands in the private repository's history before anything refuses it. The control is the repository
gate. `navigator project gate` enumerates `git ls-files --cached --others --exclude-standard`
([`cli/src/projects/repository.rs:563`](../cli/src/projects/repository.rs)), so a force-added byte is in the `--cached`
half whatever the ignore file says, and the gate rejects every file below `documents/` that is not a `.yml` pointer or
the guard ([`repository.rs:694`](../cli/src/projects/repository.rs)). Two limits follow. The gate runs when someone runs
it or when the Project's CI runs it on a pull request, so the byte is refused at review time, not at commit time; and
the `--exclude-standard` half means an ignored, unstaged plaintext file is invisible to the gate by design, which is
what lets a pulled tree pass its own gate. No local hook runs the gate before a commit.

What that leaves behind is exactly what the brief describes: privileged plaintext on a laptop, kept out of Git's default
staging and from nothing else, with no expiry. The clap description at [`cli/src/main.rs:793`](../cli/src/main.rs) says
"hydrating a fresh checkout" and nothing about what the bytes are. `sync` is the opposite: it removes each staged file
after upload, so a completed `sync` leaves no bytes.

Encryption at rest inside Navigator would not help, because the point of `pull` is to read the files with ordinary
tools; full-disk encryption on the laptop is the control that applies, and it is the operator's, not this code's.

**Recommendation.** Three changes, in order of value.

1. Add `navigator site clean`: remove every file below `documents/` that is not a `.yml` pointer or the `.gitignore`,
   the same set the repository gate refuses, and print the count. A `--dry-run` lists them.
2. Print a closing line from `pull` naming the count of privileged files now on disk and the `clean` command.
3. Do not make `pull` opt-in behind a flag. Its only purpose is to write these bytes, so a flag would be confirmed by
   reflex; the warning and the clean command are what change behaviour.

**Follow-up.** Add `site clean` and the closing warning to `site pull`.

## Finding 7: Project-portal Content Security Policy

**Severity: Medium.** A decision, and this audit's recommendation is option 1 of ENG-651.

**What the portal actually sends.** An authenticated participant's portal response carries `PORTAL_CSP`
([`portal/src/project_portal.rs:114`](../portal/src/project_portal.rs), inserted at line 401):

```text
default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:;
font-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'self'; frame-ancestors 'none'
```

The header ENG-651 measured is not this one. An anonymous `curl` against a portal mount receives the `303` to login, and
that response carries the site-wide policy from [`portal/src/lib.rs:244`](../portal/src/lib.rs). Its `img-src`,
`font-src`, and `media-src` entry naming the deployment's own host is `NAVIGATOR_ASSET_BASE_URL`'s origin
(`asset_csp_origin`, line 261), which is the app's same-origin `/assets` proxy. It is redundant with `'self'` and admits
nothing new. Either way, the conclusion in ENG-651 stands: no portal policy admits the storage origin on `img-src`, and
`connect-src 'self'` blocks a `fetch` of a signed URL, so an embedded viewer cannot read a filed document.

**Option 2, admitting the storage origin, is a real widening and should be refused.** A `connect-src` that names
`https://storage.googleapis.com` admits every bucket on that host, not ours. A portal bundle is third-party code
published by a Project repository's CI; with that directive, a compromised or careless bundle can `fetch` a filed
document through the signed URL and `PUT` it to any bucket an attacker made public-writable, on the same allowed origin.
Today `connect-src 'self'` makes exfiltration from a portal a same-origin problem only. Rendering a PDF inline would
also need `frame-src` or `object-src` widened to the same host, and `object-src 'none'` is one of the policy's two
clickjacking and plugin backstops. The option trades the portal's strongest boundary for cheaper egress.

**Option 1, proxying the bytes same-origin, keeps every directive at `'self'`.** The signed URL never reaches the
browser, so Findings 1 and 2 close for the portal path, and Finding 5's object coordinate is never issued to a client.
Two things it costs.

- Egress and CPU move to the pod. The existing `stream_through` fallback
  ([`project_documents.rs:654`](../portal/src/project_documents.rs)) buffers the whole object into memory before
  responding; with `MAX_BATCH_BYTES` at 500 MB, a production proxy needs a streaming body, not that helper.
- The route must set `Content-Disposition` deliberately: `inline` for the portal's embedded viewer, `attachment` for the
  matter page's Download link.
- **Inline delivery of a caller-typed body is a same-origin script vector.** An asset's `content_type` is whatever the
  uploader said: the multipart part's own type on a browser upload
  ([`project_documents.rs:457`](../portal/src/project_documents.rs)) or an extension map that falls back to
  `application/octet-stream` on a CLI sync ([`cli/src/document_sync.rs:683`](../cli/src/document_sync.rs)). Ingest
  validates the `kind`, not the type ([`store/src/documents.rs:120`](../store/src/documents.rs)). Today a `text/html` or
  `image/svg+xml` document rendered from a signed URL executes in the storage origin, which holds none of Navigator's
  cookies. Proxied same-origin and served `inline`, the same bytes execute as Navigator. So the proxy must serve
  `inline` only for an allowlist of passive types (`application/pdf`, raster images), send everything else as
  `attachment`, keep `X-Content-Type-Options: nosniff`, and add `Content-Security-Policy: sandbox` on the streamed
  response so a mis-typed body cannot run even if the allowlist is wrong.

**`style-src 'unsafe-inline'`.** It exists for the banner spliced into the bundle's `index.html`
([`project_portal.rs:111`](../portal/src/project_portal.rs) and 416) and for the Dioxus templates' `style` attributes.
With `connect-src 'self'` and `img-src` limited to `'self' data: blob:`, the CSS-injection exfiltration channels this
would normally open have nowhere to send data, so it is Low. A hash source for the banner's fixed styles would let the
portal policy drop the keyword; the site-wide policy would keep it until the templates stop using `style` attributes.

**Follow-up.** Implement the same-origin streaming proxy for the portal document route with true streaming, an explicit
disposition, an inline allowlist with `sandbox` on the response, keep `PORTAL_CSP` unchanged, and record the decision
beside the TTL rationale in `docs/assets.md`.

**Landed (LAW-22 / ENG-651).** The project-document download route no longer redirects. It reads the object and writes
the bytes into its own response, so no storage URL reaches the browser on this path and `PORTAL_CSP` is untouched.
`Content-Disposition` defaults to `attachment`; `?inline=1` asks for `inline` and is granted only for the passive types
in `INLINE_CONTENT_TYPES` — `application/pdf` and raster images, deliberately not `image/svg+xml`. Every streamed
response carries `X-Content-Type-Options: nosniff` and `Content-Security-Policy: sandbox`, so a mis-typed body cannot
execute as Navigator even if the allowlist is wrong. Pinned by
[`server/tests/project_document_same_origin_delivery.rs`](../server/tests/project_document_same_origin_delivery.rs),
whose storage double signs — a backend that *can* hand back a URL and is still not redirected through is what makes the
test evidence rather than a restatement of the `FsStorage` fallback.

Two parts of this follow-up did not land here, and neither is a silent omission:

- **True streaming.** The route reuses the existing `stream_through` helper, which buffers the whole object before
  responding. A streaming body needs a `StorageService` method that returns a byte stream, and the trait has none — that
  is its own change across `cloud`'s three backends. One request's memory is bounded by what upload admits
  (`MAX_BATCH_BYTES`, 500 MB), which is the ceiling to remove. Tracked separately.
- **`docs/assets.md`.** The decision is recorded here instead. That document is titled *Public assets* and covers image
  and webfont references; it carries no TTL rationale to sit beside, and a private matter document's delivery path is
  not what a reader goes there for.

## Finding 8: per-Project publisher identity

**Severity: Medium.** Isolation holds; the live shape is not the one the code and one document describe.

**What the code provisions.** [`cli/src/devx/gcp/app_publisher.rs`](../cli/src/devx/gcp/app_publisher.rs): one
`nav-pub-<code>` account per Project (line 117); a custom role holding exactly `storage.objects.create`, `get`, and
`update` (line 151), bound on the applications bucket under a condition confining it to `<code>/portal` and
`<code>/portal/` (line 188); an `app-publisher` pool with a `ghe-oidc` provider whose condition is the applications
organization on `main` (lines 128, 138, 304); and an impersonation binding pinned to one `<org>/<repo>` (line 328).
`ensure_publisher_grant` (line 549) strips `objectCreator`, `objectAdmin`, and `objectUser` from a publisher it finds
and refuses to repoint an account already bound to a different prefix.

**What staging carries.** Read on 2026-09-12 with `gcloud` pinned to `neon-law-stg`, with codes withheld:

- No `app-publisher` pool, no `ghe-oidc` provider, no `nav-pub-*` account, and no `navigatorApplicationsPublisher` role.
- Three publisher accounts named `navigator-app-<code>`, each bound on the applications bucket with the predefined
  `roles/storage.objectAdmin` under a condition of exactly the shape line 188 renders, one prefix per account. Each
  account's only IAM binding is `roles/iam.workloadIdentityUser` for one `attribute.repository/<org>/<repo>` principal
  set on the `github` pool.
- The `github` pool's `github-oidc` provider condition is one CEL expression: a disjunction of four
  `assertion.repository == '<org>/<repo>'` clauses, each also requiring `refs/heads/main` (one also admits tags). That
  expression guards the three publishers and Navigator's own deploy identity together.

[`docs/project-repositories.md:424`](project-repositories.md) already records this divergence and the reason the
provider condition must never be edited by hand; it is the shared CEL the brief refers to. The same document at line 541
says staging carries a `nav-pub-<code>` identity per sample repository, which is not what the project holds today.

**Can one Project's publisher write another's prefix?** Not by the policy as read. Each account has one conditioned
binding on one prefix, and each account can be impersonated from one repository. `objectAdmin` under the prefix
condition does grant `storage.objects.delete` on the account's own portal, which the custom role withholds; that is an
availability exposure for the Project's own portal, not a confidentiality one. `storage.objects.list` is evaluated on
the bucket resource, which the object-name condition does not match, so the predefined role does not leak other
Projects' object names. No live write was attempted; this is an IAM read, not a test of enforcement.

**The shared condition is the risk.** Four clauses in one expression, edited by hand, with Navigator's deploy identity
among them: removing or mistyping a clause revokes a publisher or the deployer with no error at edit time. The code's
shape (owner plus `main` on a pool no one else shares, per-repository impersonation) has no such expression to edit.

**Recommendation.** Converge staging onto what `app_publisher.rs` provisions by running `navigator ops gcp setup` with
the three sample repositories as `--applications-publisher-repo`, then remove the three hand-made bindings and accounts,
and correct line 541. Production should be read the same way before anything is concluded about it; the commands are in
the pull request.

**Follow-up.** Converge staging's publisher identities onto the provisioned shape and correct the staging claim in
`docs/project-repositories.md`.

## Finding 9: no audit event on a successful issuance

**Severity: Medium.**

The download handler logs a refusal (`project document download denied by access policy`,
[`project_documents.rs:535`](../portal/src/project_documents.rs)) and a signing failure
([`project_documents.rs:577`](../portal/src/project_documents.rs)), and nothing on success: the `Ok(url)` arm at line
566 returns the redirect and emits no event. The notation route behaves the same way
([`documents.rs:96`](../portal/src/documents.rs)). Neither layer above fills the gap. The API audit middleware is
mounted on `/app/api` only ([`portal/src/lib.rs:829`](../portal/src/lib.rs)), and the site-wide `TraceLayer`
([`lib.rs:1967`](../portal/src/lib.rs)) records request and response at `DEBUG` by default, below an `INFO` filter and
not shaped as an audit record.

So the record of who was handed a credential for which privileged document, and when, does not exist. For litigation
material that is a diligence gap rather than an observability nicety: an incident review cannot reconstruct the reads,
and a client cannot be told who opened what. The CI mint already shows the shape to copy: a `target: "audit"` event
naming the Project code, the person, and the event ([`portal/src/ci_auth.rs:158`](../portal/src/ci_auth.rs)).

**Recommendation.** Emit one `audit`-target event on every successful issuance and on every streamed fallback, carrying
the Project code, the asset id, the person id, the lens, and the TTL, and never the filename or slug. The same event
belongs on the proxy Finding 7 proposes.

**Follow-up.** Add the issuance audit event to both document routes and cover it with a test that captures the tracing
output.

## Finding 10: the CI document session

**Severity: Informational. The control is in place.**

A Project repository's CI verifies its committed pointers against the live records. On push to `main` with a configured
host, the `document-verify` action ([`action.yml:124`](../.github/actions/document-verify/action.yml)) runs the CLI's
`document verify --ci`, which exchanges the runner's GitHub OIDC token at `POST /auth/ci/document-token`
([`portal/src/ci_auth.rs`](../portal/src/ci_auth.rs)). On a pull request it runs the offline half only. Live
verification therefore remains main-only.

The mint checks the deployment's canonical host as audience, requires `refs/heads/main` with a `push` or
`workflow_dispatch` event, spends the token's `jti` once, and binds the repository to exactly one live Project whose
`repository_url` names it. The session is attributed to that Project's lawyer DRI and expires in ten minutes
([`portal/src/session.rs`](../portal/src/session.rs)).

The document session carries an explicit [`DocumentScope`](../portal/src/session.rs):

- `GET /app/api/projects` returns only the minted Project's `id` and `code`, the lookup fields the CLI needs.
- `GET /app/api/projects/{id}/documents/revisions?slug=` is allowed only for that exact Project id and returns revision
  metadata.
- Document downloads, signed-download issuance, writes, unrelated routes, and every other Project are refused before a
  handler runs. The same check applies when the credential is presented as a bearer token or as the session cookie.
- A CI session without an explicit scope is invalid, so an old unscoped document credential fails closed.

The separate PR-ref policy remains open in LAW-10. This control does not mint for pull-request refs or change workflow
guards; any future PR lane needs its own server-enforced read-only policy and seed dry-run decision.

## What this audit did not do

- No write to any cloud project. No read of production.
- No attempt to publish as one Project's identity into another's prefix; Finding 8 rests on reading IAM.
- No browser-level verification of viewer `Referer` behaviour from a storage-origin PDF.
- Nothing about the portals' own client-side code, and nothing requiring third-party infrastructure testing.
