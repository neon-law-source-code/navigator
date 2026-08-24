//! The firm's public Dioxus SSR pages, and the content each one renders.
//!
//! Every firm page renders through the Dioxus port, so this module — not an
//! Axum route table — is where the firm's public surface actually lives. The
//! content resolvers read the mounted brand bundle directly rather than the
//! ambient branding, because a page's copy is baked at router-build time,
//! before any request scopes branding.

use portal::hosting::PublicRouter as Router;
use portal::{dioxus_app, secure_cookies, AppState, WorkshopIndex};

use crate::firm_copy;

const WORKSHOP_INDEX_TITLE: &str = "Workshops";
const WORKSHOP_INDEX_LEDE: &str =
    "Workshops are our hands-on classes for lawyers and legal professionals who run Neon Law \
     Navigator. Each one is a working session against the real application.";
const WORKSHOP_INDEX_FOOTNOTE: &str = "More workshops land here as we run them.";

const PRESENTATION_INDEX_TITLE: &str = "Presentations";
const PRESENTATION_INDEX_LEDE: &str =
    "Presentations are the talks we give at meetups and conferences. Every code slide is an exact \
     copy of the shipped repository, kept honest by a test that fails the build when one drifts.";
const PRESENTATION_INDEX_FOOTNOTE: &str = "More talks land here as we give them.";

/// Build the `presentations` index: every material the manifest files under
/// that category, in manifest order.
///
/// The contact address is the firm's inbox — a reader on `neonlaw.com` who
/// wants us at their meetup writes to the firm.
fn presentation_index_content(
    workshops: &WorkshopIndex,
) -> webapp::catalog_index::CatalogIndexContent {
    catalog_index_content(
        workshops,
        "presentations",
        PRESENTATION_INDEX_TITLE,
        PRESENTATION_INDEX_LEDE,
        PRESENTATION_INDEX_FOOTNOTE,
    )
}

/// Build the `workshops` index — the catalog page for the Navigator classes.
///
/// Gated exactly like the classes it lists. The page names the lawyer
/// workbench, the admin deployment tier, and the contribution loop, so a
/// reader who cannot open a single class gains nothing from the list.
fn workshop_index_content(workshops: &WorkshopIndex) -> webapp::catalog_index::CatalogIndexContent {
    catalog_index_content(
        workshops,
        "workshops",
        WORKSHOP_INDEX_TITLE,
        WORKSHOP_INDEX_LEDE,
        WORKSHOP_INDEX_FOOTNOTE,
    )
}

/// One category index's Dioxus content: every material the manifest files
/// under `category`, in manifest order.
///
/// The contact address is the firm's on both catalogs — the firm gives the
/// talks and runs the classes, so a reader who wants either writes to it.
fn catalog_index_content(
    workshops: &WorkshopIndex,
    category: &str,
    title: &str,
    lede: &str,
    footnote: &str,
) -> webapp::catalog_index::CatalogIndexContent {
    webapp::catalog_index::CatalogIndexContent {
        title: title.to_string(),
        lede: lede.to_string(),
        materials: workshops
            .materials()
            .iter()
            .filter(|m| m.category == category)
            .map(|m| webapp::catalog_index::CatalogMaterial {
                href: format!("/{}/{}", m.category, m.slug),
                eyebrow: m.audience.clone(),
                title: m.title.clone(),
                summary: m.benefit.clone(),
            })
            .collect(),
        contact_email: views::brand::firm_email().to_string(),
        footnote: footnote.to_string(),
    }
}

