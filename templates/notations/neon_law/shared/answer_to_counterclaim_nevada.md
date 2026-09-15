---
kind: filing
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
custom_questions:
  answer_date:
    prompt: On what date is this answer to the counterclaim filed?
workflow:
  BEGIN:
    _: lawyer_review
  lawyer_review:
    _: END
  END: {}
---

# ANSWER TO COUNTERCLAIM

__FIXTURE DOCUMENT.__ This is a sample rendered from a notation template in the Navigator sample project. _Cruller v.
Prine_ is a simulated matter, this answer is filed on behalf of no one, and nothing in it is legal advice or a genuine
court filing.

__Eighth Judicial District Court, Clark County, Nevada__

__Case No.:__ A-26-874219-C __Dept. No.:__ XVII

__{{person__client}}, an individual,__ Counter-defendant, vs. __Wendell Prine, an individual,__ Counterclaimant.

__Date filed:__ {{custom_datetime__answer_date}}

## ANSWER TO COUNTERCLAIM FOR BREACH OF CONTRACT

Counter-defendant {{person__client}} answers the Counterclaim for Breach of Contract filed by Counterclaimant Wendell
Prine on 10 August 2026 as follows.

## 1. General denial

Except as specifically admitted below, Counter-defendant denies each and every allegation of the Counterclaim.

## 2. Specific admissions

Counter-defendant admits that on 1 April 2025 Counterclaimant offered Counter-defendant a doughnut over the hedge
dividing the parties' properties, describing it only as "neat"; that Counter-defendant took a partial bite on that
date and set the remainder aside; and that the remainder was consumed on 14 April 2026. Counter-defendant denies that
those acts formed a contract of any kind, denies that any consideration passed beyond the doughnut itself, and denies
each remaining allegation of the Counterclaim.

## 3. Affirmative defenses

- __First Affirmative Defense — failure to state a claim.__ The Counterclaim fails to state a claim upon which relief
  can be granted. A human soul is not property capable of legal conveyance, and a purported bargain for one states no
  claim a court can enforce, whatever the parties are alleged to have intended by it.

- __Second Affirmative Defense — no meeting of the minds.__ Counter-defendant never assented to the conveyance of
  anything beyond a doughnut, having never been told that a further exchange was proposed. A term neither disclosed
  nor discoverable before performance cannot be the term of an agreement.

- __Third Affirmative Defense — fraudulent concealment.__ Describing the instrument as merely "neat," while omitting
  the term Counterclaimant now says it carried, converted silence about a material term into an affirmative
  misrepresentation. A contract induced that way is voidable at Counter-defendant's election, and it has been
  rescinded.

- __Fourth Affirmative Defense — unconscionability.__ The alleged term is both procedurally and substantively
  unconscionable under Nevada law. The Nevada Supreme Court has held that a term is procedurally unconscionable where
  "a party lacks a meaningful opportunity to agree to the clause terms . . . because the clause and its effects are not
  readily ascertainable upon a review of the contract," and that substantive unconscionability turns on the
  one-sidedness of the term itself. _D.R. Horton, Inc. v. Green_, 120 Nev. 549, 96 P.3d 1159 (2004). A term placed
  where it cannot be read before the instrument is consumed is not ascertainable on any review, and a soul traded for
  one doughnut is one-sided on its face.

- __Fifth Affirmative Defense — no consideration flowed to Counter-defendant.__ Counter-defendant received nothing
  beyond the doughnut already accounted for in paragraph 2. A promise unsupported by consideration running the other
  way binds no one.

## 4. Prayer for relief

Counter-defendant prays that the Counterclaim be dismissed with prejudice, that judgment be entered for
Counter-defendant, and for costs and such other relief as the Court deems just.

{{person__client}}, by and through counsel

Neon Law, 2400 Confection Way, Suite 400, Las Vegas, Nevada 89101, Attorneys for Counter-defendant
{{person__client}}.
