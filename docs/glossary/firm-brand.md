---
title: "Firm Brand"
description: "A Firm Brand links a Firm to the house-brand keys it can use."
---

Which house-brand keys a [Firm](firm.md) wears. The `firm_brand` table is the join: `firm_id`, a `brand_key`, and
timestamps. Unique on the pair, and unique on `brand_key` globally: one storefront key belongs to at most one practice.
Distinct from [Brand](brand.md), which is the storefront a request resolved to.

`store::firms::attach_brand` validates `brand_key` against the `brand` table (`store::brands`, ENG-496): a live row
carrying that key, not the closed `CLOSED_BRAND_KEYS` array directly. A Firm may therefore wear any brand a `brand` row
names, not only the nine compiled ones. `store::firms::CLOSED_BRAND_KEYS` names every entry in the compiled registry;
`store::seed` migrates them into `brand` rows on first boot so validation has a catalog from the start.

Every footer names the Firm actually wearing the request's resolved brand, not a compiled constant (ENG-589):
`webapp::firm_footer::resolve_firm_footer_model` reads the Firm's Entity for the legal name and `firm_brand` for the
"Our Family" row, in registry order with the current brand flagged, falling back to the compiled family
(`views::brand::firm_family`) filtered to reachable live brands plus the current brand only when no Firm wears the key —
so the row is on every page from first boot without advertising an unopened host. The `/app` footer
(`webapp::firm_footer::FirmFooter`) and the public chrome's footer draw from this one resolved model rather than
duplicating the lookup or rendering a second Firm's brands. Both also carry the firm's association memberships
(`views::brand::firm_memberships`) as one "Proud member of the …" line each, linking the association's own site; a
white-label bundle that renames the firm publishes neither row, because the family and the membership are Shook Law
PLLC's.

- Schema: [`firm_brand` in `navigator.surql`](../../store/src/schema/navigator.surql) ·
  [`store::firms`](../../store/src/firms.rs)
