---
kind: agreement
title: Cookie Notice
code: business__cookie_notice
jurisdiction: US
respondent_type: person_and_entity
confidential: false
origin_url: https://github.com/General-Legal/legal-templates/tree/main/templates/cookie-notice
prompts:
  company: "What is the company’s full legal name and entity type?"
  business_services: "Which website and apps does this notice cover?"
  effective_date: "What is the effective date?"
  business_privacy_policy: "Give the published privacy policy URL."
  business_inventory: >-
    List every cookie or tracker with provider, purpose, necessary/optional category, lifetime, and data recipients;
    reconcile against an actual scan.
  business_choices: >-
    Give working preference-center instructions and URL, consent behavior, opt-out signals, and relevant browser
    controls.
  contacts: "Who are the primary and alternate contacts? Include name, title, email, and postal address."
audiences:
  company: lawyer
  business_services: lawyer
  effective_date: lawyer
  business_privacy_policy: lawyer
  business_inventory: lawyer
  business_choices: lawyer
  contacts: lawyer
questionnaire:
  BEGIN:
    _: entity__company
  entity__company:
    _: custom_text__business_services
  custom_text__business_services:
    _: custom_datetime__effective_date
  custom_datetime__effective_date:
    _: custom_text__business_privacy_policy
  custom_text__business_privacy_policy:
    _: custom_text__business_inventory
  custom_text__business_inventory:
    _: custom_text__business_choices
  custom_text__business_choices:
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
Drafting note: Scan the actual site, inventory third-party trackers and session replay, and verify consent,
reject, withdrawal, and preference-signal controls before publishing. No tracker vendor is assumed. Adapted
from General Legal’s CC0 template at revision 6d6805425eabd41bed86fc1e2ec51612760f716c. This is a review
draft, not an executed instrument.
-->

# Cookie Notice

{{entity__company.name}} uses cookies and similar technologies on {{custom_text__business_services}}. This notice takes
effect on {{custom_datetime__effective_date}} and supplements {{custom_text__business_privacy_policy}}.

## I. What these technologies do

Cookies are small files stored on a device. Pixels, local storage, and similar tools may also recognize a device or
record an interaction. Some support a requested service; others measure use or support advertising. Their actual
purposes and duration are listed below.

## II. Our inventory

{{custom_text__business_inventory}}

This inventory identifies each technology, provider, purpose, category, duration, and whether the provider receives
information for its own purposes. We use only the technologies and purposes described here. Necessary technologies
support functions such as authentication, security, and choices you request. Optional technologies follow the choices
and legal requirements applicable to your visit.

## III. Your choices

{{custom_text__business_choices}}

Where consent is required, optional technologies stay off until you consent. You can withdraw consent as easily as you
gave it; withdrawal affects future use. Rejecting optional technologies does not prevent access to functions that do not
depend on them. Browser controls can block or delete cookies, although some requested features may stop working.
Applicable opt-out preference signals are handled as described in our privacy policy. Deleting cookies may reset choices
stored in them.

## IV. Updates and contact

We update this notice when our practices change and identify the effective date above. Where required, we seek a new
choice before using technologies for a new purpose. Questions and privacy requests: {{#for c in people__contacts}}
{{c.name}} ({{c.title}}): {{c.email}}; {{c.street}}, {{c.city}}, {{c.state}} {{c.zip}}, {{c.country}}.
{{/for}}
