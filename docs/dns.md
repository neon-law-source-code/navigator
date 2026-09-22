# DNS — pointing your instance at a real domain

DNS is **not required** to run Neon Law Navigator. `web` boots, serves the portal, and self-tests against nothing but
the store and storage — the KIND loop and every worked example use `your-domain.example`, and `navigator ops gcp setup`
provisions compute, data, and a static IP but **never touches DNS**. DNS is the last-mile of a public deploy: it makes
the instance reachable at a real hostname and wires mail. This page is the recipe, and the tools you use to apply it.

DNS is also **provider-agnostic**. The record model lives behind the `DnsProvider` trait in `cli/src/devx/dns.rs`;
DNSimple is the shipped implementation and the provider Neon Law runs, but the app never dials it at runtime, and a fork
can point the same records at Cloud DNS or Route 53 without changing a line of application code. DNS sits *above* the
env-var interface — see [`third-party-integrations.md`](third-party-integrations.md).

## The three jobs DNS does for a deploy

1. **Reach the instance.** A `www` `A` record → the static gateway IP you reserved during provisioning.
2. **Redirect the apex to `www`.** A DNSimple `URL` record 301s a bare `neonlaw.com` to `https://www.neonlaw.com`, so
   the instance only ever serves `www` (and `workflows`) — the apex never reaches your cluster.
3. **Wire mail — two independent lanes.** Human mailboxes (Google Workspace) and application mail (SendGrid) coexist on
   one domain by living on different records: human mail on the apex `MX`, application inbound on a `parse.` subdomain.

## Provision it with one command

`navigator ops dns setup` reconciles the whole record set through DNSimple. Every group is opt-in via a flag, so the
same command serves a greenfield fork and Neon Law's live topology. It reads four environment variables — three
deployment/provider coordinates and the DNSimple-specific token:

```bash
export DNS_ZONE=your-domain.example        # the domain to configure (or pass --domain)
export DNS_ACCT=<your-account-id>          # DNSimple account id — see `dnsimple accounts list`
export DNS_SIMPLE=<token>                  # secret; `DNSIMPLE_API_TOKEN` is a legacy alias
export NAVIGATOR_GATEWAY_IP=<gateway-ip>   # resolved address, not the GCP resource name

navigator ops dns setup \
  --redirect-apex-to-www \
  --google-workspace \
  --google-site-verification <token> \
  --sendgrid \
  --dkim-target <s1-target> \
  --dkim-target <s2-target> \
  --sendgrid-link-brand <label>=<target> \
  --spf-include amazonses.com \
  --dmarc none \
  --dry-run
```

The production deployment serves `www.neonlaw.com` — the firm — with `workflows.neonlaw.com` on the same gateway
address. The staging deployment also has public hostnames, `staging.neonlaw.com` and `workflows-staging.neonlaw.com`, on
its own gateway address; its tailnet perimeter keeps the sample-data deployment private. The separate `neonlaw.org` zone
is served by the redirect service, which sends both its apex and `www` host to the corresponding path on
`https://www.neonlaw.com` with a 301.

| Hostnames | Deployment | DNS target |
| --- | --- | --- |
| `www.neonlaw.com` / `workflows.neonlaw.com` | the production deployment | its `<prefix>-gateway-ip` |
| `staging.neonlaw.com` / `workflows-staging.neonlaw.com` | the staging deployment | its `<prefix>-gateway-ip` |
| `staging.deleteyourdata.com` | the staging deployment | its `<prefix>-gateway-ip` |
| `staging.lawyershook.com` | the staging deployment | its `<prefix>-gateway-ip` |
| `neonlaw.org` / `www.neonlaw.org` | the redirect service | `https://www.neonlaw.com` (path-preserving 301) |

The `neonlaw.com` apex is not a deployment entry: it carries a `URL` record that 301s to `https://www.neonlaw.com`,
which is a record inside the production zone. It is the Apex→www row of the record table below. It must not appear on a
GKE `ManagedCertificate`: the name does not resolve to the load balancer, and a Google-managed certificate stays in
`Provisioning` until every listed name validates. The `neonlaw.org` zone has its own host-dispatched redirect service so
deep links survive the domain migration.

