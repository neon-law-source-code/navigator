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

To discover the repository and Drive coordinates derived from a Project, run the read-only Project check with its code:

```bash
navigator project doctor --project <project-code>
```

Check this repository locally. The gate walks the whole tree from the root it is run in, fixing what is safe to fix and
reporting what needs a person:

```bash
navigator project gate
```

Lint an arbitrary directory that is neither a Navigator checkout nor a Project repository:

```bash
navigator validate
navigator validate /path/to/tree
```

Open a matter through the logged-in site, against a pre-existing client and either an existing entity or a `Human`
entity this command creates for a solo client:

```bash
navigator project create --name "Acme LLC — Formation" --code acme-llc-formation \
  --client-email <client@example.com> --entity-name "Acme LLC" --attest
navigator project create --name "Shook Estate" --code shook-estate \
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

Before opening another instrument, inspect the matter's private notation inventory. A lawyer can then read the filed
answers and their source provenance for a particular Notation:

```bash
navigator site notation list --project <project-code>
navigator site notation answers <notation-uuid>
```

Both read matter content, so both require a firm-side participation row on the matter — of every tier, Owner and Admin
included. Being Owner or Admin is not itself a key to a matter's work product: seat yourself on the matter first, or
these answer `404`. A Clerk is not lawyer tier and gets `403`.

Before cutting a release, check that this repository's self-referencing GitHub Actions pins still name
`[workspace.package].version`. The reusable workflows and composite actions under `.github/` reference this repository's
own actions by an absolute tag rather than by the ref the caller used, so a bump that does not sweep them publishes a
gate no consumer can run:

```bash
navigator ops release pins
```

It takes no version — the manifest is the answer. It walks `.github/` and `docs/examples/`, exits `0` when every pin
agrees, and exits `2` naming the file and line of each one that does not. Comments and the named placeholders
(`@YY.M.D`) are excluded; every other ref is a pin, including one that is not a release version at all. `ops release
version`sweeps the pins as it writes the manifest, so this verifies that bump rather than replacing it;`ci.yml` runs the
same command in its always-run gate.

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
navigator notation preview templates/notations/neon_law/onboarding.md
navigator notation preview onboarding                 # a name, looked up under templates/
```

A name is looked up in three places, in order: a Project repository's flat `templates/`, Navigator's own nested
`templates/notations/`, and the catalog compiled into the binary. A checkout always wins, so an author previews the file
under their cursor; the bundled tier is what lets the command work in a directory that has no templates at all. The
printed provenance line says which one answered.

Run from a Project repository (`navigator.yaml` two directories up), this pushes the template as a **draft** to that
Project's real portal and opens a browser at it — the production show page, production chrome, and production
questionnaire engine, not a local imitation. The draft is stored and addressable but explicitly **not run**: no
Notation, no workflow instance, no PDF.

Outside a Project repository, or without a login, pass `--offline` to render the same show page locally instead as a
**lint**, not a preview: nothing is pushed and nothing is stored. It binds only to `127.0.0.1`, always on an OS-assigned
port chosen at random so two lints can run at once, and prints the URL. Stepping the questions is hydration, so it needs
the Dioxus client bundle `navigator dev build-webapp` stages; without one the page still renders every question and the
graph, and the command says so rather than leaving a dead "Next" button unexplained.

## Rendering a template to PDF or Word

```bash
navigator notation pdf templates/notations/neon_law/onboarding.md --out /tmp/onboarding.pdf
navigator notation word templates/notations/neon_law/onboarding.md --out /tmp/onboarding.docx
```

Both validate the file against the same rule set as `navigator project gate` first, resolve the render frame from the
template's own `kind:`/`output:` frontmatter, fill any `{{code}}` placeholders passed with `--answer`, and compile the
result in pure Rust. See [`pdf/README.md`](../pdf/README.md) for the render pipeline and output formats.

You do not need a site to work locally. Use `navigator project gate`, the `navigator notation` authoring commands, and
the KIND-backed `navigator dev` loop; seed a local catalog with `navigator site seed` when that command's local store
and storage environment are available, or import deployment data with `navigator site import` after logging in.
