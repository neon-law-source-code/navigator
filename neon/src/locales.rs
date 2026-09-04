//! Load each house brand's English marketing catalog and map it onto page types.
//!
//! The words live in `locales/en/<brand-key>/*.yaml`. This module is the only
//! Rust that reads them: pick the directory for the request's `BrandKey`,
//! interpolate the brand placeholders, deserialize, and fill the few runtime
//! fields a YAML file cannot know (the hero asset URL, the CLI release
//! archives). Editing published copy is a YAML change.

use views::brand::BrandKey;
use views::locales::{
    interpolate, BandCopy, CardCopy, CopyRun, HeroCtaCopy, HeroLine, HomeCopy, IncludedCopy,
    LitigationCopy, MarketingPageCopy, PackageInstallCopy, PageSkin, Paragraph, PracticeLinkCopy,
    PracticeMark, PricingCardCopy, ProjectNetworkNodeCopy, SalesStageCopy, SeparateWorkCopy,
    ServiceSectionCopy, StepCopy, TransactionalCopy, VirtueCopy,
};
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

fn card(copy: CardCopy) -> Card {
    Card {
        title: copy.title,
        chips: copy.chips,
        body: paragraphs_to_marketing(copy.body),
        href: copy.href,
        href_label: copy.href_label,
        cadence: copy.cadence,
        features: copy.features,
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
        msa_term: copy.msa_term,
        msa_definition: copy.msa_definition,
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
                 }| webapp::transactional_page::PricingOffer {
                    title,
                    price,
                    cadence,
                    blurb,
                    features,
                },
            )
            .collect(),
        availability_note: copy.availability_note,
        included_heading: copy.included_heading,
        included: copy
            .included
            .into_iter()
            .map(|IncludedCopy { name, body }| webapp::transactional_page::Included { name, body })
            .collect(),
        cycle_heading: copy.cycle_heading,
        cycle_body: copy.cycle_body,
        cycle: copy
            .cycle
            .into_iter()
            .map(
                |SalesStageCopy { stage, legal_step }| webapp::transactional_page::SalesStage {
                    stage,
                    legal_step,
                },
            )
            .collect(),
        separate_heading: copy.separate_heading,
        separate_body: copy.separate_body,
        separate: copy
            .separate
            .into_iter()
            .map(
                |SeparateWorkCopy {
                     name,
                     body,
                     href,
                     link_label,
                 }| webapp::transactional_page::SeparateWork {
                    name,
                    body,
                    href,
                    link_label,
                },
            )
            .collect(),
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
        assert_eq!(content.heading, "Everyone deserves to be seen.");
        assert_eq!(
            content
                .practices
                .iter()
                .map(|practice| practice.heading.as_str())
                .collect::<Vec<_>>(),
            [
                "Litigation",
                "Fractional GC",
                "Personal Plan",
                "One-Time Services",
            ]
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
}
