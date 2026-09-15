---
kind: filing
title: Summons (Nevada)
jurisdiction: NV
respondent_type: person
code: summons__nevada
confidential: false
questionnaire:
  BEGIN:
    _: person__client
  person__client:
    _: custom_datetime__issuance_date
  custom_datetime__issuance_date:
    _: END
  END: {}
custom_questions:
  issuance_date:
    prompt: On what date is this summons issued?
workflow:
  BEGIN:
    _: lawyer_review
  lawyer_review:
    _: END
  END: {}
---

# SUMMONS

__FIXTURE DOCUMENT.__ This is a sample rendered from a notation template in the Navigator sample project. _Cruller v.
Prine_ is a simulated matter, this summons is directed at no one, and nothing in it is legal advice or a genuine court
filing.

__Eighth Judicial District Court, Clark County, Nevada__

__Case No.:__ A-26-874219-C __Dept. No.:__ XVII

__{{person__client}}, an individual,__ Plaintiff, vs. __Wendell Prine, an individual; and DOES I through X,
inclusive,__ Defendants.

__Date issued:__ {{custom_datetime__issuance_date}}

## SUMMONS — CIVIL

### To the defendant named above

A civil complaint has been filed by the plaintiff against you in this Court. A copy of the complaint accompanies this
summons.

## 1. You must respond in writing

If you intend to defend this lawsuit, within twenty days after this summons is served on you, exclusive of the day of
service, you must do both of the following:

- File with the Clerk of this Court a written response to the complaint, in accordance with the rules of the Court,
  together with the appropriate filing fee.
- Serve a copy of your written response upon the plaintiff's attorney, whose name and address are stated at the foot
  of this summons.

If you were served outside the State of Nevada, the time within which you must file and serve your written response
is thirty days after service, rather than twenty.

## 2. What happens if you do not respond

Unless you respond, your default will be entered on the plaintiff's application, and this Court may then enter
judgment against you for the relief demanded in the complaint, which could result in a money judgment or an order
affecting your property, without any further notice to you.

## 3. Where to get help

If you cannot afford an attorney, you may be eligible for free legal services from a legal services program. A list
of these programs is available at the Clerk's office named above. Contact information for the State Bar of Nevada is
also available at the Clerk's office.

## 4. Issuance

Clerk of the Court

By: ______________________________, Deputy Clerk

Issued at the request of: Neon Law, 2400 Confection Way, Suite 400, Las Vegas, Nevada 89101, Attorneys for Plaintiff
{{person__client}}.
