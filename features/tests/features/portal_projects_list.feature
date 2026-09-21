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
    And the response body contains "pitch"
    And the response body does not contain "no onboarding"

  Scenario: A matter with an onboarding artifact on file shows the active pill
    Given a seeded person "nick@neonlaw.com" with role "admin"
    And a project "Papered Ventures LLC" with no participants
    And a project "Papered Ventures LLC" with an onboarding document
    When "nick@neonlaw.com" opens the projects list
    Then the response status is 200
    And the response body contains "active"

  Scenario: A closed matter shows the closed pill and moves off the open tab
    Given a seeded person "nick@neonlaw.com" with role "admin"
    And a project "Wrapped Up Ventures LLC" with an onboarding document
    And a project "Wrapped Up Ventures LLC" is closed
    When "nick@neonlaw.com" opens the projects list
    Then the response status is 200
    And the response body does not contain "Wrapped Up Ventures LLC"

  Scenario: The closed tab shows a closed matter and the open tab hides it
    Given a seeded person "nick@neonlaw.com" with role "admin"
    And a project "Concluded Matter LLC" with an onboarding document
    And a project "Concluded Matter LLC" is closed
    When "nick@neonlaw.com" opens the closed projects list
    Then the response status is 200
    And the response body contains "Concluded Matter LLC"
    And the response body contains "closed"

  Scenario: An archived matter stays off the open tab and stays on the closed tab
    Given a seeded person "nick@neonlaw.com" with role "admin"
    And a project "Retired Matter LLC" with an onboarding document
    And a project "Retired Matter LLC" is closed
    And a project "Retired Matter LLC" is archived
    When "nick@neonlaw.com" opens the projects list
    Then the response status is 200
    And the response body does not contain "Retired Matter LLC"

  Scenario: The closed tab also carries an archived matter
    Given a seeded person "nick@neonlaw.com" with role "admin"
    And a project "Shelved Matter LLC" with an onboarding document
    And a project "Shelved Matter LLC" is closed
    And a project "Shelved Matter LLC" is archived
    When "nick@neonlaw.com" opens the closed projects list
    Then the response status is 200
    And the response body contains "Shelved Matter LLC"

  Scenario: A supervised Clerk sees only the matter they were added to
    Given a seeded person "lawyer@neonlaw.com" with role "lawyer"
    And a seeded person "clerk@neonlaw.com" with role "clerk"
    And a project "Nimbus Ventures LLC" with "lawyer@neonlaw.com" as the supervising lawyer DRI
    And a project "Nimbus Ventures LLC" with "clerk@neonlaw.com" as a supervised clerk
    And a project "Unsupervised Matter LLC" with "lawyer@neonlaw.com" as the supervising lawyer DRI
    When "clerk@neonlaw.com" opens the projects list
    Then the response status is 200
    And the response body contains "Nimbus Ventures LLC"
    And the response body does not contain "Unsupervised Matter LLC"
