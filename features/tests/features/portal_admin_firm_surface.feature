Feature: /app — one authenticated namespace, Owner/Admin/Lawyer only on the workbench

  Firm-administration listings answer at `/app/admin/*`. The lawyer workbench
  and remaining legal-work walks answer at `/app/lawyer/*`. Authorization is
  the lawyer `lawyer_tier` only, via embedded Rego policy. Client and Clerk
  tiers do not reach those surfaces.

  People is Owner/Admin only at `/app/admin/people`.

  Background:
    Given the Neon Law Navigator app is running

  Scenario: An Owner reads the firm-wide people index
    Given a seeded person "owner@neonlaw.com" with role "owner"
    When "owner@neonlaw.com" opens /app/admin/people
    Then the response status is 200

  Scenario: An admin reads the firm-wide people index
    Given a seeded person "nick@neonlaw.com" with role "admin"
    When "nick@neonlaw.com" opens /app/admin/people
    Then the response status is 200

  Scenario: An admin impersonates a client from the people index
    Given a seeded person "nick@neonlaw.com" with role "admin"
    And a seeded person "libra@example.com" with role "client"
    When "nick@neonlaw.com" POSTs to impersonate "libra@example.com"
    Then the response status is 303
    And the browser session role is "client"
    When the browser opens /app/forms with its current session
    Then the response body contains "Impersonating libra@example.com"
    And the response body contains "/app/impersonation/stop"

  Scenario: A lawyer cannot impersonate a client
    Given a seeded person "lawyer@neonlaw.com" with role "lawyer"
    And a seeded person "libra@example.com" with role "client"
    When "lawyer@neonlaw.com" POSTs to impersonate "libra@example.com"
    Then the response status is 403

  Scenario: An admin cannot impersonate lawyer
    Given a seeded person "nick@neonlaw.com" with role "admin"
    And a seeded person "lawyer@neonlaw.com" with role "lawyer"
    When "nick@neonlaw.com" POSTs to impersonate "lawyer@neonlaw.com"
    Then the response status is 409
    And the response body contains "Only client users can be impersonated."

  Scenario: An admin cannot impersonate another admin
    Given a seeded person "nick@neonlaw.com" with role "admin"
    And a seeded person "other-admin@neonlaw.com" with role "admin"
    When "nick@neonlaw.com" POSTs to impersonate "other-admin@neonlaw.com"
    Then the response status is 409
    And the response body contains "Only client users can be impersonated."

  Scenario: A lawyer reads the firm-wide entities index
    Given a seeded person "lawyer@neonlaw.com" with role "lawyer"
    When "lawyer@neonlaw.com" opens /app/admin/entities
    Then the response status is 200

  Scenario: A lawyer reads the firm dashboard at /app/lawyer
    Given a seeded person "lawyer@neonlaw.com" with role "lawyer"
    When "lawyer@neonlaw.com" opens /app/lawyer
    Then the response status is 200
    And the response body contains "Lawyer workbench"

  Scenario: A Clerk is not treated as Lawyer by the route layer
    # `/clerk` is retired, so a Clerk now enters the same namespace as everyone
    # else. The firm dashboard denies them through `require_lawyer`, which
    # returns an error the page renders as a withheld body rather than a status
    # refusal — so assert on what came back, not on the code. In production the
    # Rego rule refuses them before the handler; this suite runs a passthrough
    # policy, which is precisely why the handler check has to exist too.
    Given a seeded person "clerk@neonlaw.com" with role "clerk"
    When "clerk@neonlaw.com" opens /app/lawyer
    Then the response body does not contain "Lawyer workbench"

  Scenario: A Clerk sees only their supervised matters on the shared surface
    Given a seeded person "lawyer@neonlaw.com" with role "lawyer"
    And a seeded person "clerk@neonlaw.com" with role "clerk"
    And a Clerk project "Mailroom queue" assigned to "clerk@neonlaw.com" and supervised by "lawyer@neonlaw.com"
    When "clerk@neonlaw.com" opens /app/projects
    Then the response status is 200
    And the response body contains "Mailroom queue"
    And the response body contains "Supervising lawyer"

  Scenario: The retired Clerk namespace is gone
    Given a seeded person "clerk@neonlaw.com" with role "clerk"
    When "clerk@neonlaw.com" opens /clerk
    Then the response status is 404

  Scenario: The retired lawyer namespace is gone
    Given a seeded person "lawyer@neonlaw.com" with role "lawyer"
    When "lawyer@neonlaw.com" opens /lawyer
    Then the response status is 404
    When "lawyer@neonlaw.com" opens /lawyer/notations
    Then the response status is 404

  # The client-blocked-from-/app/lawyer scenario is enforced by embedded Rego
  # policy's `/app/lawyer` lawyer_tier rule in production; the BDD app runs
  # with `PolicyClient::passthrough` so every request reaches the handler.
