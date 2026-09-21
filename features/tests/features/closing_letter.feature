Feature: Closing letter walk

  A matter ends the way it began — with a signed letter. The retainer
  opens on the client's signature; the closing letter closes on the
  firm's. The admin walker drives one question per request against the
  in-memory workflow runtime, and the final answer drives the
  questionnaire to END — ready for lawyer review, the rendered letter,
  and the firm's signature.

  Background:
    Given a fresh Neon Law Navigator app with the canonical templates seeded
    And a closing notation for "Libra" <libra@example.com> at BEGIN

  Scenario: First GET renders the first question
    When the lawyer visits /app/lawyer/notations/:id/step
    Then the response status is 200
    And the page asks the "entity" question
    And the page shows "Closing Letter"
    And the page shows "Step 1 of 3"

  Scenario: Walking all three questions drives the questionnaire through END
    When the lawyer submits the full questionnaire:
      | value        |
      | Libra Trust  |
      | Libra        |
      | Estate plan  |
    Then the final response status is 303
    And the questionnaire runtime has recorded 4 transitions
    And the last transition lands on "END"
