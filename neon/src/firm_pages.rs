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
const PRESENTATION_INDEX_TITLE: &str = "Presentations";
const PRESENTATION_INDEX_LEDE: &str =
    "Presentations are the talks we give at meetups and conferences. Every code slide is an exact \
     copy of the shipped repository, kept honest by a test that fails the build when one drifts.";

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
        footnote: String::new(),
    }
}

const NOTATIONS_INDEX_TITLE: &str = "Notations";
const NOTATIONS_INDEX_LEDE: &str =
    "A notation is one markdown file: the template a client signs, the questionnaire that fills \
     it in, and the workflow that carries it from intake through attorney review to signature, \
     filing, or closing. Navigator ships the sample letters that open and close a matter, and \
     the government forms the firm files.";
const NOTATIONS_INDEX_FOOTNOTE: &str =
    "Letters are the firm's confidential work product. Blank government PDFs belong to the \
     issuing agency; the catalog cards and workflows beside them are the firm's.";
const NOTATIONS_BLOB_BASE: &str =
    "https://github.com/neon-law-source-code/navigator/blob/main/templates/";

fn notation_card(
    eyebrow: &str,
    title: &str,
    path: &str,
    summary: &str,
) -> webapp::catalog_index::CatalogMaterial {
    webapp::catalog_index::CatalogMaterial {
        href: format!("{NOTATIONS_BLOB_BASE}{path}"),
        eyebrow: eyebrow.to_string(),
        title: title.to_string(),
        summary: summary.to_string(),
    }
}

