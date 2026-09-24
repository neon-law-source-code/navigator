---
kind: agreement
title: Master Services Agreement
code: business__master_services_agreement
jurisdiction: US
respondent_type: person_and_entity
confidential: false
origin_url: https://github.com/General-Legal/legal-templates/tree/main/templates/master-services-agreement
prompts:
  company: "What is the company’s full legal name and entity type?"
  other_party: "What is the other party’s full legal name and entity type?"
  effective_date: "What date does this agreement take effect?"
  business_order: "Describe the signed order, services, users, milestones, dependencies, and acceptance criteria."
  business_fees: "State fees, currency, billing schedule, payment deadline, usage charges, and approved expenses."
  business_support: "State support hours, availability commitments, service credits, and exclusions."
  business_deliverables: "Who owns commissioned deliverables, and what background-IP licenses apply?"
  business_liability: "Specify each liability cap, its measurement period, carve-outs, and any higher caps."
  business_term: "State term, renewal, cure period, and termination notice."
  business_exit: "State export format, transition assistance, retention period, and deletion requirements."
  law: "Which state’s law governs?"
  notices: "Who receives notices for each party? Include name, role, business email, and postal address."
  business_forum: "Which courts have jurisdiction, subject to mandatory law?"
audiences:
  company: lawyer
  other_party: lawyer
  effective_date: lawyer
  business_order: lawyer
  business_fees: lawyer
  business_support: lawyer
  business_deliverables: lawyer
  business_liability: lawyer
  business_term: lawyer
  business_exit: lawyer
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
    _: custom_text__business_order
  custom_text__business_order:
    _: custom_text__business_fees
  custom_text__business_fees:
    _: custom_text__business_support
  custom_text__business_support:
    _: custom_text__business_deliverables
  custom_text__business_deliverables:
    _: custom_text__business_liability
  custom_text__business_liability:
    _: custom_text__business_term
  custom_text__business_term:
    _: custom_text__business_exit
  custom_text__business_exit:
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
Drafting note: Complete every order and risk allocation. Confirm data processing, AI use, warranties,
indemnity, liability caps, service levels, tax, and exit terms. This version uses courts and does not impose
arbitration. Adapted from General Legal’s CC0 template at revision 6d6805425eabd41bed86fc1e2ec51612760f716c.
This is a review draft, not an executed instrument.
-->

# Master Services Agreement

{{entity__company.name}} (Provider) and {{entity__other_party.name}} (Customer) agree as of
{{custom_datetime__effective_date}}.

## I. Orders and services

Provider will supply the services in each signed order. The initial order is: {{custom_text__business_order}}. Each
order must identify deliverables, authorized users, dependencies, milestones, acceptance criteria, fees, and term. A
change requires written approval by both parties. An order overrides this agreement only where it expressly identifies
the provision changed. A data processing addendum controls conflicts concerning personal data.

## II. Access and cooperation

During the order term, Provider grants Customer and its authorized users a nonexclusive right to use the services for
Customer’s internal business under the order. Customer must provide necessary access, accurate instructions, and rights
to submitted data. Each party maintains the systems and security under its control. Customer is responsible for its
users and connected accounts. Third-party services require their own licenses and terms.

Customer must not unlawfully access systems, bypass agreed usage limits, introduce harmful code, or reverse engineer
services except where law permits. Provider may suspend access only to address a material security threat or uncured
material breach, using the narrowest practical suspension and prompt notice where lawful.

## III. Fees and support

Fees, currency, invoice timing, payment deadline, usage limits, expense approval, and taxes are:
{{custom_text__business_fees}}. Customer may dispute an invoice in good faith with specific reasons and must pay
undisputed amounts when due. Provider may suspend for undisputed nonpayment only after written notice and the cure
period below. Customer pays transaction taxes, excluding taxes on Provider’s income.

Support, availability commitments, and service-credit remedies are: {{custom_text__business_support}}. No other service
level is promised.

## IV. Ownership and data

Each party retains its pre-existing intellectual property. Ownership and licenses for commissioned deliverables are:
{{custom_text__business_deliverables}}. Customer retains its data. Provider may process it only to perform the services
and documented instructions, subject to the parties’ data processing addendum when required. Provider may not train a
general-purpose model on Customer data without separate written authorization. Customer decides whether outputs suit its
use and remains responsible for legally required human review.

Provider may use voluntary product suggestions without payment, but receives no license to Customer’s confidential
information. Any permitted use of de-identified data must be expressly described in the order and comply with applicable
law.

## V. Confidentiality

Each party protects the other’s nonpublic information with reasonable care, uses it only for this agreement, and shares
it only with people who need it and owe equivalent duties. The receiving party is responsible for their compliance.
Exceptions cover provably public, previously known, independently developed, or lawfully third-party-supplied
information. Required disclosure is permitted after notice and reasonable help seeking protection where lawful.
Confidentiality survives termination for three years; trade secrets remain protected while legally qualifying.

## VI. Warranties and remedies

Each party warrants authority to contract. Provider warrants professional performance and material conformity to the
order and documentation. Customer must identify a failure promptly; Provider will correct it within the agreed cure
period. If correction fails, Customer may terminate the affected service and receive prepaid fees for its unused
portion. Other than express warranties and rights law cannot exclude, services are supplied as is, without implied
warranties of merchantability, fitness, or noninfringement.

## VII. Third-party claims

Provider will defend Customer against a third-party claim that authorized use of the unmodified services infringes
intellectual property, and pay damages and settlements finally awarded or approved by Provider. This duty excludes
claims caused by Customer materials, unauthorized changes, or combinations not supplied or required by Provider.
Provider may secure rights, replace or modify the service without materially reducing function, or end the affected
service and refund unused prepaid fees if those options are not reasonable.

Customer will defend Provider against third-party claims that Customer-supplied materials infringe rights or violate
law, and pay damages and settlements finally awarded or approved by Customer. These duties require prompt notice,
reasonable cooperation at the defending party’s expense, and control of defense. No settlement may admit fault or impose
nonmonetary obligations on the protected party without consent.

## VIII. Liability

The agreed liability cap, measurement period, exclusions, and any higher caps are: {{custom_text__business_liability}}.
Subject to those express exceptions and rights law cannot exclude, neither party is liable for indirect, consequential,
special, or punitive damages or lost profits arising from this agreement. The parties must complete the liability terms
before signing.

## IX. Term and exit

The initial term, renewal procedure, breach cure period, and termination notice are: {{custom_text__business_term}}.
Either party may terminate an affected order for a material breach not cured within that period after notice. On
termination, Customer pays earned undisputed fees and stops use; Provider refunds unearned prepaid fees where
termination results from Provider’s uncured breach. Data export, retention, and deletion are:
{{custom_text__business_exit}}. Accrued payment rights, ownership, confidentiality, liability, and dispute provisions
survive.

## X. Administration

Governing law: {{jurisdiction__law}}. Courts: {{custom_text__business_forum}}. Notices:
{{#for n in people__notices}} {{n.name}} ({{n.title}}): {{n.email}}; {{n.street}}, {{n.city}}, {{n.state}}
{{n.zip}}, {{n.country}}. {{/for}} Neither party is liable for delay beyond reasonable control, except payment
obligations, if it promptly notifies and mitigates. Neither may assign without consent, except to a successor assuming
its obligations in a merger or sale. The signed documents are the entire agreement; amendments and waivers require
writing. Invalid provisions are severed to the minimum extent. Counterparts and electronic signatures are effective.

## Signatures

Provider: ____________________  Name and title: ____________________  Date: ____________________

Customer: ____________________  Name and title: ____________________  Date: ____________________
