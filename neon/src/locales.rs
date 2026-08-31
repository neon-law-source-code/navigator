//! Load the firm's English marketing catalog and map it onto page types.
//!
//! The words live in `locales/en/*.yaml`. This module is the only Rust that
//! reads them: interpolate the brand placeholders, deserialize, and fill the
//! few runtime fields a YAML file cannot know (the hero asset URL, the CLI
//! release archives). Editing published copy is a YAML change.

use views::locales::{
    interpolate, BandCopy, CardCopy, CopyRun, HeroCtaCopy, HeroLine, HomeCopy, IncludedCopy,
    LitigationCopy, MarketingPageCopy, PackageInstallCopy, PageSkin, Paragraph, PracticeLinkCopy,
    PracticeMark, ProjectNetworkNodeCopy, SalesStageCopy, SeparateWorkCopy, ServiceSectionCopy,
    StepCopy, TransactionalCopy, VirtueCopy,
};
use webapp::marketing_page::{
    Band, Card, Download, HeroCta, PackageInstall, PageContent, ProjectNetworkNode, Run, Step,
};

const HOME_YAML: &str = include_str!("../locales/en/home.yaml");
const LITIGATION_YAML: &str = include_str!("../locales/en/litigation.yaml");
const FRACTIONAL_GC_YAML: &str = include_str!("../locales/en/fractional-gc.yaml");
const FRACTIONAL_CTO_YAML: &str = include_str!("../locales/en/fractional-cto.yaml");
const NAVIGATOR_YAML: &str = include_str!("../locales/en/navigator.yaml");
const SERVICES_YAML: &str = include_str!("../locales/en/services.yaml");

/// Load one catalog file as `T`, after substituting the mounted brand.
fn load<T: serde::de::DeserializeOwned>(yaml: &str, branding: &views::brand::Branding) -> T {
    let raw = interpolate(yaml, branding.firm.site_name, branding.firm_email);
    serde_yaml::from_str(&raw)
        .expect("invariant: shipped locale YAML deserializes; navigator validate Y002 is the gate")
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
        } => Band::Cards {
            anchor,
            overline,
            heading,
            description,
            items: items.into_iter().map(card).collect(),
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

/// The firm home page, resolved from `locales/en/home.yaml`.
pub fn home(branding: &views::brand::Branding) -> webapp::home::HomeContent {
    let copy: HomeCopy = load(HOME_YAML, branding);
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

/// The `/litigation` page, resolved from `locales/en/litigation.yaml`.
pub fn litigation(branding: &views::brand::Branding) -> webapp::litigation_page::LitigationContent {
    let copy: LitigationCopy = load(LITIGATION_YAML, branding);
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

/// The `/fractional-gc` page, resolved from `locales/en/fractional-gc.yaml`.
pub fn fractional_gc(
    branding: &views::brand::Branding,
) -> webapp::transactional_page::TransactionalContent {
    let copy: TransactionalCopy = load(FRACTIONAL_GC_YAML, branding);
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

/// `/fractional-cto`, from `locales/en/fractional-cto.yaml`.
pub fn fractional_cto() -> PageContent {
    marketing_page(load(FRACTIONAL_CTO_YAML, &views::brand::DEFAULT_BRANDING))
}

/// `/navigator`, from `locales/en/navigator.yaml`.
pub fn navigator() -> PageContent {
    marketing_page(load(NAVIGATOR_YAML, &views::brand::DEFAULT_BRANDING))
}

/// `/services`, from `locales/en/services.yaml`.
pub fn legal_services() -> PageContent {
    marketing_page(load(SERVICES_YAML, &views::brand::DEFAULT_BRANDING))
}

#[cfg(test)]
mod tests {
    use super::*;
    use views::locales::parse_locale_file;

    /// Every shipped English catalog file deserializes as the page it names.
    #[test]
    fn every_english_catalog_deserializes() {
        for (stem, yaml) in [
            ("home", HOME_YAML),
            ("litigation", LITIGATION_YAML),
            ("fractional-gc", FRACTIONAL_GC_YAML),
            ("fractional-cto", FRACTIONAL_CTO_YAML),
            ("navigator", NAVIGATOR_YAML),
            ("services", SERVICES_YAML),
        ] {
            parse_locale_file(stem, yaml).unwrap_or_else(|err| panic!("{stem}: {err}"));
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
    }
}
