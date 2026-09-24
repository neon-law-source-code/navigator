---
name: legal-writing
description: >
  Fleet-wide discipline for legal writing that becomes work product — memos, letters, briefs, and responses — modeled on
  (not speaking for) the standards elite firms are known for: the decision on page one, every sentence proved against a
  saved source, and argument written for the judge who will actually read it. Trigger whenever drafting or revising a
  memo, client letter, brief, or written response the firm will stand behind. Kept identical, byte for byte, in every
  Project repository's `.agents/skills/`; this canonical copy lives in Navigator's own `.agents/skills/`. Not the
  [[legal-council]] copy-review bench (that shapes notation-bound template language before it hardens) and not
  [[litigation-filing-review]] (that is the attorney-of-record filing gate); this is the standard both of them assume
  the underlying prose already meets.
---

# Legal writing

Three standards, modeled — not spoken for. This skill does not claim to write as Wachtell, Cravath, or Quinn Emanuel; it
holds Navigator's own writing to the discipline those firms are known for.

## Page one carries the decision

**The Wachtell standard: the decision on page one.** A memo, letter, or brief opens with the recommendation, not the
build-up to it. In order: the recommendation, the odds, the one decisive fact, the deadline, and the ask. A reader who
stops after the first paragraph still knows what to do.

Recommend one course. "It depends" is refused unless the next clause says what it depends on and when the matter will
know — a fact still to be confirmed, a ruling still pending, a deadline not yet reached. "It depends, and we will know
once opposing counsel answers the interrogatory due the 14th" is page one. "It depends" alone is not writing, it is
declining to write.

## Every sentence is proved

**The Cravath standard: every sentence proved, every source saved.** A fact in the draft carries one of three labels,
visible to the supervising lawyer before the draft becomes work product:

- **Documented** — pinned to a source with a pinpoint cite (page, paragraph, or exhibit line).
- **Client-asserted** — attributed to the client's own account, not independently verified.
- **Unknown** — named as a gap, not silently assumed.

A cited authority is opened, quoted verbatim, and checked for treatment before it is relied on — never cited from a
snippet, a headnote, or memory. Odds are stated as bands tied to named fact scenarios ("likely if the signature is
authenticated, unlikely if it is not"), never as a bare point estimate: a point estimate hides which fact it depends on,
and the band is what tells the next reader what would change it.

**Nothing goes in a filing the lawyer does not believe true.** Candor comes before advocacy; an argument the lawyer does
not believe survives no further than the sentence it is cut from.

### Source preservation

Every cited source is saved in its original bytes, not just linked, so the supervising lawyer can verify it before
relying on it. A citation with no saved original is flagged as unverifiable and goes no further until it is one.

- **Law** (a case, statute, regulation, or rule) is saved as an Authority: `navigator site authorities create
  --class <class> --citation <citation> --title <title> --file <archived-copy>`, with `--checked-on` set to the date the
  source was actually opened and checked — not the date the memo was drafted. An Authority is global reference data,
  shared by every matter that cites it — never a bare git-committed file.
- **Matter-specific material** (a filed document, a produced page, an opposing brief, a piece of correspondence) is
  saved as a matter document, scoped to the matter it came from — never as a global Authority, which is shared reference
  data.
- There is currently no first-class field for the date and source a matter document was captured from; until one
  exists, record it in `--description`. A dedicated `accessed_at` field is a known gap, tracked separately — do not
  invent a workaround field or convention of your own for it here.

## Written for the actual hearing

**The Quinn Emanuel standard: tried on paper.** Write for this judge, this record, and this hearing — not for a
hypothetical reader. Before filing, the draft states:

- **The other side's best argument, at its strongest.** Steelman it, then answer it. An opponent's argument
  understated in the draft is an argument the judge hears for the first time from opposing counsel, at the worst moment
  to hear it.
- **The thirty-second story.** If the theory of the matter cannot be told in the time it takes to walk to the podium,
  it is not ready to argue.
- **The record that has to exist.** Name the declaration, exhibit, or transcript page the argument stands on before
  the hearing, not during it. An argument rests on the record actually filed, not the record the draft assumes.

## Research order

Primary law is never sourced from the open web. In order:

1. **Descrybe** for case law.
2. **Midpage** for statutes and rules.
3. **CourtListener** for dockets.

Every result still goes through source preservation above before it is relied on: found is not the same as saved and
checked.
