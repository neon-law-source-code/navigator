Feature: Public routing across the one face the site serves

  The `neon` binary serves the whole site, and the firm holds the root. The
  Neon Law Foundation's surface is retired: it had the site root, then sat
  beneath `/foundation`, and every URL from both generations now answers
  `410 Gone`.

  `410` rather than `301` or `404`. There is no firm page carrying a
  nonprofit's mission letter, its CLE curriculum, or its governance
  disclosures, so a redirect would be a promise the other end cannot keep — and
  `410` is the one answer a search engine treats as a signal to drop the URL,
  where a `404` invites it to keep asking and a reader to assume they mistyped.

  The site publishes ONE header, so `og:site_name` is "Neon Law" throughout.
  That name is what these scenarios assert: it used to differ per prefix and
  catch a page mounted under the wrong one, and what it catches now is a page
  that lost the site's name altogether.

  Background:
    Given the Neon Law Navigator public site is running

  # Every scenario here builds a whole app in its Background, and each build
  # takes a slice of the Dioxus pinned-worker pool, so this file stays a thin
  # representative sample on purpose. The exhaustive per-path tables — every
  # gated page, every retired URL — live in `server/tests/routes.rs` and
  # `neon/tests/public_routes.rs`, which drive one router.
  #
  # This harness loads no Catalog content, so the material catalogs are not
  # asserted here; `server/tests/firm_routes.rs` covers them against real
  # content.

  Scenario: The firm's front door is the site root
    When a visitor opens /
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

  Scenario Outline: The Foundation's prefixed URLs are gone
    # The surface it held last. Its home was a page rather than a redirect
    # while the nonprofit was published, so the home has to be withdrawn
    # explicitly — a `/foundation` that still served would leave the retirement
    # half-done at the one URL most likely to be linked.
    When a visitor opens <path>
    Then the response status is 410
    And the response carries no redirect

    Examples:
      | path                     |
      | /foundation              |
      | /foundation/mission      |
      | /foundation/transparency |

  Scenario Outline: The Foundation's former root URLs are gone too
    # These were live pages on the site root for as long as the Foundation was
    # canonical there, so they are the most-linked retired URLs on the site.
    # They used to `308` beneath the prefix; the prefix is retired as well, so
    # carrying them across would land a reader on another `410`.
    When a visitor opens <path>
    Then the response status is 410
    And the response carries no redirect

    Examples:
      | path          |
      | /mission      |
      | /notations    |
      | /transparency |
      | /education    |

  Scenario: A retired gated URL is gone rather than a login door
    # `/foundation/transparency` answered `303` to the login door while the
    # surface was published. Retirement outranks the session boundary: sending
    # a reader to sign in for a page that no longer exists is the one outcome
    # worse than either answer alone.
    When a visitor opens /foundation/transparency
    Then the response status is 410

  Scenario: An unknown route returns 404
    # The distinction the retirement rests on: 410 is withdrawn, 404 never
    # existed. A route that was never tabled must not borrow the retired
    # answer.
    When a visitor opens /does-not-exist
    Then the response status is 404
