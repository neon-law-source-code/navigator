Feature: Nevada entity formation, end to end

  Navigator forms a Nevada LLC through the official Secretary of State packet.
  This is the whole arc of one formation, following Libra — a first-time founder — and
  one Neon Law attorney from the first intake to a stamped filing with the
  Nevada Secretary of State: the firm opens the matter, the founder answers
  the onboarding questionnaire, the attorney reviews the filled packet, the
  founder signs and the firm countersigns, and Neon Law files. The
  nv__llc_formation template binds the state's own formation packet
  (form: nv__llc_formation), so the answers land on the official
  Secretary-of-State form via its field map.

  The recurring obligation the formation creates — the annual report owed
  every following year — is visible in the second scenario: that workflow,
  too, ends at a filing with the Secretary of State.

  Background:
    Given a fresh Neon Law Navigator app with the canonical templates seeded
    And a client named "Libra" <libra@example.com>

  Scenario: From intake to a stamped Secretary-of-State filing
    When the firm opens the "nv__llc_formation" matter for the client
    And the founder answers the formation questionnaire:
      | value                  |
      | Libra                  |
      | Bright Star Ventures   |
      | Neon Law Registered Agent |
      | members                |
      | Libra; 1 Main St; Las Vegas; NV; 89101; USA |
      | 2026-07-01             |
    And the attorney approves and sends the document
    Then the formation reaches the signature wait
    And the persisted packet is the official SoS form carrying the founder's answers
    And the generated packet is filed as a document in the matter
    When the attorney files the Articles with the Nevada Secretary of State
    Then the formation workflow reaches END
    And a filing was recorded with the "Nevada Secretary of State"
    And the founder's six onboarding answers are on file

  Scenario: The recurring annual-report obligation also files with the state
    Then the "nv__annual_report" workflow ends at a Secretary-of-State filing
