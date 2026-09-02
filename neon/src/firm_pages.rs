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
const NOTATIONS_BLOB_BASE: &str =
    "https://github.com/neon-law-source-code/navigator/blob/main/templates/";

/// A notation's card: the default link opens the show page at
/// `/notations/{slug}` (built from [`notation_preview_docs`]) — a letter's
/// paragraph-highlighted stage or a form's cover sheet — rather than the raw
/// GitHub source, which now lives as a link on that page instead. `slug`
/// names the document (`onboarding-letter`, `nevada-llc-formation`), not
/// just an eyebrow word, because it also becomes the URL's `{slug}` — and,
/// through the sitewide `stamp_document_title` path-derived tab title, the
/// words that title-case into the browser tab's title.
fn notation_card(
    eyebrow: &str,
    title: &str,
    slug: &str,
    summary: &str,
) -> webapp::catalog_index::CatalogMaterial {
    webapp::catalog_index::CatalogMaterial {
        href: format!("/notations/{slug}"),
        eyebrow: eyebrow.to_string(),
        title: title.to_string(),
        summary: summary.to_string(),
    }
}

/// A sample letter, parsed into the highlighted-preview stage.
fn letter_preview_doc(
    slug: &str,
    source_path: &str,
    src: &str,
) -> webapp::notation_preview::PreviewDoc {
    let doc = views::harvard_outline::parse(src);
    webapp::notation_preview::PreviewDoc {
        slug: slug.to_string(),
        title: doc.title.clone(),
        source_href: format!("{NOTATIONS_BLOB_BASE}{source_path}"),
        frontmatter: doc.frontmatter.clone().unwrap_or_default(),
        stage_html: views::harvard_outline::stage_html(&doc),
        origin_url: None,
    }
}

/// A government form, parsed into the same stepping stage as a letter, plus
/// a link to the government's own blank form when the template declares
/// `origin_url`.
fn form_preview_doc(
    slug: &str,
    source_path: &str,
    src: &str,
) -> webapp::notation_preview::PreviewDoc {
    let doc = views::harvard_outline::parse(src);
    let frontmatter = doc.frontmatter.clone().unwrap_or_default();
    let origin_url = views::harvard_outline::frontmatter_field(&frontmatter, "origin_url");
    webapp::notation_preview::PreviewDoc {
        slug: slug.to_string(),
        title: doc.title.clone(),
        source_href: format!("{NOTATIONS_BLOB_BASE}{source_path}"),
        frontmatter,
        stage_html: views::harvard_outline::stage_html(&doc),
        origin_url,
    }
}

