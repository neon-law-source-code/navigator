# Deployment configuration and secrets — the `deployments/` tree

A repository is the operator source for both deployment coordinates and key material. Coordinates are plaintext and
reviewable; key material is encrypted per value against that deployment's own Google Cloud KMS key. A command decrypts
and writes Google Secret Manager, and the Secret Manager CSI driver projects it into the pod.

```text
deploy repo (SOPS) --> ops secrets apply --> Secret Manager --> CSI --> <name>-web-secrets --> pod
```

## The tree is not in this repository

It lives in a private repository, beside the workflow that rolls the cluster, and this repository does not name it.
Everything below describes that checkout.

The reason is the workflow, not the ciphertext. Per-value KMS encryption is sound in the open, and these coordinates say
of themselves that they are non-secret. What could not stay is a credential that reaches the cluster sitting in a
repository that accepts pull requests from strangers; the tree followed it because the workflow needs it on disk.

So every command here takes `--deployments-dir`, naming the directory that CONTAINS `deployments/` — which is the
directory `.sops.yaml` sits in, and in practice the deploy checkout's root. `NAVIGATOR_DEPLOYMENTS_DIR` sets the same
thing for a whole shell. Run them with the released `navigator` binary rather than `cargo run`: the deploy checkout
holds no Rust source, and the GKE manifests are compiled into the binary.

What this repository keeps is the *shape*. `cli/tests/fixtures/deployment-tree/` is a synthetic two-row tree, and the
workspace suite runs every gate below against it — so the loader, the `.sops.yaml` agreement, the requirement parity,
and the projection plan all stay covered here. `navigator ops deployments --deployments-dir .` runs the same gates
against the real rows, from the deploy repository's own CI.

The application never reads this tree and never reads GitHub. It reads the projected Kubernetes Secret. This page is the
operator boundary; the deployment map itself is [`environments.md`](environments.md).

## Rotation revokes nothing unless you rotate at the provider

Read this before the layout, because it is the one thing the design cannot enforce for you.

Re-encrypting a file rotates the data key. **It revokes nothing.** Every prior ciphertext stays readable to anyone
holding repository history and the KMS key, because the key decrypts history and not just `HEAD`. A rotation is
therefore two steps in this order:

1. **Rotate the value at the provider** — issue a new SendGrid API key, a new DocuSign key pair, a new database
   password. This is the step that actually revokes the old credential.
2. **Re-encrypt the new value here**, and ship it.

Doing only step 2 leaves the old credential live and readable. Doing only step 1 leaves the deployment broken. There is
no third option in which the file edit alone is sufficient.

Operationally, one rotation is one deployment at a time, never two in one sitting:

1. Rotate the value at the provider.
2. Re-encrypt it here: `sops set deployments/<name>/secrets.enc.yaml '["KEY"]' '"<value>"'`.
3. `navigator ops secrets apply --deployment <name> --deployments-dir .` — writes the new `versions/latest`.
4. `navigator ops ship --deployment <name> --deployments-dir . --restart-only` — pods cache `envFrom` at start, so
   nothing re-reads the Secret until they are recreated.
5. Verify that deployment's `/readyz`, `/version`, and the affected provider flow.

Never copy a value between deployments. Shared vendors still receive separate per-deployment credentials, and every
runtime project uses a deployment-specific Secret.

The corollary is to keep as little long-lived material in the tree as possible. The GitHub-to-GCP path uses Workload
Identity Federation and holds no stored credential; every value that can be a short-lived federated token instead of a
stored secret should be.

## Layout

```text
deployments/<name>/config.toml        coordinates, plaintext, reviewable
deployments/<name>/secrets.enc.yaml   key material, SOPS, per-deployment KMS key
.sops.yaml                            which KMS key encrypts which deployment
```

One directory is one deployment. `config.toml` also records the `kms_key` that deployment's material is encrypted
against; `.sops.yaml` carries the matching creation rule, and a test fails the build if the two disagree.

Two properties are load-bearing rather than incidental:

- **Per-value encryption, never a whole-file blob.** `encrypted_regex` in `.sops.yaml` leaves the key names in plaintext
  and encrypts only the values. That is what makes a rotation a one-line diff naming the variable that changed, and what
  lets the parity gate read names in CI with no credential and no decrypt. An envelope over the whole file decrypts
  identically and destroys both.
- **One KMS key per deployment, in that deployment's own project.** No deployment's key is decryptable by another
  deployment's principals. The IAM binding lives on the key itself, inside the project. Never grant it at the
  organization level: organization-inherited bindings are lost when a project moves between organizations, which would
  silently change who can read the archive.

## What is a coordinate and what is key material

If publishing the value in a pull request would be a disclosure, it is key material. Otherwise it is a coordinate.

