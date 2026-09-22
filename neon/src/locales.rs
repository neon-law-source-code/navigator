//! Load each house brand's English marketing catalog and map it onto page types.
//!
//! The words live in `locales/en/<brand-key>/*.yaml`, and the sentences this
//! repository shares with `navigator-ux` live once in `locales/en/shared.yaml`.
//! This module is the only Rust that reads either: pick the directory for the
//! request's `BrandKey`, resolve `{shared:<key>}` against the shared catalog,
//! interpolate the brand placeholders, deserialize, and fill the few runtime
//! fields a YAML file cannot know (the day-rate photo URL, the CLI release
//! archives). Editing published copy is a YAML change.

use views::brand::BrandKey;
use views::locales::{
    interpolate, BandCopy, CardCopy, CopyRun, HeroCtaCopy, HeroLine, HomeCopy, LitigationCopy,
    MarketingPageCopy, PackageInstallCopy, PageSkin, Paragraph, PracticeLinkCopy, PracticeMark,
    PricingCardCopy, ProjectNetworkNodeCopy, ProvenanceLedgerRowCopy, ProvenanceMark,
    ProvenancePillarCopy, ProvenanceSectionCopy, ProvenanceStepCopy, ServiceSectionCopy, StepCopy,
    TransactionalCopy, VirtueCopy,
};
use webapp::components::DayRateBadge;
use webapp::lead_capture::LeadCaptureCopy;
use webapp::marketing_page::{
    Band, Card, Download, HeroCta, PackageInstall, PageContent, ProjectNetworkNode, Run, Step,
};

/// The cross-repository copy catalog. `navigator-ux` consumes an export of
/// this same file, pinned to one Navigator revision, so the two repositories
/// publish the same sentence without keeping two copies of it.
const SHARED_CATALOG_YAML: &str = include_str!("../locales/en/shared.yaml");

const NEON_HOME_YAML: &str = include_str!("../locales/en/neon/home.yaml");
const NEON_LITIGATION_YAML: &str = include_str!("../locales/en/neon/litigation.yaml");
const NEON_FRACTIONAL_GC_YAML: &str = include_str!("../locales/en/neon/fractional-gc.yaml");
const NEON_NOTATIONS_YAML: &str = include_str!("../locales/en/neon/notations.yaml");
const NEON_NAVIGATOR_YAML: &str = include_str!("../locales/en/neon/navigator.yaml");
const NEON_SERVICES_YAML: &str = include_str!("../locales/en/neon/services.yaml");
/// The firm's individual services as records. Only Neon publishes one; the
/// other house brands render `/services` without an individual-services band.
const NEON_SERVICES_CATALOG_YAML: &str = include_str!("../locales/en/neon/services-catalog.yaml");
const DELETE_YOUR_DATA_HOME_YAML: &str = include_str!("../locales/en/delete-your-data/home.yaml");
const VESTA_HOME_YAML: &str = include_str!("../locales/en/vesta/home.yaml");
const LAWYER_SHOOK_HOME_YAML: &str = include_str!("../locales/en/lawyer-shook/home.yaml");
const VESTA_SERVICES_YAML: &str = include_str!("../locales/en/vesta/services.yaml");
const MISERICORDIA_HOME_YAML: &str = include_str!("../locales/en/misericordia/home.yaml");
const MISERICORDIA_SERVICES_YAML: &str = include_str!("../locales/en/misericordia/services.yaml");
const ABHAYA_HOME_YAML: &str = include_str!("../locales/en/abhaya/home.yaml");
const ABHAYA_SERVICES_YAML: &str = include_str!("../locales/en/abhaya/services.yaml");
const DELETE_YOUR_DEBT_HOME_YAML: &str = include_str!("../locales/en/delete-your-debt/home.yaml");
const DELETE_YOUR_DEBT_SERVICES_YAML: &str =
    include_str!("../locales/en/delete-your-debt/services.yaml");
const SUMMONS_HOME_YAML: &str = include_str!("../locales/en/summons/home.yaml");
const SUMMONS_SERVICES_YAML: &str = include_str!("../locales/en/summons/services.yaml");
const DELETE_YOUR_DATA_SERVICES_YAML: &str =
    include_str!("../locales/en/delete-your-data/services.yaml");
const LAWYER_SHOOK_SERVICES_YAML: &str = include_str!("../locales/en/lawyer-shook/services.yaml");

/// The shipped shared catalog, parsed and validated once.
///
/// Every page load resolves references against it, so it is parsed on first
/// use and kept. `navigator project gate` (`Y002`) is the gate that keeps the
/// parse infallible, and the `shared_catalog_is_valid` test below proves it
/// in the Rust suite.
#[must_use]
pub fn shared_catalog() -> &'static views::locales::shared::SharedCatalog {
    static CATALOG: std::sync::OnceLock<views::locales::shared::SharedCatalog> =
        std::sync::OnceLock::new();
    CATALOG.get_or_init(|| {
        views::locales::shared::SharedCatalog::parse(SHARED_CATALOG_YAML).expect(
            "invariant: locales/en/shared.yaml is valid; navigator project gate Y002 is the gate",
        )
    })
}

/// The shipped YAML for `key`'s `page` stem, if that brand publishes it.
#[must_use]
pub fn catalog_yaml(key: BrandKey, page: &str) -> Option<&'static str> {
    match (key, page) {
        (BrandKey::Neon, "home") => Some(NEON_HOME_YAML),
        (BrandKey::Vesta, "home") => Some(VESTA_HOME_YAML),
        (BrandKey::LawyerShook, "home") => Some(LAWYER_SHOOK_HOME_YAML),
        (BrandKey::Vesta, "services") => Some(VESTA_SERVICES_YAML),
        (BrandKey::Misericordia, "home") => Some(MISERICORDIA_HOME_YAML),
        (BrandKey::Misericordia, "services") => Some(MISERICORDIA_SERVICES_YAML),
        (BrandKey::Abhaya, "home") => Some(ABHAYA_HOME_YAML),
        (BrandKey::Abhaya, "services") => Some(ABHAYA_SERVICES_YAML),
        (BrandKey::DeleteYourDebt, "home") => Some(DELETE_YOUR_DEBT_HOME_YAML),
        (BrandKey::DeleteYourDebt, "services") => Some(DELETE_YOUR_DEBT_SERVICES_YAML),
        (BrandKey::Summons, "home") => Some(SUMMONS_HOME_YAML),
        (BrandKey::Summons, "services") => Some(SUMMONS_SERVICES_YAML),
        (BrandKey::Neon, "litigation") => Some(NEON_LITIGATION_YAML),
        (BrandKey::Neon, "fractional-gc") => Some(NEON_FRACTIONAL_GC_YAML),
        (BrandKey::Neon, "notations") => Some(NEON_NOTATIONS_YAML),
        (BrandKey::Neon, "navigator") => Some(NEON_NAVIGATOR_YAML),
        (BrandKey::Neon, "services") => Some(NEON_SERVICES_YAML),
        (BrandKey::Neon, views::locales::services::SERVICES_CATALOG_STEM) => {
            Some(NEON_SERVICES_CATALOG_YAML)
        }
        (BrandKey::DeleteYourData, "home") => Some(DELETE_YOUR_DATA_HOME_YAML),
        (BrandKey::DeleteYourData, "services") => Some(DELETE_YOUR_DATA_SERVICES_YAML),
        (BrandKey::LawyerShook, "services") => Some(LAWYER_SHOOK_SERVICES_YAML),
        _ => None,
    }
}

