//! English marketing-copy catalogs for a brand crate.
//!
//! Public page copy lives in `locales/en/*.yaml` beside the brand that publishes
//! it. The site still publishes one language: these files are an authoring
//! catalog, not a translated surface. `{site_name}` and `{firm_email}` are the
//! only substitutions; everything a visitor reads is otherwise the YAML.
//!
//! [`parse_locale_file`] is the typed check `navigator validate` runs so a
//! copy-only edit cannot land a document the brand crate cannot load.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// The only locale directory the site publishes.
pub const DEFAULT_LOCALE: &str = "en";

/// Page stems the English catalog may hold.
pub const KNOWN_PAGES: &[&str] = &[
    "fractional-cto",
    "fractional-gc",
    "home",
    "litigation",
    "navigator",
    "services",
];

/// Replace the two brand placeholders a catalog file may carry.
#[must_use]
pub fn interpolate(raw: &str, site_name: &str, firm_email: &str) -> String {
    raw.replace("{site_name}", site_name)
        .replace("{firm_email}", firm_email)
}

/// One run of published prose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CopyRun {
    pub text: String,
    #[serde(default)]
    pub emphasis: bool,
    #[serde(default)]
    pub href: Option<String>,
}

/// A paragraph is the runs that compose it.
pub type Paragraph = Vec<CopyRun>;

/// A hero statement plus how many leading words take the brand colour.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HeroLine {
    pub text: String,
    #[serde(default)]
    pub accent_words: usize,
}

/// The decorative mark a practice page or card opens on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PracticeMark {
    #[default]
    Scales,
    Handshake,
    Gavel,
    Technology,
    Helm,
}

/// Which visual language a marketing page wears.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PageSkin {
    #[default]
    Marketing,
    Practice,
}

/// The firm home page catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomeCopy {
    pub head_title: String,
    pub meta_description: String,
    pub heading: String,
    pub lead: String,
    pub contact_label: String,
    #[serde(default)]
    pub hero: Option<HomeHeroCopy>,
    #[serde(default)]
    pub service: Option<ServiceSectionCopy>,
    #[serde(default)]
    pub practices_heading: String,
    #[serde(default)]
    pub practices: Vec<PracticeLinkCopy>,
}

/// The home hero photograph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomeHeroCopy {
    pub alt: String,
    pub asset: String,
}

/// The home page's one service section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceSectionCopy {
    pub heading: String,
    pub body: Vec<Paragraph>,
}

/// One practice box at the foot of the home page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PracticeLinkCopy {
    pub mark: PracticeMark,
    pub heading: String,
    pub body: String,
    pub href: String,
}

/// The `/litigation` catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LitigationCopy {
    pub head_title: String,
    pub meta_description: String,
    pub eyebrow: String,
    pub heading: HeroLine,
    pub lead: String,
    pub cta_label: String,
    pub body: Vec<Paragraph>,
}

/// One of the three words the fractional-GC practice is named by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtueCopy {
    pub word: String,
    pub body: String,
}

/// One line item the monthly fee covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncludedCopy {
    pub name: String,
    pub body: String,
}

/// One sales-cycle stage and the legal step inside it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SalesStageCopy {
    pub stage: String,
    pub legal_step: String,
}

/// Work quoted outside the monthly retainer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeparateWorkCopy {
    pub name: String,
    pub body: String,
    #[serde(default)]
    pub href: Option<String>,
    #[serde(default)]
    pub link_label: Option<String>,
}

/// The `/fractional-gc` catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionalCopy {
    pub head_title: String,
    pub meta_description: String,
    pub eyebrow: String,
    pub heading: HeroLine,
    pub lead: String,
    pub cta_label: String,
    pub virtues: Vec<VirtueCopy>,
    pub msa_term: String,
    pub msa_definition: String,
    pub fee_heading: String,
    pub fee_body: String,
    pub included_heading: String,
    pub included: Vec<IncludedCopy>,
    pub cycle_heading: String,
    pub cycle_body: String,
    pub cycle: Vec<SalesStageCopy>,
    pub separate_heading: String,
    pub separate_body: String,
    pub separate: Vec<SeparateWorkCopy>,
}

/// The hero's one call to action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HeroCtaCopy {
    pub href: String,
    pub label: String,
}

