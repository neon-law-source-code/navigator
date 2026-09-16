# Navigator CLI

The `cli` crate builds the `navigator` command, Navigator's control plane for every machine-bound workflow. It validates
and renders Notations, manages local KIND environments, operates deployments, maintains assets and forms, and drives
authorized matter work against a live site.

It serves developers, deployment operators, and authorized firm lawyers who need one auditable interface instead of
independent scripts or manual infrastructure steps. Centralizing those flows in Rust keeps validation, environment
selection, safety checks, and production boundaries consistent with the application.

The local staging lifecycle (`navigator dev staging`) runs under the `NAVIGATOR_ENVIRONMENT=dev` application profile;
staging is a lifecycle target, not an application environment.

Run `navigator --help` for the current command surface. Use [AGENTS.md](../AGENTS.md) for the local development loop,
[agent workflows](../docs/agent-workflows.md) for repository work, and [cloud operations](../docs/cloud-operations.md)
for deployment procedures.

## Common workflows

Log in to a deployment once; the CLI stores the short-lived bearer token locally:

```bash
navigator site login --host staging.neonlaw.com
```

List Projects visible to that login, then open one by its Project code:

```bash
navigator site projects list
navigator site projects open <project-code>
```

Admin-tier users can read every Project's lifecycle fields across the deployment:

```bash
navigator site projects lifecycle --json
```

To discover the repository and Drive coordinates derived from a Project, run the read-only Project check with its code:

```bash
navigator site projects doctor --project <project-code>
```

Validate a folder locally. The command walks Markdown and YAML files below the directory; omit the directory to use the
current folder:

```bash
navigator validate <dir>
navigator validate
```

Open a matter through the logged-in site, against a pre-existing client and either an existing entity or a `Human`
entity this command creates for a solo client:

```bash
navigator site projects create --name "Acme LLC — Formation" --code acme-llc-formation \
  --client-email <client@example.com> --entity-name "Acme LLC" --attest
navigator site projects create --name "Shook Estate" --code shook-estate \
  --client-email <client@example.com> --jurisdiction Nevada --attest
```

Create a notation on an existing Project through the logged-in site. Each Project keeps one onboarding notation and one
offboarding notation; later work uses other kinds. The shared catalog codes are `onboarding__letter` to open and
`offboarding__letter` to close:

```bash
navigator site notation create onboarding__letter \
  --project <project-code> \
  --client-email <client@example.com>
navigator site notation create offboarding__letter \
  --project <project-code> \
  --client-email <client@example.com>
```

## Exit codes for `--ci` commands

`navigator site import --ci` and `navigator site document verify --ci` first exchange the runner's GitHub Actions OIDC
token for a Navigator session at `/auth/ci/seed-token` or `/auth/ci/document-token`, then run the command's own gate.
Those are two different kinds of failure, so they exit differently:

- **`2`** — the ordinary gate failure: the mint succeeded and the command's own check found a problem (a reconciliation
  error, a document that failed live verification, and so on).
- **`3`** — the mint itself was refused before any gate ran (unauthorized, forbidden, or the deployment is
  unavailable). The refusal also prints a GitHub `::error::` workflow-command annotation naming the door
  (`/auth/ci/seed-token` or `/auth/ci/document-token`) and the server's own message, so the Actions log shows why the
  run was refused rather than a generic HTTP status.

## Reading a template the way a reader will

```bash
navigator notations preview templates/notations/neon_law/shared/onboarding_letter.md
navigator notations preview onboarding-letter          # a name, looked up under templates/
```

This serves that one template's `/notations/{slug}` show page on a local bind and prints the URL. It is the same axum
router the public site mounts, fed by the same projection, so the questionnaire section walks the template's own
declared question order in Navigator's real field controls and the workflow section draws its declared state machine. A
template that reads badly here reads badly published.

Nothing is persisted: no Notation, no Answer, no runtime signal, no store connection. Stepping the questions is
hydration, so it needs the Dioxus client bundle `navigator dev build-webapp` stages; without one the page still renders
every question and the graph, and the command says so rather than leaving a dead "Next" button unexplained. The command
runs anywhere, including inside a Project repository that has no `server/public` of its own — the stylesheets it needs
are compiled into this binary.

The preview binds only to `127.0.0.1`, uses an OS-assigned ephemeral port unless `--port` is given, opens no store,
holds no session, and persists nothing when its process stops.

## Walk a template through the local runtime

```bash
navigator notations run templates/notations/neon_law/shared/onboarding_letter.md
```

`notations run` validates the file, creates a fresh embedded store and temporary object directory, then creates only
synthetic client, lawyer, entity, and Project records. It walks the client-visible subset first, reports that boundary,
then completes the firm's full questionnaire and invokes the shared post-questionnaire drive. Because this workbench
never starts the `workflows-service` Restate worker that journals production transitions, it writes each embedded
transition to `notation_events` itself before printing, so its terminal output — the persisted answers, journaled
transitions, and resulting workflow state — reflects the same journal a real run leaves behind. Every invocation starts
empty and is discarded on exit. It reads no deployment selection or ambient Navigator configuration and never calls a
live provider.

You do not need a site to work locally. Use `navigator validate`, the `navigator notations` authoring commands, and the
KIND-backed `navigator dev` loop; seed a local catalog with `navigator site seed` when that command's local store and
storage environment are available, or import deployment data with `navigator site import` after logging in.
