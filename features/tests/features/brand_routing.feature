Feature: Public routing on the firm's own host

  The `neon` binary composes the public routes for every house brand this
  repository registers, resolved per request from the `Host:` header; the
  firm holds the root on its own registered hosts. A path with no firm page
  answers `404`, the same answer as a path that never existed.

  This harness drives every scenario against the firm's own host, so
  `og:site_name` is "Neon Law" throughout — that name is what these scenarios
  assert; its absence marks a page mounted under the wrong brand. The
  exhaustive host-by-brand matrix (a second registered host answering its own
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

  Scenario Outline: The firm's anonymous marketing surface serves at the root
    # Each is anonymous: a stranger deciding whether to hire a lawyer must not
    # meet a login door.
    When a visitor opens <path>
    Then the response status is 200

    Examples:
      | path           |
      | /notations     |
      | /contact       |
      | /litigation    |
      | /fractional-gc |
      | /services      |

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
