---
title: "Project"
---

A **[Matter](matter.md)** in client English. The durable container every [Notation](../notation.md#notation) lives in.
Has a `status` (`open`, `closed`, `archived`) and is **always opened against an [Entity](entity.md)** — a legal
organization, or a `Human` entity for a solo natural person. The `entity_id` FK is `NOT NULL`: a matter without an
entity is a bug.

Lifecycle status changes move through the shared transition command (`store::projects::transition_project_with_reason`):
the REST door is `POST /app/api/projects/{id}/lifecycle`, which the CLI's `projects close` calls, and the
`close_project` MCP tool calls the command directly. The descriptive `PATCH /app/api/projects/{id}` never touches
`status`; it rejects the field outright rather than accepting and forwarding it, so `closed_at` — derived only inside
the transition command — cannot be bypassed by a partial update reaching it through a second door. Close and archive
transitions may carry an RFC 3339 `effective_at` between matter-open and now; the command derives `closed_at` from that
value so an existing retention start can be corrected. Without it, a new close starts at the server's current time and
an existing stamp is preserved. A first close requires one of four pitch reasons or three active-matter reasons,
matching the pre-close onboarding classification; a pitch also requires an offboarding document on file. Reopen accepts
no effective time and clears both `closed_at` and `closure_reason`. A null reason on a historical closed row remains
unknown rather than being inferred.

**`source_state`** is a *derived* read-only signal on the lifecycle projection (`GET /app/api/projects/lifecycle`),
never a stored column — [`store::project_surfaces::source_state`](../../store/src/project_surfaces.rs) computes it
purely from `repository_url`, `forge_provisioned_at`, and `git_initialized_at`. `not_enabled` (no repository requested),
`unknown` (a repository URL is recorded but this deployment's own provisioning pass never stamped it — a direct edit or
a pre-stamp row), `attached` (provisioned, no validated source committed or imported yet), and `initialized`
(provisioned and carrying validated source) are the states a row can reach today; `pending` and `failed` are reserved
for an asynchronous provisioning path nothing in this codebase writes yet.

**Every Notation belongs to exactly one Project.** The schema enforces this with a `NOT NULL` `project_id` FK on
`notations`. A Notation without a Project is a bug.

Opening a Project never opens a Notation with it. The lawyer creates the engagement afterwards, on the Project, like any
other Notation — through `web`, the CLI, or Navigator MCP. Every door works this way: none auto-creates a retainer
alongside the matter, and Navigator MCP's `create_notation` names the Project it acts on rather than opening one of its
own.

Each Project has **one** deployment-scoped source repository, named for its `code`, holding that Project's notation
templates under `templates/`, client portal under `apps/portal/`, and source-side document pointers under `documents/`.
That root `documents/` directory contains committed YAML pointers and only temporarily holds Git-ignored bytes staged
for `navigator site sync`; it is not portal content or a document store. Legal files, client material, answers, and
produced documents remain in the deployment's private documents bucket (prefix `projects/<code>/documents`) and
Navigator [Assets](asset.md). Google Drive stays as a per-Project ingest dropbox — Workspace users drop files in;
Navigator copies them into the documents bucket and never treats the folder as a live store.
[`project-repositories`](../project-repositories.md) is the canonical deployment map and source boundary.

`project.code` is **lowercase letters, digits, and single hyphens**, alphanumeric at both ends, at most 80 characters —
enforced by [`store::projects::is_valid_code`](../../store/src/projects.rs) and the SurrealDB `project_code` unique
index. No uppercase, no underscores, no other punctuation, and no spaces.

**The code is the matter's URL.** Its show page is `/app/projects/{code}` and its client portal is
`/app/projects/{code}/portal/`; the internal UUID appears in neither. That holds because both directions read the `code`
column and neither consults the id — `portal::dioxus_app::project_show_path` writes a code into every link Navigator
renders, and `project_id_from_path` reads one back. A lowercase UUID is itself a well-formed code, so nothing could
refuse one on sight; what keeps ids out of URLs is the lookup, not the shape of the segment.

The code is **required at matter-open** and is **stored exactly as the caller supplies it** (normalized for case and
whitespace by `store::projects::normalize_code`, then checked against `is_valid_code`'s shape and reserved-word rules).
`store::projects::open_matter` never generates or appends anything to it: the code is a coordinate the caller already
committed to elsewhere — the `project:` value in a repository's `navigator.yaml`, the matter's Drive folder name, the
Notion `Project code` URL — so Navigator inventing a different one would strand those bindings the moment `project.code`
(`READONLY`) is written. A collision with an already-open matter's code is refused as `OpenMatterError::CodeConflict`
(surfaced as an HTTP 409 or an MCP `conflict`), not silently resolved; the caller picks a different code and retries.
`store::projects::code_from_name` still exists — it derives a default stem (name plus a short generated suffix) for the
one caller that has no operator-supplied code to begin with, the self-serve retainer walk (`portal::retainer_walk`).
Uppercase and underscores stay out deliberately: the code is also the repository name (see
[`project-repositories`](../project-repositories.md)).

**The code is immutable.** It is chosen once, at matter-open, and never changes — not on a client rename, not for a
nicer slug, not ever. `code` addresses things Navigator does not own: the matter's route (`/app/projects/{code}`), its
portal mount (`/app/projects/{code}/portal/`), and its documents-bucket prefix (`projects/{code}/documents`) all key off
the spelling picked at creation. `project.code` is `READONLY` in the SurrealDB schema, so a direct write that tries to
change it is refused by the engine; `UpdateProjectCommand` carries no `code` field. There is no rename path. The refusal
is a rule with a reason, not an absence waiting to be filled in.

**`brand` records which house [Brand](brand.md)'s storefront the client came through.** A closed key from
[`BrandKey`](../../views/src/brand.rs) (`neon`, `delete-your-data`, `lawyer-shook`), `NOT NULL`, `DEFAULT 'neon'` for a
row written before the field existed. Written by the server from the request's resolved `Host:` header at matter-open —
`store::projects::open_matter` (the lawyer form, the CLI, the JSON API) and the self-serve retainer walk both set it
this way — and never accepted from a client-submitted form or JSON field; `UpdateProjectCommand` carries no `brand`
field, so it cannot be changed after open, the same immutability `code` has. `store` does not depend on `views`, so
`Project::brand` is a plain validated `String`; the SurrealDB schema's `ASSERT` is the single source of truth for which
values are valid, not a shared Rust enum.

**`firm_id` records which [Firm](firm.md) owns the matter.** Distinct from `brand`: the brand is the door, the firm is
the house. The self-serve retainer walk (`portal::retainer_walk::start_post`) writes it at matter-open from
`store::firms::firm_id_for_brand_key` for the request brand, and refuses to open when no Firm wears that key. Existing
rows that still have none are pointed at the anchor Firm by the boot backfill. `UpdateProjectCommand` does not accept
`firm_id` from a client-submitted form.

Object-storage artifacts (rendered PDFs, signed documents, generated exports) live in
`gs://YOUR_PROJECT_ID-assets/projects/{id}/` for machine reads, and the nightly store→Parquet snapshots are immutable
objects in GCS — so deleting a Project's database rows never deletes its archives.

Working files live under the documents-bucket prefix `projects/{code}/documents` — a key convention in the deployment's
private documents bucket, not a bucket per Project. Google Drive is the Project's ingest dropbox, with one folder per
matter named for `project.code`. Workspace users drop files there, and Navigator copies them into the documents bucket.
Drive never serves content or receives CI publishes. Project participation grants Navigator and deployed application
access, never source-forge access. `store::project_surfaces` creates or adopts the handles. Their retry API/CLI are
`POST /app/api/project-surfaces/{id}` and `navigator project setup <code>`.

- Schema and commands: [`store::projects`](../../store/src/projects.rs) ·
  [`store/src/schema/navigator.surql`](../../store/src/schema/navigator.surql)