`DNS_ACCT` is the DNSimple account that holds the zone. Read it from `dnsimple accounts list` — a user token can span
several accounts, and a command run against the wrong one fails with `Zone not found` rather than a permission error,
which reads like a typo in the domain. Confirm it before a run with that command, or with the API directly:

```bash
curl -s -H "Authorization: Bearer $DNS_SIMPLE" -H "Accept: application/json" \
    https://api.dnsimple.com/v2/accounts
```

DNS remains outside the deployment tree and outside GCP provisioning: keep `DNS_SIMPLE` as an operator-local credential,
use the DNSimple CLI for the reviewed zone transaction, and never copy an account-wide token into a deployment config.
The exact commands and gateway address are in the Operating the Navigator workshop.

Run it with `--dry-run` first to read every call; drop the flag to apply. Each flag maps to one record group:

| Group | Records | Flag |
| --- | --- | --- |
| Reachability | `www` / `workflows` `A` → gateway IP | `--gateway-ip` / `--host` |
| Apex→www redirect | apex `URL` record → `https://www.<zone>` | `--redirect-apex-to-www` |
| Human mail | apex `MX` `smtp.google.com` + SPF `_spf.google.com` | `--google-workspace` |
| Domain verification | apex `google-site-verification` `TXT` | `--google-site-verification` |
| Application inbound | `parse.` `MX` `mx.sendgrid.net` + SPF `sendgrid.net` | `--sendgrid` |
| Application outbound | `s{1,2}._domainkey` DKIM + link-branding `CNAME`s | `--dkim-target` / `--sendgrid-link-brand` |
| DMARC | `_dmarc` `TXT` | `--dmarc` / `--dmarc-rua` |

Two properties make it safe to re-run. It is **idempotent**: against a domain that already has the records, every line
reports `Unchanged` and zero API calls are made; a single-valued record whose content drifted is patched in place, and a
missing member of a multi-valued set (the apex `A`/`AAAA` groups) is created. And it is **additive** — it never deletes
a record it does not manage, so a `_twilio` verification or a CAA record you added by hand is left untouched.

The same fail-closed rule applies when a managed host is migrating from a DNSimple `URL` redirect or `CNAME` to the
gateway `A` record. The command names the conflicting record type and stops. List that exact name, verify the returned
record id, delete only the obsolete redirect, and rerun the dry-run:

```bash
DNSIMPLE_TOKEN="$DNS_SIMPLE" dnsimple records list "$DNS_ZONE" \
  --account "$DNS_ACCT" --name www
RECORD_ID="replace-with-verified-record-id"
DNSIMPLE_TOKEN="$DNS_SIMPLE" dnsimple records delete "$DNS_ZONE" \
  "$RECORD_ID" --account "$DNS_ACCT"
```

### Moving a live hostname onto a deployment

Repointing a zone that already serves a site is a cutover with a TLS gap, because the Google-managed certificate covers
exactly `NAVIGATOR_PUBLIC_HOST` and cannot be issued until DNS already resolves to the load balancer. Order matters:

1. Set `NAVIGATOR_PUBLIC_HOST` and `NAVIGATOR_WORKFLOWS_HOST` in that deployment's `config.toml`, then
   `ops ship --deployment <name>`, so the `ManagedCertificate` resources request the new hostnames.
2. Delete any conflicting record at `www`. `ops dns setup` refuses to add an `A` record where a `CNAME` exists and will
   not delete one for you—that deletion is a reviewed operator action, and it takes the current site off the air.
3. Point `www` and `workflows` at the gateway IP with `ops dns setup`.
4. Wait for first issuance—roughly 15 minutes from the moment DNS resolves. The HTTP-01 challenge is served over plain
   HTTP, so do not enable the FrontendConfig's `redirectToHttps` until the certificate first reports `Active`.
5. Register the new origin's `/auth/callback` as an authorized OAuth redirect URI before expecting sign-in to work.

Between steps 2 and 4 the hostname serves a certificate error. Schedule it deliberately rather than as a side effect of
a release.

The per-domain SendGrid and Google secrets — the DKIM/link-branding `CNAME` targets and the site-verification token —
come from SendGrid's Domain Authentication wizard and Google's Admin console. The command takes them as flags and
**never invents them**. (`cargo run -p cli -- ops dns setup …` is the same command from a source checkout.)

