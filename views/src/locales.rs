//! English marketing-copy catalogs for a brand crate.
//!
//! Public page copy lives in `locales/en/<brand-key>/*.yaml` beside the brand
//! crate that publishes it (a fixture may still use the flat
//! `locales/en/<page>.yaml` layout). The site still publishes one language:
//! these files are an authoring catalog, not a translated surface.
//! `{site_name}` and `{firm_email}` are the brand substitutions, and
//! `{shared:<key>}` pulls one sentence from the cross-repository catalog in
//! [`shared`]; everything a visitor reads is otherwise the YAML.
//!
//! [`parse_locale_file`] is the typed check `navigator project gate` runs so a
//! copy-only edit cannot land a document the brand crate cannot load.

use std::path::Path;

use serde::{Deserialize, Serialize};

mod cyber_injury;
pub use cyber_injury::CyberInjuryCopy;
mod daybridge;
mod death_and_divorce;
mod estate;
mod privacy;
pub use daybridge::DaybridgeCopy;
pub use death_and_divorce::DeathAndDivorceCopy;
pub use privacy::PrivacyCopy;
pub mod services;
pub mod shared;
pub use estate::EstateCopy;

/// The only locale directory the site publishes.
pub const DEFAULT_LOCALE: &str = "en";

/// Page stems the English catalog may hold.
///
/// `gateway-delete-your-debt` is the first of a family: a Neon-only practice
/// gateway, published solely under `neon/locales/en/neon/` and never shipped
/// by another `BrandKey` (see [`crate::brand::BrandKey::catalog_pages`]). Each
/// gateway page still deserializes as [`MarketingPageCopy`] like `navigator`
/// and `services` do; only the stem is new.
pub const KNOWN_PAGES: &[&str] = &[
    "fractional-gc",
    "gateway-accidents",
    "gateway-delete-your-data",
    "gateway-delete-your-debt",
    "gateway-divorce",
    "gateway-estate-planning",
    "gateway-immigration",
    "home",
    "litigation",
    "navigator",
    "notations",
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
    pub code: bool,
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
    pub service: Option<ServiceSectionCopy>,
    #[serde(default)]
    pub practices_heading: String,
    #[serde(default)]
    pub practices: Vec<PracticeLinkCopy>,
    /// How a request becomes a durable record. `None` renders no section, so
    /// a brand that keeps no such record publishes nothing about one.
    #[serde(default)]
    pub provenance: Option<ProvenanceSectionCopy>,
    /// Company counsel pricing and the illustrative notation flow.
    #[serde(default)]
    pub company: Option<CompanyCopy>,
    /// Lifetime estate planning, absent on other practice pages.
    #[serde(default)]
    pub estate: Option<EstateCopy>,
    /// Annual data removal and privacy protection.
    #[serde(default)]
    pub privacy: Option<PrivacyCopy>,
    /// Divorce fees and separately paid legal costs.
    #[serde(default)]
    pub daybridge: Option<DaybridgeCopy>,
    /// Divorce, estate planning, and probate for the Death & Divorce brand.
    #[serde(default)]
    pub death_and_divorce: Option<DeathAndDivorceCopy>,
    /// AI-assisted personal injury campaign.
    #[serde(default)]
    pub cyber_injury: Option<CyberInjuryCopy>,
}

/// The home page's provenance section: the flow a request follows, the
/// ledger illustration beside it, and the prose under both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceSectionCopy {
    pub overline: String,
    pub heading: String,
    /// A second line of the heading set in the brand gradient. Empty renders
    /// the heading alone.
    #[serde(default)]
    pub heading_accent: String,
    #[serde(default)]
    pub lead: String,
    /// The flow, in order. Each step is drawn as a node on a rail.
    #[serde(default)]
    pub steps: Vec<ProvenanceStepCopy>,
    pub ledger_heading: String,
    /// What the ledger illustrates, said plainly beside it.
    #[serde(default)]
    pub ledger_caption: String,
    /// The ledger rows: a place a request went, and the record that followed.
    #[serde(default)]
    pub ledger: Vec<ProvenanceLedgerRowCopy>,
    /// What the record is for, as three or so tiles: privacy, security, and
    /// the federated work of the nodes.
    #[serde(default)]
    pub pillars: Vec<ProvenancePillarCopy>,
    #[serde(default)]
    pub notes: Vec<Paragraph>,
}

