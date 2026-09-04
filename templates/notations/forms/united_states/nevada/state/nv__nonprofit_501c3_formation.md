---
kind: filing
title: Nevada Nonprofit Articles of Incorporation (501(c)(3))
respondent_type: entity
code: nv__nonprofit_501c3_formation
jurisdiction: NV
origin_url: https://www.nvsos.gov/businesses/commercial-recordings/forms-fees/all-business-forms
confidential: false
questionnaire:
  BEGIN:
    _: custom_text__mission_statement
  custom_text__mission_statement:
    _: people__board_members
  people__board_members:
    _: person__registered_agent
  person__registered_agent:
    _: END
  END: {}
prompts:
  registered_agent: Who is the registered agent for?
custom_questions:
  mission_statement:
    prompt: What is the mission statement?
workflow:
  BEGIN:
    _: board_signatures
  board_signatures:
    _: lawyer_review
  lawyer_review:
    _: mailroom_send
  mailroom_send:
    _: END
  END: {}
---

Articles of Incorporation for `{{entity_name}}`, a Nevada nonprofit corporation organized exclusively for charitable,
educational, and scientific purposes within the meaning of Section 501(c)(3) of the Internal Revenue Code. Mission:
`{{custom_text__mission_statement}}`. The initial board of directors consists of `{{people__board_members}}`. The
corporation's registered agent in Nevada is `{{person__registered_agent.name}}`. On dissolution, remaining assets pass
to another 501(c)(3) organization or to the federal government for a public purpose.
