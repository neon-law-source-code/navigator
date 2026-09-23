---
title: "Asset"
---

One row in the `assets` table: the canonical store for a static byte artifact. It holds the byte pointer (content type,
byte size, SHA-256, and the storage key from [`cloud::StorageService`](../../cloud/); the bytes live in object storage)
plus, for a matter document, its metadata. Two shapes: a **document asset** (project-scoped, with
`filename`/`kind`/`source`/`received_at`) and a **bare content asset** (a template body or raw `.eml`, those columns
null). Document storage uses Project-scoped content-addressed keys (`projects/<code>/documents/<sha>`); bare content
uses `blobs/<sha>`. Each write lane dedupes by `sha256_hex`. Merges the former `blobs` + `documents` split (#449).
`visibility` (`internal`, the default, or `client`) gates whether a document asset reaches the client portal's
matter-detail listing and "download all documents" archive; every ingest call site states it explicitly (#782).

- Schema: [`asset` in `navigator.surql`](../../store/src/schema/navigator.surql) (SurrealDB; #1093, ENG-121) · Write
  lanes: [`store::documents::ingest_bytes`](../../store/src/documents.rs) (document assets),
  [`store::assets::ingest_content`](../../store/src/assets.rs) (bare content).

```text
┌─ asset ────────────────────────────────────────┐
│ id                     record                  │
│ byte_size              int                     │
│ content_type           string                  │
│ description            option<string>          │
│ filename               option<string>          │
│ inserted_at            datetime                │
│ kind                   option<string>          │
│ metadata               any                     │
│ project_id             option<record<project>> │
│ published_at           option<string>          │
│ received_at            option<string>          │
│ secondary_storage_key  option<string>          │
│ sha256_hex             string                  │
│ slug                   option<string>          │
│ source                 option<string>          │
│ source_received_at     option<string>          │
│ source_sender          option<string>          │
│ source_subject         option<string>          │
│ storage_key            string                  │
│ updated_at             datetime                │
│ visibility             string                  │
└────────────────────────────────────────────────┘
```