/// Load one catalog file as `T`, after resolving shared copy and the mounted
/// brand.
///
/// Shared references resolve first, so a shared sentence may itself carry a
/// brand placeholder and still be filled for the brand that mounted it.
fn load<T: serde::de::DeserializeOwned>(yaml: &str, branding: &views::brand::Branding) -> T {
    let shared = views::locales::shared::resolve_references(
        yaml,
        shared_catalog(),
        branding.brand_key.as_str(),
    )
    .expect("invariant: every `{shared:…}` reference resolves; the catalog test is the gate");
    let raw = interpolate(&shared, branding.firm.site_name, branding.firm_email);
    serde_yaml::from_str(&raw).expect(
        "invariant: shipped locale YAML deserializes; navigator project gate Y002 is the gate",
    )
}

/// The brand's individual-services schedule, if it publishes one.
///
/// Resolved per brand rather than parsed once, because a catalog value may
/// carry the brand placeholders the way page copy does. Routers are built at
/// startup, so this runs a handful of times per process.
#[must_use]
pub fn services_catalog(
    branding: &views::brand::Branding,
) -> Option<views::locales::services::ServicesCatalog> {
    let yaml = catalog_yaml(
        branding.brand_key,
        views::locales::services::SERVICES_CATALOG_STEM,
    )?;
    let shared = views::locales::shared::resolve_references(
        yaml,
        shared_catalog(),
        branding.brand_key.as_str(),
    )
    .expect("invariant: every `{shared:…}` reference resolves; the catalog test is the gate");
    let raw = interpolate(&shared, branding.firm.site_name, branding.firm_email);
    Some(
        views::locales::services::ServicesCatalog::parse(&raw).expect(
            "invariant: the shipped services catalog is valid; navigator project gate Y002 is the gate",
        ),
    )
}

fn load_page<T: serde::de::DeserializeOwned>(branding: &views::brand::Branding, page: &str) -> T {
    let yaml = catalog_yaml(branding.brand_key, page).unwrap_or_else(|| {
        panic!(
            "invariant: {} publishes `{page}`; BrandKey::catalog_pages is the gate",
            branding.brand_key.as_str()
        )
    });
    load(yaml, branding)
}

/// Split a hero statement into words, marking the first `accent_words` of them.
fn hero_words(line: &HeroLine) -> Vec<webapp::litigation_page::HeroWord> {
    line.text
        .split_whitespace()
        .enumerate()
        .map(|(index, text)| webapp::litigation_page::HeroWord {
            text: text.to_string(),
            accent: index < line.accent_words,
        })
        .collect()
}

fn copy_run_to_home(run: CopyRun) -> webapp::home::CopyRun {
    webapp::home::CopyRun {
        text: run.text,
        emphasis: run.emphasis,
        href: run.href,
    }
}

fn copy_run_to_marketing(run: CopyRun) -> Run {
    Run {
        text: run.text,
        emphasis: run.emphasis,
        code: run.code,
        href: run.href,
    }
}

fn paragraphs_to_home(body: Vec<Paragraph>) -> Vec<Vec<webapp::home::CopyRun>> {
    body.into_iter()
        .map(|paragraph| paragraph.into_iter().map(copy_run_to_home).collect())
        .collect()
}

fn paragraphs_to_marketing(body: Vec<Paragraph>) -> Vec<webapp::marketing_page::Paragraph> {
    body.into_iter()
        .map(|paragraph| paragraph.into_iter().map(copy_run_to_marketing).collect())
        .collect()
}

fn provenance_mark(mark: ProvenanceMark) -> webapp::home::ProvenanceMark {
    match mark {
        ProvenanceMark::Request => webapp::home::ProvenanceMark::Request,
        ProvenanceMark::Attorney => webapp::home::ProvenanceMark::Attorney,
        ProvenanceMark::Chain => webapp::home::ProvenanceMark::Chain,
    }
}

fn provenance_to_home(copy: ProvenanceSectionCopy) -> webapp::home::ProvenanceSection {
    let ProvenanceSectionCopy {
        overline,
        heading,
        heading_accent,
        lead,
        steps,
        ledger_heading,
        ledger_caption,
        ledger,
        pillars,
        notes,
    } = copy;
    webapp::home::ProvenanceSection {
        overline,
        heading,
        heading_accent,
        lead,
        steps: steps
            .into_iter()
            .map(
                |ProvenanceStepCopy {
                     mark,
                     label,
                     detail,
                 }| webapp::home::ProvenanceStep {
                    mark: provenance_mark(mark),
                    label,
                    detail,
                },
            )
            .collect(),
        ledger_heading,
        ledger_caption,
        ledger: ledger
            .into_iter()
            .map(
                |ProvenanceLedgerRowCopy { label, status }| webapp::home::ProvenanceLedgerRow {
                    label,
                    status,
                },
            )
            .collect(),
        pillars: pillars
            .into_iter()
            .map(
                |ProvenancePillarCopy { heading, body }| webapp::home::ProvenancePillar {
                    heading,
                    body,
                },
            )
            .collect(),
        notes: paragraphs_to_home(notes),
    }
}

fn practice_mark(mark: PracticeMark) -> webapp::components::PracticeMark {
    match mark {
        PracticeMark::Scales => webapp::components::PracticeMark::Scales,
        PracticeMark::Handshake => webapp::components::PracticeMark::Handshake,
        PracticeMark::Gavel => webapp::components::PracticeMark::Gavel,
        PracticeMark::Technology => webapp::components::PracticeMark::Technology,
        PracticeMark::Helm => webapp::components::PracticeMark::Helm,
    }
}

fn page_skin(skin: PageSkin) -> webapp::marketing_page::PageSkin {
    match skin {
        PageSkin::Marketing => webapp::marketing_page::PageSkin::Marketing,
        PageSkin::Practice => webapp::marketing_page::PageSkin::Practice,
    }
}

/// The photo `views::assets::asset_url` resolves for a published day rate,
/// if this amount has one. `webapp` cannot resolve this itself — it also
/// compiles for the browser, where the server-only `views` crate is not
/// available — so this is the one place a whole-dollar amount becomes the
/// badge the page actually renders. An amount with no photo (any value other
/// than the four denominations the firm has published so far) renders no
/// badge at all, same as no amount.
fn resolve_day_rate(amount: Option<u16>) -> Option<DayRateBadge> {
    let amount = amount?;
    let key = match amount {
        50 => "img/fifty-dollar-bill/fifty-dollar-bill.jpg",
        10 => "img/ten-dollar-bill/ten-dollar-bill.jpg",
        5 => "img/five-dollar-bill/five-dollar-bill.jpg",
        1 => "img/one-dollar-bill/one-dollar-bill.jpg",
        _ => return None,
    };
    Some(DayRateBadge {
        amount,
        image_src: views::assets::asset_url(key),
    })
}

