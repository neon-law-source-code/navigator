Feature: Workshop "Using the Neon Law Navigator to Rapidly Solve Legal Outcomes"

  Every Bloom-tagged claim the workshop README makes about Neon Law Navigator
  is grounded by an executable scenario in this file. If a scenario
  here breaks, the workshop's prose is stale — the AIDA + engineer
  council insisted on this contract so the page cannot drift away
  from the runtime that backs it.

  The running matter is the one the workshop README names:

    Project   — Cruller v. Prine (code: sample-litigation)
    Client    — the seeded client account
    Template  — onboarding__letter (the shared engagement letter)

  The attorney is the actor in every When step; Neon Law Navigator is the
  instrument. Scorpio's load-bearing trust claim — the retainer is not
  signed until the attorney advances the workflow — is asserted in
  the final scenario.

  Background:
    Given a fresh dev Navigator app with the sample-matter workshop seed

  Scenario: Remember — the four Neon Law Navigator nouns are real schema entities
    Then the schema defines a "project" table
    And the schema defines a "template" table
    And the schema defines a "notation" table
    And the schema defines a "person" table

  Scenario: Apply — the presenter opens the seeded running matter
    Then a project named "Cruller v. Prine" exists in the database
    And the project status is "open"

  Scenario: Apply — the attorney binds the retainer template as a notation
    When the attorney binds the retainer template as a notation
    Then a notation row exists linking the retainer template to the client
    And the retainer template body carries the "{{person__client.name}}" placeholder

  Scenario: Create — the retainer is not signed until the attorney advances the workflow
    # Scorpio's load-bearing trust claim from the engineer-council
    # review: Neon Law Navigator must never produce a signed retainer on its own.
    # Whatever the runtime calls the initial state, it must NOT be
    # `signed`, `notarized`, or `notarization__pending` — those only
    # appear after an explicit workflow advance the attorney drives.
    When the attorney binds the retainer template as a notation
    Then the notation state is not "signed"
    And the notation state is not "notarized"
    And the notation state is not "notarization__pending"
