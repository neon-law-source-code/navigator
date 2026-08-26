Feature: Mutable two-sided intake assembles and sends through DocuSign

  The whole arc the mutable intake exists for: lawyers open a matter and
  send the client a link; the client answers their part themselves; lawyer
  add a custom clause for this matter only; and because the notation now
  carries custom content, it parks for attorney review before signature.
  The attorney approves, and the exact reviewed document — template body,
  interleaved answers, and the custom clause — goes out for the client's
  signature, the firm countersigning. The bytes the attorney approved are
  the bytes that get signed.

  Background:
    Given a retainer matter opened for "Libra" <libra@example.com>

  Scenario: Lawyer and client co-fill a notation, a clause forces review, then it signs
    When the client answers their part of the intake:
      | value             |
      | Libra Prime       |
    And lawyers add the custom clause "This engagement is governed by Nevada law."
    And lawyers finish the intake walk:
      | value                                |
      | Libra Prime                          |
      | Firm Principal                       |
      | Estate Plan                          |
      | 2026-09-01                           |
      | Draft and file the matter documents. |
      | 450 per hour                         |
      | nevada                               |
    Then the matter is awaiting attorney review
    And the matter has no signature request yet
    When the attorney approves and sends the document
    Then the matter has a signature request
    And the signature envelope routes the client before the firm
    And the sent document carries the custom clause
