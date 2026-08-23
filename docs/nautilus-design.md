# Neon Law Nautilus — screening-shield design

Nautilus is the firm's $66/month consumer-report **screening shield**. A licensed attorney disputes the inaccurate
tenant-screening and employment background-check reports that keep a person out of housing or a job, answering the
consumer reporting agency under the consumer's federal Fair Credit Reporting Act rights — by letter, with an attorney
signing every one. It runs on the inbound-email engine and the `@approve` attorney-approval gate that already serve the
firm in production. The firm publishes no per-service marketing page for Nautilus; the offering is priced and engaged
through `/contact`.

This document is the canonical compliance contract for the offering. The Restate workflow PRs (intake, triage,
consumer-report dispute, reinvestigation review, referral) each cite it rather than re-deriving the scope boundary.
Every statutory claim below is grounded in an official U.S. government source so a future reader can re-verify it as the
law moves.

## The scope boundary (read this first)

Nautilus v1 is a **dispute-correspondence shield only**. It is what it does — and just as load-bearing, what it
deliberately is not:

1. **A flat legal fee, never contingent.** The fee is a flat **$66/month** for legal representation in disputing
   inaccurate consumer reports. It is never a percentage of anything, never contingent on a report changing, and never
   sold by outbound telephone solicitation. The number stays $66 whether the report has one error or ten.
2. **Not credit repair.** Nautilus disputes information that is **inaccurate**; it does not sell to "improve," "repair,"
   or "boost" a credit score or rating. That distinction is what holds the product clear of the credit-repair regime
   (below), and it is a hard rule for every marketing surface.
3. **No litigation.** An FCRA damages suit, a collection or unlawful-detainer lawsuit, a summons — that is litigation,
   referred to litigation counsel through `/contact`, never answered as correspondence.
4. **We assert the client's own rights; we do not become a user of consumer reports.** The consumer is entitled to their
   own reports for free (below); Nautilus starts there rather than pulling reports on the client, so it never takes on
   permissible-purpose obligations as a report *user*.

These four hold the product clear of the regime that would otherwise reach it, and keep it inside the firm's
no-litigation, access-to-justice identity. Each carve-out is grounded below.

### Why the Credit Repair Organizations Act (CROA) does not reach us

CROA reaches a "credit repair organization" — a person who, for money, provides services for the express or implied
purpose of "improving any consumer's credit record, credit history, or credit rating," or advice about doing so (15
U.S.C. § 1679a(3)). It does not reach Nautilus, on two grounds:

- **Not for the purpose of improving a credit rating.** Nautilus's purpose is an **accurate** consumer report — it
  disputes tenant-screening, background-check, and consumer-report items that are *inaccurate* (a mixed file, a
  dismissed or sealed case, a record that should have aged off) under the consumer's FCRA accuracy rights. It does not
  offer to raise a score or improve a rating, and no marketing surface may say so.
- **The work is the practice of law by a licensed attorney.** A licensed attorney owns every matter and signs every
  dispute letter (see UPL, below); the fee is a legal fee for that representation.

The bright line is therefore in the copy and the intake, not only in this doc: **"dispute what is inaccurate," never
"improve your credit."** A future tier that marketed score improvement would take on the CROA analysis deliberately; it
is out of v1.

- 15 U.S.C. § 1679a:
  <https://uscode.house.gov/view.xhtml?req=granuleid:USC-prelim-title15-section1679a&num=0&edition=prelim>
- 15 U.S.C. § 1679b (prohibited practices, incl. advance-fee):
  <https://uscode.house.gov/view.xhtml?req=granuleid:USC-prelim-title15-section1679b&num=0&edition=prelim>

**Open item for the attorney of record before launch.** Several states run a Credit Services Organization Act analog
(e.g. California Credit Services Act, Nev. Rev. Stat. Chapter 598, Wash. Rev. Code Chapter 19.134) that can reach
dispute services and typically exempts attorneys rendering services in the practice of law. The exemption is
**conditional** on genuine practice of law in an attorney-client relationship. The attorney of record confirms the
per-state citation before the product launches in that state; it is not asserted here.

