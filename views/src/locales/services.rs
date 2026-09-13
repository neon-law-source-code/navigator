//! The firm's individual-services catalog: the structured twin of the prose
//! cards `/services` publishes.
//!
//! A marketing band is prose — a title and a paragraph. That is the right
//! shape for an argument and the wrong shape for a schedule of work: it
//! carries no identifier, no item number, no category, and no fee anything can
//! read. This module is the schedule, modelled as records, so the page can be
//! searched and filtered and a second repository can render the same services
//! without keeping a second copy of them.
//!
//! It is the sibling of [`super::shared`] and deliberately wears the same
//! three guarantees, for the same reasons:
//!
//! - **Version.** [`ServicesCatalog::catalog_version`] is an explicit integer.
//!   A consumer built against a version it does not understand refuses the
//!   document rather than rendering half a fee schedule.
//! - **Closed vocabulary.** [`ServiceCategory`] is an enum, not a string, so a
//!   typo in a category becomes a validation error instead of a service that
//!   silently files under nothing.
//! - **Resolvable references.** A `related` id names a service this same
//!   catalog defines. A dangling one would render as a link to nowhere.
//!
//! One rule is this document's own. A service publishes **exactly one** fee:
//! either a [`FlatFee`] kind, which resolves to the catalog's single
//! [`ServicesCatalog::flat_fee`] figure, or a literal [`ServiceCopy::amount`].
//! Both is two prices on one matter; neither is a priced list with a blank in
//! it. Both are refused here rather than on the page.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::shared::{self, Provenance};

/// The catalog version this build authors and understands.
pub const SUPPORTED_CATALOG_VERSION: u32 = 1;

/// The stem a services catalog file carries:
/// `locales/en/<brand-key>/services-catalog.yaml`.
///
/// Deliberately *not* in [`super::KNOWN_PAGES`]. That list is the stems a
/// brand renders as a page, and `BrandKey::catalog_pages` loads every one of
/// them as typed page copy. This document is a catalog a page reads, the way
/// `shared.yaml` is.
pub const SERVICES_CATALOG_STEM: &str = "services-catalog";

/// Which part of the practice a service belongs to.
///
/// An enum rather than a string: the vocabulary is closed, so an unknown value
/// is refused by name at parse time. The gallery's `all` is a filter control,
/// not a category a service can carry, and is absent for that reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceCategory {
    /// Starting a business.
    Company,
    /// Keeping one running.
    Compliance,
    /// Names, marks, and the legal identity of a person or business.
    Identity,
    /// Wills, trusts, and family plans.
    Estate,
    /// Contracts and forms.
    Contracts,
    /// Disputes and housing — the work that arrives with a deadline.
    Urgent,
}

impl ServiceCategory {
    /// Every category, in the order a catalog declares them.
    pub const ALL: &'static [Self] = &[
        Self::Company,
        Self::Compliance,
        Self::Identity,
        Self::Estate,
        Self::Contracts,
        Self::Urgent,
    ];

    /// Words a service in this category should be findable by that a reader
    /// would never see printed.
    ///
    /// "Wills & family plans" is the firm's shelf label; a visitor types
    /// "family" or "legacy". These are searched and never rendered, so the
    /// page can meet a reader's word without publishing a word the firm would
    /// not choose.
    #[must_use]
    pub const fn search_aliases(self) -> &'static str {
        match self {
            Self::Estate => "personal family legacy",
            _ => "",
        }
    }

    /// The value this category is written as in YAML and JSON.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Company => "company",
            Self::Compliance => "compliance",
            Self::Identity => "identity",
            Self::Estate => "estate",
            Self::Contracts => "contracts",
            Self::Urgent => "urgent",
        }
    }
}

/// Which flat fee a service charges.
///
/// Both kinds resolve to the catalog's single [`ServicesCatalog::flat_fee`]
/// figure. They stay distinct because the *disclosure* beside them differs —
/// a form fee is waived under a plan, a trademark filing is not — and because
/// collapsing them would make that difference unrecoverable from the data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlatFee {
    /// The per-form fee.
    Form,
    /// The trademark filing fee, which a government charge is added to.
    Trademark,
}

/// One category and the words a reader sees for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CategoryCopy {
    /// What a reader sees. Field order is alphabetical throughout this module
    /// so [`ServicesCatalog::canonical_payload`] serializes with sorted keys.
    pub label: String,
    /// Which category this names.
    pub value: ServiceCategory,
}

