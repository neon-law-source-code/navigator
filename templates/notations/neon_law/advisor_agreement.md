---
kind: agreement
title: Advisor Agreement
code: business__advisor_agreement
jurisdiction: US
respondent_type: person_and_entity
confidential: false
origin_url: https://github.com/General-Legal/legal-templates/tree/main/templates/advisor-agreement
prompts:
  company: "What is the company’s full legal name and entity type?"
  advisor: "What is the advisor’s full legal name?"
  effective_date: "What date does this agreement take effect?"
  business_services: "Describe the advisory work, expected availability, and service dates."
  business_compensation: >-
    State cash fees, expense rules, and any proposed equity grant with vesting and required approvals.
  business_prior_materials: "List retained pre-existing materials and their license terms, or state none."
  law: "Which state’s law governs?"
  notices: "Who receives notices for each party? Include name, role, business email, and postal address."
  business_forum: "Which courts have jurisdiction, subject to mandatory law?"
audiences:
  company: lawyer
  advisor: lawyer
  effective_date: lawyer
  business_services: lawyer
  business_compensation: lawyer
  business_prior_materials: lawyer
  law: lawyer
  business_forum: lawyer
  notices: lawyer
questionnaire:
  BEGIN:
    _: entity__company
  entity__company:
    _: person__advisor
  person__advisor:
    _: custom_datetime__effective_date
  custom_datetime__effective_date:
    _: custom_text__business_services
  custom_text__business_services:
    _: custom_text__business_compensation
  custom_text__business_compensation:
    _: custom_text__business_prior_materials
  custom_text__business_prior_materials:
    _: jurisdiction__law
  jurisdiction__law:
    _: custom_text__business_forum
  custom_text__business_forum:
    _: people__notices
  people__notices:
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
Drafting note: Confirm securities approvals, vesting, tax treatment, conflicts, contractor classification, IP
exclusions, and protected-disclosure notice. No restrictive covenant or publicity consent is assumed. Adapted
from General Legal’s CC0 template at revision 6d6805425eabd41bed86fc1e2ec51612760f716c. This is a review
draft, not an executed instrument.
-->

# Advisor Agreement

{{entity__company.name}} (Company) engages {{person__advisor.name}} (Advisor) from {{custom_datetime__effective_date}}.

## I. Work

Advisor will provide these services and availability: {{custom_text__business_services}}. Advisor acts as an independent
contractor and has no authority to bind Company. Advisor will disclose conflicts and obtain necessary employer or
institutional permissions before beginning. No third party’s confidential information may be used in the work.

## II. Compensation

Company will provide the following cash compensation, approved expenses, or proposed equity:
{{custom_text__business_compensation}}. Any equity grant requires board approval and signed grant documents stating the
number and type of securities, exercise price, vesting schedule, service conditions, and treatment on termination. This
agreement alone does not issue securities or promise a particular tax result. No other compensation is due unless agreed
in writing. Advisor is responsible for taxes on compensation, subject to mandatory withholding.

## III. Work product

Advisor assigns to Company all rights in work product created specifically in performing the services, excluding the
pre-existing materials listed here: {{custom_text__business_prior_materials}}. Advisor will reasonably assist Company in
documenting those rights at Company’s expense. To the extent permitted by law, Advisor waives moral rights in assigned
work. Advisor grants Company a perpetual, worldwide, transferable, sublicensable, royalty-free license to use any
approved prior materials incorporated into the work as needed to use that work. No third-party material may be
incorporated without Company’s written approval and adequate rights.

## IV. Confidentiality

Advisor will use Company’s nonpublic information only to perform the services, protect it with reasonable care, and
disclose it only to approved people bound by equivalent duties. These duties exclude information Advisor can establish
was already lawfully known, became public without breach, was independently developed, or was lawfully received without
restriction. Legally compelled disclosure is permitted with prior notice where lawful and reasonable help seeking
protection.

Nothing restricts protected reports to government officials or a lawyer. Under 18 U.S.C. § 1833(b), an individual is
immune from federal and state trade-secret liability for a confidential disclosure to a government official or attorney
solely to report or investigate a suspected legal violation, or in a complaint or other document filed under seal. An
individual suing for retaliation for reporting a suspected violation may disclose the secret to counsel and use it in
court if documents containing it are filed under seal and disclosure otherwise occurs only under court order.

## V. Ending the relationship

Either party may end the engagement by written notice. Company pays earned compensation and approved expenses through
termination; equity is governed by the signed grant documents. Advisor must promptly return or delete Company property
and information, retaining only legally required records under continuing confidentiality. Ownership, confidentiality,
earned payment rights, and applicable grant terms survive. Company may use Advisor’s name or likeness publicly only with
Advisor’s written consent.

## VI. Administration

Governing law: {{jurisdiction__law}}. Courts: {{custom_text__business_forum}}. Notice details:
{{#for n in people__notices}} {{n.name}} ({{n.title}}): {{n.email}}; {{n.street}}, {{n.city}}, {{n.state}}
{{n.zip}}, {{n.country}}. {{/for}} This agreement and signed equity documents state the entire arrangement. Amendments
must be signed by both parties. Neither party may assign without the other’s consent, except Company to a successor
assuming its obligations. Invalid provisions are severed to the necessary extent. Counterparts and electronic signatures
are effective.

## Signatures

Company: ____________________  Name and title: ____________________  Date: ____________________

Advisor: ____________________  Date: ____________________
