---
kind: filing
title: Nevada Business Trust Formation
respondent_type: person_and_entity
code: nv__business_trust_formation
jurisdiction: NV
origin_url: https://www.nvsos.gov/businesses/commercial-recordings/forms-fees/all-business-forms
confidential: false
output: form
form: nv__business_trust_formation
questionnaire:
  BEGIN:
    _: person__client
  person__client:
    _: entity__company
  entity__company:
    _: person__registered_agent
  person__registered_agent:
    _: people__trustees
  people__trustees:
    _: END
  END: {}
prompts:
  client_name: What is the client's full legal name?
  entity_name: What is the legal name of your LLC?
  registered_agent: Who is the registered agent?
workflow:
  BEGIN:
    intake_submitted: intake_persisted__trustee
  intake_persisted__trustee:
    certificate_rendered: lawyer_review
  lawyer_review:
    approved: generate_pdf__certificate_pdf
    rejected: END
  generate_pdf__certificate_pdf:
    pdf_persisted: sent_for_signature__pending
  sent_for_signature__pending:
    signature_received: filing__nv_sos
    signature_declined: END
  filing__nv_sos:
    filed: END
  END: {}
---

This Nevada entity formation engagement (the "Engagement") forms `{{entity__company.name}}`, a Nevada business
trust, for `{{person__client.name}}`. It covers the
Certificate of Business Trust, the Initial List of Trustees, and the State Business License application filed with the
Nevada Secretary of State, together with the trust's registered agent of record, `{{person__registered_agent.name}}`.

The trustees of the business trust:

`{{people__trustees}}`

The first trustee listed signs the Certificate of Business Trust, and the certificate prints up to two trustees.

Your answers above are placed onto the Secretary of State's own formation packet — the same official form the state
publishes — and a licensed Neon Law attorney reviews the **filled packet** before anything is signed or filed. Nothing
reaches a government office unreviewed. The first trustee signs below and the firm countersigns; Neon Law then files the
packet with the Nevada Secretary of State and returns the stamped formation record. Confirmations go to
`{{person__client.email}}`.

{{client.signature}}

{{client.date}}

{{firm.signature}}

{{firm.date}}
