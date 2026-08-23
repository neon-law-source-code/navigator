# External provider parity across the deployments

Every cloud deployment is an independent trust boundary. A deployment may share a vendor tenant or GCP project only when
the provider requires that shape, but it never shares an application credential, webhook secret, signing key, sender
identity, Drive root, Restate journal, or GitHub organization with another deployment.

The rows below are the attachment records — one per deployment directory:

| Deployment | Public host | GCP project | GitHub organization | DocuSign |
| --- | --- | --- | --- | --- |
| `neon-law-stg` | `www.neonlaw.com` | `neon-law-stg` | `neon-law-stg-projects` | demo |
| the production deployment | `www.neonlaw.com` | its own project | its own organization | production |
| a second production row | `www.neonlaw.com` | `neon-law` | its own organization | production |

All three organizations and private Apps exist on GitHub Free. Each organization was created for the controlling
business `Shook Law PLLC`, uses `engineering@neonlaw.com` as its contact address, and began with the authenticated
operator as its sole owner; the bootstrap intentionally sent no invitations. Each App is installed only on its matching
organization. It selects all current and future repositories, grants repository Administration and Contents read/write,
and has no active webhook. Each deployment's `config.toml` carries the exact organization slug, App ID, and installation
ID; the distinct private key is in its `secrets.enc.yaml`. A later access review—not the bootstrap wizard—adds and
records any recovery owner.

| Deployment | GitHub App |
| --- | --- |
| `neon-law-stg` | `navigator-neon-law-stg` |
| the production deployment | `navigator-<deployment>` |
| a second production row | `navigator-<deployment>` |

## Google: two OAuth clients per deployment

Create **six** OAuth client registrations: one browser client and one Gemini Enterprise MCP client for each row. The
browser client owns interactive sign-in. The Gemini client identifies tokens from that deployment's Gemini Enterprise
data store.

Google Auth Platform branding, audience, and consent are configured per GCP project. Staging uses the `neon-law-stg`
project's consent configuration; the two production projects each have their own. Use an Internal audience when every
user belongs to the selected Workspace organization; an External audience requires the appropriate test-user or
verification process.

Use these names and exact browser callbacks:

| Deployment | Authorized browser redirect URI |
| --- | --- |
| `neon-law-stg` | `https://www.neonlaw.com/auth/callback` |
| the production deployment | `https://www.neonlaw.com/auth/callback` |
| a second production row | `https://www.neonlaw.com/auth/callback` |

Name the clients `navigator-<name>-browser` and `navigator-<name>-gemini`.

For each row:

1. Select its GCP project in **Google Auth Platform → Clients**.
2. Create a Web application browser client with only that row's callback.
3. Store its client ID as `NAVIGATOR_OAUTH_CLIENT_ID_BROWSER` in the matching `deployments/<name>/config.toml` and its
   secret as `OAUTH_CLIENT_SECRET` in that deployment's `secrets.enc.yaml`.
4. Create the Gemini Enterprise OAuth client using the setup in [`gemini-enterprise-mcp.md`](gemini-enterprise-mcp.md).
5. Store that client ID as `NAVIGATOR_OAUTH_CLIENT_ID_GEMINI`. Store the Gemini client secret in that deployment's
   Gemini Enterprise data-store configuration, not in a different deployment.
6. Ship, sign in through the browser client, and call an authenticated AIDA tool through the Gemini client. A client ID
   appearing in the tree is not proof that either flow works.

