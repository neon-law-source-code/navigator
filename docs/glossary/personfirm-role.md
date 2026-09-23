---
title: "Person–Firm Role"
---

A Person's membership at a [Firm](firm.md). Shaped like [Person–Project Role](personproject-role.md): `person_id`,
`firm_id`, a closed `membership` (`admin`, `lawyer`, `clerk`), `is_dri`, and timestamps. Unique on the
`person_id`/`firm_id` pair. It does not replace `person.role`. Clients do not get a row here — they reach a matter
through person–project participation. `owner` is not a membership value: the deployment-wide Owner identity stays on
`person.role`.

The command seam reads both referenced rows before writing, because a `record<>` link constrains the target table but
does not prove the row exists.

**`is_dri` is the Firm's Admin DRI marker — a different noun from a matter's DRI (ENG-499).** Every active Firm holds
exactly one: `person_firm_role.membership = 'admin'` and `person.role = admin`, never Owner, Lawyer, Clerk, or Client.
Firm creation is atomic with this designation (`store::firms::create` takes `admin_dri_person_id` and refuses anything
ineligible — there is no setup state a Firm passes through without one), and `store::firms::appoint_admin_dri` is the
only writer thereafter: an Owner-only, one-transaction transfer that clears the outgoing DRI and sets the incoming one
so a reader never observes zero or two. `store::firms::refuse_admin_dri_orphaning` is the guard every membership-removal
door consults before deleting a row or changing it away from `admin`, so a direct edit cannot orphan an active Firm's
designation either. `store::firms::admin_dri_invariant_report` is a read-only, deployment-wide scan for a Firm that is
missing, has multiple, or holds an ineligible designation regardless — `navigator ops firms doctor` prints it. None of
these repair a row; a reported Firm is fixed by a human appointing or transferring through the Owner surface. See
[Directly Responsible Individual (DRI)](directly-responsible-individual-dri.md) for how this differs from a matter's
lawyer/client DRI.

**A newly created Lawyer or Clerk joins a Firm as a standing rule, not a one-time backfill (ENG-495).**
`store::people_commands::create_person` grants the membership itself, right after the Person write: the creating surface
may name a Firm; left unnamed, it defaults to the deployment's anchor Firm (`store::firms::anchor_firm`, resolved
through `store::entities::firm_anchor_holder` → `store::firms::find_by_entity_id`). Owner and Admin membership stays
explicit — this door grants nothing for either — and a Client is never offered the write at all.
`store::seed::seed_firm_memberships` remains the separate sweep the canonical seed's own fixture people need, because
they are seeded `client` and promoted afterward, past the point this rule can see them.

- Schema: [`store::firms`](../../store/src/firms.rs) ·
  [`store/src/schema/navigator.surql`](../../store/src/schema/navigator.surql)

```text
┌─ person_firm_role ──────────┐
│ id           record         │
│ firm_id      record<firm>   │
│ inserted_at  string         │
│ is_dri       bool           │
│ membership   string         │
│ person_id    record<person> │
│ updated_at   string         │
└─────────────────────────────┘
```
