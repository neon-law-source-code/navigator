---
kind: pleading
title: Answer to Counterclaim (Nevada)
jurisdiction: NV
respondent_type: person
code: answer_to_counterclaim__nevada
confidential: false
questionnaire:
  BEGIN:
    _: person__client
  person__client:
    _: custom_datetime__answer_date
  custom_datetime__answer_date:
    _: END
  END: {}
prompts:
  answer_date: On what date is this answer to the counterclaim filed?
workflow:
  BEGIN:
    _: lawyer_review
  lawyer_review:
    _: END
  END: {}
---

## Cover sheet

Sample from a notation template. The matter is simulated. This answer is filed for no one. It is not legal advice and it
is not a filing.

## Caption

Eighth Judicial District Court, Clark County, Nevada

Cruller v. Prine

Case No. A-26-874219-C. Dept. No. XVII.

{{person__client}}, an individual, Counter-defendant, v. Wendell Prine, an individual, Counterclaimant.

Date filed: {{custom_datetime__answer_date}}

# ANSWER TO COUNTERCLAIM

## ANSWER TO COUNTERCLAIM FOR BREACH OF CONTRACT

Counter-defendant {{person__client}} answers the Counterclaim for Breach of Contract that Wendell Prine filed on 10
August 2026.

## 1. General denial

Except as admitted below, Counter-defendant denies each allegation of the Counterclaim.

## 2. Specific admissions

Counter-defendant admits that on 1 April 2025 Counterclaimant offered a doughnut over the hedge, described only as
"neat"; that Counter-defendant took a partial bite that day and set the remainder aside; and that the remainder was
eaten on 14 April 2026. Those acts formed no contract. No consideration passed beyond the doughnut. Every other
allegation is denied.

## 3. Affirmative defenses

- First affirmative defense: failure to state a claim. A human soul is not property a court can convey. A bargain for
  one states no enforceable claim.

- Second affirmative defense: no meeting of the minds. Counter-defendant never agreed to convey anything beyond a
  doughnut, and was never told that a further exchange was proposed. A term neither disclosed nor discoverable before
  performance is not a term of the agreement.

- Third affirmative defense: fraudulent concealment. Calling the instrument "neat," and omitting the term
  Counterclaimant now asserts, turned silence into a misrepresentation. The contract is voidable, and it has been
  rescinded.

- Fourth affirmative defense: unconscionability. The term is procedurally and substantively unconscionable. A term is
  procedurally unconscionable where "a party lacks a meaningful opportunity to agree to the clause terms . . . because
  the clause and its effects are not readily ascertainable upon a review of the contract." Substantive unconscionability
  turns on one-sidedness. D.R. Horton, Inc. v. Green, 120 Nev. 549, 96 P.3d 1159 (2004). A term that cannot be read
  before the instrument is eaten is not ascertainable. A soul for one doughnut is one-sided.

- Fifth affirmative defense: no consideration. Counter-defendant received nothing beyond the doughnut in paragraph 2.
  A promise without consideration the other way binds no one.

## 4. Prayer for relief

Counter-defendant asks that the Counterclaim be dismissed with prejudice, that judgment be entered for
Counter-defendant, and for costs and such other relief as the Court deems just.

{{person__client}}, by counsel

Neon Law, 2400 Confection Way, Suite 400, Las Vegas, Nevada 89101, Attorneys for Counter-defendant {{person__client}}.
