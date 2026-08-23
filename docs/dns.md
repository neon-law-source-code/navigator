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

There is one production site on one zone. The production deployment serves `www.neonlaw.com` — the firm — with
`workflows.neonlaw.com` on the same gateway address. Staging has no public host, so it has no DNS.

| Zone | Deployment | `www` / `workflows` → |
| --- | --- | --- |
| `neonlaw.com` | the production deployment | its `<prefix>-gateway-ip` |

The apex is not a third entry: `neonlaw.com` itself carries a `URL` record that 301s to `https://www.neonlaw.com`, which
is a record inside this zone rather than a deployment of its own. It is the Apex→www row of the record table below.

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
certificate for a `URL` record, so you issue a free, auto-renewing Let's Encrypt certificate for the domain (its default
covers both the apex and `www`):

```bash
# order → note the returned certificate id → issue it
dnsimple certificates order-letsencrypt "$ZONE" -a "$ACCT" --auto-renew
dnsimple certificates issue-letsencrypt   "$ZONE" -a "$ACCT" <certificate-id>
```

Issuance is asynchronous (Let's Encrypt validates through the DNSimple-delegated zone); the certificate state moves from
`new` through `requesting` to `issued`, after which the redirector serves the apex over HTTPS. The
`--redirect-apex-to-www` flag writes the `URL` record and prints this same certificate reminder — it does not issue the
certificate for you.

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
5. **Save** and confirm the rule shows **Enabled**.

The result: a sender emails `support@your-domain.example` → Workspace rewrites the envelope recipient to
`intake@parse.your-domain.example` → the `parse.` subdomain's `MX` (`mx.sendgrid.net`) delivers it to SendGrid → Inbound
Parse POSTs it to Navigator's webhook. Lawyer mail to every other address on the domain is untouched and lands in
Workspace as usual.

## Verify

```bash
navigator ops dns setup --dry-run          # every line Unchanged, 0 calls → the zone matches
dig +short www.your-domain.example          # → the gateway IP
curl -sI https://your-domain.example        # → 301 to https://www.your-domain.example (redirector + cert)
dig +short your-domain.example MX           # → smtp.google.com (Workspace)
dig +short parse.your-domain.example MX     # → mx.sendgrid.net (Inbound Parse)
```

## Related

- [`third-party-integrations.md`](third-party-integrations.md) — where DNSimple, SendGrid, and Google sit in the vendor
  model. [`email-events-pipeline.md`](email-events-pipeline.md) — the inbound/outbound email-event pipeline that Inbound
  Parse feeds into. [`gke-prod.md`](gke-prod.md) — the static gateway IP the `www` record points at.