/// The firm host's public Dioxus SSR pages, as raw routers for
/// [`portal::bootstrap`]'s `host_dioxus` argument. `bootstrap` wraps each in
/// the anonymous-access session boundary and the shared layer stack, exactly as
/// it does the built-in Dioxus routes.
///
/// Takes `state` because the content-backed pages (e.g. `/blog`) read request
/// state (`BlogIndex`) the router injects into the render context; the
/// brand-only pages ignore it.
#[must_use]
#[allow(clippy::too_many_lines)] // A flat list of the firm's public page routers.
pub fn firm_public_dioxus_routers(state: &AppState) -> Vec<Router> {
    // The blog index is per-host static content; build its wasm-safe post list
    // once (with the shared date formatting) for the Dioxus router to inject.
    let blog_posts = webapp::blog_index::BlogPosts(
        state
            .blog
            .posts()
            .iter()
            .map(|post| webapp::blog_index::BlogPostSummary {
                slug: post.slug.clone(),
                date: format_blog_date(post.date),
                title: post.title.clone(),
                description: post.description.clone(),
            })
            .collect(),
    );
    // The full post bodies keyed by slug — the `/blog/{slug}` route's pre-layer
    // resolves the matched post from this set (or redirects / 404s).
    let blog_post_set = webapp::blog_post::BlogPostSet(std::sync::Arc::new(
        state
            .blog
            .posts()
            .iter()
            .map(|post| {
                (
                    post.slug.clone(),
                    webapp::blog_post::BlogPostContent {
                        date: format_blog_date(post.date),
                        title: post.title.clone(),
                        body_html: post.body_html.clone(),
                    },
                )
            })
            .collect(),
    ));
    let mut routers = vec![
        dioxus_app::blog_index_router(blog_posts),
        dioxus_app::blog_post_router(blog_post_set),
    ];
    // The firm `/contact` page, content resolved from the
    // mounted brand bundle. Resolve the branding from `state.brand_bundle`
    // (mirroring `bootstrap`) rather than the
    // ambient `current()`: this content is baked at router-build time, before
    // any request scopes branding, so a white-label deploy's contact addresses
    // must come from the bundle directly and not the process/default fallback.
    let contact_branding = state
        .brand_bundle
        .as_ref()
        .map_or(&views::brand::DEFAULT_BRANDING, |bundle| {
            views::brand::Branding::from_manifest(&bundle.manifest)
        });
    routers.push(dioxus_app::contact_router(
        "/contact",
        resolve_firm_contact_content(contact_branding),
    ));
    // The home page (`/`): a static statement of the practice, no per-request
    // data.
    routers.push(dioxus_app::home_router(
        "/",
        resolve_firm_home_content(contact_branding),
    ));
    // The practice pages the home page's cards lead into. Static copy like the
    // home page's, resolved here so the `<title>` names the mounted brand.
    routers.push(dioxus_app::litigation_router(
        "/litigation",
        resolve_litigation_content(contact_branding),
    ));
    routers.push(dioxus_app::transactional_router(
        "/fractional-gc",
        resolve_transactional_content(contact_branding),
    ));
    // The platform page. It carries a commercial offer, so it sits with the
    // firm's own pages.
    // The lead offering. A marketing page like the platform page beside it, and
    // the first thing the header carries: the firm runs the technology function
    // for the law firms it serves, and the other three practices sit under it.
    routers.push(dioxus_app::marketing_page_router(
        dioxus_app::FIRM_FRACTIONAL_CTO_PATH,
        firm_copy::fractional_cto(),
    ));
    routers.push(dioxus_app::marketing_page_router(
        dioxus_app::FIRM_NAVIGATOR_PATH,
        firm_copy::navigator(),
    ));
    // The Legal Services page. Like the platform page above it is a marketing
    // page, not a `/services/*` catalog entry: one page describing the routine,
    // one-time work, quoted through `/contact` and publishing no price. It is
    // where the firm's government-form filing work lives.
    routers.push(dioxus_app::marketing_page_router(
        dioxus_app::FIRM_SERVICES_PATH,
        firm_copy::legal_services(),
    ));
    // The talks catalog, and the five read routes each talk publishes: the
    // hub, its light table, the classroom step face, the projector face a
    // presenter opens on a second screen, and the certificate confirmation.
    // The hub's pre-layer also owns the `…/{slug}.md` raw-Markdown twin, which
    // matchit routes there rather than to a second path.
    //
    routers.push(dioxus_app::catalog_index_router(
        dioxus_app::PRESENTATION_INDEX_PATH,
        presentation_index_content(&state.workshops),
    ));
    routers.extend(dioxus_app::catalog_material_routers(
        &dioxus_app::PRESENTATION_PATHS,
        state.workshops.clone(),
        &state.sessions,
        secure_cookies(state),
    ));
    // The Navigator classes, anonymous like the talks.
    // The certificate `POST` keeps its own gate: who may claim a completion
    // certificate is an authorization question, and it stays one even when the
    // material is free to read.
    routers.push(dioxus_app::catalog_index_router(
        dioxus_app::WORKSHOP_INDEX_PATH,
        workshop_index_content(&state.workshops),
    ));
    routers.extend(dioxus_app::catalog_material_routers(
        &dioxus_app::WORKSHOP_PATHS,
        state.workshops.clone(),
        &state.sessions,
        secure_cookies(state),
    ));
    routers.push(portal::catalog_workshop_command_routes(state));
    routers
}