/// One service the firm publishes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceCopy {
    /// A literal fee, e.g. `$350`. Exactly one of this and `flat_fee` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<String>,
    /// One sentence or two saying what the work is, in a reader's words.
    pub blurb: String,
    pub category: ServiceCategory,
    /// The flat-fee kind. Exactly one of this and `amount` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flat_fee: Option<FlatFee>,
    /// The stable identifier a `related` reference and a deep link name.
    pub id: String,
    /// What the fee buys, as scope lines. A service with no scope reads as
    /// covering everything.
    #[serde(default)]
    pub includes: Vec<String>,
    /// The numeric item code, kept as a string so a leading zero survives.
    pub item: String,
    /// Extra words a reader might search by that the name and blurb do not
    /// already contain.
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Whether this service requires a plan.
    #[serde(default)]
    pub members_only: bool,
    pub name: String,
    /// What the fee is charged per — `per form`, `per contract`, `per year`.
    pub period: String,
    /// Other services in this catalog worth reading next.
    #[serde(default)]
    pub related: Vec<String>,
    /// Whether a government body charges its own fee on top of this one.
    #[serde(default)]
    pub state_fee: bool,
}

impl ServiceCopy {
    /// Every value of this service that a reader can see or search.
    ///
    /// Used by validation to check each one is substitutable, and by the
    /// renderer to build a search haystack, so the two cannot disagree about
    /// what counts as published copy.
    fn values(&self) -> impl Iterator<Item = (&'static str, &str)> {
        [
            ("blurb", self.blurb.as_str()),
            ("name", self.name.as_str()),
            ("period", self.period.as_str()),
            ("item", self.item.as_str()),
        ]
        .into_iter()
        .chain(self.amount.as_deref().map(|amount| ("amount", amount)))
        .chain(self.includes.iter().map(|line| ("includes", line.as_str())))
        .chain(self.keywords.iter().map(|word| ("keywords", word.as_str())))
    }
}

/// The services catalog document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServicesCatalog {
    /// The contract version. See [`SUPPORTED_CATALOG_VERSION`].
    pub catalog_version: u32,
    /// The categories, in publication order, each with its reader-facing
    /// label.
    pub categories: Vec<CategoryCopy>,
    /// What a [`FlatFee`] service costs. One figure, so a page cannot print
    /// two different "flat fees".
    pub flat_fee: String,
    /// The services, in publication order.
    pub services: Vec<ServiceCopy>,
}

impl ServicesCatalog {
    /// Deserialize and validate one services catalog document.
    pub fn parse(yaml: &str) -> Result<Self, String> {
        let catalog: Self =
            serde_yaml::from_str(yaml).map_err(|err| format!("{SERVICES_CATALOG_STEM}: {err}"))?;
        catalog.validate()?;
        Ok(catalog)
    }

    fn validate(&self) -> Result<(), String> {
        if self.catalog_version != SUPPORTED_CATALOG_VERSION {
            return Err(format!(
                "{SERVICES_CATALOG_STEM}: catalog version {} is not supported; this build authors \
                 version {SUPPORTED_CATALOG_VERSION}",
                self.catalog_version
            ));
        }
        check_fee("flat_fee", &self.flat_fee)?;
        self.validate_categories()?;
        let ids = self.validate_services()?;
        for service in &self.services {
            for related in &service.related {
                if !ids.contains(related.as_str()) {
                    return Err(format!(
                        "{SERVICES_CATALOG_STEM}: `{}` is related to `{related}`, which this \
                         catalog does not define",
                        service.id
                    ));
                }
                if related == &service.id {
                    return Err(format!(
                        "{SERVICES_CATALOG_STEM}: `{}` is related to itself",
                        service.id
                    ));
                }
            }
        }
        Ok(())
    }

    /// Every category is declared exactly once, and all of them are.
    ///
    /// A catalog that omits one has a service it cannot label; a catalog that
    /// declares one twice has two labels for the same shelf and no rule for
    /// which wins.
    fn validate_categories(&self) -> Result<(), String> {
        let mut seen: BTreeSet<ServiceCategory> = BTreeSet::new();
        for category in &self.categories {
            check_value(category.value.as_str(), &category.label)?;
            if !seen.insert(category.value) {
                return Err(format!(
                    "{SERVICES_CATALOG_STEM}: category `{}` is declared twice",
                    category.value.as_str()
                ));
            }
        }
        for category in ServiceCategory::ALL {
            if !seen.contains(category) {
                return Err(format!(
                    "{SERVICES_CATALOG_STEM}: category `{}` has no label",
                    category.as_str()
                ));
            }
        }
        Ok(())
    }

