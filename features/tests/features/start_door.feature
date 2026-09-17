Feature: Public service Start door

  A signed-in client can choose a mapped service, answer its questionnaire,
  and leave the matter at the lawyer-review gate.

  Scenario: A client starts a service and reaches lawyer review
    Given a client ready to start a mapped service
    When the client starts the "llc-file" service
    Then the start response redirects to the client intake
    When the client answers the client questions:
      | value                                 |
      | Libra                                 |
    When the completed intake is sent to lawyer review
    Then the notation state is "lawyer_review"
