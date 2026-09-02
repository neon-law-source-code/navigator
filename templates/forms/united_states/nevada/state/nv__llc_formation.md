---
kind: filing
title: Nevada LLC Formation
respondent_type: person_and_entity
code: nv__llc_formation
jurisdiction: NV
origin_url: https://www.nvsos.gov/businesses/commercial-recordings/forms-fees/all-business-forms
confidential: false
output: form
form: nv__llc_formation
questionnaire:
  BEGIN:
    _: person__client
  person__client:
    _: entity__company
  entity__company:
    _: person__registered_agent
  person__registered_agent:
    _: custom_single_choice__management_structure
  custom_single_choice__management_structure:
    _: people__managing_members
  people__managing_members:
    _: custom_datetime__formation_date
  custom_datetime__formation_date:
    _: END
  END: {}
prompts:
  client_name: What is the client's full legal name?
  entity_name: What is the legal name of your LLC?
  registered_agent: Who is the registered agent?
custom_questions:
  management_structure:
    prompt: How will the company be managed?
    choices:
      members: Managed by its members — the owners
      managers: Managed by appointed managers
  formation_date:
    prompt: When was the formation date?
workflow:
  BEGIN:
    intake_submitted: intake_persisted__organizer
  intake_persisted__organizer:
    articles_rendered: lawyer_review
  lawyer_review:
    approved: generate_pdf__articles_pdf
    rejected: END
  generate_pdf__articles_pdf:
    pdf_persisted: sent_for_signature__pending
  sent_for_signature__pending:
    signature_received: filing__nv_sos
    signature_declined: END
  filing__nv_sos:
    filed: END
  END: {}
---

This Nevada entity formation engagement (the "Engagement") forms `{{entity__company.name}}`, a Nevada
limited-liability company, for `{{person__client.name}}` (the "Organizer"). It covers the
Articles of Organization, the Initial List of Managers or Managing Members,
and the State Business License application filed with the Nevada Secretary of State, together with the company's
registered
agent of record, `{{person__registered_agent.name}}`.

The company will be `{{custom_single_choice__management_structure}}`-managed. Its managers or managing members are:

`{{people__managing_members}}`

The first person listed signs the Articles of Organization as the Organizer. The Organizer asked that the company be
organized effective `{{custom_datetime__formation_date}}`. Confirmations and the official records returned by the
Secretary of State are sent to the Organizer at `{{person__client.email}}`.

Your answers above are placed onto the Secretary of State's own formation packet — the same official form the state
publishes — and a licensed Neon Law attorney reviews the **filled packet** before anything is signed or filed. Nothing
reaches a government office unreviewed. The Organizer signs below and the firm countersigns; Neon Law then files the
packet with the Nevada Secretary of State and returns the stamped formation record.

{{client.signature}}

{{client.date}}

{{firm.signature}}

{{firm.date}}