Project IDs, cluster and namespace names, bucket names, hostnames, the Workspace login domain, Drive folder and shared
Drive IDs, the GitHub App ID and bot login, and the SurrealDB namespace and database are coordinates. Database URLs,
session and webhook signing secrets, OAuth client secrets, API keys, and private keys are key material.

Four boot-required keys are in neither file. `NAVIGATOR_CLAMD_ADDR`, `NAVIGATOR_STORAGE_BACKEND`,
`NAVIGATOR_EMAIL_BACKEND`, and `GOOGLE_OAUTH_CLIENT_IDS` ship as inline Deployment env — `ship::INLINE_ENV_WEB_KEYS`
records that, and the parity gate reads the same list, so the two cannot drift.

Not everything on the Secret rail is boot-required, and the ones that are not are the ones worth checking by hand. A key
absent from `WEB_REQUIREMENTS` fails no gate, so if the pod reads it with a default, the loss is silent rather than
loud. `DOCUSIGN_SIGNER_EMAIL` is the worked example: unset, `portal::retainer_walk` falls back to `support@neonlaw.com`
and envelopes keep going out addressed to the firm's shared mailbox rather than the deployment's signer. It and
`NAVIGATOR_GITHUB_INSTALLATION_ID` are projected for that reason, not because any invariant demands them.

## Provision a deployment's key

Once per deployment, before its `secrets.enc.yaml` exists. Run it against that deployment's own project:

```bash
OPERATOR="user:you@neonlaw.com"
PROJECT="neon-law-stg"
LOCATION="us-west4"

gcloud services enable cloudkms.googleapis.com secretmanager.googleapis.com --project "$PROJECT"

gcloud kms keyrings create navigator-secrets --project "$PROJECT" --location "$LOCATION"

gcloud kms keys create deployment-config \
    --project "$PROJECT" --location "$LOCATION" \
    --keyring navigator-secrets --purpose encryption

gcloud kms keys add-iam-policy-binding deployment-config \
    --project "$PROJECT" --location "$LOCATION" --keyring navigator-secrets \
    --member "$OPERATOR" --role roles/cloudkms.cryptoKeyEncrypterDecrypter
```

Grant no runtime service account on this key. The pods read Secret Manager through `roles/secretmanager.secretAccessor`;
nothing in a cluster ever decrypts a repository file.

Prove the isolation rather than assuming it, by enumerating every path that grants access to a `CryptoKey` — the key
itself, the keyring above it, the project, and the organization. An attempted decrypt under an impersonated service
account is the tempting test and the wrong one: unless the operator holds `roles/iam.serviceAccountTokenCreator` on that
account, it fails at `iam.serviceAccounts.getAccessToken` before it ever reaches the KMS permission, so the
`PERMISSION_DENIED` says nothing about the key. Enumeration also covers principals nobody thought to test.

```bash
PROJECT="neon-law-stg"

gcloud kms keys get-iam-policy deployment-config \
    --project "$PROJECT" --location us-west4 --keyring navigator-secrets

gcloud kms keyrings get-iam-policy navigator-secrets \
    --project "$PROJECT" --location us-west4

gcloud projects get-iam-policy "$PROJECT" \
    --flatten="bindings[].members" --filter="bindings.role:cloudkms" \
    --format="table(bindings.role, bindings.members)"

# Substitute the organization from `gcloud projects get-ancestors "$PROJECT"`,
# and check any folder between the two if one exists.
gcloud organizations get-iam-policy 517367957661 \
    --flatten="bindings[].members" --filter="bindings.role:cloudkms" \
    --format="table(bindings.role, bindings.members)"
```

Passing means the key policy names the operator and nothing else, and the other three return no `cloudkms` binding at
all. A runtime service account appearing at any level means that deployment's identity can read the other's archive, so
the bindings need correcting before any value is encrypted. An organization-level result is doubly wrong: it grants
access and it disappears on a project move.

Keyrings and keys cannot be deleted, only disabled, so the location and names above are effectively permanent.

## Author or edit key material

`sops` opens the file in `$EDITOR` decrypted and re-encrypts on save. The plaintext never touches disk:

```bash
sops deployments/neon-law-stg/secrets.enc.yaml
```

For a value that must not pass through an editor buffer — a private key PEM, a database URL — set it from a pipe:

```bash
sops set deployments/neon-law-stg/secrets.enc.yaml '["SESSION_SECRET"]' "\"$(openssl rand -hex 32)\""
```

Committing a decrypted file is the one unrecoverable mistake here, so two guards sit in front of it. `.gitignore`
refuses the filenames a decrypted working copy usually takes, and `navigator ops deployments` fails if any value in the
tree is not `ENC[…]` — including a file that was never encrypted at all. Neither guard can help after a push: at that
point the value is disclosed and the only remedy is rotating it at the provider.

