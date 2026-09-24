---
kind: agreement
title: Data Processing Addendum (Global)
code: business__dpa_global
jurisdiction: US
respondent_type: person_and_entity
confidential: false
origin_url: https://github.com/General-Legal/legal-templates/tree/main/templates/dpa-global
prompts:
  company: "What is the company’s full legal name and entity type?"
  other_party: "What is the other party’s full legal name and entity type?"
  effective_date: "What date does this agreement take effect?"
  business_agreement: "Identify the underlying customer agreement."
  business_processing: >-
    State subject matter, duration, nature, purposes, individuals, data, sensitive data, frequency, retention,
    locations, and controller/processor roles.
  business_security: "Specify the implemented security controls and any incorporated security schedule."
  business_incident_deadline: "State the outer incident reporting deadline; notice must still be without undue delay."
  contacts: >-
    Identify the privacy and security contacts for each party, including role, business email, and postal address.
  business_subprocessors: "List each subprocessor, function, location, and change-notice channel."
  business_subprocessor_notice: "What advance notice period applies to subprocessor changes?"
  business_transfers: >-
    Identify and attach the executed EU SCC modules and completed annexes, UK instrument, Swiss adaptations, transfer
    assessment, and supplementary measures; state none only if no restricted transfer occurs.
  business_exit: "Specify return format, deletion deadline, backup expiration, and legally required retention."
audiences:
  company: lawyer
  other_party: lawyer
  effective_date: lawyer
  business_agreement: lawyer
  business_processing: lawyer
  business_security: lawyer
  business_incident_deadline: lawyer
  contacts: lawyer
  business_subprocessors: lawyer
  business_subprocessor_notice: lawyer
  business_transfers: lawyer
  business_exit: lawyer
questionnaire:
  BEGIN:
    _: entity__company
  entity__company:
    _: entity__other_party
  entity__other_party:
    _: custom_datetime__effective_date
  custom_datetime__effective_date:
    _: custom_text__business_agreement
  custom_text__business_agreement:
    _: custom_text__business_processing
  custom_text__business_processing:
    _: custom_text__business_security
  custom_text__business_security:
    _: custom_text__business_incident_deadline
  custom_text__business_incident_deadline:
    _: people__contacts
  people__contacts:
    _: custom_text__business_subprocessors
  custom_text__business_subprocessors:
    _: custom_text__business_subprocessor_notice
  custom_text__business_subprocessor_notice:
    _: custom_text__business_transfers
  custom_text__business_transfers:
    _: custom_text__business_exit
  custom_text__business_exit:
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
Drafting note: Lawyer must verify EU, UK, Swiss, and applicable U.S. coverage. Complete and attach the
transfer instruments before approving any restricted transfer; this draft intentionally does not paraphrase or
substitute for mandatory clauses. Adapted from General Legal’s CC0 template at revision
6d6805425eabd41bed86fc1e2ec51612760f716c. This is a review draft, not an executed instrument.
-->

# Data Processing Addendum (Global)

{{entity__company.name}} (Provider) and {{entity__other_party.name}} (Customer) enter this addendum on
{{custom_datetime__effective_date}} under {{custom_text__business_agreement}}.

## I. Processing instructions

Customer determines the purposes and means of processing as controller or business, or acts on documented instructions
from its controller. Provider acts as processor, service provider, or contractor as applicable. Provider will process
personal data only to deliver the agreed services and follow Customer’s documented lawful instructions, including this
addendum. It will promptly flag an instruction it reasonably believes unlawful and suspend that instruction pending
clarification. Legally required processing is permitted with advance notice unless law prohibits it.

The processing schedule is {{custom_text__business_processing}}. It must identify subject matter, duration, nature,
purposes, categories of individuals and data, sensitive-data restrictions, frequency, retention, locations, and each
party’s role. Customer is responsible for lawful collection, necessary notices, permissions, and instructions. Provider
must not collect additional data on Customer’s behalf beyond those instructions.

## II. Confidentiality and security

Provider limits access to authorized people bound by confidentiality and maintains the measures set out in
{{custom_text__business_security}}. Measures must be appropriate to the risks and cover access controls, encryption
where appropriate, recovery, testing, incident response, and secure disposal. Provider will not materially reduce agreed
protection during the term. Restricted or sensitive data may be processed only where expressly included in the
processing schedule with appropriate safeguards.

## III. Incidents and individual requests

