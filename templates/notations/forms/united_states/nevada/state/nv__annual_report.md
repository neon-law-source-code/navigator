---
kind: filing
title: Nevada Annual List of Managers, Members, and Registered Agent
respondent_type: entity
code: nv__annual_report
jurisdiction: NV
origin_url: https://www.nvsos.gov/businesses/commercial-recordings/forms-fees/all-business-forms
confidential: false
questionnaire:
  BEGIN:
    _: custom_single_choice__annual_or_amended
  custom_single_choice__annual_or_amended:
    _: people__managers
  people__managers:
    _: END
  END: {}
custom_questions:
  annual_or_amended:
    prompt: Is this an original annual application or is it an amendment to a previous application?
    choices:
      original: Original Application
      amended: Amendment to Previous Application
workflow:
  BEGIN:
    _: lawyer_review
  lawyer_review:
    _: mailroom_send
  mailroom_send:
    _: END
  END: {}
---

Annual List for `{{entity_name}}`, filed with the Nevada Secretary of State for the period ending
`{{custom_single_choice__annual_or_amended}}`. The current managers and members of the company are:
`{{people__managers}}`. The registered
agent remains the one of record unless updated by a separate filing.
