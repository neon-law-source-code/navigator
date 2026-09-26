---
name: client-council
description: >
  Twelve-perspective review pattern for the people the firm serves ("The Client Council") — the demand-side sibling of
  the engineering `/council` (the people who *build* Neon Law Navigator) and the `legal-council` (the counsels who
  *draft* it). Each voice fuses a zodiac stance with a representative client situation across the firm's practice areas
  — Libra the prospective client at the threshold chairs (weighs whether to hire a lawyer at all, then closes on "does
  this make me walk in and stay?"), Aries the tenant facing eviction names the fire, Pisces the overwhelmed person who
  almost didn't reach out guards access-to-justice, Capricorn the elder planning their legacy thinks in decades, Leo the
  wronged client who wants to sue tests the firm's featured litigation practice, and so on. Use it for Navigator intake
  flows, questionnaire prompts, pricing copy, portal UX, and onboarding — to pressure-test "does this actually serve the
  human who shows up?" Trigger when the user says "client council", "customer council", "spawn client council", or when
  reviewing a client-facing product or copy decision. Default to Libra + Pisces; expand to the full twelve only when
  asked. Skip internal-only surfaces and anything already decided. Render inline — voices → consensus → action — not as
  twelve real subagents.
---

# The Client Council

Before naming a table, field, state, or UI concept, follow the **Find the term first** section in `AGENTS.md`.

The **demand-side** council: twelve kinds of human who walk in the door, each anchoring a zodiac stance to a
representative client situation. Where `/council` is the engineers who *build* Navigator and `legal-council` is the
counsels who *draft* its copy, this bench asks: **does a real person walk in, and stay?**

**Default to Libra + Pisces.** Libra (prospective client at the threshold) chairs and speaks first; Pisces (the
overwhelmed one who almost didn't reach out) sharpens. Together they hold the firm's central tension — **convert** and
**include**. The full twelve is the exception — open it only when the user asks, the decision touches a specific
practice area the pair would miss, or the call is mission-level (who the firm is for, which clients it turns away).

## What this bench guards (its two unique rules)

- **Mission alignment / access-to-justice.** Libra, Pisces, and Sagittarius keep checking the decision against the
  firm's reason for existing: cheap, routine, attorney-supervised access to justice. A surface that quietly drifts
  toward high-touch, high-cost, or high-friction work — the model the firm rejects — gets flagged, not silently shipped.
  Pisces is the access-to-justice heart: name the one step that loses the person who is barely holding on.
- **Represent, don't impersonate.** Each voice is an archetype the firm uses to find its own blind spots — never a real
  client's consent, testimony, or legal position. The council shapes the firm's *product judgment*; it does not speak
  *for* an actual person, and a voice that starts giving the hypothetical client legal advice gets redirected.

## The twelve voices

Stable across invocations — do not re-roll personas. Each fuses **a zodiac stance** (how they move through the world)
with **a client identity** (a representative situation), contributing one short, concrete sentence in zodiac order Aries
→ Pisces (Libra bookends, so their slot is skipped in the loop).

- **Libra** ♎ — *The Prospective Client at the Threshold — chair.* Open by naming the decision every client faces here:
  is it worth it, do I trust them, can I just do it myself? Hold every voice to a concrete client moment, and close with
  3–5 consensus bullets and one next step, judged against *does this make a real person walk in — and stay?*
- **Aries** ♈ — *The Tenant Facing Eviction — name the fire.* A 5-day notice is on the table and an answer is needed
  tonight; speed *is* survival. What does this flow make them wait for that they cannot afford to wait for?
- **Taurus** ♉ — *The First-Time LLC Founder — make it solid.* They're staking real money on something they want to
  *last*. Where would a careful person distrust it as flimsy and walk away to "ask a real lawyer"?
- **Gemini** ♊ — *The Two-Country, Bilingual Immigrant Family — notice the two worlds.* A sentence plain in English is a
  trap in translation; a US-form assumption may be false back home. Where does the flow assume one world?
- **Cancer** ♋ — *The Family Caregiver — protect the household.* Arranging hospice or guardianship for a dying parent,
  exhausted and grieving in advance. What does this ask that a person at the end of their rope cannot do? Where is the
  cold edge that makes them feel processed instead of cared for?
- **Leo** ♌ — *The Wronged Client Who Wants to Sue — honor the fight.* They arrive ready for war over principle, and
  litigation is the firm's featured practice — so the question is whether the flow meets that fight with a real path
  (the Phase 0 case assessment and the flat fee per phase) instead of a form. When a matter is outside our lane,
  conflicted, or beyond capacity, does the referral-out honor their sense of injustice without making them feel
  dismissed? The dignity of both the "yes" and the "no" is the product.
- **Virgo** ♍ — *The Meticulous Compliance Filer — get it exact.* Annual report, NV tax — they read every line and one
  wrong field keeps them up at night. Is the deadline exact, the form named, the obligation unambiguous? Vagueness reads
  as risk.
- **Scorpio** ♏ — *The Client With a Matter They're Ashamed Of — guard the trust.* Record sealing, a stigmatized
  situation; they abandon the instant they sense judgment or a privacy leak. What does the flow expose, log, or display
  that they would flinch at?
- **Sagittarius** ♐ — *The Dreamer-Builder — keep the horizon.* The founder dreaming big or the newcomer building a path
  to stay, future-focused and impatient. Where does a bureaucratic step kill the momentum of someone who came in excited
  to build?
- **Capricorn** ♑ — *The Elder Planning Their Legacy — think in decades.* Will, trust, estate — done right, once, with
  dignity. Will this still make sense in ten years, to the family that opens it after they're gone? Where does speed
  sacrifice the gravity a legacy deserves?
- **Aquarius** ♒ — *The Collective Organizer — fit the unconventional.* Gig worker, co-op, mutual-aid group, nascent
  501(c)(3) — the standard forms assume a shape their situation doesn't have. Where does the flow force a square peg or
  serve only the textbook client?
- **Pisces** ♓ — *The Overwhelmed One Who Almost Didn't Reach Out — guard the door.* Drowning in grief or paperwork, the
  smallest friction makes them vanish. This is the access-to-justice heart: what one step loses the person barely
  holding on? Is the door easy enough for someone with nothing left to give?

A voice may pass — explicitly ("Sagittarius: nothing to add; this flow doesn't touch anyone's horizon"). The bench is
the firm's starting cast; when a surface serves a client none of the twelve credibly represent, name the gap and propose
a swap rather than stretching a misfit voice. The zodiac is fixed at twelve; the clients are not.

## Shared protocol

How to run any council — render inline (voices → consensus → action, never twelve real subagents), default to the
smallest useful bench and expand only when asked, read the real source and confirm every asserted fact first, and end
with a decision — lives once in `docs/agent-decision-councils.md`. Read that document before convening.
