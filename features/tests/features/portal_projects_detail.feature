Feature: /app/projects/:code — single matter detail, scoped to the caller

  The project detail page is the place clients spend their time. It
  reads from the same client-lens visibility rule that gates the
  portal-landing list, applied per-row: callers who can see the
  project as clients get `200`; callers who cannot get `404`, not
  `403`. Lower tiers don't get to learn that the matter exists.

  Owner and Admin still carry no privileged bypass on the matter
  surface itself (ENG-81): `store::access::matter_viewer` answers
  `None` for them exactly as it would for anyone else with no
  participation row. What they get instead of the `404` every other
  tier receives in that shape is a narrower, participation-only
  rendering — enough to see the matter and manage who is assigned to
  it, never its documents or other content. A firm admin who is also a
  client still sees their own client-side matters here as a client.

  Background:
    Given the Neon Law Navigator app is running

  Scenario: An admin who is not on the matter reaches a participation-only view
    Given a seeded person "nick@neonlaw.com" with role "admin"
    And a project "Atlas LLC" with no participants
    When "nick@neonlaw.com" opens the detail page for "Atlas LLC"
    Then the response status is 200
    And the response body contains "Atlas LLC"
    And the response body contains "Add person"

  Scenario: An owner who is not on the matter reaches a participation-only view
    Given a seeded person "owner@neonlaw.com" with role "owner"
    And a project "Zenith Capital" with no participants
    When "owner@neonlaw.com" opens the detail page for "Zenith Capital"
    Then the response status is 200
    And the response body contains "Zenith Capital"
    And the response body contains "Add person"

  Scenario: The participation-only view discloses no matter content
    Given a seeded person "nick@neonlaw.com" with role "admin"
    And a project "Vega Holdings" with no participants
    When "nick@neonlaw.com" opens the detail page for "Vega Holdings"
    Then the response status is 200
    And the response body does not contain "Upload documents"
    And the response body does not contain "Edit project"

  Scenario: A lawyer who is a client participant reads the detail page
    Given a seeded person "lawyer@neonlaw.com" with role "lawyer"
    And a project "Borealis Trust" with "lawyer@neonlaw.com" as a participant
    When "lawyer@neonlaw.com" opens the detail page for "Borealis Trust"
    Then the response status is 200
    And the response body contains "Borealis Trust"

  Scenario: A lawyer who isn't on the matter gets a 404
    Given a seeded person "lawyer@neonlaw.com" with role "lawyer"
    And a project "Cetus Holdings" with no participants
    When "lawyer@neonlaw.com" opens the detail page for "Cetus Holdings"
    Then the response status is 404

  Scenario: A client participant reads their own matter
    Given a seeded person "capricorn@example.com" with role "client"
    And a project "Capricorn Matter" with "capricorn@example.com" as a participant
    When "capricorn@example.com" opens the detail page for "Capricorn Matter"
    Then the response status is 200
    And the response body contains "Capricorn Matter"

  Scenario: A client cannot peek at someone else's matter (404, not 403)
    Given a seeded person "sagittarius@example.com" with role "client"
    And a project "Other Client's Matter" with no participants
    When "sagittarius@example.com" opens the detail page for "Other Client's Matter"
    Then the response status is 404
