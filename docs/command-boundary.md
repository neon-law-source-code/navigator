---
publish: true
---

# The REST/OpenAPI command boundary

Every user- and tool-initiated data write — create, update, delete, and workflow actions — travels one shared command.
The four callers converge on it: the Dioxus runtime (web), the `navigator` CLI, MCP, and A2A. This keeps validation,
authorization, audit, and durable side effects defined once instead of re-implemented per adapter. The issue of record
is [navigator#355](https://github.com/neon-law-source-code/navigator/issues/355).

## The rule

A write has exactly one command implementation. An adapter is a thin translation over that command; it never carries its
own persistence logic.

- **Web (Dioxus).** Server functions and page/form routes render and delegate. The mutation lives in an `/app/api/*`
  command handler or a shared `portal` command module; the route is an adapter over it. Cookie-authenticated browser
  writes keep CSRF (`portal::csrf`, credential-keyed); bearer clients are CSRF-exempt but face the same actor, role, and
  embedded Rego checks.
- **CLI.** A subcommand either calls an authenticated `/app/api/*` route over HTTP (`cli/src/remote.rs`, bearer token)
  or, where it cannot depend on `portal`, calls the **same** shared `store` / `workflows` command the `/app/api` handler
  calls. Convergence is at the command layer, not necessarily over HTTP: the `navigator db project create` subcommand
  (`cli/src/project.rs::create`) and `POST /app/api/projects` both call the same `store::projects::open_matter`.
- **Seed reconciliation.** `navigator site import <MODEL_NAME> <SEED_FILE>` reads seed YAML locally and sends it
  with the bearer from `navigator site login` to `POST /app/api/seed`. The deployment resolves the glossary model and
  validates its `lookup_fields`, then performs lookup/create there; `--overwrite` changes only fields represented in the
  seed model. The CLI never reads database credentials.
- **Document upload.** `navigator site document upload --project <code> --file <path> --kind <kind>` reads the file
  locally and sends it with the same bearer to `POST /app/api/projects/{id}/documents`. `--kind` is required and must be
  an asset-lane value — the same enum OpenAPI publishes on that operation. The CLI never writes the store itself.
- **MCP.** A tool in `mcp/src/tools/` translates its arguments into a shared command. The `mcp` crate cannot depend on
  `portal`, so it converges at the `store` / `workflows` layer — e.g. `aida_link_person_project` calls
  `store::participation::add_participant` / `update_participant`, the same commands the participation `/app/api` door
  and the lawyer form use. That convergence is what makes one invariant hold at all three doors: the commands derive
  `participation` from `persons.role`, so none of the three can name one. A2A wraps the same tools behind its
  confirmation gate.

Where the command lives: `store` and `workflows` hold the persistence and durable-execution cores plus their typed error
enums; `portal` command modules hold adapter logic that must not live in `store` (matter-scope resolution, client-lens
checks). The `/app/api` handler and the web form are both thin adapters over that one command. Every `/app/api` route is
documented in `portal::api::documented_api_operations()` and `portal/src/openapi.rs`, guarded by
`server/tests/openapi_drift.rs`, and carries an authorization-matrix test across anonymous callers plus Owner, Admin,
Lawyer, Clerk, and Client. Role semantics follow [`access-model`](access-model.md): embedded Rego reads the system tier
(`persons.role`), and `participation` is derived from that same column rather than named by a caller.

## Carve-outs — paths allowed to write directly

These would be **system- and internal-initiated** writes, not user or tool commands, so they would not travel the
boundary. The allowlist in `cli/tests/command_boundary.rs` is currently empty: every write in `cli/src` and `mcp/src`
routes through the shared command boundary, with no exemption. A new carve-out needs a documented reason added both here
and to that allowlist.

Categories outside `cli/src` and `mcp/src` altogether are not part of that allowlist, but they are still
system-initiated rather than user- or tool-initiated, so they do not travel the boundary either:

- **Schema migrations and canonical catalog seed** — `store` migrations and `store::seed`. Firm-owned reference and
  bootstrap data, not a runtime write.
- **Restate durable workers** — the `workflows*` crates. A journaled handler *is* the command execution; it must never
  make an HTTP call back into `web` for its own durable side effect (see [`durable-workflows`](durable-workflows.md)).
- **Inbound webhooks** — DKIM-verified inbound email and equivalent event ingress. System-initiated by an external
  event, not a user POST.
- **Git smart-HTTP / LFS transport** — the repo-hosting path serves git's own protocol, not application commands.
- **Archive and billing batch jobs** — scheduled/library-driven writes triggered by a job, not a user action.

## Enforcement

`cli/tests/command_boundary.rs` ratchets the machine-caller adapter layers: neither `cli/src` nor `mcp/src` may
construct an entity `ActiveModel` for a write outside the (currently empty) carve-out allowlist above, and the allowlist
cannot rot (a listed file that stops writing fails the test). A new inline write in a CLI subcommand or MCP tool fails
there, pointing the author at the shared command. Web-adapter convergence is enforced per slice by each door's
authorization-matrix test plus the OpenAPI drift guard.

See also [`access-model`](access-model.md) (who may write), [`rego-policy`](rego-policy.md) (the decision engine), and
[`workspace-layout`](workspace-layout.md) (the crate map).
