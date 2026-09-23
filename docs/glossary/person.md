---
title: "Person"
---

A human contact. The system-wide tier — `owner`, `admin`, `lawyer`, `clerk`, or `client` — lives on this row in the
`role` field, not on the OIDC token. The Rauthy / Google id_token carries only `sub` and `email`; the callback handler
links that pair to a Person via the presenting provider's own subject column — `oidc_subject` for the primary slot,
`microsoft_subject`, or `apple_subject` — and reads `role` from the DB. `lawyer` means a person licensed to practice law
authorized for Navigator legal work, not a firm email or source-forge membership. See
[`docs/access-model`](../access-model.md) and [`docs/oidc`](../oidc.md).

This is a SurrealDB table and [`store::persons`](../../store/src/persons.rs) is the only module that reads or writes it.
Every `person_id` on another table is therefore an unenforced cross-engine id, resolved in Rust. A [Lead](lead.md) is a
public contact request, not a second directory: conversion and linking write this table and point `lead.person_id` at
the row.

One Person per mailbox is protected from forking by a claim in the `person_mailbox` table, whose record id is the
lowercased email, rather than by the UNIQUE `person_email_lower` index alone. The index is the backstop behind it — it
refuses a fork that is not a race, but racers write no shared key for the engine to conflict on, so the claim is what
serializes them (ENG-114). It matters here more than elsewhere because `role` is the authorization root: a forked
mailbox is one human carrying two roles. The claim moves when an edit moves the email and is released when the Person is
deleted, so a mailbox is reusable rather than locked out.

- Schema: [`person` in `navigator.surql`](../../store/src/schema/navigator.surql) Queries:
  [`store::persons`](../../store/src/persons.rs)

```text
┌─ person ──────────────────────────┐
│ id                 record         │
│ apple_subject      option<string> │
│ email              string         │
│ email_confirmed    bool           │
│ email_lower        string         │
│ family_name        option<string> │
│ given_name         option<string> │
│ inserted_at        datetime       │
│ is_admitted        bool           │
│ linkedin_url       option<string> │
│ microsoft_subject  option<string> │
│ middle_name        option<string> │
│ name               string         │
│ oidc_subject       option<string> │
│ phone              option<string> │
│ profile_image_url  option<string> │
│ role               string         │
│ title              option<string> │
│ updated_at         datetime       │
│ xero_contact_id    option<string> │
└───────────────────────────────────┘
```
