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

/// The firm host with the bundled workspace documentation loaded.
///
/// The shared builder ships `DocsIndex::empty()`, so `/docs` would 404 on it for
/// want of content rather than for want of a route — which would let an
/// anonymous-access assertion pass against a page that renders nothing.
async fn site_app_with_docs() -> Router {
    let mut state = site_state().await;
    state.docs = portal::docs::loader::bundled();
    site_router(state)
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
        "/fractional-cto",
        "/services",
        "/litigation",
        "/fractional-gc",
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

#[tokio::test]
async fn the_fractional_cto_page_leads_with_the_offering_and_prices_through_contact() {
    // The firm's fractional CTO engagement: it runs the technology function for
    // a law firm. This page now carries the copy the home page used to open on,
    // which moved here when the site began leading with the litigation
    // practice. It quotes through `/contact` — the scope of running a firm's
    // technology is not knowable in advance, so no figure and no turnaround
    // appear.
    let app = site_app().await;
    let resp = anon_get(&app, "/fractional-cto").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("<title>Neon Law | Fractional CTO"),
        "the page titles itself Fractional CTO: {body}"
    );
    // The hero, then the engagements prose the home page used to open on, then
    // the CTA. Asserted as structure: the wording is the firm's to edit.
    assert!(
        body.contains(r#"<h1 class="fm-hero__title""#),
        "the page states its offering in an h1: {body}"
    );
    assert!(
        body.contains(r#"class="firm-eyebrow""#),
        "the hero carries its eyebrow: {body}"
    );
    assert_eq!(
        body.matches(r#"class="fm-band fm-band--statement""#)
            .count(),
        1,
        "one statement band, carrying the engagements prose: {body}"
    );
    // The band's opening line is a paragraph inside the card rather than a
    // large lead above it, so the whole of the prose reads at one size.
    assert!(
        !body.contains(r#"class="fm-statement__lead""#),
        "the statement carries no large lead above its card: {body}"
    );
    assert_eq!(
        body.matches(r#"class="fm-statement__body""#).count(),
        1,
        "the prose sits in the band's body: {body}"
    );
    // The hero carries the practice-page header: the accent statement, the lead
    // under it, and one call to action.
    assert!(
        body.contains(r#"class="fm-word fm-word--accent""#),
        "the hero statement sets its opening words in the firm's colour: {body}"
    );
    assert!(
        body.contains(r#"class="fm-hero__lead""#),
        "the hero carries a lead: {body}"
    );
    assert!(
        body.contains(
            r#"class="nav-btn nav-btn--primary fm-hero__cta" href="mailto:contact@neonlaw.com""#
        ),
        "the hero carries one call to action: {body}"
    );
    // The prose names Navigator from inside a sentence rather than repeating
    // that page here.
    assert!(
        body.contains(r#"href="/navigator""#),
        "the prose links the platform inline: {body}"
    );
    assert!(
        body.contains("mailto:"),
        "the page quotes through a contact CTA: {body}"
    );
    // The cards band came off the page when the home copy moved onto it.
    assert!(
        !body.contains(r#"class="fm-cards"#),
        "no cards band renders: {body}"
    );
}

/// `/fractional-cto` and `/services` wear the practice-page header.
///
/// The same five parts `/litigation` opens on: the ringed mark, the eyebrow
/// above the statement, the statement with its opening words in the firm's own
/// colour, the lead under it, and one call to action. Asserted as structure on
/// both pages at once — the wording is the firm's to edit, the shape is the
/// thing that has to match across the practice pages.
#[tokio::test]
async fn the_practice_pages_wear_the_same_header() {
    let app = site_app().await;
    for path in ["/fractional-cto", "/services"] {
        let body = body_string(anon_get(&app, path).await).await;
        for part in [
            r#"class="fm-hero fm-hero--page""#,
            r#"class="fm-hero__mark"#,
            r#"class="firm-eyebrow""#,
            r#"<h1 class="fm-hero__title""#,
            r#"class="fm-hero__line""#,
            r#"class="fm-word fm-word--accent""#,
            r#"class="fm-hero__lead""#,
            r#"class="nav-btn nav-btn--primary fm-hero__cta" href="mailto:contact@neonlaw.com""#,
        ] {
            assert!(
                body.contains(part),
                "{path} carries {part} in its header: {body}"
            );
        }
        // The practice skin is what the header's typography keys off, so a page
        // that lost the modifier would render the parts unstyled.
        assert!(
            body.contains("fm-page--practice"),
            "{path} wears the practice skin: {body}"
        );
        // The accent run is the opening of the statement, not the whole of it:
        // the practice name is in brand and the claim after it is in text.
        // Matched on the closing quote rather than the tag's `>`: SSR writes
        // hydration attributes after the class, and the accented form is
        // `class="fm-word fm-word--accent"`, so the quote is what tells a plain
        // word from an accented one.
        assert!(
            body.contains(r#"class="fm-word""#),
            "{path} keeps unaccented words after the accent run: {body}"
        );
        // The statement sets its own break rather than taking the viewport's:
        // both practice pages read as two lines.
        assert_eq!(
            body.matches(r#"class="fm-hero__line""#).count(),
            2,
            "{path} breaks its statement into two lines: {body}"
        );
    }
}

#[tokio::test]
async fn site_host_serves_the_legal_services_page() {
    // The firm's Legal Services page: the flat-fee schedule of one-time
    // consumer matters, each scoped, with a numbered process and a licensed
    // attorney's review before filing. A single page, not a `/services/*`
    // catalog.
    let app = site_app().await;
    let resp = anon_get(&app, "/services").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("<title>Neon Law | Services"),
        "the page titles itself Legal Services: {body}"
    );
    assert!(
        body.contains(r#"<h1 class="fm-hero__title""#),
        "the page states its offering in an h1: {body}"
    );
    // Classless on purpose: `theme.css` cues inline prose links through
    // `.nav-theme :is(p, li) > a:not([class])`, so a class here would leave this
    // link distinguishable by colour alone (axe `link-in-text-block`, which is
    // how this page failed the public accessibility gate).
    assert!(
        body.contains(r#"<a href="/fractional-gc""#),
        "business filings link to the fractional GC page, with no class: {body}"
    );
    assert!(
        body.contains("Our process is designed with speed in mind"),
        "the page carries the numbered process: {body}"
    );
    for step in [
        "Create an account",
        "Answer some questions",
        "Upload your documentation",
    ] {
        assert!(
            body.contains(step),
            "the page names the step {step:?}: {body}"
        );
    }
    assert!(
        body.contains("A licensed attorney reviews it"),
        "the attorney-review promise before filing: {body}"
    );
    assert!(
        body.contains("Ready to get started?") && body.contains("mailto:"),
        "the page has a contact CTA to get started: {body}"
    );
}

#[tokio::test]
async fn litigation_is_the_statement_and_the_filed_paragraphs() {
    // The page is the firm's own filed copy: a statement, who the firm
    // represents, the breadth of what it takes on, and how a matter runs. It
    // arrived here by subtraction — a Rule 23 explainer, six
    // certification-element cards, a chip strip, a phase rail, an authority
    // strip, and a fee section all came off — so this asserts what is on it and
    // the next test asserts what is not.
    let app = site_app().await;
    let resp = anon_get(&app, "/litigation").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("<title>Neon Law | Litigation</title>"));
    assert!(body.contains("built"), "the statement: {body}");
    assert!(body.contains("speed."), "the statement: {body}");
    assert!(
        body.contains("Values-Based Litigation"),
        "the eyebrow: {body}"
    );
    let seen = body
        .find("We represent those who haven\u{2019}t been justly seen")
        .expect("the who-we-represent paragraph");
    let breadth = body
        .find("The disputes we take are specific")
        .expect("the inventory paragraph");
    assert!(seen < breadth, "in the filed order: {body}");
    // The matter types the firm names, which is the part an edit shortens
    // first. They are the whole reason a reader can tell whether this is their
    // practice: "those who haven't been justly seen" is a stance, and these are
    // what it means in cases. Categories only, never a matter. The page names
    // the inventory; it does not say the firm will take anything.
    for named in [
        "trademark and copyright disputes",
        "prison rights litigation",
        "restraining orders",
        "domestic violence",
    ] {
        assert!(
            body.contains(named),
            "the filed copy keeps {named:?}: {body}"
        );
    }
    assert!(
        !body.contains("There is little we will not take on"),
        "the page does not hold itself out as unlimited: {body}"
    );
    // The conflicts caveat is the one qualifier on an otherwise open door, and
    // it is a real check rather than a hedge: `store::conflicts` runs a bounded
    // multi-hop traversal before the firm can take a matter.
    assert!(
        body.contains("as long as we are not conflicted out"),
        "the page states the conflicts caveat: {body}"
    );
    // The third paragraph: how a matter runs here, after who the firm
    // represents. It sits last because a reader decides whether this is their
    // practice before they care how the file is kept.
    let system = body
        .find("All litigation cases run on")
        .expect("the case-system paragraph");
    assert!(
        breadth < system,
        "how the work runs comes after who the firm represents: {body}"
    );
    // It is the one paragraph on the page that links, and the link is the
    // reason the body carries runs rather than plain strings. A copy edit that
    // flattens the runs loses it silently, because the sentence still reads
    // correctly with "Neon Law Navigator" as bare text.
    assert!(
        body.contains(r#"href="/navigator""#),
        "the Navigator mention links its page: {body}"
    );
    // The events the paragraph names. This is what makes it a description
    // rather than a slogan — an edit that trims the list back to "agentic
    // workflows" leaves an adjective and nothing a reader can check.
    for event in ["a new court docket filing", "letter", "new research"] {
        assert!(
            body.contains(event),
            "the paragraph names the {event:?} event: {body}"
        );
    }
}

/// The page describes *how* the work runs and never *how much* that saves.
///
/// The distinction is the whole reason this paragraph is publishable. The
/// mechanism — events in, work started, groundwork laid before the deadline
/// rather than after it — is open source in this workspace and stays true. A
/// number attached to it is a result: it goes stale against the next matter, it
/// invites the reading that the firm promises the same saving to this reader,
/// and on an attorney-advertising page that is a bar problem rather than a copy
/// preference. `publishes_no_currency_amount` guards the fee half of this;
/// this guards the efficiency half.
#[tokio::test]
async fn litigation_publishes_no_quantified_efficiency_claim() {
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/litigation").await).await;
    let lowered = body.to_lowercase();
    for banned in [
        "substantially saves",
        "substantially less",
        "substantially reduce",
        "half the time",
        "half the cost",
        "cuts your",
        "faster than",
        "more efficient than",
        "saves you hours",
        "hours saved",
    ] {
        assert!(
            !lowered.contains(banned),
            "the litigation page must publish no quantified efficiency claim \
             ({banned:?}): {body}"
        );
    }
}

/// Speed is stated as method, never as outcome.
///
/// This is the load-bearing line of the reframe. "Litigation built
/// for speed" is a claim a reader can hear as a promise about how fast their
/// own case ends, and on an attorney-advertising page that reading is a bar
/// problem rather than a copy preference. What makes the heading publishable is
/// the paragraph that says so outright, so the disclaimer of it is guarded
/// here: delete the sentence and the heading stops being defensible.
#[tokio::test]
async fn litigation_states_speed_as_method_and_not_as_outcome() {
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/litigation").await).await;
    assert!(
        body.contains("It is not a promise about your result"),
        "the page disclaims speed as an outcome: {body}"
    );
    // The firm turns work away because of how it works, and says so. This is
    // the sentence that makes the speed claim credible rather than salesy, and
    // it is the first thing a later copy edit would smooth off.
    assert!(
        body.contains("we will not be everyone\u{2019}s lawyer"),
        "the page says who it is not for: {body}"
    );
}

/// The litigation page publishes no em dash.
///
/// The firm's style call for this page: it reads as terse and direct, and an em
/// dash is the punctuation that turns two short sentences into one long one. A
/// guard rather than a habit because the resolver's own doc comments are full
/// of them, so a sentence moved from a comment into the copy carries one in
/// silently.
#[tokio::test]
async fn litigation_publishes_no_em_dash() {
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/litigation").await).await;
    assert!(
        !body.contains('\u{2014}'),
        "the litigation page must publish no em dash: {body}"
    );
}

/// The paragraph claims only capabilities this workspace carries.
///
/// Each needle below is a capability the copy asserts, matched to the module
/// that implements it. The guard is the *pairing*: if a future edit deletes the
/// engine or the graph, this test fails and the sentence on the public page has
/// to come down with it, rather than quietly becoming marketing for something
/// the firm no longer runs.
#[tokio::test]
async fn litigation_claims_only_capabilities_the_workspace_carries() {
    let engine = include_str!("../../workflows-service/src/main.rs");
    assert!(
        engine.contains("Endpoint::builder"),
        "the copy says event-driven workflows start the work an event implies; \
         the Restate worker that runs them must still exist"
    );
    let relationships = include_str!("../../store/src/relationship_logs.rs");
    assert!(
        relationships.contains("relationship_log"),
        "the copy says the matter is a graph whose relationships are logged as \
         the case moves; that log must still exist"
    );

    let app = site_app().await;
    let body = body_string(anon_get(&app, "/litigation").await).await;
    assert!(
        body.contains("a new court docket filing"),
        "the grounded court-filing event renders: {body}"
    );
    assert!(
        body.contains("event-driven agentic workflows"),
        "the grounded engine claim renders: {body}"
    );
    // What the firm does not run out of this workspace, and so does not say.
    // Daily evidence scraping is the claim this page came closest to
    // publishing; there is no scraper, no docket poller, and no case-reporter
    // client in the tree, so the page must not imply one.
    //
    // The rest of this list is what the speed reframe drafted and had to cut.
    // Each was true of how the firm works and false of what this workspace
    // implements, which is the exact gap this test exists to hold: there is no
    // embedding or vector index anywhere in the tree, so no semantic search and
    // no vendor behind one; there is no fact-extraction module; and
    // `templates/` carries exactly one litigation template (a TRO), so a
    // per-pleading library must not be advertised.
    for unbuilt in [
        "scrapers we run",
        "scrape",
        "every day we",
        "crawl the web",
        "semantic search",
        "midpage",
        "descrybe",
        "regex",
        "extract the facts",
        "motion to dismiss",
        "service of summons",
        "statutory deadline",
    ] {
        assert!(
            !body.to_lowercase().contains(unbuilt),
            "the page must not claim {unbuilt:?}, which the workspace does not \
             implement: {body}"
        );
    }
}

#[tokio::test]
async fn litigation_carries_none_of_the_sections_it_shed() {
    // Each of these was a reasonable-looking addition to a practice page, and
    // together they buried what the firm actually wanted to say. Guarded by the
    // copy that carried them, so re-adding any one fails here.
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/litigation").await).await;
    for gone in [
        "What a class action is",
        "What we litigate",
        "How a case runs",
        "The law this practice runs on",
        "When we are not the right answer",
        "Numerosity",
        "Predominance",
        "Case assessment",
        "uscode.house.gov",
        "uscourts.gov",
        "zeal-rail",
        "zeal-area",
        "zeal-figure",
    ] {
        assert!(!body.contains(gone), "{gone} is gone: {body}");
    }
}

#[tokio::test]
async fn litigation_cites_no_decided_case() {
    // Decided cases came off the strip deliberately. A firm page that lists
    // opinions invites two readings it cannot control — that the firm litigated
    // them, or that it is characterizing holdings a reader will rely on — so
    // the strip carries enacted text only. This guards the shape, not one
    // citation: no reporter cite, and no link to a case reporter.
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/litigation").await).await;
    assert!(
        !body.contains("courtlistener.com"),
        "the page must link no case reporter: {body}"
    );
    // Reporter volumes rather than party names: the hero says the firm
    // litigates "on both sides of the v.", so a bare " v. " needle would match
    // ordinary prose.
    for reporter in [
        "U.S. 413",
        "U.S. 338",
        "U.S. 591",
        "U.S. 27 (",
        "U.S. 442",
        "F.3d ",
    ] {
        assert!(
            !body.contains(reporter),
            "the page must publish no case citation ({reporter:?}): {body}"
        );
    }
}

#[tokio::test]
async fn litigation_carries_the_regulated_copy_and_no_results_promise() {
    // The disclaimer is the page's one piece of regulated copy: it is required
    // wherever a reader could infer a result, so dropping it is a bar problem
    // rather than a design change. Everything else on the page is the firm's own
    // filed copy, and the sections that came off are guarded above.
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/litigation").await).await;
    // The notice reaches the reader once, through the shared footer, and it
    // opens by naming itself an advertisement. The page carries no second copy
    // of its own: what the rule requires is that the reader sees it, not that
    // a given page repeats it.
    assert!(
        body.contains("Attorney advertisement."),
        "the footer labels the page an attorney advertisement: {body}"
    );
    assert!(
        body.contains("Nothing here is legal advice without a signed retainer"),
        "the no-advice line: {body}"
    );
    assert!(
        body.contains("Past results do not guarantee future outcomes."),
        "the past-results line: {body}"
    );
    assert!(
        !body.contains("zeal-disclaimer"),
        "the page-level duplicate disclaimer is gone: {body}"
    );
    for banned in [
        "we will win",
        "guaranteed recovery",
        "best litigators",
        "world-class",
        "industry-leading",
    ] {
        assert!(
            !body.to_lowercase().contains(banned),
            "the litigation page must not publish {banned:?}: {body}"
        );
    }
}

#[tokio::test]
async fn transactional_names_the_fee_structure_and_prices_through_contact() {
    // The firm's public site publishes no fee. The page says how the fee works
    // — one flat monthly amount, contracts metered on top — and sends every
    // number to `/contact`, so no page can go stale against what the firm
    // charges or read as a binding offer.
    let app = site_app().await;
    let resp = anon_get(&app, "/fractional-gc").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("<title>Neon Law | Fractional GC</title>"));
    assert!(
        body.contains("Accurate.") && body.contains("Efficient.") && body.contains("Speedy."),
        "the statement: {body}"
    );
    assert!(
        body.contains("One flat monthly fee"),
        "the structure: {body}"
    );
    // The engagement-letter block came off the page: what it said is a term of
    // the engagement, not something the marketing surface has to close on.
    assert!(
        !body.contains("Engagement letter governs"),
        "the engagement-letter block is gone: {body}"
    );
}

#[tokio::test]
async fn neither_practice_page_publishes_a_currency_amount() {
    // One guard for the rule behind both pages: the firm describes how a fee
    // works and quotes the number itself per engagement. A `$` in the page body
    // is the whole failure mode — a rate, a retainer, or an illustrative figure
    // a reader would take for a price.
    let app = site_app().await;
    for path in ["/litigation", "/fractional-gc"] {
        let body = body_string(anon_get(&app, path).await).await;
        // Measured over the page body: the shared header and footer chrome is
        // not this page's copy, and a guard that swept them would fail for a
        // reason no edit here could fix.
        let main = body
            .split_once("public-shell__main")
            .and_then(|(_, rest)| rest.split_once("site-footer"))
            .map_or(body.as_str(), |(page, _)| page);
        assert!(
            !main.contains('$'),
            "{path} must publish no currency amount: {main}"
        );
    }
}

#[tokio::test]
async fn transactional_states_its_turnaround_in_prose() {
    // The turnaround dial came off — it drew the published figure as a ring with
    // a qualifier beneath, stating in a graphic what the Speedy virtue states in
    // a sentence. The commitment itself stays, in prose, and so does the scope
    // that makes it honest: the firm controls its own turnaround and controls
    // nothing about the counterparty or the deal.
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/fractional-gc").await).await;
    assert!(
        body.contains("one business day on a redline"),
        "the commitment survives as prose: {body}"
    );
    for gone in ["speed-dial", "Measured from a complete intake"] {
        assert!(!body.contains(gone), "the dial is gone ({gone}): {body}");
    }
    // The page uses "MSA", so the page defines it.
    assert!(
        body.contains("master services agreement"),
        "the term is spelled out where it is used: {body}"
    );
    // Financings and litigation are quoted separately, and the litigation
    // half routes to the practice page rather than dead-ending.
    assert!(body.contains("Financings"), "separate work: {body}");
    assert!(
        body.contains(r#"href="/litigation""#),
        "the litigation cross-link: {body}"
    );
}

#[tokio::test]
async fn both_practice_pages_hoist_their_own_stylesheet() {
    // Each page carries its own animation layer after the brand layer. A page
    // that lost the link would still render — and silently lose every piece of
    // motion the copy is built around.
    let app = site_app().await;
    for (path, sheet) in [
        ("/litigation", "/public/css/litigation.css"),
        ("/fractional-gc", "/public/css/transactional.css"),
    ] {
        let body = body_string(anon_get(&app, path).await).await;
        assert!(
            body.contains("/public/css/brand-firm.css"),
            "{path} hoists the brand layer: {body}"
        );
        assert!(body.contains(sheet), "{path} hoists {sheet}: {body}");
    }
}

#[tokio::test]
async fn the_firm_nav_leads_with_the_lead_offering_then_the_practices() {
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/").await).await;
    // The header carries the lead offering and the three practices. Sliced to
    // the `<ul>` because these hrefs also appear elsewhere on the page (the
    // footer row, the statement CTA), and it is the header list this test
    // speaks for.
    let header = body
        .split_once(r#"class="site-header__links""#)
        .and_then(|(_, rest)| rest.split_once("</ul>"))
        .map(|(links, _)| links)
        .expect("the header renders its link list");
    for href in [
        r#"href="/fractional-cto""#,
        r#"href="/services""#,
        r#"href="/litigation""#,
        r#"href="/fractional-gc""#,
    ] {
        assert!(
            header.contains(href),
            "the header nav carries {href}: {header}"
        );
    }
    // `/team` publishes no page, so a header entry would link a `404`.
    assert!(
        !header.contains(r#"href="/team""#),
        "the team page does not exist, so the header must not link it: {header}"
    );
    // Litigation leads, then the three engagements beside it. The ordering is a
    // product decision and is asserted rather than left to the array literal:
    // the firm leads with the practice the home page opens on.
    let litigation = header
        .find(r#"href="/litigation""#)
        .expect("Litigation is in the nav");
    let cto = header
        .find(r#"href="/fractional-cto""#)
        .expect("Fractional CTO is in the nav");
    let transactional = header
        .find(r#"href="/fractional-gc""#)
        .expect("Fractional GC is in the nav");
    let services = header
        .find(r#"href="/services""#)
        .expect("Legal Services is in the nav");
    assert!(
        litigation < cto && cto < transactional && transactional < services,
        "the lead practice leads, then the two quoted engagements, then the schedule: {header}"
    );
    assert_eq!(
        header.matches("<li").count(),
        4,
        "the header is the lead practice and the three engagements, and nothing \
         else: {header}"
    );
    assert!(
        !header.contains(r#"href="/foundation""#),
        "no header entry reaches a path the site does not publish: {header}"
    );
}

#[tokio::test]
async fn the_footer_carries_the_pages_the_header_does_not() {
    // All ten routes are one click away from every public page. Checked on
    // `/litigation` rather than `/`, because the footer is shared chrome and a
    // page that is not the home page proves it renders everywhere.
    //
    // Workshops joined the row when the classes became public, and Docs when the
    // workspace documentation did. While either was gated neither row carried
    // it, so that the site never sent a signed-out reader at a login door; now
    // that anyone may read them, these links are what stop each being reachable
    // only by typing the URL.
    //
    // `/privacy` and `/terms` ride the row on the same footing as the rest.
    // UX is the one entry that links off-site, to the platform's design
    // showcase, rather than to a path this host serves. `/api` and `/team`
    // joined when the Swagger explorer alias and the firm's roster published.
    const ROW: [&str; 12] = [
        "/api",
        "/blog",
        "/contact",
        "/docs",
        "/navigator",
        "/notations",
        "/presentations",
        "/privacy",
        "/team",
        "/terms",
        "https://neon-law-source-code.github.io/navigator-ux/",
        "/workshops",
    ];
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/litigation").await).await;
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
    // `views::brand::FIRM_ATTORNEYS` is empty today; `/team` names a contact
    // card per attorney, not a bar-credential disclosure.
    //
    // Both halves are the assertion. A bar number reappearing means
    // `views::brand::FIRM_ATTORNEYS` was refilled; an office note reappearing
    // means an address is being published with a qualification on it. Checked
    // on `/litigation` because the footer is shared chrome.
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/litigation").await).await;
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
    let full_body = body_string(anon_get(&app, "/litigation").await).await;
    let body = full_body
        .split(r#"<ul class="site-footer__offices""#)
        .nth(1)
        .and_then(|rest| rest.split("</ul>").next())
        .expect("the offices grid renders");
    for (street, unit, city) in [
        ("5150 Mae Anne Ave", "Ste 405-9002", "Reno, NV 89523"),
        ("12 E 49th St", "18th Floor", "New York, NY 10017"),
        ("720 Seneca St", "Ste 107-715", "Seattle, WA 98101"),
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
        9,
        "three lines for each of the firm's three offices: {body}"
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
    for path in ["/", "/litigation", "/fractional-gc"] {
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
async fn the_sitemap_advertises_both_practice_pages() {
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/sitemap.xml").await).await;
    // `/navigator` is a public marketing page like the two practice pages, so
    // it is discoverable like one. A page reachable only from a footer link is
    // a page search engines never find.
    for path in ["/services", "/litigation", "/fractional-gc", "/navigator"] {
        assert!(
            body.contains(path),
            "the sitemap must advertise {path}: {body}"
        );
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

/// No page on this host publishes a fee. Any fee.
///
/// The firm quotes engagements through `/contact` and bills each matter on its
/// own invoice, because the work is bespoke and a posted number would fit
/// nobody — the routine, one-time work at `/services` included. `/contact` is
/// here too because it once named a consultation fee. A fee added to any page
/// here fails rather than ships.
#[tokio::test]
async fn no_firm_page_publishes_a_fee() {
    let app = site_app().await;

    for unpriced in [
        "/",
        "/services",
        "/notations",
        "/contact",
        "/litigation",
        "/fractional-gc",
        "/navigator",
        "/blog",
        "/privacy",
        "/terms",
    ] {
        let body = body_string(anon_get(&app, unpriced).await).await;
        assert!(
            !publishes_a_fee(&body),
            "{unpriced} must publish no fee — the firm posts no rate anywhere: {body}"
        );
    }
}

/// `/llms.txt` publishes no fee either.
///
/// The machine-readable index is the other place the firm could leak a number.
/// It lists the Legal Services page, but never with a figure — exactly as the
/// pages do.
#[tokio::test]
async fn the_llms_index_publishes_no_fee() {
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/llms.txt").await).await;

    assert!(
        body.contains("/services"),
        "the index lists the Legal Services page: {body}"
    );

    // No figure a machine reads here, exactly as on the pages.
    let amounts = currency_amounts(&body);
    assert!(
        amounts.is_empty(),
        "the index publishes no figure: {amounts:?}"
    );
}

/// The platform page is the firm's, and it makes one invitation.
///
/// The firm builds Navigator, and the page invites the reader to co-counsel a
/// pro bono case with the firm. Two things must survive on the rendered page:
/// the co-counsel invitation, and the absence of any published rate.
#[tokio::test]
async fn the_navigator_page_invites_pro_bono_co_counsel_and_publishes_no_rate() {
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/navigator").await).await;
    assert!(
        body.contains("Co-Counsel a Pro Bono Case with Us"),
        "the only invitation is pro bono co-counsel: {body}"
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

    // The Homebrew route, and the reason it is the recommended one on a Mac.
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
    assert!(
        body.contains("not yet signed or notarized"),
        "the page says why brew is the macOS route rather than implying the \
         browser download just works: {body}"
    );

    for href in [
        "/docs/validate",
        "https://github.com/neon-law-source-code/navigator/blob/main/docs/gitops.md",
        "/docs/oss-install",
        "/workshops",
    ] {
        assert!(
            body.contains(&format!(r#"href="{href}""#)),
            "the read-next band links {href}: {body}"
        );
    }
}

/// The workspace documentation reads for a visitor with no account.
///
/// It sat behind the session boundary while the source was closed. The
/// repository is source-available now, so a login door stood in front of the one
/// document that explains how to run software anyone can clone. This asserts the
/// hub, one document beneath it, and the `/docs/{slug}` redirect all answer a
/// browser that has never signed in — a `303` to `/auth/login` is the failure.
#[tokio::test]
async fn the_workspace_documentation_reads_anonymously() {
    let app = site_app_with_docs().await;

    for path in ["/docs", "/docs/glossary"] {
        let response = anon_get(&app, path).await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{path} renders for a reader with no account"
        );
    }

    // The canonicalizing redirect is the pre-layer's, not the login door's.
    let response = anon_get(&app, "/docs/index").await;
    assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(
        response.headers().get("location").unwrap(),
        "/docs",
        "an anonymous reader gets the canonical URL, not a login redirect"
    );

    // `/app/docs` is untouched. It is a second door to the same index wearing
    // the application chrome, and what it gates is that surface.
    assert_eq!(
        anon_get(&app, "/app/docs").await.status(),
        StatusCode::SEE_OTHER,
        "the in-application documentation surface stays behind the boundary"
    );
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
async fn home_points_at_the_three_practices_from_its_foot() {
    // The page leads with one offering, and these three boxes say the firm
    // has work for visitors who did not come for a dispute. Each links the
    // page that explains it, so a reader who came for counsel, a technology
    // function, or a filing is one click from that page rather than reading
    // the lead and leaving.
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/").await).await;
    // The section renders, with its heading wired to the copy rather than
    // hard-coded in the view.
    assert!(
        body.contains(r#"aria-labelledby="home-practices-heading""#),
        "the section labels itself by its own heading: {body}"
    );
    assert!(
        body.contains(r#"id="home-practices-heading""#),
        "the heading renders: {body}"
    );
    // Which pages the boxes reach is routing, not copy. Litigation is
    // deliberately *not* among them: it is the page's lead and its close, so a
    // fourth box would put the practice the page is built around into a row of
    // alternatives.
    for href in ["/fractional-cto", "/fractional-gc", "/services"] {
        assert!(
            body.contains(&format!(
                r#"<a class="neon-card home-practice" href="{href}""#
            )),
            "the box for {href} is itself the link: {body}"
        );
    }
    assert!(
        !body.contains(r#"<a class="neon-card home-practice" href="/litigation""#),
        "litigation is the lead, not one of the boxes: {body}"
    );
    assert_eq!(
        body.matches(r#"<a class="neon-card home-practice" href="#)
            .count(),
        3,
        "three boxes and no more: {body}"
    );
    // Each box opens on a drawn mark, hidden from assistive technology because
    // the heading beside it already names the practice. Stroked in
    // `currentColor` so it is white on the dark theme — which is why these are
    // line marks rather than emoji: `color` does not reach a colour glyph.
    assert_eq!(
        body.matches(r#"class="home-practice__mark""#).count(),
        3,
        "one mark per box: {body}"
    );
    assert!(
        body.contains(r#"stroke="currentColor""#),
        "the marks take the card's colour: {body}"
    );
    // The whole box is the link now, so the separate "read more" anchor each
    // box used to end in must not render: it was a second thing to click.
    assert!(
        !body.contains("home-practice__link"),
        "no second anchor inside a box: {body}"
    );
    // The boxes sit under the litigation prose, not above it: above, they would
    // read as the page offering four things rather than leading with one.
    let service = body.find("home-service").expect("the litigation section");
    let practices = body.find("home-practices").expect("the practice boxes");
    assert!(service < practices, "prose then boxes: {body}");
}

#[tokio::test]
async fn home_opens_on_the_new_york_photograph_and_practice_statement() {
    // The supplied skyline is the public home-page hero. The statement still
    // follows it and remains the page's only h1.
    let app = site_app().await;
    let body = body_string(anon_get(&app, "/").await).await;

    assert!(
        body.contains(r#"<h1 class="home-statement__heading""#),
        "the statement is the first thing on the page: {body}"
    );
    assert!(
        body.contains("new-york.png"),
        "the home page ships the skyline: {body}"
    );
    assert!(
        body.contains("New York City skyline at sunset"),
        "the hero has accurate alt text: {body}"
    );
    assert_eq!(
        body.matches("<h1").count(),
        1,
        "the page keeps exactly one h1: {body}"
    );
}

#[tokio::test]
async fn home_renders_the_statement_and_the_practice_prose() {
    // The shape of the page, not its wording. The copy lives in
    // `neon::firm_pages::resolve_firm_home_content` and is the firm's to edit;
    // what this guards is that every part of it reaches the reader, in order,
    // and that the retired surfaces do not come back with it.
    let app = site_app().await;
    let resp = anon_get(&app, "/").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;

    // The statement: one heading, one lead, one call to action, and the CTA
    // goes to `/contact` because every engagement is quoted there.
    for rendered in [
        r#"class="home-statement__heading""#,
        r#"class="home-statement__lead""#,
        r#"class="nav-btn nav-btn--primary home-statement__cta" href="mailto:contact@neonlaw.com""#,
    ] {
        assert!(body.contains(rendered), "{rendered} renders: {body}");
    }
    assert!(body.contains("<title>Neon Law | Home</title>"));
    assert!(
        body.contains("Everyone deserves to be seen."),
        "the statement is the firm's tagline: {body}"
    );
    assert!(
        body.contains("We are litigators."),
        "the lead names the practice: {body}"
    );
    // Causes of action belong on `/litigation`. Listing them in the home hero
    // is what made the page read as four firms at once.
    for retired in [
        "Personal injury",
        "criminal investigations",
        "business divorce",
        "Every problem is unique",
        "Whatever brings you in",
        "We are by your side through tough times",
        "Our complementary practice",
    ] {
        assert!(
            !body.contains(retired),
            "the home page must not publish {retired:?}: {body}"
        );
    }

    // The practice prose: one card of paragraphs under one heading. A card
    // *per* practice area is the shape this page sheds, so the count is what
    // the guard is for rather than the text inside it.
    assert_eq!(
        body.matches(r#"class="neon-card home-service""#).count(),
        1,
        "exactly one prose card: {body}"
    );
    assert!(
        body.contains(r#"aria-labelledby="home-service-heading""#),
        "the section labels itself by its own heading: {body}"
    );
    assert!(
        body.matches(r#"class="home-service__paragraph""#).count() > 1,
        "the practice is stated in paragraphs: {body}"
    );
    // One paragraph links the litigation practice from inside the sentence,
    // which is what `CopyRun::href` exists for: the method is stated there
    // rather than restated here.
    assert!(
        body.contains(r#"class="home-service__link" href="/litigation""#),
        "the prose links the litigation practice inline: {body}"
    );

    // The retired home-page surfaces. Each is markup rather than wording: a
    // practice grid, a per-practice card, a chip list, and the glow whose wash
    // bled past the hero's edge into the page margin.
    for retired in [
        r#"class="practice-grid""#,
        r#"class="practice__heading""#,
        r#"class="litigation__heading""#,
        r#"class="firm-chip""#,
        "home-service__commitment",
        // The numbered 1-2-3 and the closing band came off the page. Guarded so
        // the markup does not come back empty with the next copy edit.
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

    // The page's sections, in the order the page argues in: the statement, what
    // leading with litigation means, and the engagements beside it.
    let at = |needle: &str| {
        body.find(needle)
            .unwrap_or_else(|| panic!("{needle}: {body}"))
    };
    let order = [
        at("home-statement"),
        at("home-service"),
        at("home-practices"),
    ];
    assert!(
        order.windows(2).all(|pair| pair[0] < pair[1]),
        "statement, then prose, then boxes: {body}"
    );

    // The shared chrome survives — header nav and the legal footer.
    assert!(body.contains("site-header"), "public header chrome");
    assert!(body.contains("site-footer__legal"), "public legal footer");
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
    for heading in [
        "Fractional CTO",
        "Litigation",
        "Fractional GC",
        "One-time services",
    ] {
        assert!(slides.contains(heading), "missing {heading}: {slides}");
    }
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

    for path in ["/", "/litigation", "/presentations"] {
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

    for path in ["/", "/litigation", "/presentations"] {
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
        "/workshops",
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
    let index = body_string(role_get(&app, "/workshops", lawyer).await).await;
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
        anon_get(&app, "/health").await.status(),
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

// ---- Notations catalog (same hero as workshops and presentations) ----

#[tokio::test]
async fn notations_page_uses_the_catalog_hero_and_links_the_letters_and_forms() {
    let app = site_app().await;
    let resp = anon_get(&app, "/notations").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("<title>Neon Law | Notations</title>"),
        "brand-prefixed document title: {body}"
    );
    assert!(
        body.contains(r#"class="catalog-hero""#),
        "catalog hero: {body}"
    );
    assert!(body.contains(">Notations<"), "page heading: {body}");
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
        body.contains(r#"href="mailto:contact@neonlaw.com""#),
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

/// The `/team` surface is live: the index and one profile per attorney,
/// `/team/nick` and `/team/jask`. A slug naming nobody on the roster still
/// answers `404` rather than a stray redirect — a crawler or a bookmark
/// holding a typo must not land on a page that claims to be someone.
///
/// `/app/team` is a different surface and is deliberately NOT checked here. It
/// is the authenticated matter-side roster inside the portal, and conflating
/// the two is how a working page gets deleted next.
#[tokio::test]
async fn the_team_surface_publishes_the_index_and_each_attorneys_profile() {
    let app = site_router(site_state().await);
    for path in ["/team", "/team/nick", "/team/jask"] {
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{path} is published");
    }
    for path in ["/team/jacob", "/team/nobody"] {
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "{path} names nobody on the roster and must not redirect anywhere"
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
        .split_once("home-statement__heading")
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
    let body = body_string(anon_get(&app, "/litigation").await).await;
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
