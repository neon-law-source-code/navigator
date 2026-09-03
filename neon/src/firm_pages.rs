//! The firm's public Dioxus SSR pages, and the content each one renders.
//!
//! Every firm page renders through the Dioxus port, so this module — not an
//! Axum route table — is where the firm's public surface actually lives. Copy
//! for pages a brand publishes is loaded per `BrandKey` and injected on the
//! request task (the same seam public chrome uses), so a house-brand host
//! never renders another brand's heading.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{from_fn, from_fn_with_state, Next};
use axum::response::{IntoResponse, Response};
use portal::hosting::PublicRouter as Router;
use portal::{dioxus_app, secure_cookies, AppState, WorkshopIndex};
use views::brand::BrandKey;

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

/// The template's declared questionnaire, in order, ready for the "Try
/// answering this" demo (ENG-452) — parsed with no live Notation and no
/// domain-crate dependency (brand crates cannot depend on `workflows` or
/// `store`; see `views::questionnaire_preview`'s own doc comment).
fn demo_questions(frontmatter: &str) -> Vec<webapp::notation_demo::DemoQuestion> {
    views::questionnaire_preview::parse(frontmatter)
        .into_iter()
        .map(|q| {
            let interactive = q.is_interactive();
            webapp::notation_demo::DemoQuestion {
                code: q.code,
                answer_type: q.answer_type,
                prompt: q.prompt,
                choices: q.choices,
                interactive,
            }
        })
        .collect()
}

/// A sample letter, parsed into the highlighted-preview stage.
fn letter_preview_doc(
    slug: &str,
    source_path: &str,
    src: &str,
) -> webapp::notation_preview::PreviewDoc {
    let doc = views::harvard_outline::parse(src);
    let frontmatter = doc.frontmatter.clone().unwrap_or_default();
    webapp::notation_preview::PreviewDoc {
        slug: slug.to_string(),
        title: doc.title.clone(),
        source_href: format!("{NOTATIONS_BLOB_BASE}{source_path}"),
        demo_questions: demo_questions(&frontmatter),
        frontmatter,
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
        demo_questions: demo_questions(&frontmatter),
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
    let contact_copy = branded_map(branding, |resolved| {
        webapp::contact_page::InjectedContact(resolve_firm_contact_content(resolved))
    });
    routers.push(with_branded(
        dioxus_app::contact_router("/contact", resolve_firm_contact_content(branding)),
        contact_copy,
    ));
    // The firm's `/team` roster: the index and one generic `/team/{slug}`
    // profile route, both live — every request queries the current
    // confirmed, non-client `Person` rows, rather than a roster fixed at
    // boot. `state.surreal` moves through by field access; this crate never
    // names a `store` type itself (`cli/tests/brand_crate_dependencies.rs`).
    routers.push(dioxus_app::team_index_router(
        "/team",
        state.surreal.clone(),
    ));
    routers.push(dioxus_app::team_profile_router(state.surreal.clone()));
    let home = resolve_firm_home_content(branding);
    // The home page (`/`): a static statement of the practice, no per-request
    // data. The practice boxes on `/` are the YAML catalog workshop slides
    // reuse — one list, not a second Rust copy. Slides always expand the
    // Neon catalog, even when another host is serving `/`.
    let practice_catalog = locales::home(&views::brand::DEFAULT_BRANDING)
        .practices
        .clone();
    let home_copy = branded_map(branding, |resolved| {
        webapp::home::InjectedHome(locales::home(resolved))
    });
    routers.push(with_branded(dioxus_app::home_router("/", home), home_copy));
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
        firm_copy::fractional_cto(branding),
    ));
    routers.push(dioxus_app::marketing_page_router(
        dioxus_app::FIRM_NAVIGATOR_PATH,
        firm_copy::navigator(branding),
    ));
    let services_copy = branded_map(branding, |resolved| {
        webapp::marketing_page::InjectedMarketingPage(firm_copy::legal_services(resolved))
    });
    routers.push(with_branded(
        dioxus_app::marketing_page_router(
            dioxus_app::FIRM_SERVICES_PATH,
            firm_copy::legal_services(branding),
        ),
        services_copy,
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
        practice_catalog.clone(),
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
        practice_catalog,
    ));
    routers.push(portal::catalog_workshop_command_routes(state));
    routers
        .into_iter()
        .map(|router| router.layer(from_fn(reject_unpublished_firm_path)))
        .collect()
}

/// Human-readable publish date for the blog (e.g. `"June 19, 2026"`).
/// Kept in `web` so the `views` crate stays free of `chrono`.
fn format_blog_date(date: chrono::NaiveDate) -> String {
    date.format("%B %-d, %Y").to_string()
}

fn branded_map<T>(
    default: &'static views::brand::Branding,
    build: impl Fn(&views::brand::Branding) -> T,
) -> HashMap<BrandKey, T> {
    BrandKey::ALL
        .iter()
        .copied()
        .map(|key| (key, build(key.resolve_branding(default))))
        .collect()
}