/// Every bundled notation's show-page content — the two sample letters and
/// every government form — the content
/// [`portal::dioxus_app::notation_preview_router`] serves at
/// `/notations/{slug}`.
fn notation_preview_docs() -> Vec<webapp::notation_preview::PreviewDoc> {
    const ONBOARDING: &str = include_str!("../../templates/neon_law/shared/onboarding_letter.md");
    const OFFBOARDING: &str = include_str!("../../templates/neon_law/shared/offboarding_letter.md");
    const FORM_990: &str =
        include_str!("../../templates/forms/united_states/federal/irs/us__form_990.md");
    const NATURALIZATION: &str =
        include_str!("../../templates/forms/united_states/federal/uscis/us__naturalization.md");
    const NV_LLC: &str =
        include_str!("../../templates/forms/united_states/nevada/state/nv__llc_formation.md");
    const NV_PROFIT_CORP: &str = include_str!(
        "../../templates/forms/united_states/nevada/state/nv__profit_corp_formation.md"
    );
    const NV_BUSINESS_TRUST: &str = include_str!(
        "../../templates/forms/united_states/nevada/state/nv__business_trust_formation.md"
    );
    const NV_NONPROFIT: &str = include_str!(
        "../../templates/forms/united_states/nevada/state/nv__nonprofit_501c3_formation.md"
    );
    const NV_ANNUAL_REPORT: &str =
        include_str!("../../templates/forms/united_states/nevada/state/nv__annual_report.md");
    const NV_DISSOLUTION: &str =
        include_str!("../../templates/forms/united_states/nevada/state/nv__dissolution.md");
    const NV_MODIFIED_BUSINESS_TAX: &str = include_str!(
        "../../templates/forms/united_states/nevada/state/nv__modified_business_tax.md"
    );
    const NV_CHARITABLE: &str = include_str!(
        "../../templates/forms/united_states/nevada/state/nv__charitable_solicitation_registration.md"
    );

    vec![
        letter_preview_doc(
            "onboarding-letter",
            "neon_law/shared/onboarding_letter.md",
            ONBOARDING,
        ),
        letter_preview_doc(
            "offboarding-letter",
            "neon_law/shared/offboarding_letter.md",
            OFFBOARDING,
        ),
        form_preview_doc(
            "irs-form-990",
            "forms/united_states/federal/irs/us__form_990.md",
            FORM_990,
        ),
        form_preview_doc(
            "application-for-naturalization",
            "forms/united_states/federal/uscis/us__naturalization.md",
            NATURALIZATION,
        ),
        form_preview_doc(
            "nevada-llc-formation",
            "forms/united_states/nevada/state/nv__llc_formation.md",
            NV_LLC,
        ),
        form_preview_doc(
            "nevada-profit-corporation-formation",
            "forms/united_states/nevada/state/nv__profit_corp_formation.md",
            NV_PROFIT_CORP,
        ),
        form_preview_doc(
            "nevada-business-trust-formation",
            "forms/united_states/nevada/state/nv__business_trust_formation.md",
            NV_BUSINESS_TRUST,
        ),
        form_preview_doc(
            "nevada-nonprofit-formation",
            "forms/united_states/nevada/state/nv__nonprofit_501c3_formation.md",
            NV_NONPROFIT,
        ),
        form_preview_doc(
            "nevada-annual-list",
            "forms/united_states/nevada/state/nv__annual_report.md",
            NV_ANNUAL_REPORT,
        ),
        form_preview_doc(
            "nevada-llc-dissolution",
            "forms/united_states/nevada/state/nv__dissolution.md",
            NV_DISSOLUTION,
        ),
        form_preview_doc(
            "nevada-modified-business-tax",
            "forms/united_states/nevada/state/nv__modified_business_tax.md",
            NV_MODIFIED_BUSINESS_TAX,
        ),
        form_preview_doc(
            "nevada-charitable-solicitation-registration",
            "forms/united_states/nevada/state/nv__charitable_solicitation_registration.md",
            NV_CHARITABLE,
        ),
    ]
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
                "onboarding-letter",
                "The sample letter that opens a matter (`onboarding__letter`).",
            ),
            notation_card(
                "Letter",
                "Closing Letter",
                "offboarding-letter",
                "The sample letter that closes a matter (`offboarding__letter`).",
            ),
            notation_card(
                "Form · Federal",
                "IRS Form 990",
                "irs-form-990",
                "Return of Organization Exempt From Income Tax.",
            ),
            notation_card(
                "Form · Federal",
                "Application for Naturalization (N-400)",
                "application-for-naturalization",
                "Intake summary for Form N-400.",
            ),
            notation_card(
                "Form · Nevada",
                "Nevada LLC Formation",
                "nevada-llc-formation",
                "Articles of organization for a Nevada limited-liability company.",
            ),
            notation_card(
                "Form · Nevada",
                "Nevada Profit Corporation Formation",
                "nevada-profit-corporation-formation",
                "Articles of incorporation for a Nevada profit corporation.",
            ),
            notation_card(
                "Form · Nevada",
                "Nevada Business Trust Formation",
                "nevada-business-trust-formation",
                "Certificate of business trust for Nevada.",
            ),
            notation_card(
                "Form · Nevada",
                "Nevada Nonprofit Articles of Incorporation (501(c)(3))",
                "nevada-nonprofit-formation",
                "Articles that form a Nevada nonprofit seeking 501(c)(3) status.",
            ),
            notation_card(
                "Form · Nevada",
                "Nevada Annual List",
                "nevada-annual-list",
                "Annual list of managers, members, and registered agent.",
            ),
            notation_card(
                "Form · Nevada",
                "Nevada LLC Articles of Dissolution",
                "nevada-llc-dissolution",
                "The filing that dissolves a Nevada LLC.",
            ),
            notation_card(
                "Form · Nevada",
                "Nevada Modified Business Tax Return",
                "nevada-modified-business-tax",
                "Nevada Modified Business Tax return.",
            ),
            notation_card(
                "Form · Nevada",
                "Nevada Charitable Solicitation Registration",
                "nevada-charitable-solicitation-registration",
                "Registration before soliciting donations in Nevada.",
            ),
        ],
        contact_email: views::brand::firm_email().to_string(),
        footnote: String::new(),
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
    routers.push(dioxus_app::notation_preview_router(notation_preview_docs()));
    // The firm `/contact` page, content resolved from the same mounted brand
    // bundle as the pages around it.
    routers.push(dioxus_app::contact_router(
        "/contact",
        resolve_firm_contact_content(branding),
    ));
    // The firm's `/team` roster: the index and one profile per person. Static
    // like the pages around it — a firm's own team does not change per
    // request.
    routers.push(dioxus_app::team_index_router("/team", team_index_content()));
    for (path, content) in team_profiles() {
        routers.push(dioxus_app::team_profile_router(&path, content));
    }
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
        email_label: "Email".to_string(),
        phone_label: "Phone".to_string(),
        firm_email: branding.firm_email.to_string(),
        firm_phone: branding.firm_phone.to_string(),
    }
}

