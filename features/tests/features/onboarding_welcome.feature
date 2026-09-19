Feature: Welcome email on first identity creation

  A supported verified identity creates a Person on first login and
  receives the welcome once. The configured Owner email is the
  bootstrap carve-out so a fresh deployment is never locked out; it is
  created as `owner`, the highest tier, rather than the ordinary
  `client`. Promotion of an existing operator-seeded email is not a
  new Person and sends nothing.

  The synchronous send is a stopgap. The durable version drives the
  same email via the `onboarding__welcome` workflow spec
  (`workflows/specs/onboarding__welcome.yaml`), `email_send__welcome`
  step, executed on the Restate worker. The spec is bundled today;
  the worker handler for `email_send__*` is a follow-up.

  Background:
    Given a CapturingEmail backend wired into the app

  Scenario: The bootstrap Owner's first login fires a welcome
    Given the IdP issues sub="rauthy-nick-subject", email="nick@neonlaw.com", name="Nick"
    When the bootstrap Owner completes the OAuth login dance
    Then exactly 1 captured email exists
    And the captured email is addressed to "nick@neonlaw.com"
    And the captured email subject is "Welcome to Neon Law"
    And the captured email body mentions "Nick"

  Scenario: The bootstrap Owner's return login does not re-send the welcome
    Given the IdP issues sub="rauthy-nick-subject", email="nick@neonlaw.com", name="Nick"
    When the bootstrap Owner completes the OAuth login dance
    And the bootstrap Owner completes the OAuth login dance again
    Then exactly 1 captured email exists

  Scenario: Seeded email promotion does not trigger a welcome
    Given a seeded person with email "lawyer@neonlaw.com" and role "lawyer"
    And the IdP issues sub="rauthy-lawyer-subject", email="lawyer@neonlaw.com", name="Lawyer"
    When Lawyer completes the OAuth login dance
    Then no captured emails exist
