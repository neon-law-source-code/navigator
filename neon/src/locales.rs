//! Load each house brand's English marketing catalog and map it onto page types.
//!
//! The words live in `locales/en/<brand-key>/*.yaml`. This module is the only
//! Rust that reads them: pick the directory for the request's `BrandKey`,
//! interpolate the brand placeholders, deserialize, and fill the few runtime
//! fields a YAML file cannot know (the hero asset URL, the CLI release
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
use webapp::marketing_page::{
    Band, Card, Download, HeroCta, PackageInstall, PageContent, ProjectNetworkNode, Run, Step,
};

const NEON_HOME_YAML: &str = include_str!("../locales/en/neon/home.yaml");
const NEON_LITIGATION_YAML: &str = include_str!("../locales/en/neon/litigation.yaml");
const NEON_FRACTIONAL_GC_YAML: &str = include_str!("../locales/en/neon/fractional-gc.yaml");
const NEON_PERSONAL_PLAN_YAML: &str = include_str!("../locales/en/neon/personal-plan.yaml");
const NEON_NAVIGATOR_YAML: &str = include_str!("../locales/en/neon/navigator.yaml");
const NEON_SERVICES_YAML: &str = include_str!("../locales/en/neon/services.yaml");
const DELETE_YOUR_DATA_HOME_YAML: &str = include_str!("../locales/en/delete-your-data/home.yaml");
const DELETE_YOUR_DATA_SERVICES_YAML: &str =
    include_str!("../locales/en/delete-your-data/services.yaml");
const LAWYER_SHOOK_SERVICES_YAML: &str = include_str!("../locales/en/lawyer-shook/services.yaml");

/// The shipped YAML for `key`'s `page` stem, if that brand publishes it.
#[must_use]
pub fn catalog_yaml(key: BrandKey, page: &str) -> Option<&'static str> {
    match (key, page) {
        (BrandKey::Neon, "home") => Some(NEON_HOME_YAML),
        (BrandKey::Neon, "litigation") => Some(NEON_LITIGATION_YAML),
        (BrandKey::Neon, "fractional-gc") => Some(NEON_FRACTIONAL_GC_YAML),
        (BrandKey::Neon, "personal-plan") => Some(NEON_PERSONAL_PLAN_YAML),
        (BrandKey::Neon, "navigator") => Some(NEON_NAVIGATOR_YAML),
        (BrandKey::Neon, "services") => Some(NEON_SERVICES_YAML),
        (BrandKey::DeleteYourData, "home") => Some(DELETE_YOUR_DATA_HOME_YAML),
        (BrandKey::DeleteYourData, "services") => Some(DELETE_YOUR_DATA_SERVICES_YAML),
        (BrandKey::LawyerShook, "services") => Some(LAWYER_SHOOK_SERVICES_YAML),
        _ => None,
    }
}