fn card(copy: CardCopy) -> Card {
    Card {
        title: copy.title,
        chips: copy.chips,
        body: paragraphs_to_marketing(copy.body),
        href: copy.href,
        href_label: copy.href_label,
        cadence: copy.cadence,
        features: copy.features,
        day_rate: resolve_day_rate(copy.day_rate_bill),
    }
}

fn step(copy: StepCopy) -> Step {
    Step {
        title: copy.title,
        body: paragraphs_to_marketing(copy.body),
    }
}

fn network_node(copy: ProjectNetworkNodeCopy) -> ProjectNetworkNode {
    ProjectNetworkNode {
        label: copy.label,
        detail: copy.detail,
    }
}

fn network_label(value: String, fallback: &str) -> String {
    if value.is_empty() {
        fallback.to_string()
    } else {
        value
    }
}

fn package_install(copy: PackageInstallCopy) -> PackageInstall {
    PackageInstall {
        heading: copy.heading,
        body: paragraphs_to_marketing(copy.body),
        commands: vec![webapp::cli_release::HOMEBREW_INSTALL_COMMAND.to_string()],
    }
}

fn fill_downloads(
    anchor: String,
    overline: String,
    heading: String,
    description: Option<String>,
    archive_label: String,
    package: Option<PackageInstallCopy>,
) -> Band {
    let version = webapp::cli_release::release_version();
    Band::Downloads {
        anchor,
        overline,
        heading,
        description,
        version: version.clone(),
        archive_href: webapp::cli_release::RELEASES_HREF.to_string(),
        archive_label,
        items: webapp::cli_release::PLATFORMS
            .iter()
            .map(|platform| Download {
                platform: platform.slug.to_string(),
                label: platform.label.to_string(),
                detail: platform.detail.to_string(),
                filename: webapp::cli_release::asset_filename(&version, platform),
                href: webapp::cli_release::asset_href(&version, platform),
                mark: platform.mark,
            })
            .collect(),
        package: package.map(package_install),
    }
}

/// One service, resolved for rendering: the catalog's record with its fee and
/// category label filled in.
fn service(
    catalog: &views::locales::services::ServicesCatalog,
    record: &views::locales::services::ServiceCopy,
) -> webapp::services_search::Service {
    webapp::services_search::Service {
        id: record.id.clone(),
        item: record.item.clone(),
        name: record.name.clone(),
        blurb: record.blurb.clone(),
        category: catalog.category_label(record.category).to_string(),
        audience: record.category.search_aliases().to_string(),
        includes: record.includes.clone(),
        keywords: record.keywords.clone(),
        fee: catalog.fee(record).to_string(),
        period: record.period.clone(),
        members_only: record.members_only,
        state_fee: record.state_fee,
        package: catalog.package_quote(record).map(|quote| {
            webapp::services_search::ServicePackageQuote {
                members: quote.members,
            }
        }),
        plan_price: record
            .plan_price
            .as_ref()
            .map(|price| webapp::services_search::PlanPrice {
                amount: price.amount.clone(),
                plan: price.plan.clone(),
            }),
        template: record.template.clone(),
    }
}

/// The individual-services band: page copy from the catalog file's sibling
/// `services.yaml`, and the services themselves from the catalog.
///
/// Panics on any other band, which the one call site's `copy @` pattern makes
/// unreachable.
fn services_band(copy: BandCopy, catalog: &views::locales::services::ServicesCatalog) -> Band {
    let BandCopy::Services {
        anchor,
        overline,
        heading,
        description,
        search_label,
        search_placeholder,
        submit_label,
        examples,
        fee_label,
        includes_label,
        package_badge,
        package_members_label,
        members_badge,
        state_fee_badge,
        empty,
        empty_help,
        clear_label,
    } = copy
    else {
        unreachable!("services_band is called with a services band")
    };
    Band::Services(Box::new(webapp::services_search::ServicesBand {
        anchor,
        overline,
        heading,
        description,
        search_label,
        search_placeholder,
        submit_label,
        examples: examples
            .into_iter()
            .map(|example| webapp::services_search::SearchExample {
                label: example.label,
                query: example.query,
            })
            .collect(),
        fee_label,
        includes_label,
        package_badge,
        package_members_label,
        members_badge,
        state_fee_badge,
        empty,
        empty_help,
        clear_label,
        start_label: catalog.start.label.clone(),
        start_microcopy: catalog.start.microcopy.clone(),
        services: catalog
            .services
            .iter()
            .map(|record| service(catalog, record))
            .collect(),
    }))
}

fn band(copy: BandCopy, catalog: Option<&views::locales::services::ServicesCatalog>) -> Band {
    match copy {
        BandCopy::Statement {
            heading,
            lead,
            body,
        } => Band::Statement {
            heading,
            lead,
            body: paragraphs_to_marketing(body),
        },
        BandCopy::Cards {
            anchor,
            overline,
            heading,
            description,
            items,
            pricing_style,
        } => Band::Cards {
            anchor,
            overline,
            heading,
            description,
            items: items.into_iter().map(card).collect(),
            pricing_style,
        },
        BandCopy::Steps {
            anchor,
            overline,
            heading,
            description,
            items,
        } => Band::Steps {
            anchor,
            overline,
            heading,
            description,
            items: items.into_iter().map(step).collect(),
        },
        copy @ BandCopy::ProjectNetwork { .. } => project_network_band(copy),
        BandCopy::Downloads {
            anchor,
            overline,
            heading,
            description,
            archive_label,
            package,
        } => fill_downloads(
            anchor,
            overline,
            heading,
            description,
            archive_label,
            package,
        ),
        copy @ BandCopy::Services { .. } => services_band(
            copy,
            catalog.expect(
                "invariant: a page carrying a `services` band publishes a services catalog; \
                 `the_services_band_only_ships_where_a_catalog_does` is the gate",
            ),
        ),
        BandCopy::Cta {
            heading,
            body,
            email,
            email_subject,
        } => Band::Cta {
            heading,
            body,
            email,
            email_subject,
        },
    }
}

fn project_network_band(copy: BandCopy) -> Band {
    let BandCopy::ProjectNetwork {
        anchor,
        overline,
        heading,
        description,
        center_eyebrow,
        center_heading,
        center_detail,
        left_lane_label,
        right_lane_label,
        left,
        right,
        mcp_tools,
        agentic_coding_tools,
        saas_tools,
    } = copy
    else {
        unreachable!("project network helper receives only project network copy")
    };

    Band::ProjectNetwork {
        anchor,
        overline,
        heading,
        description,
        center_eyebrow: network_label(center_eyebrow, "The Project center"),
        center_heading: network_label(center_heading, "Navigator"),
        center_detail,
        left_lane_label: network_label(
            left_lane_label,
            "Project resources to the left of Navigator",
        ),
        right_lane_label: network_label(
            right_lane_label,
            "Project resources to the right of Navigator",
        ),
        left: left.into_iter().map(network_node).collect(),
        right: right.into_iter().map(network_node).collect(),
        mcp_tools,
        agentic_coding_tools,
        saas_tools,
    }
}

