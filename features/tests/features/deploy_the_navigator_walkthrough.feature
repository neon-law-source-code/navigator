Feature: Workshop "Operating Neon Law Navigator"

  Every renderable claim the "Operating Neon Law Navigator" workshop (DEPLOY.md)
  makes is grounded by a scenario here. If one breaks, the workshop's
  prose is stale — the same contract the sibling
  workshop_navigator_walkthrough.feature carries.

  This feature owns the half that needs the running web app: the
  workshop is registered on the firm surface, renders under the
  firm brand, opens with deploying your own, splits into
  stepped content, and shows the reader the real provisioning command. The other half —
  that the services, buckets, and command the prose names match what
  `navigator ops gcp setup` actually calls — is asserted next to the code in
  `cli/src/devx/gcp/mod.rs::deploy_workshop_prose_matches_the_dry_run_pipeline`,
  the only place `cli`'s `devx::gcp::run` is reachable. The two halves together
  are the cross-reference.

  The workshop is addressed to a deployer standing up their own instance —
  never to a legal client. It lives on the firm's host and it is public: the
  repository is open source, and the class explains how to run the software
  that repository publishes, so a login door in front of it would gate the one
  document a stranger who just cloned the tree needs. Claiming a completion
  certificate is still an authorization question, and that `POST` keeps its
  gate — pinned against the embedded policy in `server/tests/routes.rs`.

  Background:
    Given the "Operating Neon Law Navigator" workshop is loaded from the content directory

  Scenario: Remember — the workshop is registered on the firm surface
    When a reader visits "/workshops/deploy-the-navigator"
    Then the response status is 200
    And the page title is "Neon Law | Workshops | Deploy The Navigator"
    And the page shows no "not accepting clients" banner

  Scenario: Remember — the class is public, so no login door stands in front of it
    When a reader visits "/workshops/deploy-the-navigator"
    Then the response status is 200
    And the response carries no login redirect

  Scenario: Understand — the deploy walkthrough opens a stepped walkthrough
    Then the workshop's first section is titled "Deploy your own"
    And the workshop splits into at least 7 sections
    And the rendered body carries no duplicate top-level heading

  Scenario: Apply — the workshop shows the reader the real provisioning command
    Then the rendered workshop shows the command "cargo run -p cli -- ops gcp setup --project-id"
    And the rendered workshop shows the "--dry-run" flag

  Scenario: Verify — the markdown twin serves raw markdown
    When a reader visits "/workshops/deploy-the-navigator.md"
    Then the response status is 200
    And the response content-type is "text/markdown; charset=utf-8"
    And the markdown twin contains "## Agenda"