/// One tile under the provenance flow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenancePillarCopy {
    pub heading: String,
    pub body: String,
}

/// The decorative mark a provenance step opens on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceMark {
    #[default]
    Request,
    Attorney,
    Chain,
}

/// One step of the provenance flow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceStepCopy {
    #[serde(default)]
    pub mark: ProvenanceMark,
    pub label: String,
    pub detail: String,
}

/// One row of the provenance ledger illustration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceLedgerRowCopy {
    pub label: String,
    pub status: String,
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

/// The `/disputes` catalog.
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

/// One flat-fee pricing card the fractional-GC page publishes for the base
/// retainer itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PricingCardCopy {
    pub title: String,
    pub price: String,
    #[serde(default)]
    pub cadence: Option<String>,
    pub blurb: String,
    #[serde(default)]
    pub features: Vec<String>,
    /// Whole-dollar day rate the flat fee works out to, e.g. `10` for a fee
    /// that comes out to $10 a day. Draws a small banknote glyph beside the
    /// price. Absent renders no glyph.
    #[serde(default)]
    pub day_rate_bill: Option<u16>,
}

/// The `/business` catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionalCopy {
    pub head_title: String,
    pub meta_description: String,
    pub eyebrow: String,
    pub heading: HeroLine,
    pub lead: String,
    pub cta_label: String,
    pub virtues: Vec<VirtueCopy>,
    pub fee_heading: String,
    pub fee_body: String,
    #[serde(default)]
    pub pricing: Vec<PricingCardCopy>,
    pub closing_heading: String,
    pub closing_body: String,
    pub closing_email: String,
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
    /// The billing cadence, when the band renders in the pricing-card style
    /// (`/year`, `/day`, "flat fee"). Read only when the band's own
    /// `pricing_style` is set; ignored otherwise.
    #[serde(default)]
    pub cadence: Option<String>,
    /// Bullet features, rendered only in the pricing-card style.
    #[serde(default)]
    pub features: Vec<String>,
    /// Whole-dollar day rate the flat fee works out to, read only in the
    /// pricing-card style. See [`PricingCardCopy::day_rate_bill`].
    #[serde(default)]
    pub day_rate_bill: Option<u16>,
}

/// One example search the services band offers as a chip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SearchExampleCopy {
    pub label: String,
    pub query: String,
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
        /// Render this band's cards with the shared "Navigator-UX" pricing-card
        /// treatment (the same one `/business` uses) instead of the plain
        /// grid. A card's `href`/`href_label` become its call to action, and
        /// its first chip becomes the price beside `cadence`.
        #[serde(default)]
        pricing_style: bool,
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
    /// The firm's individual services, rendered from the brand's
    /// `services-catalog.yaml` rather than listed here.
    ///
    /// The services are a schedule of work with identifiers and fees; this
    /// band is the page copy *around* them — the heading, the search chrome, a
    /// reader sees when the search finds nothing, and the two regulated badges.
    /// Keeping the two apart is what lets the schedule be exported and
    /// consumed while the page copy stays page copy.
    Services {
        #[serde(default)]
        anchor: String,
        overline: String,
        heading: String,
        #[serde(default)]
        description: Option<String>,
        search_label: String,
        search_placeholder: String,
        submit_label: String,
        #[serde(default)]
        examples: Vec<SearchExampleCopy>,
        fee_label: String,
        includes_label: String,
        /// The chip a Notation package carries.
        package_badge: String,
        /// The label above a package's included Notations.
        package_members_label: String,
        /// The chip a service requiring a plan carries.
        members_badge: String,
        /// The chip a service carrying a government charge carries. A
        /// regulated disclosure, not decoration.
        state_fee_badge: String,
        empty: String,
        empty_help: String,
        clear_label: String,
    },
    ProjectNetwork {
        #[serde(default)]
        anchor: String,
        overline: String,
        heading: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        center_eyebrow: String,
        #[serde(default)]
        center_heading: String,
        #[serde(default)]
        center_detail: String,
        #[serde(default)]
        left_lane_label: String,
        #[serde(default)]
        right_lane_label: String,
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

/// A marketing page (`/navigator`, `/services`).
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
    pub hero_lead_runs: Paragraph,
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
    /// `shared.yaml` — the cross-repository copy catalog, not a page.
    Shared,
    /// `services-catalog.yaml` — the structured individual-services schedule
    /// `/services` renders, not a page of its own.
    ServicesCatalog,
}

