---
kind: filing
title: Nevada Modified Business Tax Return
respondent_type: entity
code: nv__modified_business_tax
jurisdiction: NV
origin_url: https://tax.nv.gov/Forms/Modified_Business_Tax_Return_Forms
confidential: true
questionnaire:
  BEGIN:
    _: custom_datetime__tax_year
  custom_datetime__tax_year:
    _: custom_usd__gross_revenue
  custom_usd__gross_revenue:
    _: END
  END: {}
custom_questions:
  tax_year:
    prompt: What tax year does this return cover?
  gross_revenue:
    prompt: What is the gross revenue?
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

Nevada Modified Business Tax Return for `{{entity_name}}` covering tax year `{{custom_datetime__tax_year}}`.
Total Nevada gross revenue for the period is `{{custom_usd__gross_revenue}}`. The signing member certifies under
penalty of perjury that this return is true, correct, and complete to the best of their knowledge.