### We assert the client's free-disclosure right rather than buying reports

The consumer is entitled to their own consumer file for free, which is where Nautilus starts:

- **A free annual file disclosure from every nationwide and nationwide-specialty consumer reporting agency** — including
  the tenant-screening and employment-history agencies, which are "nationwide specialty consumer reporting agencies" (15
  U.S.C. § 1681a(w), (x); § 1681j(a)).
- **A second free disclosure after an adverse action** — when a landlord or employer denies the consumer based on a
  report, the consumer may request a free copy from the agency named in the adverse-action notice within 60 days (15
  U.S.C. § 1681j(b); § 1681m(a)).

Because the client obtains their own reports, Nautilus does not pull consumer reports *on* the client and does not take
on the permissible-purpose duties of a report user (15 U.S.C. § 1681b). If a matter ever needs the firm to obtain a paid
record, it is disclosed to the client first and passed through at actual cost with no markup.

- 15 U.S.C. § 1681a:
  <https://uscode.house.gov/view.xhtml?req=granuleid:USC-prelim-title15-section1681a&num=0&edition=prelim>
- 15 U.S.C. § 1681j:
  <https://uscode.house.gov/view.xhtml?req=granuleid:USC-prelim-title15-section1681j&num=0&edition=prelim>
- 15 U.S.C. § 1681m:
  <https://uscode.house.gov/view.xhtml?req=granuleid:USC-prelim-title15-section1681m&num=0&edition=prelim>
- 15 U.S.C. § 1681b:
  <https://uscode.house.gov/view.xhtml?req=granuleid:USC-prelim-title15-section1681b&num=0&edition=prelim>

### Unauthorized practice of law

A licensed attorney reviews and signs **every** outbound dispute letter via the `@approve` gate — the lawyer-reply
approval bridge already live in production. No letter auto-sends. The attorney is load-bearing: the fee buys an actual
lawyer in the loop, not software pretending to be one. Every Nautilus Restate workflow PR reuses this `@approve` gate as
its UPL control; none introduces an auto-send path.

### The engagement letter governs

A written engagement letter signed before representation begins states the exact scope, the $66 monthly fee, any
out-of-scope cost, and how either party may end the representation. The no-contingency rule lives in the engagement
letter itself, not only in marketing. Nautilus guarantees no particular result and does not promise a report changes —
it makes sure the consumer's accuracy rights are asserted and that the reporting agency deals with the consumer's
lawyer. The letter is compliant across the firm's California, Nevada, and Washington admissions.

## The core letter and its statutory hooks

The dispute letter carries role-scoped signature anchors so the **attorney** signs, and it goes out only through the
`@approve` gate.

- **Consumer-report dispute** — FCRA 15 U.S.C. § 1681i(a). A written dispute of an inaccurate item obliges the consumer
  reporting agency to conduct a free, reasonable reinvestigation within 30 days and to correct or delete what it cannot
  verify. The agency's own accuracy duty is 15 U.S.C. § 1681e(b).

Official sources for the operative text:

- 15 U.S.C. § 1681i:
  <https://uscode.house.gov/view.xhtml?req=granuleid:USC-prelim-title15-section1681i&num=0&edition=prelim>
- 15 U.S.C. § 1681e:
  <https://uscode.house.gov/view.xhtml?req=granuleid:USC-prelim-title15-section1681e&num=0&edition=prelim>

Two adjacent provisions the workflows track as the product grows: a **furnisher** must reinvestigate a dispute forwarded
by the agency (15 U.S.C. § 1681s-2(b)), and a consumer reporting agency that reports **public-record information for
employment** carries heightened procedures (15 U.S.C. § 1681k). The CFPB's Regulation V (12 C.F.R. part 1022) is the
FCRA implementing rule and the layer most likely to drift first, so workflow copy should track it as the regulation,
with the statute as the anchor.

