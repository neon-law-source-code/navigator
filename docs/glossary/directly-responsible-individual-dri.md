---
title: "Directly Responsible Individual (DRI)"
description: "\"DRI\" names two distinct accountability markers, one per domain, and neither implies the other."
---

"DRI" names two distinct accountability markers, one per domain, and neither implies the other:

- **A matter's DRI** — this entry. `person_project_role.is_lawyer_dri` / `is_client_dri`, seeded at matter-open, scoped
  to one [Project](project.md).
- **A Firm's Admin DRI** — `person_firm_role.is_dri` (ENG-499), scoped to one [Firm](firm.md), unrelated to any matter.
  See [Person–Firm Role](personfirm-role.md) for its invariant (exactly one per active Firm) and the commands that
  enforce it.

The rest of this entry is the matter-level marker. The natural [Person](person.md) accountable for a [Matter](matter.md)
— the name to ask "where does this stand?". Every matter carries **two sides** of accountability, seeded at matter-open,
and each side is a **set**:

- **Lawyer DRIs** — the attorneys/admins accountable for the matter inside the firm. The opening lawyer by default
  (else the firm principal, resolved by role). A matter always has at least one; it may have several, which is how one
  matter is genuinely two lawyers' responsibility rather than one lawyer's with a note.
- **Client DRIs** — the **client-side** people accountable for the matter. Each must be a real, pre-existing
  [Person](person.md) with `role = client` (never a firm attorney — a matter's client of record is a client). The client
  field exists before the project; the matter is opened *for* that client.

Each is an **accountability marker on the person's participation row** — `person_project_role.is_lawyer_dri` and
`is_client_dri`, booleans any number of rows per matter may carry. A DRI is therefore a matter person **by
construction**: the marker lives on the membership row, so there is no way to name a DRI who is not on the matter. The
participation ledger still records the broader involvement/access it always did (a `client` participation for portal
visibility, co-counsel, other lawyers); the DRI flags single out *who is accountable* on each side. Participation
answers "who's involved and what can they see?"; the flags answer "who owns this?".

Two rules bound the sets, both enforced in `store::participation` rather than by the schema — SurrealDB has no partial
unique index, so the cardinality rules live in Rust where they can be tested:

- **The lawyer set is never empty.** The last accountable lawyer cannot step off and cannot be removed from the matter.
- **Changing either side is authorized and audited.** A matter's lawyer DRIs govern their own side — any of them may
  add or remove any other — while the client side takes the lawyer tier and above. Owner and Admin pass both. A matter
  whose lawyer set is empty is named by a lawyer-tier participant already on it, or by Owner/Admin. Every designation
  and removal appends a `relationship_log` entry naming the actor, the matter, and the person moved.

A matter is opened against a pre-existing [Entity](entity.md), **for** a pre-existing client, **and** always on a
[retainer](engagement--retainer.md) — a project is not official until a retainer exists. The matter-open service
validates the entity and the client role before any row is created.

- Schema and command: [`store::projects`](../../store/src/projects.rs) (the `is_lawyer_dri` / `is_client_dri` fields);
  `store::projects::designate_dri_in_surreal` writes the membership record and its marker as one act
- Rules, authorization, and the audit write: [`store::participation`](../../store/src/participation.rs)
- Nightly digest: the `DriDigest` Restate workflow
  ([`workflows-service/src/dri_digest.rs`](../../workflows-service/src/dri_digest.rs)) reads
  `store::projects::dri_digest` and posts firm ops one Slack line per project naming both DRI sides, fired nightly by
  the `dri-digest-trigger` `CronJob`