## Apply to Secret Manager

Dry run first. It reads names only — it does not shell out to `sops`, needs no KMS permission, and cannot print a value:

```bash
navigator ops secrets apply --deployment <row> --deployments-dir . --dry-run
```

The dry run names the target project and every object, grouped by which file supplies it, and fails closed listing any
object the `SecretProviderClass` projects that neither file carries.

Read the *skipped* line as carefully as the failure. A skipped object is one the shared list names and this deployment
will not write, and each skip carries its reason — scoped to another deployment, integration not declared by this
deployment, or requirement satisfied another way. Those are exactly the entries `ops ship` omits from this deployment's
rendered class, so the line doubles as the list of what its mount will *not* ask for. Read it to confirm the deployment
is declining what you meant it to decline. The empty case is held in CI by
`the_automation_home_supplies_every_object_the_manifest_references`, against the synthetic automation-home row, and `ops
ship` resolves every surviving reference against live Secret Manager before it reconciles anything. Correct the tree
until the dry run is clean, then:

```bash
navigator ops secrets apply --deployment <row> --deployments-dir .
```

Each object is created if absent and receives a new version, which becomes `versions/latest`. Values ride in a JSON
request body: never in `argv`, never in an error message, never in a log line. Run it per deployment, staging first;
never against a production row before the same tag has proven itself on staging.

Applying a value does not restart anything. The CSI driver picks up a new `versions/latest` on its next rotation poll,
and `navigator ops ship --deployment <name> --deployments-dir . --restart-only` forces the pods to re-read immediately.

## The CSI projection

The Secret Manager CSI driver projects each deployment's Secret into the pod. Both halves —
`secrets/secret-provider-class.yaml` and `secrets/web-secrets-csi-mount.yaml` — are wired into
`examples/deploy/k8s/gke/kustomization.yaml`, and the class alone projects nothing, so they are added and removed
together: the GKE driver reconciles the projected Secret only while a pod mounts the volume.

The manifests are per-deployment. `ops ship` substitutes the project, namespace, and projected Secret name from the
deployment being shipped, so the one embedded file renders one disjoint `SecretProviderClass` per deployment, each
reading only its own project's Secret Manager. Two render guards in `cli/src/devx/ship.rs` fail the build if a literal
is left behind.

A CSI mount fails outright on any object it cannot read, so a deployment may only mount this class once its own project
holds every object the *rendered* class references. `ops ship` resolves all of them to an `ENABLED` `versions/latest`
before it reconciles anything and aborts naming the gap, so a deployment that is not ready fails at deploy rather than
as a crash-looping pod.

The object list in the embedded manifest is the superset, not what every deployment mounts. `ops ship` renders it per
deployment, dropping every entry `ops secrets apply` reports as skipped — from both the `parameters.secrets` mounts and
the `secretObjects` mappings — so the class each deployment applies references exactly what that deployment writes
(`ship::omit_unwritten_objects`). One shared list cannot state an object that is required in one project and forbidden
in another, and this is what resolves that:

- **Scoped to another deployment.** The engineering webhook trio — `NAVIGATOR_GITHUB_WEBHOOK_SECRET`,
  `NAVIGATOR_GITHUB_CANONICAL_REPOSITORY`, `NAVIGATOR_GITHUB_APP_LOGIN` — belongs to
  `store::deployment::GITHUB_AUTOMATION_HOME_PROJECT`. Every other deployment renders without it.
  `github_webhooks::ReceiverConfig::from_env` already refuses outside the automation home, so no deployment loses a
  receiver it was running.
- **Integration declined.** A deployment that supplies no `DOCUSIGN_BASE_URL` declares no DocuSign and renders none of
  its nine objects. The production deployment is that case; it runs `StubSignatureProvider`, which `portal::signature`
  reaches only through genuine absence.

Referencing an object a deployment does not write used to abort its ship at the resolve preflight, which is what kept
the production deployment from completing a first ship. Adding an object to the manifest is therefore safe once **any**
deployment carries it; the rows that do not carry it render without it, and
`ship::tests::the_rendered_class_references_exactly_what_the_deployment_writes` holds that for every deployment in the
tree.

The production deployment is live on the projected Secret. Bringing another deployment onto it, one at a time and never
two in one sitting:

1. **Reconcile the naming.** The `SecretProviderClass` references uppercase object names (`SESSION_SECRET`,
   `SESSION_SECRET`, …). Migrate or delete any object in that project under another convention so nothing is ambiguous.
