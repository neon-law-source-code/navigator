Feature: Profit corporation and business trust formations on the official packets

  Navigator forms a Nevada entity — and "entity" is
  bigger than the LLC. These journeys follow the same founder through the
  other two formation packets the Secretary of State publishes: a profit
  corporation (NRS 78) and a business trust (NRS 88A). Each template binds
  the state's own AcroForm packet, so the founder's answers land on the
  official form via its re-authored field layer, a Neon Law attorney reviews the filled
  packet, and the matter ends at a recorded Secretary-of-State filing.

  Background:
    Given a fresh Neon Law Navigator app with the canonical templates seeded
    And a client named "Libra" <libra@example.com>

  Scenario: A profit corporation forms on the official SoS packet
    When the firm opens the "nv__profit_corp_formation" matter for the client
    And the founder answers the formation questionnaire:
      | value |
      | Libra |
      | Bright Star Inc |
      | Neon Law Registered Agent |
      | Libra; 1 Main St; Las Vegas; NV; 89101; USA |
      | Libra; President; 1 Main St; Las Vegas; NV; 89101; USA |
      | 1000 |
      | 0.01 |
    And the attorney approves and sends the document
    Then the formation reaches the signature wait
    And the persisted corporation packet carries the founder's answers
    When the attorney files the formation packet with the Nevada Secretary of State
    Then the formation workflow reaches END
    And a filing was recorded with the "Nevada Secretary of State"

  Scenario: A business trust forms on the official SoS packet
    When the firm opens the "nv__business_trust_formation" matter for the client
    And the founder answers the formation questionnaire:
      | value |
      | Libra |
      | Bright Star Holdings |
      | Neon Law Registered Agent |
      | Libra; Trustee; 1 Main St; Las Vegas; NV; 89101; USA |
    And the attorney approves and sends the document
    Then the formation reaches the signature wait
    And the persisted business-trust packet carries the founder's answers
    When the attorney files the formation packet with the Nevada Secretary of State
    Then the formation workflow reaches END
    And a filing was recorded with the "Nevada Secretary of State"
