Feature: /app/projects/:code — the client's invoice card reads the Xero mirror

  The per-project invoice card the client sees is rendered from the
  local `xero_invoices` mirror, never from Xero live: the Dioxus
  `webapp::portal_project_detail` loader reads the mirror and the card
  shows the amount, the status, and a Paid/Due badge. The nightly
  `ReconcileInvoices`
  workflow folds Xero's payment state onto the mirror; this feature
  grounds the read side — what the client actually sees before and
  after that fold. The reconcile arithmetic lives in
  billing-workflows reconcile.rs.

  Background:
    Given the Neon Law Navigator app is running

  Scenario: A freshly raised invoice shows the matter total as Due
    Given a seeded person "capricorn@example.com" with role "client"
    And a project "Capricorn Estate" with "capricorn@example.com" as a participant
    And an AUTHORISED invoice of 333300 cents is mirrored for "Capricorn Estate"
    When "capricorn@example.com" opens the detail page for "Capricorn Estate"
    Then the response status is 200
    And the response body contains "Invoice"
    And the response body contains "Status: AUTHORISED"
    And the invoice card shows the "Due" badge

  Scenario: Once reconcile sees it paid in full, the card flips to Paid
    Given a seeded person "capricorn@example.com" with role "client"
    And a project "Capricorn Estate" with "capricorn@example.com" as a participant
    And an AUTHORISED invoice of 333300 cents is mirrored for "Capricorn Estate"
    And the invoice for "Capricorn Estate" is reconciled as paid in full
    When "capricorn@example.com" opens the detail page for "Capricorn Estate"
    Then the response status is 200
    And the response body contains "Status: PAID"
    And the invoice card shows the "Paid" badge

  Scenario: A matter with no invoice shows no invoice card
    Given a seeded person "leo@example.com" with role "client"
    And a project "Leo Matter" with "leo@example.com" as a participant
    When "leo@example.com" opens the detail page for "Leo Matter"
    Then the response status is 200
    And the page shows no invoice card