fn marketing_page(
    copy: MarketingPageCopy,
    catalog: Option<&views::locales::services::ServicesCatalog>,
    branding: &views::brand::Branding,
) -> PageContent {
    PageContent {
        head_title: copy.head_title,
        meta_description: copy.meta_description,
        title: copy.title,
        hero_mark: copy.hero_mark.map(practice_mark),
        tagline: copy.tagline,
        hero_lines: copy.hero_lines.iter().map(hero_words).collect(),
        hero_lead: copy.hero_lead,
        hero_lead_runs: copy
            .hero_lead_runs
            .into_iter()
            .map(copy_run_to_marketing)
            .collect(),
        hero_cta: copy
            .hero_cta
            .map(|HeroCtaCopy { href, label }| HeroCta { href, label }),
        skin: page_skin(copy.skin),
        bands: copy
            .bands
            .into_iter()
            .map(|copy| band(copy, catalog))
            .collect(),
        lead_capture: lead_capture(branding),
    }
}

/// Resolve the shared consent language for the mounted brand. The marketing
/// page constructors replace this default with the request's brand copy below;
/// the helper is kept at the same catalog boundary as the page readers.
pub fn lead_capture(branding: &views::brand::Branding) -> LeadCaptureCopy {
    let copy = |key: &str| {
        let raw = shared_catalog()
            .lookup(branding.brand_key.as_str(), key)
            .unwrap_or_else(|| panic!("invariant: shared lead key `{key}` is present"));
        interpolate(raw, branding.firm.site_name, branding.firm_email)
    };
    LeadCaptureCopy {
        consent_sentence: copy("lead.consent"),
        phone_helper: copy("lead.phone_helper"),
        sms_label: copy("lead.sms_label"),
    }
}

/// The lifetime estate offer, mapped from the catalog onto the page type.
///
/// `webapp::home::EstateContent` mirrors `views::locales::EstateCopy` because
/// `views` is server-gated and the page type is compiled for the wasm client
/// too, so this is the seam that carries one onto the other.
fn estate_content(copy: views::locales::EstateCopy) -> webapp::home::EstateContent {
    webapp::home::EstateContent {
        eyebrow: copy.eyebrow,
        process_link: copy.process_link,
        plan_label: copy.plan_label,
        price: copy.price,
        price_term: copy.price_term,
        features: copy.features,
        fee_note: copy.fee_note,
        video_label: copy.video_label,
        // The catalog authors the object key; the page needs the URL it
        // resolves to in this deployment.
        video_src: views::assets::asset_url(&copy.video_src),
        transcript_label: copy.transcript_label,
        video_transcript: copy.video_transcript,
        process_label: copy.process_label,
        process_heading: copy.process_heading,
        steps: copy.steps,
        record_label: copy.record_label,
        record_price: copy.record_price,
        record_unit: copy.record_unit,
        record_status: copy.record_status,
        record_heading: copy.record_heading,
        record_body: copy.record_body,
        record_note: copy.record_note,
        closing_heading: copy.closing_heading,
        closing_body: copy.closing_body,
    }
}

fn privacy_content(copy: views::locales::PrivacyCopy) -> webapp::home::PrivacyContent {
    webapp::home::PrivacyContent {
        eyebrow: copy.eyebrow,
        price: copy.price,
        price_term: copy.price_term,
        offer_note: copy.offer_note,
        gift_link: copy.gift_link,
        pause_label: copy.pause_label,
        benefits_heading: copy.benefits_heading,
        record_label: copy.record_label,
        record_heading: copy.record_heading,
        record_body: copy.record_body,
        gift_heading: copy.gift_heading,
        gift_body: copy.gift_body,
        gift_cta: copy.gift_cta,
        gift_card_label: copy.gift_card_label,
        gift_card_term: copy.gift_card_term,
        closing_heading: copy.closing_heading,
        benefits: copy.benefits,
    }
}

/// A sibling practice's public home, when it has one.
///
/// `None` while the practice is held out of launch, because
/// `portal::canonical_host` refuses its hosts and the link would land a
/// reader on a `404`. `views::brand::BrandKey::public_home_href_for` builds
/// the address for this deployment; the launch gate decides whether there is
/// one to hand out.
fn sibling_practice_href(
    key: views::brand::BrandKey,
    deployment_host: Option<&str>,
) -> Option<String> {
    key.is_live()
        .then(|| key.public_home_href_for(deployment_host))
}

/// The firm home page, resolved from this brand's `home.yaml`.
pub fn home(branding: &views::brand::Branding) -> webapp::home::HomeContent {
    home_for_host(branding, None)
}

/// The firm home page, with cross-brand links matched to this deployment.
pub fn home_for_host(
    branding: &views::brand::Branding,
    deployment_host: Option<&str>,
) -> webapp::home::HomeContent {
    let copy: HomeCopy = load_page(branding, "home");
    webapp::home::HomeContent {
        head_title: copy.head_title,
        meta_description: copy.meta_description,
        heading: copy.heading,
        lead: copy.lead,
        contact_href: if matches!(
            branding.brand_key,
            BrandKey::Vesta | BrandKey::DeleteYourData
        ) {
            branding.consultation_url.to_string()
        } else {
            format!("mailto:{}", branding.firm_email)
        },
        contact_label: copy.contact_label,
        service: copy.service.map(|ServiceSectionCopy { heading, body }| {
            webapp::home::ServiceSection {
                heading,
                body: paragraphs_to_home(body),
            }
        }),
        practices_heading: copy.practices_heading,
        practices: copy
            .practices
            .into_iter()
            .map(
                |PracticeLinkCopy {
                     mark,
                     heading,
                     body,
                     href,
                 }| webapp::home::PracticeLink {
                    mark: practice_mark(mark),
                    heading,
                    body,
                    href,
                    logo_href: String::new(),
                    font_family: String::new(),
                },
            )
            .collect(),
        provenance: copy.provenance.map(provenance_to_home),
        company: copy.company.map(|copy| webapp::home::CompanyContent {
            booking_href: copy.booking_href,
            pricing_link: copy.pricing_link,
            hero_note: copy.hero_note,
            flow_caption: copy.flow_caption,
            flow_steps: copy.flow_steps,
            packages: copy.packages,
            pause_label: copy.pause_label,
            pricing_heading: copy.pricing_heading,
            video_label: copy.video_label,
            video_src: views::assets::asset_url(views::assets::HOME_PRESENTATION_KEY),
            membership_label: copy.membership_label,
            membership_price: copy.membership_price,
            membership_unit: copy.membership_unit,
            membership_body: copy.membership_body,
            membership_features: copy.membership_features,
            review_heading: copy.review_heading,
            review_body: copy.review_body,
            review_columns: copy.review_columns,
            review_rows: copy.review_rows,
            review_note: copy.review_note,
            drafting_heading: copy.drafting_heading,
            drafting_packages: copy.drafting_packages,
            closing_heading: copy.closing_heading,
            closing_body: copy.closing_body,
            litigation_heading: copy.litigation_heading,
            litigation_link: copy.litigation_link,
            litigation_price: copy.litigation_price,
            litigation_unit: copy.litigation_unit,
            litigation_body: copy.litigation_body,
            litigation_note: copy.litigation_note,
            people_heading: copy.people_heading,
            people_body: copy.people_body,
            immigration_label: copy.immigration_label,
            estate_label: copy.estate_label,
            // The launch gate decides whether each sibling's name links. Both
            // practices are real and separately engaged whatever it says;
            // what it governs is whether the page hands a reader an address.
            immigration_href: sibling_practice_href(
                views::brand::BrandKey::Abhaya,
                deployment_host,
            ),
            estate_href: sibling_practice_href(views::brand::BrandKey::Vesta, deployment_host),
            navigator_heading: copy.navigator_heading,
            navigator_body: copy.navigator_body,
            navigator_link: copy.navigator_link,
            source_label: copy.source_label,
            source_note: copy.source_note,
        }),
        // Lawyer Shook's resolver supplies its firm notice over this catalog.
        bare: None,
        estate: copy.estate.map(estate_content),
        privacy: copy.privacy.map(privacy_content),
    }
}

