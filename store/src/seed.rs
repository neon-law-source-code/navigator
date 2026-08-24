//! Canonical seed loader: insert the workspace-bundled YAML fixtures
//! (`store/seeds/*.yaml`) into every entity table the schema knows
//! about. Re-running is a no-op on the natural keys of each table.
//!
//! The YAML files use a `lookup_fields: + records:` shape (see
//! `store/seeds/`); this loader resolves nested foreign references
//! (e.g., `entity.entity_type.name`) by looking up rows by their
//! natural key.
//!
//! Both binaries in the workspace go through this module:
//! - `navigator list ...` calls [`seed_canonical`] before reading.
//! - a brand binary calls [`seed_environment`] after migrations on startup,
//!   naming the brand it serves so that brand's own seeds apply too.
//!
//! Seeds come in three layers — canonical, brand, and the sample-matter
//! fixture. [`seed_environment`] documents which reaches a deployment holding
//! real client files and why.

use anyhow::Context as _;

use crate::jurisdictions::{self, NewJurisdiction};
use crate::surreal::SurrealDb;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Per-entity insert counts for one seed pass.
#[derive(Debug, Default, Clone, Copy)]
pub struct SeedReport {
    pub jurisdictions_inserted: usize,
    pub entity_types_inserted: usize,
    pub entities_inserted: usize,
    pub persons_inserted: usize,
    pub persons_updated: usize,
    pub projects_inserted: usize,
    pub notations_inserted: usize,
    pub assets_inserted: usize,
    pub communications_inserted: usize,
    pub git_repositories_inserted: usize,
    pub questions_inserted: usize,
    pub mailrooms_inserted: usize,
    pub addresses_inserted: usize,
    pub letters_inserted: usize,
    pub answers_inserted: usize,
    pub person_entity_roles_inserted: usize,
    pub person_project_roles_inserted: usize,
    pub credentials_inserted: usize,
    pub templates_inserted: usize,
    pub testimonials_inserted: usize,
    /// Glossary terms materialized from `docs/glossary.md`. Reference
    /// data: environment-blind, upserted by slug on every boot.
    pub glossary_terms_written: usize,
}

impl SeedReport {
    /// One-line summary suitable for CLI output. Reports every entity
    /// even when zero so re-runs make it visible that the pass was
    /// a no-op.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "Seeded: {} jurisdictions, {} entity_types, {} entities, {} persons \
             (+{} role updates), {} projects, {} notations, {} assets, {} communications, \
             {} git_repos, {} questions, \
             {} mailrooms, {} addresses, {} letters, {} answers, \
             {} person_entity_roles, {} person_project_roles, {} credentials, \
             {} templates, {} testimonials, {} glossary_terms.",
            self.jurisdictions_inserted,
            self.entity_types_inserted,
            self.entities_inserted,
            self.persons_inserted,
            self.persons_updated,
            self.projects_inserted,
            self.notations_inserted,
            self.assets_inserted,
            self.communications_inserted,
            self.git_repositories_inserted,
            self.questions_inserted,
            self.mailrooms_inserted,
            self.addresses_inserted,
            self.letters_inserted,
            self.answers_inserted,
            self.person_entity_roles_inserted,
            self.person_project_roles_inserted,
            self.credentials_inserted,
            self.templates_inserted,
            self.testimonials_inserted,
            self.glossary_terms_written,
        )
    }
}

// ---------- Embedded canonical YAMLs ----------
//
// Bundled at compile time so the installed `navigator` binary is
// self-contained — no runtime lookup of `store/seeds/`.

/// The canonical jurisdiction reference data, embedded at compile time.
/// Exposed so cross-crate reconciliation tests (e.g. `cli`) can assert the
/// path vocabulary in `rules::f110` stays in sync with the seeded rows
/// without reaching into `store`'s private modules.
pub const JURISDICTION_SEED_YAML: &str = canonical::JURISDICTION;

/// The firm Entity that anchors the canonical seed. `Entity.yaml` re-creates
/// this row by exact name on every boot, so every deployment carries it.
/// `web` reads this to keep the row's delete and rename guards aligned with
/// the name the seed looks up.
///
/// This is the professional LLC a client engages — the entity of record behind
/// the Neon Law mark, which is why it is the row the application refuses to
/// delete. `Neon Law` is what the site is signed with; `Shook Law PLLC` is the
/// legal person that renders the services and owns the mark, and only a legal
/// person can anchor a client relationship. It is also the copyright holder in
/// Navigator and the Licensor named in `LICENSE`. Moving this name is a data
/// change as well as a code one: `seed_entities` reconciles
/// `entities.firm_anchor_key` on every boot, because the delete guard reads
/// that column and not the name.
pub const FIRM_ENTITY_NAME: &str = "Shook Law PLLC";

/// Which brand's own seeds a boot applies.
///
/// This is the third seed layer, and the only one besides the canonical set
/// that reaches production. The canonical layer is what every deployment
/// shares; the sample-matter fixture is disposable; this layer
/// is the data one brand owns and another must not carry. The Firm's postal
/// identities are the founding case: they are real, they belong in production,
/// and they belong in no other deployment's database.
///
/// A brand binary declares its own value in [`hosting::Brand`], so adding a
/// brand is a new variant plus its seed directory rather than a branch in
/// this module.
///
/// [`hosting::Brand`]: https://docs.rs/portal
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrandSeed {
    /// `neonlaw.com` — Neon Law: the firm, serving the whole site from its
    /// root.
    ///
    /// Rows stay keyed to the entity that owns them, so a further entity is a
    /// further record rather than a further variant.
    Neon,
    /// A white-label tenant deployment, which carries none of our own
    /// entities' data. This is a real value rather than an absent one: a
    /// tenant boot must be a deliberate "seed nothing", not a brand someone
    /// forgot to name.
    Tenant,
}

impl BrandSeed {
    /// The brand's short key, matching `hosting::Brand::key`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Neon => "neon",
            Self::Tenant => "tenant",
        }
    }

    /// The brand's own `Entity.yaml` as `(contents, path)`, or `None` for a
    /// brand that owns no entities beyond the shared registry.
    const fn entities(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Neon => Some((brand_seeds::NEON_ENTITY, "neon/Entity.yaml")),
            Self::Tenant => None,
        }
    }

    /// The brand's own `Mailroom.yaml` as `(contents, path)`, or `None` for a
    /// brand that rents no mail facility of its own.
    const fn mailrooms(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Neon => Some((brand_seeds::NEON_MAILROOM, "neon/Mailroom.yaml")),
            Self::Tenant => None,
        }
    }

    /// The brand's own `Address.yaml` as `(contents, path)`, embedded at
    /// compile time, or `None` for a brand that owns no addresses of ours.
    /// The path travels with the contents so a parse error names the file a
    /// reader can open rather than the brand that loaded it.
    const fn addresses(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Neon => Some((brand_seeds::NEON_ADDRESS, "neon/Address.yaml")),
            Self::Tenant => None,
        }
    }
}

/// Per-brand seeds, embedded at compile time exactly like the canonical set
/// so an installed binary carries its own brand's data with no runtime
/// lookup of `store/seeds/`.
mod brand_seeds {
    pub const NEON_ENTITY: &str = include_str!("../seeds/neon/Entity.yaml");
    pub const NEON_MAILROOM: &str = include_str!("../seeds/neon/Mailroom.yaml");
    pub const NEON_ADDRESS: &str = include_str!("../seeds/neon/Address.yaml");
}

mod canonical {
    pub const JURISDICTION: &str = include_str!("../seeds/Jurisdiction.yaml");
    pub const ENTITY_TYPE: &str = include_str!("../seeds/EntityType.yaml");
    pub const ENTITY: &str = include_str!("../seeds/Entity.yaml");
    pub const PERSON: &str = include_str!("../seeds/Person.yaml");
    pub const USER: &str = include_str!("../seeds/User.yaml");
    pub const GIT_REPOSITORY: &str = include_str!("../seeds/GitRepository.yaml");
    pub const QUESTION: &str = include_str!("../seeds/Question.yaml");
    pub const LETTER: &str = include_str!("../seeds/Letter.yaml");
    pub const ANSWER: &str = include_str!("../seeds/Answer.yaml");
    pub const PERSON_ENTITY_ROLE: &str = include_str!("../seeds/PersonEntityRole.yaml");
    pub const PERSON_PROJECT_ROLE: &str = include_str!("../seeds/PersonProjectRole.yaml");
    pub const CREDENTIAL: &str = include_str!("../seeds/Credential.yaml");
    pub const TESTIMONIAL: &str = include_str!("../seeds/Testimonial.yaml");

    /// Bundled notation templates. Each entry is `(path, full_md)`
    /// where `path` exists only as a label in the seed report.
    /// Adding a template here lets the cluster carry
    /// it without a separate `navigator catalog-seed` step. The full
    /// shipped catalog is bundled so a fresh cluster carries every
    /// template without an import pass.
    pub const TEMPLATE_RETAINER: &str = include_str!("../../templates/neon_law/shared/retainer.md");
    pub const TEMPLATE_CLOSING_LETTER: &str =
        include_str!("../../templates/neon_law/shared/closing_letter.md");
    pub const TEMPLATE_ANNUAL_REPORT_NV: &str =
        include_str!("../../templates/forms/united_states/nevada/state/nv__annual_report.md");
    pub const TEMPLATE_DISSOLUTION_NV: &str =
        include_str!("../../templates/forms/united_states/nevada/state/nv__dissolution.md");
    pub const TEMPLATE_LLC_CA: &str =
        include_str!("../../templates/neon_law/nest/ca__llc_operating_agreement.md");
    pub const TEMPLATE_FORM990: &str =
        include_str!("../../templates/forms/united_states/federal/irs/us__form_990.md");
    pub const TEMPLATE_NONPROFIT_501C3_NV: &str = include_str!(
        "../../templates/forms/united_states/nevada/state/nv__nonprofit_501c3_formation.md"
    );
    pub const TEMPLATE_CHARITABLE_SOLICITATION_NV: &str = include_str!(
        "../../templates/forms/united_states/nevada/state/nv__charitable_solicitation_registration.md"
    );
    pub const TEMPLATE_NV_MBT: &str = include_str!(
        "../../templates/forms/united_states/nevada/state/nv__modified_business_tax.md"
    );
    pub const TEMPLATE_TRUST_NV: &str =
        include_str!("../../templates/neon_law/northstar/nv__generic_trust.md");
    pub const TEMPLATE_WILL_SIMPLE: &str =
        include_str!("../../templates/neon_law/northstar/nv__simple_will.md");
    pub const TEMPLATE_ESTATE: &str =
        include_str!("../../templates/neon_law/northstar/estate_plan.md");
    // Northstar estate instrument stubs — the will, trust, and the two
    // directives the `document_drafts__estate` step renders from the
    // sitting's answers into one `review_documents` row each.
    pub const TEMPLATE_NORTHSTAR_WILL: &str =
        include_str!("../../templates/neon_law/northstar/nv__will.md");
    pub const TEMPLATE_NORTHSTAR_TRUST: &str =
        include_str!("../../templates/neon_law/northstar/nv__trust.md");
    pub const TEMPLATE_NORTHSTAR_DIRECTIVE_HEALTH: &str =
        include_str!("../../templates/neon_law/northstar/nv__directive_health.md");
    pub const TEMPLATE_NORTHSTAR_DIRECTIVE_FINANCIAL: &str =
        include_str!("../../templates/neon_law/northstar/nv__directive_financial.md");
    pub const TEMPLATE_NEST_NV: &str =
        include_str!("../../templates/forms/united_states/nevada/state/nv__llc_formation.md");
    pub const TEMPLATE_NEST_CORP_NV: &str = include_str!(
        "../../templates/forms/united_states/nevada/state/nv__profit_corp_formation.md"
    );
    pub const TEMPLATE_NEST_BUSINESS_TRUST_NV: &str = include_str!(
        "../../templates/forms/united_states/nevada/state/nv__business_trust_formation.md"
    );
    pub const TEMPLATE_NEXUS: &str =
        include_str!("../../templates/neon_law/nexus/fractional_gc.md");
    pub const TEMPLATE_EMPLOYMENT_W2: &str =
        include_str!("../../templates/neon_law/nexus/nv__employment_agreement.md");
    pub const TEMPLATE_CONTRACTOR_1099: &str =
        include_str!("../../templates/neon_law/nexus/nv__contractor_agreement.md");
    pub const TEMPLATE_CONTRACT_REVIEW: &str =
        include_str!("../../templates/neon_law/nexus/contract_review.md");
    pub const TEMPLATE_NAUTILUS_FCRA: &str =
        include_str!("../../templates/neon_law/nautilus/fcra_dispute.md");
    pub const TEMPLATE_NATURALIZATION: &str =
        include_str!("../../templates/forms/united_states/federal/uscis/us__naturalization.md");
}

/// One bundled notation template that the canonical seed inserts into the
/// shared catalog.
#[derive(Debug, Clone, Copy)]
pub struct SeededTemplate {
    pub label: &'static str,
    pub markdown: &'static str,
}

/// The full bundled notation-template catalog, in seed insertion order.
///
/// This is the canonical list consumed by both the database seeder and
/// cross-crate catalog/spec drift guards. Adding a template here makes it a
/// shared seeded template, so its code must also resolve to a questionnaire
/// through the workflow catalog or through an intentionally carried template
/// body.
pub const SEEDED_TEMPLATES: &[SeededTemplate] = &[
    SeededTemplate {
        label: "neon_law/shared/retainer.md",
        markdown: canonical::TEMPLATE_RETAINER,
    },
    SeededTemplate {
        label: "neon_law/shared/closing_letter.md",
        markdown: canonical::TEMPLATE_CLOSING_LETTER,
    },
    SeededTemplate {
        label: "forms/united_states/nevada/state/nv__annual_report.md",
        markdown: canonical::TEMPLATE_ANNUAL_REPORT_NV,
    },
    SeededTemplate {
        label: "forms/united_states/nevada/state/nv__dissolution.md",
        markdown: canonical::TEMPLATE_DISSOLUTION_NV,
    },
    SeededTemplate {
        label: "neon_law/nest/ca__llc_operating_agreement.md",
        markdown: canonical::TEMPLATE_LLC_CA,
    },
    SeededTemplate {
        label: "forms/united_states/federal/irs/us__form_990.md",
        markdown: canonical::TEMPLATE_FORM990,
    },
    SeededTemplate {
        label: "forms/united_states/nevada/state/nv__nonprofit_501c3_formation.md",
        markdown: canonical::TEMPLATE_NONPROFIT_501C3_NV,
    },
    SeededTemplate {
        label: "forms/united_states/nevada/state/nv__charitable_solicitation_registration.md",
        markdown: canonical::TEMPLATE_CHARITABLE_SOLICITATION_NV,
    },
    SeededTemplate {
        label: "forms/united_states/nevada/state/nv__modified_business_tax.md",
        markdown: canonical::TEMPLATE_NV_MBT,
    },
    SeededTemplate {
        label: "neon_law/northstar/nv__generic_trust.md",
        markdown: canonical::TEMPLATE_TRUST_NV,
    },
    SeededTemplate {
        label: "neon_law/northstar/nv__simple_will.md",
        markdown: canonical::TEMPLATE_WILL_SIMPLE,
    },
    SeededTemplate {
        label: "neon_law/northstar/estate_plan.md",
        markdown: canonical::TEMPLATE_ESTATE,
    },
    SeededTemplate {
        label: "neon_law/northstar/nv__will.md",
        markdown: canonical::TEMPLATE_NORTHSTAR_WILL,
    },
    SeededTemplate {
        label: "neon_law/northstar/nv__trust.md",
        markdown: canonical::TEMPLATE_NORTHSTAR_TRUST,
    },
    SeededTemplate {
        label: "neon_law/northstar/nv__directive_health.md",
        markdown: canonical::TEMPLATE_NORTHSTAR_DIRECTIVE_HEALTH,
    },
    SeededTemplate {
        label: "neon_law/northstar/nv__directive_financial.md",
        markdown: canonical::TEMPLATE_NORTHSTAR_DIRECTIVE_FINANCIAL,
    },
    SeededTemplate {
        label: "forms/united_states/nevada/state/nv__llc_formation.md",
        markdown: canonical::TEMPLATE_NEST_NV,
    },
    SeededTemplate {
        label: "forms/united_states/nevada/state/nv__profit_corp_formation.md",
        markdown: canonical::TEMPLATE_NEST_CORP_NV,
    },
    SeededTemplate {
        label: "forms/united_states/nevada/state/nv__business_trust_formation.md",
        markdown: canonical::TEMPLATE_NEST_BUSINESS_TRUST_NV,
    },
    SeededTemplate {
        label: "neon_law/nexus/fractional_gc.md",
        markdown: canonical::TEMPLATE_NEXUS,
    },
    SeededTemplate {
        label: "neon_law/nexus/nv__employment_agreement.md",
        markdown: canonical::TEMPLATE_EMPLOYMENT_W2,
    },
    SeededTemplate {
        label: "neon_law/nexus/nv__contractor_agreement.md",
        markdown: canonical::TEMPLATE_CONTRACTOR_1099,
    },
    SeededTemplate {
        label: "neon_law/nexus/contract_review.md",
        markdown: canonical::TEMPLATE_CONTRACT_REVIEW,
    },
    SeededTemplate {
        label: "neon_law/nautilus/fcra_dispute.md",
        markdown: canonical::TEMPLATE_NAUTILUS_FCRA,
    },
    SeededTemplate {
        label: "forms/united_states/federal/uscis/us__naturalization.md",
        markdown: canonical::TEMPLATE_NATURALIZATION,
    },
];

