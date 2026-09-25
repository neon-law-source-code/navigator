---
kind: pleading
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
prompts:
  issuance_date: On what date is this summons issued?
workflow:
  BEGIN:
    _: lawyer_review
  lawyer_review:
    _: END
  END: {}
---

## Cover sheet

Sample from a notation template. The matter is simulated. This summons is directed at no one. It is not legal advice and
it is not a filing.

## Caption

Eighth Judicial District Court, Clark County, Nevada

Cruller v. Prine

Case No. A-26-874219-C. Dept. No. XVII.

{{person__client}}, an individual, Plaintiff, v. Wendell Prine, an individual, and DOES I through X, inclusive,
Defendants.

Date issued: {{custom_datetime__issuance_date}}

# SUMMONS

## SUMMONS — CIVIL

### To the defendant named above

The plaintiff has filed a civil complaint against you. A copy accompanies this summons.

## 1. You must respond in writing

To defend, do both of the following within twenty days after service, excluding the day of service:

- File a written response with the Clerk of this Court, under the Court's rules, with the filing fee.
- Serve a copy on the plaintiff's attorney, named at the foot of this summons.

Service outside Nevada extends that time to thirty days.

## 2. What happens if you do not respond

If you do not respond, the plaintiff may take your default. The Court may then enter judgment for the relief demanded in
the complaint, including a money judgment or an order affecting your property, without further notice to you.

## 3. Where to get help

If you cannot afford an attorney, you may qualify for free legal services. The Clerk's office named above has a list of
those programs and contact information for the State Bar of Nevada.

## 4. Issuance

Clerk of the Court

By: ______________________________, Deputy Clerk

Issued at the request of Neon Law, 2400 Confection Way, Suite 400, Las Vegas, Nevada 89101, Attorneys for Plaintiff
{{person__client}}.
