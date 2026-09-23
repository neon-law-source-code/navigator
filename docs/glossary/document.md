---
title: "Document"
---

A matter document — a project-scoped [Asset](asset.md) carrying the metadata callers see (`filename`, `kind`, `source`,
`received_at`) alongside the byte pointer. `kind` is a closed asset-lane classification from
[`rules::kind::Kind`](../../rules/src/kind.rs) (`valid_for(Lane::Asset)`). `POST /app/api/projects/{id}/documents` and
`navigator site document upload` require it; omitted or blank is `400 kind_required`. Inbound email attachments still
file as `unclassified`.

> **Source of truth = object storage plus the assets row.** When the application generates or proxies a document (a
> rendered retainer PDF, a raw inbound email body), the bytes land in object storage via
> [`cloud::StorageService`](../../cloud/) and the `assets` row is the canonical pointer. A Project repository's root
> `documents/` directory is the source-side document contract: it holds committed YAML pointers and, temporarily,
> Git-ignored files staged for `navigator site sync`. Sync uploads each staged file, replaces it with its pointer, and
> removes the local bytes. `documents/` is a sibling of `apps/` and `templates/`, never part of the portal bundle and
> never a second document store. Raw legal-document bytes never enter Project Git.

- Schema: [`asset` in `navigator.surql`](../../store/src/schema/navigator.surql) Queries:
  [`store::assets`](../../store/src/assets.rs) Lives in: the `asset` table in SurrealDB (document-shaped rows)