## By hand, or on another DNS provider

If you manage DNS yourself or run a different provider, the same records are standard. The DNSimple-native tool is the
`dnsimple` CLI — `brew install dnsimple/dnsimple/dnsimple`, then authenticate:

```bash
dnsimple auth login                            # browser flow; stores a named context
echo "$DNSIMPLE_TOKEN" | dnsimple auth login --with-token   # headless / CI: token on stdin
```

Set the coordinates as shell variables first — don't paste literals into the commands. `ZONE` is the domain you are
configuring; `ACCT` is your DNSimple account id, which you read from `dnsimple accounts list` (a user token can span
several accounts, so pick the one that holds the domain); `NAVIGATOR_GATEWAY_IP` is the resolved address of the static
IP you reserved during provisioning, the same variable `ops dns setup` reads:

```bash
ZONE=your-domain.example              # the domain you are configuring
ACCT=<your-account-id>                # from `dnsimple accounts list`
NAVIGATOR_GATEWAY_IP=<gateway-ip>     # gcloud compute addresses describe <name> --global --format='value(address)'
```

Every address, token, and vendor target below is a deployment coordinate rather than a fact about Navigator, so each one
reads from the environment. That is what lets this page be published: a fork substitutes its own values, and no live
instance's addresses ship in the documentation.

### Reach the instance — `www` (and `workflows`) → the gateway static IP

```bash
dnsimple records create "$ZONE" -a "$ACCT" --type A --name www       --content "$NAVIGATOR_GATEWAY_IP" --ttl 300
dnsimple records create "$ZONE" -a "$ACCT" --type A --name workflows --content "$NAVIGATOR_GATEWAY_IP" --ttl 300
```

### Redirect the apex to `www` — a DNSimple `URL` record

DNSimple's redirector answers the bare domain with a `301` to `https://www.<zone>`, so the redirect lives entirely in
the DNS provider and your cluster only ever serves `www`:

```bash
dnsimple records create "$ZONE" -a "$ACCT" --type URL --name "" --content "https://www.$ZONE" --ttl 300
```