/// Human-readable publish date for the blog (e.g. `"June 19, 2026"`).
/// Kept in `web` so the `views` crate stays free of `chrono`.
fn format_blog_date(date: chrono::NaiveDate) -> String {
    date.format("%B %-d, %Y").to_string()
}

/// Resolve the firm `/contact` content from the mounted `branding`'s addresses
/// — the wasm-safe [`webapp::contact_page::ContactContent`] the Dioxus contact
/// router injects. Takes the resolved `branding` explicitly because the content
/// is baked at router-build time, before per-request branding scope.
fn resolve_firm_contact_content(
    branding: &views::brand::Branding,
) -> webapp::contact_page::ContactContent {
    let firm_name = branding.firm.site_name;

    let page_title = "Contact";
    webapp::contact_page::ContactContent {
        head_title: format!("{firm_name} | {page_title}"),
        meta_description: format!(
            "Reach {firm_name} for estate planning, corporate formation, litigation, and ongoing \
             legal services."
        ),
        page_title: page_title.to_string(),
        firm_heading: firm_name.to_string(),
        // No figure here. No page on this host posts a rate — every engagement
        // is quoted through this page — so a consultation fee would be the first
        // posted number on a surface whose whole purpose is to start a
        // conversation before anything is priced. The page promises the quote,
        // not its amount.
        firm_intro: format!(
            "Email {firm_name} with a short description of the matter — estate planning, \
             corporate formation, ongoing services. We respond within one business day with a \
             flat-fee quote and a calendar link. The first appointment is 30 minutes with a \
             licensed attorney."
        ),
        email_label: "Email".to_string(),
        phone_label: "Phone".to_string(),
        firm_email: branding.firm_email.to_string(),
        firm_phone: branding.firm_phone.to_string(),
    }
}

/// One plain run of practice prose.
fn plain(text: &str) -> webapp::home::CopyRun {
    webapp::home::CopyRun {
        text: text.to_string(),
        emphasis: false,
        href: None,
    }
}

/// One run of practice prose that links, rendered as an inline anchor.
fn link(text: &str, href: &str) -> webapp::home::CopyRun {
    webapp::home::CopyRun {
        text: text.to_string(),
        emphasis: false,
        href: Some(href.to_string()),
    }
}

