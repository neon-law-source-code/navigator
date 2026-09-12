# A synthetic deployment tree

**These are not deployments.** Nothing here names a resource that exists, and nothing here is shipped. The real rows —
`neon-law-stg` and the production deployment — live in a private repository, together with the workflow that rolls them.
See [`docs/deployment-secrets.md`](../../../../docs/deployment-secrets.md).

This tree exists so the gates that read a deployment keep running in the workspace suite after the real tree left. It is
the input to `cli/src/devx/deployments.rs`, `cli/src/devx/ship.rs`, and `cli/src/devx/gcp/kms.rs` — the loader, the
`.sops.yaml` agreement check, the `store::deployment::WEB_REQUIREMENTS` parity gate, and the `SecretProviderClass`
projection plan.

## Why both rows declare `provisioned = true`

Every one of those gates skips an unprovisioned row, because such a row has blank coordinates and no `secrets.enc.yaml`
to encrypt against a key that does not exist. Both real rows are unprovisioned today, so **every one of those gates was
dormant** — passing without asserting anything, which reads exactly like passing. Declaring these two provisioned is
what arms them, and it is the reason this fixture is worth more than the tree it replaces.

## The two rows are not interchangeable

`store::deployment::WEB_REQUIREMENTS` scopes some requirements to one project and gates others behind a trigger key, so
one row cannot exercise both sides of either fork:

| | `example-deployment` | `example-automation-home` |
| --- | --- | --- |
| `NAVIGATOR_GCP_PROJECT_ID` | `example-deployment` | `neon-law-stg`, what `GITHUB_AUTOMATION_HOME_PROJECT` names |
| Webhook five-tuple | not applicable | required, and supplied |
| Dash0 | declined: no `DASH0_ENDPOINT` | declared: endpoint and dataset coordinates plus encrypted token |
| DocuSign | declined: no `DOCUSIGN_BASE_URL` | declared, so every DocuSign key is demanded |

The automation-home row carries a real project id because the requirement is scoped by that exact string in
`store/src/deployment.rs`. Its directory name is deliberately not that string: the row is synthetic, and a directory
called `neon-law-stg` sitting here would read as the real tree never having moved.

## The encrypted files are fake, and that is checked

`secrets.enc.yaml` here carries `ENC[FIXTURE]` in place of every ciphertext. `deployments::encrypted_key_names` only
asserts the shape — a `sops` metadata block, an environment-variable name per key, and an `ENC[` prefix per value — and
the shape is the whole point: it is what refuses a plaintext secret and what refuses a whole-file envelope. No test here
decrypts, so no KMS key, no credential, and no `sops` binary is involved.
