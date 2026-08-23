# Agent decision councils

Councils are repeatable review lenses, not products, real subagents, or personas. Use one inside a codebase action from
[`agent-workflows.md`](agent-workflows.md) when a linear pass could miss a stakeholder, trust boundary, or maintenance
cost. Skip councils for mechanical, trivial, or decided work.

## The three councils

- **Engineering Council** — the people who build Neon Law Navigator. Use for architecture, refactors, abstractions, and
  doc clarity. The normal form is the full twelve voices.
- **Legal Council** — the counsels who draft legal copy. Use before copy becomes a template, prompt, email, or
  engagement paragraph. Default to Capricorn + Scorpio; use the full twelve for mission-level or unusual practice-area
  copy.
- **Client Council** — the people the firm serves. Use for client-facing product, intake, pricing, portal, and
  onboarding decisions. Default to Libra + Pisces; use the full twelve for mission-level or practice-specific client
  surfaces.

Read first, then convene: every voice reacts to evidence.

## How to run a council

The `council`, `legal-council`, and `client-council` skills supply their casts and triggers. Every council:

- reads the real source first and verifies each asserted path, symbol, address, fee, entity fact, bar number, date, and
  citation against the repository or user;
- defaults to its named pair, or the chair plus the needed voice, expanding only when asked or when the decision is
  mission-level, governance-level, or outside that bench's coverage;
- renders inline as framing → voices → consensus → one action; it is parallel framing in one response, not twelve
  subagents, unless the user explicitly requests subagents; and
- ends with a decision. A real fork becomes an explicit user go/no-go, not an invented answer.

## Engineering Council

Use the Engineering Council for architecture, design, cross-cutting refactors, abstraction pressure tests, PR
sequencing, and documentation clarity. Virgo chairs: name the decision, require concrete paths and symbols, then close
with consensus and one action. The other voices contribute one concrete sentence in zodiac order:

- ♈ Aries, incident commander: name the missing or broken thing.
- ♉ Taurus, production engineer: make the claim concrete in a file, deploy, or user moment.
- ♊ Gemini, API/integration engineer: notice overloaded words, dual contracts, and layer confusion.
- ♋ Cancer, new-hire reader: ask what a first-time reader sees and misunderstands.
- ♌ Leo, tech lead/devrel: find the memorable line the team can repeat.
- ♎ Libra, release manager: weigh scope and sequencing.
- ♏ Scorpio, security/trust engineer: pressure-test the load-bearing assumption.
- ♐ Sagittarius, product manager: keep the mission and user impact visible.
- ♑ Capricorn, lawyer engineer: guard long-term maintainability.
- ♒ Aquarius, platform engineer: surface the broader systems pattern.
- ♓ Pisces, original author/migration engineer: preserve what already works.

Output shape: Virgo opens, facts if useful, eleven voices, Virgo closes with consensus, then the concrete action.

## Legal Council

The Legal Council is a council of counsels for the firm's legal drafting before copy becomes a template body,
questionnaire prompt, engagement paragraph, follow-up email, or public policy. It neither advises a client nor replaces
attorney review. Default to:

- ♑ Capricorn, managing partner/senior counsel: institutional memory, ethics opinions, bar-facing commitments, and prior
  incidents.
- ♏ Scorpio, ethics and compliance counsel: the fiduciary duty, conflict, UPL, candor, or trust claim everything rests
  on.

Use the full bench only on request, for an unusual practice area, or for firm mission copy. Start with
Capricorn, then Scorpio, then Aries through Pisces:

- ♈ Aries, trial attorney: lead with the harm.
- ♉ Taurus, business attorney: make the language operative.
- ♊ Gemini, appellate attorney: find ambiguity and dual meanings.
- ♋ Cancer, legal-aid/tenant-defense attorney: read as the stressed applicant.
- ♌ Leo, immigration defense attorney: speak boldly for the right to remain.
- ♍ Virgo, tax attorney: demand exact cites, dates, forms, and triggers.
- ♎ Libra, mediator/family-law attorney: weigh protection against cost.
- ♐ Sagittarius, public-interest/civil-rights attorney: check the access-to-justice mission.
- ♒ Aquarius, legal-tech/knowledge-management attorney: find reusable drafting patterns.
- ♓ Pisces, estate-planning counselor/mental-health-court lens: honor the human story.

End with revised copy or a named go/no-go question.

## Client Council

Use the Client Council for intake, questionnaire ordering, portal UX, pricing, onboarding, errors, referrals, and other
decisions about whether a real person walks in and stays. Default to:

- ♎ Libra, prospective client at the threshold: does this feel worth it, trustworthy, and easier than going elsewhere?
- ♓ Pisces, overwhelmed person who almost did not reach out: is the door easy enough for someone with nothing left to
  give?

Use the full bench only on request, for a mission-level decision, or when the pair misses a practice-specific client.
Libra chairs; Pisces holds the access-to-justice door. The other voices are:

- ♈ Aries, tenant facing eviction: speed is survival.
- ♉ Taurus, first-time LLC founder: does the product feel solid enough to trust?
- ♊ Gemini, bilingual immigrant family: where does one-world wording fail two-world lives?
- ♋ Cancer, family caregiver: what asks too much of an exhausted household?
- ♌ Leo, wronged client who wants to sue: honor the dignity of a no-litigation referral.
- ♍ Virgo, meticulous compliance filer: eliminate vague deadlines, forms, and obligations.
- ♏ Scorpio, client with a matter they are ashamed of: guard privacy and avoid shame.
- ♐ Sagittarius, dreamer-builder: preserve momentum and horizon.
- ♑ Capricorn, elder planning a legacy: keep gravity and long-term meaning.
- ♒ Aquarius, collective organizer: fit nonstandard entities and communities.

End with the product or copy action, or the user's go/no-go on a real strategic fork.

## Shared guardrails

- Cite real files, routes, screens, symbols, or copy. English is the source language for portal UI, docs, internal
  artifacts, and legal template bodies, and Navigator publishes no translated surface — see [`AGENTS.md`](../AGENTS.md).
- Respect the role model in [`access-model.md`](access-model.md): every Person has one `persons.role`; project scope is
  separate in `person_project_roles.participation`.
- Telemetry and cloud operations log identifiers and counts, never client names, answers, email addresses, document
  bodies, privileged substance, or other client content. The same exclusion applies to public docs.