Provider must notify Customer without undue delay after becoming aware of a personal-data breach and within
{{custom_text__business_incident_deadline}}, subject to any shorter legal deadline. Notices go to
{{#for n in people__contacts}} {{n.name}} ({{n.title}}): {{n.email}}; {{n.street}}, {{n.city}}, {{n.state}}
{{n.zip}}, {{n.country}}. {{/for}} Notices must include available information about the incident, affected data, likely
consequences, mitigation, and a contact for updates. Provider must mitigate, preserve relevant evidence, and cooperate.
An initial notice must not await a full investigation.

Provider will promptly forward individual requests and assist Customer in access, correction, deletion, portability,
opt-outs, and other applicable rights. It will not respond substantively unless instructed or required by law.
Assistance must enable Customer to meet legal deadlines. Provider will reasonably assist with security duties, impact
assessments, and regulator consultations, taking account of the processing and available information. Ordinary
compliance with this addendum is included in service fees; extraordinary assistance requires an agreed quote where law
permits.

## IV. Subprocessors

Approved subprocessors, services, locations, and notice channel are {{custom_text__business_subprocessors}}. Customer
gives general authorization for that list. Provider must give {{custom_text__business_subprocessor_notice}} advance
notice of additions or replacements, allowing a reasonable data-protection objection. The parties will seek a practical
alternative; if none is available, Customer may end the affected service before the new processing begins and recover
unused prepaid fees. Each subprocessor must accept equivalent data-protection obligations. Provider remains responsible
for its performance.

## V. Verification

Provider will supply information reasonably needed to demonstrate compliance, including relevant independent reports. If
those are insufficient, Customer or an independent auditor bound by confidentiality may conduct a proportionate
inspection on reasonable notice. Restrictions must not frustrate mandatory audit rights or regulator access. Provider
will promptly report an inability to comply and allow reasonable steps to stop and remedy unauthorized processing.

## VI. U.S. state privacy requirements

Provider certifies that it understands and will comply with the applicable contractual restrictions. It must not sell or
share personal data; retain, use, or disclose it outside the specified business purposes or the direct business
relationship; or combine it with data from other sources except as applicable law expressly permits. It must provide the
level of privacy protection required by applicable law and cooperate with Customer’s reasonable monitoring and
remediation. Data disclosed under this addendum is supplied only for the limited purposes in the processing schedule.

Provider may not train a general-purpose AI model on Customer personal data or make a legally significant decision about
an individual unless separately instructed in writing with a lawful basis and necessary safeguards. De-identified data
use requires written instructions, reasonable measures against reidentification, and any required public commitment and
downstream restrictions.

## VII. European processing and international transfers

For processing subject to the GDPR, UK GDPR, or Swiss data-protection law, Provider also undertakes the processor duties
required by those laws. Customer’s documented instructions include permitted transfer destinations only where a lawful
transfer mechanism is in place. Provider will assist with impact assessments and prior consultation and make compliance
information available as required by those laws.

The completed transfer package is {{custom_text__business_transfers}}. Before a restricted transfer starts, the parties
must execute or validly incorporate the applicable unmodified European Commission standard contractual clauses, select
the correct modules, and complete all annexes, party details, supervisory authority, governing law, and courts. UK
transfers require the applicable UK instrument; Swiss transfers require the corresponding adaptations. The package must
include a transfer assessment and any supplementary safeguards required for the actual destinations and access risks.
This summary does not itself supply or replace those instruments.

Provider must promptly report an inability to comply with the transfer package. The affected transfer must be suspended
until lawful safeguards are restored, and terminated with data returned or deleted where required. No commercial term
limits an individual’s rights or the authority of a competent regulator under the applicable instrument.

## VIII. Return, deletion, and priority

On service completion, Customer chooses return or deletion under {{custom_text__business_exit}}. Provider must delete
remaining copies unless law requires retention, document the legal basis, isolate retained data, and use it only for
that required purpose. It will confirm completion on request and ensure subprocessors follow the same obligations.
Protection continues until deletion.

This addendum controls conflicts about personal data. Mandatory transfer instruments control inconsistent contract
terms. Service-agreement liability provisions apply only to the extent compatible with mandatory data-protection rights
and obligations. Changes require writing. Electronic signatures and counterparts are effective.

## Signatures

Provider: ____________________  Name and title: ____________________  Date: ____________________

Customer: ____________________  Name and title: ____________________  Date: ____________________