/// Build the engagements section the home page carries under its statement.
///
/// Four paragraphs, and the firm's own words in each: what vibe coding buys a
/// lawyer, what we configure and deploy, what Navigator does and does not see,
/// and the co-counsel half. Not four cards — the page leads with one offering,
/// and a card grid here would read as four to choose between.
///
/// The named third parties are named deliberately: a firm evaluating this wants
/// to know whether we work with the tools it already runs, and a list is a
/// factual statement about what we configure rather than a claim about outcomes.
/// The Navigator mention links its own page rather than repeating that page here.
fn resolve_service_section() -> webapp::home::ServiceSection {
    webapp::home::ServiceSection {
        heading: "Our engagements".to_string(),
        body: vec![
            vec![plain(
                "We believe vibe coding is an incredibly powerful storytelling tool that allows \
                 you to connect on a deeper level of understanding with your clients. Using \
                 state-of-the-art frontier models, you can create dynamic worlds that are unique \
                 and bespoke to the unique client needs, such as litigation or an estate plan. We \
                 empower you with a safety harness to build these worlds responsibly.",
            )],
            vec![
                plain(
                    "We configure your technical architecture, common software as a service tools \
                     such as Google Workspace, DocuSign, and Xero, AI tooling like Claude and \
                     OpenAI, MCP servers like Descrybe, Midpage, and Trellis, and deploy ",
                ),
                link("Neon Law Navigator", "/navigator"),
                plain(" securely in your environment."),
            ],
            vec![plain(
                "Neon Law Navigator is designed with privacy disclosure and professional ethics \
                 in mind. By default, we do not see our clients' matters. We only collect \
                 anonymized telemetry to ensure your systems are still working.",
            )],
            vec![plain(
                "That being said, our partner firms tap into our litigation and transactional \
                 experience routinely to co-counsel on matters. We work fast, diligently, and \
                 cost-effectively.",
            )],
        ],
    }
}

/// Build the three boxes at the foot of the home page: the practices the firm
/// runs beside its lead offering.
///
/// A sentence each and a link out. No area chips and no figure: the chip lists
/// belong on the pages these link to, and every one of these practices is quoted
/// per engagement — `no_firm_page_publishes_a_fee` covers this page like the
/// rest.
///
/// Each sentence names the fee *arrangement* rather than an amount, because for
/// these three the arrangement is part of the offer: a reader deciding whether to
/// call needs to know a contingency case costs nothing to bring and that the
/// company-counsel work is one monthly figure rather than an hourly meter.
///
/// The marks are drawn line icons rather than emoji. The brief asked for a bigger
/// mark in white, and a colour emoji cannot be recoloured — see
/// [`webapp::home::PracticeMark`] for why that ruled emoji out.
fn resolve_practice_links() -> Vec<webapp::home::PracticeLink> {
    use webapp::home::{PracticeLink, PracticeMark};

    vec![
        PracticeLink {
            mark: PracticeMark::Scales,
            heading: "Litigation".to_string(),
            body: "We try cases on both sides of the v., in complex disputes over technology, \
                   trade secrets, and fraud. Contingency and monthly arrangements rather than an \
                   hourly meter."
                .to_string(),
            href: "/litigation".to_string(),
        },
        PracticeLink {
            mark: PracticeMark::Handshake,
            heading: "Fractional general counsel".to_string(),
            body: "Company counsel on one flat monthly fee, working at the pace your sales cycle \
                   already runs at — contracts, licences, financings, and the corporate advice \
                   under them."
                .to_string(),
            href: "/fractional-gc".to_string(),
        },
        PracticeLink {
            mark: PracticeMark::Gavel,
            heading: "One-time legal services".to_string(),
            body: "The routine matters a person or a company walks in with: a will, a trust, a \
                   formation, a trademark, an annual report. One scope and one flat fee, agreed \
                   before we start."
                .to_string(),
            href: "/services".to_string(),
        },
    ]
}