/// The `/disputes` page, resolved from this brand's `litigation.yaml`.
pub fn litigation(branding: &views::brand::Branding) -> webapp::litigation_page::LitigationContent {
    let copy: LitigationCopy = load_page(branding, "litigation");
    webapp::litigation_page::LitigationContent {
        head_title: copy.head_title,
        meta_description: copy.meta_description,
        eyebrow: copy.eyebrow,
        heading: hero_words(&copy.heading),
        lead: copy.lead,
        cta_href: format!("mailto:{}", branding.firm_email),
        cta_label: copy.cta_label,
        body: paragraphs_to_home(copy.body),
    }
}

/// The `/business` page, resolved from this brand's `fractional-gc.yaml`.
pub fn fractional_gc(
    branding: &views::brand::Branding,
) -> webapp::transactional_page::TransactionalContent {
    let copy: TransactionalCopy = load_page(branding, "fractional-gc");
    webapp::transactional_page::TransactionalContent {
        head_title: copy.head_title,
        meta_description: copy.meta_description,
        eyebrow: copy.eyebrow,
        heading: hero_words(&copy.heading),
        lead: copy.lead,
        cta_href: format!("mailto:{}", branding.firm_email),
        cta_label: copy.cta_label,
        virtues: copy
            .virtues
            .into_iter()
            .map(|VirtueCopy { word, body }| webapp::transactional_page::Virtue { word, body })
            .collect(),
        fee_heading: copy.fee_heading,
        fee_body: copy.fee_body,
        pricing: copy
            .pricing
            .into_iter()
            .map(
                |PricingCardCopy {
                     title,
                     price,
                     cadence,
                     blurb,
                     features,
                     day_rate_bill,
                 }| webapp::transactional_page::PricingOffer {
                    title,
                    price,
                    cadence,
                    blurb,
                    features,
                    day_rate: resolve_day_rate(day_rate_bill),
                },
            )
            .collect(),
        closing_heading: copy.closing_heading,
        closing_body: copy.closing_body,
        closing_email: copy.closing_email,
    }
}

/// `/navigator`, from this brand's `navigator.yaml`.
pub fn navigator(branding: &views::brand::Branding) -> PageContent {
    marketing_page(load_page(branding, "navigator"), None, branding)
}

/// `/services`, from this brand's `services.yaml`.
pub fn legal_services(branding: &views::brand::Branding) -> PageContent {
    marketing_page(
        load_page(branding, "services"),
        services_catalog(branding).as_ref(),
        branding,
    )
}

/// The public notation format explanation, loaded from the English catalog.
pub(crate) fn notations_content() -> PageContent {
    let branding = &views::brand::DEFAULT_BRANDING;
    marketing_page(load_page(branding, "notations"), None, branding)
}

#[cfg(test)]
mod tests {
    use super::*;
    use views::brand::BrandKey;
    use views::locales::parse_locale_file;
    use webapp::marketing_page::Band as RenderedBand;

    #[test]
    fn home_company_contract_publishes_membership_and_separate_review_prices() {
        let copy: serde_yaml::Value = serde_yaml::from_str(NEON_HOME_YAML).unwrap();
        let company = &copy["company"];
        assert_eq!(company["membership_price"].as_str(), Some("$50"));
        assert_eq!(company["review_rows"][0][1].as_str(), Some("$100"));
        assert_eq!(company["review_rows"][2][2].as_str(), Some("$5,000"));
        assert!(company["review_note"]
            .as_str()
            .unwrap()
            .contains("5 p.m. PST"));
        assert!(company["source_note"]
            .as_str()
            .unwrap()
            .contains("BUSL-1.1"));
    }

    /// The shipped shared catalog is the contract both repositories consume.
    /// If it stops parsing, the export the other repository pins stops being
    /// producible — so this fails here rather than in the exporter.
    #[test]
    fn shared_catalog_is_valid() {
        let catalog = shared_catalog();
        assert_eq!(
            catalog.catalog_version,
            views::locales::shared::SUPPORTED_CATALOG_VERSION
        );
        for key in views::locales::shared::REQUIRED_KEYS {
            assert!(
                catalog.lookup(BrandKey::Neon.as_str(), key).is_some(),
                "the shared catalog must publish `{key}`"
            );
        }
    }

    /// Every `{shared:…}` a shipped page references must name a key the
    /// catalog defines, for every brand that publishes that page. An
    /// unresolved reference would otherwise reach a reader as a brace.
    #[test]
    fn every_shared_reference_a_shipped_page_makes_resolves() {
        let catalog = shared_catalog();
        for key in BrandKey::ALL {
            for page in key.catalog_pages() {
                let yaml = catalog_yaml(*key, page).expect("shipped catalog");
                for referenced in views::locales::shared::referenced_keys(yaml) {
                    assert!(
                        catalog.lookup(key.as_str(), &referenced).is_some(),
                        "{} `{page}` references `{referenced}`, which the shared catalog does not define",
                        key.as_str()
                    );
                }
                views::locales::shared::resolve_references(yaml, catalog, key.as_str())
                    .unwrap_or_else(|err| panic!("{} `{page}`: {err}", key.as_str()));
            }
        }
    }

    /// The shared catalog is not decoration: the pages that reference it must
    /// actually be the ones that carry the duplicated sentences.
    #[test]
    fn shared_copy_is_consumed_by_pages_or_the_exported_home_contract() {
        let mut referenced: std::collections::BTreeSet<String> = BrandKey::ALL
            .iter()
            .flat_map(|key| {
                key.catalog_pages()
                    .iter()
                    .filter_map(|page| catalog_yaml(*key, page))
                    .flat_map(views::locales::shared::referenced_keys)
            })
            .collect();
        // Lead copy is consumed by the shared form component rather than a
        // page YAML document, so account for that typed catalog reader here.
        referenced.extend([
            "lead.consent".to_string(),
            "lead.phone_helper".to_string(),
            "lead.sms_label".to_string(),
        ]);
        // The versioned home keys remain part of the navigator-ux export.
        // Neon's company offer is authored in its own home catalog.
        for key in shared_catalog()
            .keys()
            .into_iter()
            .filter(|key| !key.starts_with("home."))
        {
            assert!(
                referenced.contains(key),
                "`{key}` is authored in the shared catalog but no page reads it"
            );
        }
    }