/// One card in a card band.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CardCopy {
    pub title: String,
    #[serde(default)]
    pub chips: Vec<String>,
    #[serde(default)]
    pub body: Vec<Paragraph>,
    #[serde(default)]
    pub href: Option<String>,
    #[serde(default)]
    pub href_label: Option<String>,
}

/// One entry in a numbered walk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StepCopy {
    pub title: String,
    #[serde(default)]
    pub body: Vec<Paragraph>,
}

/// One labeled place in a Project's connected-work diagram.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProjectNetworkNodeCopy {
    pub label: String,
    pub detail: String,
}

/// The package-manager route beside the download boxes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PackageInstallCopy {
    pub heading: String,
    #[serde(default)]
    pub body: Vec<Paragraph>,
}

/// One horizontal band of a marketing page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BandCopy {
    Statement {
        heading: String,
        #[serde(default)]
        lead: String,
        #[serde(default)]
        body: Vec<Paragraph>,
    },
    Cards {
        #[serde(default)]
        anchor: String,
        overline: String,
        heading: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        items: Vec<CardCopy>,
    },
    Steps {
        #[serde(default)]
        anchor: String,
        overline: String,
        heading: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        items: Vec<StepCopy>,
    },
    ProjectNetwork {
        #[serde(default)]
        anchor: String,
        overline: String,
        heading: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        left: Vec<ProjectNetworkNodeCopy>,
        #[serde(default)]
        right: Vec<ProjectNetworkNodeCopy>,
        #[serde(default)]
        mcp_tools: Vec<String>,
        #[serde(default)]
        agentic_coding_tools: Vec<String>,
        #[serde(default)]
        saas_tools: Vec<String>,
    },
    Downloads {
        #[serde(default)]
        anchor: String,
        overline: String,
        heading: String,
        #[serde(default)]
        description: Option<String>,
        archive_label: String,
        #[serde(default)]
        package: Option<PackageInstallCopy>,
    },
    Cta {
        heading: String,
        #[serde(default)]
        body: Option<String>,
        email: String,
        #[serde(default)]
        email_subject: Option<String>,
    },
}

/// A marketing page (`/fractional-cto`, `/navigator`, `/services`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketingPageCopy {
    pub head_title: String,
    pub meta_description: String,
    pub title: String,
    #[serde(default)]
    pub hero_mark: Option<PracticeMark>,
    #[serde(default)]
    pub tagline: String,
    #[serde(default)]
    pub hero_lines: Vec<HeroLine>,
    #[serde(default)]
    pub hero_lead: String,
    #[serde(default)]
    pub hero_cta: Option<HeroCtaCopy>,
    #[serde(default)]
    pub skin: PageSkin,
    #[serde(default)]
    pub bands: Vec<BandCopy>,
}

/// Which typed document a catalog filename must deserialize as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalePageKind {
    Home,
    Litigation,
    Transactional,
    Marketing,
}

/// The page kind for a catalog stem, if the stem is one this catalog publishes.
#[must_use]
pub fn locale_page_kind(stem: &str) -> Option<LocalePageKind> {
    match stem {
        "home" => Some(LocalePageKind::Home),
        "litigation" => Some(LocalePageKind::Litigation),
        "fractional-gc" => Some(LocalePageKind::Transactional),
        "fractional-cto" | "navigator" | "services" => Some(LocalePageKind::Marketing),
        _ => None,
    }
}

/// Whether `path` is a brand locale catalog file: `…/locales/<locale>/<page>.yaml`.
#[must_use]
pub fn is_locale_yaml_path(path: &Path) -> bool {
    locale_yaml_parts(path).is_some()
}