/// Resolve the firm home page's static copy from the mounted `branding` — the
/// wasm-safe [`webapp::home::HomeContent`] the Dioxus home router injects.
/// Brand-safe like [`resolve_firm_contact_content`]: the `<title>` names the
/// mounted brand, resolved at router-build time.
///
/// The page leads with one offering. The firm's clients here are other law
/// firms: it runs their technology function, carries the privacy and compliance
/// work, and sits beside them as complex counsel and co-counsel. So the body is
/// the statement, then one section of prose — no practice grid and no price.
/// Litigation, Fractional GC, and Legal Services are real practices with their
/// own pages; the header carries them, and every fee is quoted through
/// `/contact`.
pub(crate) fn resolve_firm_home_content(
    branding: &views::brand::Branding,
) -> webapp::home::HomeContent {
    let mark = branding.firm.site_name;
    webapp::home::HomeContent {
        head_title: format!("{mark} | {}", "Home"),
        meta_description: "Fractional CTO for law firms — AI enablement delivered through the \
                           firm, with the privacy and compliance work, complex counsel, and a \
                           co-counsel network on Navigator."
            .to_string(),
        // No hero photograph. The page opens on the statement itself: a firm
        // deciding whether to bring us in is served by the first sentence, not
        // by a landscape above it.
        hero: None,
        // One line, read at a glance. What the sentence means is the section
        // below it.
        heading: "Fractional CTO for law firms".to_string(),
        lead: "We leverage our litigation, transactional, and FAANG-engineering experience to \
               enhance legal practices with state-of-the-art agentic tooling. We help all lawyers \
               and clerks tell wonderful stories with vibe-coding that align to their clients' \
               needs."
            .to_string(),
        contact_href: "/contact".to_string(),
        contact_label: "Contact us".to_string(),
        service: Some(resolve_service_section()),
        practices: resolve_practice_links(),
    }
}

/// Split a heading into its words, marking the first `accent_words` of them as
/// the run the firm sets in its own colour.
fn hero_words(heading: &str, accent_words: usize) -> Vec<webapp::litigation_page::HeroWord> {
    heading
        .split_whitespace()
        .enumerate()
        .map(|(index, text)| webapp::litigation_page::HeroWord {
            text: text.to_string(),
            accent: index < accent_words,
        })
        .collect()
}

