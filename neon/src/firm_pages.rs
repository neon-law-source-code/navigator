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
use crate::locales;

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
    // The firm `/contact` page, content resolved from the same mounted brand
    // bundle as the pages around it.
    routers.push(dioxus_app::contact_router(
        "/contact",
        resolve_firm_contact_content(branding),
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
    // one-time work, quoted through `mailto:contact@neonlaw.com` and
    // publishing no price. It is where the firm's government-form filing work
    // lives.
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
/// `locales/en/home.yaml` says what it means, and the three boxes are the
/// engagements that sit beside it. The fractional CTO engagement is still real
/// work with a page of its own; it is no longer what this page opens on, and
/// the copy that used to open this page now opens that one.
///
/// The home page opens on a New York skyline, supplied as a finished PNG in the
/// public asset lane. No price, on any section — every engagement is quoted
/// through `mailto:contact@neonlaw.com`.
pub(crate) fn resolve_firm_home_content(
    branding: &views::brand::Branding,
) -> webapp::home::HomeContent {
    locales::home(branding)
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
/// **The body is the firm's own filed copy and `locales/en/litigation.yaml` holds it verbatim.**
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
    locales::litigation(branding)
}

/// Resolve the firm `/fractional-gc` page — the flat-monthly-fee company
/// counsel practice, the published turnaround, and the work that sits outside
/// the retainer.
///
/// Brand-safe like [`resolve_firm_home_content`]. The page names how the flat
/// monthly fee works and sends the figure itself to
/// `mailto:contact@neonlaw.com`; it publishes no amount.
pub(crate) fn resolve_transactional_content(
    branding: &views::brand::Branding,
) -> webapp::transactional_page::TransactionalContent {
    locales::fractional_gc(branding)
}