    /// Check each service and return the set of ids, for the `related` pass.
    fn validate_services(&self) -> Result<BTreeSet<&str>, String> {
        let mut ids: BTreeSet<&str> = BTreeSet::new();
        let mut items: BTreeSet<&str> = BTreeSet::new();
        for service in &self.services {
            if !ids.insert(service.id.as_str()) {
                return Err(format!(
                    "{SERVICES_CATALOG_STEM}: `{}` is defined twice",
                    service.id
                ));
            }
            if !items.insert(service.item.as_str()) {
                return Err(format!(
                    "{SERVICES_CATALOG_STEM}: item number `{}` is used twice, most recently by \
                     `{}`",
                    service.item, service.id
                ));
            }
            match (&service.flat_fee, &service.amount) {
                (Some(_), Some(_)) => {
                    return Err(format!(
                        "{SERVICES_CATALOG_STEM}: `{}` publishes both a flat fee and an amount; a \
                         matter carries one fee",
                        service.id
                    ))
                }
                (None, None) => {
                    return Err(format!(
                        "{SERVICES_CATALOG_STEM}: `{}` publishes no fee; set `flat_fee` or \
                         `amount`",
                        service.id
                    ))
                }
                _ => {}
            }
            if let Some(amount) = service.amount.as_deref() {
                check_fee(&format!("{}.amount", service.id), amount)?;
            }
            if service.includes.is_empty() {
                return Err(format!(
                    "{SERVICES_CATALOG_STEM}: `{}` names no scope, which reads as covering \
                     everything",
                    service.id
                ));
            }
            for (field, value) in service.values() {
                check_value(&format!("{}.{field}", service.id), value)?;
            }
        }
        Ok(ids)
    }

    /// The words a reader sees for `category`.
    ///
    /// Infallible by construction: [`Self::validate_categories`] proves every
    /// category carries a label before a catalog exists.
    #[must_use]
    pub fn category_label(&self, category: ServiceCategory) -> &str {
        self.categories
            .iter()
            .find(|declared| declared.value == category)
            .map_or("", |declared| declared.label.as_str())
    }

    /// The service `id` names, if this catalog defines it.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&ServiceCopy> {
        self.services.iter().find(|service| service.id == id)
    }

    /// What `service` charges: its literal amount, or the catalog's one flat
    /// fee. Infallible for the same reason [`Self::category_label`] is.
    #[must_use]
    pub fn fee<'a>(&'a self, service: &'a ServiceCopy) -> &'a str {
        service.amount.as_deref().unwrap_or(self.flat_fee.as_str())
    }

    /// The bytes an integrity digest covers: compact JSON with every object
    /// key sorted.
    ///
    /// Every struct in this module declares its serialized fields in
    /// alphabetical order, which is what makes `serde_json`'s struct
    /// serialization already canonical — a consumer that re-serializes the
    /// parsed value reproduces these exact bytes and can therefore check the
    /// digest.
    #[must_use]
    pub fn canonical_payload(&self, source: &Provenance) -> String {
        let payload = ExportPayload {
            catalog_version: self.catalog_version,
            categories: self.categories.clone(),
            flat_fee: self.flat_fee.clone(),
            services: self.services.clone(),
            source: source.clone(),
        };
        serde_json::to_string(&payload).expect("invariant: the export payload is plain JSON data")
    }
}

/// The digest-covered half of an export. Fields are alphabetical; see
/// [`ServicesCatalog::canonical_payload`].
#[derive(Debug, Serialize)]
struct ExportPayload {
    catalog_version: u32,
    categories: Vec<CategoryCopy>,
    flat_fee: String,
    services: Vec<ServiceCopy>,
    source: Provenance,
}

