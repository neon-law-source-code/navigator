---
kind: letter
output: contract
title: Employee Offer Letter (California Exempt)
code: business__employee_offer_letter
jurisdiction: CA
respondent_type: person_and_entity
confidential: false
origin_url: https://github.com/General-Legal/legal-templates/tree/main/templates/employee-offer-letter
prompts:
  company: "What is the company’s full legal name and entity type?"
  employee: "What is the employee’s full legal name?"
  accept_by: "What is the acceptance deadline?"
  business_role: "State title, actual duties, manager, California work location, and start date."
  business_classification: >-
    Identify the exemption and confirm current salary and duties requirements, including any occupation- specific rules.
  annual_salary: "What is the annual salary?"
  business_salary_basis: "What is the salary basis?"
  business_paydays: "On which days is the employee paid?"
  business_payroll_frequency: "What is the payroll frequency?"
  business_incentives: "Describe bonus and proposed equity terms, or state none."
  business_benefits: "State benefit eligibility, leave, and expense reimbursement."
  business_conditions: "List only lawful offer conditions and the sequence for any screening."
  contacts: "Who are the primary and alternate contacts? Include name, title, email, and postal address."
audiences:
  company: lawyer
  employee: lawyer
  accept_by: lawyer
  business_role: lawyer
  business_classification: lawyer
  annual_salary: lawyer
  business_salary_basis: lawyer
  business_paydays: lawyer
  business_payroll_frequency: lawyer
  business_incentives: lawyer
  business_benefits: lawyer
  business_conditions: lawyer
  contacts: lawyer
questionnaire:
  BEGIN:
    _: entity__company
  entity__company:
    _: person__employee
  person__employee:
    _: custom_datetime__accept_by
  custom_datetime__accept_by:
    _: custom_text__business_role
  custom_text__business_role:
    _: custom_text__business_classification
  custom_text__business_classification:
    _: custom_usd__annual_salary
  custom_usd__annual_salary:
    _: custom_text__business_salary_basis
  custom_text__business_salary_basis:
    _: custom_text__business_paydays
  custom_text__business_paydays:
    _: custom_text__business_payroll_frequency
  custom_text__business_payroll_frequency:
    _: custom_text__business_incentives
  custom_text__business_incentives:
    _: custom_text__business_benefits
  custom_text__business_benefits:
    _: custom_text__business_conditions
  custom_text__business_conditions:
    _: people__contacts
  people__contacts:
    _: END
  END: {}
workflow:
  BEGIN:
    _: lawyer_review
  lawyer_review:
    approved: END
    changes_requested: reask__draft
  reask__draft:
    resubmitted: lawyer_review
  END: {}
---

<!--
Drafting note: Confirm current California and local salary thresholds, duties tests, payroll rules, fair-
chance requirements, and required separate notices. Exempt status is not established by a title or this
letter. Arbitration from the source is omitted. Adapted from General Legal’s CC0 template at revision
6d6805425eabd41bed86fc1e2ec51612760f716c. This is a review draft, not an executed instrument.
-->

# Offer of Employment

{{entity__company.name}} offers {{person__employee.name}} the position described below. Please accept by
{{custom_datetime__accept_by}}.

## I. Role and start

Your position, duties, manager, California work location, and expected start date are: {{custom_text__business_role}}.
Your anticipated exempt classification and its duties and salary basis are: {{custom_text__business_classification}}.
Classification depends on applicable law and actual duties; this letter does not waive wages, overtime, breaks, or other
protections to which you are entitled.

## II. Pay and benefits

Your annual salary is {{custom_usd__annual_salary}}. Your salary basis is {{custom_text__business_salary_basis}}. Your
regular paydays are {{custom_text__business_paydays}}, and your payroll frequency is
{{custom_text__business_payroll_frequency}}. Pay is subject to required withholding and lawful deductions. Any bonus or
equity terms are: {{custom_text__business_incentives}}. An equity award requires the applicable approvals and separate
grant documents; this letter does not itself grant equity. Benefits, paid leave, and business-expense reimbursement are:
{{custom_text__business_benefits}}. Mandatory statutory benefits and reimbursement rights apply regardless of a plan’s
wording. Prospective changes require appropriate notice and compliance with law.

## III. Employment relationship

Employment is at will. You or Company may end it at any time, with or without notice or cause, subject to applicable
law. No statement changes that relationship unless set out in a written agreement signed by you and Company’s authorized
officer. This letter guarantees no fixed period of employment.

## IV. Conditions and responsibilities

The lawful conditions of this offer, including work authorization and any separately authorized screening, are:
{{custom_text__business_conditions}}. Company will follow applicable notice, consent, and fair-chance requirements. You
must follow lawful workplace policies. Do not bring or use another employer’s confidential information.

Any confidentiality and invention-assignment agreement must be separately provided and signed. It must preserve
applicable employee-invention exclusions, protected disclosures, wage discussions, and other nonwaivable rights. This
letter imposes no noncompete, mandatory arbitration, or class-action waiver.

## V. Complete offer

This letter and the documents expressly identified in it state the offer. California law governs, subject to controlling
federal law. No term limits rights that cannot lawfully be waived. Questions and notices should go to
{{#for c in people__contacts}} {{c.name}} ({{c.title}}): {{c.email}}; {{c.street}}, {{c.city}}, {{c.state}}
{{c.zip}}, {{c.country}}. {{/for}}

## Acceptance

Company representative: ____________________  Title: ____________________  Date: ____________________

I accept this offer on the terms stated above.

Employee: ____________________  Date: ____________________
