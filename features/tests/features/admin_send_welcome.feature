Feature: Admin re-sends welcome email from /app/admin/people

  The firm sometimes needs to re-fire a welcome email — the OAuth callback
  fires one on first signup, but a user who never opened that one (or
  whose first signup predates this feature) should be reachable from
  the admin people index without leaving the browser.

  Every send through the `EmailService` trait is journaled to
  `sent_emails` by the `LoggingEmail` decorator regardless of trigger
  source. That guarantees the admin button and the callback share one
  audit story.

  Background:
    Given the application uses a CapturingEmail backend wrapped in LoggingEmail

  Scenario: Lawyer clicks Send welcome for an existing person
    Given a persons row for "Aries" with email "aries@example.com"
    When a lawyer POSTs to /app/api/people/{aries.id}/welcome
    Then the response HX-Redirects to the person's show view
    And exactly 1 sent_emails row exists
    And the sent_emails row has recipient "aries@example.com"
    And the sent_emails row has subject "Welcome to Neon Law"
    And the sent_emails row has template_slug "welcome"
    And the sent_emails row has outcome "sent"

  Scenario: Lawyer clicks Send welcome twice in a row
    Given a persons row for "Aries" with email "aries@example.com"
    When a lawyer POSTs to /app/api/people/{aries.id}/welcome
    And a lawyer POSTs to /app/api/people/{aries.id}/welcome
    Then exactly 2 sent_emails rows exist
    # Append-only: each click is its own row, never an UPDATE.

  Scenario: Lawyer clicks Send welcome for a missing person
    When a lawyer POSTs to /app/api/people/{random_uuid}/welcome
    Then the response is 404
    And no sent_emails rows are written