/// Refuse a value a page cannot publish.
///
/// The same three rules [`super::shared`] applies, and for the same reasons: a
/// value is substituted into a quoted scalar, so a newline or a double quote
/// breaks the document it lands in, and a placeholder outside the supported
/// set reaches a reader as a literal brace.
fn check_value(key: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{SERVICES_CATALOG_STEM}: `{key}` is empty"));
    }
    if value.contains('\n') {
        return Err(format!(
            "{SERVICES_CATALOG_STEM}: `{key}` spans more than one line; a catalog value is \
             substituted into a quoted scalar"
        ));
    }
    if value.contains('"') {
        return Err(format!(
            "{SERVICES_CATALOG_STEM}: `{key}` contains a double quote, which cannot be substituted \
             into a quoted scalar"
        ));
    }
    for placeholder in shared::placeholders(value) {
        if !shared::SUPPORTED_PLACEHOLDERS.contains(&placeholder.as_str()) {
            return Err(format!(
                "{SERVICES_CATALOG_STEM}: `{key}` uses unsupported placeholder \
                 `{{{placeholder}}}`; expected one of {}",
                shared::SUPPORTED_PLACEHOLDERS.join(", ")
            ));
        }
    }
    Ok(())
}

/// Refuse a published fee that is not a figure a reader can pay.
///
/// [`check_value`] asks only whether a string is safe to substitute. A fee is
/// advertised to the public and has to clear more than that: `$0` on a legal
/// fee schedule reads as free work the firm is not offering, `-$50` is not a
/// price at all, and `free` is a claim rather than an amount — yet all three
/// satisfy every text rule and would reach the page.
///
/// Positivity is decided by looking for a non-zero digit rather than by
/// parsing. A fee is money, and money is the wrong thing to route through a
/// binary float on its way to a public page.
fn check_fee(key: &str, value: &str) -> Result<(), String> {
    check_value(key, value)?;
    let refuse = |reason: &str| {
        Err(format!(
            "{SERVICES_CATALOG_STEM}: `{key}` publishes `{value}`, which {reason}"
        ))
    };
    let Some(amount) = value.strip_prefix('$') else {
        return refuse("is not a fee; a published fee is a dollar figure such as `$350`");
    };
    let (whole, cents) = match amount.split_once('.') {
        Some((whole, cents)) => (whole, Some(cents)),
        None => (amount, None),
    };
    if whole.is_empty()
        || !whole
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b',')
    {
        return refuse(
            "is not a fee; the amount is digits, optionally grouped with commas, such as `$3,650`",
        );
    }
    if let Some(cents) = cents {
        if cents.len() != 2 || !cents.bytes().all(|byte| byte.is_ascii_digit()) {
            return refuse("is not a fee; cents are exactly two digits, such as `$12.50`");
        }
    }
    if !amount.bytes().any(|byte| matches!(byte, b'1'..=b'9')) {
        return refuse("is not a fee a reader can pay; a published fee is greater than zero");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-service catalog with every required field, which each test below
    /// mutates in exactly one way.
    fn fixture() -> String {
        String::from(
            r"catalog_version: 1
flat_fee: $50
categories:
  - value: company
    label: Start a business
  - value: compliance
    label: Run a business
  - value: identity
    label: Names and brands
  - value: estate
    label: Wills and family plans
  - value: contracts
    label: Contracts and forms
  - value: urgent
    label: Disputes and housing
services:
  - id: llc-file
    item: '1101'
    name: Start a company
    blurb: We set up your company and a lawyer files the papers.
    category: company
    flat_fee: form
    period: per form
    state_fee: true
    includes:
      - Prepare the papers
    related:
      - nv-address
    keywords:
      - LLC
  - id: nv-address
    item: '1202'
    name: Nevada business address
    blurb: A Nevada street address for legal notices.
    category: compliance
    amount: $350
    period: per year
    state_fee: false
    includes:
      - A street address
",
        )
    }

    #[test]
    fn the_fixture_parses() {
        let catalog = ServicesCatalog::parse(&fixture()).expect("the fixture is valid");
        assert_eq!(catalog.services.len(), 2);
        assert_eq!(
            catalog.category_label(ServiceCategory::Company),
            "Start a business"
        );
        // A flat-fee service reads the catalog's one figure; a literal amount
        // reads itself.
        let llc = catalog.get("llc-file").expect("llc-file");
        assert_eq!(catalog.fee(llc), "$50");
        let address = catalog.get("nv-address").expect("nv-address");
        assert_eq!(catalog.fee(address), "$350");
        assert!(catalog.get("nothing").is_none());
    }

    #[test]
    fn a_duplicate_id_is_refused() {
        let err = ServicesCatalog::parse(&fixture().replace("id: nv-address", "id: llc-file"))
            .expect_err("duplicate id");
        assert!(err.contains("`llc-file` is defined twice"), "{err}");
    }

    #[test]
    fn a_duplicate_item_number_is_refused() {
        let err = ServicesCatalog::parse(&fixture().replace("item: '1202'", "item: '1101'"))
            .expect_err("duplicate item");
        assert!(err.contains("item number `1101` is used twice"), "{err}");
    }

    /// The vocabulary is closed, so an unknown category is named and the
    /// accepted ones are listed — serde's own unknown-variant error.
    #[test]
    fn an_unknown_category_is_refused() {
        let err =
            ServicesCatalog::parse(&fixture().replace("category: company", "category: crypto"))
                .expect_err("unknown category");
        assert!(err.contains("crypto"), "{err}");
        assert!(err.contains("company"), "{err}");
    }

    #[test]
    fn a_category_declared_twice_is_refused() {
        let err = ServicesCatalog::parse(&fixture().replace(
            "  - value: compliance\n    label: Run a business",
            "  - value: company\n    label: Run a business",
        ))
        .expect_err("duplicate category");
        assert!(
            err.contains("category `company` is declared twice"),
            "{err}"
        );
    }

    #[test]
    fn a_category_with_no_label_is_refused() {
        let err = ServicesCatalog::parse(
            &fixture().replace("  - value: urgent\n    label: Disputes and housing\n", ""),
        )
        .expect_err("missing category label");
        assert!(err.contains("category `urgent` has no label"), "{err}");
    }

    #[test]
    fn a_dangling_related_id_is_refused() {
        let err =
            ServicesCatalog::parse(&fixture().replace("      - nv-address\n", "      - trust\n"))
                .expect_err("dangling related");
        assert!(
            err.contains("`llc-file` is related to `trust`, which this catalog does not define"),
            "{err}"
        );
    }

    #[test]
    fn a_service_related_to_itself_is_refused() {
        let err = ServicesCatalog::parse(
            &fixture().replace("      - nv-address\n", "      - llc-file\n"),
        )
        .expect_err("self-related");
        assert!(err.contains("`llc-file` is related to itself"), "{err}");
    }

    #[test]
    fn a_stray_placeholder_is_refused() {
        let err = ServicesCatalog::parse(
            &fixture().replace("period: per year", "period: per year, call {firm_phone}"),
        )
        .expect_err("stray placeholder");
        assert!(err.contains("{firm_phone}"), "{err}");
        assert!(err.contains("nv-address.period"), "{err}");
    }

    /// The two brand placeholders are supported, so a catalog may use them.
    #[test]
    fn the_brand_placeholders_are_accepted() {
        ServicesCatalog::parse(&fixture().replace(
            "blurb: A Nevada street address for legal notices.",
            "blurb: A Nevada street address, forwarded by {site_name} to {firm_email}.",
        ))
        .expect("brand placeholders are supported");
    }

    #[test]
    fn a_value_spanning_two_lines_is_refused() {
        let err = ServicesCatalog::parse(&fixture().replace(
            "blurb: A Nevada street address for legal notices.",
            "blurb: |\n      A Nevada street address.\n      Forwarded to you.",
        ))
        .expect_err("multi-line value");
        assert!(err.contains("spans more than one line"), "{err}");
    }

    #[test]
    fn an_empty_value_is_refused() {
        let err = ServicesCatalog::parse(
            &fixture().replace("name: Nevada business address", "name: ' '"),
        )
        .expect_err("empty value");
        assert!(err.contains("`nv-address.name` is empty"), "{err}");
    }

    #[test]
    fn publishing_two_fees_is_refused() {
        let err = ServicesCatalog::parse(&fixture().replace(
            "    amount: $350\n",
            "    amount: $350\n    flat_fee: form\n",
        ))
        .expect_err("two fees");
        assert!(
            err.contains("`nv-address` publishes both a flat fee and an amount"),
            "{err}"
        );
    }

    #[test]
    fn publishing_no_fee_is_refused() {
        let err = ServicesCatalog::parse(&fixture().replace("    amount: $350\n", ""))
            .expect_err("no fee");
        assert!(err.contains("`nv-address` publishes no fee"), "{err}");
    }

    /// A published fee has to be a figure a reader can pay. These all satisfy
    /// every text rule — one line, no quotes, no stray placeholder — and would
    /// otherwise reach the public fee schedule.
    #[test]
    fn a_fee_that_is_not_a_payable_figure_is_refused() {
        for (amount, expected) in [
            ("$0", "greater than zero"),
            ("$0.00", "greater than zero"),
            ("$000", "greater than zero"),
            ("-$50", "is not a fee"),
            ("$-50", "is not a fee"),
            ("free", "is not a fee"),
            ("350", "is not a fee"),
            ("$35.0", "cents are exactly two digits"),
            ("$35.000", "cents are exactly two digits"),
            ("$", "is not a fee"),
        ] {
            let err = ServicesCatalog::parse(
                &fixture().replace("amount: $350", &format!("amount: '{amount}'")),
            )
            .unwrap_err();
            assert!(
                err.contains("nv-address.amount") && err.contains(expected),
                "`{amount}` must be refused as a fee mentioning {expected:?}: {err}"
            );
        }
    }

    #[test]
    fn a_fee_a_reader_can_pay_is_accepted() {
        for amount in ["$5", "$350", "$3,650", "$12.50", "$0.50"] {
            ServicesCatalog::parse(
                &fixture().replace("amount: $350", &format!("amount: '{amount}'")),
            )
            .unwrap_or_else(|err| panic!("`{amount}` is a payable fee: {err}"));
        }
    }

    /// The catalog-level flat fee is held to the same rule: it is the figure
    /// every flat-fee service prints.
    #[test]
    fn a_flat_fee_that_is_not_a_payable_figure_is_refused() {
        let err = ServicesCatalog::parse(&fixture().replace("flat_fee: $50", "flat_fee: '$0'"))
            .expect_err("a zero flat fee");
        assert!(err.contains("`flat_fee`"), "{err}");
        assert!(err.contains("greater than zero"), "{err}");
    }

    #[test]
    fn a_service_with_no_scope_is_refused() {
        let err = ServicesCatalog::parse(
            &fixture().replace("    includes:\n      - A street address\n", ""),
        )
        .expect_err("no scope");
        assert!(err.contains("`nv-address` names no scope"), "{err}");
    }

    #[test]
    fn an_unsupported_catalog_version_is_refused() {
        let err =
            ServicesCatalog::parse(&fixture().replace("catalog_version: 1", "catalog_version: 99"))
                .expect_err("unsupported version");
        assert!(err.contains("catalog version 99 is not supported"), "{err}");
    }

    #[test]
    fn an_empty_flat_fee_is_refused() {
        let err = ServicesCatalog::parse(&fixture().replace("flat_fee: $50", "flat_fee: ' '"))
            .expect_err("empty flat fee");
        assert!(err.contains("`flat_fee` is empty"), "{err}");
    }

    /// The canonical payload is the bytes a digest covers, so re-serializing
    /// the parsed value must reproduce it exactly — the property that lets a
    /// consumer detect a hand-edited artifact.
    #[test]
    fn the_canonical_payload_has_sorted_keys() {
        let catalog = ServicesCatalog::parse(&fixture()).expect("fixture");
        let source = Provenance {
            path: "neon/locales/en/neon/services-catalog.yaml".into(),
            repository: "neon-law-source-code/navigator".into(),
            revision: "0123456789abcdef0123456789abcdef01234567".into(),
        };
        let payload = catalog.canonical_payload(&source);
        let reparsed: serde_json::Value = serde_json::from_str(&payload).expect("payload is json");
        assert_eq!(
            serde_json::to_string(&reparsed).expect("re-serialize"),
            payload,
            "a consumer must be able to reproduce the hashed bytes"
        );
    }

    /// Every category carries a distinct written form, so the enum and its
    /// YAML spelling cannot drift apart.
    #[test]
    fn every_category_has_a_distinct_value() {
        let values: BTreeSet<&str> = ServiceCategory::ALL
            .iter()
            .map(|category| category.as_str())
            .collect();
        assert_eq!(values.len(), ServiceCategory::ALL.len());
        for category in ServiceCategory::ALL {
            let round_trip: ServiceCategory =
                serde_yaml::from_str(category.as_str()).expect("category round-trips");
            assert_eq!(round_trip, *category);
        }
    }
}
