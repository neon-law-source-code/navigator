//! Route parity for the Neon Law public host.
//!
//! Neon Law serves the firm brand surface and the host legal/crawler documents,
//! and the shared Navigator boundary still closes the authenticated surface. The state carries `PolicyClient::passthrough`, so
//! a `/app/lawyer` redirect proves the boundary is router composition, not policy.

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use store::test_support::mem_surreal;
use tower::ServiceExt;

async fn site_state() -> portal::AppState {
    let mut state = portal::test_support::app_state(mem_surreal().await).await;
    // A configured OAuth door so the login redirect target exists.
    state.oauth = Some(portal::OAuthConfig::new(
        "navigator",
        "secret",
        "http://localhost:3001/auth/callback",
        "https://rauthy.example/auth/v1/oidc/authorize",
        "https://rauthy.example/auth/v1/oidc/token",
    ));
    state
}

fn site_router(state: portal::AppState) -> Router {
    // Compose exactly as the `neon` binary does, through the same two
    // functions its `main` calls. Building the Dioxus half by hand here is
    // what let the binary ship without it: the suite proved a router this file
    // assembled, not the one `main` does.
    let host_dioxus = neon::public_dioxus_routers(&state);
    portal::bootstrap(
        state,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
        neon::public_routes(),
        neon::PUBLIC_PATHS,
        host_dioxus,
    )
    .expect("Neon Law public routes must not collide with Navigator")
}

async fn site_app() -> Router {
    site_router(site_state().await)
}

/// The firm host with the bundled Catalog materials loaded.
///
/// The shared builder ships an empty `WorkshopIndex`, so a talk's own page
/// would 404 on it for want of content rather than for want of a route — which
/// is the half most likely to drift after the catalog moved hosts.
async fn site_app_with_talks() -> Router {
    let mut state = site_state().await;
    state.workshops = portal::WorkshopIndex::new(
        portal::workshops::loader::load_navigator(std::path::Path::new(
            portal::DEFAULT_WORKSHOPS_DIR,
        ))
        .expect("load the bundled workshop materials"),
    );
    site_router(state)
}

/// A signed session cookie for `role`, against the key
/// `portal::test_support::app_state` builds its `SessionStore` with.
fn session_cookie_for_role(role: store::persons::Role) -> String {
    let sessions = portal::SessionStore::new(portal::test_support::TEST_SESSION_KEY);
    format!(
        "{}={}",
        portal::session::SESSION_COOKIE_NAME,
        sessions.encode(&portal::SessionData::fresh("firm-route-test", role))
    )
}

