Feature: Public service Start door

  A signed-in client can choose a mapped service, answer every question its
  questionnaire asks, and leave the matter at the lawyer-review gate.

  Scenario: A client starts a service and answers its whole questionnaire
    Given a client ready to start a mapped service
    When the client starts the "llc-file" service
    Then the start response redirects to the client intake
    And the intake asks question 1 of 6
    When the client answers every question the intake asks:
      | question                                   | answer                 |
      | person__client                             | Libra Client           |
      | entity__company                            | Northstar Ventures LLC |
      | person__registered_agent                   | Aries Agent Services   |
      | custom_single_choice__management_structure | members                |
      | people__managing_members                   | Libra Client           |
      | custom_datetime__formation_date            | 2026-01-15             |
    Then the client's part of the intake is complete
    When the firm advances the completed intake to lawyer review
    Then the notation state is "lawyer_review"