- 15 U.S.C. § 1681s-2:
  <https://uscode.house.gov/view.xhtml?req=granuleid:USC-prelim-title15-section1681s-2&num=0&edition=prelim>
- 15 U.S.C. § 1681k:
  <https://uscode.house.gov/view.xhtml?req=granuleid:USC-prelim-title15-section1681k&num=0&edition=prelim>

## The on-chain record (Neon Law Node)

Every attorney-signed Nautilus letter can be anchored to the blockchain through Neon Law Node, reusing Node's Solana
rail: one transaction binds the firm wallet, the client wallet, and a **SHA-256 fingerprint** of the signed letter. The
letter itself stays private with the client; only the fingerprint goes on-chain, so the client can independently prove
the exact letter existed and was signed, without exposing its contents. Anchoring is **optional** and opt-in.

**Pricing — anchoring is included, not a second attestation fee.** Node is a $44 per-attestation product because that
fee buys a *new attorney attestation of a fact*. In Nautilus the attorney has **already** signed the letter under the
`@approve` gate and the $66/month fee, so anchoring records a fingerprint of an already-signed document; there is no
second attestation to bill. Only the Solana network fee (fractions of a cent) passes through at cost. Billing an
indigent client $44 per letter on top of the flat fee would be regressive and off-mission; Node remains the $44 product
for standalone attestations of new facts.

## The referral seams

Nautilus refers out the moment a matter leaves dispute correspondence:

- **Any lawsuit or summons** (an FCRA damages suit, a collection suit, an unlawful-detainer/eviction action) →
  litigation counsel through `/contact` (Sethi Legal). Never answered as a letter.
- **A viable FCRA damages claim** → litigation counsel. Asserting the accuracy right by letter is in scope; suing on a
  violation is not.

## Intake & portal UX contract

The Client Council's findings are requirements for the surfaces the workflows build:

- **One-tap forward.** Forwarding the adverse-action notice and the report is a single action that accepts a phone photo
  *or* a forwarded email — never a demand for a scanned PDF.
- **A sent-letters timeline.** The client sees each dispute letter, the attorney who signed it, the date sent, and the
  30-day reinvestigation deadline being tracked — so protection is visible, not asserted.
- **The trust line is unmissable.** "$66/month flat — we never take a percentage of anything, and this is not credit
  repair" appears on intake, pricing, and the portal header.
- **Privacy-safe notifications.** Neutral notification subject lines; the report detail — a criminal record, an eviction
  entry — lives only behind authentication, for the client who is ashamed of what the report shows.
- **Plain language.** The rights are stated plainly; the statute numbers stay in this design doc.

## Build sequence

Nautilus engagements are `projects` matters opened by `onboarding__` and closed by `closing__letter`. The workflows ride
the existing `workflows-service` Restate worker — one worker, no per-workflow pod — and the existing inbound-email
engine and `@approve` gate. Build order, each as one PR:

1. **01 — Intake & consumer-report dispute** (`fcra_dispute`, § 1681i; 30-day reinvestigation timer).
2. **02 — Inbound triage** — classify each inbound `.eml` (adverse-action notice, forwarded report, agency
   reinvestigation result) against active matters; the deadline-tracking spine.
3. **03 — Reinvestigation review** — the agency's § 1681i response (corrected/deleted vs verified-unchanged), surfaced
   to the client and queued for attorney review of the next step.
4. **04 — Furnisher dispute** (`furnisher_dispute`, § 1681s-2(b)) — the escalation when the agency verifies an item the
   furnisher's records do not support.
5. **05 — Referral** (lawsuit/summons or viable FCRA damages claim → litigation referral).

See [`docs/glossary.md`](glossary.md) for the Person / Entity / role vocabulary these workflows use, and the
[`agent-workflows.md`](agent-workflows.md) for the feature-first recipe each PR follows.
