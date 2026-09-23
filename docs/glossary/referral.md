---
title: "Referral"
---

A [Matter](matter.md) the firm hears out but does not take on, handed instead to outside counsel whose practice fits it.
The firm's practice is flat-fee, document-driven work — company formation, governing documents, state and court filings;
contested courtroom work (litigation, appeals, contested hearings) is referred out. A Referral is **client-English, not
a schema noun** — like [Matter](matter.md) and [Engagement / Retainer](engagement--retainer.md), it names a thing a
lawyer says out loud, not a table. There is no `referrals` table: a Matter is the same row as a [Project](project.md) in
the database, and a referred Matter is simply one the firm closes (or never opens) after pointing the client to trial
counsel. The firm publishes no per-service marketing pages — the home page states the practice (litigation and flat-fee
transactional work) and quotes each engagement by email. The firm-footer disclaimer ("every legal matter is different,
and past results do not guarantee a similar result") in [`views/src/brand.rs`](../../views/src/brand.rs) covers
transactional and referred matters alike.