The redirector serves plain HTTP out of the box, but **HTTPS needs two things: a certificate for the apex, and a
DNSimple Teams plan or higher.** HTTPS redirects are a Teams-tier feature — on the Solo plan the redirector answers port
80 only, and `https://<zone>` fails the TLS handshake no matter what. On Teams, DNSimple still does not auto-issue a
certificate for a `URL` record, so `--redirect-apex-to-www` also reconciles a free, auto-renewing Let's Encrypt
certificate for the bare apex (`name: ""`, never `www` — GKE's own `ManagedCertificate` already covers that host; see
[Grounding TLS in the release inventory](#grounding-tls-in-the-release-inventory) below):

- No certificate exists yet → order one and report the new certificate's id.
- A certificate is pending → request issuance, in case domain-control validation already completed.
- A certificate is already active → report it and do nothing.
- A certificate cannot be advanced automatically (still pending after an issue attempt, or in some other non-active
  state) → report the exact remediation, never a bare exit code.

This is `cli::devx::dns::ensure_apex_certificate`, and it is idempotent and safe to rerun exactly like the record
reconciliation above — rerunning `ops dns setup --redirect-apex-to-www` is how you both check on a pending certificate's
issuance and re-request it. It never issues an ECDSA/RSA choice or an alternate name on your behalf: it orders and
checks a certificate, it does not invent settings you did not ask for. Issuance is asynchronous — Let's Encrypt
validates through the DNSimple-delegated zone, and a certificate's own state moves from `new` through `requesting` to
`issued`. If the automated order/issue exhausts what it can do from your account's own state (a plan without Teams, or a
certificate parked in an unexpected state), the command names the manual fallback:

```bash
# order → note the returned certificate id → issue it
dnsimple certificates order-letsencrypt "$ZONE" -a "$ACCT" --auto-renew
dnsimple certificates issue-letsencrypt   "$ZONE" -a "$ACCT" <certificate-id>
```

### Reconciling a family of domains in one run

`--domain` is repeatable, so a set of sibling domains that share a record shape is one reviewable run rather than four
near-identical ones:

```bash
navigator ops dns setup \
  --domain first-domain.example --domain second-domain.example \
  --gateway-ip "$NAVIGATOR_GATEWAY_IP" --host www --host staging \
  --redirect-apex-to-www --dry-run
```

Each zone is reconciled independently, in the order given, under one heading so a `(root)` line can be traced to the
apex it belongs to. They are separate zones rather than one transaction: a failure on the third names that zone and
leaves the first two applied, so the fix is to correct that zone and re-run — which is safe, because the command is
idempotent. A domain repeated on one command line is rejected rather than applied twice, since reconciling a zone twice
in one run would double every create.

The apex `URL` record always targets **its own** `www`, derived per zone. That is the copy-paste failure this form
exists to remove: four hand-edited invocations differing only in the domain are exactly where one brand's apex ends up
redirecting to another brand's site.

### The apex redirect is not done when the `URL` record lands

A `URL` record with no certificate behind it is the failure this trips over most, because it **passes a casual check**.
The redirector answers port 80 immediately, so `curl -I http://<zone>` returns the 301 you were looking for and the
record looks finished. Browsers and pasted links default to HTTPS, where the same host fails the TLS handshake outright
— so the first person to find it is a visitor, not the operator.

Two things must both be true before the apex is actually reachable, and neither implies the other:

1. The account is on **Teams or higher**. HTTPS redirects are a Teams-tier feature; below it the redirector serves port
   80 only, and no certificate changes that.
2. A **certificate exists for that domain and is active**. Teams does not issue one for a `URL` record on its own;
   `--redirect-apex-to-www` orders and requests issuance for you, but issuance is asynchronous and can still be pending
   the first time you check — read its printed report (or rerun `navigator ops brand-readiness`, below) rather than
   assuming the flag alone finished the job.

Check the tier once per account and the certificate once per domain, since a single account holding several domains will
have certificates for some and not others:

```bash
curl -s -H "Authorization: Bearer $DNS_SIMPLE" -H "Accept: application/json" \
  https://api.dnsimple.com/v2/accounts                                  # → plan_identifier
curl -s -H "Authorization: Bearer $DNS_SIMPLE" -H "Accept: application/json" \
  "https://api.dnsimple.com/v2/$DNS_ACCT/domains/$DNS_ZONE/certificates"  # → [] means HTTP only
```

Verify **both** schemes, never just one — checking only `http://` is what lets the broken state ship:

```bash
curl -sI "http://$DNS_ZONE"  | head -1   # → 301
curl -sI "https://$DNS_ZONE" | head -1   # → 301, not a TLS error
```

**Migrating an existing domain** whose apex still points at another redirect (e.g. a set of apex `A`/`AAAA` forwarding
records) requires deleting those apex records first. `ops dns setup` is additive and never deletes, and a `URL` record
cannot coexist with address records on the same name, so the command **refuses to run** — `conflicting records at the
apex` — until you remove them by hand:

```bash
dnsimple records list "$ZONE" -a "$ACCT" | awk '$2=="A" || $2=="AAAA"'   # find the apex forwarding record ids
dnsimple records delete "$ZONE" -a "$ACCT" <record-id>                    # remove each, then create the URL record above
```

### Human mail — Google Workspace (the apex `MX`)

```bash
dnsimple records create "$ZONE" -a "$ACCT" --type MX --name "" --content smtp.google.com --priority 1 --ttl 3600
dnsimple records create "$ZONE" -a "$ACCT" --type TXT --name "" --ttl 3600 \
  --content 'v=spf1 include:_spf.google.com include:amazonses.com include:sendgrid.net -all'
dnsimple records create "$ZONE" -a "$ACCT" --type CNAME --name chat --content ghs.googlehosted.com --ttl 300
```

The single SPF record authorizes all three senders for the domain at once — Google Workspace (`_spf.google.com`), Amazon
SES, and SendGrid — so mail from any of them passes SPF.

### Application mail — SendGrid (outbound authentication + inbound parse)

The two DKIM targets are issued per SendGrid account by the Domain Authentication wizard — they encode your account id,
so they are yours and are not guessable from this page. Read them out of the wizard and export them:

```bash
SENDGRID_DKIM_S1=<s1-target>   # e.g. s1.domainkey.uNNNNNNNN.wlNNN.sendgrid.net
SENDGRID_DKIM_S2=<s2-target>   # the matching s2 target from the same wizard screen
```

```bash
dnsimple records create "$ZONE" -a "$ACCT" --type CNAME --name s1._domainkey --ttl 3600 \
  --content "$SENDGRID_DKIM_S1"
dnsimple records create "$ZONE" -a "$ACCT" --type CNAME --name s2._domainkey --ttl 3600 \
  --content "$SENDGRID_DKIM_S2"
dnsimple records create "$ZONE" -a "$ACCT" --type MX --name parse --content mx.sendgrid.net --priority 10 --ttl 3600
```

Then, in SendGrid → **Inbound Parse**, register the host `parse.your-domain.example` and point it at Navigator's inbound
webhook. The application side — the webhook route and the event pipeline — is
[`email-events-pipeline.md`](email-events-pipeline.md).

SendGrid's optional `spam_check` is a spam score, not an attachment-malware verdict. Navigator independently streams
every attachment to the cluster-private `clamd` address in `NAVIGATOR_CLAMD_ADDR` before any raw-email, conversation,
notification, or matter-document side effect. Clean bytes may be forwarded and filed; a malware finding is quarantined
inside the archived raw message, and scanner errors return `503` so SendGrid retries.

## Why the app `MX` lives on a subdomain

Human mail and application mail share one domain without ever colliding, because they live on different records: the
command writes **Google Workspace's `MX` at the apex** (`smtp.google.com`) and **SendGrid Inbound Parse's `MX` on the
`parse.` subdomain** (`mx.sendgrid.net`). That is why `--google-workspace` and `--sendgrid` can both be set on the same
run — one touches the apex, the other a subdomain, so neither overwrites the other's mail exchanger.

## Routing `support@` into Navigator — the Google Workspace step

Client mail sent to `support@your-domain.example` is a Google Workspace address, but we want it to flow into Navigator's
inbound pipeline. Workspace forwards it to SendGrid Inbound Parse with **one Gmail routing rule** — no DNS change:

1. Google Admin → **Apps → Google Workspace → Gmail → Routing** (<https://admin.google.com/ac/apps/gmail/routing>).
2. **Add another rule.** Name it descriptively — Neon Law's is `support => sendgrid inbound parse` — and apply it to
   **inbound** messages for your organization.
3. **Match:** envelope recipient is `support@your-domain.example`.
4. **Action:** check **Change envelope recipient → Replace recipient** and enter `intake@parse.your-domain.example`.
5. **Save** and confirm the rule shows **Enabled**. Configure the deployment's
   `NAVIGATOR_SUMMARY_ENVELOPE_RECIPIENTS` with this final replacement address, because that is the envelope recipient
   SendGrid reports to Navigator.

The result: a sender emails `support@your-domain.example` → Workspace rewrites the envelope recipient to
`intake@parse.your-domain.example` → the `parse.` subdomain's `MX` (`mx.sendgrid.net`) delivers it to SendGrid → Inbound
Parse POSTs it to Navigator's webhook. Lawyer mail to every other address on the domain is untouched and lands in
Workspace as usual.

## Grounding TLS in the release inventory

A next release must carry every registered house brand's certificate, not only the launched ones.
`views::brand::BrandKey::ALL` names eight compiled keys today; `views::brand::release_brand_hosts()` is those keys
crossed with the hosts each one serves (`www.<domain>` and `staging.<domain>`) — the **release inventory**
`cli::devx::ship` reads to render every brand's `ManagedCertificate` and Ingress rule, in both environments, whether or
not that brand has launched.

This is a deliberate split from `views::brand::BrandKey::LIVE`, the **launch gate** — the approved list that controls
the request router's host admission, the crawler's `robots.txt`/sitemap base, the apex redirect, and the footer's "Our
Family" row. All eight keys are now admitted; the summons key serves a Coming Soon page while its service catalog
remains unpublished. Before ENG-808 the certificate/Ingress render read the launch gate too, so a held-out brand had no
TLS at all until the very change that launched it. That coupled two decisions that do not belong together: a certificate
is release infrastructure that should already be valid, trusted, and serving the right SAN well before a launch is
approved. Now a held-out brand's host still serves a real, trusted certificate — it just answers `404`
(`views::brand::held_out_host`) instead of the brand's page, never a TLS handshake failure. Each family keeps its own
isolated `ManagedCertificate` regardless (ENG-768), so an unpointed or held-out name in `Provisioning` never holds
another family's certificate hostage.

### Verifying it — `navigator ops brand-readiness`

A rendered manifest and a successful `kubectl apply` are a request, not proof: a `ManagedCertificate` can sit in
`Provisioning` indefinitely, and the old primary-host smoke check only ever looked at one of sixteen production/staging
hosts. `navigator ops brand-readiness --deployment <name>` is the receipt:

```bash
cargo run -p cli -- ops brand-readiness --deployment neon-law-stg
cargo run -p cli -- ops brand-readiness --deployment neon-production
```

For every host the release inventory names on that deployment's environment, it performs a real TLS handshake through
the host's ordinary trust store — **never `-k`/insecure** — and checks the actual response:

- a **live** brand must answer `200` with its own `og:site_name` (the same marker
  [`features/brand_routing.feature`](../features/tests/features/brand_routing.feature) already asserts on every route);
- a **held-out** brand must answer `404` — a `200` there is a host-admission regression, not a pass;
- on the production run only, every **live** brand's apex must redirect to its own canonical `www` host (apex checks
  are skipped on staging, which carries no apex at all — see [The apex redirect is not done when the `URL` record
  lands](#the-apex-redirect-is-not-done-when-the-url-record-lands) above).

It is bounded (one `curl` per host, `--max-time`-limited, no retry loop and no polling for a certificate to become
`Active`) and it mutates nothing, so it is safe to run as part of every staging-first handoff and as many times after
that as you like. It exits nonzero and names every failing host — missing, pending, or otherwise invalid — the moment
any one of them is not ready; it never reports success from the certificate manifests alone.

### Staging-first release handoff

1. Render and ship staging (`navigator ops ship --deployment neon-law-stg --tag <tag>`), then run
   `navigator ops brand-readiness --deployment neon-law-stg` and confirm every host is `Ready`.
2. Only once staging is fully green, ship the same immutable tag to production
   (`navigator ops ship --deployment neon-production --tag <tag>`) and run `navigator ops brand-readiness --deployment
   neon-production`.
3. **Production apply remains operator-run.** Neither `ops ship` nor `ops brand-readiness` runs unattended against
   production; an operator holding the release tag and cluster credentials runs both commands themselves, in that order,
   and reads the readiness receipt before calling the release done.
4. **Renewal is automatic and needs no rerun of this flow.** GKE reissues each `ManagedCertificate` on its own before
   expiry, and a DNSimple Let's Encrypt certificate ordered with `--auto-renew` (the default this command passes) does
   the same for the apex. `ops brand-readiness` is how you confirm a renewal actually landed, not how you trigger one.
5. **Rollback** is the same command against the previous tag (`ops ship --deployment <name> --tag <previous-tag>`);
   certificates are per-brand-family and untouched by which application tag is running, so a rollback never re-triggers
   issuance.

## Verify

```bash
navigator ops dns setup --dry-run          # every line Unchanged, 0 calls → the zone matches
dig +short www.your-domain.example          # → the gateway IP
curl -sI https://your-domain.example        # → 301 to https://www.your-domain.example (redirector + cert)
dig +short your-domain.example MX           # → smtp.google.com (Workspace)
dig +short parse.your-domain.example MX     # → mx.sendgrid.net (Inbound Parse)
navigator ops brand-readiness --deployment <name>   # the full per-brand TLS/content receipt, see above
```

## Related

- [`third-party-integrations.md`](third-party-integrations.md) — where DNSimple, SendGrid, and Google sit in the vendor
  model. [`email-events-pipeline.md`](email-events-pipeline.md) — the inbound/outbound email-event pipeline that Inbound
  Parse feeds into. [`gke-prod.md`](gke-prod.md) — the static gateway IP the `www` record points at, and the release
  inventory's `ManagedCertificate`/Ingress render.
