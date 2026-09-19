Feature: OIDC callback persons linking

  On every `/auth/callback`, the server decodes `sub` + `email` +
  `name` from the id_token and links a `persons` row to the IdP via
  the `oidc_subject` column. A supported verified identity with an
  email that the firm has not yet seen is JIT-created as a client.
  Three paths:

    1. No seeded row → create a `client` with no Project participation.
    2. Seeded email with `oidc_subject IS NULL` → PROMOTE (link the
       existing row, preserve the seeded role).
    3. Returning sub → no-op (idempotent).

  The id_token is RS256-signed with the shared test keypair and the
  app carries the matching verifier, so every scenario passes through
  the production signature + `iss`/`aud`/`nonce` checks. wiremock
  stands in for Rauthy.

  Background:
    Given a mock IdP returning an id_token

  Scenario: A first sign-in creates a client
    Given the IdP issues sub="rauthy-libra-subject", email="libra@example.com", name="Libra"
    When Libra completes the OAuth login dance
    Then the callback redirects with 303
    And the callback lands on "/app/projects"
    And exactly 1 persons row exists
    And the persons row has oidc_subject "rauthy-libra-subject"
    And the persons row keeps the "client" role

  Scenario: A seeded email is promoted on first login, preserving the lawyer role
    Given a seeded person with email "lawyer@neonlaw.com" and role "lawyer"
    And the IdP issues sub="rauthy-lawyer-subject", email="lawyer@neonlaw.com", name="Lawyer"
    When Lawyer completes the OAuth login dance
    Then the callback redirects with 303
    And the callback lands on "/app/team"
    And exactly 1 persons row exists
    And the persons row has oidc_subject "rauthy-lawyer-subject"
    And the persons row has email "lawyer@neonlaw.com"
    And the persons row keeps the "lawyer" role

  Scenario: A returning subject does not create a duplicate row and lands a client on their matters
    Given a seeded person with email "cancer@example.com" and role "client"
    And the IdP issues sub="rauthy-cancer-subject", email="cancer@example.com", name="Cancer"
    When Cancer completes the OAuth login dance
    And Cancer completes the OAuth login dance again
    Then exactly 1 persons row exists
    And the callback lands on "/app/projects"
