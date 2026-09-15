---
kind: filing
title: Notice of Rescission (Nevada)
jurisdiction: NV
respondent_type: person
code: rescission_notice__nevada
confidential: false
questionnaire:
  BEGIN:
    _: person__client
  person__client:
    _: custom_datetime__offer_date
  custom_datetime__offer_date:
    _: custom_datetime__completion_date
  custom_datetime__completion_date:
    _: custom_datetime__discovery_date
  custom_datetime__discovery_date:
    _: custom_datetime__notice_date
  custom_datetime__notice_date:
    _: END
  END: {}
custom_questions:
  offer_date:
    prompt: On what date was the doughnut offered?
  completion_date:
    prompt: On what date was the remainder of the doughnut consumed?
  discovery_date:
    prompt: On what date did the client learn of the soul-conveyance term?
  notice_date:
    prompt: What is the date of this notice?
workflow:
  BEGIN:
    _: lawyer_review
  lawyer_review:
    _: END
  END: {}
---

# NOTICE OF RESCISSION

__FIXTURE DOCUMENT.__ This is a sample rendered from a notation template in the
Navigator sample project. _Cruller v. Prine_ is a simulated matter, this
notice is addressed to no one, and nothing in it is legal advice.

__To:__ Wendell Prine

__From:__ {{person__client}}

__Date:__ {{custom_datetime__notice_date}}

__Matter:__ Cruller v. Prine — Count II

## 1. The instrument

On {{custom_datetime__offer_date}} you offered the undersigned one glazed
doughnut over the hedge dividing the parties' properties. You described the
doughnut only as "neat." You did not state, and the undersigned did not know,
that the doughnut was said to carry a term conveying the undersigned's soul.

The undersigned took a partial bite on that date and set the remainder aside.
The remainder was consumed on {{custom_datetime__completion_date}}.

## 2. Grounds for rescission

The purported agreement is voidable, and is hereby rescinded, on each of the
following independent grounds.

* __Fraudulent concealment of a material term.__ Assent runs only to terms the
  offeree had a reasonable opportunity to read. A term placed inside the
  instrument, where it cannot be reached without destroying the thing being
  accepted, is not such a term. Describing the instrument as merely "neat"
  converted that silence into an affirmative misrepresentation.

* __No meeting of the minds.__ The undersigned never assented to the conveyance
  of anything, having never been told that a conveyance was proposed.

* __Unconscionability.__ The consideration received was one doughnut.

## 3. No affirmance

Consumption of the remainder on {{custom_datetime__completion_date}} was not an
affirmance of the purported agreement. Knowledge of the material facts
constituting the fraud is an essential requirement of waiver or election, and
the undersigned first learned of the term on
{{custom_datetime__discovery_date}} — after the instrument had been consumed in
full.

## 4. Timeliness

An action for relief on the ground of fraud accrues upon the discovery by the
aggrieved party of the facts constituting the fraud. This notice is served
promptly after that discovery.

## 5. Demand

Demand is made that you confirm in writing, within fourteen days, that you
assert no interest of any kind in the soul of the undersigned. Tender of
restitution for the doughnut is available on request.

{{person__client}}

By: ______________________________