fn with_branded<T: Clone + Send + Sync + 'static>(
    router: Router,
    copies: HashMap<BrandKey, T>,
) -> Router {
    router.layer(from_fn_with_state(Arc::new(copies), inject_branded::<T>))
}

async fn inject_branded<T: Clone + Send + Sync + 'static>(
    State(copies): State<Arc<HashMap<BrandKey, T>>>,
    mut req: Request,
    next: Next,
) -> Response {
    let key = req
        .extensions()
        .get::<BrandKey>()
        .copied()
        .unwrap_or_default();
    if let Some(value) = copies.get(&key).cloned() {
        req.extensions_mut().insert(value);
    }
    next.run(req).await
}

async fn reject_unpublished_firm_path(req: Request, next: Next) -> Response {
    let key = req
        .extensions()
        .get::<BrandKey>()
        .copied()
        .unwrap_or_default();
    if key.publishes_firm_path(req.uri().path()) {
        next.run(req).await
    } else {
        (StatusCode::NOT_FOUND, webapp::error_pages::not_found()).into_response()
    }
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
    let meta_description = match branding.brand_key {
        BrandKey::DeleteYourData => format!(
            "Reach {firm_name}, a practice of Shook Law PLLC, about a data-deletion request. \
             Attorney advertisement. Nothing here is legal advice without a signed retainer for \
             an active project."
        ),
        BrandKey::Neon => format!(
            "Reach {firm_name} for estate planning, corporate formation, litigation, and ongoing \
             legal services."
        ),
    };
    webapp::contact_page::ContactContent {
        head_title: format!("{firm_name} | {page_title}"),
        meta_description,
        page_title: page_title.to_string(),
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
/// read through. The lead under it names the docket (cases of every kind) and
/// the work the firm focuses on (impact litigation whose point is to make a
/// person's life better) because a reader deciding whether to call needs both
/// the open door and the aim in the first two sentences. It does not list
/// causes of action; those live on `/litigation`. Speed stays method, and the
/// lead binds the aim so it is not read as a promised result.
///
/// **The page leads with litigation, then shows the whole firm.** The
/// statement opens on impact litigation, `locales/en/home.yaml` says what it
/// means, and the four boxes are the practice: disputes, company counsel,
/// technology for law firms, and one-time filings. The fractional CTO
/// engagement is still real work with a page of its own; it is no longer what
/// this page opens on, and the copy that used to open this page now opens that
/// one. The prose names the team and links `/team` rather than claiming a
/// headcount the roster does not have.
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
/// practice. The docket is open: the page says the firm takes cases of every
/// kind, then names types it has litigated so a reader can still recognise
/// their matter. The focus is impact litigation, stated as aim rather than as
/// a promised result. Naming experience is also precisely the situation the
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

#[cfg(test)]
mod formation_engagement_copy_tests {
    /// The three Nevada formation bodies, read as the bytes
    /// `notation_preview_docs` publishes at `/notations/{slug}`. The constants
    /// there are function-local, so this reads the same files rather than
    /// reaching into that scope.
    const BODIES: [(&str, &str); 3] = [
        (
            "nv__business_trust_formation",
            include_str!(
                "../../templates/forms/united_states/nevada/state/nv__business_trust_formation.md"
            ),
        ),
        (
            "nv__llc_formation",
            include_str!("../../templates/forms/united_states/nevada/state/nv__llc_formation.md"),
        ),
        (
            "nv__profit_corp_formation",
            include_str!(
                "../../templates/forms/united_states/nevada/state/nv__profit_corp_formation.md"
            ),
        ),
    ];

    /// Each body defines `the "Engagement"` and must then use that defined term
    /// to carry its scope. A bare `It covers the` leaves the pronoun to resolve
    /// across two intervening nouns — the entity type and the client's name —
    /// so a reader can attach the scope list to the entity rather than to the
    /// Engagement. The defined term is already in the sentence; using it costs
    /// nothing and removes the ambiguity.
    #[test]
    fn scope_sentence_uses_the_defined_term() {
        for (code, body) in BODIES {
            assert!(
                body.contains("(the \"Engagement\")"),
                "{code}: body no longer defines the \"Engagement\" term"
            );
            assert!(
                body.contains("The Engagement covers the"),
                "{code}: scope sentence does not carry the defined term"
            );
            assert!(
                !body.contains("It covers the"),
                "{code}: scope sentence still leads with the ambiguous pronoun"
            );
        }
    }

    /// Each body opens an engagement, states a scope, and carries both
    /// signature blocks, so it is a paper the client signs. NV RPC 1.5(b)
    /// requires the basis or rate of the fee to be communicated in writing.
    /// These bodies are published templates rather than the fee agreement, so
    /// they satisfy that by naming where the fee is set — not by quoting one.
    #[test]
    fn each_body_says_where_the_fee_is_set() {
        for (code, body) in BODIES {
            assert!(
                body.contains("{{client.signature}}") && body.contains("{{firm.signature}}"),
                "{code}: expected a body both parties sign"
            );
            assert!(
                body.contains("set in the separate signed fee agreement"),
                "{code}: body states a scope but never says where the fee is set"
            );
            assert!(
                body.contains("passed through at cost"),
                "{code}: body does not name filing fees as pass-through"
            );
        }
    }

    /// A published template is a fixed artifact, so any amount baked into one
    /// goes stale the moment the firm re-prices — which is what a former
    /// catalog list price did here. The bodies name where the fee lives and
    /// publish no amount, matching the convention `resolve_transactional_content`
    /// documents for the firm pages.
    #[test]
    fn no_body_publishes_an_amount() {
        for (code, body) in BODIES {
            assert!(
                !body
                    .as_bytes()
                    .windows(2)
                    .any(|w| w[0] == b'$' && w[1].is_ascii_digit()),
                "{code}: body publishes a currency amount"
            );
            assert!(
                !body.contains("per year"),
                "{code}: body publishes a recurring price cadence"
            );
        }
    }
}

#[cfg(test)]
mod notation_catalog_tests {
    use std::{collections::BTreeSet, fs, path::Path};

    use super::{notation_preview_docs, notations_index_content, NOTATIONS_BLOB_BASE};

    fn collect_markdown_paths(root: &Path, base: &Path, paths: &mut BTreeSet<String>) {
        for entry in fs::read_dir(root).expect("read legal template shelf") {
            let path = entry.expect("read legal template directory entry").path();
            if path.is_dir() {
                collect_markdown_paths(&path, base, paths);
            } else if path.extension().is_some_and(|extension| extension == "md") {
                paths.insert(
                    path.strip_prefix(base)
                        .expect("legal template is beneath templates/")
                        .to_string_lossy()
                        .replace(std::path::MAIN_SEPARATOR, "/"),
                );
            }
        }
    }

    #[test]
    fn public_notations_catalog_matches_legal_templates() {
        let repository_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let templates_root = repository_root.join("templates");
        let mut intended_paths = BTreeSet::new();
        // These are the legal-template shelves. The GitHub shelf and prose such
        // as templates/README.md are deliberately outside this catalog.
        for shelf in ["forms", "neon_law"] {
            collect_markdown_paths(
                &templates_root.join(shelf),
                &templates_root,
                &mut intended_paths,
            );
        }

        let catalog = notations_index_content();
        let cards: Vec<String> = catalog
            .materials
            .iter()
            .map(|material| {
                material
                    .href
                    .strip_prefix("/notations/")
                    .expect("catalog card links to a notation preview")
                    .to_string()
            })
            .collect();
        let card_slugs: BTreeSet<&str> = cards.iter().map(String::as_str).collect();
        assert_eq!(
            card_slugs.len(),
            cards.len(),
            "catalog contains a duplicate card"
        );

        let previews = notation_preview_docs();
        let preview_slugs: Vec<&str> = previews
            .iter()
            .map(|preview| preview.slug.as_str())
            .collect();
        let unique_preview_slugs: BTreeSet<&str> = preview_slugs.iter().copied().collect();
        assert_eq!(
            unique_preview_slugs.len(),
            preview_slugs.len(),
            "preview catalog contains a duplicate slug"
        );
        assert_eq!(
            card_slugs, unique_preview_slugs,
            "every catalog card must have exactly one matching preview"
        );

        let preview_paths: Vec<String> = previews
            .iter()
            .map(|preview| {
                preview
                    .source_href
                    .strip_prefix(NOTATIONS_BLOB_BASE)
                    .expect("preview source points at the Navigator template repository")
                    .to_string()
            })
            .collect();
        let unique_preview_paths: BTreeSet<&str> =
            preview_paths.iter().map(String::as_str).collect();
        assert_eq!(
            unique_preview_paths.len(),
            preview_paths.len(),
            "preview catalog contains a duplicate source path"
        );
        assert_eq!(
            intended_paths,
            preview_paths.iter().cloned().collect(),
            "the public catalog must contain every legal template exactly once"
        );

        for preview in previews {
            let source_path = preview
                .source_href
                .strip_prefix(NOTATIONS_BLOB_BASE)
                .expect("preview source points at the Navigator template repository");
            let source = fs::read_to_string(templates_root.join(source_path))
                .expect("preview source path exists");
            let document = views::harvard_outline::parse(&source);
            assert_eq!(
                preview.title, document.title,
                "preview title must come from {source_path}"
            );
            assert_eq!(
                preview.frontmatter,
                document.frontmatter.clone().unwrap_or_default(),
                "preview frontmatter must come from {source_path}"
            );
            assert_eq!(
                preview.stage_html,
                views::harvard_outline::stage_html(&document),
                "preview body must come from {source_path}"
            );
        }
    }
}
