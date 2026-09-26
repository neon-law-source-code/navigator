---
title: "Verification"
---

Evidence that a licensed human checked a citation before it was filed. A **domain record with an audit trail, not
telemetry**: it is diligence, it may need to be produced, it needs retention rules, and it carries the quote and the
citation, which are document content.

Verification decomposes into three **independent** axes, each separately settable and separately displayed: whether the
citation is real and correctly formatted, whether the quoted text is accurate to the source, and whether the source
actually supports the assertion it is cited for. The third is the one a single boolean hides, and the one that catches a
real case, accurately quoted, cited for something it does not say.

Every axis seeds `unverified`. Recording an axis as passing when nobody checked it overclaims — it asserts diligence
that did not happen — which is worse than having no verification at all.

A Verification **must name the revision it verified**. It pins the commit SHA of the draft it was checked against, the
same seam a committed notation pins. A verification that does not name its revision is worthless the moment the draft
moves: the citation may still be right, but nothing records that anyone confirmed it against the current text. When the
draft moves, axes that made a claim about the text are carried to `stale` rather than silently retained.

The corresponding telemetry event carries identifiers and outcomes only — verification, project, and authority ids, the
axis, the outcome, the verifier, the revision SHA, and a duration. No quote, no citation string, no proposition.

- Vocabulary: [`rules::citation`](../../rules/src/citation.rs) · Schema:
  [`verification` in `navigator.surql`](../../store/src/schema/navigator.surql) Queries:
  [`store::verifications`](../../store/src/verifications.rs) Lives in: the `verification` table in SurrealDB
