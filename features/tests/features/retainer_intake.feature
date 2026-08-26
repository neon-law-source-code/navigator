Feature: Retainer intake walk

  The admin retainer walker drives one question per request against
  the in-memory workflow runtime. Each answer journals a
  questionnaire transition; the final answer drives the post-intake
  workflow to its terminal state.

  Background:
    Given a fresh Neon Law Navigator app with the canonical templates seeded
    And a retainer notation for "Libra" <libra@example.com> at BEGIN

  Scenario: First GET renders the first question
    When the lawyer visits /lawyer/notations/:id/step
    Then the response status is 200
    And the page asks the "person__client" question
    And the page shows "step 1 of 7"

  Scenario: Answering the first question advances to person__lawyer_dri
    When the lawyer submits "Libra" to /lawyer/notations/:id/step
    Then the response status is 303
    And the response redirects back to /lawyer/notations/:id/step
    And the questionnaire runtime has recorded 1 transition
    And the last transition lands on "person__client"
    And an answer row exists with value "Libra"

  Scenario: Walking all seven questions drives the workflow through END
    When the lawyer submits the full questionnaire:
      | value                                 |
      | Libra                                 |
      | Firm Principal                        |
      | Estate plan                           |
      | 2026-09-01                            |
      | Draft and file the matter documents.  |
      | 450 per hour                          |
      | nevada                                |
    Then the final response status is 303
    And the questionnaire runtime has recorded 8 transitions
    And the last transition lands on "END"
    And a GET to /lawyer/notations/:id/step now redirects to /app/lawyer

  Scenario: Posting to an unknown notation is a 404
    When the lawyer submits "x" to /lawyer/notations/00000000-0000-0000-0000-000000000000/step
    Then the response status is 404
