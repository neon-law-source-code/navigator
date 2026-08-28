Feature: Bulk-import to engagement, end to end

  A new book of business arrives as a list — organizations and the people
  who work at them — and the firm turns it into real records, then opens a
  matter for one of those people. This follows the lawyer who runs the
  import and the founder, Gemini, who becomes the firm's first engagement
  off that list.

  The list goes through the one shared import engine every surface uses (the
  CLI, the AIDA bulk-import tool): organizations become entities, contacts
  become persons, and each contact is linked to its organization. An
  imported contact carries the `client_contact` role — a known person, not
  yet an engaged client — until a matter is opened for them.

  Background:
    Given a fresh Neon Law Navigator app with the canonical templates seeded

  Scenario: Lawyer imports a contact list, then engages one of the contacts
    When a lawyer bulk-imports two organizations and three contacts
    Then the import succeeds with no errors
    And 2 organizations and 3 contacts are created
    And the contact "gemini@example.com" is linked to their organization
    When the firm opens the "onboarding__letter" matter for the imported contact "gemini@example.com"
    Then the matter is bound to the imported contact