/// Wrap a list of records under the YAML's `records:` key. Every seed
/// YAML in `store/seeds/` has the same outer shape.
#[derive(Debug, Deserialize)]
struct Records<T> {
    #[serde(default)]
    lookup_fields: Vec<String>,
    #[serde(default = "Vec::new")]
    records: Vec<T>,
}

fn parse<T>(yaml: &str, file: &str) -> anyhow::Result<Vec<T>>
where
    T: for<'de> Deserialize<'de>,
{
    let r: Records<T> =
        serde_yaml::from_str(yaml).map_err(|e| anyhow::anyhow!("parse {file}: {e}"))?;
    Ok(r.records)
}

/// The glossary-backed Surreal tables that may be reconciled from a seed
/// document by an authenticated operator. This is deliberately a typed,
/// closed registry rather than a table-name escape hatch: each model keeps
/// its store invariants (mailbox claims, reference resolution, and firm-anchor
/// protection) on the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedModel {
    Person,
    Entity,
}

impl SeedModel {
    /// Resolve the singular glossary term and Surreal table name supplied by
    /// `navigator db seed`.
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "person" => Ok(Self::Person),
            "entity" => Ok(Self::Entity),
            _ => anyhow::bail!(
                "unsupported seed model `{value}`; supported glossary terms: person, entity"
            ),
        }
    }

    #[must_use]
    pub const fn term(self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::Entity => "entity",
        }
    }
}

/// The result of reconciling one seed document through the operator API.
#[derive(Debug, Default, Serialize)]
pub struct ReconcileReport {
    pub model: String,
    pub created: usize,
    pub updated: usize,
    pub unchanged: usize,
}

/// Reconcile one seed-shaped YAML document.
///
/// The document must use the same `lookup_fields` / `records` envelope as
/// `store/seeds`. By default an existing lookup match is left untouched; with
/// `overwrite`, the fields represented by that model's seed record replace the
/// existing values. The dispatch is typed so this authenticated path shares
/// the same natural-key and write machinery as bootstrap seeding, without
/// permitting a caller to name arbitrary SurrealQL tables or fields.
pub async fn reconcile_yaml(
    surreal: &SurrealDb,
    model: SeedModel,
    yaml: &str,
    firm_anchor: &str,
    overwrite: bool,
) -> anyhow::Result<ReconcileReport> {
    match model {
        SeedModel::Person => reconcile_people(surreal, yaml, overwrite).await,
        SeedModel::Entity => reconcile_entities(surreal, yaml, firm_anchor, overwrite).await,
    }
}

fn parse_seed<T>(
    yaml: &str,
    model: SeedModel,
    expected_lookup_fields: &[&str],
) -> anyhow::Result<Vec<T>>
where
    T: for<'de> Deserialize<'de>,
{
    let parsed: Records<T> = serde_yaml::from_str(yaml)
        .map_err(|error| anyhow::anyhow!("parse {} seed: {error}", model.term()))?;
    let supplied: Vec<&str> = parsed.lookup_fields.iter().map(String::as_str).collect();
    if supplied != expected_lookup_fields {
        anyhow::bail!(
            "{} seed must declare lookup_fields: {}",
            model.term(),
            expected_lookup_fields.join(", ")
        );
    }
    Ok(parsed.records)
}

async fn reconcile_people(
    surreal: &SurrealDb,
    yaml: &str,
    overwrite: bool,
) -> anyhow::Result<ReconcileReport> {
    let mut report = ReconcileReport {
        model: SeedModel::Person.term().to_string(),
        ..ReconcileReport::default()
    };
    for rec in parse_seed::<PersonRec>(yaml, SeedModel::Person, &["email"])? {
        let existing = crate::persons::find_by_email_ci(surreal, &rec.email).await?;
        match existing {
            None => {
                crate::persons::create(
                    surreal,
                    &crate::persons::NewPerson {
                        profile_image_url: rec.profile_image_url,
                        ..crate::persons::NewPerson::new(rec.name, rec.email)
                    },
                )
                .await?;
                report.created += 1;
            }
            Some(_) if !overwrite => report.unchanged += 1,
            Some(existing) => {
                let updated = crate::persons::edit(
                    surreal,
                    existing.id,
                    &crate::persons::PersonEdit {
                        name: Some(rec.name),
                        profile_image_url: Some(rec.profile_image_url),
                        ..crate::persons::PersonEdit::default()
                    },
                )
                .await
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                if updated.is_some() {
                    report.updated += 1;
                }
            }
        }
    }
    Ok(report)
}

async fn reconcile_entities(
    surreal: &SurrealDb,
    yaml: &str,
    firm_anchor: &str,
    overwrite: bool,
) -> anyhow::Result<ReconcileReport> {
    let mut report = ReconcileReport {
        model: SeedModel::Entity.term().to_string(),
        ..ReconcileReport::default()
    };
    for rec in parse_seed::<EntityRec>(yaml, SeedModel::Entity, &["name", "entity_type_id"])? {
        let entity_type = crate::entity_types::find_by_name(surreal, &rec.entity_type.name)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "entity seed references unknown entity type {:?}",
                    rec.entity_type.name
                )
            })?;
        let jurisdiction_name = rec
            .entity_type
            .jurisdiction
            .as_ref()
            .map_or("Nevada", |jurisdiction| jurisdiction.name.as_str());
        let jurisdiction = jurisdictions::find_by_name(surreal, jurisdiction_name)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!("entity seed references unknown jurisdiction {jurisdiction_name:?}")
            })?;
        let existing =
            crate::entities::find_by_name_and_type(surreal, &rec.name, entity_type.id).await?;
        match existing {
            None => {
                crate::entity_commands::create_entity(
                    surreal,
                    firm_anchor,
                    &crate::entity_commands::CreateEntityCommand {
                        name: rec.name,
                        entity_type_id: entity_type.id,
                        jurisdiction_id: jurisdiction.id,
                    },
                )
                .await
                .map_err(|error| anyhow::anyhow!(error.user_message()))?;
                report.created += 1;
            }
            Some(_) if !overwrite => report.unchanged += 1,
            Some(existing) => {
                crate::entity_commands::update_entity(
                    surreal,
                    existing.id,
                    firm_anchor,
                    &crate::entity_commands::UpdateEntityCommand {
                        name: rec.name,
                        entity_type_id: entity_type.id,
                        jurisdiction_id: jurisdiction.id,
                    },
                )
                .await
                .map_err(|error| anyhow::anyhow!(error.user_message()))?;
                report.updated += 1;
            }
        }
    }
    Ok(report)
}

/// Run the full canonical seed pass against `db`. Each entity table
/// is populated from its corresponding `store/seeds/*.yaml` file.
/// Idempotent: re-running inserts no new rows.
/// Apply the production-safe canonical seed: reference data plus the
/// firm-owned baseline (jurisdictions, entity types, the protected firm
/// [`FIRM_ENTITY_NAME`] Entity and its people, questions, credentials,
/// templates, products, testimonials). It is **environment-blind** — it
/// runs identically in production, so it must never insert a disposable
/// Project, mailroom, letter, or answer row. Those live in
/// [`seed_sample_portfolio`] and are applied on a `dev` profile only.
pub async fn seed_canonical(
    surreal: &SurrealDb,
    storage: &std::sync::Arc<dyn cloud::StorageService>,
) -> anyhow::Result<SeedReport> {
    let mut r = SeedReport::default();
    seed_canonical_into(surreal, storage, &mut r).await?;
    Ok(r)
}

/// Apply the compiled sample-matter fixture on top of the canonical seed.
/// It gives an environment the three synthetic matters, their participants, and
/// the reference rows the portal walkthrough needs. Applied on a `dev` profile
/// only. Idempotent: a second run inserts zero duplicates.
///
/// The profile is an argument because the fixture's portal publish consults it
/// too — a production-profile boot publishes nothing at all. See
/// [`publish_sample_portal`].
pub async fn seed_sample_portfolio(
    surreal: &SurrealDb,
    storage: &std::sync::Arc<dyn cloud::StorageService>,
    environment: crate::DeploymentEnvironment,
) -> anyhow::Result<SeedReport> {
    let mut r = SeedReport::default();
    seed_sample_portfolio_into(surreal, storage, environment, &mut r).await?;
    Ok(r)
}

/// Apply one brand's own seeds. This is production data, deliberately: the
/// Firm's postal identities are real, and they belong only in the deployment
/// that serves that brand. Idempotent on the same natural keys as every other
/// layer.
///
/// # Errors
///
/// Propagates any store error from the underlying writes.
pub async fn seed_brand(surreal: &SurrealDb, brand: BrandSeed) -> anyhow::Result<SeedReport> {
    let mut r = SeedReport::default();
    seed_brand_into(surreal, brand, &mut r).await?;
    Ok(r)
}

/// The single environment-aware orchestration call, and the three layers it
/// composes.
///
/// 1. The **canonical** seed, on every boot of every brand in every
///    environment: the shared identities, reference data, and catalog.
/// 2. The booting **brand's own** seed, likewise in every environment
///    including production. This is the layer that carries data one brand owns
///    and another must not: `neon` seeds the Firm's own mailboxes, and a
///    white-label tenant seeds none.
/// 3. The disposable **sample-matter fixture**, on a `dev` profile only, so
///    synthetic Project, mail, or answer rows can never reach a deployment
///    holding real files.
///
/// The profile is the whole predicate for layer 3, and
/// [`crate::config::sample_matters`] is **not** consulted here.
/// `NAVIGATOR_SIMULATED_MATTERS` decides one thing — whether a deployment
/// announces that its matters are simulated — and writes nothing. Keeping the
/// announcement separate from the writing is what lets a production-profile
/// deployment carry the banner over a portfolio it was given once and now
/// keeps, rather than one this seed re-asserts on every boot.
///
/// Every layer is idempotent, so a reset/recreate that runs this again
/// restores the exact same baseline.
///
/// # Errors
///
/// Propagates any store error from the underlying writes.
pub async fn seed_environment(
    surreal: &SurrealDb,
    storage: &std::sync::Arc<dyn cloud::StorageService>,
    environment: crate::DeploymentEnvironment,
    brand: BrandSeed,
) -> anyhow::Result<SeedReport> {
    seed_environment_with(surreal, storage, environment, brand).await
}

/// [`seed_environment`], with the profile as an argument so a test drives both
/// answers without mutating process environment.
///
/// # Errors
///
/// Propagates any store error from the underlying writes.
pub async fn seed_environment_with(
    surreal: &SurrealDb,
    storage: &std::sync::Arc<dyn cloud::StorageService>,
    environment: crate::DeploymentEnvironment,
    brand: BrandSeed,
) -> anyhow::Result<SeedReport> {
    let mut r = SeedReport::default();
    seed_canonical_into(surreal, storage, &mut r).await?;
    seed_brand_into(surreal, brand, &mut r).await?;
    if environment != crate::DeploymentEnvironment::Production {
        seed_sample_portfolio_into(surreal, storage, environment, &mut r).await?;
    }
    Ok(r)
}

/// The brand layer runs after [`seed_canonical_into`] because it leans on it
/// twice: its Entities resolve an entity type and a jurisdiction the canonical
/// layer seeds, and its addresses hang off Entities. Entities therefore come
/// first here too — `seed_addresses` *skips* a record whose Entity it cannot
/// resolve, so the wrong order would be a silent no-op rather than an error.
async fn seed_brand_into(
    surreal: &SurrealDb,
    brand: BrandSeed,
    r: &mut SeedReport,
) -> anyhow::Result<()> {
    if let Some((yaml, path)) = brand.entities() {
        seed_entities(surreal, yaml, path, r).await?;
    }
    if let Some((yaml, path)) = brand.mailrooms() {
        seed_mailrooms(surreal, yaml, path, r).await?;
    }
    seed_addresses(surreal, brand, r).await
}

async fn seed_canonical_into(
    surreal: &SurrealDb,
    storage: &std::sync::Arc<dyn cloud::StorageService>,
    r: &mut SeedReport,
) -> anyhow::Result<()> {
    seed_jurisdictions(surreal, r).await?;
    seed_entity_types(surreal, r).await?;
    seed_entities(surreal, canonical::ENTITY, "Entity.yaml", r).await?;
    seed_persons(surreal, r).await?;
    seed_user_roles(surreal, r).await?;
    seed_questions(surreal, r).await?;
    seed_person_entity_roles(surreal, r).await?;
    seed_credentials(surreal, r).await?;
    seed_templates(surreal, storage, r).await?;
    seed_testimonials(surreal, r).await?;
    seed_glossary_terms(surreal, r).await?;
    Ok(())
}

async fn seed_sample_portfolio_into(
    surreal: &SurrealDb,
    _storage: &std::sync::Arc<dyn cloud::StorageService>,
    environment: crate::DeploymentEnvironment,
    r: &mut SeedReport,
) -> anyhow::Result<()> {
    seed_role_matrix_sample(surreal, environment, r).await?;
    seed_git_repositories(surreal, r).await?;
    seed_letters(surreal, r).await?;
    seed_answers(surreal, r).await?;
    seed_person_project_roles(surreal, r).await?;
    Ok(())
}

/// One sample matter the development and staging fixture carries.
///
/// Three of these ship. Each is a whole matter — a client Entity, a Project
/// code that is also its portal's URL segment, the practice it demonstrates,
/// and the public repository whose Vite bundle mounts on it — so adding a
/// fourth is a row here rather than a fourth branch through the seed.
///
/// **Everything in this table is invented.** No client, matter, dispute, or
/// estate named here corresponds to a real one, which is the whole reason the
/// fixture may be published to a deployment anyone can reach.
struct SampleMatter {
    /// The Project code, which is also the URL segment its portal mounts under
    /// at `/app/projects/{code}/portal/`.
    code: &'static str,
    /// The matter as a reader sees it named.
    name: &'static str,
    /// The client Entity this matter is opened for.
    client_entity: &'static str,
    /// Whether that client is a person or a company. The two `Entity` shapes
    /// the fixture needs, resolved against the seeded entity types.
    client_kind: SampleClientKind,
    /// The practice the matter demonstrates, rendered as the Project's
    /// description.
    description: &'static str,
    /// The public repository whose built bundle mounts on this matter.
    ///
    /// Recorded on the Project rather than compiled into the command that
    /// clones it, so each sample matter demonstrates the real mechanism: a
    /// Project names its own repository, on whatever forge hosts it.
    repository_url: &'static str,
    /// The deterministic document published when no built bundle is staged.
    portal_index: &'static str,
}

/// Which seeded entity type a sample matter's client resolves to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SampleClientKind {
    /// A natural person — the individual plaintiff and the individual testator.
    Human,
    /// A Nevada C-Corp — the company that engages the firm as outside counsel.
    Company,
}

impl SampleClientKind {
    /// The seeded `entity_types.name` this kind resolves to.
    const fn entity_type(self) -> &'static str {
        match self {
            Self::Human => "Human",
            Self::Company => "C-Corp",
        }
    }
}

