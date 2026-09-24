---
kind: agreement
title: Privacy Policy (GDPR Enhanced)
code: business__privacy_policy_gdpr
jurisdiction: US
respondent_type: person_and_entity
confidential: false
origin_url: https://github.com/General-Legal/legal-templates/tree/main/templates/privacy-policy-gdpr
prompts:
  company: "What is the company’s full legal name and entity type?"
  business_services: "Identify covered websites, apps, audiences, and controller/business role."
  effective_date: "What is the effective date?"
  contacts: "Who are the primary and alternate contacts? Include name, title, email, and postal address."
  business_data: "Inventory data categories, sensitive data, people, and specific sources."
  business_purposes: "Map each category to its actual purpose and disclose any automated decision-making."
  business_sharing: >-
    Map data categories to recipients and purposes; state actual sale, sharing, advertising, and opt-out practices.
  business_cookies: "Give the cookie notice and preference-center URLs."
  business_retention: "State retention periods or criteria by category, security practices, and backup treatment."
  business_rights: >-
    Give working request methods, verification, authorized-agent, appeal, response, and marketing opt-out procedures.
  business_state_disclosures: >-
    Complete applicable state disclosures, including California collection/sale/sharing information for the required
    period and financial incentives, if any.
  business_bases: >-
    Map each processing purpose to its lawful basis, identifying legitimate interests and consent withdrawal methods.
  business_transfers: >-
    Identify actual destinations, adequacy decisions or executed safeguards, and how people can obtain a copy.
  business_representatives: >-
    Give applicable DPO, EU/UK representative, and competent supervisory-authority details; explain any role that does
    not apply.
  business_children: "State intended ages, child-data practices, and parent contact process."
audiences:
  company: lawyer
  business_services: lawyer
  effective_date: lawyer
  contacts: lawyer
  business_data: lawyer
  business_purposes: lawyer
  business_sharing: lawyer
  business_cookies: lawyer
  business_retention: lawyer
  business_rights: lawyer
  business_state_disclosures: lawyer
  business_bases: lawyer
  business_transfers: lawyer
  business_representatives: lawyer
  business_children: lawyer
questionnaire:
  BEGIN:
    _: entity__company
  entity__company:
    _: custom_text__business_services
  custom_text__business_services:
    _: custom_datetime__effective_date
  custom_datetime__effective_date:
    _: people__contacts
  people__contacts:
    _: custom_text__business_data
  custom_text__business_data:
    _: custom_text__business_purposes
  custom_text__business_purposes:
    _: custom_text__business_sharing
  custom_text__business_sharing:
    _: custom_text__business_cookies
  custom_text__business_cookies:
    _: custom_text__business_retention
  custom_text__business_retention:
    _: custom_text__business_rights
  custom_text__business_rights:
    _: custom_text__business_state_disclosures
  custom_text__business_state_disclosures:
    _: custom_text__business_bases
  custom_text__business_bases:
    _: custom_text__business_transfers
  custom_text__business_transfers:
    _: custom_text__business_representatives
  custom_text__business_representatives:
    _: custom_text__business_children
  custom_text__business_children:
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
Drafting note: Verify territorial scope, lawful bases, Article 13/14 disclosures, representatives, automated
decisions, transfer safeguards, and U.S. state disclosures against actual practices. Adapted from General
Legal’s CC0 template at revision 6d6805425eabd41bed86fc1e2ec51612760f716c. This is a review draft, not an
executed instrument.
-->

# Privacy Policy (GDPR Enhanced)

{{entity__company.name}} is responsible for the practices described here for {{custom_text__business_services}}.
Effective date: {{custom_datetime__effective_date}}. Privacy contact and postal address:
{{#for c in people__contacts}} {{c.name}} ({{c.title}}): {{c.email}}; {{c.street}}, {{c.city}}, {{c.state}}
{{c.zip}}, {{c.country}}. {{/for}}

## I. Scope

This policy covers the services and audiences identified above. Where we process information solely for a business
customer under its instructions, that customer’s privacy notice governs its purposes; requests should be directed to
that customer. Our separate contractual duties still apply.

## II. Information and sources

{{custom_text__business_data}}

This schedule identifies each category of personal information, the people it concerns, whether it is sensitive, and its
source: directly from you, your device, a customer, or another identified source. We request only information needed for
the purposes below. Required fields and the consequences of not providing them are explained when collected.

## III. Purposes

{{custom_text__business_purposes}}

We use information only for the described purposes and compatible or otherwise lawful uses. These may include providing
requested services, securing accounts, responding to questions, meeting legal duties, and the specific analytics or
marketing activities stated above. We obtain consent where required. A new use requiring notice or consent is addressed
before it begins.

## IV. Sharing, advertising, and tracking

{{custom_text__business_sharing}}

This schedule identifies the categories of recipients and information disclosed, their purposes, any sale or sharing for
targeted advertising, and applicable opt-out links. Service providers act under appropriate contractual restrictions. We
may disclose information when legally required, to protect lawful rights, or in a business transfer subject to
applicable protections. We do not describe information as anonymous if it can reasonably identify you.

Cookies and other device technologies, their purposes, and choices are described at {{custom_text__business_cookies}}.
Where law requires an opt-out preference signal to be honored, we apply it to the browser or account as required and
explain any limits in the choice interface.

## V. Retention and security

{{custom_text__business_retention}}

Retention periods or criteria are specified by category and purpose, including legal holds and backup deletion. We
retain information only as long as needed for those purposes or as law requires. We use reasonable safeguards suited to
the information and risks. No system guarantees complete security.

## VI. Your rights and choices

{{custom_text__business_rights}}

Depending on applicable law, you may request access, correction, deletion, a portable copy, or an opt-out of sale,
sharing, targeted advertising, or certain profiling. Where applicable, you may limit use of sensitive information and
use an authorized agent. We verify requests proportionately and request only information needed for verification. We
explain a denial and provide any required appeal procedure and regulator contact. We do not unlawfully discriminate for
exercising rights. You may opt out of marketing messages using their unsubscribe control; necessary service messages may
continue.

## VII. U.S. state disclosures

{{custom_text__business_state_disclosures}}

The state schedule must describe applicable rights and request methods, the relevant collection and disclosure periods,
categories sold or shared, sensitive-information uses, financial incentives if any, and the company’s actual practices.
Mandatory rights apply even where this notice does not enumerate them.

## VIII. European, UK, and Swiss information

Our lawful basis for each purpose, including any specific legitimate interest, is {{custom_text__business_bases}}. Where
we rely on consent, you may withdraw it at any time without affecting prior lawful processing. Where applicable, you may
object to legitimate-interest processing and direct marketing, ask us to restrict processing, and request portability.
Any solely automated decisions with legal or similarly significant effects, their logic and consequences, and available
safeguards are described in the purposes schedule; do not infer consent from use of the service.

International destinations, transfer mechanisms, and how to obtain safeguard information are
{{custom_text__business_transfers}}. Our applicable data-protection officer, EU or UK representative, and relevant
supervisory-authority contact details are {{custom_text__business_representatives}}. You may complain to the competent
supervisory authority, including where you live, work, or believe an infringement occurred. Our request process does not
limit that right.

## IX. Children, links, and changes

Our intended ages, treatment of children’s information, and parent request process are:
{{custom_text__business_children}}. We obtain legally required parental consent or avoid collecting information that
requires it. If we discover prohibited collection, we take appropriate deletion and protective steps.

Third-party sites have their own policies. We update this policy when practices change, post the effective date, and
give additional notice or seek consent when law requires. Contact us using the details at the beginning of this policy.