/// The locale directory name and page stem for a catalog path.
#[must_use]
pub fn locale_yaml_parts(path: &Path) -> Option<(&str, &str)> {
    let ext = path.extension()?.to_str()?;
    if !ext.eq_ignore_ascii_case("yaml") && !ext.eq_ignore_ascii_case("yml") {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    let locale_dir = path.parent()?;
    let catalog_root = locale_dir.parent()?;
    if catalog_root.file_name()?.to_str()? != "locales" {
        return None;
    }
    let locale = locale_dir.file_name()?.to_str()?;
    Some((locale, stem))
}

/// Deserialize one catalog file as the page its stem names.
///
/// `stem` is the filename without extension (`home`, `litigation`). The YAML is
/// checked as authored — placeholders stay in the strings — so validate does
/// not need a mounted brand.
pub fn parse_locale_file(stem: &str, yaml: &str) -> Result<(), String> {
    let kind = locale_page_kind(stem).ok_or_else(|| {
        format!(
            "unknown locale page `{stem}`; expected one of {}",
            KNOWN_PAGES.join(", ")
        )
    })?;
    match kind {
        LocalePageKind::Home => deserialize::<HomeCopy>(stem, yaml),
        LocalePageKind::Litigation => deserialize::<LitigationCopy>(stem, yaml),
        LocalePageKind::Transactional => deserialize::<TransactionalCopy>(stem, yaml),
        LocalePageKind::Marketing => deserialize::<MarketingPageCopy>(stem, yaml),
    }
}

fn deserialize<T: for<'de> Deserialize<'de>>(stem: &str, yaml: &str) -> Result<(), String> {
    serde_yaml::from_str::<T>(yaml)
        .map(|_| ())
        .map_err(|err| format!("{stem}: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolate_replaces_the_two_brand_placeholders() {
        let out = interpolate(
            "{site_name} writes to {firm_email}",
            "Neon Law",
            "contact@neonlaw.com",
        );
        assert_eq!(out, "Neon Law writes to contact@neonlaw.com");
    }

    #[test]
    fn home_catalog_deserializes() {
        parse_locale_file(
            "home",
            r#"
head_title: "{site_name} | Home"
meta_description: Everyone deserves to be seen.
heading: Everyone deserves to be seen.
lead: We fight for people.
contact_label: Contact us
hero:
  alt: A skyline.
  asset: img/new-york/new-york.png
service:
  heading: We are by your side.
  body:
    - - text: We stand with people.
practices_heading: Our complementary practice
practices:
  - mark: technology
    heading: Fractional CTO
    body: We run the technology function.
    href: /fractional-cto
"#,
        )
        .expect("home catalog");
    }

    #[test]
    fn litigation_catalog_deserializes() {
        parse_locale_file(
            "litigation",
            r#"
head_title: "{site_name} | Litigation"
meta_description: Litigation attorneys built for speed.
eyebrow: Values-Based Litigation
heading:
  text: Litigation built for speed.
  accent_words: 1
lead: Our strategy is generally the same.
cta_label: Contact us
body:
  - - text: We represent those who have not been justly seen.
    - text: Neon Law Navigator
      href: /navigator
"#,
        )
        .expect("litigation catalog");
    }

    #[test]
    fn marketing_catalog_deserializes_a_downloads_band() {
        parse_locale_file(
            "navigator",
            r#"
head_title: "Neon Law Navigator — {site_name}"
meta_description: Vibe coding for lawyers.
title: Neon Law Navigator
hero_mark: helm
tagline: Vibe coding for lawyers.
skin: marketing
bands:
  - kind: downloads
    anchor: download
    overline: Download
    heading: Run Navigator yourself
    archive_label: every release
    package:
      heading: Install with Homebrew
      body:
        - - text: On a Mac this is the route we recommend.
  - kind: cta
    heading: Co-Counsel a Pro Bono Case with Us
    email: "{firm_email}"
    email_subject: Co-Counseling for Good with AI
"#,
        )
        .expect("navigator catalog");
    }

    #[test]
    fn unknown_stem_is_refused() {
        let err = parse_locale_file("about", "title: About\n").expect_err("unknown stem");
        assert!(err.contains("unknown locale page `about`"), "{err}");
    }

    #[test]
    fn missing_required_field_is_refused() {
        let err = parse_locale_file("home", "heading: Hello\n").expect_err("incomplete home");
        assert!(err.contains("home:"), "{err}");
    }

    #[test]
    fn locale_yaml_parts_reads_the_locales_en_layout() {
        let path = Path::new("/tmp/neon/locales/en/home.yaml");
        assert_eq!(locale_yaml_parts(path), Some(("en", "home")));
        assert!(is_locale_yaml_path(path));
        assert!(!is_locale_yaml_path(Path::new("/tmp/seeds/Person.yaml")));
    }

    #[test]
    fn every_known_page_has_a_kind() {
        for page in KNOWN_PAGES {
            assert!(
                locale_page_kind(page).is_some(),
                "{page} must map to a typed catalog"
            );
        }
    }
}
