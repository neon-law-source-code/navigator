Feature: /app/projects — the matter list, scoped per tier

  Owner and Admin read every matter in the deployment here
  (`store::projects::all`), the same administrative-listing shape the
  repository-reconciliation report already uses for its own
  deployment-wide question: privileged reach is a place you navigate
  to, not a silent widening of the matter surface itself. A Lawyer
  without that tier keeps the scoped read,
  `store::access::visible_projects_as_lawyer`, which grants no such
  bypass — a matter nobody put them on stays off their list.

  The status pill folds the "missing onboarding" signal in
  (`store::projects::matter_lifecycle`) rather than carrying a second,
  duplicate badge next to the matter name.

  Background:
    Given the Neon Law Navigator app is running

  Scenario: An admin sees a matter nobody put them on
    Given a seeded person "nick@neonlaw.com" with role "admin"
    And a project "Nebula Trust" with no participants
    When "nick@neonlaw.com" opens the projects list
    Then the response status is 200
    And the response body contains "Nebula Trust"

  Scenario: A lawyer does not see a matter nobody put them on
    Given a seeded person "lawyer@neonlaw.com" with role "lawyer"
    And a project "Hidden Matter" with no participants
    When "lawyer@neonlaw.com" opens the projects list
    Then the response status is 200
    And the response body does not contain "Hidden Matter"

  Scenario: The projects list no longer carries a duplicate onboarding pill
    Given a seeded person "nick@neonlaw.com" with role "admin"
    And a project "Freshly Opened LLC" with no participants
    When "nick@neonlaw.com" opens the projects list
    Then the response status is 200
    And the response body contains "needs onboarding"
    And the response body does not contain "no onboarding"