/// Resolve the firm `/litigation` page — the statement, the practice, and how
/// the firm runs a matter.
///
/// **The page's claim is speed, and speed is stated as method rather than as
/// outcome.** "Litigation attorneys built for speed" is a differentiator a bar
/// examiner reads as an implied result unless the body binds it to *how the
/// firm works*, so the closing paragraph says so outright. The same line is why
/// `publishes_no_quantified_efficiency_claim` matters more here than it did
/// under the previous framing: a page that leads with speed is one number away
/// from advertising a result.
///
/// The copy carries no em dash. That is the firm's own style call for this
/// page, and `publishes_no_em_dash` holds it.
///
/// Brand-safe like [`resolve_firm_home_content`]: the `<title>` names the
/// mounted brand, resolved at router-build time.
///
/// **The page names matter *types* the firm has litigated and never a matter.**
/// Trademark and copyright, prison rights, divorce, restraining orders, and
/// domestic violence are categories, so none of them identifies a client, a
/// Project code, or an outcome. That distinction is what keeps the copy inside
/// the no-client-data rule while still telling a reader whether this is their
/// practice. Naming experience is also precisely the situation the footer's
/// "Past results do not guarantee future outcomes." exists to cover, and
/// `carries_the_regulated_copy_and_no_results_promise` asserts it reaches the
/// reader.
///
/// **The body is the firm's own filed copy and this resolver holds it verbatim.**
/// The page arrived at these paragraphs by subtraction: it was a Rule 23
/// explainer with six certification-element cards, an authority strip, a phase
/// rail, a chip list, and a fee section. Each was a reasonable answer to a
/// question a prospective client does not walk in with.
///
/// The last four paragraphs — how a matter actually runs here — are additions
/// since, and they are deliberately *prose in the same card* rather than feature
/// sections, because a heading and a grid is the shape of everything this page
/// shed. `renders_two_sections_and_no_more` is what keeps that distinction, so a
/// paragraph may be added here and a section may not.
///
/// The first of them is the only paragraph on the page that links, which is why
/// the body is runs rather than plain strings: it names Navigator and points at
/// `/navigator` instead of restating that page here, the same way the home
/// page's prose does.
///
/// **Every mechanism named is one the workspace can be opened to prove.** The
/// durable event-driven engine lives in `workflows-service`; the inbound triage
/// that classifies a filing or a letter onto a live matter is `nautilus`'s
/// `LAWSUIT_MARKERS`, and "match that record literally" is the honest verb for
/// it, because it tests literal markers rather than searching semantically;
/// `DeadlineKind` calendars a window from the statute that sets it, which is
/// why the sentence says "a statutory deadline" rather than "every deadline";
/// the graph is the `relationship` relation plus the append-only
/// `relationship_log`; and the filing kinds are `store::cases::EntryKind`.
///
/// **Four claims were drafted for this page and cut for want of an
/// implementation**: semantic case-law search and the vendors behind it, regex
/// over the record (the matcher is literal substring), fact extraction, and a
/// per-pleading template library (the tree carries one litigation template, a
/// TRO). A vendor name or a capability on this page is a claim that the
/// workspace carries it, and
/// `litigation_claims_only_capabilities_the_workspace_carries` is the guard
/// that keeps the claim checkable. Describe the step; name the tool only once a
/// module in this tree calls it.
///
/// **This page states no disclaimer of its own.** It used to carry a
/// past-results line under the body, duplicating what the shared footer says on
/// every firm page. The notice now lives once, in
/// `views::brand::DEFAULT_BRANDING`'s `firm_disclaimer`, which opens with
/// "Attorney advertisement." and reaches this page through `PublicFooter`.
///
/// **The page no longer states a fee arrangement, and that is a deliberate
/// deletion rather than an oversight.** The two paragraphs that came out named
/// contingency, monthly billing, and "no cost due if we lose", and an earlier
/// revision kept them on the reasoning that for this practice the arrangement
/// is part of the offer: a reader deciding whether to call needs to know a
/// contingency case costs them nothing to bring. That reasoning did not stop
/// being true when the paragraphs changed. It arguably binds harder now, since
/// the copy addresses people whose first question is whether they can afford to
/// walk in at all. Restoring a single sentence to that effect is the open
/// question against this revision; it is recorded here rather than silently
/// dropped. Fee *amounts* stay off the page either way, and the currency guard
/// still holds that.
pub(crate) fn resolve_litigation_content(
    branding: &views::brand::Branding,
) -> webapp::litigation_page::LitigationContent {
    use webapp::litigation_page::LitigationContent;

    let mark = branding.firm.site_name;
    LitigationContent {
        head_title: format!("{mark} | Litigation"),
        meta_description: "Litigation attorneys built for speed. Plaintiff and defense, in \
                           complex technology disputes for companies and fraud cases for the \
                           people on the receiving end."
            .to_string(),
        eyebrow: "Litigation, plaintiff and defense".to_string(),
        heading: hero_words("Litigation attorneys built for speed.", 2),
        lead: "Our strategy is always the same. Do as much as we can, as early as we can, and get \
               you to a resolution sooner. That is not the right approach for every case. It \
               could be the right one for yours."
            .to_string(),
        cta_href: "/contact".to_string(),
        cta_label: "Contact us".to_string(),
        // The practice in the firm's own two paragraphs, as filed. The company
        // side first, then the individuals — and the fee arrangement in each,
        // because for this practice the arrangement *is* part of the offer: a
        // reader deciding whether to call needs to know a contingency case
        // costs them nothing to bring.
        body: vec![
            vec![plain(
                "We represent those who haven\u{2019}t been justly seen. We have litigated \
                 trademark and copyright disputes, prison rights litigation, and divorce, among \
                 others. Every problem is unique and as long as we are not conflicted out, we will listen to \
                 your story, surround ourselves with experts, and do everything we can when \
                 everything is on the line.",
            )],
            vec![plain(
                "There is little we will not take on. We have handled restraining orders and \
                 domestic violence matters. What these cases share is a person whose rights were \
                 taken, phased out, or lost in a situation nobody had a form for. They do not \
                 wait, and neither do we.",
            )],
            vec![
                plain("All litigation cases run on "),
                link("Neon Law Navigator", "/navigator"),
                plain(
                    ", the firm\u{2019}s case system. Work arrives on the matter as an event: a \
                     new court docket filing, a letter, or new research. Our event-driven agentic \
                     workflows start the work that event implies instead of waiting for someone \
                     to notice it. A statutory deadline is calendared from the statute that sets \
                     it.",
                ),
            ],
            vec![plain(
                "The system is data engineering applied to a case file. We build the matter as a \
                 graph and log each relationship in it as the case moves, so the record of who is \
                 connected to whom is kept as we go rather than reconstructed later. We match \
                 that record literally, because litigation turns on what the record actually \
                 says.",
            )],
            vec![plain(
                "Filings land on the matter by kind: pleading, motion, opposition, reply, order. \
                 The docket, the discovery, and the disclosures sit together, so the next \
                 document starts from what the case already holds.",
            )],
            vec![plain(
                "Speed is how we work. It is not a promise about your result, and it does not \
                 suit every matter. It is why we will not be everyone\u{2019}s lawyer. If you \
                 want to be seen today, we can be your lawyer.",
            )],
        ],
    }
}