/// The three sample matters, in the order a reader meets them.
///
/// They are three deliberately different shapes of legal work, because one
/// matter could only ever demonstrate one: a dispute in front of a court, a
/// company on a monthly retainer, and an estate plan. A person signing in as
/// the fixture Client sees all three at once, which is what makes the
/// participation-scoped list worth looking at.
const SAMPLE_MATTERS: &[SampleMatter] = &[
    SampleMatter {
        code: SAMPLE_LITIGATION_CODE,
        name: "Cruller v. Prine",
        client_entity: "Dermot Cruller",
        client_kind: SampleClientKind::Human,
        description: "trespass to land, and rescission of the doughnut instrument",
        repository_url: "https://github.com/neon-law-staging/sample-litigation",
        portal_index: SAMPLE_LITIGATION_PORTAL_INDEX,
    },
    SampleMatter {
        code: SAMPLE_TRANSACTIONAL_CODE,
        name: "Widget Works — Outside Counsel",
        client_entity: "Widget Works, Inc.",
        client_kind: SampleClientKind::Company,
        description: "employment agreements and contract review on a monthly retainer",
        repository_url: "https://github.com/neon-law-staging/sample-transactional",
        portal_index: SAMPLE_TRANSACTIONAL_PORTAL_INDEX,
    },
    SampleMatter {
        code: SAMPLE_ESTATE_CODE,
        name: "Estate of Cornelius Montgomery",
        client_entity: "Cornelius Montgomery",
        client_kind: SampleClientKind::Human,
        description: "estate plan dividing the residue among nieces and nephews",
        repository_url: "https://github.com/neon-law-staging/sample-estate",
        portal_index: SAMPLE_ESTATE_PORTAL_INDEX,
    },
];

/// The environment variable naming the deployment's bootstrap owner.
pub const BOOTSTRAP_OWNER_EMAIL_ENV: &str = "NAVIGATOR_BOOTSTRAP_OWNER_EMAIL";

/// The bootstrap owner a tier falls back to when it configures none — the KIND
/// Rauthy fixture's Owner, so a local sample matter's DRI is an account a
/// developer can sign in as.
const DEFAULT_BOOTSTRAP_OWNER_EMAIL: &str = "owner@neonlaw.com";

/// The email of the identity that owns this deployment.
///
/// The same variable the OIDC callback reads to decide which unseeded identity
/// may create itself as `owner`, so the sample portfolio's accountable lawyer
/// and the deployment's bootstrap identity cannot drift apart. A blank value is
/// treated as unset rather than as an empty address.
#[must_use]
pub fn bootstrap_owner_email() -> String {
    std::env::var(BOOTSTRAP_OWNER_EMAIL_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_BOOTSTRAP_OWNER_EMAIL.to_string())
}

/// The disputes matter's Project code, and therefore its portal's URL segment.
///
/// Exposed because the browser walkthrough and the `dev sample-project` command
/// both name the matter they drive, and a code that drifted between the seed
/// and either of them would 404 at the mount rather than fail a test.
pub const SAMPLE_LITIGATION_CODE: &str = "sample-litigation";

/// The transactional matter's Project code.
pub const SAMPLE_TRANSACTIONAL_CODE: &str = "sample-transactional";

/// The estate matter's Project code.
pub const SAMPLE_ESTATE_CODE: &str = "sample-estate";

/// Look up one sample matter by its Project code.
#[must_use]
pub fn sample_matter_codes() -> Vec<&'static str> {
    SAMPLE_MATTERS.iter().map(|matter| matter.code).collect()
}

/// Every sample matter's caption, in table order.
///
/// Exposed so a test asserting what a participant sees can derive the list
/// rather than restate it: a fourth matter would otherwise silently narrow
/// what those tests check to the three they were written against.
#[must_use]
pub fn sample_matter_names() -> Vec<&'static str> {
    SAMPLE_MATTERS.iter().map(|matter| matter.name).collect()
}

/// The public repository whose bundle mounts on one sample matter, or
/// `None` for a code that names no sample matter.
#[must_use]
pub fn sample_matter_repository(code: &str) -> Option<&'static str> {
    SAMPLE_MATTERS
        .iter()
        .find(|matter| matter.code == code)
        .map(|matter| matter.repository_url)
}