    /// Every registry key ships every catalog page it declares, and each file
    /// deserializes as that page. A missing file fails here, not at first request.
    #[test]
    fn every_registry_key_has_every_required_catalog_file() {
        for key in BrandKey::ALL {
            for page in key.catalog_pages() {
                let yaml = catalog_yaml(*key, page).unwrap_or_else(|| {
                    panic!(
                        "{} is missing locales/en/{}/{page}.yaml",
                        key.as_str(),
                        key.as_str()
                    )
                });
                parse_locale_file(page, yaml)
                    .unwrap_or_else(|err| panic!("{} `{page}`: {err}", key.as_str()));
            }
        }
    }

    /// The band a brand declares in its `services.yaml` and the catalog file
    /// beside it must agree. A `kind: services` band with no catalog panics at
    /// router build; a catalog nothing renders is a file nobody reads.
    #[test]
    fn the_services_band_only_ships_where_a_catalog_does() {
        for key in BrandKey::ALL {
            if !key.catalog_pages().contains(&"services") {
                continue;
            }
            let branding = key.resolve_branding(&views::brand::DEFAULT_BRANDING);
            let declares_band = catalog_yaml(*key, "services")
                .expect("a brand shipping `services` ships the file")
                .contains("kind: services");
            assert_eq!(
                declares_band,
                services_catalog(branding).is_some(),
                "{}: a `kind: services` band and a services catalog ship together",
                key.as_str()
            );
        }
    }

    /// `/services` renders the schedule from the catalog: every service, with
    /// its fee resolved and its `related` ids turned into names a reader can
    /// follow.
    #[test]
    fn the_services_page_renders_the_whole_catalog() {
        let catalog =
            services_catalog(&views::brand::DEFAULT_BRANDING).expect("Neon ships a catalog");
        let content = legal_services(&views::brand::DEFAULT_BRANDING);
        let band = content
            .bands
            .iter()
            .find_map(RenderedBand::services)
            .expect("the page renders its services band");
        assert_eq!(band.services.len(), catalog.services.len());
        assert_eq!(band.services.len(), 18);

        // The fee is resolved: a flat-fee service reads the catalog's one
        // figure rather than shipping an empty price.
        let llc = band
            .services
            .iter()
            .find(|service| service.id == "llc-file")
            .expect("llc-file renders");
        assert_eq!(llc.fee, catalog.flat_fee);
        assert_eq!(llc.category, "Start a business");
        // The estate audience words ride along, searched and never printed.
        let will = band
            .services
            .iter()
            .find(|service| service.id == "will")
            .expect("will renders");
        assert_eq!(will.audience, "personal family legacy");
        assert!(
            will.matches("family"),
            "estate work is findable by `family`"
        );
        assert_eq!(will.fee, "$3,000");
        let family_plan = band
            .services
            .iter()
            .find(|service| service.id == "estate-package")
            .expect("estate-package renders");
        let quote = family_plan
            .package
            .as_ref()
            .expect("estate-package is a Notation package");
        assert_eq!(quote.members.len(), 3);
        assert_eq!(family_plan.fee, "$5,000");
        // The retired consumer plan's discount is gone with the plan.
        assert_eq!(family_plan.plan_price, None);
    }

    /// The search finds services by the words a reader would actually type,
    /// through the shipped catalog rather than a fixture.
    #[test]
    fn the_shipped_catalog_answers_a_readers_words() {
        let content = legal_services(&views::brand::DEFAULT_BRANDING);
        let band = content
            .bands
            .iter()
            .find_map(RenderedBand::services)
            .expect("the page renders its services band");
        for (needle, expected) in [
            ("help with my LLC", "llc-file"),
            ("trademark", "trademark"),
            ("eviction", "eviction"),
            ("I need a will", "will"),
            ("1504", "msa"),
        ] {
            let found = band.matching(needle);
            assert!(
                found.iter().any(|service| service.id == expected),
                "`{needle}` must find `{expected}`; found {:?}",
                found.iter().map(|s| &s.id).collect::<Vec<_>>()
            );
        }
        // Every example chip the band offers finds something. A chip that
        // lands on the empty state is a chip advertising a dead end.
        for example in &band.examples {
            assert!(
                !band.matching(&example.query).is_empty(),
                "the `{}` chip finds nothing",
                example.label
            );
        }
    }

    /// Brand placeholders become the mounted site name and inbox.
    #[test]
    fn home_catalog_names_the_mounted_brand() {
        let content = home(&views::brand::DEFAULT_BRANDING);
        assert!(content
            .head_title
            .contains(views::brand::FIRM_BRAND.site_name));
        assert_eq!(
            content.contact_href,
            format!("mailto:{}", views::brand::firm_email())
        );
        assert_eq!(content.heading, "Keep building.");
        assert!(content.company.is_some());
        assert!(content.practices.is_empty());
    }

    /// The firm's own site markets to emerging technology companies alone.
    ///
    /// This is the "Done when" of retiring `/personal`, and it is asserted on
    /// the rendered copy rather than on the routing table, because the page
    /// can be unreachable while the words that sold it survive in the hero,
    /// the plan chooser, or a link — which is exactly what happened on
    /// `DeleteYourData`, whose own pages went on offering the retired plan.
    #[test]
    fn the_home_page_no_longer_markets_to_individuals() {
        let content = home(&views::brand::DEFAULT_BRANDING);
        let mut text = vec![content.heading.clone(), content.lead.clone()];
        if let Some(service) = content.service.as_ref() {
            text.extend(service.body.iter().flatten().map(|run| run.text.clone()));
        }
        for practice in &content.practices {
            text.push(practice.heading.clone());
            text.push(practice.body.clone());
        }
        let text = text.join(" ");

        for gone in ["Personal plan", "Personal Plan", "your family", "/personal"] {
            assert!(
                !text.contains(gone),
                "{gone:?} still on the home page: {text}"
            );
        }
        assert!(
            text.contains("technology"),
            "and the audience is named: {text}"
        );
    }

    /// `/business` publishes its base package as a $50-a-day retainer.
    #[test]
    fn fractional_gc_publishes_its_fifty_dollar_day_rate() {
        let content = fractional_gc(&views::brand::DEFAULT_BRANDING);
        let offer = content.pricing.first().expect("the Business plan offer");
        assert_eq!(offer.price, "$50");
        assert_eq!(offer.cadence.as_deref(), Some("/day"));
        for benefit in [
            "Contract-library access",
            "Name Neon Law as your counsel",
            "$10,000 minimum retainer to start",
            "60 days before daily credits run out",
            "unchanged template for signature at $5",
            "Notations for prepared or revised documents start at $100",
            "One agreed scope and price for each Notation",
        ] {
            assert!(
                offer
                    .features
                    .iter()
                    .any(|feature| feature.contains(benefit)),
                "the Business plan names {benefit:?}: {:?}",
                offer.features
            );
        }
        assert!(
            offer.blurb.contains("Ulysses S. Grant"),
            "the Business plan gives the requested daily-price reference"
        );
        assert_eq!(
            offer.day_rate.as_ref().map(|badge| badge.amount),
            Some(50),
            "the Business plan renders the matching $50 bill mark"
        );
        assert!(
            offer
                .day_rate
                .as_ref()
                .is_some_and(|badge| badge.image_src.ends_with("fifty-dollar-bill.jpg")),
            "the Business plan uses the published $50 bill asset"
        );
    }

