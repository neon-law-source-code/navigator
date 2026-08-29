Feature: Client self-serve intake (the magic link)

  A client answers the client-facing questions on their notation
  themselves — the demand-side mirror of the admin walker. Lawyers can
  pre-fill answers on the client's behalf; the client sees them
  pre-filled and editable, and both authorships interleave on the one
  notation. The typed glossary registry exposes the client-facing
  retainer question to the client; a non-participant cannot reach the
  surface at all.

  Background:
    Given a retainer matter opened for "Libra" <libra@example.com>

  Scenario: The client confirms a lawyer-prefilled answer and finishes their part
    Given a lawyer entered the entity name as "Northstar Ventures LLC"
    And a lawyer entered the entity's principal office as "100 Innovation Way, Reno, NV 89501"
    And a lawyer pre-filled the client's name as "Lawyer-typed Libra"
    When the client opens their intake
    Then the intake asks the "person__client" question
    And the intake is pre-filled with "Lawyer-typed Libra"
    When the client answers "Libra Prime"
    And the client opens their intake
    Then the client's part of the intake is complete
    And the client's name answer on file is "Libra Prime" from the client

  Scenario: A non-participant cannot reach the intake
    When a stranger opens the client's intake
    Then the intake response status is 404
