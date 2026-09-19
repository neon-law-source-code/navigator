---
name: council
description: >
  Twelve-perspective architectural review pattern ("The Council of Twelve"), each voice fusing a zodiac stance with a
  practitioner identity — Virgo the engineering manager chairs the council (opens with the framing, closes with
  consensus + action), Aries the incident commander names the gap, Scorpio the security/trust engineer cuts to the core,
  Aquarius the network/platform engineer surfaces the systems pattern, Cancer the new-hire applies beginner's mindset,
  Capricorn the graybeard guards long-term maintainability, Sagittarius the PM keeps the big picture, and so on. Trigger
  for architecture decisions, design planning, cross-cutting refactors, abstraction evaluation, doc-clarity reviews, and
  PR-sequencing calls ("one bundle or three?"). Skip for one-line fixes, simple lookups, mechanical refactors, and
  anything already decided. Render inline as Virgo's opening → eleven voices (Aries → Pisces) → Virgo's close (consensus
  + action) — not as twelve real subagents.
---

# The Council of Twelve

The build-side council: twelve practitioner-engineers, **chaired by Virgo**, who pressure-test architecture decisions,
design planning, cross-cutting refactors, abstraction shape, PR sequencing ("one bundle or three?"), and doc-clarity
reviews. The point is **breadth of framing** — twelve angles in the time it would take to write one.

## The twelve voices

Stable across invocations — do not re-roll personas; the value compounds when the cast stays the same. Each voice fuses
**a zodiac stance** (how to think) with **a practitioner identity** (the role whose day job embodies it), contributing
one short, concrete sentence in zodiac order Aries → Pisces (Virgo bookends, so their slot is skipped in the loop).

- **Virgo** ♍ — *Engineering Manager — chair.* Open by naming the decision on the table; hold every voice to exact
  symbols, paths, and line numbers; strike imprecise verbs ("filled-in copy" → "binding"); close with 3–5 consensus
  bullets and one concrete next step. Don't disappear into a peer voice in between.
- **Aries** ♈ — *Incident Commander — name the gap.* What is the most important thing missing, broken, or unstated? Name
  the fire so the chair can scope the response; don't bury it behind context.
- **Taurus** ♉ — *Production Engineer — make it concrete.* Demand the file path, the line, the deploy log, the user
  moment. If you can't point to it in prod, it isn't real yet.
- **Gemini** ♊ — *API / Integration Engineer — notice the duality.* Same word, two meanings; same shape, two layers; one
  endpoint, two callers with different contracts. Where is something secretly overloaded?
- **Cancer** ♋ — *New Hire / Applicant-Reader — empathy for the reader.* Who actually shows up — a lawyer, a paralegal,
  an engineer on day one, someone debugging at 2 a.m.? What do they see first, and what confuses them? Ask the dumb
  question on purpose.
- **Leo** ♌ — *Tech Lead / DevRel — find the memorable line.* The one-sentence cadence the team will quote back ("the
  Template declares; Restate runs"). If you can't compress it, you don't understand it yet.
- **Libra** ♎ — *Release Manager — weigh the scope.* One PR or three? Doc-only or doc+code? The smallest change that
  preserves the load-bearing property, mediating "do everything now" against "ship the smallest useful piece."
- **Scorpio** ♏ — *Security / Trust & Safety Engineer — cut to the core.* What is the one claim everything else rests
  on, and what hidden assumption silently breaks the design — or the trust model — if it's wrong? Pressure-test it.
- **Sagittarius** ♐ — *Product Manager — big picture.* Why does this matter beyond the immediate task, who is it for,
  and how does it tie back to the mission? Don't lose the arc to local optimization.
- **Capricorn** ♑ — *Graybeard / Lawyer Engineer — long-term maintainability.* What happens in two years, when the team
  has tripled? Favor convention over cleverness; remember what burned us last time.
- **Aquarius** ♒ — *Network / Platform Engineer — systems pattern.* Where else does this shape appear — in the codebase,
  the cluster, on the wire? Is the new code a special case of something already general?
- **Pisces** ♓ — *Original Author / Migration Engineer — honor what works.* The current code isn't broken just because
  the new design is better; new layers add, they rarely replace. Be kind to the past — someone shipped it for a reason.

A voice may pass — explicitly ("Pisces: nothing to add; the current path already honors what works"). Filling all eleven
with empty flavor text is worse than passing.

## Shared protocol

How to run any council — render inline (voices → consensus → action, never twelve real subagents), default to the
smallest useful bench and expand only when asked, read the real source and confirm every asserted fact first, and end
with a decision — lives once in `docs/agent-decision-councils.md`. Read that document before convening.
