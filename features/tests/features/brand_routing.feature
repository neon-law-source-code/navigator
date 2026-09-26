Feature: Public routing on the firm's own host

  The `neon` binary composes the public routes for every house brand this
  repository registers, resolved per request from the `Host:` header; the
  firm holds the root on its own registered hosts. A path with no firm page
  answers `404`, the same answer as a path that never existed.

  This harness drives every scenario against the firm's own host, so
  `og:site_name` is "Neon Law" throughout — that name is what these scenarios
  assert; its absence marks a page mounted under the wrong brand. The
  exhaustive host-by-brand matrix (a registered host answering its own
  brand, an unregistered host redirecting) lives in `server/tests/routes.rs`,
  which drives one composed router directly rather than building a whole app
  per scenario.

  Background:
    Given the Neon Law Navigator public site is running

  # Every scenario here builds a whole app in its Background, and each build
  # takes a slice of the Dioxus pinned-worker pool, so this file stays a thin
  # representative sample on purpose. The exhaustive per-path table lives in
  # `server/tests/routes.rs`, which drives one router.
  #
  # This harness loads no Catalog content, so the material catalogs are not
  # asserted here; `server/tests/firm_routes.rs` covers them against real
  # content.

  Scenario: The firm's front door is the site root
    When a visitor opens /
    Then the response status is 200
    And the page is branded "Neon Law"

  Scenario Outline: A launched practice host serves its authored home page
    When a visitor opens / on host "<host>"
    Then the response status is 200
    And the page is branded "<brand>"
    And the response body contains "<title>"
    And the response body contains "<copy>"
    And the response body does not contain "Coming Soon"

    Examples:
      | host                           | brand                    | title                                      | copy                                  |
      | staging.neonlaw.com            | Neon Law                 | Neon Law \| Home                           | Keep building.                         |
      | staging.deleteyourdata.com     | DeleteYourData.com       | DeleteYourData.com \| Home                  | Your Life. Less Exposed.               |
      | staging.vestaestateplanning.com | Vesta Estate Planning    | Vesta Estate Planning \| Home    | For the life you build.                |
      | staging.misericordialaw.com     | Misericordia Injury Law  | Misericordia Injury Law \| Home  | You were hurt. Talk to a lawyer.       |
      | staging.abhayaimmigration.com   | Abhaya Immigration       | Abhaya Immigration \| Home       | Help with your immigration case.       |
      | staging.deleteyourdebt.com      | DeleteYourDebt.com       | DeleteYourDebt.com \| Home        | We defend you against debt collectors. |

  Scenario Outline: A live holding host serves its explicit holding page
    When a visitor opens / on host "<host>"
    Then the response status is 200
    And the page is branded "<brand>"
    And the response body contains "<title>"
    And the response body contains "<marker>"

    Examples:
      | host                       | brand           | title                    | marker                                  |
      | staging.lawyershook.com    | Lawyer Shook    | Lawyer Shook \| Home     | Shook Law PLLC is an American law firm  |
      | staging.summonsdefense.nyc | Summons Defense | Summons Defense \| Home  | Coming Soon                             |

  Scenario Outline: The firm's published anonymous surface serves at the root
    # Each is anonymous: a stranger deciding whether to hire a lawyer must not
    # meet a login door.
    When a visitor opens <path>
    Then the response status is 200

    Examples:
      | path       |
      | /notations |
      | /contact   |

  Scenario Outline: A retired practice path answers 404 rather than redirecting
    # The firm consolidated its whole public offer onto `/`, so these three
    # paths no longer name a page. They answer the same `404` as a path that
    # never existed, and carry no `Location`: a reader who follows an old link
    # is told the page is gone rather than silently rerouted to the root, where
    # they would have to find the section themselves.
    When a visitor opens <path>
    Then the response status is 404
    And the response carries no redirect

    Examples:
      | path      |
      | /disputes |
      | /business |
      | /services |

  Scenario Outline: A path with no firm page answers 404
    When a visitor opens <path>
    Then the response status is 404
    And the response carries no redirect

    Examples:
      | path                     |
      | /foundation              |
      | /foundation/mission      |
      | /foundation/transparency |
      | /mission                 |
      | /transparency            |
      | /education               |
      | /does-not-exist          |