async fn role_get(
    app: &Router,
    path: &str,
    role: store::persons::Role,
) -> axum::http::Response<Body> {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header(axum::http::header::COOKIE, session_cookie_for_role(role))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn anon_get(app: &Router, path: &str) -> axum::http::Response<Body> {
    app.clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn body_string(resp: axum::http::Response<Body>) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// Every heading level in a rendered page, in document order.
///
/// Deliberately a scan for `<hN` rather than a parse: the pages under test are
/// SSR output with hydration comments inside the heading text, and the contract
/// being checked is only the sequence of levels.
fn heading_levels(html: &str) -> Vec<u32> {
    let bytes = html.as_bytes();
    let mut levels = Vec::new();
    for i in 0..bytes.len().saturating_sub(2) {
        if bytes[i] == b'<' && bytes[i + 1].eq_ignore_ascii_case(&b'h') {
            if let Some(level) = (bytes[i + 2] as char).to_digit(10) {
                if (1..=6).contains(&level) {
                    levels.push(level);
                }
            }
        }
    }
    levels
}

#[tokio::test]
async fn site_host_serves_the_firm_surface_and_host_documents() {
    let app = site_app().await;

    for path in [
        "/",
        "/navigator",
        "/blog",
        "/notations",
        "/contact",
        "/privacy",
        "/terms",
        "/robots.txt",
        "/sitemap.xml",
        "/llms.txt",
    ] {
        assert_ne!(
            anon_get(&app, path).await.status(),
            StatusCode::NOT_FOUND,
            "the Neon Law host must serve the firm/host page {path}"
        );
    }
}

/// The retired consumer plan's path 301s instead of 404ing.
///
/// The page is gone, but its URL was published, so inbound links and search
/// results still point at it. A 404 would strand every one of them. The
/// destination is the data-removal practice: the plan's largest block, and
/// the only part of it with a sibling practice of its own.
///
/// Asserted as a redirect rather than as an absence, because "the page does
/// not render" is also true of a 404 — which is the failure this guards.
#[tokio::test]
async fn the_retired_personal_plan_path_redirects_instead_of_404ing() {
    let app = site_app().await;
    let resp = anon_get(&app, "/personal").await;

    assert_eq!(
        resp.status(),
        StatusCode::MOVED_PERMANENTLY,
        "a retired marketing URL is a permanent move, not a temporary one"
    );
    assert_eq!(
        resp.headers()
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("https://www.deleteyourdata.com/"),
    );
}

/// Nothing on the firm's own site still sells to an individual.
///
/// The route being gone is not the same as the marketing being gone: the
/// words that sold the plan lived in the hero, the plan chooser, the services
/// page's subscription band, and the services catalog's plan pricing, each of
/// which renders independently of `/personal`. This walks the pages a visitor
/// actually reads.
#[tokio::test]
async fn no_firm_page_still_markets_the_retired_consumer_plan() {
    let app = site_app().await;
    for path in ["/"] {
        let body = body_string(anon_get(&app, path).await).await;
        for gone in [
            "Personal plan",
            "Personal Plan",
            "Personal-plan",
            r#"href="/personal""#,
        ] {
            assert!(
                !body.contains(gone),
                "{path} still carries {gone:?}: {body}"
            );
        }
    }
}

#[tokio::test]
async fn the_home_books_consultations_and_retires_separate_service_pages() {
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/").await).await;
    assert!(body.contains("Book a consultation"));
    assert!(body.contains("https://calendar.notion.so/meet/nick-shook/or15n4yy7"));
    assert!(body.contains("Employment") && body.contains("$5,000"));
    assert!(body.contains("/ once"));
    assert!(body.contains("One time comprehensive contract coverage. Scope agreed at the start."));
    assert!(body.contains("img/neon-home/neon-home-presentation.mp4"));
    assert!(body.contains("<video") && body.contains("video/mp4"));
    assert!(body.contains("href=\"/notations\""));
    assert!(!body.contains("Drafting, review, and litigation are priced separately."));
    assert!(body.contains("Commercial licenses available."));
    assert!(body.contains("/public/ferris.svg"));
    assert!(!body.contains("Our family") && !body.contains("A practice of"));
    assert!(body.contains("https://www.lawyershook.com"));
    for path in ["/business", "/services", "/disputes"] {
        let response = anon_get(&app, path).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert!(!response.headers().contains_key("location"));
        assert!(!body.contains(&format!("href=\"{path}\"")));
    }
}

#[tokio::test]
async fn the_neon_home_lead_modal_reuses_the_contact_form_contract() {
    let app = site_app().await;
    let home = body_string(anon_get(&app, "/").await).await;
    let contact = body_string(anon_get(&app, "/contact").await).await;

    assert!(home.contains("Book Consultation"), "primary CTA: {home}");
    assert!(
        home.contains(r#"action="/leads""#),
        "shared lead handler: {home}"
    );
    assert!(
        home.contains(r#"name="source_path" value="/""#),
        "the home form records the home path: {home}"
    );
    assert!(
        !home.contains(r#"name="source_path" value="/contact""#),
        "the home form does not claim to be the contact page: {home}"
    );
    assert!(
        home.contains(r#"name="website""#)
            && home.contains(r#"class="nav-honeypot nav-visually-hidden" aria-hidden="true""#)
            && home.contains(r#"tabindex="-1""#),
        "the honeypot remains present and hidden: {home}"
    );

    for legal_text in [
        "Optional. Message frequency varies. Message and data rates may apply. Reply STOP to opt out or HELP for help. Our ",
        "explain how we text and what we keep.",
        "Yes, Neon Law may send me text messages about this inquiry at this number, including automated texts. Texting is not a condition of hiring the firm.",
    ] {
        assert!(home.contains(legal_text), "home legal copy: {legal_text}: {home}");
        assert!(
            contact.contains(legal_text),
            "contact legal copy: {legal_text}: {contact}"
        );
    }
    assert!(home.contains(r#"href="/privacy""#), "privacy link: {home}");
    assert!(
        home.contains(r#"href="/privacy#text-messaging-sms""#),
        "texting terms link: {home}"
    );
}

#[tokio::test]
async fn the_footer_carries_the_pages_the_header_does_not() {
    // All twelve routes are one click away from every public page. Checked on
    // `/navigator` rather than `/`, because the footer is shared chrome and a
    // page that is not the home page proves it renders everywhere.
    //
    // Docs joined the row when workspace documentation became public. The
    // testimonials page is a public firm surface, so it stays one click away
    // on the same row.
    //
    // `/privacy` and `/terms` ride the row on the same footing as the rest.
    // UX is the one entry that links off-site, to the platform's design
    // showcase, rather than to a path this host serves. `/api` and `/team`
    // joined when the Swagger explorer alias and the firm's roster published.
    const ROW: [&str; 12] = [
        "/api",
        "/blog",
        "/contact",
        "/glossary",
        "/navigator",
        "/notations",
        "/presentations",
        "/privacy",
        "/team",
        "/terms",
        "/testimonials",
        "https://neon-law-source-code.github.io/navigator-ux/",
    ];
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/navigator").await).await;
    let footer = body
        .split_once(r#"aria-label="More pages""#)
        .and_then(|(_, rest)| rest.split_once("</nav>"))
        .map(|(row, _)| row)
        .expect("the footer renders its link row as a labelled landmark");
    // The row is alphabetized by label. The ordering is a product decision,
    // asserted by position rather than left to the array literal.
    let positions: Vec<usize> = ROW
        .iter()
        .map(|href| {
            footer
                .find(&format!(r#"href="{href}""#))
                .unwrap_or_else(|| panic!("the footer links {href}: {footer}"))
        })
        .collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "the footer row is alphabetized by label: {footer}"
    );
    // Neither row links a page the site does not publish: the whole
    // `/foundation` tree.
    assert!(
        !footer.contains(r#"href="/foundation""#),
        "/foundation names no page, so the footer must not link it: {footer}"
    );
}

#[tokio::test]
async fn the_firm_footer_publishes_no_bar_number_and_no_qualified_office() {
    // The firm's regulated footer strip names the entity, the disclaimer, and
    // the offices — and nothing about who is licensed under what number.
    // `views::brand::FIRM_ATTORNEYS` is empty today, and `/team` is a static
    // statement with no per-attorney bar-credential disclosure at all.
    //
    // Both halves are the assertion. A bar number reappearing means
    // `views::brand::FIRM_ATTORNEYS` was refilled; an office note reappearing
    // means an address is being published with a qualification on it. Checked
    // on `/disputes` because the footer is shared chrome.
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/navigator").await).await;
    for retired in [
        "Bar No.",
        "Admitted in",
        r#"class="site-footer__licenses""#,
        r#"class="site-footer__office-note""#,
    ] {
        assert!(
            !body.contains(retired),
            "the firm footer must not carry {retired:?}: {body}"
        );
    }
    // The offices themselves still publish — the note went, not the address.
    // Each is set line by line, so its parts are asserted as the separate lines
    // the footer renders them as.
    assert!(
        body.contains("5150 Mae Anne Ave")
            && body.contains("Ste 405-9002")
            && body.contains("Reno, NV 89523"),
        "the firm's office is published, unqualified: {body}"
    );
}

/// Every published address is set over three lines — street, unit, then city —
/// so the suite has its own line and the city starts one rather than landing
/// wherever the narrow footer column wrapped.
///
/// Asserted at the route level as well as in the component because these are the
/// firm's real addresses out of `views::brand`: the component's fixture could
/// drift from them. The exact markup is pinned on the pure SSR path in
/// `webapp::components::site_footer`; hydration comments split each span's text
/// node here, so this asserts the breaks rather than the tags around them.
///
/// Scoped to the `site-footer__offices` grid rather than the whole page body, so
/// this asserts the line breaks the grid renders rather than any run of the same
/// street elsewhere on the page.
#[tokio::test]
async fn the_firm_footer_sets_each_office_over_three_lines() {
    let app = site_app().await;
    let full_body = body_string(anon_get(&app, "/navigator").await).await;
    let body = full_body
        .split(r#"<ul class="site-footer__offices""#)
        .nth(1)
        .and_then(|rest| rest.split("</ul>").next())
        .expect("the offices grid renders");
    for (street, unit, city) in [
        ("5150 Mae Anne Ave", "Ste 405-9002", "Reno, NV 89523"),
        ("12 E 49th St", "18th Floor", "New York, NY 10017"),
    ] {
        // Every line publishes...
        for line in [street, unit, city] {
            assert!(body.contains(line), "{line} publishes: {body}");
        }
        // ...and each break is real, not a line the column happened to wrap.
        assert!(
            !body.contains(&format!("{street}, {unit}")),
            "{street} breaks before {unit}: {body}"
        );
        assert!(
            !body.contains(&format!("{unit}, {city}")),
            "{unit} breaks before {city}: {body}"
        );
    }
    assert_eq!(
        body.matches(r#"class="site-footer__office-line""#).count(),
        6,
        "three lines for each of the firm's two offices: {body}"
    );
}

#[tokio::test]
async fn every_public_page_wears_the_brand_mark_as_its_tab_icon() {
    // The favicon is the same hexagon the header paints, so the tab cannot drift
    // from the page. Asserted by parts rather than as one literal tag:
    // `document::Link` decides its own attribute order.
    //
    // Route-level rather than a component test on purpose — `dioxus_ssr` renders
    // no `document::*` content, so a component test would pass on a page that
    // ships no icon at all.
    let app = site_app().await;
    for path in ["/"] {
        let body = body_string(anon_get(&app, path).await).await;
        let head = body.split_once("</head>").map_or("", |(head, _)| head);
        assert!(
            head.contains(r#"rel="icon""#),
            "{path} declares a tab icon in its head: {head}"
        );
        // The mark itself, from `views::brand`, and the `type` derived from it.
        // A `type` that disagrees with the bytes is an icon the browser
        // declines to draw, so the pair is asserted rather than the href alone.
        assert!(
            head.contains(r#"href="/public/logo.svg""#),
            "{path}'s tab icon is the firm's own mark: {head}"
        );
        assert!(
            head.contains(r#"type="image/svg+xml""#),
            "{path}'s icon type matches the mark's bytes: {head}"
        );
    }
}

#[tokio::test]
async fn the_sitemap_excludes_retired_service_pages() {
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/sitemap.xml").await).await;
    assert!(body.contains("/navigator"));
    for path in ["/services", "/disputes", "/business"] {
        assert!(!body.contains(path));
    }
}

/// Every currency amount `body` prints, each returned with the short run of
/// text that follows it.
///
/// Keyed on `$` against a digit rather than a bare `$`: a rendered Dioxus
/// document carries hydration script where a lone dollar sign is ordinary.
fn currency_amounts(body: &str) -> Vec<&str> {
    body.match_indices('$')
        .filter(|(at, _)| body[at + 1..].starts_with(|c: char| c.is_ascii_digit()))
        .map(|(at, _)| {
            let tail = &body[at..];
            let end = tail
                .char_indices()
                .nth(40)
                .map_or(tail.len(), |(offset, _)| offset);
            &tail[..end]
        })
        .collect()
}

/// Whether `body` prints a **fee** — a currency amount that is not a past
/// result.
///
/// The two are different regulated claims and must not be conflated. A fee is
/// what the firm charges, governed by Rule 7.1 and the firm's own rule that
/// engagements are quoted rather than posted. A past result is what a lawyer
/// recovered for a former client — an amount in a `/team` biography, covered
/// by the standing "past results do not guarantee a similar result" disclaimer
/// in the footer, and a legitimate thing for a litigator's bio to state.
///
/// Recovery amounts are written at scale (`$230 million`), so that is how they
/// are told apart. The consequence is the useful one: a bio that grew a *rate*
/// would still be caught, because a rate is not written in millions.
fn publishes_a_fee(body: &str) -> bool {
    currency_amounts(body)
        .iter()
        .any(|amount| !(amount.contains("million") || amount.contains("billion")))
}

/// No page on this host publishes a fee, except the two that now do on
/// purpose: `/services` and `/business`.
///
/// Litigation, being quoted per engagement, stays unpriced because its scope
/// is not knowable in advance and a posted number would fit nobody. `/contact`
/// is here too because it once named a consultation fee. A fee added to any
/// other page here fails rather than ships.
#[tokio::test]
async fn supporting_pages_do_not_publish_separate_fees() {
    let app = site_app().await;

    for unpriced in [
        "/notations",
        "/contact",
        "/navigator",
        "/blog",
        "/privacy",
        "/terms",
    ] {
        let body = body_string(anon_get(&app, unpriced).await).await;
        assert!(
            !publishes_a_fee(&body),
            "{unpriced} must publish no fee: {body}"
        );
    }
}

/// The platform page is the firm's, and it makes one invitation.
///
/// The firm builds Navigator, and the page offers free use to attorneys who
/// co-counsel a case with it. The invitation and the absence of a published
/// rate must survive on the rendered page.
#[tokio::test]
async fn the_navigator_page_invites_pro_bono_co_counsel_and_publishes_no_rate() {
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/navigator").await).await;
    assert!(
        body.contains("Free use for those who co-counsel with us."),
        "the page offers free use to co-counseling attorneys: {body}"
    );
    assert!(
        body.contains(
            "Anyone who co-counsels a case with us gets the software free for life for their own practices."
        ),
        "the lifetime software offer reaches the rendered page: {body}"
    );
    // The co-counsel invitation prefills the email subject, so the mailto the
    // page renders carries it through the recipient's client.
    assert!(
        body.contains("?subject=Co%2DCounseling%20for%20Good%20with%20AI"),
        "the invitation's email subject reaches the rendered mailto: {body}"
    );
    // A price, not a bare `$`: the rendered document carries hydration script
    // where a lone dollar sign is ordinary. What must never appear is a dollar
    // sign against a digit.
    let priced = body
        .match_indices('$')
        .any(|(at, _)| body[at + 1..].starts_with(|c: char| c.is_ascii_digit()));
    assert!(
        !priced,
        "the firm publishes no price on the website: {body}"
    );
    for removed in [
        "The manuals that go with the binary",
        "What a firm works with",
        "The licence, and the one thing we sell around it",
        "not yet signed or notarized",
    ] {
        assert!(
            !body.contains(removed),
            "the retired copy remains: {removed}: {body}"
        );
    }
}

/// The public Navigator page maps the Project's connected work around Navigator.
#[tokio::test]
async fn the_navigator_page_maps_a_connected_project() {
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/navigator").await).await;

    assert!(
        body.contains(r#"class="fm-project-network""#)
            && body.contains(r#"src="/public/navigator-wheel.svg""#),
        "the connected-Project diagram renders the Navigator wheel: {body}"
    );
    for label in [
        "Internal Slack",
        "Internal Notion",
        "GitHub",
        "Client portal",
        "Per-Project Inbox",
        "Google Drive folder",
        "Shared Slack",
        "Shared Notion",
        "Navigator",
        "Web API MCP CLI",
        "GitHub",
        "MCPs",
        "Court Listener",
        "Descrybe",
        "Exa",
        "Midpage",
        "Perplexity",
        "Agentic Legal Coding",
        "Antigravity",
        "Claude Code",
        "Codex",
        "Cursor",
        "SaaS",
        "DocuSign",
        "Google Workspace",
        "Descript",
        "Chatwoot",
        "Highlight",
        "Linear",
        "Mercury",
        "Twilio",
        "Xero",
    ] {
        assert!(body.contains(label), "the diagram names {label}: {body}");
    }
    assert!(
        !body.contains("separate from protected Project documents"),
        "the removed public-site node detail is absent: {body}"
    );
    assert!(
        !body.contains("Navigator keeps the Project in view while each connected service retains its own access controls."),
        "the removed access-controls sentence is absent: {body}"
    );
    assert!(
        !body.contains("The firms we serve work on it too."),
        "the removed co-counsel-network paragraph is absent: {body}"
    );
    assert!(
        !body.contains("Navigator Web"),
        "the center names Navigator rather than its prior web-only label: {body}"
    );
    assert!(
        body.contains("Per-project versioned text including notation templates and client portal."),
        "the GitHub node describes the Project source contract: {body}"
    );
    assert!(
        body.contains("Large document intake"),
        "the Google Drive node describes its intake role: {body}"
    );
    assert!(
        body.contains("Client collaboration when the Project uses it."),
        "the Shared Notion node describes its collaboration role: {body}"
    );
    assert!(
        body.contains("A Project can include one or more cases, companies, filings, and more")
            && body.contains("related to the best interest of our clients."),
        "the Project description explains how related work belongs together: {body}"
    );
    assert!(
        !body.contains("Navigator is a website, MCP, and CLI that helps us rapidly create documents, ground sources and truth claims, organize files and folders, and reuse the glossary and ontology."),
        "the removed Navigator summary is absent: {body}"
    );
}

/// `/navigator` publishes the CLI as three download boxes and the Homebrew
/// route, anonymously, at the release this deployment runs.
///
/// **This is the covering assertion for the whole downloads band**, and it has
/// to be a route test rather than a unit test for two reasons the unit tests
/// name: `document::Stylesheet` is collected by the fullstack head collector and
/// never appears in `dioxus_ssr::render` output, so only the real route can
/// prove `home.css` reaches the document; and the version is resolved from the
/// process environment at router-build time, so only the real composition proves
/// the page names a release at all.
///
/// The version is checked for CONSISTENCY rather than against a literal. Pinning
/// `26.8.20-hotfix.4` here would make every release bump a failing test, and it
/// would assert the manifest against itself. What must hold is that the string
/// the page prints is the string all three archives are fetched at — a page
/// naming one release and linking another is worse than one naming none.
#[tokio::test]
async fn the_navigator_page_publishes_the_cli_at_the_release_it_runs() {
    const DOWNLOAD_BASE: &str =
        "https://github.com/neon-law-source-code/navigator/releases/download/";

    let app = site_app().await;
    let body = body_string(anon_get(&app, "/navigator").await).await;

    // The version, read out of the first download href.
    //
    // Out of an ATTRIBUTE rather than the printed element's text, and that is
    // not fussiness: the fullstack SSR path writes hydration comment markers
    // between an element and its text, so splitting on the first `<` after
    // `<code class="fm-downloads__tag">` yields the marker and an empty string.
    // Attribute values carry no markers. The printed version is checked against
    // this one below, which is the assertion that actually matters.
    let version = body
        .split_once(DOWNLOAD_BASE)
        .and_then(|(_, rest)| rest.split_once('/'))
        .map(|(version, _)| version.to_string())
        .expect("the band links a release archive");
    assert!(
        !version.is_empty() && version != "unknown",
        "a deployment that cannot name its release must not publish a download \
         link built from the word `unknown`: {version}"
    );

    // The version the band PRINTS is the version it LINKS. A page naming one
    // release and fetching another is worse than one naming none.
    let printed = body
        .split_once(r#"class="fm-downloads__tag""#)
        .and_then(|(_, rest)| rest.split_once("</code>"))
        .map(|(region, _)| region)
        .expect("the band prints the release it runs");
    assert!(
        printed.contains(&version),
        "the printed release must be the one every href carries ({version}): {printed}"
    );

    // Linux, macOS in the middle, Windows on the right — each an absolute URL
    // at the public Release, and each saved rather than navigated to.
    let mut previous = 0usize;
    for (slug, extension) in [("linux", "tar.gz"), ("macos", "tar.gz"), ("windows", "zip")] {
        let filename = format!("navigator-{version}-{slug}.{extension}");
        let href = format!(
            "https://github.com/neon-law-source-code/navigator/releases/download/\
             {version}/{filename}"
        )
        .replace(char::is_whitespace, "");
        let at = body
            .find(&href)
            .unwrap_or_else(|| panic!("the {slug} box links {href}: {body}"));
        assert!(at > previous, "the boxes run Linux, macOS, Windows: {body}");
        previous = at;
        assert!(
            body.contains(&format!(r#"download="{filename}""#)),
            "the {slug} box saves its archive rather than navigating: {body}"
        );
    }

    // The boxes are the home page's illuminated card, which only holds while
    // the page hoists the sheet that defines it. A Dioxus page loads exactly
    // the stylesheets it names, so this is the assertion that stops the band
    // rendering as three unstyled anchors.
    assert!(
        body.contains("/public/css/home.css"),
        "the page hoists the sheet its boxes are styled by: {body}"
    );
    assert!(
        body.contains(r#"class="home-practices__grid fm-downloads__grid""#),
        "the boxes sit in the home page's grid, which arms the hover wash: {body}"
    );

    // The Homebrew route remains available alongside the archive downloads.
    let install = webapp::cli_release::HOMEBREW_INSTALL_COMMAND;
    assert_eq!(
        body.matches(install).count(),
        1,
        "the tap-qualified install command renders once: {body}"
    );
    assert!(
        !body.contains("brew upgrade "),
        "brew upgrades in place, so the page does not publish a second line: {body}"
    );
    for removed in [
        "The manuals that go with the binary",
        "What a firm works with",
        "The licence, and the one thing we sell around it",
        "not yet signed or notarized",
    ] {
        assert!(
            !body.contains(removed),
            "the retired copy remains: {removed}: {body}"
        );
    }
}

/// The glossary — the documentation — reads for a visitor with no account,
/// and every retired documentation path sends that visitor to it rather than
/// to the login door.
#[tokio::test]
async fn the_glossary_reads_anonymously_on_the_firm_host() {
    let app = site_app().await;

    let response = anon_get(&app, "/glossary").await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "/glossary renders for a reader with no account"
    );

    for path in ["/docs", "/docs/glossary", "/docs/index", "/app/docs"] {
        let response = anon_get(&app, path).await;
        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT, "{path}");
        assert_eq!(
            response.headers().get("location").unwrap(),
            "/glossary",
            "{path} gets the glossary, not a login redirect"
        );
    }
}

#[tokio::test]
async fn home_publishes_no_amount_in_controversy_and_no_co_counsel_claim() {
    // Three claims came off the home page deliberately, and each is the kind a
    // future copy edit could reintroduce without noticing.
    //
    // The amount in controversy described *pending* matters rather than a
    // result, so the standing "past results do not guarantee a similar result"
    // disclaimer in the footer does not cover it. It came off both the record
    // strip and the prose sentence beneath it — removing only one leaves the
    // claim on the page in a different font.
    //
    // The co-counsel paragraph called the bench "elite litigators", a
    // comparative superlative about other lawyers. The `/team` bench card is a
    // separate surface and keeps its own copy; this guard is home-page only.
    //
    // The CTA lost its pricing suffix: "Contact us" is a substring of the
    // retired "Contact us for pricing", so the positive assertion in
    // `home_states_the_practice_and_prices_through_contact` cannot tell the two
    // labels apart. This is what pins the shorter one.
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/").await).await;
    for retired in [
        "9 figures",
        "In controversy",
        "nine figures in controversy",
        // The matter count came off the strip too. Guarded on the label rather
        // than on "6+", which is short enough to collide with unrelated markup.
        // The prose's lowercase "active matters" is a different string and is
        // deliberately still there.
        "Active matters",
        "elite litigators",
        "Contact us for pricing",
        // The record strip itself came off, taking the courts-and-admissions
        // figure with it. Guarded on the strip's own label and on the markup
        // that framed it: the prose still says "state and federal courts" in
        // lowercase, which is a different string and stays.
        "State &amp; federal courts",
        "litigation__stats",
    ] {
        assert!(
            !body.contains(retired),
            "the home page must not publish {retired:?}: {body}"
        );
    }
}

#[tokio::test]
async fn home_presents_company_counsel_pricing_and_business_library() {
    let app = site_app().await;
    let response = anon_get(&app, "/").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert_eq!(body.matches("<h1").count(), 1);
    for text in [
        "Keep building.",
        "Attorneys for the Ambitious.",
        "company-principles",
        "Justice Tech Association",
        "Rust NYC",
        "No VC vig",
        "pro bono",
        "site-header",
        "site-footer__legal",
    ] {
        assert!(body.contains(text), "missing {text}: {body}");
    }
    let experience = body
        .find("Lawyers who understand what you’re building.")
        .expect("experience section");
    let pricing = body.find("id=\"pricing\"").expect("pricing anchor");
    assert!(experience < pricing, "show experience before fees");
    assert!(body.contains("a 30-petabyte data warehouse for Apple Finance"));
    assert!(!body.contains("12 years of experience. Nine figures in exits."));
    assert!(!body.contains("A clear daily meter"));
    assert!(!body.contains("Separate work quoted before it starts"));
    assert!(!body.contains("Independent counsel for ambitious companies"));
    assert!(body.contains("Standard contract reviews returned within five business days"));
    assert!(body.contains("$500"));
    assert!(body.contains("/ contract"));
    assert!(body.contains("deal-exhibition"));
    assert!(body.contains("Computable Contracts that Scale"));
    assert!(body.contains("Notations turn essential agreements into clear, fillable documents"));
    assert!(body.contains("Explore the notations"));
    assert!(!body.contains("Explore the notations ↗"));
    assert!(!body.contains("Legal glue for your agreements."));
    assert!(!body.contains("The work behind your next move."));
    assert!(body.contains("Counsel on call."));
    assert!(body.contains("handle the paperwork."));
    assert!(!body.contains("company-questions"));
    assert!(!body.contains("A few useful answers."));
    assert!(body.contains("Your contracts. One fee."));
    assert!(body.contains("/ once"));
    assert!(body.contains("One time comprehensive contract coverage. Scope agreed at the start."));
    assert!(body.matches("Keep building.").count() >= 2);
    assert!(body.contains(r#"href="/presentations/rust-in-peace""#));
    assert!(body.contains(r#"href="/presentations""#));
    assert!(body.contains(r##"href="#pricing""##));
    // The pricing anchor includes the starting retainer and withdrawal terms.
    assert!(body.contains(r#"id="pricing" class="company-pricing""#));
    assert!(body.contains(r#"src="/public/ferris.svg""#));
    assert!(body.contains(r#"href="/notations#templates""#));
    assert!(body.contains("NEON LAW"));
    assert!(!body.contains("NEON LAW /"));
    assert!(!body.contains("Twelve business templates"));
    assert!(body.contains(r#"href="https://calendar.notion.so/meet/nick-shook/or15n4yy7""#));
}

#[tokio::test]
async fn home_explains_retainer_and_additional_fees() {
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/").await).await;
    for text in [
        "$50",
        "$500",
        "$5,000",
        "One time comprehensive contract coverage. Scope agreed at the start.",
        "A $10,000 retainer is held in trust when you sign up.",
        "We draw after we perform the work.",
        "The engagement letter sets scope, rates, and refunds.",
        "A daily minimum that keeps us on the line.",
        "Expedited reviews.",
        "A lawyer reviews your contract and provides feedback within in one business day.",
        "Each contract includes up to 50 pages. Each additional page is $5.",
        "US letter size (8.5 × 11 inches), in Times New Roman larger than 10 pt.",
        "Day-to-day questions over Slack",
        "Built in Rust. Open to inspection.",
        "We build production engineering systems to accurately and swiftly solve your matters in confidence.",
        "per active case",
        "You pay legal fees and case expenses.",
        "BUSL-1.1",
        "Commercial licenses available.",
    ] {
        assert!(body.contains(text), "missing fee or scope: {text}");
    }
    for allocation in [
        "After selected services: $5,500 earned, $4,500 remains held in trust.",
        "After selected services: $6,000 earned, $4,000 remains held in trust.",
        "After selected services: $6,500 earned, $3,500 remains held in trust.",
        "After selected services: $7,000 earned, $3,000 remains held in trust.",
    ] {
        assert!(
            body.contains(allocation),
            "incorrect trust allocation: {allocation}"
        );
    }
    assert!(body.contains("company-simulator"));
    for retired in ["Same-day review", "Privileged Slack channel"] {
        assert!(!body.contains(retired), "superseded offer: {retired}");
    }
    assert!(!body.contains(
        "We review contracts within five business days of acceptance as part of your plan"
    ));
    assert!(!body.contains("Notation Packages"));
    assert!(!body.contains("Your legal plan"));
    assert!(!body.contains("Revisions at Scale"));
    assert!(!body.contains("All your contracts"));
    assert!(!body.contains("Onboard and offboard contractors and employees worldwide."));
    assert!(!body.contains("This daily fee applies whether"));
    assert!(!body.contains("Choose a size when you need review the same day instead"));
    assert!(!body.contains("30 days cost $1,500."));
    let starting_amounts = body.find("Your contracts. One fee.").expect("setup");
    let daily_plan = body.find("Counsel on call.").expect("daily counsel plan");
    assert!(
        starting_amounts < daily_plan,
        "the $5,000 setup must appear before the daily plan"
    );
    assert!(body.contains(r#"href="/navigator""#), "missing /navigator");
    // The two sibling practices are named in the same section either way; the
    // launch gate decides whether each name is a link. A held-out practice's
    // host answers `404` (see `server::tests::routes`'s launch-gate tests), so
    // linking it would advertise an address rather than the offer.
    //
    // Derived from `is_live()` rather than listed, so the day Vesta or Abhaya
    // launches this test follows without an edit — a hand-maintained list
    // here is the same drift the gate exists to prevent.
    for (key, label) in [
        (views::brand::BrandKey::Abhaya, "Abhaya / Immigration"),
        (views::brand::BrandKey::Vesta, "Vesta / Estate planning"),
    ] {
        assert!(body.contains(label), "missing {label}");
        let linked = body.contains(&format!(r#"href="{}""#, key.public_home_href()));
        assert_eq!(
            linked,
            key.is_live(),
            "{} is {}live, so its name must {}link",
            key.as_str(),
            if key.is_live() { "" } else { "not " },
            if key.is_live() { "" } else { "not " },
        );
    }
    assert!(!body.contains("open source"));
}

#[tokio::test]
async fn home_does_not_restore_retired_home_surfaces() {
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/").await).await;
    // Causes of action belong on `/disputes`. Listing them in the home hero
    // is what made the page read as four firms at once.
    for retired in [
        "Personal injury",
        "criminal investigations",
        "business divorce",
        "Every problem is unique",
        "Whatever brings you in",
        "We are by your side through tough times",
        "Our complementary practice",
        "If you did not come for a dispute",
    ] {
        assert!(
            !body.contains(retired),
            "the home page must not publish {retired:?}: {body}"
        );
    }
    // Retired markup rather than wording: a practice grid, a per-practice card,
    // a chip list, and the glow whose wash bled past the hero's edge.
    for retired in [
        r#"class="practice-grid""#,
        r#"class="practice__heading""#,
        r#"class="litigation__heading""#,
        r#"class="firm-chip""#,
        "home-service__commitment",
        "home-process",
        "home-step",
        "home-closing",
        "firm-glow",
        "hero-neon",
        "catalog-card",
        "testimonial-section",
        "justice-banner",
    ] {
        assert!(
            !body.contains(retired),
            "the home page must not render {retired:?}: {body}"
        );
    }
}

/// The home page loads the firm's mark, and the mark's own files are served.
///
/// Split from the copy guard above it: that test speaks for what the page says,
/// this one for the brand assets it and every social scraper load. The site
/// carries exactly one NL mark, in two forms: `logo.svg` (the header vector,
/// `views::brand`'s `logo_href`) and `logo.png` (the full-resolution raster,
/// `social_image`, proven a PNG by `views::brand`'s
/// `the_brand_publishes_a_raster_social_image` since social scrapers won't
/// rasterize SVG). There is no separate firm mark or wand asset.
#[tokio::test]
async fn home_loads_the_firm_mark_and_serves_its_files() {
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/").await).await;
    assert!(
        body.contains(r#"src="/public/logo.svg""#),
        "home header loads the NL mark: {body}"
    );
    assert!(
        body.contains(r#"property="og:image""#) && body.contains("/public/logo.png"),
        "home social metadata loads the NL mark: {body}"
    );

    for (path, label, content_type) in [
        ("/public/logo.svg", "header vector mark", "image/svg+xml"),
        ("/public/logo.png", "full-resolution raster", "image/png"),
    ] {
        let asset = anon_get(&app, path).await;
        assert_eq!(asset.status(), StatusCode::OK, "{label} status");
        assert_eq!(
            asset
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some(content_type),
            "{label} content type"
        );
    }
}

#[tokio::test]
async fn firm_brand_png_is_a_high_resolution_square_asset() {
    let app = site_app().await;

    let response = anon_get(&app, "/public/logo.png").await;
    assert_eq!(response.status(), StatusCode::OK, "logo.png serves");
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("PNG body");
    assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"), "logo.png is a PNG");
    let width = u32::from_be_bytes(bytes[16..20].try_into().expect("PNG width"));
    let height = u32::from_be_bytes(bytes[20..24].try_into().expect("PNG height"));
    assert_eq!((width, height), (1024, 1024), "logo.png dimensions");
}

/// A path that names no page this site publishes answers `404`, and nothing
/// else does — a page that came back would republish an organization this
/// site does not represent.
#[tokio::test]
async fn an_unpublished_path_answers_not_found() {
    let app = site_app().await;

    for path in [
        "/foundation",
        "/foundation/education",
        "/foundation/attorneys",
        "/foundation/mission",
        "/foundation/notations",
        "/foundation/transparency",
        "/foundation/legal-aid",
        "/education",
        "/attorneys",
        "/mission",
        "/transparency",
        "/legal-aid",
    ] {
        let response = anon_get(&app, path).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert!(
            response.headers().get("location").is_none(),
            "{path} must not redirect: the site publishes no page there"
        );
    }
}

/// The talks catalog is the firm's, anonymous, and reachable at the two URLs
/// the decks are advertised under.
#[tokio::test]
async fn site_host_serves_the_talks_catalog_anonymously() {
    let app = site_app_with_talks().await;

    for path in [
        "/presentations",
        "/presentations/rust-in-peace",
        "/presentations/rust-in-peace.md",
        "/presentations/rust-in-peace/slides",
        "/presentations/rust-in-peace/step/1",
        "/presentations/rust-in-peace/display/1",
    ] {
        assert_eq!(
            anon_get(&app, path).await.status(),
            StatusCode::OK,
            "the Neon Law host publishes the talk page {path} to an anonymous reader"
        );
    }

    let index = body_string(anon_get(&app, "/presentations").await).await;
    let talk = "href=\"/presentations/rust-in-peace\"";
    assert!(index.contains(talk), "the catalog lists {talk}: {index}");
    assert!(
        !index.contains("More talks land here as we give them."),
        "the talks catalog has no placeholder footnote: {index}"
    );
}

/// A talk's hub renders under the firm's brand, and carries the deck
/// affordances: the start button, the Markdown twin, and the chapter rail.
#[tokio::test]
async fn a_talk_hub_renders_under_the_firm_brand() {
    let app = site_app_with_talks().await;

    let body = body_string(anon_get(&app, "/presentations/rust-in-peace").await).await;
    assert!(
        body.contains("<title>Neon Law | Presentations | Rust In Peace</title>"),
        "the talk's title names the firm, not the nonprofit: {body}"
    );
    // The overview's "Start →" button points at the first step under the
    // talk's base.
    assert!(body.contains("href=\"/presentations/rust-in-peace/step/1\""));
    // Live follow-along presentation mode is gone — each browser drives its
    // own deck, so no `/present` entry point may be offered.
    assert!(
        !body.contains("/present\""),
        "overview must not link a live presentation mode: {body}"
    );
    // It advertises its Markdown twin for machine readers. Asserted by parts
    // rather than as one literal tag: `document::Link` decides its own
    // attribute order, and the contract is the three values, not their
    // sequence.
    assert!(
        body.contains("rel=\"alternate\"")
            && body.contains("text/markdown")
            && body.contains("href=\"/presentations/rust-in-peace.md\""),
        "the markdown twin must be advertised in the head: {body}"
    );

    // Step 1 is the cover slide — its heading and the Megadeth/Ferris cover
    // image — with the rail showing chapter and section progress. Step 1 is
    // pinned because it is the entry point the overview links, not because of
    // where it sits in a running count.
    let step = body_string(anon_get(&app, "/presentations/rust-in-peace/step/1").await).await;
    assert!(step.contains("<h3>May my soul rust in peace</h3>"));
    assert!(
        step.contains("img/rust-in-peace/cover.png"),
        "the cover slide renders its published image: {step}"
    );
    assert!(step.contains("Chapter 1 of"));
    assert!(step.contains("Section 1 of"));

    // Every step page nests its headings h1 → h2 → h3: the deck title, the
    // chapter, then the slide. Deliberately not asserted per index — a deck is
    // authored prose and reordering it is not a regression, so this walks
    // whatever the deck currently holds. Before the rail carried the first two
    // levels, a slide's own `h3` was the page's first heading, skipping two
    // levels for anyone navigating by heading.
    let total = step
        .split("Section 1 of ")
        .nth(1)
        .and_then(|rest| rest.split('<').next())
        .and_then(|n| n.trim().parse::<usize>().ok())
        .unwrap_or_else(|| panic!("the rail must state the deck length: {step}"));
    assert!(total > 1, "a deck of {total} slides is not a deck");
    for n in 1..=total {
        let page =
            body_string(anon_get(&app, &format!("/presentations/rust-in-peace/step/{n}")).await)
                .await;
        let levels = heading_levels(&page);
        assert_eq!(
            levels.first(),
            Some(&1),
            "step {n} must open on an h1, got {levels:?}: {page}"
        );
        assert!(
            levels.contains(&2) && levels.contains(&3),
            "step {n} must carry its chapter as h2 and its slide as h3, got {levels:?}"
        );
        for pair in levels.windows(2) {
            assert!(
                pair[1] <= pair[0] + 1,
                "step {n} skips from h{} to h{} — headings must not skip a level, got {levels:?}",
                pair[0],
                pair[1]
            );
        }
    }

    // The two custom slide components replace their Markdown markers wherever
    // the deck author placed them, so they are found across the light table
    // rather than at a fixed step.
    let slides = body_string(anon_get(&app, "/presentations/rust-in-peace/slides").await).await;
    assert!(
        slides.contains("workshop-product-slide") && slides.contains("What our firm does"),
        "the custom firm-services slide must replace its Markdown marker: {slides}"
    );
    assert!(
        slides.contains("workshop-navigator-slide")
            && slides.contains(r#"data-practice-mark="helm""#)
            && slides.contains("github.com/neon-law-source-code/navigator"),
        "the Navigator identity slide must replace its Markdown marker: {slides}"
    );
}

/// A talk wears the firm's chrome, including its footer disclaimer.
///
/// The two categories share five router constructors, and this pins that a talk
/// page carries the firm's own footer rather than a bare one.
#[tokio::test]
async fn a_talk_wears_the_firm_footer() {
    let app = site_app_with_talks().await;

    for path in ["/presentations", "/presentations/rust-in-peace"] {
        let body = body_string(anon_get(&app, path).await).await;
        assert!(
            body.contains("Nothing here is legal advice without a signed retainer"),
            "{path} carries the firm's own required disclosure: {body}"
        );
    }
}

/// Every public firm page links the catalog from its footer, so a reader who
/// saw a talk at a conference finds it from anywhere on the site.
#[tokio::test]
async fn the_firm_footer_links_the_talks_catalog() {
    let app = site_app_with_talks().await;

    for path in ["/", "/presentations"] {
        let body = body_string(anon_get(&app, path).await).await;
        assert!(
            body.contains("href=\"/presentations\""),
            "{path} links the talks catalog from its footer: {body}"
        );
    }
}

/// Every public firm page links its own Privacy Policy and Terms of Service
/// from the footer.
///
/// Both documents already served at `/privacy` and `/terms`, and neither was
/// linked from the header or the legal strip — so before this row carried
/// them, a reader could only reach either by typing the URL. They are checked
/// on the same footing as the Blog and Contact because that is where they now
/// sit: one row, alphabetized, on every page of both faces.
#[tokio::test]
async fn the_firm_footer_links_privacy_and_terms() {
    let app = site_app_with_talks().await;

    for path in ["/", "/presentations"] {
        let body = body_string(anon_get(&app, path).await).await;
        for href in ["/privacy", "/terms"] {
            assert!(
                body.contains(&format!("href=\"{href}\"")),
                "{path} links {href} from its footer: {body}"
            );
        }
    }
}

/// The Navigator classes moved here with the talks, and read anonymously
/// exactly as the talks do — the catalog page included.
///
/// Every read face is checked, not just the hub: the catalog, the hub, the
/// light table, a classroom step, and the certificate confirmation. The gate
/// is absent from `catalog_material_routers` as a set, so every linked face
/// opens consistently.
#[tokio::test]
async fn the_workshops_surface_reads_anonymously() {
    let app = site_app_with_talks().await;

    for path in [
        "/workshops/use-the-navigator",
        "/workshops/use-the-navigator/slides",
        "/workshops/use-the-navigator/step/1",
        "/workshops/use-the-navigator/certificate/sent",
    ] {
        assert_eq!(
            anon_get(&app, path).await.status(),
            StatusCode::OK,
            "an anonymous reader opens {path}"
        );
    }

    // The catalog is public, so a missing class returns a direct `404`.
    assert_eq!(
        anon_get(&app, "/workshops/genai-training").await.status(),
        StatusCode::NOT_FOUND,
        "an unknown class is a 404, not a login redirect"
    );
}

#[tokio::test]
async fn the_retired_workshops_index_redirects_to_presentations() {
    let app = site_app_with_talks().await;
    let response = anon_get(&app, "/workshops").await;
    assert_eq!(response.status(), StatusCode::MOVED_PERMANENTLY);
    assert_eq!(
        response.headers().get("location").unwrap(),
        "/presentations"
    );
}

/// The certificate `POST` keeps its gate, and it is the only thing that does.
///
/// Who may CLAIM a completion certificate is an authorization question and
/// stays one even when the material is free to read, so it is asserted
/// separately from the read faces above.
#[tokio::test]
async fn the_certificate_claim_still_meets_the_session_boundary() {
    let app = site_app_with_talks().await;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/workshops/use-the-navigator/certificate")
                .header(
                    axum::http::header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .body(Body::from("name=A+Reader&email=reader@example.com"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "an anonymous claim meets the login door even though the class reads freely"
    );
}

/// The three Navigator classes load from the real content directory, render
/// under the firm brand, and land side by side on the catalog page.
#[tokio::test]
async fn the_three_classes_render_and_land_beside_each_other() {
    let app = site_app_with_talks().await;
    let lawyer = store::persons::Role::Lawyer;

    let body =
        body_string(role_get(&app, "/workshops/contribute-to-the-navigator", lawyer).await).await;
    assert!(
        body.contains("<title>Neon Law | Workshops | Contribute To The Navigator</title>"),
        "the class title names the firm: {body}"
    );
    assert!(
        body.contains("href=\"/workshops/contribute-to-the-navigator/step/1\""),
        "overview links its first slide"
    );

    // The catalog lists all three, simple titles and all.
    let index = body_string(role_get(&app, "/presentations", lawyer).await).await;
    for href in [
        "href=\"/workshops/use-the-navigator\"",
        "href=\"/workshops/deploy-the-navigator\"",
        "href=\"/workshops/contribute-to-the-navigator\"",
    ] {
        assert!(index.contains(href), "index should list {href}: {index}");
    }
    assert!(
        index.contains("For Lawyers and Clerks"),
        "the workshop audience label names both tiers: {index}"
    );
    assert!(
        index.contains("For Admins and Owners"),
        "the operations audience label names both tiers: {index}"
    );
    assert!(
        !index.contains("More workshops land here as we run them."),
        "the workshops catalog has no placeholder footnote: {index}"
    );

    // The markdown twin serves raw markdown with the right content type — the
    // machine-reader surface every class has.
    let resp = role_get(&app, "/workshops/contribute-to-the-navigator.md", lawyer).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default(),
        "text/markdown; charset=utf-8"
    );
    assert!(
        body_string(resp)
            .await
            .contains("# Contributing to Neon Law Navigator"),
        "raw markdown title"
    );

    // Workshops are public teaching material, so llms.txt advertises their
    // raw Markdown twins to the anonymous crawler.
    let llms = body_string(anon_get(&app, "/llms.txt").await).await;
    assert!(
        llms.contains("/workshops/"),
        "llms.txt must advertise the public workshop corpus: {llms}"
    );
}

/// The Using-the-Navigator class teaches the single litigation matter development flow,
/// read from the real content directory.
#[tokio::test]
async fn the_navigator_class_renders_the_sample_project_exercise() {
    let materials = portal::workshops::loader::load_navigator(std::path::Path::new(
        portal::DEFAULT_WORKSHOPS_DIR,
    ))
    .expect("load real workshop content");
    let step = materials
        .iter()
        .find(|material| material.slug == "use-the-navigator")
        .expect("navigator workshop")
        .sections
        .iter()
        .position(|section| section.title == "Make a sample-project change")
        .expect("sample-project section")
        + 1;

    let app = site_app_with_talks().await;
    let body = body_string(
        role_get(
            &app,
            &format!("/workshops/use-the-navigator/step/{step}"),
            store::persons::Role::Lawyer,
        )
        .await,
    )
    .await;
    assert!(body.contains("sample-litigation"), "{body}");
    assert!(body.contains("stages the output"), "{body}");
    assert!(body.contains("manifest name remains"), "{body}");
}

/// The one write on the class surface takes the same gate as the pages, so an
/// anonymous caller cannot request a certificate for a class they cannot read.
#[tokio::test]
async fn the_workshop_certificate_post_refuses_an_anonymous_caller() {
    let app = site_app_with_talks().await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/workshops/use-the-navigator/certificate")
                .header(
                    axum::http::header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .body(Body::from(
                    "name=Jane&email=jane%40example.com&csrf_token=bogus",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn site_host_still_closes_the_shared_navigator_boundary() {
    let app = site_app().await;

    assert_eq!(
        anon_get(&app, "/app/lawyer").await.status(),
        StatusCode::SEE_OTHER,
        "an anonymous browser at /app/lawyer is sent to the login door"
    );
    assert_eq!(
        anon_get(&app, "/app/api/people").await.status(),
        StatusCode::UNAUTHORIZED,
        "an anonymous machine caller at /app/api/people gets a structured 401"
    );
    assert_eq!(
        anon_get(&app, "/app/health").await.status(),
        StatusCode::OK,
        "the health probe stays anonymous"
    );
}

// ---- Blog surface (firm-owned, relocated from web with the host split #771) ----

fn blog_state_with_one_post() -> portal::BlogIndex {
    portal::BlogIndex::new(vec![portal::BlogPost {
        slug: "thanks-apple".into(),
        date: chrono::NaiveDate::from_ymd_opt(2026, 6, 19).unwrap(),
        title: "Thanks, Apple".into(),
        description: "A short note of thanks.".into(),
        body_html: "<p>We want to say thank you.</p>".into(),
    }])
}

#[tokio::test]
async fn blog_index_lists_posts() {
    let mut state = site_state().await;
    state.blog = blog_state_with_one_post();
    let app = site_router(state);
    let resp = app
        .oneshot(Request::builder().uri("/blog").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("Thanks, Apple"));
    assert!(body.contains("href=\"/blog/thanks-apple\""));
    assert!(body.contains("June 19, 2026"));
}

#[tokio::test]
async fn blog_post_renders_body() {
    let mut state = site_state().await;
    state.blog = blog_state_with_one_post();
    let app = site_router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/blog/thanks-apple")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("We want to say thank you."));
    assert!(body.contains("href=\"/blog\""));
}

#[tokio::test]
async fn real_thanks_apple_post_is_capped_and_renders_the_photo_collage() {
    // End-to-end over the SHIPPED post file: the loader parses
    // `content/blog/20260619_thanks_apple.md`, the router renders it, and
    // we assert the two things this change wired up — the 65ch reading
    // measure and the photo collage, authored as a Bootstrap grid of images
    // that resolves through the asset seam to `/public/img/thanks-apple/*.jpg`.
    let mut state = site_state().await;
    state.blog = portal::blog::load_dir(std::path::Path::new(portal::DEFAULT_BLOG_DIR)).unwrap();
    let app = site_router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/blog/thanks-apple")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    // Same measure as the mission letter.
    assert!(
        body.contains("class=\"blog-post\"") && body.contains("max-width: 65ch"),
        "post should carry the blog-post class capped at 65ch"
    );
    // Every recovered collage photo renders through the same seam.
    for slug in [
        "collage-3",
        "collage-4",
        "collage-5",
        "collage-6",
        "collage-8",
        "apple-park-sunset",
        "apple-park-team",
        "ethiopian-dinner",
        "team-lunch",
        "london-tower-bridge",
        "sharks-game",
        "farewell-crew",
        "curry-night",
    ] {
        let src = format!("src=\"/public/img/thanks-apple/{slug}.jpg\"");
        assert!(body.contains(&src), "farewell-row photo missing: {src}");
    }
    // The original letter copy is untouched.
    assert!(body.contains("Thanks, Apple"));
}

#[tokio::test]
async fn blog_unknown_slug_returns_404() {
    let mut state = site_state().await;
    state.blog = blog_state_with_one_post();
    let app = site_router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/blog/nope")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn blog_post_head_prefixes_the_brand_and_shares_the_card() {
    // The Dioxus post head carries the brand-prefixed `<title>` the
    // `PageLayout` emitted and the same Open Graph / Twitter share card, so a
    // shared post link still previews the firm ahead of the post name.
    let mut state = site_state().await;
    state.blog = blog_state_with_one_post();
    let app = site_router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/blog/thanks-apple")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("<title>Neon Law | Blog | Thanks Apple</title>"),
        "brand-prefixed document title: {body}"
    );
    assert!(
        body.contains(r#"<meta content="Neon Law | Thanks, Apple" property="og:title"/>"#),
        "Open Graph share title"
    );
    assert!(
        body.contains(r#"<meta name="twitter:card" content="summary"/>"#),
        "Twitter Card"
    );
}

#[tokio::test]
async fn blog_post_wraps_in_the_public_shell() {
    // The port renders inside the shared public shell (header + legal footer)
    // and stamps the English `<html lang>`, like every other firm Dioxus page.
    let mut state = site_state().await;
    state.blog = blog_state_with_one_post();
    let app = site_router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/blog/thanks-apple")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("site-header"), "public header chrome: {body}");
    assert!(
        body.contains("site-footer__legal"),
        "public legal footer chrome"
    );
    assert!(
        body.contains("<html lang=\"en\">"),
        "English document language"
    );
}

#[tokio::test]
async fn blog_legacy_underscore_slug_redirects_to_kebab() {
    // A legacy underscore link (`thanks_apple`) permanently redirects to the
    // canonical kebab-case URL, the behavior the handler owned.
    let mut state = site_state().await;
    state.blog = blog_state_with_one_post();
    let app = site_router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/blog/thanks_apple")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(
        resp.headers().get("location").and_then(|v| v.to_str().ok()),
        Some("/blog/thanks-apple"),
    );
}

#[tokio::test]
async fn blog_percent_encoded_slug_resolves_to_the_post() {
    // A percent-encoded spelling of a valid slug (`thanks%2Dapple`, the hyphen
    // encoded) resolves to the same post, because the pre-layer decodes the
    // `{slug}` path parameter — the behavior the handler's `Path<String>`
    // owned. Comparing the raw, still-encoded segment would 404 a valid URL.
    let mut state = site_state().await;
    state.blog = blog_state_with_one_post();
    let app = site_router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/blog/thanks%2Dapple")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("We want to say thank you."));
}

/// The legacy underscore form of a slug permanently redirects to the
/// canonical kebab-case form — `thanks_apple` becomes `thanks-apple` —
/// so links written either way resolve to the same post.
#[tokio::test]
async fn blog_underscore_slug_redirects_to_kebab() {
    let mut state = site_state().await;
    state.blog = blog_state_with_one_post();
    let app = site_router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/blog/thanks_apple")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::LOCATION)
            .and_then(|v| v.to_str().ok()),
        Some("/blog/thanks-apple"),
    );
}

/// Every underscore in a multi-word slug is rewritten, and the redirect
/// target then resolves to the real post.
#[tokio::test]
async fn blog_redirect_rewrites_all_underscores_and_target_resolves() {
    let mut state = site_state().await;
    state.blog = portal::BlogIndex::new(vec![portal::BlogPost {
        slug: "a-long-post-title".into(),
        date: chrono::NaiveDate::from_ymd_opt(2026, 6, 19).unwrap(),
        title: "A Long Post Title".into(),
        description: "Multi-word slug.".into(),
        body_html: "<p>Body here.</p>".into(),
    }]);
    let app = site_router(state);

    // Underscore request → 308 to the all-hyphen form.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/blog/a_long_post_title")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
    let location = resp
        .headers()
        .get(axum::http::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();
    assert_eq!(location, "/blog/a-long-post-title");

    // Following the redirect lands on the real post.
    let resp = app
        .oneshot(
            Request::builder()
                .uri(&location)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("Body here."));
}

/// A kebab-case slug is served directly — no redirect bounce.
#[tokio::test]
async fn blog_kebab_slug_is_served_without_redirect() {
    let mut state = site_state().await;
    state.blog = blog_state_with_one_post();
    let app = site_router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/blog/thanks-apple")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ---- Notations format introduction and template catalog ----

#[tokio::test]
async fn notations_page_renders_format_structure_and_template_links() {
    let app = site_app().await;
    let resp = anon_get(&app, "/notations").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("notations-page"),
        "notation format page: {body}"
    );
    for section in ["notation-flow", "notation-source", "rules", "templates"] {
        assert!(
            body.contains(&format!("id=\"{section}\"")),
            "missing {section}"
        );
    }
    assert!(body.contains("/public/notation-pen.svg"));
    assert!(!body.contains("/public/navigator-wheel.svg"));
    assert!(body.contains("/public/css/notations.css"));
    let navigator = body_string(anon_get(&app, "/navigator").await).await;
    assert!(!navigator.contains("id=\"notation-flow\""));
    // Every card's default link opens the notation's own show page now — the
    // raw GitHub source lives on that page, not the catalog card.
    assert!(
        body.contains(r#"href="/notations/onboarding-letter""#),
        "onboarding letter: {body}"
    );
    assert!(
        body.contains(r#"href="/notations/offboarding-letter""#),
        "offboarding letter: {body}"
    );
    assert!(
        body.contains(r#"href="/notations/nevada-llc-formation""#),
        "LLC formation form: {body}"
    );
    assert!(
        body.contains(r#"href="/notations/irs-form-990""#),
        "Form 990: {body}"
    );
    assert!(body.contains("site-header"), "public header chrome");
    assert!(body.contains("site-footer__legal"), "public legal footer");
}

// ---- Contact surface (firm-owned, Dioxus SSR port #641 / #730 PR6) ----

#[tokio::test]
async fn contact_page_lists_the_firm_channel_and_shares_the_card() {
    // The Dioxus contact port renders the firm's contact channels and the
    // social-share card the head declares.
    let app = site_app().await;
    let resp = anon_get(&app, "/contact").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("<title>Neon Law | Contact</title>"),
        "brand-prefixed document title: {body}"
    );
    assert!(
        body.contains(r#"<meta content="Neon Law | Contact" property="og:title"/>"#),
        "the share card carries the page title: {body}"
    );
    assert!(body.contains("site-header"), "public header chrome");
    assert!(body.contains("site-footer__legal"), "public legal footer");
}

#[tokio::test]
async fn contact_returns_contact_page_html() {
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/contact").await).await;
    assert!(body.contains("<title>Neon Law | Contact</title>"));
    // The published address, which is `contact@` rather than the `support@`
    // mailbox some other CTAs write to.
    assert!(body.contains("mailto:contact@neonlaw.com"));
    assert!(
        body.contains(r#"href="https://calendar.notion.so/meet/nick-shook/or15n4yy7""#),
        "the contact CTA reaches the firm: {body}"
    );
    // The page's own content, not just chrome that happens to mention contact
    // — a reader looking for how to reach the firm must find the inbox inside
    // the page article itself.
    let article = body
        .split(r#"<article class="contact-page""#)
        .nth(1)
        .expect("the contact page's own content renders");
    assert!(
        article.contains("mailto:contact@neonlaw.com"),
        "the contact channel sits inside the page's own article: {article}"
    );
}

/// The shared footer publishes the source repository on every public page, on
/// both faces of the site.
///
/// The component and chrome tests prove the line renders from the right props;
/// this proves the props actually reach a served page — the wiring through
/// `chrome_for` and the two `inject_*_chrome` layers, which no unit test sees.
///
/// The star count is deliberately not asserted. It comes from a cache that only
/// `portal::hosting::run` starts filling, so a test-built router publishes the
/// link with no number — which is the point: the suite reaches no network, and
/// the page is complete without the count.
#[tokio::test]
async fn every_public_page_links_the_source_repository() {
    let app = site_router(site_state().await);
    for uri in ["/", "/notations", "/navigator"] {
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{uri}");
        let body = body_string(resp).await;
        assert!(
            body.contains(r#"href="https://github.com/neon-law-source-code/navigator""#),
            "{uri} links the repository: {body}"
        );
        assert!(
            body.contains("github-stars__repo") && body.contains("neon-law-source-code/navigator"),
            "{uri} names it as the project's source: {body}"
        );
        // No number, because nothing spawned the refresh — the link stands on
        // its own rather than rendering a placeholder.
        assert!(
            !body.contains("GitHub stars"),
            "{uri} publishes no count it has not fetched: {body}"
        );
    }
}

/// `/team` is a static page now — one statement, no roster and no per-person
/// profile. A slug that used to resolve to a live team member (`/team/nick`,
/// `/team/jask`) names nothing any more and must 404 rather than fall through
/// to anything.
///
/// `/app/team` is a different surface and is deliberately NOT checked here. It
/// is the authenticated matter-side roster inside the portal, and conflating
/// the two is how a working page gets deleted next.
#[tokio::test]
async fn the_team_page_publishes_the_one_statement_and_no_profile() {
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/team").await).await;
    assert!(
        body.contains(webapp::team_page::STATEMENT),
        "the page states the one sentence: {body}"
    );
    for path in ["/team/nick", "/team/jask"] {
        let resp = anon_get(&app, path).await;
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "{path} named a live profile the roster no longer has; now it names nothing"
        );
    }
}

/// A mounted white-label brand bundle rebrands the firm home and its chrome.
/// This is the render-side coverage that complements the declared-asset-serving
/// test in `routes.rs`.
#[tokio::test]
async fn a_mounted_brand_bundle_rebrands_the_firm_home() {
    let bundle_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        bundle_dir.path().join("navigator.yaml"),
        "version: 1\nbrand:\n  firm: Acme Law\n  firm_legal_entity: Acme Law\n  support_email: help@acme.example\nassets:\n  firm_logo: logo.svg\n  firm_logo_raster: logo.png\n  static_files:\n    theme.css: theme.css\n",
    )
    .unwrap();
    std::fs::write(
        bundle_dir.path().join("logo.svg"),
        br#"<svg xmlns="http://www.w3.org/2000/svg"></svg>"#,
    )
    .unwrap();
    std::fs::write(bundle_dir.path().join("logo.png"), b"synthetic-png").unwrap();
    std::fs::write(bundle_dir.path().join("theme.css"), b":root{--brand:test}").unwrap();
    let bundle = views::brand_bundle::BrandBundle::load(bundle_dir.path()).unwrap();
    let mut state = site_state().await;
    state.brand_bundle = Some(bundle);
    let app = site_router(state);

    // The custom firm brand rebrands the home page title and its chrome logo.
    let home = app
        .clone()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(home.status(), StatusCode::OK);
    let html = body_string(home).await;
    assert!(html.contains("<title>Acme Law | Home</title>"), "{html}");
    assert!(
        html.contains("src=\"/public/brand/firm-logo.svg\""),
        "{html}"
    );
    // A white-label host must not publish this firm's name at the top of its own
    // home page. The hero used to carry a wordmark and this checked that
    // element; the photograph carries no text now, so the two places the brand
    // still speaks are the header mark and the `<h1>` statement beneath it.
    //
    // Scoped to those rather than the whole document, because the built-in legal
    // entity legitimately reaches this page elsewhere: a bundle naming no
    // `firm_legal_entity` inherits the compiled default, which the footer
    // copyright then prints (`views::brand::Branding::from_manifest`, pinned by
    // `views::brand::tests`).
    let (_, after_brand) = html
        .split_once("site-header__brand")
        .expect("the header renders a brand mark");
    let (brand, _) = after_brand
        .split_once("</a>")
        .expect("the brand mark closes its anchor");
    assert!(
        brand.contains("Acme Law"),
        "the header carries the mounted brand's name: {html}"
    );
    assert!(
        !brand.contains("Neon Law"),
        "the rebranded header must not carry this firm's wordmark: {html}"
    );
    let (_, after_h1) = html
        .split_once("<h1")
        .expect("the home page renders its statement");
    let (statement, _) = after_h1
        .split_once("</h1>")
        .expect("the statement closes its heading");
    assert!(
        !statement.contains("Neon Law"),
        "the statement names no firm, so a rebrand cannot leak one: {html}"
    );

    // The public catalog and footer render the bundle's support email.
    let notations = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/notations")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(notations.status(), StatusCode::OK);
    assert!(body_string(notations).await.contains("help@acme.example"));

    // The contact page renders the bundle's support email.
    let contact = app
        .oneshot(
            Request::builder()
                .uri("/contact")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(contact.status(), StatusCode::OK);
    assert!(body_string(contact).await.contains("help@acme.example"));
}

/// The footer closes on the platform line, publishing neither organization's
/// registered address under it.
///
/// The firm's Reno box is already the Nevada tile in the contact band above, so
/// a second copy of the same street, suite, and city at the very bottom told a
/// reader nothing the band had not. It went.
///
/// Asserted here rather than in the component, because the addresses were the
/// firm's real ones out of `views::brand` — a component fixture that simply
/// stops passing them would pass whether or not the row still renders.
/// `405-9999` is the sharpest half: that suffix reached no other surface, so its
/// absence is specific to this row rather than to the page happening not to say
/// it. It is a retired box now — the nonprofit that held it is gone from the
/// seed — which makes the marker no weaker for this purpose and is why the
/// assertion stays. The Nevada office is asserted in the same test so the
/// removal cannot be satisfied by dropping the address the firm does publish.
#[tokio::test]
async fn the_firm_footer_publishes_no_registered_address_row() {
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/navigator").await).await;
    for retired in [
        "site-footer__legal-addresses",
        r#"class="site-footer__legal-address""#,
        "405-9999",
    ] {
        assert!(
            !body.contains(retired),
            "the footer must not carry {retired:?}: {body}"
        );
    }
    assert!(
        body.contains("Ste 405-9002"),
        "the office the band publishes is untouched: {body}"
    );
}

/// No page leaks an unresolved reference. A `{shared:…}` that reached the
/// reader would be a brace where a sentence belongs, and it would render
/// perfectly well in every test that only checks for the words around it.
#[tokio::test]
async fn no_firm_page_publishes_an_unresolved_placeholder() {
    let app = site_app().await;
    for path in ["/", "/navigator"] {
        let body = body_string(anon_get(&app, path).await).await;
        for token in ["{shared:", "{site_name}", "{firm_email}"] {
            assert!(
                !body.contains(token),
                "{path} published the unresolved placeholder {token}"
            );
        }
    }
}
