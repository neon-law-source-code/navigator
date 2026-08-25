#![allow(clippy::doc_markdown)]
//! `/privacy` and `/terms` on **both** hosts, composed exactly as their binaries
//! compose them.
//!
//! The two pages render through Dioxus since #956 Phase 4, which moved them out
//! of the `host_crawler_and_legal_routes` table and into a Dioxus router pair.
//! A host's public surface has two halves, and assembling only one of them here
//! is how a binary ships serving nothing: these tests go through
//! `<host>::public_routes()` + `<host>::public_dioxus_routers()`, the same pair
//! each `main` hands to the run loop.
//!
//! The copy is per-deployment: the site builds its pair from
//! `neon/content/*.md`, so a white-label tenant can diverge without editing
//! this one. Both carry the text-messaging (SMS) program disclosures.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use store::test_support::mem_surreal;
use tower::ServiceExt;

async fn state() -> portal::AppState {
    portal::test_support::app_state(mem_surreal().await).await
}

fn compose(
    state: portal::AppState,
    public: Router<portal::AppState>,
    public_paths: &'static [&'static str],
    dioxus: Vec<Router>,
) -> Router {
    portal::bootstrap(
        state,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
        public,
        public_paths,
        dioxus,
    )
    .expect("brand public routes must not collide with Navigator")
}

/// The site, composed exactly as the binary composes it.
async fn app() -> Router {
    let state = state().await;
    let dioxus = neon::public_dioxus_routers(&state);
    compose(state, neon::public_routes(), neon::PUBLIC_PATHS, dioxus)
}

async fn get(app: &Router, path: &str) -> (StatusCode, String) {
    let resp = app
        .clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
async fn the_firm_host_serves_both_legal_documents() {
    let app = app().await;

    let (status, html) = get(&app, "/privacy").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("<title>Neon Law | Privacy Policy</title>"),
        "{html}"
    );
    // A section only the real `neon/content/privacy.md` carries, so the body is
    // the compiled-in document and not an empty shell.
    assert!(html.contains("Donor Privacy"), "{html}");

    let (status, html) = get(&app, "/terms").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("<title>Neon Law | Terms of Service</title>"),
        "{html}"
    );
    assert!(html.contains("No Legal Advice"), "{html}");
}

#[tokio::test]
async fn the_privacy_policy_keeps_its_access_to_justice_commitments() {
    // Privacy is a fundamental right here: the policy commits to deletion and
    // CCPA/GDPR/NRS-603A rights for everyone, not only where the law strictly
    // compels it. The page asserted this; the commitment outlives the
    // renderer.
    let app = app().await;
    let (_, html) = get(&app, "/privacy").await;
    let collapsed = html.split_whitespace().collect::<Vec<_>>().join(" ");
    for commitment in ["right to delete", "CCPA", "GDPR", "NRS 603A"] {
        assert!(collapsed.contains(commitment), "missing {commitment}");
    }
    assert!(collapsed.contains("mailto:support@neonlaw.org"), "{html}");
}

/// The footer and the document body must name the **same** entity.
///
/// They are on one screen: `LegalPage` renders the body and `PublicFooter`
/// together, so a reader deciding who they are about to hire sees both claims at
/// once. They disagreed — the footer resolved the firm's legal person from
/// `views::brand` while the Markdown hardcoded a different name — which on a law
/// firm's engagement page is a misattribution rather than a typo. Both now come
/// from the one brand value, and this asserts they agree rather than asserting
/// either name on its own, so a rename cannot re-open the gap.
#[tokio::test]
async fn the_terms_and_the_footer_name_the_same_engagement_entity() {
    let entity = views::brand::FIRM_BRAND.legal_entity;
    assert_eq!(
        entity, "Shook Law PLLC",
        "the firm of record is the entity that holds the engagement"
    );

    let app = app().await;
    let (_, html) = get(&app, "/terms").await;
    let collapsed = html.split_whitespace().collect::<Vec<_>>().join(" ");

    // The footer's legal person, which since #145 is the copyright line.
    assert!(
        collapsed.contains(&format!("© 2026 {entity}")),
        "the footer must name {entity}: {collapsed}"
    );
    // The body's engagement sentence — the one that says when the relationship
    // begins, which is what the reader is actually looking for.
    assert!(
        collapsed.contains(&format!("written engagement agreement with {entity}")),
        "the Terms must form the engagement with {entity}: {collapsed}"
    );

    // Neon Law is the mark, and a mark cannot hold a retainer — a client
    // engages a legal person. Both sentences above therefore have to name the
    // entity even though the page is signed with the brand, and this holds the
    // trade name out of the two positions where it would read as the
    // counterparty. That is the misattribution this whole test exists to catch.
    for trade_name_as_counterparty in ["engagement agreement with Neon Law", "© 2026 Neon Law"] {
        assert!(
            !collapsed.contains(trade_name_as_counterparty),
            "the engagement runs through {entity}, but the page says \
             `{trade_name_as_counterparty}`"
        );
    }
}

/// `/terms` names one firm however the reader reached it.
///
/// Every public page shares this document. A reader is being told who they
/// would engage, and that answer cannot depend on the path they came in
/// through: an engagement sentence that varied by entry point would tell some
/// readers they are hiring somebody other than the firm of record.
#[tokio::test]
async fn the_terms_name_one_firm_however_the_reader_arrived() {
    let app = app().await;
    let (_, html) = get(&app, "/terms").await;
    let collapsed = html.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        collapsed.contains("written engagement agreement with Shook Law PLLC"),
        "the engagement runs through the firm of record: {collapsed}"
    );
    assert!(
        !collapsed.contains("engagement agreement with the Neon Law Foundation"),
        "no other organization renders these legal services: {collapsed}"
    );
}