The browser registration can precede the Gemini data store. During that interval `NAVIGATOR_OAUTH_CLIENT_ID_GEMINI` is
absent and `ops ship` renders a browser-only allowlist. Issue
[#1126](https://github.com/neon-law-source-code/navigator/issues/1126) tracks the remaining staging registration and
removal of that temporary nullable seam.

Google does not permit these general Google Sign-In/API OAuth clients to be created or modified programmatically. The
similarly named `gcloud iam oauth-clients` surface creates Workforce Identity Federation clients and is not a
substitute. An authorized operator must complete the eight client registrations in Google Auth Platform and record each
resulting ID in the matching `config.toml` and each secret in the matching `secrets.enc.yaml` (`sops set`).

Retiring the old brand staging rows also retires `navigator-neon-law-stg-browser`, `navigator-neon-law-stg-gemini`,
`navigator-neon-staging-browser`, and `navigator-neon-staging-gemini` in the `neon-law-stg` project. Delete those
registrations manually in Google Auth Platform; do not repurpose their IDs, callbacks, or secrets for a surviving row.

Google requires the authorization request's redirect URI to exactly match the client registration, including scheme,
case, path, and trailing slash. Never place two deployments' callback URIs on one browser client.

## GitHub: one organization and private App per deployment

An organization owner must sign up for or create all three organizations in the matrix. For each organization:

1. Confirm GitHub Free, `engineering@neonlaw.com`, `Shook Law PLLC`, and the authenticated owner. Send no bootstrap
   invitations. Before production launch, run a separate access review, enable two-factor authentication, and record any
   approved recovery owner.
2. Confirm the **private**, organization-owned GitHub App in the matrix remains installed only in that organization.
3. Grant the repository permissions Navigator actually exercises: Administration read/write and Contents read/write. Do
   not grant access to an unrelated organization.
4. Generate a private key and capture the numeric App ID. Store `NAVIGATOR_GITHUB_ORG`, `NAVIGATOR_GITHUB_APP_ID`, and
   the discovered `NAVIGATOR_GITHUB_INSTALLATION_ID` in only the matching `config.toml`, and
   `NAVIGATOR_GITHUB_APP_PRIVATE_KEY` in that deployment's `secrets.enc.yaml`.
5. Set `NAVIGATOR_FORGE_BACKEND=github`, run `ops secrets apply --deployment <name>`, create a synthetic Project, and
   verify that its private repository appears in the matching organization and nowhere else.

The three Apps provision private Project repositories. They do **not** turn the canonical engineering receiver into
three copies. `neon-law-stg` alone receives the public Navigator repository webhook, watches
`neon-law-source-code/navigator`, and binds the DevX Restate services. Keep `NAVIGATOR_GITHUB_CANONICAL_REPOSITORY`,
`NAVIGATOR_GITHUB_APP_LOGIN`, `NAVIGATOR_GITHUB_WEBHOOK_SECRET`, and the DevX guardrails scoped to that singleton
receiver.

Set `NAVIGATOR_FORGE_BACKEND=github` in every deployment's `config.toml`. The GKE stacks import that selector and the
App credentials from their deployment Secret. The disposable local KIND integration surface keeps its in-cluster Forgejo
and is not one of these stacks.

## DocuSign: three isolated attachments

Sign up for DocuSign Developer accounts for staging and production-capable eSignature accounts for production. Each
deployment gets a distinct app/account attachment, RSA keypair, impersonated API user consent, Connect HMAC key, and
Navigator webhook path secret.

- Staging uses the demo hosts `https://demo.docusign.net/restapi` and `https://account-d.docusign.com`. Demo envelopes
  are non-binding.
- Production needs an eSignature plan that permits the API workload, a production account and sender, and an integration
  approved or promoted through DocuSign Go-Live. Discover the account-specific REST base rather than assuming a
  production shard.
- A promoted integration key may exist in both DocuSign tiers, but that does not make it one deployment credential. Keep
  each row's account ID, user ID, RSA private key, consent, HMAC key, and webhook secret unique.

Store the selected row's `DOCUSIGN_BASE_URL`, `DOCUSIGN_ACCOUNT_ID`, `DOCUSIGN_INTEGRATION_KEY`, `DOCUSIGN_USER_ID`,
`DOCUSIGN_PRIVATE_KEY`, `DOCUSIGN_OAUTH_BASE`, `DOCUSIGN_SIGNER_EMAIL`, `DOCUSIGN_SIGNER_NAME`, `DOCUSIGN_HMAC_KEY`, and
`DOCUSIGN_WEBHOOK_SECRET` in that deployment's `secrets.enc.yaml`. Prefer JWT grant over the short-lived
`DOCUSIGN_ACCESS_TOKEN` fallback. Prove staging with a completed demo envelope and production with one deliberate smoke
envelope plus a verified Connect delivery.

## SendGrid: sender and webhook isolation

Sign up for a Twilio SendGrid account or provider-native subuser boundary that can supply four separately revocable API
keys and three event/inbound webhook configurations. The selected plan must support the required number of webhooks.
Authenticate the matching sender domain and keep staging senders visibly non-production.

Each row receives its own restricted mail-send API key, `SENDGRID_FROM_EMAIL`, inbound parse route,
`SENDGRID_INBOUND_SECRET`, event route, `SENDGRID_EVENTS_SECRET`, and signed-event-webhook public key. Test outbound
delivery, inbound parse, and a signed event callback in that row before marking it complete.

## Google Workspace Drive: one root and service account per deployment

Each row needs its own Projects root folder within the selected Shared Drive. The Neon Law-controlled Workspace, Drive
service account, delegated user, JSON key, and domain-wide-delegation grant serve the three roots; a Workspace Super
Admin must authorize that service account's OAuth client ID. Staging and NLF use roots of their own in that Workspace,
and firm production uses its separate production root. The Drive key blocks live in each deployment's
`deployments/<name>/` tree; see [`environments.md`](environments.md#matter-storage-and-workspace-attachment).

## Restate: three journals

Create a Restate Cloud account and one environment per deployment. Arrange a plan that supports three environments and
the production availability requirements. Create a separate full-permission API key for each environment and store only
its broker URL, ingress URL, admin URL, and token in the matching deployment's `secrets.enc.yaml`. Register and verify
`workflows-service` independently in every environment.

## Completion record

Provider parity is complete only when every row has:

- a named human owner and recovery owner;
- a vendor account, tenant, project, organization, or environment identifier;
- every required key present in that deployment's `deployments/<name>/` tree, with unique values where the value is a
  credential (the parity gate in `cli/src/devx/deployments.rs` checks the names in CI);
- a dated rotation or revocation test;
- a real provider smoke test and a link to non-secret evidence.

Never put provider secret values, private keys, tokens, or service-account JSON in this document, Slack, a ticket, or a
smoke-test transcript. The target state remains one provider attachment per deployment; a credential is never copied
between deployments.
