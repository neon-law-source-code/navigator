---
kind: offboarding
title: Closing Letter
respondent_type: person_and_entity
code: offboarding__letter
jurisdiction: NV
confidential: true
prompts:
  client_name: What is the client's full legal name?
  project_name: What is the project name for this engagement?
custom_questions:
  matter_summary:
    prompt: Summarize the matter and the work the firm completed.
  fee_status:
    prompt: What is the fee status as the matter closes?
    choices:
      paid_in_full: Paid in full
      balance_due: Balance due
      waived: Fees waived
  file_retention:
    prompt: How will the client's file be retained or returned?
  next_obligation:
    prompt: What is the client's next obligation or deadline, if any?
questionnaire:
  BEGIN:
    _: person__client
  person__client:
    _: project__engagement
  project__engagement:
    _: custom_text__matter_summary
  custom_text__matter_summary:
    _: custom_single_choice__fee_status
  custom_single_choice__fee_status:
    _: custom_text__file_retention
  custom_text__file_retention:
    _: custom_text__next_obligation
  custom_text__next_obligation:
    _: END
  END: {}
workflow:
  BEGIN:
    close_requested: lawyer_review
  lawyer_review:
    approved: generate_pdf__closing_letter
    rejected: END
  generate_pdf__closing_letter:
    pdf_persisted: firm_signature__closing_letter
  firm_signature__closing_letter:
    signed: END
  END: {}
---

This letter confirms that Neon Law (the "Firm") has completed its work for `{{person__client.name}}` (the "Client")
on the matter referred to as `{{project__engagement.name}}`, and that the Firm's representation of the Client on this
matter is now concluded.

Summary of the work completed: `{{custom_text__matter_summary}}`.

Fee status at closing: `{{custom_single_choice__fee_status}}`. The Client remains responsible only for fees and expenses
already incurred and invoiced on this matter; closing the matter itself adds no further charge.

The Client's file will be handled as follows: `{{custom_text__file_retention}}`. The Client may request a copy of the
file during the retention period at no additional cost.

Next steps that belong to the Client: `{{custom_text__next_obligation}}`. The Firm will take no further action on this
matter. Should a new need arise, the Client is welcome to open a new matter with the Firm at any time.

It has been our privilege to do this work alongside you. This letter is signed on behalf of the Firm by the Neon Law
lawyer of record for the matter.
