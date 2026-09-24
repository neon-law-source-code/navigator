---
kind: agreement
title: Mutual Non-Disclosure Agreement
code: business__mutual_nda
jurisdiction: US
respondent_type: person_and_entity
confidential: false
origin_url: https://github.com/General-Legal/legal-templates/tree/main/templates/mutual-nda
prompts:
  company: "What is the company’s full legal name and entity type?"
  other_party: "What is the other party’s full legal name and entity type?"
  effective_date: "What date does this agreement take effect?"
  business_purpose: "What specific business purpose permits use of the information?"
  business_term: "How long may the parties disclose information under this agreement?"
  business_survival: >-
    How long do confidentiality duties continue after termination (trade secrets remain protected while legally
    qualifying)?
  law: "Which state’s law governs?"
  notices: "Who receives notices for each party? Include name, role, business email, and postal address."
  business_forum: "Which courts have jurisdiction, subject to mandatory law?"
audiences:
  company: lawyer
  other_party: lawyer
  effective_date: lawyer
  business_purpose: lawyer
  business_term: lawyer
  business_survival: lawyer
  law: lawyer
  business_forum: lawyer
  notices: lawyer
questionnaire:
  BEGIN:
    _: entity__company
  entity__company:
    _: entity__other_party
  entity__other_party:
    _: custom_datetime__effective_date
  custom_datetime__effective_date:
    _: custom_text__business_purpose
  custom_text__business_purpose:
    _: custom_text__business_term
  custom_text__business_term:
    _: custom_text__business_survival
  custom_text__business_survival:
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
Drafting note: Confirm the parties, governing law, commercial terms, and signing authority before use. Adapted
from General Legal’s CC0 template at revision 6d6805425eabd41bed86fc1e2ec51612760f716c. This is a review
draft, not an executed instrument.
-->

# Mutual Non-Disclosure Agreement

{{entity__company.name}} and {{entity__other_party.name}} enter this agreement on {{custom_datetime__effective_date}}.

## I. Purpose and information

Each party may disclose and receive information. Confidential information means nonpublic business, financial,
technical, or personal information disclosed for {{custom_text__business_purpose}}, in any form, that is marked
confidential or reasonably understood to be confidential. It includes copies, analyses, and the existence and terms of
the parties’ discussions.

Information is excluded if the recipient can show that it became public without a breach, was already lawfully known
without a duty of confidence, was received lawfully from another source without restriction, or was developed
independently without using the disclosed information. A public component does not make a confidential combination
public.

## II. Care and permitted use

The recipient may use confidential information only for the stated purpose. It must use reasonable care, and at least
the care it uses for its own similar information. It may share information only with representatives who need it for
that purpose and are bound by confidentiality duties at least as protective. The recipient remains responsible for their
compliance. Sharing with an affiliate requires the discloser’s prior written consent.

The recipient must not reverse engineer disclosed materials, seek intellectual property rights in them, or use them for
another person’s benefit. It must promptly report unauthorized access, use, or disclosure and reasonably help contain
the harm.

## III. Required disclosure and protected reports

A recipient compelled by law to disclose information must, where legally permitted, give prompt notice and reasonable
help seeking protection. It may disclose only what is legally required. Nothing prevents a lawful report to a regulator,
protected whistleblowing, or a disclosure that applicable law protects. No prior notice or consent is required for a
protected report.

## IV. Ownership and return

The discloser retains its information and intellectual property. This agreement grants only the limited right to
evaluate the stated purpose. It creates no obligation to transact or disclose further information. Information is
supplied as is, without a warranty of accuracy or fitness, to the extent law permits. Neither party may disclose
information in breach of a third party’s rights.

On request or termination, the recipient must promptly return or destroy the information and confirm completion. It may
retain a legally required archive and inaccessible routine backups until ordinary deletion, subject to this agreement
and no further business use.

## V. Term and remedies

The disclosure period is {{custom_text__business_term}}. Either party may end future disclosures by written notice.
Confidentiality duties continue for {{custom_text__business_survival}} after termination; trade secrets remain protected
while they qualify under applicable law. Accrued rights survive. A party may seek available equitable relief for a
threatened or actual breach, subject to the court’s requirements.

## VI. Administration

Governing law: {{jurisdiction__law}}. Courts: {{custom_text__business_forum}}. Notices:
{{#for n in people__notices}} {{n.name}} ({{n.title}}): {{n.email}}; {{n.street}}, {{n.city}}, {{n.state}}
{{n.zip}}, {{n.country}}. {{/for}}

This agreement is the entire agreement on its subject. Changes and waivers must be in writing signed by both parties.
Delay in enforcement is not a waiver. An unenforceable provision is severed only to the extent necessary. Neither party
may assign without consent, except to a successor in a merger or sale of substantially all relevant assets that assumes
these obligations. Electronic signatures and counterparts are effective.

## Signatures

| Party | Authorized signature | Name and title | Date |
| --- | --- | --- | --- |
| {{entity__company.name}} | ____________________ | ____________________ | ____________________ |
| {{entity__other_party.name}} | ____________________ | ____________________ | ____________________ |
