---
kind: filing
title: Nevada Profit Corporation Formation
respondent_type: person_and_entity
code: nv__profit_corp_formation
jurisdiction: NV
origin_url: https://www.nvsos.gov/businesses/commercial-recordings/forms-fees/all-business-forms
confidential: false
output: form
form: nv__profit_corp_formation
questionnaire:
  BEGIN:
    _: person__client
  person__client:
    _: entity__company
  entity__company:
    _: person__registered_agent
  person__registered_agent:
    _: people__directors
  people__directors:
    _: people__corporate_officers
  people__corporate_officers:
    _: custom_text__shares_authorized
  custom_text__shares_authorized:
    _: custom_text__par_value
  custom_text__par_value:
    _: END
  END: {}
prompts:
  client_name: What is the client's full legal name?
  entity_name: What is the legal name of your LLC?
  registered_agent: Who is the registered agent?
custom_questions:
  shares_authorized:
    prompt: How many shares is the corporation authorized to issue?
  par_value:
    prompt: What is the par value of each share, in dollars?
workflow:
  BEGIN:
    intake_submitted: intake_persisted__incorporator
  intake_persisted__incorporator:
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

This Nevada entity formation engagement (the "Engagement") incorporates `{{entity__company.name}}`, a Nevada profit
corporation, for `{{person__client.name}}` (the "Incorporator"). The Engagement covers the Articles of Incorporation,
the Initial List of Officers, and the State Business License application filed with the Nevada Secretary of State,
together with the corporation's registered agent of record, `{{person__registered_agent.name}}`.

Fees for this Engagement are set in the separate signed fee agreement between you and the Firm, which controls the fee;
Nevada Secretary of State filing fees and the State Business License fee are passed through at cost.

The corporation's board of directors:

`{{people__directors}}`

Its officers, as reported on the Initial List:

`{{people__corporate_officers}}`

The corporation is authorized to issue `{{custom_text__shares_authorized}}` shares at a par value of
\$`{{custom_text__par_value}}` per share. The
first director listed signs the Articles of Incorporation as the Incorporator.

Your answers above are placed onto the Secretary of State's own formation packet — the same official form the state
publishes — and a licensed Neon Law attorney reviews the **filled packet** before anything is signed or filed. Nothing
reaches a government office unreviewed. The Incorporator signs below and the firm countersigns; Neon Law then files the
packet with the Nevada Secretary of State and returns the stamped formation record. Confirmations go to
`{{person__client.email}}`.

{{client.signature}}

{{client.date}}

{{firm.signature}}

{{firm.date}}