/// The public `/notations` catalog: the sample engagement letters and every
/// government form in `templates/forms/`.
fn notations_index_content() -> webapp::catalog_index::CatalogIndexContent {
    webapp::catalog_index::CatalogIndexContent {
        title: NOTATIONS_INDEX_TITLE.to_string(),
        lede: NOTATIONS_INDEX_LEDE.to_string(),
        materials: vec![
            notation_card(
                "Letter",
                "Onboarding Letter",
                "neon_law/shared/onboarding_letter.md",
                "The sample letter that opens a matter (`onboarding__letter`).",
            ),
            notation_card(
                "Letter",
                "Closing Letter",
                "neon_law/shared/offboarding_letter.md",
                "The sample letter that closes a matter (`offboarding__letter`).",
            ),
            notation_card(
                "Form · Federal",
                "IRS Form 990",
                "forms/united_states/federal/irs/us__form_990.md",
                "Return of Organization Exempt From Income Tax.",
            ),
            notation_card(
                "Form · Federal",
                "Application for Naturalization (N-400)",
                "forms/united_states/federal/uscis/us__naturalization.md",
                "Intake summary for Form N-400.",
            ),
            notation_card(
                "Form · Nevada",
                "Nevada LLC Formation",
                "forms/united_states/nevada/state/nv__llc_formation.md",
                "Articles of organization for a Nevada limited-liability company.",
            ),
            notation_card(
                "Form · Nevada",
                "Nevada Profit Corporation Formation",
                "forms/united_states/nevada/state/nv__profit_corp_formation.md",
                "Articles of incorporation for a Nevada profit corporation.",
            ),
            notation_card(
                "Form · Nevada",
                "Nevada Business Trust Formation",
                "forms/united_states/nevada/state/nv__business_trust_formation.md",
                "Certificate of business trust for Nevada.",
            ),
            notation_card(
                "Form · Nevada",
                "Nevada Nonprofit Articles of Incorporation (501(c)(3))",
                "forms/united_states/nevada/state/nv__nonprofit_501c3_formation.md",
                "Articles that form a Nevada nonprofit seeking 501(c)(3) status.",
            ),
            notation_card(
                "Form · Nevada",
                "Nevada Annual List",
                "forms/united_states/nevada/state/nv__annual_report.md",
                "Annual list of managers, members, and registered agent.",
            ),
            notation_card(
                "Form · Nevada",
                "Nevada LLC Articles of Dissolution",
                "forms/united_states/nevada/state/nv__dissolution.md",
                "The filing that dissolves a Nevada LLC.",
            ),
            notation_card(
                "Form · Nevada",
                "Nevada Modified Business Tax Return",
                "forms/united_states/nevada/state/nv__modified_business_tax.md",
                "Nevada Modified Business Tax return.",
            ),
            notation_card(
                "Form · Nevada",
                "Nevada Charitable Solicitation Registration",
                "forms/united_states/nevada/state/nv__charitable_solicitation_registration.md",
                "Registration before soliciting donations in Nevada.",
            ),
        ],
        contact_email: views::brand::firm_email().to_string(),
        footnote: NOTATIONS_INDEX_FOOTNOTE.to_string(),
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
    // Resolve the branding from `state.brand_bundle` (mirroring `bootstrap`)
    // rather than the ambient `current()`: this content is baked at
    // router-build time, before any request scopes branding.
    let branding = state
        .brand_bundle
        .as_ref()
        .map_or(&views::brand::DEFAULT_BRANDING, |bundle| {
            views::brand::Branding::from_manifest(&bundle.manifest)
        });
    routers.push(dioxus_app::catalog_index_router(
        dioxus_app::NOTATIONS_INDEX_PATH,
        notations_index_content(),
    ));
    // The home page (`/`): a static statement of the practice, no per-request
    // data.
    routers.push(dioxus_app::home_router(
        "/",
        resolve_firm_home_content(branding),
    ));
    // The practice pages the home page's cards lead into. Static copy like the
    // home page's, resolved here so the `<title>` names the mounted brand.
    routers.push(dioxus_app::litigation_router(
        "/litigation",
        resolve_litigation_content(branding),
    ));
    routers.push(dioxus_app::transactional_router(
        "/fractional-gc",
        resolve_transactional_content(branding),
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

/// Build the litigation statement the home page carries under its tagline.
///
/// **The page leads with litigation, and this is the section that says what
/// that means.** Four paragraphs: who the firm stands with, why speed is the
/// method rather than the point, how a matter is taken on, and what the rest of
/// the firm's work is for. Not four cards, because a card grid here would read
/// as four things to choose between and the page leads with one.
///
/// **The practice areas named are categories, never a matter.** Personal
/// injury, divorce, business divorce, and criminal investigations describe
/// kinds of dispute, so none of them identifies a client, a Project code, or an
/// outcome. That is what keeps this copy inside the no-client-data rule while
/// still telling a reader whether this is their practice.
///
/// **Speed is stated as method, not as result.** "We move fast" on its own
/// reads as an implied outcome, so the paragraph binds it to *how the firm
/// works* and then says outright that speed is in service of being seen. The
/// `/litigation` page states the same method at length, and this paragraph
/// links there rather than restating it, which is why the body is runs rather
/// than plain strings.
///
/// No fee arrangement and no amount: `no_firm_page_publishes_a_fee` covers this
/// page like the rest, and every engagement is quoted through `/contact`.
fn resolve_service_section() -> webapp::home::ServiceSection {
    webapp::home::ServiceSection {
        heading: "We are by your side through tough times.".to_string(),
        body: vec![
            vec![plain(
                "We stand with people who have been wrongly accused, and with people against \
                 whom a real wrong was committed and no form was ever written for it. Personal \
                 injury. Divorce, and the business divorce that looks nothing like it on paper \
                 and exactly like it in the room. Criminal investigations, from the first letter \
                 onward.",
            )],
            vec![
                plain("We move fast and judiciously, and the "),
                link("litigation practice", "/litigation"),
                plain(
                    " states the method: do as much as we can, as early as we can, and get to a \
                     resolution sooner. Speed is not the point of it. A person who is not seen \
                     loses ground whether or not the case is close, and moving early is how we \
                     keep that from happening quietly.",
                ),
            ],
            vec![plain(
                "Every problem is unique. As long as we are not conflicted out, we will listen \
                 to your story, surround ourselves with the experts it needs, and do everything \
                 we can when everything is on the line.",
            )],
            vec![plain(
                "The rest of what we do exists because litigation taught us to build it: the \
                 systems, the discipline, and the habit of writing everything down. Whatever \
                 brings you in, the work runs the same way.",
            )],
        ],
    }
}

/// Build the three boxes at the foot of the home page: the engagements the firm
/// runs beside the litigation practice it leads with.
///
/// **Litigation is not one of these boxes any more, and that is the point.** It
/// is the page's lead and its close, so a fourth box repeating it here would
/// put the practice the page is built around in a row of alternatives. What is
/// left is the three the header carries beside it: the fractional CTO
/// engagement, the fractional general counsel engagement, and the one-time
/// legal services schedule.
///
/// A sentence each and a link out. No area chips and no figure: the chip lists
/// belong on the pages these link to, and every one of these engagements is
/// quoted per engagement, so `no_firm_page_publishes_a_fee` covers this page
/// like the rest.
///
/// The marks are drawn line icons rather than emoji, because a colour emoji
/// cannot be recoloured to sit white on the dark theme — see
/// [`webapp::home::PracticeMark`] for why that ruled emoji out.
fn resolve_practice_links() -> Vec<webapp::home::PracticeLink> {
    use webapp::home::{PracticeLink, PracticeMark};

    vec![
        PracticeLink {
            mark: PracticeMark::Technology,
            heading: "Fractional CTO".to_string(),
            body: "We run the technology function for law firms: the architecture, the AI \
                   tooling, and the privacy and compliance work under both. The same systems we \
                   litigate on."
                .to_string(),
            href: "/fractional-cto".to_string(),
        },
        PracticeLink {
            mark: PracticeMark::Handshake,
            heading: "Fractional GC".to_string(),
            body: "Company counsel on one flat monthly fee, working at the pace your sales cycle \
                   already runs at: contracts, licences, financings, and the corporate advice \
                   under them."
                .to_string(),
            href: "/fractional-gc".to_string(),
        },
        PracticeLink {
            mark: PracticeMark::Gavel,
            heading: "One-Time Services".to_string(),
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
/// **The page's statement is the firm's tagline, and the practice it leads with
/// is litigation.** "Everyone deserves to be seen." is the whole of the `<h1>`:
/// it is what the firm is for, and it is short enough to be read rather than
/// read through. The lead under it names the two kinds of person the litigation
/// practice is for — wrongly accused, or wronged — because a reader deciding
/// whether to call needs to recognise themselves in the first two sentences.
///
/// **The page leads with litigation.** The statement opens on it,
/// [`resolve_service_section`] says what it means, and the three boxes are the
/// engagements that sit beside it. The fractional CTO engagement is still real
/// work with a page of its own; it is no longer what this page opens on, and
/// the copy that used to open this page now opens that one.
///
/// The home page opens on a New York skyline, supplied as a finished PNG in the
/// public asset lane. No price, on any section — every engagement is quoted
/// through `/contact`.
pub(crate) fn resolve_firm_home_content(
    branding: &views::brand::Branding,
) -> webapp::home::HomeContent {
    let mark = branding.firm.site_name;
    webapp::home::HomeContent {
        head_title: format!("{mark} | {}", "Home"),
        meta_description: "Everyone deserves to be seen. Litigation for the wrongly accused and \
                           the wronged: personal injury, divorce, business divorce, and criminal \
                           investigations."
            .to_string(),
        hero: Some(webapp::home::HeroPicture {
            // This finished PNG is served directly from the public asset lane;
            // it does not need the responsive-photo build manifest.
            sources: Vec::new(),
            fallback_src: views::assets::asset_url("img/new-york/new-york.png"),
            alt: "New York City skyline at sunset, viewed from above Lower Manhattan.".to_string(),
            sizes: "100vw".to_string(),
        }),
        // The firm's tagline, and the whole of the h1. Read at a glance; what
        // it means is the section below it.
        heading: "Everyone deserves to be seen.".to_string(),
        lead: "We fight for people who have been wrongly accused, and for people who were \
               wronged and handed no form to put it on. We move fast and judiciously, because \
               being heard late is its own injury."
            .to_string(),
        contact_href: format!("mailto:{}", branding.firm_email),
        contact_label: "Contact us".to_string(),
        service: Some(resolve_service_section()),
        practices_heading: "Our complementary practice".to_string(),
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
/// outcome.** "Litigation built for speed" is a differentiator a bar
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
        eyebrow: "Values-Based Litigation".to_string(),
        heading: hero_words("Litigation built for speed.", 1),
        lead: "Our strategy is generally the same. Do as much as we can, as early as we can, and get \
               you to a resolution sooner. That is not the right approach for every case. It \
               could be the right one for yours."
            .to_string(),
        cta_href: format!("mailto:{}", branding.firm_email),
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
        heading: hero_words("Accurate. Efficient. Speedy.", 1),
        lead: "Company counsel on one flat monthly fee, working at the pace your sales cycle \
               already runs at. A redline comes back in one business day."
            .to_string(),
        cta_href: format!("mailto:{}", branding.firm_email),
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
