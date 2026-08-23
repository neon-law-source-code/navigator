Feature: Public routing across one site's two faces

  The `neon` binary serves the whole site. Neon Law — the firm — holds the
  root, and the Neon Law Foundation sits beneath `/foundation`. They were two
  binaries on two hosts; the prefix is what keeps them separable now that one
  binary answers for both.

  The site publishes ONE header, so `og:site_name` is "Neon Law" on both faces:
  the nonprofit no longer wears a wordmark of its own, and is reached from the
  shared header's own Foundation entry. That name is what these scenarios assert
  — it used to differ per prefix and catch a page mounted under the wrong one,
  and what it catches now is a page that lost the site's name altogether.

  Background:
    Given the Neon Law Navigator public site is running

  # Every scenario here builds a whole app in its Background, and each build
  # takes a slice of the Dioxus pinned-worker pool, so this file stays a thin
  # representative sample on purpose. The exhaustive per-path tables — every
  # gated page, every retired redirect — live in `server/tests/routes.rs`,
  # which drives one router.
  #
  # This harness loads no Catalog content, so the material catalogs are not
  # asserted here; `server/tests/firm_routes.rs` covers them against real
  # content.

  Scenario: The firm's front door is the site root
    When a visitor opens /
    Then the response status is 200
    And the page is branded "Neon Law"

  Scenario: The Foundation's front door is its own prefix, under the shared name
    When a visitor opens /foundation
    Then the response status is 200
    And the page is branded "Neon Law"

  Scenario Outline: The firm's anonymous marketing surface serves at the root
    # These 404'd on the Foundation host while the two were separate. One
    # binary serves them now, and each is anonymous: a stranger deciding
    # whether to hire a lawyer must not meet a login door.
    When a visitor opens <path>
    Then the response status is 200

    Examples:
      | path           |
      | /contact       |
      | /litigation    |
      | /fractional-gc |
      | /services      |

  Scenario Outline: Everything else the Foundation publishes needs a session
    # The nav still names these, so a signed-out reader learns they exist and
    # meets the login door rather than a 404.
    When a visitor opens <path>
    Then the response status is 303

    Examples:
      | path                     |
      | /foundation/transparency |

  Scenario Outline: The Foundation's former root URLs redirect beneath its prefix
    # These were live pages on `neonlaw.org` for as long as the Foundation had
    # a host of its own, so they are the most-linked retired URLs on the site.
    # The firm holds the root now, so each has to be carried across rather than
    # dropped on a firm page.
    When a visitor opens <path>
    Then the response status is 308
    And the response redirects to <destination>

    Examples:
      | path            | destination                |
      | /mission        | "/foundation/mission"      |
      | /notations      | "/foundation/notations"    |
      | /transparency   | "/foundation/transparency" |
      | /education      | "/foundation/education"    |

  Scenario: The Foundation home is a page, not a redirect
    # It `301`ed to `/` while the Foundation was canonical at the site root.
    # Reinstating that would bounce the nonprofit's own home page onto the
    # firm's — the single most damaging way this consolidation could regress.
    When a visitor opens /foundation
    Then the response status is 200

  Scenario: An unknown route returns 404
    When a visitor opens /does-not-exist
    Then the response status is 404
