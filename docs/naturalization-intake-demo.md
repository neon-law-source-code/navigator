# Create a naturalization notation locally — `us__naturalization`

This local demo opens a USCIS Form N-400 notation, then hands questionnaire intake to the authenticated site. All
services run in KIND.

`templates/notations/forms/united_states/federal/uscis/us__naturalization.md` defines ten questions and parks at
[`lawyer_review`](glossary.md#lawyer-review) before generating the vendored N-400 AcroForm. The blank lives in the
assets bucket. First run [`navigator template forms sync`](gov-forms.md#vendoring) against this environment to upload a
prepared blank or verify its pin.

## Prerequisites

- Repository checkout on `main`.
- Running Docker.
- `kind`, `kubectl`, and `helm` on `PATH`; `dev up` reports missing tools.
- `rustup`; `rust-toolchain.toml` selects Rust.

Every command below runs from the repository root.

## 1. Start the dependency stack

```bash
cargo run --release -p cli -- dev up
```

This creates or reuses the persistent `navigator` cluster, restores port-forwards, and writes `.devx/env`. "Already
exists/alive" is success.

On `bind: address already in use`, clear stale port-forwards and rerun:

```bash
pkill -f "kubectl.*port-forward"
cargo run --release -p cli -- dev up
```

## 2. Boot `web`

In a second terminal, from the repository root:

```bash
set -a; source .devx/env; set +a
cargo run -p neon
```

Wait for `web listening` at the `.devx/env` base URL and leave it running.

## 3. Seed the database

Load the environment `dev up` wrote, then import the template catalog and pre-seed the lawyer user:

```bash
set -a; source .devx/env; set +a
cargo run --release -p cli --quiet -- site seed templates
cargo run --release -p cli --quiet -- dev grant-lawyer
```

`site seed` validates and idempotently seeds clean templates and referenced question codes; any skipped error-level file
makes it exit nonzero. `Seeded 0 workspace-shared template(s)` is valid. Run `grant-lawyer` **before** login so the
session carries the seeded role.

## 4. Sign in

```bash
cargo run --release -p cli --quiet -- site login --host http://localhost:3001
```

Sign in through local Rauthy as `lawyer@neonlaw.com` / `password`. The CLI stores its short-lived loopback token in
`~/.navigator.json`. Confirm:

```bash
cargo run --release -p cli --quiet -- site whoami
```

Expect `lawyer@neonlaw.com (lawyer)` and token lifetime.

## 5. Open the matter

```bash
cargo run --release -p cli --quiet -- site notation create us__naturalization \
  --client-email applicant@example.com
```

Copy the printed notation UUID. Use only synthetic or reserved-domain email addresses.

## 6. Continue intake in the site

Open the authenticated project intake page at `http://localhost:3001/app/projects/<project-code>/intake/<notation-id>`.
The site presents the questionnaire's typed and pick-list questions, records each answer, and advances the notation to
`lawyer_review` when complete. See the [Using the Navigator workshop](/workshops/use-the-navigator), which requires a
signed-in firm-side account.

## 7. Review as lawyer: approve and download

```bash
cargo run --release -p cli --quiet -- site notation status <notation-id>
cargo run --release -p cli --quiet -- site notation approve <notation-id>
cargo run --release -p cli --quiet -- site notation document <notation-id> --out /tmp/n400.pdf
```

Before approval, expect `lawyer_review` and `document_ready false`. Approval generates the N-400. Verify the downloaded
PDF answers. This demo sends nothing outbound. Signature dispatch is separate, and a licensed attorney must review a
real application before it leaves the firm.

## If the walk 500s after a schema-changing merge (version skew)

After changing worker/shared workflow code or a database contract, reload the worker; the pod otherwise retains its
loaded image and can reject a new write with HTTP 500.

Reload it through the CLI:

```bash
set -a; source .devx/env; set +a
cargo run -p cli -- dev worker-reload
```

The command rebuilds, loads, and waits for the RestateDeployment. Then retry the site intake page.

## Cleaning up

Stop `web` with `Ctrl-C`. Leave the persistent KIND fixture running; use `dev down` only for a deliberate clean rebuild.
