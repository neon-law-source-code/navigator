---
kind: letter
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
prompts:
  offer_date: On what date was the doughnut offered?
  completion_date: On what date was the remainder of the doughnut consumed?
  discovery_date: On what date did the client learn of the soul-conveyance term?
  notice_date: What is the date of this notice?
workflow:
  BEGIN:
    _: lawyer_review
  lawyer_review:
    _: END
  END: {}
---

## Cover sheet

Sample from a notation template. The matter is simulated. This notice is addressed to no one. It is not legal advice.

## Caption

To: Wendell Prine

From: {{person__client}}

Date: {{custom_datetime__notice_date}}

Cruller v. Prine, Count II

# NOTICE OF RESCISSION

## 1. The instrument

On {{custom_datetime__offer_date}} you offered the undersigned one glazed doughnut over the hedge. You called it "neat."
You did not say, and the undersigned did not know, that it was said to carry a term conveying the undersigned's soul.

The undersigned took a partial bite that day and set the remainder aside. The remainder was eaten on
{{custom_datetime__completion_date}}.

## 2. Grounds for rescission

The purported agreement is voidable, and is rescinded, on each independent ground below.

- Fraudulent concealment. Assent runs only to terms the offeree could read. A term inside the instrument, reachable
  only by destroying it, is not such a term. Calling it "neat" made that silence a misrepresentation.
- No meeting of the minds. The undersigned never agreed to a conveyance, and was never told one was proposed.
- Unconscionability. The consideration was one doughnut.

## 3. No affirmance

Eating the remainder on {{custom_datetime__completion_date}} did not affirm the agreement. Waiver requires knowledge of
the facts constituting the fraud. The undersigned first learned of the term on {{custom_datetime__discovery_date}},
after the instrument was gone.

## 4. Timeliness

A claim for fraud accrues when the aggrieved party discovers the facts. This notice follows that discovery.

## 5. Demand

Within fourteen days, confirm in writing that you assert no interest in the soul of the undersigned. Restitution for the
doughnut is available on request.

{{person__client}}

By: ______________________________