/// Build one sample matter's deterministic portal document.
///
/// A self-contained static page — no inline `<script>`, because the portal
/// serve CSP is `script-src 'self'` — so it renders under the same
/// participation-gated stream a real Vite bundle would. `id="{code}-portal-ready"`
/// is the hook the browser walkthrough looks for, which is why the code is
/// interpolated rather than written out per matter.
macro_rules! portal_index {
    ($code:expr, $title:expr, $subtitle:expr, $steps:expr) => {
        concat!(
            r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>"#,
            $title,
            r#" — Client Portal</title>
<style>
  :root { color-scheme: light dark; }
  body { margin: 0; font-family: -apple-system, system-ui, sans-serif; background: #0f1216; color: #e8eef5; }
  header { padding: 2.5rem 2rem 2rem; background: linear-gradient(135deg, #1b2430, #0f1216); border-bottom: 1px solid #263140; }
  .pill { display: inline-block; background: #12351f; color: #57d98a; border-radius: 999px; padding: .2rem .7rem; font-size: .8rem; }
  h1 { margin: .5rem 0 .25rem; font-size: 1.7rem; }
  .sub { color: #8aa0b6; }
  main { padding: 2rem; max-width: 720px; }
  .card { background: #151b22; border: 1px solid #263140; border-radius: 12px; padding: 1.25rem 1.5rem; margin-bottom: 1rem; }
  .card h2 { margin: .1rem 0 .6rem; font-size: 1.05rem; }
  ul { margin: .4rem 0; padding-left: 1.2rem; } li { margin: .3rem 0; }
  footer { padding: 1.5rem 2rem; color: #5f7085; font-size: .85rem; }
</style>
</head>
<body>
<header>
  <div class="pill" id=""#,
            $code,
            r#"-portal-ready">Client portal · live</div>
  <h1>"#,
            $title,
            r#"</h1>
  <div class="sub">"#,
            $subtitle,
            r#"</div>
</header>
<main>
  <div class="card">
    <h2>Where things stand</h2>
    <p>This is the client portal application served for your matter, streamed from Navigator's per-deployment applications bucket.</p>
  </div>
  <div class="card">
    <h2>Next steps</h2>
    <ul>
"#,
            $steps,
            r#"
    </ul>
  </div>
</main>
<footer>Fixture data only — "#,
            $title,
            r#" is a sample matter.</footer>
</body>
</html>
"#
        )
    };
}

const SAMPLE_LITIGATION_PORTAL_INDEX: &str = portal_index!(
    "sample-litigation",
    "Cruller v. Prine",
    "Trespass and rescission — your matter workspace",
    "      <li>Review the complaint draft</li>
      <li>Confirm the discovery timeline</li>
      <li>Message your legal team with questions</li>"
);

const SAMPLE_TRANSACTIONAL_PORTAL_INDEX: &str = portal_index!(
    "sample-transactional",
    "Widget Works — Outside Counsel",
    "Employment agreements and contract review — your matter workspace",
    "      <li>Review this month's employment agreement queue</li>
      <li>Send a Redline contract for one-business-day turnaround</li>
      <li>Message your legal team with questions</li>"
);

const SAMPLE_ESTATE_PORTAL_INDEX: &str = portal_index!(
    "sample-estate",
    "Estate of Cornelius Montgomery",
    "Estate plan — your matter workspace",
    "      <li>Confirm the list of nieces and nephews</li>
      <li>Review the draft will and trust</li>
      <li>Message your legal team with questions</li>"
);

/// Seed the sample matters every fixture login lands on, with a participant
/// for each firm and client tier so the same projects can be opened through
/// every lens the KIND Rauthy fixture signs in as — including Owner, who
/// carries a firm-side row so they appear in an Owner's participation-scoped
/// `/app/projects` list.
///
/// Idempotent, and it publishes each matter's portal "little app" to the
/// applications bucket so `/app/projects/{code}/portal/` streams rather than
/// 404s. The publish is best-effort twice over: a tier without an applications
/// bucket configured logs and skips rather than failing the whole seed, and a
/// tier on the production profile declines to overwrite what an operator
/// already published there — see [`publish_sample_portal`], which is why the
/// profile travels this far down.
async fn seed_role_matrix_sample(
    surreal: &SurrealDb,
    environment: crate::DeploymentEnvironment,
    report: &mut SeedReport,
) -> anyhow::Result<()> {
    let nevada = jurisdictions::find_by_name(surreal, "Nevada")
        .await?
        .ok_or_else(|| anyhow::anyhow!("seed: jurisdiction `Nevada` must be seeded first"))?;

    // One person per tier, matching the KIND Rauthy fixture's five accounts.
    let owner_id = ensure_dev_person(
        surreal,
        report,
        "Olive Owner",
        "owner@neonlaw.com",
        crate::persons::Role::Owner,
    )
    .await?;
    // The Admin Person exists so the fixture account can sign in; it is
    // deliberately given no participation on any of these matters (see below),
    // so its id is not bound.
    ensure_dev_person(
        surreal,
        report,
        "Ada Admin",
        "admin@neonlaw.com",
        crate::persons::Role::Admin,
    )
    .await?;
    let lawyer_id = ensure_dev_person(
        surreal,
        report,
        "Lawrence Lawyer",
        "lawyer@neonlaw.com",
        crate::persons::Role::Lawyer,
    )
    .await?;
    let clerk_id = ensure_dev_person(
        surreal,
        report,
        "Clara Clerk",
        "clerk@neonlaw.com",
        crate::persons::Role::Clerk,
    )
    .await?;
    let client_id = ensure_dev_person(
        surreal,
        report,
        "Cleo Client",
        "client@neonlaw.com",
        crate::persons::Role::Client,
    )
    .await?;

    // The lawyer DRI is the deployment's bootstrap owner — the one identity a
    // deployment is guaranteed to have, created on first login when nothing else
    // is seeded. A local tier resolves it to the fixture Owner, so the
    // accountable side of a sample matter stays an account a developer can
    // sign in as.
    let dri_email = bootstrap_owner_email();
    let dri_id = if dri_email.eq_ignore_ascii_case("owner@neonlaw.com") {
        owner_id
    } else {
        ensure_dev_person(
            surreal,
            report,
            "Navigator Owner",
            &dri_email,
            crate::persons::Role::Owner,
        )
        .await?
    };

    let cast = FixtureCast {
        owner: owner_id,
        lawyer: lawyer_id,
        clerk: clerk_id,
        client: client_id,
        dri: dri_id,
    };
    let applications = match cloud::applications_from_env().await {
        Ok(applications) => Some(applications),
        Err(error) => {
            tracing::warn!(%error, "seed: no applications bucket; skipping the sample portals");
            None
        }
    };

    for matter in SAMPLE_MATTERS {
        open_sample_matter(
            surreal,
            report,
            matter,
            &cast,
            nevada.id,
            applications.as_ref(),
            environment,
        )
        .await?;
    }
    Ok(())
}

/// The four fixture accounts that get a participation row on every sample
/// matter, resolved once.
///
/// Admin is deliberately absent, and its absence is the point — see
/// [`open_sample_matter`]. Grouping the four into one value is what keeps the
/// per-matter half a readable argument list rather than seven positional ids.
struct FixtureCast {
    owner: Uuid,
    lawyer: Uuid,
    clerk: Uuid,
    client: Uuid,
    /// The lawyer DRI on every sample matter: the deployment's bootstrap
    /// owner. Equal to `owner` when that identity *is* the fixture Owner, which
    /// is the local default.
    dri: Uuid,
}

/// Open one sample matter: its client Entity, its Project, its repository
/// pointer, its participation rows, its two DRIs, and its portal bundle.
///
/// Split from [`seed_role_matrix_sample`] because the two halves answer
/// different questions — who the fixture accounts are, and what one matter is —
/// and only the second one repeats.
async fn open_sample_matter(
    surreal: &SurrealDb,
    report: &mut SeedReport,
    matter: &SampleMatter,
    cast: &FixtureCast,
    jurisdiction_id: Uuid,
    applications: Option<&std::sync::Arc<dyn cloud::StorageService>>,
    environment: crate::DeploymentEnvironment,
) -> anyhow::Result<()> {
    let FixtureCast {
        owner: owner_id,
        lawyer: lawyer_id,
        clerk: clerk_id,
        client: client_id,
        dri: dri_id,
    } = *cast;
    {
        let entity_type =
            crate::entity_types::find_by_name(surreal, matter.client_kind.entity_type())
                .await?
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "seed: entity_type `{}` must be seeded first",
                        matter.client_kind.entity_type()
                    )
                })?;
        let entity_id = ensure_dev_entity(
            surreal,
            report,
            matter.client_entity,
            entity_type.id,
            jurisdiction_id,
        )
        .await?;
        let project_id = ensure_dev_project(
            surreal,
            report,
            matter.code,
            matter.name,
            entity_id,
            matter.description,
        )
        .await?;

        crate::projects::set_repository_url(surreal, project_id, Some(matter.repository_url))
            .await?;

        // Client side and firm side. The disclosed lawyer is what lets the
        // supervised Clerk resolve the matter.
        ensure_participation(surreal, report, project_id, client_id, "client").await?;
        ensure_participation(surreal, report, project_id, lawyer_id, "attorney").await?;
        ensure_participation(surreal, report, project_id, clerk_id, "clerk").await?;
        // Owner gets a firm-side row so the demo matters appear in the Owner's
        // own list: since ENG-81 the whole matter surface — `/app/projects` and
        // `/app/projects/{code}` alike — is participation-scoped for every tier,
        // with no privileged bypass.
        ensure_participation(surreal, report, project_id, owner_id, "owner").await?;
        // The DRI needs a participation row of its own: the matter surface is
        // participation-scoped for every tier, so accountability without
        // participation would name a DRI who cannot open the matter.
        if dri_id != owner_id {
            ensure_participation(surreal, report, project_id, dri_id, "owner").await?;
        }
        // Admin deliberately gets **no** row. These matters are what an
        // administrator who was never assigned to a matter looks like, which is
        // the ENG-81 decision made visible: privileged reach is a place you
        // navigate to (`/app/admin`, which reads and writes the participation
        // ledger), not a silent widening of a shared route. Because one row
        // gates both the list and the detail view, this is also why none of
        // these appear in an admin's `/app/projects` list — the absence is the
        // point, not an oversight.
        // The lawyer DRI is the deployment's bootstrap owner — the one identity
        // a deployment is guaranteed to have, created on first login when
        // nothing else is seeded. Naming it rather than the fixture lawyer keeps
        // the accountable side of every sample matter pointing at whoever
        // actually runs the deployment.
        crate::projects::designate_dri_in_surreal(
            surreal,
            project_id,
            dri_id,
            crate::projects::DriSide::Lawyer,
        )
        .await?;
        crate::projects::designate_dri_in_surreal(
            surreal,
            project_id,
            client_id,
            crate::projects::DriSide::Client,
        )
        .await?;

        if let Some(applications) = applications {
            publish_sample_portal(applications, matter, environment).await?;
        }
    }
    Ok(())
}

/// Publish one sample matter's client portal, preferring a locally built
/// bundle.
///
/// Local `dev` boot clones and builds each matter's repository and stages it,
/// naming the staged directories in [`crate::sample_project::STAGE_ENV`]. Boot
/// publishes the bundle whose manifest names this matter; the generated
/// environment points the web process at the staged directories.
///
/// A staged bundle must declare its Project in `navigator.yml`. Publishing a
/// bundle that names another matter would put one client's application on
/// another client's portal, so a mismatch — like an unbuilt or unparsable
/// staging directory — leaves the deterministic document in place and is
/// reported as a failed local application build.
///
/// # One rule per profile
///
/// **Production writes nothing at all**, staged bundle or not. Whatever sits in
/// the deployment's applications bucket is authoritative: a client portal
/// application there was published by an operator or by a Project repository's
/// CI, and the seed has no business writing into that prefix. Overwriting
/// `index.html` with the placeholder is quiet and partial — the hashed assets
/// survive, so the portal renders the placeholder while the complete bundle
/// sits unreferenced in the same prefix — and a 404 is the honest signal that
/// nothing has published there yet.
///
/// **`dev` prefers the staged bundle and falls back to the deterministic
/// document.** That fallback is the case this function was written for: it
/// keeps a developer's portal serving something while a Vite build is broken.
///
/// **A test gets the deterministic document**, and does so by construction
/// rather than by a profile check: a test stages no bundle and injects its own
/// `get`, so the staged branch is simply not reachable. There is deliberately
/// no environment flag for the test lane. `NAVIGATOR_CI_HARNESS` is the
/// nearest candidate and is the wrong question — `.devx/env` sets it for the
/// ordinary local loop, so keying on it would stop a developer's own build
/// from ever publishing.
async fn publish_sample_portal(
    applications: &std::sync::Arc<dyn cloud::StorageService>,
    matter: &SampleMatter,
    environment: crate::DeploymentEnvironment,
) -> anyhow::Result<()> {
    publish_sample_portal_with(applications, matter, environment, |key| {
        std::env::var(key).ok()
    })
    .await
}

/// [`publish_sample_portal`] with the staging directory read through `get`, so
/// a test drives the staged and unstaged branches without depending on process
/// environment — a sourced `.devx/env` sets
/// [`crate::sample_project::STAGE_ENV`], and would otherwise decide which
/// branch these assertions exercise.
async fn publish_sample_portal_with<F: Fn(&str) -> Option<String>>(
    applications: &std::sync::Arc<dyn cloud::StorageService>,
    matter: &SampleMatter,
    environment: crate::DeploymentEnvironment,
    get: F,
) -> anyhow::Result<()> {
    // Production first, and before anything reads the staging directory: this
    // profile writes nothing into the applications bucket on any path.
    if environment == crate::DeploymentEnvironment::Production {
        tracing::info!(
            code = matter.code,
            "seed: production profile; leaving the published portal application alone"
        );
        return Ok(());
    }

    if let Some(staged) = crate::sample_project::staged_from(matter.code, get) {
        match publish_staged_portal(applications, &staged, matter.code).await {
            Ok(count) => {
                tracing::info!(
                    code = matter.code,
                    root = %staged.root.display(),
                    objects = count,
                    "seed: published the built sample project portal"
                );
                return Ok(());
            }
            Err(error) => {
                // Keep the deterministic portal document available while the
                // local application build is repaired.
                tracing::warn!(
                    code = matter.code,
                    root = %staged.root.display(),
                    %error,
                    "seed: staged sample project unusable; keeping the portal document"
                );
            }
        }
    }

    applications
        .put_cached(
            &format!(
                "{}/{}",
                crate::sample_project::portal_prefix(matter.code),
                crate::sample_project::ENTRY_DOCUMENT
            ),
            matter.portal_index.as_bytes(),
            "text/html; charset=utf-8",
            crate::sample_project::ENTRY_CACHE_CONTROL,
        )
        .await?;
    Ok(())
}

/// Publish one staged bundle, returning how many objects landed. Every failure
/// mode — missing manifest, wrong Project, or unbuilt `dist/` — is returned as
/// an error for the local boot to report.
async fn publish_staged_portal(
    applications: &std::sync::Arc<dyn cloud::StorageService>,
    staged: &crate::sample_project::StagedProject,
    expected_code: &str,
) -> anyhow::Result<usize> {
    let manifest = std::fs::read_to_string(staged.manifest()).with_context(|| {
        format!(
            "reading {} — a project application must declare its Project",
            staged.manifest().display()
        )
    })?;
    let code = crate::sample_project::project_code_for(&manifest, expected_code)?;

    let plan = crate::sample_project::publish_plan(&staged.dist, &code)?;
    anyhow::ensure!(
        !plan.is_empty(),
        "{} has no {} — that is a failed build, not a bundle",
        staged.dist.display(),
        crate::sample_project::ENTRY_DOCUMENT
    );

    let count = plan.len();
    for object in plan {
        let bytes = std::fs::read(&object.source)?;
        applications
            .put_cached(
                &object.key,
                &bytes,
                object.content_type,
                object.cache_control,
            )
            .await?;
    }
    Ok(count)
}

async fn ensure_dev_person(
    surreal: &SurrealDb,
    report: &mut SeedReport,
    name: &str,
    email: &str,
    role: crate::persons::Role,
) -> anyhow::Result<Uuid> {
    let existing = crate::persons::find_by_email_ci(surreal, email).await?;
    let row = crate::persons::find_or_create(
        surreal,
        &crate::persons::NewPerson::with_role(name, email, role),
    )
    .await?;
    if existing.is_none() {
        report.persons_inserted += 1;
        return Ok(row.id);
    }
    if row.name != name || row.role != role {
        crate::persons::edit(
            surreal,
            row.id,
            &crate::persons::PersonEdit {
                name: Some(name.into()),
                role: Some(role),
                ..crate::persons::PersonEdit::default()
            },
        )
        .await?;
        report.persons_updated += 1;
    }
    Ok(row.id)
}

/// Idempotently ensure one sample matter's client Entity, of whatever type
/// that matter's client is. The type is a parameter rather than fixed to
/// `Human` because the fixture carries a company client as well as two people,
/// and a company engaging outside counsel is a different Entity shape from an
/// individual plaintiff.
async fn ensure_dev_entity(
    surreal: &SurrealDb,
    report: &mut SeedReport,
    name: &str,
    entity_type_id: Uuid,
    jurisdiction_id: Uuid,
) -> anyhow::Result<Uuid> {
    if let Some(row) = crate::entities::find_by_name_and_type(surreal, name, entity_type_id).await?
    {
        // Same repair as `seed_entities`: a persisted row's
        // `jurisdiction_id` may name a jurisdiction the reset local
        // engine no longer holds, and the engine never validated it.
        if row.jurisdiction_id != jurisdiction_id {
            crate::entities::repoint_jurisdiction(surreal, row.id, jurisdiction_id).await?;
        }
        return Ok(row.id);
    }
    let row = crate::entities::create(surreal, &new_entity(name, entity_type_id, jurisdiction_id))
        .await?;
    report.entities_inserted += 1;
    Ok(row.id)
}

/// The write shape every seeded Entity shares.
///
/// The firm's own row is the reason this is a helper rather than a
/// literal: it must carry `firm_anchor_key`, or the seeded firm would be
/// forkable through `entity_commands` — the guard reads the key, and a
/// row seeded without one leaves the `entity_firm_anchor` index free.
/// The shipped default stands in for the configured anchor here because
/// `is_firm_anchor` protects it under every configuration, and the
/// canonical seed ships no other firm.
fn new_entity(
    name: &str,
    entity_type_id: Uuid,
    jurisdiction_id: Uuid,
) -> crate::entities::NewEntity {
    crate::entities::NewEntity {
        name: name.to_string(),
        entity_type_id,
        jurisdiction_id,
        phone: None,
        url: None,
        firm_anchor_key: crate::entity_commands::firm_anchor_key(FIRM_ENTITY_NAME, name),
    }
}

async fn ensure_dev_project(
    surreal: &SurrealDb,
    report: &mut SeedReport,
    code: &str,
    name: &str,
    entity_id: Uuid,
    description: &str,
) -> anyhow::Result<Uuid> {
    let input = crate::projects::NewProject {
        code: code.to_string(),
        name: name.to_string(),
        status: "open".to_string(),
        entity_id,
        description: Some(description.to_string()),
    };
    let row = match crate::projects::find_by_code(surreal, code).await? {
        Some(existing) => crate::projects::upsert_with_id(surreal, existing.id, &input).await?,
        None => crate::projects::find_or_create_by_code(surreal, Uuid::now_v7(), &input).await?,
    };
    report.projects_inserted += 1;
    Ok(row.id)
}

/// Idempotently record one person's participation on a project.
async fn ensure_participation(
    surreal: &SurrealDb,
    report: &mut SeedReport,
    project_id: Uuid,
    person_id: Uuid,
    participation: &str,
) -> anyhow::Result<()> {
    if let Some(row) =
        crate::projects::participation_for_person(surreal, person_id, project_id).await?
    {
        // Restore the seeded participation if a persisted database drifted it:
        // the client/lawyer visibility this fixture demonstrates depends on the
        // exact value, so a stale `paralegal` or blank row must be reconciled.
        if row.participation != participation {
            crate::projects::update_participation(surreal, row.id, person_id, participation)
                .await?;
        }
        return Ok(());
    }
    ensure_participation_in_surreal(surreal, project_id, person_id, participation).await?;
    report.person_project_roles_inserted += 1;
    Ok(())
}

/// Reconcile one participation row in the Surreal ledger. Split out because
/// the memory-backed local engine resets with its pod, so this must
/// converge whether the row is already there or not.
async fn ensure_participation_in_surreal(
    surreal: &SurrealDb,
    project_id: Uuid,
    person_id: Uuid,
    participation: &str,
) -> anyhow::Result<()> {
    match crate::projects::participation_for_person(surreal, person_id, project_id).await? {
        Some(existing) if existing.participation == participation => Ok(()),
        Some(existing) => {
            crate::projects::update_participation(surreal, existing.id, person_id, participation)
                .await?;
            Ok(())
        }
        None => {
            match crate::projects::add_participation(surreal, project_id, person_id, participation)
                .await
            {
                Ok(_) => Ok(()),
                // `person_project_role_pair` is UNIQUE, so a concurrent seed
                // may have filed this pair between the read above and this
                // write. Losing that race is not an error — adopt the winner's
                // row and reconcile its value, the same settle-on-one-row
                // contract `find_or_create_by_code` gives the project itself.
                Err(err) if err.to_string().contains("person_project_role_pair") => {
                    if let Some(existing) =
                        crate::projects::participation_for_person(surreal, person_id, project_id)
                            .await?
                    {
                        if existing.participation != participation {
                            crate::projects::update_participation(
                                surreal,
                                existing.id,
                                person_id,
                                participation,
                            )
                            .await?;
                        }
                    }
                    Ok(())
                }
                Err(err) => Err(err.into()),
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct TemplateFrontmatter {
    code: String,
    title: String,
    respondent_type: String,
    /// forms-registry code of the government form this template fills
    /// (`form: nv__llc_formation`); absent for Typst-rendered
    /// templates.
    #[serde(default)]
    form: Option<String>,
    /// Declared notation kind (`retainer`/`letter`/`filing`) from the
    /// `kind:` key; `None` until declared.
    #[serde(default)]
    kind: Option<String>,
}

/// Template codes in the shared seeded catalog.
///
/// The codes are parsed from the same frontmatter the seeder uses, so a
/// cross-crate guard can derive its coverage from the actual catalog instead
/// of maintaining a second list by hand.
pub fn seeded_template_codes() -> anyhow::Result<Vec<String>> {
    SEEDED_TEMPLATES
        .iter()
        .map(|template| {
            let (fm_str, _) = split_template(template.markdown)
                .ok_or_else(|| anyhow::anyhow!("{}: missing YAML frontmatter", template.label))?;
            let fm: TemplateFrontmatter = serde_yaml::from_str(fm_str)
                .map_err(|e| anyhow::anyhow!("{}: parse frontmatter: {e}", template.label))?;
            Ok(fm.code)
        })
        .collect()
}

/// Split a notation template's markdown into `(frontmatter, body)`.
/// The frontmatter is the YAML between the opening and closing `---`
/// markers; the body is everything after.
///
/// Accepts both LF and CRLF line endings, mirroring
/// `rules::frontmatter::extract`. The two parsers exist for different
/// call paths — that one validates other repositories' files, this one
/// splits the templates bundled into this binary — but they read the
/// same delimiters and must agree about them.
///
/// The CRLF arm is not hypothetical. These templates reach the parser
/// through `include_str!`, which reads the working tree at compile
/// time, so the line endings of whichever checkout compiled the binary
/// are baked in with no runtime opportunity to normalise. Git for
/// Windows defaults to `core.autocrlf=true` and the tree carries no
/// `.gitattributes`, so a Windows checkout materialises every template
/// with `\r\n`; probing only for `\n` made this return `None` for all
/// of them, and both callers turn `None` into a hard error on the boot
/// path.
///
/// The returned slices are borrowed and so may retain interior `\r`.
/// `serde_yaml` accepts that in the frontmatter. For the body, see the
/// normalisation note at the ingest call in `seed_templates`.
fn split_template(md: &str) -> Option<(&str, &str)> {
    let after_open = md
        .strip_prefix("---\n")
        .or_else(|| md.strip_prefix("---\r\n"))?;

    // Empty frontmatter: the closer immediately follows the opener.
    if let Some(body) = after_open
        .strip_prefix("---\n")
        .or_else(|| after_open.strip_prefix("---\r\n"))
    {
        return Some(("", body));
    }
    if after_open == "---" || after_open == "---\r" {
        return Some(("", ""));
    }

    if let Some((end, delim_len)) = find_closer(after_open) {
        return Some((&after_open[..end], &after_open[end + delim_len..]));
    }

    // Closer at EOF with no trailing newline, so no body follows.
    let fm = after_open
        .strip_suffix("\r\n---")
        .or_else(|| after_open.strip_suffix("\n---"))?;
    Some((fm, ""))
}

/// Byte offset of the closing `---` delimiter line within the text
/// following the opener, paired with that delimiter's own length so the
/// caller can slice the body that starts after it.
///
/// Takes the earlier of the two matches, so a file with mixed endings
/// closes at the first real delimiter rather than the first *LF* one.
/// `"\n---\n"` cannot match inside `"\r\n---\r\n"`, so the two probes
/// never overlap on a single delimiter and the earlier offset is always
/// a genuine closer.
fn find_closer(after_open: &str) -> Option<(usize, usize)> {
    const LF: &str = "\n---\n";
    const CRLF: &str = "\r\n---\r\n";
    match (after_open.find(LF), after_open.find(CRLF)) {
        (Some(lf), Some(crlf)) => {
            if lf <= crlf {
                Some((lf, LF.len()))
            } else {
                Some((crlf, CRLF.len()))
            }
        }
        (Some(lf), None) => Some((lf, LF.len())),
        (None, Some(crlf)) => Some((crlf, CRLF.len())),
        (None, None) => None,
    }
}

/// The exact bytes of a template body that get content-addressed, with
/// leading whitespace trimmed and line endings normalised to LF.
///
/// The normalisation is load-bearing rather than tidiness. These bytes
/// decide the `asset_id`, and `templates::save_version` is immutable by
/// policy: a body whose bytes differ from the stored version appends a
/// new current version and retires the prior one. Because `include_str!`
/// bakes in the compiling checkout's line endings, hashing them raw
/// would make a template's identity depend on which platform built the
/// binary — the same logical document would fork into two versions, and
/// a Notation pinned to one would resolve to bytes the other tier never
/// writes.
///
/// This is a no-op for every deployment that exists: images are built on
/// Linux, where these bodies are already LF, so the bytes are unchanged
/// and no new version is written. What it does is keep that true if an
/// image is ever built from a CRLF checkout. The borrowed arm means the
/// LF path stays allocation-free and provably identical to the bytes
/// hashed before this function existed.
///
/// Only the body is normalised. The frontmatter slice is left as
/// `split_template` returned it, interior `\r` and all, because
/// `serde_yaml` accepts that and it is the parsed values that get
/// stored, not the raw slice.
fn normalized_body_bytes(body: &str) -> std::borrow::Cow<'_, [u8]> {
    let trimmed = body.trim_start();
    if trimmed.contains('\r') {
        std::borrow::Cow::Owned(trimmed.replace("\r\n", "\n").into_bytes())
    } else {
        std::borrow::Cow::Borrowed(trimmed.as_bytes())
    }
}

/// Seed the workspace-bundled notation templates into the
/// `templates` table. Idempotent on `code` — re-running is a
/// no-op. The full shipped catalog is bundled; add more
/// `include_str!` entries in `canonical` above and a row here to
/// extend.
#[allow(clippy::too_many_lines)]
async fn seed_templates(
    surreal: &SurrealDb,
    storage: &std::sync::Arc<dyn cloud::StorageService>,
    report: &mut SeedReport,
) -> anyhow::Result<()> {
    for template in SEEDED_TEMPLATES {
        let label = template.label;
        let md = template.markdown;
        let (fm_str, body) = split_template(md)
            .ok_or_else(|| anyhow::anyhow!("{label}: missing YAML frontmatter"))?;
        let fm: TemplateFrontmatter = serde_yaml::from_str(fm_str)
            .map_err(|e| anyhow::anyhow!("{label}: parse frontmatter: {e}"))?;

        // The body lives in a content-addressed asset; ingest it (sha
        // dedup) and reference it by `asset_id`. See
        // `normalized_body_bytes` for why the line endings are normalised
        // before hashing.
        let body_bytes = normalized_body_bytes(body);
        let asset_id =
            crate::assets::ingest_content(surreal, storage, &body_bytes, "text/markdown")
                .await
                .map_err(|e| anyhow::anyhow!("{label}: ingest body asset: {e}"))?;

        // Immutable by policy: a fresh cluster writes the first version;
        // an unchanged re-seed is a no-op; a changed body/form/title
        // appends a new current version and retires the prior one, so a
        // Notation already opened against the old bytes keeps resolving to
        // them (`notation.template_id` pins the version).
        let saved = crate::templates::save_version(
            surreal,
            None,
            &fm.code,
            crate::templates::Version {
                title: fm.title,
                respondent_type: fm.respondent_type,
                asset_id: Some(asset_id),
                form_code: fm.form,
                kind: fm.kind,
                // The seeded workspace catalog comes from bundled files,
                // not a git repo — no commit provenance.
                source_commit_sha: None,
            },
        )
        .await?;
        if saved.was_written() {
            report.templates_inserted += 1;
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct CredentialRec {
    person: PersonEmailRef,
    jurisdiction: JurisdictionRef,
    license_number: String,
}

async fn seed_credentials(surreal: &SurrealDb, report: &mut SeedReport) -> anyhow::Result<()> {
    for rec in parse::<CredentialRec>(canonical::CREDENTIAL, "Credential.yaml")? {
        let Some(p) = crate::persons::find_by_email_ci(surreal, &rec.person.email).await? else {
            continue;
        };
        let Some(j) = jurisdictions::find_by_name(surreal, &rec.jurisdiction.name).await? else {
            continue;
        };
        // Find-or-grant rather than read-then-write: the seed runs on
        // every boot, and two boots racing would otherwise both miss the
        // read and collide on `credential_person_jurisdiction`.
        let before = crate::credentials::find_by_person_and_jurisdiction(surreal, p.id, j.id)
            .await?
            .is_some();
        crate::credentials::find_or_grant(surreal, p.id, j.id, &rec.license_number).await?;
        if !before {
            report.credentials_inserted += 1;
        }
    }
    Ok(())
}

// ---------- Per-entity loaders ----------

#[derive(Debug, Deserialize)]
struct JurisdictionRec {
    name: String,
    code: String,
    jurisdiction_type: String,
}

/// Materialize the authored firm glossary into `glossary_term` rows — in
/// SurrealDB, where the table lives since its slice of #1093 (ENG-20).
///
/// Reference data, so it sits in the canonical seed beside jurisdictions:
/// environment-blind, identical in every deployment, and idempotent — the
/// write is keyed on slug, so an edited definition converges rather than
/// appending a second row. It inserts nothing matter-scoped, which is why
/// it is safe in production.
async fn seed_glossary_terms(surreal: &SurrealDb, report: &mut SeedReport) -> anyhow::Result<()> {
    report.glossary_terms_written +=
        crate::glossary::materialize(surreal, crate::glossary::GLOSSARY_MD).await?;
    Ok(())
}

/// Seed the jurisdiction reference table — into SurrealDB, where the
/// table lives since its slice of #1093 (ENG-20). Canonical seed data:
/// it runs identically in every environment, production included, and
/// is idempotent on `code`.
async fn seed_jurisdictions(surreal: &SurrealDb, report: &mut SeedReport) -> anyhow::Result<()> {
    for rec in parse::<JurisdictionRec>(canonical::JURISDICTION, "Jurisdiction.yaml")? {
        if jurisdictions::find_by_code(surreal, &rec.code)
            .await?
            .is_some()
        {
            continue;
        }
        match jurisdictions::create(
            surreal,
            &NewJurisdiction::new(rec.name, rec.code, rec.jurisdiction_type),
        )
        .await
        {
            Ok(_) => report.jurisdictions_inserted += 1,
            // A concurrent boot won the `jurisdiction_code` unique index
            // between the check and the insert; the row exists, which is
            // all the seed wants.
            Err(jurisdictions::JurisdictionError::CodeTaken) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct EntityTypeRec {
    name: String,
}

/// Seed the entity-type reference table — into SurrealDB, where the
/// table lives since its slice of #1093 (ENG-20). Canonical seed data:
/// it runs identically in every environment, production included, and
/// is idempotent on `name` — `find_or_create` absorbs a concurrent
/// boot's winning write.
async fn seed_entity_types(surreal: &SurrealDb, report: &mut SeedReport) -> anyhow::Result<()> {
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for rec in parse::<EntityTypeRec>(canonical::ENTITY_TYPE, "EntityType.yaml")? {
        if !seen.insert(rec.name.clone()) {
            continue;
        }
        if crate::entity_types::find_by_name(surreal, &rec.name)
            .await?
            .is_some()
        {
            continue;
        }
        crate::entity_types::find_or_create(surreal, &rec.name).await?;
        report.entity_types_inserted += 1;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct EntityRec {
    name: String,
    entity_type: EntityTypeRef,
}

#[derive(Debug, Deserialize)]
struct EntityTypeRef {
    name: String,
    #[serde(default)]
    jurisdiction: Option<JurisdictionRef>,
}

#[derive(Debug, Deserialize)]
struct JurisdictionRef {
    name: String,
}

async fn seed_entities(
    surreal: &SurrealDb,
    yaml: &str,
    path: &str,
    report: &mut SeedReport,
) -> anyhow::Result<()> {
    // Rows the seed already owns whose `firm_anchor_key` disagrees with
    // the name [`FIRM_ENTITY_NAME`] now carries. Setting is deferred to a
    // second pass so every stale key is surrendered before the new one is
    // claimed: the `entity_firm_anchor` index is UNIQUE, and a rename
    // between two names the seed both ships would otherwise be refused by
    // the row it is moving away from.
    let mut claim_anchor: Vec<(Uuid, String)> = Vec::new();
    for rec in parse::<EntityRec>(yaml, path)? {
        let et = crate::entity_types::find_by_name(surreal, &rec.entity_type.name)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Entity.yaml references unknown entity_type {name:?}",
                    name = rec.entity_type.name
                )
            })?;
        let jurisdiction_name = rec
            .entity_type
            .jurisdiction
            .as_ref()
            .map_or("Nevada", |j| j.name.as_str());
        let jur = jurisdictions::find_by_name(surreal, jurisdiction_name)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!("Entity.yaml references unknown jurisdiction {jurisdiction_name:?}")
            })?;
        if let Some(row) = crate::entities::find_by_name_and_type(surreal, &rec.name, et.id).await?
        {
            // A persisted database can outlive the memory-backed local
            // engine, whose re-seeded jurisdictions carry fresh ids, and
            // the engine never validated the link. Repoint so the
            // reference resolves again instead of dangling.
            if row.jurisdiction_id != jur.id {
                crate::entities::repoint_jurisdiction(surreal, row.id, jur.id).await?;
            }
            // Nothing above rewrites an existing row, so moving the
            // configured anchor would strand `firm_anchor_key` on the
            // outgoing firm and never mint it on the incoming one — and
            // that column, not the name, is what `delete_unless_firm_anchor`
            // reads. Reconcile the row to the name it should carry now.
            let expected = crate::entity_commands::firm_anchor_key(FIRM_ENTITY_NAME, &rec.name);
            if row.firm_anchor_key != expected {
                match expected {
                    Some(key) => claim_anchor.push((row.id, key)),
                    None => {
                        crate::entities::set_firm_anchor_key(surreal, row.id, None).await?;
                    }
                }
            }
            continue;
        }
        match crate::entities::create(surreal, &new_entity(&rec.name, et.id, jur.id)).await {
            Ok(_) => report.entities_inserted += 1,
            // The firm's own row is the one fixture two seeds can race for,
            // because it is the only one that takes `firm_anchor_key` — and
            // the UNIQUE index is what makes the loser lose. The cucumber
            // suites run scenarios concurrently against one shared engine and
            // each re-seeds this fixture, so losing that race has to be
            // "someone else already did it", not a failed seed.
            Err(crate::entities::EntityError::FirmAnchorTaken) => {}
            Err(error) => return Err(error.into()),
        }
    }
    for (id, key) in claim_anchor {
        // Losing the key to a concurrent seed is the same outcome the create
        // arm absorbs: another pass already moved the anchor onto this
        // identity, so this one has nothing left to do.
        match crate::entities::set_firm_anchor_key(surreal, id, Some(key)).await {
            Ok(_) | Err(crate::entities::EntityError::FirmAnchorTaken) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct PersonRec {
    email: String,
    name: String,
    #[serde(default)]
    profile_image_url: Option<String>,
}

async fn seed_persons(surreal: &SurrealDb, report: &mut SeedReport) -> anyhow::Result<()> {
    for rec in parse::<PersonRec>(canonical::PERSON, "Person.yaml")? {
        let email = rec.email.clone();
        let before = crate::persons::find_by_email_ci(surreal, &email).await?;
        crate::persons::find_or_create(
            surreal,
            &crate::persons::NewPerson {
                profile_image_url: rec.profile_image_url,
                ..crate::persons::NewPerson::new(rec.name, rec.email)
            },
        )
        .await?;
        if before.is_none() {
            report.persons_inserted += 1;
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct TestimonialRec {
    project: ProjectCodenameRef,
    person: PersonEmailRef,
    #[serde(default)]
    quote: String,
    #[serde(default)]
    attribution_label: Option<String>,
    #[serde(default)]
    consented_at: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    display_order: i32,
}

async fn seed_testimonials(surreal: &SurrealDb, report: &mut SeedReport) -> anyhow::Result<()> {
    for rec in parse::<TestimonialRec>(canonical::TESTIMONIAL, "Testimonial.yaml")? {
        let Some(project) = crate::projects::find_by_name(surreal, &rec.project.codename).await?
        else {
            continue;
        };
        let Some(person) = crate::persons::find_by_email_ci(surreal, &rec.person.email).await?
        else {
            continue;
        };
        crate::testimonials::find_or_create(
            surreal,
            &crate::testimonials::NewTestimonial {
                project_id: project.id,
                person_id: person.id,
                quote: &rec.quote,
                attribution_label: rec.attribution_label,
                consented_at: rec.consented_at,
                published_at: rec.published_at,
                display_order: rec.display_order,
            },
        )
        .await?;
        report.testimonials_inserted += 1;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct UserRec {
    person: PersonEmailRef,
    role: String,
}

#[derive(Debug, Deserialize)]
struct PersonEmailRef {
    email: String,
}

/// Firm-domain convention for seeded organization roles: any `owner`, `admin`,
/// or `clerk` row must use a **lowercase** `@neonlaw.com` email. `lawyer` is a
/// licensed-lawyer tier, not an employment or email-domain assertion, so an
/// outside lawyer's Lawyer seed may use their own domain. The
/// lowercase requirement is exact-match: the seed is the canonical
/// source of truth, so it stores one spelling rather than relying on
/// readers to normalize. Lookups themselves are case-insensitive
/// (`store::persons::find_by_email_ci`, backed by the
/// `persons_email_lower_key` unique index), so mixed-case input
/// resolves correctly — this rule keeps the seed data itself tidy.
/// See `docs/access-model.md`.
fn require_firm_domain(email: &str, role: crate::persons::Role) -> anyhow::Result<()> {
    use crate::persons::Role;
    if !matches!(role, Role::Owner | Role::Admin | Role::Clerk) {
        return Ok(());
    }
    if email != email.to_ascii_lowercase() {
        anyhow::bail!(
            "User.yaml: {role:?} seed for {email:?} must be lowercase \
             (see docs/access-model.md)",
        );
    }
    if !email.ends_with("@neonlaw.com") {
        anyhow::bail!(
            "User.yaml: {role:?} seed for {email:?} violates the firm-domain \
             convention — owner/admin/clerk records must use an @neonlaw.com email \
             (see docs/access-model.md)",
        );
    }
    Ok(())
}

/// User.yaml carries a `role` per person; the `users` table doesn't
/// exist as its own entity here — the system-wide tier lives on
/// `persons.role`. Resolve each user record by email, parse the role
/// token, and update the row if the requested tier is higher than
/// what's already stored. The ladder is owner > admin > lawyer > clerk > client.
async fn seed_user_roles(surreal: &SurrealDb, report: &mut SeedReport) -> anyhow::Result<()> {
    use crate::persons::Role;

    fn parse_role_token(s: &str) -> Role {
        match s {
            "owner" => Role::Owner,
            "admin" => Role::Admin,
            "lawyer" => Role::Lawyer,
            "clerk" => Role::Clerk,
            _ => Role::Client,
        }
    }
    for rec in parse::<UserRec>(canonical::USER, "User.yaml")? {
        let requested = parse_role_token(&rec.role);
        require_firm_domain(&rec.person.email, requested)?;
        let Some(p) = crate::persons::find_by_email_ci(surreal, &rec.person.email).await? else {
            continue;
        };
        if p.role.authority_rank() >= requested.authority_rank() {
            continue;
        }
        crate::persons::set_role(surreal, p.id, requested).await?;
        report.persons_updated += 1;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct GitRepoRec {
    repository_name: String,
}

/// Seed the tracked-repository provenance rows — into SurrealDB, where
/// the table lives since its slice of #1093 (ENG-20). Idempotent on
/// `remote_hash`; `find_or_create` absorbs a concurrent boot's winning
/// write.
async fn seed_git_repositories(surreal: &SurrealDb, report: &mut SeedReport) -> anyhow::Result<()> {
    for rec in parse::<GitRepoRec>(canonical::GIT_REPOSITORY, "GitRepository.yaml")? {
        let remote_hash = remote_hash(&rec.repository_name);
        if crate::git_repositories::find_by_remote_hash(surreal, &remote_hash)
            .await?
            .is_some()
        {
            continue;
        }
        crate::git_repositories::find_or_create(surreal, &remote_hash, &"0".repeat(40)).await?;
        report.git_repositories_inserted += 1;
    }
    Ok(())
}

fn remote_hash(name: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(name.as_bytes());
    h.finalize().iter().fold(String::new(), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    })
}

#[derive(Debug, Deserialize)]
struct QuestionRec {
    code: String,
    prompt: String,
    #[serde(default)]
    question_type: Option<String>,
    /// `lawyer` | `client` | `both` — which side of the intake sees this
    /// question. Defaults `both` when the YAML omits it.
    #[serde(default)]
    audience: Option<String>,
    // `help_text` / `choices` exist in the YAML but the schema has
    // no column for them — silently dropped.
}

async fn seed_questions(surreal: &SurrealDb, report: &mut SeedReport) -> anyhow::Result<()> {
    for rec in parse::<QuestionRec>(canonical::QUESTION, "Question.yaml")? {
        // `find_or_create` rather than read-then-insert: the cucumber
        // suites run scenarios concurrently against one shared engine, so a
        // seeder that assumed exclusivity would lose the race and surface
        // `CodeTaken`. It also keeps the second dev boot a no-op.
        let existed = crate::questions::find_by_code(surreal, &rec.code)
            .await?
            .is_some();
        crate::questions::find_or_create(
            surreal,
            &crate::questions::NewQuestion::new(
                rec.code,
                rec.prompt,
                rec.question_type.unwrap_or_else(|| "string".into()),
            )
            .with_audience(
                rec.audience
                    .unwrap_or_else(|| crate::questions::AUDIENCE_BOTH.to_string()),
            ),
        )
        .await?;
        if !existed {
            report.questions_inserted += 1;
        }
    }
    Ok(())
}

/// A question's canonical definition narrowed to its `code` and the
/// optional `choices:` block — the slice of `Question.yaml` the
/// [`question_choices`] reader needs. Every other field (prompt,
/// help_text, audience, …) is ignored.
#[derive(Debug, Deserialize)]
struct ChoiceQuestionRec {
    code: String,
    #[serde(default)]
    choices: Option<serde_yaml::Mapping>,
}

/// The attorney-reviewed answer choices for a `radio` question, as
/// ordered `(value, label)` pairs read from the canonical
/// `Question.yaml`. Returns an empty vec for a question with no
/// `choices:` block (every non-`radio` question) or an unknown code.
///
/// Choices live in the question's canonical seed definition but have no
/// column on the `questions` table — they are presentational, dropped at
/// seed time (see [`QuestionRec`]). The one surface that needs them at
/// runtime, the CLI questionnaire walker's machine-readable step
/// (`GET …/step?format=json`), reads them here rather than from the row,
/// so the choices a terminal shows are the same bytes the seed defines.
#[must_use]
pub fn question_choices(code: &str) -> Vec<(String, String)> {
    let code = code.split_once("__").map_or(code, |(prefix, _)| prefix);
    let Ok(parsed) = serde_yaml::from_str::<Records<ChoiceQuestionRec>>(canonical::QUESTION) else {
        return Vec::new();
    };
    parsed
        .records
        .into_iter()
        .find(|r| r.code == code)
        .and_then(|r| r.choices)
        .map(|m| {
            m.into_iter()
                .filter_map(|(k, v)| Some((k.as_str()?.to_string(), v.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Debug, Deserialize)]
struct MailroomRec {
    name: String,
}

/// `mailrooms.address_id` is NOT NULL; the YAML carries no separate
/// address for the mailroom itself. We synthesize a placeholder
/// address per mailroom so the column is satisfied — flagged with a
/// `(via mailroom)` line1 so it's obvious in row dumps. That placeholder
/// is the row the `address` schema's missing XOR assert exists for: it
/// names neither a person nor an entity.
async fn seed_mailrooms(
    surreal: &SurrealDb,
    yaml: &str,
    path: &str,
    report: &mut SeedReport,
) -> anyhow::Result<()> {
    for rec in parse::<MailroomRec>(yaml, path)? {
        if crate::mailrooms::find_by_name(surreal, &rec.name)
            .await?
            .is_some()
        {
            continue;
        }
        let addr = crate::addresses::create(
            surreal,
            &crate::addresses::NewAddress {
                line1: format!("(via mailroom: {})", rec.name),
                ..crate::addresses::NewAddress::default()
            },
        )
        .await?;
        // Find-or-create, not create: the cucumber suites seed
        // concurrently against one shared engine, so the read above can
        // miss a mailroom another scenario is creating right now.
        let created = crate::mailrooms::find_or_create(surreal, &rec.name, addr.id).await?;
        if created.address_id == addr.id {
            report.mailrooms_inserted += 1;
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct AddressRec {
    entity: EntityNameRef,
    street: String,
    city: String,
    state: String,
    country: String,
    zip: String,
}

#[derive(Debug, Deserialize)]
struct EntityNameRef {
    name: String,
}

/// Both `person_id` and `entity_id` are real `record<>` links since the
/// entities cluster ported (ENG-120), so this step is single-engine.
async fn seed_addresses(
    surreal: &SurrealDb,
    brand: BrandSeed,
    report: &mut SeedReport,
) -> anyhow::Result<()> {
    let Some((yaml, path)) = brand.addresses() else {
        return Ok(());
    };
    for rec in parse::<AddressRec>(yaml, path)? {
        let Some(ent) = crate::entities::find_by_name(surreal, &rec.entity.name).await? else {
            continue;
        };
        let (_, created) = crate::addresses::find_or_create_for_entity(
            surreal,
            &crate::addresses::NewAddress {
                entity_id: Some(ent.id),
                line1: rec.street,
                city: rec.city,
                region: rec.state,
                postal_code: rec.zip,
                country: rec.country,
                ..crate::addresses::NewAddress::default()
            },
        )
        .await?;
        if created {
            report.addresses_inserted += 1;
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct LetterRec {
    subject: String,
    sender: String,
    mailroom: MailroomNameRef,
}

#[derive(Debug, Deserialize)]
struct MailroomNameRef {
    name: String,
}

async fn seed_letters(surreal: &SurrealDb, report: &mut SeedReport) -> anyhow::Result<()> {
    for rec in parse::<LetterRec>(canonical::LETTER, "Letter.yaml")? {
        let Some(mr) = crate::mailrooms::find_by_name(surreal, &rec.mailroom.name).await? else {
            continue;
        };
        if crate::letters::find_by_mailroom_sender_summary(
            surreal,
            mr.id,
            &rec.sender,
            &rec.subject,
        )
        .await?
        .is_some()
        {
            continue;
        }
        crate::letters::record(
            surreal,
            &crate::letters::NewLetter {
                mailroom_id: mr.id,
                direction: crate::letters::DIRECTION_INCOMING.to_string(),
                sender: rec.sender,
                recipient: rec.mailroom.name.clone(),
                summary: rec.subject,
            },
        )
        .await?;
        report.letters_inserted += 1;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct AnswerRec {
    question_code: String,
    person_email: String,
    value: String,
}

async fn seed_answers(surreal: &SurrealDb, report: &mut SeedReport) -> anyhow::Result<()> {
    for rec in parse::<AnswerRec>(canonical::ANSWER, "Answer.yaml")? {
        let Some(q) = crate::questions::find_by_code(surreal, &rec.question_code).await? else {
            continue;
        };
        let Some(p) = crate::persons::find_by_email_ci(surreal, &rec.person_email).await? else {
            continue;
        };
        let value = crate::answers::primitive(&rec.value);
        // Idempotent on the lookup fields (question, person, value) so a
        // second dev boot inserts zero duplicates. These fixtures are
        // person-scoped and carry no Notation, so there is no state to key
        // on — the value itself is the natural key.
        if crate::answers::exists_with_value(surreal, q.id, p.id, &value).await? {
            continue;
        }
        crate::answers::record(surreal, &crate::answers::NewAnswer::new(q.id, p.id, value)).await?;
        report.answers_inserted += 1;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct PersonEntityRoleRec {
    person: PersonEmailRef,
    entity: EntityNameRef,
    role: String,
}

async fn seed_person_entity_roles(
    surreal: &SurrealDb,
    report: &mut SeedReport,
) -> anyhow::Result<()> {
    for rec in parse::<PersonEntityRoleRec>(canonical::PERSON_ENTITY_ROLE, "PersonEntityRole.yaml")?
    {
        let Some(p) = crate::persons::find_by_email_ci(surreal, &rec.person.email).await? else {
            continue;
        };
        let Some(e) = crate::entities::find_by_name(surreal, &rec.entity.name).await? else {
            continue;
        };
        // `grant` is find-or-create behind the UNIQUE `entity_role_tie`
        // index, so re-seeding a live database adds nothing and two
        // concurrent seeds settle on one edge rather than racing.
        let existing = crate::entity_roles::find(surreal, p.id, e.id, &rec.role).await?;
        crate::entity_roles::grant(surreal, p.id, e.id, &rec.role).await?;
        if existing.is_none() {
            report.person_entity_roles_inserted += 1;
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct PersonProjectRoleRec {
    person: PersonEmailRef,
    project: ProjectCodenameRef,
    role: String,
}

#[derive(Debug, Deserialize)]
struct ProjectCodenameRef {
    codename: String,
}

async fn seed_person_project_roles(
    surreal: &SurrealDb,
    report: &mut SeedReport,
) -> anyhow::Result<()> {
    for rec in
        parse::<PersonProjectRoleRec>(canonical::PERSON_PROJECT_ROLE, "PersonProjectRole.yaml")?
    {
        let Some(p) = crate::persons::find_by_email_ci(surreal, &rec.person.email).await? else {
            continue;
        };
        let Some(pr) = crate::projects::find_by_name(surreal, &rec.project.codename).await? else {
            continue;
        };
        if crate::projects::participation_for_person(surreal, p.id, pr.id)
            .await?
            .is_some()
        {
            continue;
        }
        crate::projects::add_participation(surreal, pr.id, p.id, &rec.role).await?;
        report.person_project_roles_inserted += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        normalized_body_bytes, reconcile_yaml, seed_canonical, seeded_template_codes,
        split_template, SeedModel, TemplateFrontmatter, SEEDED_TEMPLATES,
    };
    use crate::jurisdictions;
    use crate::persons::{self, Role};
    use crate::question_registry::QuestionType;
    use crate::test_support::mem_surreal;

    /// A filesystem-backed storage at a fixed path so the bytes a seed
    /// writes are readable by a later `templates::body` call in the same
    /// test — blobs are content-addressed, so sharing the dir across
    /// tests is safe (identical bytes dedup).
    async fn fs_storage() -> std::sync::Arc<dyn cloud::StorageService> {
        std::sync::Arc::new(
            cloud::FsStorage::new(std::env::temp_dir().join("navigator-seed-test-storage"))
                .await
                .expect("temp FsStorage"),
        )
    }

    #[tokio::test]
    async fn operator_person_seed_creates_then_overwrites_only_when_requested() {
        let surreal = mem_surreal().await;
        let initial = r"
lookup_fields:
  - email
records:
  - email: operator@example.com
    name: First Name
";
        let changed = r"
lookup_fields:
  - email
records:
  - email: operator@example.com
    name: Updated Name
";

        let created = reconcile_yaml(&surreal, SeedModel::Person, initial, "Firm", false)
            .await
            .expect("create from seed");
        assert_eq!(
            (created.created, created.updated, created.unchanged),
            (1, 0, 0)
        );

        let unchanged = reconcile_yaml(&surreal, SeedModel::Person, changed, "Firm", false)
            .await
            .expect("default leaves match alone");
        assert_eq!(
            (unchanged.created, unchanged.updated, unchanged.unchanged),
            (0, 0, 1)
        );
        assert_eq!(
            persons::find_by_email_ci(&surreal, "operator@example.com")
                .await
                .unwrap()
                .unwrap()
                .name,
            "First Name"
        );

        let overwritten = reconcile_yaml(&surreal, SeedModel::Person, changed, "Firm", true)
            .await
            .expect("overwrite matching record");
        assert_eq!(
            (
                overwritten.created,
                overwritten.updated,
                overwritten.unchanged
            ),
            (0, 1, 0)
        );
        assert_eq!(
            persons::find_by_email_ci(&surreal, "operator@example.com")
                .await
                .unwrap()
                .unwrap()
                .name,
            "Updated Name"
        );
    }

    /// The firm's own Entity is the one seeded row that takes
    /// `firm_anchor_key`, so it is the one row the `entity_firm_anchor`
    /// index can refuse (ENG-120). The seed re-runs over a live database
    /// on every boot, so it has to survive losing that write.
    ///
    /// Shook Law PLLC holds its own private mailbox at the Ridgeview Mail
    /// Center, and within that mail centre the box number is the whole address
    /// — `405-9002`, `405-9005`, and `405-9011` are the same street, suite, and
    /// ZIP, so a wrong suffix delivers the firm's mail to another entity of
    /// ours rather than bouncing.
    ///
    /// The two halves land in different layers on purpose, and this asserts
    /// both: the Entity is canonical, so every deployment carries it, while
    /// the mailbox is the Firm's own and rides the brand layer.
    #[tokio::test]
    async fn shook_law_holds_mailbox_9002_at_ridgeview() {
        let surreal = mem_surreal().await;
        let storage = fs_storage().await;

        seed_canonical(&surreal, &storage).await.expect("seed");
        let firm = crate::entities::find_by_name(&surreal, super::FIRM_ENTITY_NAME)
            .await
            .unwrap()
            .expect("the firm anchor is canonical, so every deployment carries it");

        // Canonical alone carries no address: the mailbox is the Firm's.
        assert!(
            crate::addresses::for_entity(&surreal, firm.id)
                .await
                .unwrap()
                .is_empty(),
            "the Firm's own addresses must stay out of the canonical layer"
        );

        super::seed_brand(&surreal, super::BrandSeed::Neon)
            .await
            .expect("brand seed");
        let at_ridgeview: Vec<String> = crate::addresses::for_entity(&surreal, firm.id)
            .await
            .unwrap()
            .into_iter()
            .map(|a| a.line1)
            .filter(|line| line.starts_with("5150 Mae Anne Ave"))
            .collect();
        assert_eq!(
            at_ridgeview,
            vec!["5150 Mae Anne Ave Ste 405-9002".to_string()],
            "the firm holds exactly one box at the mail centre, and 9002 is that box"
        );

        // The box is unique across the mail center: no other seeded entity
        // may answer to it, or mail routes to whichever row is read first.
        let holders = crate::addresses::list_all(&surreal)
            .await
            .unwrap()
            .into_iter()
            .filter(|a| a.line1.ends_with("405-9002"))
            .count();
        assert_eq!(holders, 1, "one box, one holder");
    }

    /// The California law corporation is the Firm's, so it rides the brand
    /// layer rather than the shared registry — and it seeds under its
    /// own jurisdiction, not the Nevada every sibling row carries.
    ///
    /// The jurisdiction is worth pinning because the two lookups in
    /// `seed_entities` do not agree: the *entity type* resolves by name
    /// alone, so the seed finds `Professional Corporation` whatever
    /// jurisdiction it was declared under, while the *entity's* jurisdiction
    /// comes from the nested `jurisdiction.name`. Drop that nested key and
    /// the row still seeds cleanly — into Nevada, silently, because `Nevada`
    /// is the fallback. A law corporation in the wrong state is not a
    /// cosmetic error: its registration and its regulator both follow the
    /// jurisdiction.
    #[tokio::test]
    async fn the_california_law_corporation_seeds_under_california() {
        let surreal = mem_surreal().await;

        seed_canonical(&surreal, &fs_storage().await)
            .await
            .expect("seed");
        assert!(
            crate::entities::find_by_name(&surreal, "Yakcobieus Industries PC")
                .await
                .unwrap()
                .is_none(),
            "the Firm's own corporation must not reach the shared registry"
        );

        super::seed_brand(&surreal, super::BrandSeed::Neon)
            .await
            .expect("brand seed");

        let pc = crate::entities::find_by_name(&surreal, "Yakcobieus Industries PC")
            .await
            .unwrap()
            .expect("the California law corporation seeds on a brand boot");
        let jurisdiction = jurisdictions::find_by_id(&surreal, pc.jurisdiction_id)
            .await
            .unwrap()
            .expect("its jurisdiction resolves");
        assert_eq!(jurisdiction.name, "California");

        let entity_type = crate::entity_types::find_by_name(&surreal, "Professional Corporation")
            .await
            .unwrap()
            .expect("the professional-corporation type seeds");
        assert_eq!(pc.entity_type_id, entity_type.id);
    }

    /// Losing it is not hypothetical: this seed finds an existing entity
    /// by `(name, entity_type_id)`, so an anchor row carrying a different
    /// entity type is invisible to the find and reaches the create — and
    /// the index refuses it. That has to read as "already seeded", not as
    /// a failed boot.
    #[tokio::test]
    async fn a_seed_that_loses_the_firm_anchor_write_still_succeeds() {
        let surreal = mem_surreal().await;

        // An anchor the seed's own find cannot see: right name, a type it
        // will not look under.
        let decoy_type = crate::entity_types::create(&surreal, "Decoy Type")
            .await
            .unwrap();
        crate::entities::create(
            &surreal,
            &crate::entities::NewEntity {
                name: super::FIRM_ENTITY_NAME.into(),
                entity_type_id: decoy_type.id,
                jurisdiction_id: uuid::Uuid::now_v7(),
                phone: None,
                url: None,
                firm_anchor_key: Some(super::FIRM_ENTITY_NAME.to_lowercase()),
            },
        )
        .await
        .unwrap();

        seed_canonical(&surreal, &fs_storage().await)
            .await
            .expect("a seed that loses the anchor write must still succeed");

        let anchors = crate::entities::all(&surreal)
            .await
            .unwrap()
            .into_iter()
            .filter(|row| row.name == super::FIRM_ENTITY_NAME)
            .count();
        assert_eq!(anchors, 1, "the firm anchor must stay a single row");
    }

    /// The firm anchor has moved twice — to `Neon Law` and back to
    /// `Shook Law PLLC` when the practice consolidated under the Neon Law mark
    /// — and every deployment that booted under a previous name carries that
    /// row with the key still on it. The seed skips rows that already exist, so
    /// without reconciliation the outgoing firm would stay undeletable and the
    /// incoming one would be deletable: `delete_unless_firm_anchor` reads
    /// `firm_anchor_key`, not the name.
    ///
    /// The retired partnership is gone from `Entity.yaml`, so the wrong holder
    /// here is any other seeded row. That is the more general statement of the
    /// same rule, and it is what a real database looks like — whatever row last
    /// held the key still holds it until a reseed takes it away.
    #[tokio::test]
    async fn a_reseed_moves_the_anchor_key_off_the_previous_firm() {
        let surreal = mem_surreal().await;
        seed_canonical(&surreal, &fs_storage().await)
            .await
            .expect("seed");

        // Rewind to the pre-rename shape: the key sits on a row the seed no
        // longer anchors on, and the anchor carries none.
        let anchor = crate::entities::find_by_name(&surreal, super::FIRM_ENTITY_NAME)
            .await
            .unwrap()
            .expect("the anchor seeds");
        let previous = crate::entities::find_by_name(&surreal, "shook.family")
            .await
            .unwrap()
            .expect("shook.family is an ordinary seeded Entity");
        crate::entities::set_firm_anchor_key(&surreal, anchor.id, None)
            .await
            .unwrap();
        crate::entities::set_firm_anchor_key(
            &surreal,
            previous.id,
            Some("shook.family".to_string()),
        )
        .await
        .unwrap();

        seed_canonical(&surreal, &fs_storage().await)
            .await
            .expect("a reseed reconciles the anchor");

        let anchor = crate::entities::find_by_id(&surreal, anchor.id)
            .await
            .unwrap()
            .expect("the anchor row survives");
        assert!(
            anchor.is_firm_anchor(),
            "{} must hold the key the delete guard reads",
            super::FIRM_ENTITY_NAME
        );
        let previous = crate::entities::find_by_id(&surreal, previous.id)
            .await
            .unwrap()
            .expect("the previous firm survives as an ordinary row");
        assert!(
            !previous.is_firm_anchor(),
            "Shook Law PLLC must surrender the key and become deletable"
        );
        assert_eq!(
            crate::entities::all(&surreal)
                .await
                .unwrap()
                .into_iter()
                .filter(crate::entities::Entity::is_firm_anchor)
                .count(),
            1,
            "exactly one row may be protected"
        );
    }

    #[tokio::test]
    async fn seeds_full_question_set() {
        let surreal = mem_surreal().await;
        let report = seed_canonical(&surreal, &fs_storage().await)
            .await
            .expect("seed");
        // The canonical question catalog is the closed type registry.
        // Template-specific prompt keys live after the `__` discriminator
        // in state names rather than as seeded question rows.
        let expected = QuestionType::all_tokens().len();
        let qs = crate::questions::list_all(&surreal).await.unwrap();
        assert_eq!(qs.len(), expected);
        assert!(qs.iter().any(|q| q.code == "person"));
        assert!(qs.iter().any(|q| q.code == "people"));
        assert!(qs.iter().any(|q| q.code == "custom_text"));
        assert!(qs.iter().any(|q| q.code == "custom_single_choice"));
        assert!(qs.iter().any(|q| q.code == "custom_datetime"));
        assert_eq!(report.questions_inserted, expected);
    }

    #[tokio::test]
    async fn seeds_full_jurisdiction_set() {
        let surreal = mem_surreal().await;
        seed_canonical(&surreal, &fs_storage().await)
            .await
            .expect("seed");
        let js = jurisdictions::list_all(&surreal).await.unwrap();
        // 50 states + DC + the ISO 3166-1 country set (alpha-3 codes,
        // with United States and Germany on their pre-ISO codes).
        assert_eq!(js.len(), 248);
        let codes: Vec<&str> = js.iter().map(|j| j.code.as_str()).collect();
        for code in [
            "NV", "CA", "NY", "TX", "WY", "DC", "US", "GMBH", "MEX", "CAN", "GBR",
        ] {
            assert!(codes.contains(&code), "expected `{code}` in jurisdictions");
        }
        // `jurisdiction_type` is reconciled with the seed: states are
        // `state`, sovereigns are `country` — the boundary the `country`
        // question type's option filter rides on.
        let by_code = |c: &str| js.iter().find(|j| j.code == c).unwrap();
        assert_eq!(by_code("NV").jurisdiction_type, "state");
        assert_eq!(by_code("US").jurisdiction_type, "country");
        assert_eq!(by_code("GMBH").jurisdiction_type, "country");
        assert_eq!(by_code("MEX").jurisdiction_type, "country");
        // The state Georgia and the country Georgia stay distinct by
        // name, so a name-keyed answer can never be ambiguous.
        assert_eq!(by_code("GA").name, "Georgia");
        assert_eq!(by_code("GEO").name, "Georgia (country)");
    }

    #[test]
    fn seeded_template_codes_are_derived_from_the_bundled_catalog() {
        let codes = seeded_template_codes().expect("seeded template codes");
        assert_eq!(codes.len(), SEEDED_TEMPLATES.len());
        assert!(codes.iter().any(|code| code == "onboarding__retainer"));
        assert!(codes.iter().any(|code| code == "northstar__will"));
        assert!(
            !codes
                .iter()
                .any(|code| code.starts_with("onboarding__retainer_")),
            "the service-specific retainers are retired; one generic retainer remains"
        );
    }

    /// A canonical LF template, and the byte-for-byte CRLF twin a
    /// Windows checkout materialises from it.
    ///
    /// The assertions below compare *parsed values* across the pair
    /// rather than counting errors. The failure mode this guards is
    /// silent absence — `split_template` returning `None`, or returning
    /// a frontmatter slice that happens to deserialise into different
    /// values — and neither shows up in an error count. A count-based
    /// test would have passed against the original LF-only parser on
    /// Linux and told us nothing about Windows.
    const LF_DOC: &str = "---\ncode: t__demo\ntitle: Demo\nrespondent_type: org\nkind: letter\n---\n# Body\n\nSecond paragraph.\n";

    fn crlf(s: &str) -> String {
        s.replace('\n', "\r\n")
    }

    fn parse_fm(md: &str) -> (TemplateFrontmatter, String) {
        let (fm_str, body) = split_template(md).expect("frontmatter present");
        let fm: TemplateFrontmatter = serde_yaml::from_str(fm_str).expect("frontmatter parses");
        (fm, body.to_string())
    }

    #[test]
    fn split_template_parses_the_same_values_from_lf_and_crlf() {
        let (lf, lf_body) = parse_fm(LF_DOC);
        let (crlf_fm, crlf_body) = parse_fm(&crlf(LF_DOC));

        // Every frontmatter value, not merely "it parsed".
        assert_eq!(lf.code, crlf_fm.code);
        assert_eq!(lf.title, crlf_fm.title);
        assert_eq!(lf.respondent_type, crlf_fm.respondent_type);
        assert_eq!(lf.form, crlf_fm.form);
        assert_eq!(lf.kind, crlf_fm.kind);
        assert_eq!(crlf_fm.code, "t__demo");
        assert_eq!(crlf_fm.kind.as_deref(), Some("letter"));

        // The body is delimited at the same logical point in both. It
        // still carries CRLF here; normalisation happens at ingest.
        assert_eq!(crlf_body, crlf(&lf_body));
        assert_eq!(lf_body, "# Body\n\nSecond paragraph.\n");
    }

    #[test]
    fn split_template_accepts_a_crlf_opener_and_closer() {
        let (fm, body) =
            parse_fm("---\r\ncode: a\r\ntitle: A\r\nrespondent_type: org\r\n---\r\nbody\r\n");
        assert_eq!(fm.code, "a");
        assert_eq!(fm.title, "A");
        assert_eq!(body, "body\r\n");
    }

    #[test]
    fn split_template_accepts_a_crlf_closer_at_eof_without_a_trailing_newline() {
        for md in [
            "---\r\ncode: a\r\ntitle: A\r\nrespondent_type: org\r\n---",
            "---\ncode: a\ntitle: A\nrespondent_type: org\n---",
        ] {
            let (fm, body) = parse_fm(md);
            assert_eq!(fm.code, "a", "{md:?}");
            assert_eq!(fm.respondent_type, "org", "{md:?}");
            assert_eq!(body, "", "closer at EOF leaves no body: {md:?}");
        }
    }

    #[test]
    fn split_template_accepts_empty_frontmatter_in_either_ending() {
        for (md, want_body) in [
            ("---\n---\nbody\n", "body\n"),
            ("---\r\n---\r\nbody\r\n", "body\r\n"),
            ("---\n---", ""),
            ("---\r\n---\r", ""),
        ] {
            let (fm_str, body) =
                split_template(md).expect("empty frontmatter is still frontmatter");
            assert_eq!(fm_str, "", "{md:?}");
            assert_eq!(body, want_body, "{md:?}");
        }
    }

    #[test]
    fn split_template_closes_a_mixed_ending_file_at_the_first_real_delimiter() {
        // CRLF frontmatter, but the closer was written with LF. The
        // earlier of the two matches must win, so the `---` inside the
        // body is not mistaken for the closer.
        let (fm, body) = parse_fm(
            "---\r\ncode: a\r\ntitle: A\r\nrespondent_type: org\n---\nbody\n---\ntrailing\n",
        );
        assert_eq!(fm.code, "a");
        assert_eq!(body, "body\n---\ntrailing\n");
    }

    #[test]
    fn split_template_still_reports_absent_frontmatter() {
        // The widened parser must not start accepting documents that
        // genuinely have no frontmatter — that would trade a false
        // negative for a false positive.
        for md in [
            "# no frontmatter\n",
            "",
            "--\ncode: a\n--\n",
            "---\ncode: a\n",
        ] {
            assert!(split_template(md).is_none(), "{md:?} has no frontmatter");
        }
    }

    #[test]
    fn every_bundled_template_splits_on_this_checkout() {
        // The regression that motivated the fix: on a CRLF checkout this
        // failed for all of them, and the first caller turned it into a
        // fatal boot error. Asserts against the real bundled catalog, so
        // it fails on whichever platform is actually broken.
        for template in SEEDED_TEMPLATES {
            let (fm_str, _) = split_template(template.markdown)
                .unwrap_or_else(|| panic!("{}: frontmatter not found", template.label));
            let fm: TemplateFrontmatter = serde_yaml::from_str(fm_str)
                .unwrap_or_else(|e| panic!("{}: frontmatter parse: {e}", template.label));
            assert!(!fm.code.is_empty(), "{}: empty code", template.label);
        }
    }

    #[test]
    fn template_body_bytes_are_platform_independent() {
        // The asset-sha question from ENG-265. `ingest_content` is
        // content-addressed, so these bytes decide the `asset_id`; if an
        // LF and a CRLF checkout produced different bytes, the same
        // logical template would fork into two immutable versions
        // depending on which platform compiled the binary.
        let crlf_doc = crlf(LF_DOC);
        let lf_bytes = normalized_body_bytes(split_template(LF_DOC).expect("lf").1);
        let crlf_bytes = normalized_body_bytes(split_template(&crlf_doc).expect("crlf").1);
        assert_eq!(lf_bytes, crlf_bytes);
        assert!(
            !crlf_bytes.contains(&b'\r'),
            "normalised body must carry no carriage returns"
        );
    }

    #[tokio::test]
    async fn seeds_the_bundled_template_catalog() {
        let surreal = mem_surreal().await;
        let report = seed_canonical(&surreal, &fs_storage().await)
            .await
            .expect("seed");
        assert_eq!(
            report.templates_inserted,
            SEEDED_TEMPLATES.len(),
            "expected the full bundled template catalog to be inserted on first pass"
        );
        // Spot-check templates from across the catalog so a dropped
        // `include_str!` entry is caught, not just the retainer.
        for code in [
            "onboarding__retainer",
            "closing__letter",
            "trusts__nevada",
            "will__simple",
            "ca__llc_operating_agreement",
            "us__form_990",
            "services__contract_review",
            "employment__nonprofit_w2",
            "contractor__nonprofit_1099",
        ] {
            assert!(
                crate::templates::resolve(&surreal, None, code)
                    .await
                    .unwrap()
                    .is_some(),
                "expected bundled template `{code}` to be seeded"
            );
        }
        let tmpl = crate::templates::resolve(&surreal, None, "onboarding__retainer")
            .await
            .unwrap()
            .expect("template row");
        assert_eq!(tmpl.title, "Retainer Agreement");
        assert_eq!(tmpl.respondent_type, "person_and_entity");
        assert!(tmpl.project_id.is_none(), "bundled templates are shared");
        // The body now lives in a blob — fetch it via the storage
        // accessor. Just the markdown body, no frontmatter, so the
        // renderer's dotted glossary interpolation finds
        // its targets.
        let body = crate::templates::body(&surreal, &fs_storage().await, &tmpl)
            .await
            .expect("template body in storage");
        assert!(
            !body.starts_with("---"),
            "body should not include the YAML frontmatter; got {:?}",
            &body[..body.len().min(20)]
        );
        assert!(body.contains("{{person__client.name}}"));
        assert!(body.contains("{{person__client.email}}"));
        assert!(body.contains("{{project__engagement.name}}"));
        assert!(body.contains("{{custom_clauses}}"));
    }

    #[tokio::test]
    async fn template_seeder_is_idempotent_on_second_pass() {
        let surreal = mem_surreal().await;
        let first = seed_canonical(&surreal, &fs_storage().await).await.unwrap();
        let second = seed_canonical(&surreal, &fs_storage().await).await.unwrap();
        assert_eq!(first.templates_inserted, SEEDED_TEMPLATES.len());
        assert_eq!(
            second.templates_inserted, 0,
            "second pass must skip every existing template"
        );
        let count = crate::templates::list_current(&surreal)
            .await
            .unwrap()
            .into_iter()
            .filter(|t| t.code == "onboarding__retainer")
            .count();
        assert_eq!(count, 1, "exactly one current retainer template row");
    }

    /// The firm's one engagement agreement carries the three load-bearing
    /// elements every matter needs: the JAMS arbitration clause (forum
    /// selection only, with the non-waivable fee-arbitration carve-out and
    /// the independent-counsel sentence — never a liability limitation), the
    /// `contact@neonlaw.com` reach-the-Firm clause, and the custom-clause
    /// slot the fee terms and any practice-area ethics reading arrive through.
    ///
    /// These moved here when the twelve service-specific retainers retired:
    /// they were the only bodies carrying them, so without this the firm
    /// would have shipped an engagement agreement with no arbitration clause
    /// and no fee-arbitration disclosure.
    #[tokio::test]
    async fn the_retainer_carries_arbitration_contact_and_the_clause_slot() {
        let surreal = mem_surreal().await;
        seed_canonical(&surreal, &fs_storage().await).await.unwrap();
        let storage = fs_storage().await;

        // Distinctive phrases from the three clauses. Checked against the
        // body with its line wrapping collapsed, so reflowing a paragraph to
        // satisfy the Markdown linter cannot silently drop a clause from
        // this guard.
        let required = [
            "binding arbitration administered by **JAMS**",
            "seated in **Reno, Nevada**",
            "limit, cap, or waive the Firm's responsibility for its own work",
            "right to consult independent counsel of your own choosing before you agree to it",
            "Mandatory Fee Arbitration Act",
            "Washington State Bar Association",
            "Write to contact@neonlaw.com",
            "{{custom_clauses}}",
            "{{client.signature}}",
            "{{firm.signature}}",
        ];

        let code = "onboarding__retainer";
        let tmpl = crate::templates::resolve(&surreal, None, code)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("{code} seeded"));
        let body = crate::templates::body(&surreal, &storage, &tmpl)
            .await
            .expect("retainer body");
        let flat = body.split_whitespace().collect::<Vec<_>>().join(" ");
        for phrase in required {
            assert!(
                flat.contains(phrase),
                "{code} must carry the clause phrase {phrase:?}"
            );
        }

        // It states the basis of the fee without stating an amount or a
        // cadence: the figure arrives as a custom clause (ENG-146).
        assert!(
            !flat.contains("billed monthly") && !flat.contains("rate sheet attached"),
            "the generic retainer asserts no cadence and no rate sheet"
        );

        // It is practice-neutral. The old body excluded litigation, which
        // would have made the firm's own litigation practice unopenable on
        // its only engagement agreement.
        assert!(
            !flat.contains("does not include litigation"),
            "the engagement agreement must not exclude the firm's own practice areas"
        );

        // The arbitration clause must not read as a liability waiver
        // (RPC 1.8(h)). Guard against a regression that re-introduces
        // limiting language.
        for forbidden in ["limit our liability", "waive any claim against the Firm"] {
            assert!(
                !flat.contains(forbidden),
                "{code} must not limit malpractice liability ({forbidden:?})"
            );
        }

        // Governing law is fillable per engagement (#364 pattern propagated
        // in #363): the clause names the questionnaire variable, not a
        // hardcoded jurisdiction. The token is bare, not a code span, so the
        // letter renderer fills and highlights it like every other
        // placeholder. The arbitration *seat* stays fixed at Reno (asserted
        // above) — venue does not flex with governing law.
        assert!(
            flat.contains("This Agreement is governed by the law of")
                && flat.contains("{{custom_single_choice__governing_law}}"),
            "{code} must fill governing law from the questionnaire, not hardcode it"
        );
        assert!(
            !flat.contains("decided under Nevada law"),
            "{code} must not hardcode 'decided under Nevada law'; use the fillable clause"
        );
    }

    #[tokio::test]
    async fn seed_is_idempotent() {
        let surreal = mem_surreal().await;
        let first = seed_canonical(&surreal, &fs_storage().await)
            .await
            .expect("seed 1");
        let second = seed_canonical(&surreal, &fs_storage().await)
            .await
            .expect("seed 2");
        assert_eq!(second.questions_inserted, 0);
        assert_eq!(second.jurisdictions_inserted, 0);
        assert_eq!(second.persons_inserted, 0);
        assert!(first.questions_inserted > 0);
    }

    #[tokio::test]
    async fn seed_is_idempotent_when_a_seeded_email_has_different_casing() {
        let surreal = mem_surreal().await;
        seed_canonical(&surreal, &fs_storage().await)
            .await
            .expect("initial seed");
        let nick = persons::find_by_email_ci(&surreal, "nick@neonlaw.com")
            .await
            .unwrap()
            .expect("nick exists");
        persons::edit(
            &surreal,
            nick.id,
            &crate::persons::PersonEdit {
                email: Some("Nick@NeonLaw.com".into()),
                ..crate::persons::PersonEdit::default()
            },
        )
        .await
        .expect("re-case Nick's email");

        let rerun = seed_canonical(&surreal, &fs_storage().await)
            .await
            .expect("re-seeding must resolve case-insensitive email references");
        assert_eq!(rerun.persons_inserted, 0);
        assert_eq!(
            persons::find_by_email_ci(&surreal, "nick@neonlaw.com")
                .await
                .unwrap()
                .expect("case-insensitive lookup preserves Nick")
                .role,
            Role::Admin
        );
    }

    #[tokio::test]
    async fn seeds_attorney_credentials_with_correct_numbers() {
        let surreal = mem_surreal().await;
        let report = seed_canonical(&surreal, &fs_storage().await)
            .await
            .expect("seed");
        let nick = persons::find_by_email_ci(&surreal, "nick@neonlaw.com")
            .await
            .unwrap()
            .expect("nick exists");
        let creds = crate::credentials::for_person(&surreal, nick.id)
            .await
            .unwrap();
        assert_eq!(creds.len(), 3, "expected NV + CA + WA admissions");
        // The state bar numbers are public-record disclosures; pin them
        // explicitly so a seed YAML edit can't silently change the
        // attorney advertising disclosure rendered on the firm site.
        let mut by_juris: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for c in &creds {
            let j = jurisdictions::find_by_id(&surreal, c.jurisdiction_id)
                .await
                .unwrap()
                .expect("the credential's jurisdiction is a seeded Surreal row");
            by_juris.insert(j.code, c.license_number.clone());
        }
        assert_eq!(by_juris.get("NV").map(String::as_str), Some("13400"));
        assert_eq!(by_juris.get("CA").map(String::as_str), Some("337252"));
        assert_eq!(by_juris.get("WA").map(String::as_str), Some("63446"));
        assert_eq!(report.credentials_inserted, 3);
    }

    #[tokio::test]
    async fn user_role_lifts_persons_to_admin() {
        let surreal = mem_surreal().await;
        seed_canonical(&surreal, &fs_storage().await)
            .await
            .expect("seed");
        let nick = persons::find_by_email_ci(&surreal, "nick@neonlaw.com")
            .await
            .unwrap()
            .expect("nick exists");
        assert_eq!(nick.role, Role::Admin);
    }

    #[test]
    fn firm_domain_convention_accepts_lowercase_neon_law_for_organization_roles() {
        use super::require_firm_domain;
        use crate::persons::Role;
        assert!(require_firm_domain("owner@neonlaw.com", Role::Owner).is_ok());
        assert!(require_firm_domain("nick@neonlaw.com", Role::Admin).is_ok());
        assert!(require_firm_domain("clerk@neonlaw.com", Role::Clerk).is_ok());
    }

    #[test]
    fn firm_domain_convention_rejects_mixed_case_privileged_emails() {
        use super::require_firm_domain;
        use crate::persons::Role;
        assert!(require_firm_domain("Owner@NeonLaw.com", Role::Owner).is_err());
        let err = require_firm_domain("Nick@NeonLaw.com", Role::Admin).unwrap_err();
        assert!(
            err.to_string().contains("lowercase"),
            "error should call out lowercase, got: {err}",
        );
        assert!(require_firm_domain("nick@NEONLAW.COM", Role::Admin).is_err());
    }

    #[test]
    fn firm_domain_convention_allows_any_domain_for_client() {
        use super::require_firm_domain;
        use crate::persons::Role;
        assert!(require_firm_domain("libra@example.com", Role::Client).is_ok());
        // Client rows aren't held to lowercase here; that's a normalization
        // concern for the persons table, not the seed convention.
        assert!(require_firm_domain("Libra@Example.com", Role::Client).is_ok());
    }

    #[test]
    fn firm_domain_convention_allows_an_external_lawyer() {
        use super::require_firm_domain;
        use crate::persons::Role;
        assert!(require_firm_domain("counsel@legalaid.example", Role::Lawyer).is_ok());
    }

    #[test]
    fn question_choices_is_empty_after_the_vocabulary_collapse() {
        use super::question_choices;
        // With the vocabulary collapsed to the registry, no seeded question
        // carries a `choices:` block — a one-off choice set (`fee_status`,
        // `management_structure`, …) lives in the template that asks it, as a
        // `custom_single_choice__<key>` state. So the seed reader is empty for
        // every code, and an unknown code still answers with an empty vec
        // rather than panicking.
        assert!(question_choices("custom_single_choice").is_empty());
        assert!(question_choices("custom_text").is_empty());
        assert!(question_choices("no_such_question_code").is_empty());
    }

    /// The seed vocabulary is exactly the closed registry — every question
    /// is a glossary ORM model (record/reference), its plural list form, or
    /// a `custom_*` primitive. No bespoke per-matter codes. This grounds
    /// `Question.yaml` to `store::question_registry::QuestionType` so the two
    /// can never drift (issue #235).
    #[test]
    fn question_yaml_is_exactly_the_registry() {
        use std::collections::BTreeSet;
        let codes: BTreeSet<String> =
            super::parse::<super::QuestionRec>(super::canonical::QUESTION, "Question.yaml")
                .unwrap()
                .into_iter()
                .map(|q| q.code)
                .collect();
        let registry: BTreeSet<String> = crate::question_registry::QuestionType::all_tokens()
            .into_iter()
            .map(str::to_string)
            .collect();
        assert_eq!(
            codes, registry,
            "Question.yaml codes must be exactly store::question_registry::QuestionType"
        );
    }

    /// Every localized prompt maps to a real question code — no orphaned
    /// translations after a rename.
    #[test]
    fn firm_domain_convention_rejects_off_domain_organization_role_seeds() {
        use super::require_firm_domain;
        use crate::persons::Role;
        let err = require_firm_domain("libra@example.com", Role::Clerk).unwrap_err();
        assert!(
            err.to_string().contains("@neonlaw.com"),
            "error should mention the firm domain, got: {err}",
        );
        assert!(require_firm_domain("nick@gmail.com", Role::Admin).is_err());
    }

    /// A `FsStorage` at a directory no other test shares, so a test can assert
    /// on the exact bytes under one portal key.
    ///
    /// Distinct from [`fs_storage`], which deliberately shares a fixed path
    /// because content-addressed template blobs dedup. Portal objects are
    /// keyed rather than content-addressed, so sharing a root here would let
    /// one test's publish satisfy another test's assertion.
    async fn applications_bucket() -> std::sync::Arc<dyn cloud::StorageService> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "navigator-seed-applications-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        std::sync::Arc::new(cloud::FsStorage::new(dir).await.expect("temp FsStorage"))
    }

    /// Every sample matter's repository lives in the staging organization, and
    /// its repository name **is** the Project code.
    ///
    /// Both halves are load-bearing, and neither is cosmetic.
    ///
    /// The organization is where these three repositories actually are — they
    /// moved out of `neon-law-source-code` so the staging deployment houses the
    /// fixtures it serves. A stale URL here does not fail loudly: GitHub
    /// redirects a transferred repository, so `dev sample-project` would keep
    /// cloning successfully from a path that no longer describes anything.
    ///
    /// The name-equals-code half is what the publish path depends on.
    /// `.github/actions/application-publish` derives the object prefix from the
    /// repository name and then asserts the built bundle is mounted at
    /// `/app/projects/<that>/portal/`. So the two names must agree, and nothing
    /// in either system derives one from the other — this test is the only
    /// thing holding them together. A rename on either side breaks here rather
    /// than at a publish against a real bucket.
    #[test]
    fn every_sample_matter_repository_is_the_staging_org_named_for_its_code() {
        for matter in super::SAMPLE_MATTERS {
            assert_eq!(
                matter.repository_url,
                format!("https://github.com/{SAMPLE_MATTER_ORG}/{}", matter.code),
                "sample matter `{}` must name `{SAMPLE_MATTER_ORG}/{}`",
                matter.code,
                matter.code
            );
        }
    }

    /// The organization the three sample project repositories live in.
    const SAMPLE_MATTER_ORG: &str = "neon-law-staging";

    /// The key one sample matter's portal document publishes under.
    fn entry_key(code: &str) -> String {
        format!(
            "{}/{}",
            crate::sample_project::portal_prefix(code),
            crate::sample_project::ENTRY_DOCUMENT
        )
    }

    /// A production-profile boot must not touch a published portal
    /// application. The persistent staging deployment is exactly this shape —
    /// the production runtime profile carrying simulated matters — and it
    /// stages nothing, so before ENG-278 every boot reverted `index.html` to
    /// the compiled-in placeholder while the hashed assets of the real bundle
    /// survived beside it, unreferenced.
    ///
    /// The staging lookup is injected as absent rather than read from the
    /// process, because a sourced `.devx/env` sets
    /// `NAVIGATOR_SAMPLE_PROJECTS_DIR` and would send this down the staged
    /// branch instead.
    #[tokio::test]
    async fn a_production_boot_leaves_a_published_portal_bundle_byte_for_byte() {
        let applications = applications_bucket().await;
        let matter = &super::SAMPLE_MATTERS[0];
        let key = entry_key(matter.code);

        // What an operator published: a real bundle's document, referencing
        // hashed assets that only it knows the names of.
        let published =
            br#"<!doctype html><html><head><script type="module" src="/sample-litigation/portal/assets/index-a1b2c3d4.js"></script></head><body><div id="root"></div></body></html>"#;
        applications
            .put_cached(
                &key,
                published,
                "text/html; charset=utf-8",
                crate::sample_project::ENTRY_CACHE_CONTROL,
            )
            .await
            .expect("the operator's publish");

        super::publish_sample_portal_with(
            &applications,
            matter,
            crate::DeploymentEnvironment::Production,
            |_| None,
        )
        .await
        .expect("a production boot must not fail for declining to publish");

        let after = applications.get(&key).await.expect("still published");
        assert_eq!(
            after.bytes, published,
            "a production-profile boot must leave the published portal application byte for byte"
        );
        assert!(
            !String::from_utf8_lossy(&after.bytes).contains(matter.portal_index),
            "the compiled-in placeholder must not have replaced the published document"
        );
    }

    /// The placeholder is a `dev` affordance, and it must keep landing: it is
    /// what leaves a developer's portal serving something while that matter's
    /// Vite build is broken. An empty bucket is the case a fresh
    /// `worktree-env up` presents.
    #[tokio::test]
    async fn a_dev_boot_publishes_the_placeholder_into_an_empty_bucket() {
        let applications = applications_bucket().await;
        let matter = &super::SAMPLE_MATTERS[0];
        let key = entry_key(matter.code);

        super::publish_sample_portal_with(
            &applications,
            matter,
            crate::DeploymentEnvironment::Dev,
            |_| None,
        )
        .await
        .expect("publish");

        let stored = applications
            .get(&key)
            .await
            .expect("the placeholder landed");
        assert_eq!(
            stored.bytes,
            matter.portal_index.as_bytes(),
            "a dev boot with nothing staged publishes the deterministic document"
        );
        assert_eq!(stored.content_type, "text/html; charset=utf-8");
    }

    /// The production guard sits *before* the staged-bundle branch, so a
    /// production-profile boot writes nothing even when a bundle is staged.
    ///
    /// This is the stronger half of the rule, and the one worth a test: the
    /// weaker version — skip only the placeholder — still lets a boot that
    /// happens to have `NAVIGATOR_SAMPLE_PROJECTS_DIR` set overwrite a live
    /// client portal with whatever a developer last built. Publishing to a real
    /// deployment is an operator act with its own lane; boot is never it.
    #[tokio::test]
    async fn a_staged_bundle_does_not_publish_under_the_production_profile() {
        let applications = applications_bucket().await;
        let matter = &super::SAMPLE_MATTERS[0];
        let staging = std::env::temp_dir().join(format!(
            "navigator-seed-staged-{}-{}",
            std::process::id(),
            matter.code
        ));
        let dist = staging.join(matter.code).join("dist");
        std::fs::create_dir_all(&dist).expect("dist");
        std::fs::write(
            staging.join(matter.code).join("navigator.yml"),
            format!("name: {}\n", matter.code),
        )
        .expect("manifest");
        std::fs::write(dist.join("index.html"), b"<!doctype html><p>built</p>")
            .expect("built document");
        let configured = staging.to_string_lossy().into_owned();

        super::publish_sample_portal_with(
            &applications,
            matter,
            crate::DeploymentEnvironment::Production,
            move |_| Some(configured.clone()),
        )
        .await
        .expect("publish");

        assert!(
            applications.get(&entry_key(matter.code)).await.is_err(),
            "a production-profile boot must publish nothing, staged bundle or not"
        );
        std::fs::remove_dir_all(&staging).ok();
    }

    /// A staged bundle whose manifest names another matter is refused on the
    /// production profile as well — and the refusal must not fall through into
    /// publishing the placeholder over whatever is there.
    #[tokio::test]
    async fn a_wrong_project_bundle_is_refused_without_overwriting_on_production() {
        let applications = applications_bucket().await;
        let matter = &super::SAMPLE_MATTERS[0];
        let key = entry_key(matter.code);
        let published = b"<!doctype html><p>the operator's bundle</p>";
        applications
            .put_cached(
                &key,
                published,
                "text/html; charset=utf-8",
                crate::sample_project::ENTRY_CACHE_CONTROL,
            )
            .await
            .expect("the operator's publish");

        let staging = std::env::temp_dir().join(format!(
            "navigator-seed-wrong-project-{}-{}",
            std::process::id(),
            matter.code
        ));
        let dist = staging.join(matter.code).join("dist");
        std::fs::create_dir_all(&dist).expect("dist");
        // The directory is named for the disputes matter; the manifest inside
        // it declares the estate matter.
        std::fs::write(
            staging.join(matter.code).join("navigator.yml"),
            format!("name: {}\n", super::SAMPLE_ESTATE_CODE),
        )
        .expect("manifest");
        std::fs::write(
            dist.join("index.html"),
            b"<!doctype html><p>another matter</p>",
        )
        .expect("built document");
        let configured = staging.to_string_lossy().into_owned();

        super::publish_sample_portal_with(
            &applications,
            matter,
            crate::DeploymentEnvironment::Production,
            move |_| Some(configured.clone()),
        )
        .await
        .expect("a wrong-Project bundle is reported and skipped, not fatal");

        let after = applications.get(&key).await.expect("still published");
        assert_eq!(
            after.bytes, published,
            "refusing a wrong-Project bundle must not overwrite the published document either"
        );
        assert!(
            applications
                .get(&format!(
                    "{}/index.html",
                    crate::sample_project::portal_prefix(super::SAMPLE_ESTATE_CODE)
                ))
                .await
                .is_err(),
            "one matter's bundle must never land on another matter's portal"
        );
        std::fs::remove_dir_all(&staging).ok();
    }
}
