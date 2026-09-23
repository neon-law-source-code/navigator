---
title: "Firm"
---

An owning practice. The `firm` table is the tenancy boundary for [Projects](project.md) and for the people who work
them: `project.firm_id` points at the practice that owns the matter, and [`Person–Firm Role`](personfirm-role.md)
records who belongs to that practice. A Firm **is** an [Entity](entity.md): `firm.entity_id` is the legal person that
practice is (the seeded practice is `Shook Law PLLC`). A Firm is not a [Brand](brand.md). Brand is the storefront a
request resolved to; Firm is which practice owns the matter. `firm_brand` records which closed house-brand keys that
practice wears.

Distinct from [`firm_anchor`](../../store/src/schema/navigator.surql), which still pins exactly one [Entity](entity.md)
as the Firm-of-record on the [Conflict-Check Graph](conflict-check-graph.md). A deployment may hold several `firm` rows;
it still holds at most one `firm_anchor` claim. `person.role = owner` remains the one system-wide Owner tier; it is not
a value on `person_firm_role`. Owner reaches `/app/owner` to list every practice and its brands. Admin is scoped to the
firms they hold a `person_firm_role` row on: the people directory and the matter directory at `/app/admin` list only
those firms' rows.

Embedded Rego still does not isolate every project or person route by firm (`ENG-463`). The Owner listing and the two
Admin directories named above do.

`firm.status` is `active`, `suspended`, or `archived` (ENG-494). Owner, or a Firm's own Admin membership, edits a Firm's
name/status/entity (`store::firms::update`), changes or removes a person's membership (`store::firms::update_membership`
/ `remove_membership`), and detaches a brand key (`store::firms::detach_brand`) — each gated through the same
`FirmCapability::ManageMembership` check `add_membership` already used. None of these write `is_dri`; only
[`appoint_admin_dri`](personfirm-role.md) does, and each membership-removal door refuses a change that would leave an
active Firm without one. Deleting a Firm that still owns Projects is refused. The Firm detail view at
`/app/admin/firms/{id}` (`webapp::firm_show`) is where these are read together: a Firm's own fields, its brands, its
Admin-DRI standing, and every person on it, with an Edit link for whichever caller holds `ManageMembership` on it.

Owner opens a second (or subsequent) Firm at `/app/owner/firms/new` (ENG-585), naming its Entity and its first Admin DRI
in one submission — `store::firms::create`'s own atomic guarantee. That Firm's Admin DRI (or Owner) then edits its name,
status, and Entity at `/app/admin/firms/{id}/edit`.

The Firm show page also renders a trailing-30-day billing rollup (ENG-591), from
`store::xero_invoices::firm_thirty_day_rollup`, which sums every mirrored Xero invoice issued on one of the Firm's
Projects in the last 30 days, in cents, grouped by brand and by lawyer DRI (a Project with none groups under
`"Unassigned"`), and never sums across currencies — a Firm billing in two currencies gets two independent totals rather
than one misleading sum. `webapp::firm_invoice_graphs` draws the result as two horizontal grouped bar charts (invoiced
cents beside paid cents) in inline server-rendered SVG, following the no-charting-library precedent
`webapp::lawyer_dashboard`'s status pie already set. Every label the chart draws is a brand's or a person's display name
(or `"Unassigned"`) — never a Project code, matter name, or Xero invoice id, which stay out of this surface entirely.

- Schema: [`firm` in `navigator.surql`](../../store/src/schema/navigator.surql) ·
  [`store::firms`](../../store/src/firms.rs)

```text
┌─ firm ──────────────────────────────┐
│ id           record                 │
│ entity_id    option<record<entity>> │
│ inserted_at  string                 │
│ name         string                 │
│ status       string                 │
│ updated_at   string                 │
└─────────────────────────────────────┘
```
