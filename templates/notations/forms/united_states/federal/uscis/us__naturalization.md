---
kind: filing
title: Application for Naturalization — Form N-400 Intake Summary
respondent_type: person
code: us__naturalization
jurisdiction: US
origin_url: https://www.uscis.gov/n-400
confidential: true
output: form
form: us__naturalization
questionnaire:
  BEGIN:
    _: person__client
  person__client:
    _: custom_datetime__date_of_birth
  custom_datetime__date_of_birth:
    _: country__of_birth
  country__of_birth:
    _: country__of_citizenship
  country__of_citizenship:
    _: custom_datetime__lpr_since
  custom_datetime__lpr_since:
    _: custom_phone__daytime_phone
  custom_phone__daytime_phone:
    _: custom_single_choice__eligibility_basis
  custom_single_choice__eligibility_basis:
    _: custom_single_choice__marital_status
  custom_single_choice__marital_status:
    _: custom_text__time_outside_us
  custom_text__time_outside_us:
    _: custom_yes_no__good_moral_character
  custom_yes_no__good_moral_character:
    _: END
  END: {}
prompts:
  client_name: What is the client's full legal name?
  of_birth: In what country were you born?
  of_citizenship: Of what country are you currently a citizen or national?
custom_questions:
  date_of_birth:
    prompt: What is your date of birth?
  lpr_since:
    prompt: On what date did you become a lawful permanent resident?
  daytime_phone:
    prompt: What is the best daytime phone number to reach you?
  eligibility_basis:
    prompt: Which path to naturalization are you applying under?
    choices:
      five_year: Five years as a permanent resident
      three_year_marriage: Three years married to a U.S. citizen
      military: Qualifying U.S. military service
  marital_status:
    prompt: What is your current marital status?
    choices:
      single: Single, never married
      married: Married
      divorced: Divorced
      widowed: Widowed
  time_outside_us:
    prompt: About how many total days have you spent outside the United States in the last five years?
  good_moral_character:
    prompt: >-
      Is there anything in your history — arrests, citations, or unpaid taxes — your attorney should know before we
      file?
workflow:
  BEGIN:
    intake_submitted: intake_persisted__applicant
  intake_persisted__applicant:
    application_rendered: lawyer_review
  lawyer_review:
    approved: generate_pdf__n400_summary
    rejected: END
  generate_pdf__n400_summary:
    pdf_persisted: sent_for_signature__pending
  sent_for_signature__pending:
    signature_received: e_filing__uscis
    signature_declined: END
  e_filing__uscis:
    filed: mailroom_receive__biometrics_notice
  mailroom_receive__biometrics_notice:
    received: mailroom_receive__interview_notice
  mailroom_receive__interview_notice:
    received: mailroom_receive__oath_notice
  mailroom_receive__oath_notice:
    certificate_received: document_intake__certificate_of_naturalization
  document_intake__certificate_of_naturalization:
    certificate_filed: END
  END: {}
---

This naturalization engagement (the "Engagement") prepares and files Form N-400, Application for Naturalization, with
U.S. Citizenship and Immigration Services ("USCIS") on behalf of `{{person__client.name}}` (the "Applicant").

The Applicant was born on `{{custom_datetime__date_of_birth}}` in `{{country__of_birth.name}}`, is a citizen or
national of `{{country__of_citizenship.name}}`, and became a lawful permanent resident on
`{{custom_datetime__lpr_since}}`. The Applicant is `{{custom_single_choice__marital_status}}` and applies under the
`{{custom_single_choice__eligibility_basis}}` path to naturalization.

This summary records what the Applicant told the firm at intake so it can be reviewed before anything is filed. It is
not the application itself and is not legal advice. The firm prepares the full Form N-400 from these answers, and a
licensed Neon Law attorney reviews the completed application with the Applicant before it is signed. Nothing reaches
USCIS unreviewed, and the firm does not promise any particular outcome — USCIS alone decides the application.

After the Applicant signs, the firm files the Form N-400 with USCIS and stays with the Applicant through each step that
follows: the biometrics appointment, the interview and civics test, and the oath ceremony. The Engagement concludes when
USCIS issues the Applicant's Certificate of Naturalization (Form N-550) — the lifelong proof of U.S. citizenship.

Appointment notices and confirmations are sent to the Applicant at `{{person__client.email}}`, and the firm reaches
the Applicant by phone at `{{custom_phone__daytime_phone}}`. The Applicant reported roughly
`{{custom_text__time_outside_us}}` days outside the United States in the last five years; the attorney reviews the exact
travel dates against the continuous-residence requirement before filing.

The Applicant signs below to confirm these intake answers are true and complete to the best of the Applicant's
knowledge, and the firm countersigns to open the matter.

{{client.signature}}

{{client.date}}

{{firm.signature}}

{{firm.date}}
