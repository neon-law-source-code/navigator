Feature: Neon Law Nautilus correspondence workflows

  Inbound screening mail — adverse-action notices, forwarded reports, and a
  consumer reporting agency's reinvestigation results — is classified so a
  lawsuit leaves the correspondence path. These scenarios lock down inbound
  triage and the litigation boundary.

  Scenario Outline: Inbound triage routes screening mail on an active matter
    Given an inbound screening email on an active matter saying "<text>"
    Then it is classified as "<class>" and routed to "<route>"

    Examples:
      | text                                                                    | class                 | route                 |
      | You are being sued; a summons is enclosed in this civil action.         | LawsuitOrSummons      | ReferLitigation       |
      | Enclosed are the results of your reinvestigation of the disputed item.  | ReinvestigationResult | ReinvestigationReview |
      | We denied your application based on information in your consumer report. | AdverseAction         | OpenDispute           |
      | Attached is the tenant screening report the landlord ran on you.        | ReportForwarded       | OpenDispute           |
      | Attached is my screening report; the eviction record is not mine.       | ReportForwarded       | OpenDispute           |
      | Please call our office at your convenience.                             | Other                 | LawyerReview           |

  Scenario: Inbound mail from an unmatched sender is flagged for a lawyer
    Given an inbound screening email with no matching matter saying "We denied your application based on your consumer report."
    Then it is routed to "LawyerReview"

  Scenario Outline: A consumer reporting agency's reinvestigation result is classified for the client
    Given a consumer reporting agency reinvestigation response saying "<text>"
    Then the FCRA result is "<result>"

    Examples:
      | text                                                         | result             |
      | The disputed item has been deleted from your file.           | CorrectedOrDeleted |
      | We verified the item as accurate; it remains on your report. | VerifiedUnchanged  |

  Scenario: A lawsuit leaves the shield and is referred to litigation counsel
    Given an inbound screening email on an active matter saying "You are being sued; a summons in this civil action is enclosed."
    Then it is classified as "LawsuitOrSummons" and routed to "ReferLitigation"
    And the litigation referral links to "mailto:contact@neonlaw.com" and is not answered as correspondence