/// The page kind for a catalog stem, if the stem is one this catalog publishes.
#[must_use]
pub fn locale_page_kind(stem: &str) -> Option<LocalePageKind> {
    match stem {
        "home" => Some(LocalePageKind::Home),
        "litigation" => Some(LocalePageKind::Litigation),
        "fractional-gc" => Some(LocalePageKind::Transactional),
        "navigator"
        | "notations"
        | "services"
        | "gateway-accidents"
        | "gateway-delete-your-data"
        | "gateway-delete-your-debt"
        | "gateway-divorce"
        | "gateway-estate-planning"
        | "gateway-immigration" => Some(LocalePageKind::Marketing),
        shared::SHARED_CATALOG_STEM => Some(LocalePageKind::Shared),
        services::SERVICES_CATALOG_STEM => Some(LocalePageKind::ServicesCatalog),
        _ => None,
    }
}

/// One catalog file under `locales/`.
///
/// Two layouts are valid, both English-only:
///
/// - `locales/<locale>/<page>.yaml` — a fixture or a single-brand tree
/// - `locales/<locale>/<brand-key>/<page>.yaml` — a house-of-brands tree
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocaleYamlParts<'a> {
    pub locale: &'a str,
    pub brand_key: Option<&'a str>,
    pub stem: &'a str,
}

/// Whether `path` is a brand locale catalog file.
#[must_use]
pub fn is_locale_yaml_path(path: &Path) -> bool {
    locale_yaml_parts(path).is_some()
}

/// The locale directory, optional brand-key directory, and page stem.
#[must_use]
pub fn locale_yaml_parts(path: &Path) -> Option<LocaleYamlParts<'_>> {
    let ext = path.extension()?.to_str()?;
    if !ext.eq_ignore_ascii_case("yaml") && !ext.eq_ignore_ascii_case("yml") {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    let parent = path.parent()?;
    let parent_name = parent.file_name()?.to_str()?;
    let grandparent = parent.parent()?;
    if grandparent.file_name()?.to_str()? == "locales" {
        return Some(LocaleYamlParts {
            locale: parent_name,
            brand_key: None,
            stem,
        });
    }
    let catalog_root = grandparent.parent()?;
    if catalog_root.file_name()?.to_str()? != "locales" {
        return None;
    }
    let locale = grandparent.file_name()?.to_str()?;
    Some(LocaleYamlParts {
        locale,
        brand_key: Some(parent_name),
        stem,
    })
}