2. **Render every key into Secret Manager** with `navigator ops secrets apply --deployment <name>` — the `--dry-run`
   first: it names any object the tree does not supply, and any object it reports as *skipped* is one the class
   references and this deployment will not write — a mount blocker, not a footnote.
3. **Confirm the driver name**: `kubectl get csidrivers` should show `secrets-store-gke.csi.k8s.io`. The `provider: gke`
   field and that driver name are the GKE-specific pieces.
4. **Confirm the bound GSA** holds `roles/secretmanager.secretAccessor` in that project.
5. **Rehearse with `navigator ops ship --deployment <name> --tag <YY.M.D> --dry-run`.** It resolves every referenced
   object read-only and reports the referenced and resolved counts; they must match before anything is applied. The
   operator needs `secretmanager.versions.get` in that project — the states are read, never the payloads, so
   `versions.access` is deliberately not required and no projected credential passes through the operator's machine.
6. **Deploy and verify** with `navigator ops ship --deployment <name> --tag <YY.M.D>`, then confirm the projected Secret
   carries every expected key and that `web` and `workflows-service` are healthy.
7. **Retire the plain Secret** only after step 6 proves the projected one works, and only for that deployment. The
   projected Secret takes the same name, so delete the plain one and let the driver recreate it, then run `navigator ops
   ship --deployment <name> --restart-only` so the pods re-read it.

A deployment holding real client matters goes last, in the risk-ascending order the release itself uses.

## Known gaps

The production deployment's `SENDGRID_API_KEY` is a **stub**, not a credential. It satisfies the boot invariant so the
parity gate is green, and it sends no mail — that deployment's outbound email is dead until a real key from the
production SendGrid account replaces it. The value is encrypted and therefore invisible in a diff, so the stub is
recorded in the production deployment's `config.toml` as well; a green gate here does not mean a working mail rail. This
is the one case where the gate reports satisfied without the deployment being usable, which is why it is written down
twice.

Objects that no deployment supplies are trimmed from the manifest rather than stubbed, because a stub is a value the
mount succeeds on and the feature then fails against. Three sets have been trimmed on those grounds: the four Workspace
Drive ids (documented in [`environments.md`](environments.md) but unconfigured; only
`NAVIGATOR_DRIVE_GCP_SERVICE_ACCOUNT_ID` exists), the three Xero ids (required only where that capability is enabled),
and `DOCUSIGN_ACCESS_TOKEN` (the alternative to the DocuSign JWT triple, which every deployment authenticates with
instead). Re-add each alongside the deployment that first carries a real value.

`NAVIGATOR_GITHUB_APP_LOGIN` is misfiled, and it is the one entry in the production deployment's `secrets.enc.yaml` that
is not key material. A bot login is a public coordinate and belongs in `config.toml`.

**Moving it to `config.toml` is the fix.** Until it moves, the exception is recorded here and in that deployment's
`config.toml`, so nobody has to rediscover why a public identifier is sitting in an encrypted file.

## The parity gate

`store::deployment::WEB_REQUIREMENTS` is the list of keys `web` enforces at boot, and every deployment is checked
against it in CI by `deployments::tests::every_deployment_satisfies_the_requirements_that_apply_to_it`.

The check is not "every deployment carries every key." A requirement may name one project, and the engineering webhook
five-tuple names `store::deployment::GITHUB_AUTOMATION_HOME_PROJECT` — that receiver runs in exactly one deployment and
no other may hold its credentials. Each deployment is measured against the requirements that apply to it.

A requirement may also be *triggered* by another key, which is how a deployment declines a whole integration. DocuSign
is the worked example: `DOCUSIGN_BASE_URL` declares it, and only then are the account, auth, HMAC, and webhook keys
required. A deployment that executes no documents supplies none of them and runs `StubSignatureProvider`.

Declining an integration is not the same as stubbing one, and the difference is load-bearing. A placeholder value does
not reach the stub — `portal::signature::DocuSignSignatureProvider::from_env` returns `Some` for any non-empty value, so
a fake credential boots the *real* provider and the deployment ships green and fails on its first signature request. The
stub is reachable only through genuine absence. Where an integration has no such fork, a placeholder is still the only
option: the production deployment's `SENDGRID_API_KEY` is a documented stub for exactly that reason, because
`NAVIGATOR_EMAIL_BACKEND` admits no stub backend to select.

The gate reads key names and never decrypts, so it runs with no credential and no network call. That is the whole point
of the drift bug being mechanical now: a key renamed in one place and nowhere else fails the build instead of surfacing
later as an opaque `ops ship` abort.

## See also

- [`environments`](environments.md) — the deployment matrix every coordinate here comes from.
- [`cloud-operations`](cloud-operations.md) — the operator boundary.
- [`gitops`](gitops.md) — release order and the ordered rollout.