    #[test]
    fn litigation_starts_with_a_free_consultation_without_a_plan() {
        let content = litigation(&views::brand::DEFAULT_BRANDING);
        let text = content
            .body
            .iter()
            .flatten()
            .map(|run| run.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(content.cta_label, "Request a free consultation");
        assert!(content.lead.contains("free consultation"));
        assert!(text.contains("do not need a plan"));
        assert!(text.contains("what it costs before you decide"));
        assert!(!text.contains("Navigator"));
        // Scoped to company disputes, and saying so — without it the page
        // keeps drawing individual matters through a different door.
        for named in [
            "intellectual property",
            "employment",
            "investor",
            "We do not take personal injury",
        ] {
            assert!(text.contains(named), "disputes names {named:?}: {text}");
        }
        assert!(
            text.contains("We do not promise a result"),
            "no outcome promise: {text}"
        );
    }

    /// The services catalog remains brand-safe even though the route is
    /// gated off by `BrandKey::publishes_firm_path`.
    #[test]
    fn lawyer_shook_services_catalog_is_brand_keyed_and_attributed() {
        let branding = &views::brand::LAWYER_SHOOK_BRANDING;
        let services_content = legal_services(branding);
        assert!(services_content.hero_lead.contains("Shook Law PLLC"));
        assert!(!services_content.meta_description.contains("flat-fee"));
    }

    /// Every word one of these pages renders, flattened, so a claim placed in
    /// any field — a lead, a card body, a step, a chip — is visible to a
    /// guard here. Scoped to this test module rather than reused from
    /// `firm_copy::firm_copy_tests`, whose `band_text` guards the firm's own
    /// `/navigator` and `/services` pages, not a house brand's.
    fn dyd_page_text(content: &PageContent) -> String {
        fn paragraphs(body: &[Vec<Run>]) -> String {
            body.iter()
                .flat_map(|p| p.iter().map(|r| r.text.clone()))
                .collect::<Vec<_>>()
                .join(" ")
        }
        let bands = content
            .bands
            .iter()
            .map(|band| match band {
                Band::Statement {
                    heading,
                    lead,
                    body,
                } => {
                    format!("{heading} {lead} {}", paragraphs(body))
                }
                Band::Cards {
                    overline,
                    heading,
                    description,
                    items,
                    ..
                } => {
                    let cards = items
                        .iter()
                        .map(|c| {
                            format!("{} {} {}", c.title, c.chips.join(" "), paragraphs(&c.body))
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    format!(
                        "{overline} {heading} {} {cards}",
                        description.clone().unwrap_or_default()
                    )
                }
                Band::Steps {
                    overline,
                    heading,
                    description,
                    items,
                    ..
                } => {
                    let steps = items
                        .iter()
                        .map(|s| format!("{} {}", s.title, paragraphs(&s.body)))
                        .collect::<Vec<_>>()
                        .join(" ");
                    format!(
                        "{overline} {heading} {} {steps}",
                        description.clone().unwrap_or_default()
                    )
                }
                Band::Cta { heading, body, .. } => {
                    format!("{heading} {}", body.clone().unwrap_or_default())
                }
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join(" ");
        format!(
            "{} {} {} {bands}",
            content.tagline, content.hero_lead, content.meta_description
        )
    }

    #[test]
    fn privacy_offer_is_annual_and_monitoring_is_optional() {
        let branding = &views::brand::DELETE_YOUR_DATA_BRANDING;
        let content = home(branding);
        let privacy = content.privacy.expect("privacy offer");
        assert_eq!(privacy.price, "$50");
        assert_eq!(privacy.price_term, "/ year");
        assert!(privacy
            .benefits
            .iter()
            .any(|item| item[1].contains("Choose whether to opt in")));
        assert!(content.practices.is_empty());
        assert!(content.service.is_none());
        let text = serde_yaml::to_string(&privacy).expect("privacy copy serializes");
        assert!(!text.contains("$10"));
        assert!(!text.contains("we guarantee"));
        assert!(home(&views::brand::DEFAULT_BRANDING).privacy.is_none());
        assert!(home(&views::brand::VESTA_BRANDING).privacy.is_none());
        assert!(!dyd_page_text(&legal_services(branding)).contains("$10"));
    }

    #[test]
    fn neon_home_links_to_each_sibling_in_the_same_deployment() {
        let production = home_for_host(&views::brand::DEFAULT_BRANDING, Some("www.neonlaw.com"))
            .company
            .expect("Neon home has the company section");
        assert_eq!(
            production.immigration_href.as_deref(),
            Some("https://www.abhayaimmigration.com")
        );
        assert_eq!(
            production.estate_href.as_deref(),
            Some("https://www.vestaestateplanning.com")
        );

        let staging = home_for_host(&views::brand::DEFAULT_BRANDING, Some("staging.neonlaw.com"))
            .company
            .expect("Neon home has the company section");
        assert_eq!(
            staging.immigration_href.as_deref(),
            Some("https://staging.abhayaimmigration.com")
        );
        assert_eq!(
            staging.estate_href.as_deref(),
            Some("https://staging.vestaestateplanning.com")
        );
    }

    // --- ENG-744…749: the practice brands, and the lines that bind them ---

    /// Every word a practice brand publishes, home and services together, so
    /// a constraint is checked against the whole site rather than one page.
    fn brand_text(key: views::brand::BrandKey) -> String {
        let branding = key.resolve_branding(&views::brand::DEFAULT_BRANDING);
        let home = home(branding);
        let mut text = vec![
            home.head_title.clone(),
            home.meta_description.clone(),
            home.heading.clone(),
            home.lead.clone(),
        ];
        if let Some(service) = home.service.as_ref() {
            text.extend(service.body.iter().flatten().map(|run| run.text.clone()));
        }
        if let Some(estate) = home.estate.as_ref() {
            text.push(serde_yaml::to_string(estate).expect("estate copy serializes"));
        }
        for practice in &home.practices {
            text.push(practice.heading.clone());
            text.push(practice.body.clone());
        }
        text.push(dyd_page_text(&legal_services(branding)));
        text.push(branding.mission_description.to_string());
        text.push(branding.service_description.to_string());
        text.join(" ")
    }

    const PRACTICE_BRANDS: &[views::brand::BrandKey] = &[
        views::brand::BrandKey::Vesta,
        views::brand::BrandKey::Misericordia,
        views::brand::BrandKey::Abhaya,
        views::brand::BrandKey::DeleteYourDebt,
        views::brand::BrandKey::Summons,
    ];

    /// Every practice brand loads both catalogs it declares. A missing file
    /// is a test failure here rather than a panic on a visitor's first
    /// request.
    #[test]
    fn every_practice_brand_publishes_the_pages_it_declares() {
        for key in PRACTICE_BRANDS {
            let branding = key.resolve_branding(&views::brand::DEFAULT_BRANDING);
            assert_eq!(
                key.catalog_pages(),
                ["home", "services"],
                "{}",
                key.as_str()
            );
            assert!(
                !home(branding).heading.is_empty(),
                "{} home renders",
                key.as_str()
            );
            assert!(
                !dyd_page_text(&legal_services(branding)).is_empty(),
                "{} services renders",
                key.as_str()
            );
        }
    }

    /// No brand promises a result, anywhere. Attorney advertising rules make
    /// this the one claim none of these sites may make, whatever the
    /// practice area.
    #[test]
    fn no_practice_brand_promises_an_outcome() {
        for key in PRACTICE_BRANDS {
            let text = brand_text(*key).to_lowercase();
            for promise in [
                "we guarantee",
                "guaranteed result",
                "we will win",
                "you will win",
                "we always win",
                "guarantee that",
            ] {
                assert!(
                    !text.contains(promise),
                    "{} promises an outcome: {promise:?}",
                    key.as_str()
                );
            }
        }
    }

    /// **18 U.S.C. § 706.** The Misericordia of Florence carried a red cross,
    /// and that emblem later became the protected symbol of medical care. It
    /// is not available to this firm, in any form — so the brand's words
    /// never reach for it either, which is where it would creep back in once
    /// the palette is settled.
    #[test]
    fn misericordia_never_reaches_for_the_red_cross() {
        let text = brand_text(views::brand::BrandKey::Misericordia).to_lowercase();
        for emblem in ["red cross", "redcross", "cross symbol", "crimson cross"] {
            assert!(!text.contains(emblem), "Misericordia names {emblem:?}");
        }
    }

    /// No case results and no dollar figures on Misericordia. A headline
    /// number is not cured by the advertising disclaimer beneath it, so the
    /// fee page explains the contingency in words and publishes no amount.
    #[test]
    fn misericordia_publishes_no_recovery_figures() {
        let text = brand_text(views::brand::BrandKey::Misericordia);
        assert!(
            !text.contains('$'),
            "Misericordia publishes a dollar figure: {text}"
        );
        // Boasts, not the word "recover": describing *how the fee works* —
        // a percentage of what is recovered — is the honest disclosure this
        // practice owes. What is banned is a past result offered as a
        // prediction, which the advertising disclaimer does not cure.
        for boast in [
            "we have recovered",
            "we've recovered",
            "million",
            "verdict",
            "record settlement",
            "results speak",
            "won over",
        ] {
            assert!(
                !text.to_lowercase().contains(boast),
                "Misericordia publishes a case result: {boast:?}"
            );
        }
        // What it must say instead: the contingency, honestly, including the
        // losing case.
        assert!(text.contains("no fee unless we recover") || text.contains("owe us no attorney"));
    }

    /// Abhaya may never imply it can secure or speed a USCIS decision.
    #[test]
    fn abhaya_never_implies_it_controls_uscis() {
        let text = brand_text(views::brand::BrandKey::Abhaya).to_lowercase();
        for claim in [
            "we can get you a visa",
            "guaranteed approval",
            "fast-track",
            "expedite your case",
            "we can speed",
            "approval is certain",
        ] {
            assert!(!text.contains(claim), "Abhaya implies {claim:?}");
        }
        assert!(
            text.contains("cannot promise"),
            "and says so plainly: {text}"
        );
    }

    /// `DeleteYourDebt` is collection defense. Settlement framing pulls in a
    /// different regulatory regime — the FTC Telemarketing Sales Rule's
    /// advance-fee provisions and state debt-adjuster licensing — whose
    /// attorney exemption is narrower than it is usually assumed to be.
    ///
    /// The banned phrases are allowed only inside an explicit denial, which
    /// is how the site tells a reader it is *not* that service. So this
    /// asserts on sentences, not on the page: a phrase may appear only where
    /// "not" or "do not" appears with it.
    #[test]
    fn delete_your_debt_only_names_settlement_to_disclaim_it() {
        let text = brand_text(views::brand::BrandKey::DeleteYourDebt);
        for sentence in text.split('.') {
            let lowered = sentence.to_lowercase();
            let claims_settlement = [
                "settle your debt",
                "reduce what you owe",
                "negotiate your balance",
                "pennies on the dollar",
                "eliminate your debt",
            ]
            .iter()
            .any(|phrase| lowered.contains(phrase));
            if claims_settlement {
                assert!(
                    lowered.contains("not") || lowered.contains("do not"),
                    "settlement framing outside a denial: {sentence:?}"
                );
            }
        }
        assert!(
            text.contains("We do not settle debts")
                || text.contains("does not settle debts")
                || text.contains("We are not a debt settlement"),
            "and the disclaimer is actually present: {text}"
        );
    }

    /// The Summons catalog must not read as the tribunal it appears before.
    /// Keep this invariant at the locale boundary: inspect the catalog keys
    /// and values directly, while route and rendered-page behavior belongs to
    /// the firm-page and server tests.
    #[test]
    fn the_summons_catalog_disclaims_affiliation_with_the_city() {
        let value = |page: &str, key: &str| {
            let yaml = catalog_yaml(BrandKey::Summons, page).expect("Summons catalog page");
            let document: serde_yaml::Value =
                serde_yaml::from_str(yaml).expect("Summons catalog parses");
            document[key]
                .as_str()
                .unwrap_or_else(|| panic!("Summons {page} catalog key {key:?} is text"))
                .to_string()
        };

        let home_lead = value("home", "lead");
        assert!(
            home_lead.contains("not affiliated with the City of New York"),
            "home.lead: {home_lead}"
        );
        let services_hero_lead = value("services", "hero_lead");
        assert!(
            services_hero_lead.contains("not affiliated with the City of New York"),
            "services.hero_lead: {services_hero_lead}"
        );
    }

    /// Every public brand says whose practice it is. That disclosure is what
    /// keeps a trade name from being misleading, so it is derived from the
    /// brand rather than listed.
    #[test]
    fn every_public_brand_names_the_firm_behind_it() {
        for key in views::brand::BrandKey::ALL {
            let branding = key.resolve_branding(&views::brand::DEFAULT_BRANDING);
            assert_eq!(
                branding.firm.legal_entity,
                "Shook Law PLLC",
                "{} names the firm",
                key.as_str()
            );
            assert_ne!(
                branding.firm.site_name,
                branding.firm.legal_entity,
                "{} publishes a public brand name distinct from its legal entity",
                key.as_str()
            );
        }
    }

    /// Nothing in this family loads a third-party font or analytics host.
    #[test]
    fn no_practice_brand_copy_references_a_third_party_host() {
        for key in PRACTICE_BRANDS {
            let text = brand_text(*key).to_lowercase();
            for host in [
                "fonts.googleapis.com",
                "fonts.gstatic.com",
                "google-analytics",
                "googletagmanager",
                "facebook.net",
            ] {
                assert!(!text.contains(host), "{} references {host}", key.as_str());
            }
        }
    }
}