/// Load one catalog file as `T`, after substituting the mounted brand.
fn load<T: serde::de::DeserializeOwned>(yaml: &str, branding: &views::brand::Branding) -> T {
    let raw = interpolate(yaml, branding.firm.site_name, branding.firm_email);
    serde_yaml::from_str(&raw)
        .expect("invariant: shipped locale YAML deserializes; navigator validate Y002 is the gate")
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
/// than the two denominations the firm has published so far) renders no
/// badge at all, same as no amount.
fn resolve_day_rate(amount: Option<u16>) -> Option<DayRateBadge> {
    let amount = amount?;
    let key = match amount {
        10 => "img/ten-dollar-bill/ten-dollar-bill.jpg",
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

fn band(copy: BandCopy) -> Band {
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
        BandCopy::ProjectNetwork {
            anchor,
            overline,
            heading,
            description,
            left,
            right,
            mcp_tools,
            agentic_coding_tools,
            saas_tools,
        } => Band::ProjectNetwork {
            anchor,
            overline,
            heading,
            description,
            left: left.into_iter().map(network_node).collect(),
            right: right.into_iter().map(network_node).collect(),
            mcp_tools,
            agentic_coding_tools,
            saas_tools,
        },
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

fn marketing_page(copy: MarketingPageCopy) -> PageContent {
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
        bands: copy.bands.into_iter().map(band).collect(),
    }
}

/// The firm home page, resolved from this brand's `home.yaml`.
pub fn home(branding: &views::brand::Branding) -> webapp::home::HomeContent {
    let copy: HomeCopy = load_page(branding, "home");
    webapp::home::HomeContent {
        head_title: copy.head_title,
        meta_description: copy.meta_description,
        hero: copy.hero.map(|hero| webapp::home::HeroPicture {
            sources: Vec::new(),
            fallback_src: views::assets::asset_url(&hero.asset),
            alt: hero.alt,
            sizes: "100vw".to_string(),
        }),
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

/// The `/litigation` page, resolved from this brand's `litigation.yaml`.
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

/// The `/fractional-gc` page, resolved from this brand's `fractional-gc.yaml`.
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

/// `/personal-plan`, from this brand's `personal-plan.yaml`.
pub fn personal_plan(branding: &views::brand::Branding) -> PageContent {
    marketing_page(load_page(branding, "personal-plan"))
}

/// `/navigator`, from this brand's `navigator.yaml`.
pub fn navigator(branding: &views::brand::Branding) -> PageContent {
    marketing_page(load_page(branding, "navigator"))
}

/// `/services`, from this brand's `services.yaml`.
pub fn legal_services(branding: &views::brand::Branding) -> PageContent {
    marketing_page(load_page(branding, "services"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use views::brand::BrandKey;
    use views::locales::parse_locale_file;

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
        assert_eq!(content.heading, "What is your legal need?");
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
                "Business plan",
                "Personal plan",
                "Individual services",
                "Disputes",
            ]
        );
    }

    /// `/fractional-gc` publishes its base package as a $10-a-day retainer;
    /// the bill photo the hero and fee section draw reads that same figure
    /// and resolves through the asset seam rather than a bare filename.
    #[test]
    fn fractional_gc_publishes_its_ten_dollar_day_rate() {
        let content = fractional_gc(&views::brand::DEFAULT_BRANDING);
        let offer = content.pricing.first().expect("the Business plan offer");
        assert_eq!(offer.price, "$3,650");
        assert_eq!(offer.cadence.as_deref(), Some("/year"));
        for benefit in [
            "company records",
            "hiring employees and contractors",
            "within three business days",
            "who owns your company",
            "business information private",
            "business taxes and state paperwork",
        ] {
            assert!(
                offer.features.iter().any(|feature| feature.contains(benefit)),
                "the Business plan names {benefit:?}: {:?}",
                offer.features
            );
        }
        let badge = content
            .pricing
            .first()
            .and_then(|offer| offer.day_rate.clone());
        assert_eq!(
            badge.as_ref().map(|badge| badge.amount),
            Some(10),
            "the base package states its day rate as a figure, not only in the blurb sentence"
        );
        assert!(
            badge.is_some_and(|badge| badge.image_src.contains("ten-dollar-bill")),
            "the badge resolves the $10 bill photo"
        );
    }

    /// `/personal-plan` publishes its one flat fee as a $1-a-day plan.
    #[test]
    fn personal_plan_publishes_its_one_dollar_day_rate() {
        let content = personal_plan(&views::brand::DEFAULT_BRANDING);
        let plan = content
            .bands
            .iter()
            .find_map(|band| match band {
                webapp::marketing_page::Band::Cards { items, pricing_style, .. }
                    if *pricing_style => items.first(),
                _ => None,
            })
            .expect("the Personal plan offer");
        assert_eq!(plan.chips.first().map(String::as_str), Some("$365"));
        assert_eq!(plan.cadence.as_deref(), Some("/year"));
        assert!(plan.features.contains(&"Optional credit monitoring".to_string()));
        let day_rate = content.bands.iter().find_map(|band| match band {
            webapp::marketing_page::Band::Cards { items, .. } => {
                items.first().and_then(|card| card.day_rate.clone())
            }
            _ => None,
        });
        assert!(
            day_rate
                .as_ref()
                .is_some_and(|badge| badge.image_src.contains("one-dollar-bill")),
            "the badge resolves the $1 bill photo"
        );
        assert_eq!(
            day_rate.map(|badge| badge.amount),
            Some(1),
            "the plan states its day rate as a figure, not only in the blurb sentence"
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

    /// The `DeleteYourData` home and services pages both publish the flat
    /// removal-request fee and the Neon Law Personal Plan as the way to get
    /// it at no added cost, and neither page still tells a reader every
    /// request is quoted — the pre-existing framing this fee contradicted.
    #[test]
    fn delete_your_data_publishes_its_flat_fee_and_the_personal_plan_link() {
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
        let personal_plan_run = home_service
            .body
            .iter()
            .flatten()
            .find(|run| run.href.as_deref() == Some("https://www.neonlaw.com/personal-plan"))
            .expect("a run links the Neon Law Personal Plan");
        assert_eq!(personal_plan_run.text, "Neon Law Personal Plan");
        // The linked run's own text carries no leading/trailing run-boundary
        // artifact — the bug this test would have caught twice while this
        // paragraph was drafted.
        assert!(
            !home_text.contains("Planmember"),
            "run boundaries: {home_text}"
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
            services_text.contains("Personal Plan"),
            "the services page names the Personal Plan alternative: {services_text}"
        );
        assert!(
            !services_text.to_lowercase().contains("fees are quoted before work begins"),
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
}
