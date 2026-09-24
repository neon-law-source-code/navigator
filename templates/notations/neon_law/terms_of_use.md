---
kind: agreement
title: Terms of Use
code: business__terms_of_use
jurisdiction: US
respondent_type: person_and_entity
confidential: false
origin_url: https://github.com/General-Legal/legal-templates/tree/main/templates/terms-of-use
prompts:
  business_services: "Identify the website, app, and covered services."
  company: "What is the company’s full legal name and entity type?"
  effective_date: "What is the effective date?"
  contacts: "Who are the primary and alternate contacts? Include name, title, email, and postal address."
  business_acceptance: "Describe the actual notice and assent mechanism, including how changes are accepted."
  business_eligibility: "State the audience, age limits, and organizational authority requirements."
  business_permitted_use: "Describe permitted personal or business use."
  business_content_license: >-
    Define content processing, sharing settings, and the license needed to operate the service.
  business_privacy: "Give the privacy and cookie notice URLs."
  business_charges: >-
    State fees, renewals, cancellation, refunds, and support, or clearly state that the service is free.
  business_exit: "Give the account closure and data export procedure."
  business_liability: >-
    State a reasonable cap, measurement period, and exceptions appropriate to the service and applicable law.
  law: "Which state’s law governs?"
  business_local_notices: "Provide applicable state and consumer notices and current official complaint contacts."
  business_accessibility: "Provide an accessibility help channel and available accommodations."
  business_forum: "Which courts have jurisdiction, subject to mandatory law?"
audiences:
  business_services: lawyer
  company: lawyer
  effective_date: lawyer
  contacts: lawyer
  business_acceptance: lawyer
  business_eligibility: lawyer
  business_permitted_use: lawyer
  business_content_license: lawyer
  business_privacy: lawyer
  business_charges: lawyer
  business_exit: lawyer
  business_liability: lawyer
  law: lawyer
  business_forum: lawyer
  business_local_notices: lawyer
  business_accessibility: lawyer
questionnaire:
  BEGIN:
    _: custom_text__business_services
  custom_text__business_services:
    _: entity__company
  entity__company:
    _: custom_datetime__effective_date
  custom_datetime__effective_date:
    _: people__contacts
  people__contacts:
    _: custom_text__business_acceptance
  custom_text__business_acceptance:
    _: custom_text__business_eligibility
  custom_text__business_eligibility:
    _: custom_text__business_permitted_use
  custom_text__business_permitted_use:
    _: custom_text__business_content_license
  custom_text__business_content_license:
    _: custom_text__business_privacy
  custom_text__business_privacy:
    _: custom_text__business_charges
  custom_text__business_charges:
    _: custom_text__business_exit
  custom_text__business_exit:
    _: custom_text__business_liability
  custom_text__business_liability:
    _: jurisdiction__law
  jurisdiction__law:
    _: custom_text__business_forum
  custom_text__business_forum:
    _: custom_text__business_local_notices
  custom_text__business_local_notices:
    _: custom_text__business_accessibility
  custom_text__business_accessibility:
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
Drafting note: Confirm actual assent, consumer law, age restrictions, renewals, IP license, liability, local
notices, and accessibility. The source’s alternative arbitration and broad releases are omitted; add only
through separate legal review. Adapted from General Legal’s CC0 template at revision
6d6805425eabd41bed86fc1e2ec51612760f716c. This is a review draft, not an executed instrument.
-->

# Terms of Use

These terms govern {{custom_text__business_services}}, operated by {{entity__company.name}}. Effective date:
{{custom_datetime__effective_date}}. Company contact and notice details: {{#for c in people__contacts}} {{c.name}}
({{c.title}}): {{c.email}}; {{c.street}}, {{c.city}}, {{c.state}} {{c.zip}}, {{c.country}}. {{/for}}

## I. Agreement and eligibility

The service presents these terms for acceptance through {{custom_text__business_acceptance}}. Eligibility, age limits,
and any authority needed to act for an organization are {{custom_text__business_eligibility}}. If you do not accept, do
not use the features that require acceptance. Separate signed service or purchase agreements control their specific
subject matter.

## II. Accounts and permitted use

Give accurate registration information, keep credentials secure, and promptly report unauthorized access. You receive a
limited, nonexclusive, nontransferable right to use the service for {{custom_text__business_permitted_use}} while
complying with these terms. You may not break the law, interfere with security, access another person’s account,
introduce harmful code, or infringe others’ rights. Restrictions do not override rights applicable law or an express
open-source license gives you.

## III. Content and ownership

Company and its licensors retain the service and its intellectual property. You retain your submitted content. You grant
Company only the rights needed to host, process, and display that content to provide the service as described in
{{custom_text__business_content_license}}. You must hold the necessary rights. Do not submit another person’s
confidential material without authority. Voluntary feedback may be used to improve the service without payment; that
permission does not authorize use of your confidential information.

## IV. Privacy and third parties

Our privacy and cookie notices are {{custom_text__business_privacy}}. They explain actual data practices and choices;
accepting these terms does not replace a consent separately required by law. Third-party links and integrations are
subject to their own terms. Company is responsible for its own conduct and makes no endorsement merely by linking to a
third party.

## V. Charges and changes

Any fees, renewal, cancellation, refund, and support terms are {{custom_text__business_charges}}. Required purchase and
renewal disclosures are provided before payment. Company may improve or change the service with reasonable notice of
material adverse changes where practical and any notice law requires. Changes to these terms take effect through the
notice and assent process identified above. They do not retroactively remove accrued rights.

## VI. Suspension and termination

You may stop using the service and close your account through {{custom_text__business_exit}}. Company may suspend or end
access for a material breach, unlawful use, or a material security threat, with notice and a reasonable opportunity to
cure where appropriate. Export, retention, and deletion follow the privacy notice and any service agreement. Earned
payment rights and provisions that by nature survive remain effective.

## VII. Warranties and liability

Except for express promises and rights law cannot exclude, the service is provided as is and as available, without
implied warranties of merchantability, fitness, or noninfringement. Company does not promise uninterrupted or error-free
operation.

The agreed liability limit and exceptions are {{custom_text__business_liability}}. To the extent law permits and subject
to those exceptions, neither party is liable for indirect, special, or consequential damages arising from the service.
No term excludes liability or consumer rights that applicable law makes nonwaivable. These provisions apply only to the
extent enforceable for the user and transaction.

## VIII. Disputes and local rights

Governing law is {{jurisdiction__law}}. Competent courts are {{custom_text__business_forum}}, subject to mandatory
consumer protections and forum rights. The parties will first try to resolve a dispute by contacting the notice
addresses, without delaying emergency relief or mandatory filing deadlines. These terms do not require arbitration or
waive a jury, class, or representative action.

Required local disclosures and consumer complaint contacts are {{custom_text__business_local_notices}}. Accessibility
assistance is available through {{custom_text__business_accessibility}}. These terms do not represent certification to a
technical standard.

## IX. General

These terms and any expressly incorporated agreement state the complete agreement on their subject. A waiver requires
writing and applies only to the stated instance. Invalid provisions are severed only as necessary. Neither party may
assign obligations without consent, except Company to a successor that assumes them without reducing mandatory user
rights. Electronic communications may satisfy writing requirements where law permits. Export and sanctions rules apply
to use of the service.
