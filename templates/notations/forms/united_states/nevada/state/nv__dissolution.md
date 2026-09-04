---
kind: filing
title: Nevada LLC Articles of Dissolution
respondent_type: entity
code: nv__dissolution
jurisdiction: NV
origin_url: https://www.nvsos.gov/businesses/commercial-recordings/forms-fees/all-business-forms
confidential: false
questionnaire:
  BEGIN:
    _: custom_text__dissolution_reason
  custom_text__dissolution_reason:
    _: custom_yes_no__final_debts_settled
  custom_yes_no__final_debts_settled:
    _: END
  END: {}
custom_questions:
  dissolution_reason:
    prompt: What is the dissolution reason?
  final_debts_settled:
    prompt: Have all final debts been settled?
workflow:
  BEGIN:
    _: member_signatures
  member_signatures:
    _: lawyer_review
  lawyer_review:
    _: mailroom_send
  mailroom_send:
    _: END
  END: {}
---

Articles of Dissolution for `{{entity_name}}`, a Nevada limited liability company. The members have voted to dissolve
the company for the following reason: `{{custom_text__dissolution_reason}}`. All debts and obligations of the company
have been settled or otherwise resolved per `{{custom_yes_no__final_debts_settled}}`. The company directs the Nevada
Secretary of State to enter the dissolution in its records.
