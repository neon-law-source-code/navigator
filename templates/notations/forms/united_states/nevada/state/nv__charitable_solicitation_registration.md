---
kind: filing
title: Nevada Charitable Solicitation Registration
respondent_type: entity
code: nv__charitable_solicitation_registration
jurisdiction: NV
origin_url: https://www.nvsos.gov/businesses/commercial-recordings/forms-fees/all-business-forms
confidential: false
questionnaire:
  BEGIN:
    _: custom_single_choice__annual_or_amended
  custom_single_choice__annual_or_amended:
    _: custom_text__fundraising_activities
  custom_text__fundraising_activities:
    _: END
  END: {}
custom_questions:
  annual_or_amended:
    prompt: Is this an original annual application or is it an amendment to a previous application?
    choices:
      original: Original Application
      amended: Amendment to Previous Application
  fundraising_activities:
    prompt: What are the fundraising activities?
workflow:
  BEGIN:
    _: lawyer_review
  lawyer_review:
    _: mailroom_send
  mailroom_send:
    _: END
  END: {}
---

Nevada Charitable Solicitation Registration Statement for `{{entity_name}}` filed with the Secretary of State for the
period ending `{{custom_single_choice__annual_or_amended}}`. The organization's fundraising activities during the
period are: `{{custom_text__fundraising_activities}}`. This registration is required of any nonprofit that solicits
contributions from Nevada residents, and is renewed annually.
