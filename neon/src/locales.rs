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
const NEON_NAVIGATOR_YAML: &str = include_str!("../locales/en/neon/navigator.yaml");
const NEON_SERVICES_YAML: &str = include_str!("../locales/en/neon/services.yaml");
/// The firm's individual services as records. Only Neon publishes one; the
/// other house brands render `/services` without an individual-services band.
const NEON_SERVICES_CATALOG_YAML: &str = include_str!("../locales/en/neon/services-catalog.yaml");
const DELETE_YOUR_DATA_HOME_YAML: &str = include_str!("../locales/en/delete-your-data/home.yaml");
const VESTA_HOME_YAML: &str = include_str!("../locales/en/vesta/home.yaml");
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
        center_detail: network_label(center_detail, "Web API MCP CLI"),
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

/// The firm home page, resolved from this brand's `home.yaml`.
pub fn home(branding: &views::brand::Branding) -> webapp::home::HomeContent {
    let copy: HomeCopy = load_page(branding, "home");
    webapp::home::HomeContent {
        head_title: copy.head_title,
        meta_description: copy.meta_description,
        heading: copy.heading,
        lead: copy.lead,
        contact_href: format!("mailto:{}", branding.firm_email),
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
                },
            )
            .collect(),
        provenance: copy.provenance.map(provenance_to_home),
        // Every brand that loads a `home.yaml` publishes an ordinary
        // marketing page; only Lawyer Shook's hardcoded holding statement
        // (`neon::firm_pages::lawyer_shook_holding_content`) sets `bare`.
        bare: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use views::brand::BrandKey;
    use views::locales::parse_locale_file;
    use webapp::marketing_page::Band as RenderedBand;

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
    fn the_shared_catalog_is_the_source_the_neon_pages_read() {
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
        for key in shared_catalog().keys() {
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
        assert_eq!(content.heading, "What does your technology company need?");
        assert!(content
            .service
            .as_ref()
            .is_some_and(|service| service.heading == "Everyone deserves to be seen."));
        assert_eq!(
            content
                .practices
                .iter()
                .map(|practice| practice.heading.as_str())
                .collect::<Vec<_>>(),
            [
                "Fractional general counsel",
                "Individual services",
                "Disputes",
            ],
            "the plan chooser leads with the counsel relationship and no \
             longer offers a consumer plan"
        );
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

    #[test]
    fn delete_your_data_home_catalog_names_its_own_heading() {
        let content = home(&views::brand::DELETE_YOUR_DATA_BRANDING);
        assert!(content
            .head_title
            .contains(views::brand::DELETE_YOUR_DATA_BRANDING.firm.site_name));
        assert_eq!(content.heading, "Ask companies to delete your data.");
        assert!(content.lead.contains("Shook Law PLLC"));
        assert!(!content.heading.contains("Everyone deserves to be seen."));
        assert_eq!(
            content
                .practices
                .iter()
                .map(|practice| practice.heading.as_str())
                .collect::<Vec<_>>(),
            ["Data-deletion requests"]
        );
    }

    /// Every word the provenance section publishes, for the advertising checks.
    fn provenance_text(provenance: &webapp::home::ProvenanceSection) -> String {
        let steps = provenance
            .steps
            .iter()
            .map(|step| format!("{} {}", step.label, step.detail))
            .collect::<Vec<_>>()
            .join(" ");
        let pillars = provenance
            .pillars
            .iter()
            .map(|pillar| format!("{} {}", pillar.heading, pillar.body))
            .collect::<Vec<_>>()
            .join(" ");
        let notes = provenance
            .notes
            .iter()
            .flatten()
            .map(|run| run.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        format!(
            "{} {} {} {steps} {} {pillars} {notes}",
            provenance.heading,
            provenance.heading_accent,
            provenance.lead,
            provenance.ledger_caption
        )
        .to_lowercase()
    }

    /// The `DeleteYourData` home says what happens to a request after a lawyer
    /// verifies it — a record uploaded to Solana — and what the lawyer-attested
    /// nodes are for. The other two brands keep no such record and publish no
    /// such section.
    #[test]
    fn delete_your_data_home_carries_the_solana_provenance_section() {
        let content = home(&views::brand::DELETE_YOUR_DATA_BRANDING);
        let provenance = content
            .provenance
            .expect("the DeleteYourData home publishes its provenance section");
        assert_eq!(
            provenance
                .steps
                .iter()
                .map(|step| step.mark)
                .collect::<Vec<_>>(),
            [
                webapp::home::ProvenanceMark::Request,
                webapp::home::ProvenanceMark::Attorney,
                webapp::home::ProvenanceMark::Chain,
            ],
            "request, verify, record — in that order"
        );
        assert!(!provenance.ledger.is_empty(), "the ledger has rows to draw");
        let text = provenance_text(&provenance);
        assert!(
            text.contains("verif"),
            "the request is verified first: {text}"
        );
        assert!(text.contains("solana"), "the record goes to Solana: {text}");
        assert!(
            text.contains("lawyer-attested nodes") && text.contains("provenance"),
            "the nodes are named as long-term provenance: {text}"
        );
        assert!(
            text.contains("attorney advertisement"),
            "the section carries the advertising notice: {text}"
        );
        assert!(
            home(&views::brand::DEFAULT_BRANDING).provenance.is_none(),
            "Neon keeps no removal record and publishes no section"
        );
        // Lawyer Shook's `/` no longer loads through this catalog loader at
        // all (it is a hardcoded bare statement — see
        // `firm_pages::lawyer_shook_holding_content`), so this checks the
        // resolved page rather than `home()` directly.
        assert!(
            crate::firm_pages::resolve_firm_home_content(&views::brand::LAWYER_SHOOK_BRANDING)
                .provenance
                .is_none(),
            "Lawyer Shook keeps no removal record and publishes no section"
        );
    }

    /// The three tiles say what the record is for — privacy, security, and the
    /// federated work of the nodes — and, like the rest of the section, make no
    /// claim a lawyer cannot defend: a record that a request was made is not a
    /// promise about what the company did with it, and the chain is described
    /// without the superlatives a chain's own front page reaches for.
    #[test]
    fn the_provenance_section_upsells_without_an_indefensible_claim() {
        let provenance = home(&views::brand::DELETE_YOUR_DATA_BRANDING)
            .provenance
            .expect("the DeleteYourData home publishes its provenance section");
        let headings = provenance
            .pillars
            .iter()
            .map(|pillar| pillar.heading.to_lowercase())
            .collect::<Vec<_>>();
        for pillar in ["privacy", "security", "federated"] {
            assert!(
                headings.iter().any(|heading| heading.contains(pillar)),
                "a {pillar:?} tile: {headings:?}"
            );
        }
        let text = provenance_text(&provenance);
        for banned in [
            "guarantee",
            "certified",
            "permanent",
            "tamper-proof",
            "immutable",
            "leading",
            "fastest",
            "world's",
        ] {
            assert!(!text.contains(banned), "no {banned:?} claim: {text}");
        }
    }

    /// Lawyer Shook's `/` is no longer a catalog page (see
    /// [`crate::firm_pages::lawyer_shook_holding_content`]), so this only
    /// covers `/services` — still a real YAML catalog page even though the
    /// route itself is gated off by `BrandKey::publishes_firm_path`.
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

    /// The `DeleteYourData` home and services pages publish the flat
    /// removal-request fee, and neither still tells a reader every request is
    /// quoted — the pre-existing framing that fee contradicted.
    ///
    /// They also no longer offer the request "at no added cost" to a Neon Law
    /// Personal plan member. That plan is retired, so the clause described an
    /// offer nobody could take up — and it is asserted absent here because it
    /// was *this brand's* published price, not a stale link: a dead
    /// cross-brand offer is a pricing defect, and it would have survived a
    /// link check.
    #[test]
    fn delete_your_data_publishes_its_flat_fee_without_the_retired_plan_offer() {
        let branding = &views::brand::DELETE_YOUR_DATA_BRANDING;
        let home_content = home(branding);
        let services_content = legal_services(branding);

        let home_service = home_content.service.expect("the home service section");
        let home_text = home_service
            .body
            .iter()
            .flatten()
            .map(|run| run.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            home_text.contains("$10"),
            "the home page states the fee: {home_text}"
        );
        assert!(
            !home_service
                .body
                .iter()
                .flatten()
                .any(|run| run.href.as_deref() == Some("https://www.neonlaw.com/personal")),
            "the retired plan is not linked: {home_text}"
        );
        assert!(
            !home_text.contains("Personal Plan"),
            "nor named: {home_text}"
        );

        let practice_bodies = home_content
            .practices
            .iter()
            .map(|practice| practice.body.as_str())
            .collect::<Vec<_>>();
        assert!(
            practice_bodies.iter().any(|body| body.contains("$10")),
            "the practice box states the fee: {practice_bodies:?}"
        );

        let services_text = dyd_page_text(&services_content);
        assert!(
            services_text.contains("$10"),
            "the services page states the fee: {services_text}"
        );
        assert!(
            !services_text.contains("Personal Plan"),
            "the services page no longer offers the retired plan: {services_text}"
        );
        assert!(
            !services_text
                .to_lowercase()
                .contains("fees are quoted before work begins"),
            "the blanket quoted-only claim is gone now that a flat fee is published: {services_text}"
        );

        // The `$10` chip on the Removal Request card is the one this page
        // already shipped (PR #358); the surrounding prose must agree with
        // it rather than call every request a bespoke quote.
        let removal_request = services_content
            .bands
            .iter()
            .find_map(|band| match band {
                Band::Cards { items, .. } => {
                    items.iter().find(|card| card.title == "Removal Request")
                }
                _ => None,
            })
            .expect("the Removal Request card");
        assert_eq!(removal_request.chips, vec!["$10".to_string()]);
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

    /// The NYC summons practice must not read as the tribunal it appears
    /// before. OATH runs a free Help Center, so an implied affiliation is a
    /// Rule 7.1 problem rather than a trademark one — and the domain carries
    /// the agency's name, which is exactly why the line has to be in the
    /// copy rather than left to the domain.
    #[test]
    fn the_summons_practice_disclaims_affiliation_with_the_city() {
        let branding =
            views::brand::BrandKey::Summons.resolve_branding(&views::brand::DEFAULT_BRANDING);

        // On the home page and the services page, not merely somewhere.
        let home_text = {
            let home = home(branding);
            let mut parts = vec![home.lead.clone(), home.meta_description.clone()];
            if let Some(service) = home.service.as_ref() {
                parts.extend(service.body.iter().flatten().map(|run| run.text.clone()));
            }
            parts.join(" ")
        };
        assert!(
            home_text.contains("not affiliated with the City of New York"),
            "home: {home_text}"
        );
        let services_text = dyd_page_text(&legal_services(branding));
        assert!(
            services_text.contains("not affiliated with"),
            "services: {services_text}"
        );

        // New York Rule 7.5(b) bars a trade name for private practice, so the
        // masthead is the firm itself rather than a brand.
        assert_eq!(branding.firm.site_name, "Shook Law PLLC");
    }

    /// Every brand that wears a name other than the firm's says whose
    /// practice it is. That disclosure is what keeps a Nevada trade name from
    /// being misleading, so it is derived from the brand rather than listed.
    #[test]
    fn a_trade_name_brand_names_the_firm_behind_it() {
        for key in views::brand::BrandKey::ALL {
            let branding = key.resolve_branding(&views::brand::DEFAULT_BRANDING);
            assert_eq!(
                branding.firm.legal_entity,
                "Shook Law PLLC",
                "{} names the firm",
                key.as_str()
            );
            // A brand whose masthead already *is* the firm needs no further
            // disclosure; every other one is owed the footer's "A practice
            // of Shook Law PLLC". That line is derived from exactly this
            // comparison in `webapp::public_chrome`, and asserted there —
            // what this test pins is that the comparison has a stable
            // answer, i.e. every brand agrees on who the firm is.
            if branding.firm.site_name == branding.firm.legal_entity {
                assert_eq!(
                    key.as_str(),
                    "summons",
                    "only the NY practice wears the firm's own name"
                );
            }
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
