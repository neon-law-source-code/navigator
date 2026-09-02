Feature: The client portal's signed-copy link reflects signature evidence, not storage alone

  ENG-421: the matter-detail page's "Signed copy" download must not
  appear just because an object exists at the signed-document storage
  key — that key is derived from the notation id alone, and nothing on
  a storage-only path confirms a provider ever executed the document.
  The link (and, with it, the client-facing execution claim) reads a
  completed `store::signatures` record instead.

  Background:
    Given the Neon Law Navigator app is running
    And a seeded person "capricorn@example.com" with role "client"
    And a project "Capricorn Estate Plan" with "capricorn@example.com" as a participant
    And a notation for "Capricorn Estate Plan" sent for signature

  Scenario: An object at the signed-document key alone offers no signed copy
    Given a document lands at the signed-document storage key for "Capricorn Estate Plan" outside the signature webhook
    When "capricorn@example.com" opens the detail page for "Capricorn Estate Plan"
    Then the response status is 200
    And the page offers no signed copy download

  Scenario: A completed signature offers the signed copy
    Given the notation's signature for "Capricorn Estate Plan" is completed by the provider
    When "capricorn@example.com" opens the detail page for "Capricorn Estate Plan"
    Then the response status is 200
    And the page offers a signed copy download

  Scenario: A declined envelope offers no signed copy, distinct from one still outstanding
    Given the notation's signature for "Capricorn Estate Plan" is declined by the provider
    When "capricorn@example.com" opens the detail page for "Capricorn Estate Plan"
    Then the response status is 200
    And the page offers no signed copy download
