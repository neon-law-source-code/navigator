Feature: /app/projects/:code — a client sees their own trust funds and no one else's

  IOLTA is a pooled bank account: the firm's Nevada account holds many
  clients' money at once. What a client may see is their own matter's
  position — paid in, still held, earned and drawn, refunded — folded from
  `store::trust`, which mirrors what the firm recorded in Xero. The pooled
  balance and every other matter's cents stay off this page.

  The mirroring arithmetic lives in billing-workflows reconcile.rs; this
  feature grounds what the client actually reads.

  Background:
    Given the Neon Law Navigator app is running

  Scenario: A matter that has never held client funds says nothing
    Given a seeded person "aries@example.com" with role "client"
    And a project "Aries Matter" with "aries@example.com" as a participant
    When "aries@example.com" opens the detail page for "Aries Matter"
    Then the response status is 200
    And the page shows no trust card

  Scenario: A deposit shows as funds held for that client
    Given a seeded person "taurus@example.com" with role "client"
    And a project "Taurus Matter" with "taurus@example.com" as a participant
    And a trust deposit of 500000 cents is mirrored for "Taurus Matter"
    When "taurus@example.com" opens the detail page for "Taurus Matter"
    Then the response status is 200
    And the response body contains "Funds we hold for you"
    And the response body contains "$5,000.00"

  Scenario: A refund lowers what is held without rewriting what was paid in
    Given a seeded person "gemini@example.com" with role "client"
    And a project "Gemini Matter" with "gemini@example.com" as a participant
    And a trust deposit of 500000 cents is mirrored for "Gemini Matter"
    And a trust refund of 200000 cents is mirrored for "Gemini Matter"
    When "gemini@example.com" opens the detail page for "Gemini Matter"
    Then the response status is 200
    And the response body contains "Paid in: $5,000.00"
    And the response body contains "Refunded: $2,000.00"
    And the response body contains "$3,000.00"

  Scenario: One client's page never carries another matter's cents
    Given a seeded person "cancer@example.com" with role "client"
    And a project "Cancer Matter" with "cancer@example.com" as a participant
    And a trust deposit of 111100 cents is mirrored for "Cancer Matter"
    And a seeded person "scorpio@example.com" with role "client"
    And a project "Scorpio Matter" with "scorpio@example.com" as a participant
    And a trust deposit of 999900 cents is mirrored for "Scorpio Matter"
    When "cancer@example.com" opens the detail page for "Cancer Matter"
    Then the response status is 200
    And the response body contains "$1,111.00"
    And the response body does not contain "$9,999.00"

  Scenario: One pooled withdrawal, and each client sees only their own share
    Given a seeded person "libra@example.com" with role "client"
    And a project "Libra Matter" with "libra@example.com" as a participant
    And a trust deposit of 700000 cents is mirrored for "Libra Matter"
    And a seeded person "pisces@example.com" with role "client"
    And a project "Pisces Matter" with "pisces@example.com" as a participant
    And a trust deposit of 700000 cents is mirrored for "Pisces Matter"
    And one pooled withdrawal settles 600000 cents for "Libra Matter" and 400000 cents for "Pisces Matter"
    When "libra@example.com" opens the detail page for "Libra Matter"
    Then the response status is 200
    And the response body contains "How your funds were applied"
    And the response body contains "$6,000.00"
    And the response body does not contain "$4,000.00"
    And the response body does not contain "$10,000.00"
