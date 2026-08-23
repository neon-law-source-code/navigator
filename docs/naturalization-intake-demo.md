# Walk the naturalization intake locally — `us__naturalization` with the CLI

This local demo opens a USCIS Form N-400 matter, answers intake through `navigator`, records lawyer approval, and
downloads the rendered form. All services run in KIND.

`templates/forms/united_states/federal/uscis/us__naturalization.md` defines ten questions and parks at
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
cargo run --release -p cli --quiet -- db catalog-seed templates
cargo run --release -p cli --quiet -- dev grant-lawyer
```

`catalog-seed` validates and idempotently seeds clean templates and referenced question codes; any skipped error-level
file makes it exit nonzero. `Seeded 0 workspace-shared template(s)` is valid. Run `grant-lawyer` **before** login so the
session carries the seeded role.

## 4. Sign in

```bash
cargo run --release -p cli --quiet -- login --host http://localhost:3001
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

## 6. Walk the questionnaire

```bash
cargo run --release -p cli --quiet -- site intake answer <notation-id>
```

Typed prompts cover two `YYYY-MM-DD` dates, phone, eligibility and marital choices, days abroad, and moral-character
disclosure. Client, birth country, and citizenship country are pick-lists over matter people and seeded jurisdictions.
Enter the printed row number or id; the answer stores the canonical id. See the [Using the Navigator
workshop](/workshops/use-the-navigator), which requires a signed-in firm-side account.

To script the same walk non-interactively, pass a `--select <question>=<row>` for each of the three pick-lists (the row
is the list number the walk prints, or the row's id) and one `--answer` per typed question, all in questionnaire order:

```bash
cargo run --release -p cli --quiet -- site intake answer <notation-id> \
  --select person__client=2 \
  --select country__of_birth=114 \
  --select country__of_citizenship=114 \
  --answer "1990-04-12" \
  --answer "2019-03-01" \
  --answer "702-555-0100" \
  --answer "five_year" \
  --answer "married" \
  --answer "45" \
  --answer "no"
```

Rows are sorted alphabetically; the example selects the applicant and Mexico. Read row numbers from an interactive walk
because seed differences can change them. The final answer persists intake and advances to `lawyer_review`.

### Answer from a transcript instead

If the applicant's answers are already on record — a recorded intake call, an email thread — hand the walk a plain-text
transcript and it pre-fills what it can:

```bash
cargo run --release -p cli --quiet -- site intake answer <notation-id> --transcript /tmp/n400-intake.txt
```

The offline coverage engine offers transcript proposals as Enter-to-accept defaults. Nothing is auto-accepted; confirm
or correct each proposal, and answer uncovered questions normally. Confirmed proposals become ordinary answer rows.

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

The command rebuilds, loads, and waits for the RestateDeployment. Then retry `intake answer`.

## Cleaning up

Stop `web` with `Ctrl-C`. Leave the persistent KIND fixture running; use `dev down` only for a deliberate clean rebuild.