/// The firm's `/team` roster: one `(path, name, email, LinkedIn profile)` row
/// per person, in the order the index lists them and the order
/// [`team_profiles`] mounts their routes.
///
/// Two people today. A third is a row here and nowhere else — no generic
/// `/team/{slug}` router exists, because two literal routes are the whole of
/// what a two-person roster needs.
const TEAM_ROSTER: [(&str, &str, &str, &str); 2] = [
    (
        "nick",
        "Nick",
        "nick@neonlaw.com",
        "https://www.linkedin.com/in/nicholas-shook/",
    ),
    (
        "jask",
        "Jask",
        "jask@neonlaw.com",
        "https://www.linkedin.com/in/jasks/",
    ),
];

/// The `/team` index content: every roster row, linked to its own profile.
fn team_index_content() -> webapp::team_page::TeamIndexContent {
    let firm_name = views::brand::FIRM_BRAND.site_name;
    webapp::team_page::TeamIndexContent {
        head_title: format!("{firm_name} | Team"),
        meta_description: format!("The people at {firm_name}, and how to reach each of them."),
        page_title: "Team".to_string(),
        members: TEAM_ROSTER
            .iter()
            .map(|(slug, name, _, _)| webapp::team_page::TeamMemberSummary {
                name: (*name).to_string(),
                href: format!("/team/{slug}"),
            })
            .collect(),
    }
}

/// Every `/team/{slug}` profile, paired with the path it mounts at.
fn team_profiles() -> Vec<(String, webapp::team_page::TeamProfileContent)> {
    let firm_name = views::brand::FIRM_BRAND.site_name;
    TEAM_ROSTER
        .iter()
        .map(|(slug, name, email, linkedin_href)| {
            (
                format!("/team/{slug}"),
                webapp::team_page::TeamProfileContent {
                    head_title: format!("{firm_name} | {name}"),
                    meta_description: format!("Reach {name} at {firm_name}."),
                    name: (*name).to_string(),
                    email: (*email).to_string(),
                    linkedin_href: (*linkedin_href).to_string(),
                },
            )
        })
        .collect()
}

/// Resolve the firm home page's static copy from the mounted `branding` — the
/// wasm-safe [`webapp::home::HomeContent`] the Dioxus home router injects.
/// Brand-safe like [`resolve_firm_contact_content`]: the `<title>` names the
/// mounted brand, resolved at router-build time.
///
/// **The page's statement is the firm's tagline, and the practice it leads with
/// is litigation.** "Everyone deserves to be seen." is the whole of the `<h1>`:
/// it is what the firm is for, and it is short enough to be read rather than
/// read through. The lead under it names the practice (litigators) and the two
/// kinds of client it is for — a person wronged or wrongly accused, or a
/// company in a dispute — because a reader deciding whether to call needs to
/// recognise themselves in the first two sentences. It does not list causes of
/// action; those live on `/litigation`.
///
/// **The page leads with litigation.** The statement opens on it,
/// `locales/en/home.yaml` says what it means, and the three boxes are the
/// engagements that sit beside it, each labeled for a different visitor. The
/// fractional CTO engagement is still real work with a page of its own; it is
/// no longer what this page opens on, and the copy that used to open this page
/// now opens that one.
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
/// practice. The inventory is finite on purpose: the page does not say the firm
/// will take anything. Naming experience is also precisely the situation the
/// footer's "Past results do not guarantee future outcomes." exists to cover,
/// and `carries_the_regulated_copy_and_no_results_promise` asserts it reaches
/// the reader.
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
/// durable event-driven engine lives in `workflows-service`; the graph is the
/// `relationship` relation plus the append-only `relationship_log`; and the
/// filing kinds are `store::cases::EntryKind`.
///
/// **Four claims were drafted for this page and cut for want of an
/// implementation**: semantic case-law search and the vendors behind it, regex
/// over the record, fact extraction, and a per-pleading template library (the
/// tree carries one litigation template, a TRO). A vendor name or a capability
/// on this page is a claim that the workspace carries it, and
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