/// What the flat monthly transactional fee covers.
const TRANSACTIONAL_INCLUDED: &[(&str, &str)] = &[
    (
        "Cap table management",
        "The ledger stays current as options are granted, exercised, and forfeited — reconciled \
         against the signed instruments and the board consents, not against the spreadsheet.",
    ),
    (
        "Employee and contractor agreements",
        "Offer letters, IP assignment, confidentiality, contractor agreements, and the \
         option-grant paperwork that has to match what the board actually approved.",
    ),
    (
        "Basic taxes and state filings",
        "Nevada Commerce Tax and Modified Business Tax filings, the annual list, and the \
         registered-agent and state calendar that keeps the entity in good standing.",
    ),
    (
        "Corporate housekeeping",
        "Board and stockholder consents, the minute book, and the corporate record a diligence \
         request is going to ask for on two days' notice.",
    ),
    (
        "Counsel on call",
        "The questions that would otherwise wait for a scheduled call. Ask them the day you have \
         them; the answer is inside the monthly fee.",
    ),
];

/// The customer's sales cycle, and the legal step that runs inside each stage
/// rather than after it.
const SALES_CYCLE: &[(&str, &str)] = &[
    (
        "Discovery call",
        "Mutual NDA out the same business day, from your paper or ours.",
    ),
    (
        "Evaluation",
        "Pilot or trial agreement, with the security and data terms your buyer's review is about \
         to ask for already in it.",
    ),
    (
        "Negotiation",
        "Standard MSA drafted or redlined in one business day; counterparty paper reviewed \
         against the fallback positions we set with you in advance.",
    ),
    (
        "Close",
        "Order form, signature routing, and the executed set filed where diligence will look for \
         it a year from now.",
    ),
];

/// Accurate, Efficient, Speedy — the three the practice is named by, each with
/// the sentence that turns it from an adjective into something checkable.
///
/// The bodies are the whole point of the section: "speedy" alone is hyperbole,
/// "one business day on a redline" is a commitment.
fn transactional_virtues() -> Vec<webapp::transactional_page::Virtue> {
    use webapp::transactional_page::Virtue;

    vec![
        Virtue {
            word: "Accurate".to_string(),
            body: "A licensed attorney reads and signs off on every document. The cap table \
                   reconciles to the signed instruments, and the agreements say what the deal \
                   actually is."
                .to_string(),
        },
        Virtue {
            word: "Efficient".to_string(),
            body: "One flat monthly fee covers the recurring work, so a routine question costs \
                   nothing to ask and the answer arrives the day you asked it."
                .to_string(),
        },
        Virtue {
            word: "Speedy".to_string(),
            body: "Turnarounds are published rather than negotiated deal by deal: one business \
                   day on a redline, same business day on an NDA."
                .to_string(),
        },
    ]
}