/// Deserialize one catalog file as the page its stem names.
///
/// `stem` is the filename without extension (`home`, `litigation`). The YAML is
/// checked as authored — placeholders stay in the strings — so validate does
/// not need a mounted brand.
pub fn parse_locale_file(stem: &str, yaml: &str) -> Result<(), String> {
    let kind = locale_page_kind(stem).ok_or_else(|| {
        format!(
            "unknown locale page `{stem}`; expected one of {}, or `{}`, or `{}`",
            KNOWN_PAGES.join(", "),
            shared::SHARED_CATALOG_STEM,
            services::SERVICES_CATALOG_STEM
        )
    })?;
    if kind == LocalePageKind::Shared {
        return shared::SharedCatalog::parse(yaml).map(|_| ());
    }
    if kind == LocalePageKind::ServicesCatalog {
        return services::ServicesCatalog::parse(yaml).map(|_| ());
    }
    // A page catalog is checked as authored: `{site_name}`, `{firm_email}`,
    // and `{shared:<key>}` all stay in the strings, so validate needs neither
    // a mounted brand nor the shared catalog beside it. What it can still
    // prove is that no *other* brace reaches a reader as a literal.
    shared::check_page_placeholders(stem, yaml)?;
    match kind {
        LocalePageKind::Home => deserialize::<HomeCopy>(stem, yaml),
        LocalePageKind::Litigation => deserialize::<LitigationCopy>(stem, yaml),
        LocalePageKind::Transactional => deserialize::<TransactionalCopy>(stem, yaml),
        LocalePageKind::Marketing => deserialize::<MarketingPageCopy>(stem, yaml),
        LocalePageKind::Shared | LocalePageKind::ServicesCatalog => {
            unreachable!("handled above")
        }
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
service:
  heading: We are by your side.
  body:
    - - text: We stand with people.
practices_heading: Our complementary practice
practices:
  - mark: technology
    heading: Individual services
    body: We handle one-time matters at a published fee.
    href: /services
"#,
        )
        .expect("home catalog");
    }

    /// The provenance section is optional, and when present its steps and
    /// ledger rows deserialize as typed copy rather than free-form maps.
    #[test]
    fn home_catalog_deserializes_a_provenance_section() {
        parse_locale_file(
            "home",
            r#"
head_title: "{site_name} | Home"
meta_description: Ask companies to delete your data.
heading: Ask companies to delete your data.
lead: A licensed attorney helps.
contact_label: Contact us
provenance:
  overline: How the record works
  heading: Verified, then recorded.
  lead: We verify the request first.
  steps:
    - mark: request
      label: You send the request
      detail: Name the company.
    - mark: attorney
      label: A licensed attorney verifies it
      detail: Reviewed before it goes out.
    - mark: chain
      label: We upload the record to Solana
      detail: A hash, never the request.
  ledger_heading: Where your data has been removed from
  ledger_caption: An illustration, not a count.
  ledger:
    - label: A data broker
      status: Attested
  pillars:
    - heading: Privacy
      body: Only a hash goes on the chain.
  notes:
    - - text: Our lawyer-attested nodes are long-term provenance.
"#,
        )
        .expect("home catalog with provenance");
        // An unknown mark is a typo in the catalog, not a fourth glyph.
        let err = parse_locale_file(
            "home",
            r"
head_title: Home
meta_description: d
heading: h
lead: l
contact_label: c
provenance:
  overline: o
  heading: h
  ledger_heading: l
  steps:
    - mark: rocket
      label: x
      detail: y
",
        )
        .expect_err("unknown mark");
        assert!(err.contains("rocket"), "{err}");
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

    /// A Neon-only practice gateway deserializes as a `MarketingPageCopy` like
    /// `navigator` and `services` do; only the stem is new.
    #[test]
    fn gateway_catalog_deserializes() {
        parse_locale_file(
            "gateway-delete-your-debt",
            r#"
head_title: "Debt-collection defense | {site_name}"
meta_description: Debt-collection defense is offered through DeleteYourDebt.com.
title: Debt-collection defense
tagline: Defend yourself against debt collectors.
hero_mark: scales
hero_lead: >-
  Collection-lawsuit defense is offered through DeleteYourDebt.com, a Shook
  Law PLLC practice. It does not settle debts or negotiate balances.
hero_cta:
  href: "https://www.deleteyourdebt.com/"
  label: Visit DeleteYourDebt.com
skin: practice
"#,
        )
        .expect("gateway catalog");
        assert_eq!(
            locale_page_kind("gateway-delete-your-debt"),
            Some(LocalePageKind::Marketing)
        );
    }

    /// A gateway catalog is still a `MarketingPageCopy`: a missing required
    /// field fails the same way it would on `navigator` or `services`, and a
    /// `hero_cta` with no `label` fails too — the CTA's label is what names
    /// the destination, so it cannot be silently absent.
    #[test]
    fn gateway_catalog_rejects_missing_or_invalid_fields() {
        let err = parse_locale_file(
            "gateway-delete-your-debt",
            "meta_description: d\ntitle: t\n",
        )
        .expect_err("missing head_title");
        assert!(err.contains("gateway-delete-your-debt"), "{err}");

        let err = parse_locale_file(
            "gateway-delete-your-debt",
            r#"
head_title: "Debt-collection defense | {site_name}"
meta_description: d
title: t
hero_cta:
  href: "https://www.deleteyourdebt.com/"
"#,
        )
        .expect_err("hero_cta with no label");
        assert!(err.contains("label"), "{err}");
    }

    /// The services catalog reaches its own validator through the same
    /// `parse_locale_file` seam every page does, so `navigator project gate`'s
    /// locale pass covers it under `Y002` with no change to the walker.
    #[test]
    fn the_services_catalog_stem_routes_to_its_validator() {
        assert_eq!(
            locale_page_kind(services::SERVICES_CATALOG_STEM),
            Some(LocalePageKind::ServicesCatalog)
        );
        // Built from the supported version rather than a literal, so a
        // version bump cannot silently turn this into a test of the version
        // check instead of the category-label rule it asserts.
        let err = parse_locale_file(
            services::SERVICES_CATALOG_STEM,
            &format!(
                "catalog_version: {}\nflat_fee: $50\ncategories: []\nservices: []\n",
                services::SUPPORTED_CATALOG_VERSION
            ),
        )
        .expect_err("a catalog with no category labels");
        assert!(err.contains("category `company` has no label"), "{err}");
    }

    #[test]
    fn unknown_stem_is_refused() {
        let err = parse_locale_file("about", "title: About\n").expect_err("unknown stem");
        assert!(err.contains("unknown locale page `about`"), "{err}");
        // Both non-page catalogs are named, so the message tells an author
        // every stem this directory accepts.
        assert!(err.contains(shared::SHARED_CATALOG_STEM), "{err}");
        assert!(err.contains(services::SERVICES_CATALOG_STEM), "{err}");
    }

    #[test]
    fn missing_required_field_is_refused() {
        let err = parse_locale_file("home", "heading: Hello\n").expect_err("incomplete home");
        assert!(err.contains("home:"), "{err}");
    }

    #[test]
    fn locale_yaml_parts_reads_the_locales_en_layout() {
        let path = Path::new("/tmp/neon/locales/en/home.yaml");
        assert_eq!(
            locale_yaml_parts(path),
            Some(LocaleYamlParts {
                locale: "en",
                brand_key: None,
                stem: "home",
            })
        );
        assert!(is_locale_yaml_path(path));
        assert!(!is_locale_yaml_path(Path::new("/tmp/seeds/Person.yaml")));
    }

    #[test]
    fn locale_yaml_parts_reads_a_brand_keyed_catalog() {
        let path = Path::new("/tmp/neon/locales/en/delete-your-data/home.yaml");
        assert_eq!(
            locale_yaml_parts(path),
            Some(LocaleYamlParts {
                locale: "en",
                brand_key: Some("delete-your-data"),
                stem: "home",
            })
        );
        assert!(is_locale_yaml_path(path));
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

/// The company-counsel home page, authored entirely in the home catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CompanyCopy {
    pub principles: Vec<[String; 2]>,
    pub services_heading: String,
    pub services_note: String,
    pub services_link: String,
    pub services: Vec<[String; 2]>,
    pub community_heading: String,
    pub community_body: String,
    pub community_links: Vec<[String; 2]>,
    pub booking_href: String,
    pub pricing_link: String,
    pub retainer_note: String,
    pub retainer_amount: u32,
    pub simulator_heading: String,
    pub simulator_body: String,
    pub simulator_days_label: String,
    pub simulator_reviews_label: String,
    pub simulator_plan_label: String,
    pub simulator_review_label: String,
    pub simulator_contract_label: String,
    pub simulator_total_label: String,
    pub simulator_note: String,
    pub pause_label: String,
    pub packages: Vec<String>,
    pub pricing_heading: String,
    pub video_label: String,
    pub membership_label: String,
    pub membership_price: String,
    pub membership_unit: String,
    pub membership_body: String,
    pub membership_features: Vec<String>,
    pub express_heading: String,
    pub express_price: String,
    pub express_unit: String,
    pub express_body: String,
    pub page_note: String,
    pub drafting_heading: String,
    pub drafting_body: String,
    pub drafting_packages: Vec<[String; 3]>,
    pub closing_heading: String,
    pub closing_body: String,
    pub litigation_heading: String,
    pub litigation_link: String,
    pub litigation_price: String,
    pub litigation_unit: String,
    pub litigation_body: String,
    pub litigation_note: String,
    pub people_heading: String,
    pub people_body: String,
    pub immigration_label: String,
    pub estate_label: String,
    pub navigator_heading: String,
    pub navigator_body: String,
    pub navigator_link: String,
    pub source_label: String,
    pub source_note: String,
}