/// Who renders the legal services moved; who owns the software and the mark did
/// not. A rebrand that also rewrote these two sentences would misstate the
/// registrant of a live U.S. registration.
///
/// `/privacy` used to carry this as a *licence* — one organization operating
/// Navigator under licence from the Firm. There is one organization now, so the
/// same fact is stated as ownership rather than as a grant between two parties.
#[tokio::test]
async fn the_pages_keep_the_navigator_ownership_attribution() {
    let app = app().await;

    let (_, terms) = get(&app, "/terms").await;
    let terms = terms.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        terms.contains("registered trademark of Shook Law PLLC"),
        "NEON LAW's registrant must survive the firm rename: {terms}"
    );

    let (_, privacy) = get(&app, "/privacy").await;
    let privacy = privacy.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        privacy.contains("owned and operated by Shook Law PLLC"),
        "Navigator's owner must survive the firm rename: {privacy}"
    );
}

/// Both surfaces that cite the registration point a reader at the register.
///
/// A notice a reader cannot check is the site's own word for who owns the name
/// on its door. The registration number is the claim and the USPTO record is
/// where it resolves, so the footer that carries the mark on every page and the
/// Terms section that explains it both link out — and the whole route is what
/// proves it, since the footer's notice is resolved from the brand per request
/// rather than written into the page.
#[tokio::test]
async fn the_registration_links_to_the_uspto_record_on_the_page_and_in_the_terms() {
    const RECORD: &str = "https://tmsearch.uspto.gov/search/search-results/90039224";

    let app = app().await;
    let (_, terms) = get(&app, "/terms").await;
    let flat = terms.split_whitespace().collect::<Vec<_>>().join(" ");

    // The footer renders on every page, so this one response carries both.
    assert!(
        flat.contains("is a registered trademark of Shook Law PLLC, "),
        "the footer notices the mark and names the registrant: {flat}"
    );
    assert!(
        flat.contains("U.S. Reg. No. 6,325,650"),
        "and cites the registration: {flat}"
    );
    assert_eq!(
        terms.matches(RECORD).count(),
        2,
        "the footer notice and the Terms section each link the record: {terms}"
    );
}

/// The firm publishes an SMS program (clients text the firm about their
/// matter), so its legal copy must carry the A2P 10DLC disclosures a carrier
/// campaign review requires. The privacy policy names the two the reviewer
/// flagged as missing — the message-frequency and the rates disclosures — and
/// commits that the mobile number and its consent are not shared onward.
#[tokio::test]
async fn the_firm_privacy_policy_discloses_the_sms_program() {
    let app = app().await;
    let (status, html) = get(&app, "/privacy").await;
    assert_eq!(status, StatusCode::OK);
    let collapsed = html.split_whitespace().collect::<Vec<_>>().join(" ");

    assert!(
        collapsed.contains("Message frequency varies"),
        "the privacy policy must disclose message frequency: {collapsed}"
    );
    assert!(
        collapsed.contains("message and data rates may apply"),
        "the privacy policy must carry the rates disclosure: {collapsed}"
    );
    // The mobile-information sharing commitment the carrier registry requires.
    assert!(
        collapsed.contains("do not share or sell your mobile phone number or your SMS consent"),
        "the privacy policy must promise not to share the SMS opt-in: {collapsed}"
    );
}

/// The full SMS program terms the campaign review listed: a named program, the
/// use case, message frequency, the rates line, STOP and HELP instructions, a
/// customer-care contact, a privacy-policy link, and the carrier-liability
/// disclaimer. Each is a discrete registry requirement, so each is asserted.
#[tokio::test]
async fn the_firm_terms_carry_the_sms_program_disclosures() {
    let app = app().await;
    let (status, html) = get(&app, "/terms").await;
    assert_eq!(status, StatusCode::OK);
    let collapsed = html.split_whitespace().collect::<Vec<_>>().join(" ");

    for required in [
        // The messaging use case and the frequency.
        "text-messaging (SMS) program",
        "Message frequency varies",
        // Costs.
        "Message and data rates may apply",
        // Opt-out and help.
        "Reply STOP",
        "Reply HELP",
        // Customer-care contact. Read from the branding constant the footer
        // publishes rather than written out here, so the disclosure and the
        // number a reader would dial cannot drift apart.
        views::brand::firm_phone(),
        // Carrier-liability disclaimer, verbatim as the registry expects it.
        "Carriers are not liable for any delayed or undelivered messages",
    ] {
        assert!(
            collapsed.contains(required),
            "the Terms must carry the SMS disclosure `{required}`: {collapsed}"
        );
    }
    // The program terms link back to the privacy policy.
    assert!(
        html.contains(r#"href="/privacy""#),
        "the SMS terms must link the privacy policy: {html}"
    );
}

#[tokio::test]
async fn the_documents_render_their_markdown_as_html() {
    // The body is the deployment's own `content/*.md` run through
    // `views::markdown`. If it were escaped instead of emitted, the reader would
    // see the tags as text.
    let app = app().await;
    let (_, html) = get(&app, "/privacy").await;
    assert!(html.contains("<h2"), "no rendered heading: {html}");
    assert!(!html.contains("&lt;h2"), "the body was escaped: {html}");
}
