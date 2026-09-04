---
kind: filing
title: IRS Form 990 — Return of Organization Exempt From Income Tax
respondent_type: entity
code: us__form_990
jurisdiction: US
origin_url: https://www.irs.gov/forms-pubs/about-form-990
confidential: false
questionnaire:
  BEGIN:
    _: custom_datetime__tax_year
  custom_datetime__tax_year:
    _: custom_text__revenue_strategy
  custom_text__revenue_strategy:
    _: END
  END: {}
custom_questions:
  tax_year:
    prompt: What tax year does this return cover?
  revenue_strategy:
    prompt: What is the revenue strategy?
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

IRS Form 990 for `{{entity_name}}` covering tax year `{{custom_datetime__tax_year}}`. Summary of gross revenue,
program-service expense, and end-of-year net assets: `{{custom_text__revenue_strategy}}`. The officer signing this
return certifies under penalty of perjury that the return is true, correct, and complete to the best of their
knowledge. Filed with the Internal Revenue Service no later than the 15th day of the 5th month after the close of
the tax year.