/// The included line items and the sales-cycle stages, mapped out of their
/// tables. Split out for the same reason as [`litigation_phases`].
fn transactional_sections() -> (
    Vec<webapp::transactional_page::Included>,
    Vec<webapp::transactional_page::SalesStage>,
) {
    use webapp::transactional_page::{Included, SalesStage};

    let included = TRANSACTIONAL_INCLUDED
        .iter()
        .map(|(name, body)| Included {
            name: (*name).to_string(),
            body: (*body).to_string(),
        })
        .collect();
    let cycle = SALES_CYCLE
        .iter()
        .map(|(stage, legal_step)| SalesStage {
            stage: (*stage).to_string(),
            legal_step: (*legal_step).to_string(),
        })
        .collect();
    (included, cycle)
}

/// The work quoted outside the monthly retainer.
///
/// Litigation carries a link to its own page; financings do not, because the
/// firm publishes no financings page and a link that went nowhere would be
/// worse than the sentence alone.
fn transactional_separate_work() -> Vec<webapp::transactional_page::SeparateWork> {
    use webapp::transactional_page::SeparateWork;

    vec![
        SeparateWork {
            name: "Financings".to_string(),
            body: "Priced rounds, SAFEs, convertible notes, and the closing set that goes with \
                   them. Quoted per round once we have seen the term sheet."
                .to_string(),
            href: None,
            link_label: None,
        },
        SeparateWork {
            name: "Litigation".to_string(),
            body: "Disputes, demands, and class actions, on either side of the caption. Quoted \
                   per phase after a case assessment."
                .to_string(),
            href: Some("/litigation".to_string()),
            link_label: Some("The litigation practice".to_string()),
        },
    ]
}

/// Resolve the firm `/fractional-gc` page — the flat-monthly-fee company
/// counsel practice, the published turnaround, and the work that sits outside
/// the retainer.
///
/// Brand-safe like [`resolve_firm_home_content`]. The page names how the flat
/// monthly fee works and sends the figure itself to `/contact`; it publishes no
/// amount.
pub(crate) fn resolve_transactional_content(
    branding: &views::brand::Branding,
) -> webapp::transactional_page::TransactionalContent {
    use webapp::transactional_page::TransactionalContent;

    let mark = branding.firm.site_name;
    let (included, cycle) = transactional_sections();
    TransactionalContent {
        head_title: format!("{mark} | Fractional General Counsel"),
        meta_description: "Company counsel on a flat monthly fee — cap table, employee \
                           agreements, and state tax filings, with a one-business-day redline \
                           turnaround."
            .to_string(),
        eyebrow: "Fractional General Counsel".to_string(),
        heading: "Accurate. Efficient. Speedy.".to_string(),
        lead: "Company counsel on one flat monthly fee, working at the pace your sales cycle \
               already runs at. A redline comes back in one business day."
            .to_string(),
        cta_href: "/contact".to_string(),
        cta_label: "Contact us".to_string(),
        virtues: transactional_virtues(),
        msa_term: "MSA — master services agreement".to_string(),
        msa_definition: "The one contract that sets the terms between you and a customer once: \
                         payment, liability, IP ownership, confidentiality, term, and \
                         termination. Every later deal becomes a short order form that points \
                         back at it instead of a fresh negotiation, which is why getting the MSA \
                         right is what makes the next ten deals quick."
            .to_string(),
        fee_heading: "One flat monthly fee".to_string(),
        fee_body: "One amount, the same in a quiet month as in a loud one, billed monthly. We \
                   quote it for your company rather than posting a number here, and it is fixed \
                   in the engagement letter before the first month runs."
            .to_string(),
        included_heading: "What the monthly fee covers".to_string(),
        included,
        cycle_heading: "It runs inside your sales cycle".to_string(),
        cycle_body: "Legal is a step inside the stage your rep is already in, not a stage that \
                     comes after the deal. Published turnarounds are what make that forecastable."
            .to_string(),
        cycle,
        separate_heading: "Priced separately".to_string(),
        separate_body: "Two kinds of work sit outside the retainer, because neither is recurring \
                        and neither can be quoted by the month."
            .to_string(),
        separate: transactional_separate_work(),
    }
}
