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

You do not need a site to work locally. Use `navigator validate`, the `navigator notations` authoring commands, and the
KIND-backed `navigator dev` loop. `navigator project create` opens a matter directly against the local store, and
`navigator erd` introspects its schema; seed a local catalog with `navigator site seed` when that command's local store
and storage environment are available, or import deployment data with `navigator site import` after logging in.
