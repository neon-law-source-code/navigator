//! A versioned, fixture-shaped synthetic portfolio that can be planned
//! (`dry_run`) or written (`apply`) against a deployment, on top of the
//! existing seed, invoice-mirror, trust, IOLTA, participation, document, and
//! portal seams.
//!
//! This is deliberately **not** the automatic dev-profile sample-matter layer
//! ([`crate::seed::seed_sample_portfolio`]): that layer applies itself on
//! every non-production boot, and persistent staging boots on the
//! `Production` profile so that layer never runs there. This module is the
//! explicit counterpart — an operator-invoked apply, decoupled from any boot
//! path, that a deployment carries forward once written rather than one a
//! boot re-asserts.
//!
//! # Why the target check is separate from the disclosure flag
//!
//! [`crate::config::sample_matters`] answers one question — does this
//! deployment *say* its matters are simulated — and it deliberately writes
//! nothing on its own (see that module's docs). Persistent staging runs the
//! `Production` [`crate::DeploymentEnvironment`] profile with that flag set
//! `true`, so neither the profile nor the flag alone can be this module's
//! write gate: the profile is identical to real production, and the flag is
//! disclosure, not authority.
//!
//! [`verify_target`] is the write gate instead. It requires an operator to
//! name [`STAGING_TARGET`] explicitly — nothing is inferred from the
//! environment — and it additionally requires the disclosure flag to already
//! read `true`, so applying still refuses a deployment that has not (yet, or
//! ever) disclosed simulated matters. Two independent, explicit signals must
//! agree before the first write; either one alone refuses.
//!
//! # Idempotency
//!
//! Every write here goes through a seam that is already idempotent on a
//! natural key: [`crate::persons::find_or_create`] on email,
//! [`crate::entities::create`]/lookup on `(name, entity_type)`,
//! [`crate::projects::find_or_create_by_code`] on `code`,
//! [`crate::projects::add_participation`] on `(person, project)`,
//! [`crate::assets::find_filed_copy`]/[`crate::documents::ingest_bytes_exactly_once`]
//! on `(project, filename, sha256)`, [`crate::xero_invoices::upsert`] and
//! [`crate::iolta_accounts::upsert`] on the provider id as the row's own
//! record id, [`crate::trust::record_project_movement`] on `external_ref`,
//! and [`crate::iolta_withdrawals::apply`] on the transaction id. A repeat
//! `apply` therefore inserts nothing new; this module adds no additional
//! locking or dedup of its own.
//!
//! # No live provider
//!
//! Every provider-shaped id below (`xero_invoice_id`, IOLTA `xero_account_id`,
//! `xero_transaction_id`) is a literal constant. Nothing here calls Xero, a
//! bank, Drive, GitHub, or any other live provider; every write lands in the
//! local store or the configured [`cloud::StorageService`] mirror/stub.

use std::sync::Arc;

use anyhow::anyhow;
use uuid::Uuid;

use crate::surreal::SurrealDb;

/// The version of the fixture this module writes. Bump when the manifest
/// below changes shape; a stale reader can then tell it apart from an
/// earlier apply.
///
/// `2` added the lifecycle/participation scenario set below: an open pitch,
/// a closed-active matter, a closed pitch carrying its required offboarding
/// artifact, an archived matter, and a matter closed then reopened — each
/// with its own synthetic client, closed through
/// [`crate::projects::transition_project_with_reason`] and the closure
/// vocabulary `store::projects::ClosureReason` defines. It also adds a
/// supervised Clerk (added once the reopened matter is active again) and an
/// Admin who participates in nothing, both sharing the fixture's one lawyer
/// and entity type.
pub const PORTFOLIO_VERSION: u32 = 2;

/// The one literal an operator may pass to apply the portfolio. Nothing
/// else — not a deployment name, not a value derived from
/// [`crate::DeploymentEnvironment`] — is accepted; see the module docs.
pub const STAGING_TARGET: &str = "staging";

const LAWYER_NAME: &str = "Portia Ledger";
const LAWYER_EMAIL: &str = "portia.ledger@synthetic-portfolio.example";
const CLIENT_NAME: &str = "Dana Fixture";
const CLIENT_EMAIL: &str = "dana.fixture@synthetic-portfolio.example";
const CLIENT_ENTITY_NAME: &str = "Fixture Ridgeline Holdings, Inc.";
const CLIENT_ENTITY_TYPE: &str = "C-Corp";
const JURISDICTION_NAME: &str = "Nevada";
const PROJECT_CODE: &str = "synthetic-portfolio-001";
const PROJECT_NAME: &str = "Fixture Ridgeline — Synthetic Portfolio Matter";
const PROJECT_DESCRIPTION: &str = "Versioned synthetic staging portfolio fixture matter. Every \
     party, document, and financial event on it is invented.";
const PROJECT_BRAND: &str = "neon";

/// The workspace-shared template every deployment carries from the
/// canonical seed (every boot of every profile runs
/// [`crate::seed::seed_canonical`]), reused here as the fixture's
/// engagement-letter notation.
const ONBOARDING_TEMPLATE_CODE: &str = "onboarding__letter";
const NOTATION_STATE: &str = "BEGIN";

const DOCUMENT_FILENAME: &str = "synthetic-portfolio-intake.md";
const DOCUMENT_CONTENT: &str = "# Synthetic Portfolio Intake\n\n\
     Invented intake note for the versioned synthetic staging portfolio \
     fixture. No real client, matter, or person is represented here.\n";

const XERO_INVOICE_ID: &str = "synthetic-portfolio-invoice";
const XERO_INVOICE_REFERENCE: &str = "Synthetic portfolio invoice";
const INVOICE_CENTS: i64 = 50_000;

const TRUST_DEPOSIT_EXTERNAL_REF: &str = "synthetic-portfolio-deposit";

const IOLTA_XERO_ACCOUNT_ID: &str = "synthetic-portfolio-iolta-account";
const IOLTA_ACCOUNT_NAME: &str = "IOLTA Trust — Nevada (Synthetic Portfolio)";
const IOLTA_XERO_TRANSACTION_ID: &str = "synthetic-portfolio-withdrawal";

const PORTAL_INDEX: &str = "<!doctype html>\n<title>Synthetic Portfolio</title>\n\
     <p>Fixture portal for the versioned synthetic staging portfolio matter. \
     Nothing on this page is a real client.</p>\n";

/// An Admin who participates in no matter here, demonstrating the ENG-81
/// participation-only rendering `docs/access-model.md#admin` describes: the
/// tier still resolves every code below (route-admission bypass), but
/// `store::access::matter_viewer` answers `None` for every one of them, same
/// as anyone else with no row.
const ADMIN_NAME: &str = "Simone Okafor";
const ADMIN_EMAIL: &str = "simone.okafor@synthetic-portfolio.example";

/// One matter of a versioned lifecycle/participation scenario: its own
/// synthetic client, whether it carries onboarding and/or offboarding
/// artifacts, whether and how it closes, and whether it is archived,
/// reopened, or handed a supervised Clerk once it reaches its final state.
///
/// Every scenario shares the fixture's one lawyer, designated that matter's
/// lawyer DRI ([`plan_lawyer_dri`]) — several of the shapes below need a
/// flagged, currently-licensed lawyer DRI (the Clerk supervision contract
/// does), and there is exactly one lawyer in this fixture, so every matter
/// simply names them.
struct LifecycleScenario {
    client_name: &'static str,
    client_email: &'static str,
    entity_name: &'static str,
    project_code: &'static str,
    project_name: &'static str,
    project_description: &'static str,
    /// `Some((filename, content))` files a `kind: "onboarding"` asset before
    /// any close — this matter reads [`crate::projects::MatterLifecycle::OnboardingOnFile`]
    /// rather than [`crate::projects::MatterLifecycle::NeedsOnboarding`], and
    /// a subsequent close may use an active-side [`crate::projects::ClosureReason`].
    onboarding: Option<(&'static str, &'static str)>,
    /// `Some((filename, content))` files a `kind: "offboarding"` asset before
    /// any close — LAW-36's gate requires this on file before a pitch (a
    /// matter with no onboarding artifact) may close.
    offboarding: Option<(&'static str, &'static str)>,
    /// `Some(reason)` closes the matter with that reason, after any
    /// onboarding/offboarding artifacts above are filed.
    close_reason: Option<crate::projects::ClosureReason>,
    /// Archive the matter after closing it. Terminal — mutually exclusive
    /// with `reopen` in this fixture.
    archive: bool,
    /// Reopen the matter after closing it, clearing `closed_at` and
    /// `closure_reason` per the lifecycle contract.
    reopen: bool,
    /// `Some((name, email))` adds a supervised Clerk once the matter has
    /// reached its final state above.
    clerk: Option<(&'static str, &'static str)>,
}

/// The lifecycle/participation scenario set: an open pitch, a closed-active
/// matter, a closed pitch with its required offboarding artifact, an
/// archived matter, and a matter closed then reopened with a supervised
/// Clerk added once it is active again.
const LIFECYCLE_SCENARIOS: &[LifecycleScenario] = &[
    // An open pitch: no onboarding artifact on file, never closed.
    LifecycleScenario {
        client_name: "Perry Halcyon",
        client_email: "perry.halcyon@synthetic-portfolio.example",
        entity_name: "Fixture Halcyon Ventures, Inc.",
        project_code: "synthetic-portfolio-pitch",
        project_name: "Fixture Halcyon Ventures — Pitch",
        project_description: "Versioned synthetic staging portfolio fixture matter: an open \
            pitch with no onboarding artifact on file. Every party and document on it is \
            invented.",
        onboarding: None,
        offboarding: None,
        close_reason: None,
        archive: false,
        reopen: false,
        clerk: None,
    },
    // A closed active matter: papered, then closed as a completed engagement.
    LifecycleScenario {
        client_name: "Cora Ashworth",
        client_email: "cora.ashworth@synthetic-portfolio.example",
        entity_name: "Fixture Ashworth Logistics, Inc.",
        project_code: "synthetic-portfolio-closed-active",
        project_name: "Fixture Ashworth Logistics — Closed Engagement",
        project_description: "Versioned synthetic staging portfolio fixture matter: a \
            representation that ran to completion and closed with an `engagement_completed` \
            reason. Every party and document on it is invented.",
        onboarding: Some((
            "synthetic-portfolio-closed-active-onboarding.md",
            "# Onboarding\n\nInvented engagement letter content for the closed-active \
             synthetic portfolio fixture matter.\n",
        )),
        offboarding: None,
        close_reason: Some(crate::projects::ClosureReason::EngagementCompleted),
        archive: false,
        reopen: false,
        clerk: None,
    },
    // A closed pitch: never papered, closed with its required offboarding
    // artifact and a pitch-side reason.
    LifecycleScenario {
        client_name: "Milo Fenwick",
        client_email: "milo.fenwick@synthetic-portfolio.example",
        entity_name: "Fixture Fenwick Robotics, Inc.",
        project_code: "synthetic-portfolio-closed-pitch",
        project_name: "Fixture Fenwick Robotics — Declined Pitch",
        project_description: "Versioned synthetic staging portfolio fixture matter: a pitch \
            that never converted, closed with a `pitch_declined` reason and the offboarding \
            artifact LAW-36 requires before a pitch may close. Every party and document on it \
            is invented.",
        onboarding: None,
        offboarding: Some((
            "synthetic-portfolio-closed-pitch-offboarding.md",
            "# Offboarding\n\nInvented offboarding letter content closing out the declined \
             synthetic portfolio fixture pitch.\n",
        )),
        close_reason: Some(crate::projects::ClosureReason::PitchDeclined),
        archive: false,
        reopen: false,
        clerk: None,
    },
    // An archived matter: papered, closed, then archived — terminal.
    LifecycleScenario {
        client_name: "Talia Moorcroft",
        client_email: "talia.moorcroft@synthetic-portfolio.example",
        entity_name: "Fixture Moorcroft Textiles, Inc.",
        project_code: "synthetic-portfolio-archived",
        project_name: "Fixture Moorcroft Textiles — Archived Matter",
        project_description: "Versioned synthetic staging portfolio fixture matter: a \
            completed representation closed and then archived. Every party and document on \
            it is invented.",
        onboarding: Some((
            "synthetic-portfolio-archived-onboarding.md",
            "# Onboarding\n\nInvented engagement letter content for the archived synthetic \
             portfolio fixture matter.\n",
        )),
        offboarding: None,
        close_reason: Some(crate::projects::ClosureReason::EngagementCompleted),
        archive: true,
        reopen: false,
        clerk: None,
    },
    // A reopened matter: papered, closed, then reopened — clearing
    // `closed_at` and `closure_reason` — with a supervised Clerk added once
    // it is active again.
    LifecycleScenario {
        client_name: "Devon Ashgrove",
        client_email: "devon.ashgrove@synthetic-portfolio.example",
        entity_name: "Fixture Ashgrove Analytics, Inc.",
        project_code: "synthetic-portfolio-reopened",
        project_name: "Fixture Ashgrove Analytics — Reopened Matter",
        project_description: "Versioned synthetic staging portfolio fixture matter: closed \
            as a terminated client relationship, then reopened, clearing its closed timestamp \
            and reason. Every party and document on it is invented.",
        onboarding: Some((
            "synthetic-portfolio-reopened-onboarding.md",
            "# Onboarding\n\nInvented engagement letter content for the reopened synthetic \
             portfolio fixture matter.\n",
        )),
        offboarding: None,
        close_reason: Some(crate::projects::ClosureReason::ClientTerminated),
        archive: false,
        reopen: true,
        clerk: Some(("Riley Doyle", "riley.doyle@synthetic-portfolio.example")),
    },
];

/// Why an operator-supplied portfolio target was refused, before any write.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PortfolioTargetError {
    /// No `--target` (or an empty one) was given. Ambiguity refuses exactly
    /// like an unrecognized value — there is no permissive default.
    #[error("the synthetic portfolio requires an explicit target; none was given")]
    MissingTarget,
    /// Anything other than the exact literal [`STAGING_TARGET`] — a
    /// real-matter deployment name, a typo, or a guess.
    #[error("the synthetic portfolio refuses target `{0}`: only `{STAGING_TARGET}` is accepted")]
    UnrecognizedTarget(String),
    /// The disclosure flag could not be read.
    #[error("the synthetic portfolio refuses to apply: {0}")]
    SampleMatters(#[from] crate::config::SampleMattersError),
    /// The disclosure flag reads `false` even though the target named
    /// [`STAGING_TARGET`]: this deployment has not disclosed that its
    /// matters are simulated, so it must not receive the fixture.
    #[error(
        "the synthetic portfolio refuses to apply: NAVIGATOR_SIMULATED_MATTERS does not read \
         `true`, so this deployment has not disclosed that its matters are simulated"
    )]
    NotDisclosedSimulated,
}

/// Refuse before the first write: require the operator to name
/// [`STAGING_TARGET`] explicitly, and require the deployment to have already
/// disclosed simulated matters. See the module docs for why both are
/// required and neither alone triggers a write.
///
/// # Errors
///
/// A [`PortfolioTargetError`] naming exactly why the target was refused.
pub fn verify_target<F: Fn(&str) -> Option<String>>(
    target: Option<&str>,
    environment: crate::DeploymentEnvironment,
    get: F,
) -> Result<(), PortfolioTargetError> {
    match target.map(str::trim) {
        Some(STAGING_TARGET) => {}
        Some(other) if !other.is_empty() => {
            return Err(PortfolioTargetError::UnrecognizedTarget(other.to_string()));
        }
        _ => return Err(PortfolioTargetError::MissingTarget),
    }
    if !crate::config::sample_matters_from(environment, get)? {
        return Err(PortfolioTargetError::NotDisclosedSimulated);
    }
    Ok(())
}

/// Whether an apply performs its writes or only reports what it would do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    DryRun,
    Apply,
}

impl Mode {
    const fn is_apply(self) -> bool {
        matches!(self, Mode::Apply)
    }
}

/// What planning one fixture item found: whether applying it would insert a
/// new row/object or leave an existing one unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlannedAction {
    Create,
    Unchanged,
}

const fn action_for(exists: bool) -> PlannedAction {
    if exists {
        PlannedAction::Unchanged
    } else {
        PlannedAction::Create
    }
}

/// One line of the plan: what kind of row, its natural key, and what
/// applying it would do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedItem {
    pub kind: &'static str,
    pub key: String,
    pub action: PlannedAction,
}

/// The full plan `dry_run`/`apply` produce: the fixture version, the target
/// it was planned or applied against, and one [`PlannedItem`] per row/object
/// the fixture touches, in the order it is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortfolioPlan {
    pub version: u32,
    pub target: String,
    pub items: Vec<PlannedItem>,
}

impl PortfolioPlan {
    fn new() -> Self {
        Self {
            version: PORTFOLIO_VERSION,
            target: STAGING_TARGET.to_string(),
            items: Vec::new(),
        }
    }

    fn record(&mut self, kind: &'static str, key: impl Into<String>, action: PlannedAction) {
        self.items.push(PlannedItem {
            kind,
            key: key.into(),
            action,
        });
    }

    #[must_use]
    pub fn created(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.action == PlannedAction::Create)
            .count()
    }

    #[must_use]
    pub fn unchanged(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.action == PlannedAction::Unchanged)
            .count()
    }
}

/// Report the fixture's plan against `surreal`/`storage` with zero writes.
///
/// # Errors
///
/// [`PortfolioTargetError`] (wrapped) when `target` is refused, or any store
/// read error.
pub async fn dry_run(
    surreal: &SurrealDb,
    storage: &Arc<dyn cloud::StorageService>,
    environment: crate::DeploymentEnvironment,
    target: Option<&str>,
) -> anyhow::Result<PortfolioPlan> {
    dry_run_with(surreal, storage, environment, target, |key| {
        std::env::var(key).ok()
    })
    .await
}

/// [`dry_run`] with the disclosure flag read through `get`, so a test drives
/// the refusal boundary without mutating process environment — the same
/// seam [`crate::config::sample_matters_from`] uses.
async fn dry_run_with<F: Fn(&str) -> Option<String>>(
    surreal: &SurrealDb,
    storage: &Arc<dyn cloud::StorageService>,
    environment: crate::DeploymentEnvironment,
    target: Option<&str>,
    get: F,
) -> anyhow::Result<PortfolioPlan> {
    verify_target(target, environment, get)?;
    run(surreal, storage, Mode::DryRun).await
}

/// Apply the fixture to `surreal`/`storage`. Idempotent: a repeat apply
/// inserts nothing new.
///
/// # Errors
///
/// [`PortfolioTargetError`] (wrapped) when `target` is refused before any
/// write, or any store read/write error.
pub async fn apply(
    surreal: &SurrealDb,
    storage: &Arc<dyn cloud::StorageService>,
    environment: crate::DeploymentEnvironment,
    target: Option<&str>,
) -> anyhow::Result<PortfolioPlan> {
    apply_with(surreal, storage, environment, target, |key| {
        std::env::var(key).ok()
    })
    .await
}

/// [`apply`] with the disclosure flag read through `get`; see
/// [`dry_run_with`].
async fn apply_with<F: Fn(&str) -> Option<String>>(
    surreal: &SurrealDb,
    storage: &Arc<dyn cloud::StorageService>,
    environment: crate::DeploymentEnvironment,
    target: Option<&str>,
    get: F,
) -> anyhow::Result<PortfolioPlan> {
    verify_target(target, environment, get)?;
    run(surreal, storage, Mode::Apply).await
}

async fn run(
    surreal: &SurrealDb,
    storage: &Arc<dyn cloud::StorageService>,
    mode: Mode,
) -> anyhow::Result<PortfolioPlan> {
    let mut plan = PortfolioPlan::new();

    let jurisdiction = crate::jurisdictions::find_by_name(surreal, JURISDICTION_NAME)
        .await?
        .ok_or_else(|| {
            anyhow!("synthetic portfolio: jurisdiction `{JURISDICTION_NAME}` must be seeded first")
        })?;
    let entity_type = crate::entity_types::find_by_name(surreal, CLIENT_ENTITY_TYPE)
        .await?
        .ok_or_else(|| {
            anyhow!("synthetic portfolio: entity type `{CLIENT_ENTITY_TYPE}` must be seeded first")
        })?;
    let template = crate::templates::resolve_exact(surreal, None, ONBOARDING_TEMPLATE_CODE)
        .await?
        .ok_or_else(|| {
            anyhow!(
                "synthetic portfolio: template `{ONBOARDING_TEMPLATE_CODE}` must be seeded first"
            )
        })?;

    let lawyer_id = plan_primary_matter(
        surreal,
        storage,
        &mut plan,
        mode,
        jurisdiction.id,
        entity_type.id,
        template.id,
    )
    .await?;

    for scenario in LIFECYCLE_SCENARIOS {
        plan_lifecycle_scenario(
            surreal,
            storage,
            &mut plan,
            mode,
            lawyer_id,
            entity_type.id,
            jurisdiction.id,
            scenario,
        )
        .await?;
    }

    // An Admin who participates in nothing above — see [`ADMIN_NAME`].
    plan_person(
        surreal,
        &mut plan,
        mode,
        ADMIN_NAME,
        ADMIN_EMAIL,
        crate::persons::Role::Admin,
    )
    .await?;

    Ok(plan)
}

/// Plan and (in [`Mode::Apply`]) write the original fixture matter this
/// module shipped with (ENG-818): one lawyer and client, its entity and
/// project, an onboarding document and notation, and the invoice/trust/IOLTA
/// chain the versioned lifecycle scenarios below do not touch. Returns the
/// fixture's lawyer id, shared as every [`LifecycleScenario`]'s DRI.
async fn plan_primary_matter(
    surreal: &SurrealDb,
    storage: &Arc<dyn cloud::StorageService>,
    plan: &mut PortfolioPlan,
    mode: Mode,
    jurisdiction_id: Uuid,
    entity_type_id: Uuid,
    template_id: Uuid,
) -> anyhow::Result<Uuid> {
    let lawyer_id = plan_person(
        surreal,
        plan,
        mode,
        LAWYER_NAME,
        LAWYER_EMAIL,
        crate::persons::Role::Lawyer,
    )
    .await?;
    let client_id = plan_person(
        surreal,
        plan,
        mode,
        CLIENT_NAME,
        CLIENT_EMAIL,
        crate::persons::Role::Client,
    )
    .await?;

    let entity_id = plan_entity(
        surreal,
        plan,
        mode,
        CLIENT_ENTITY_NAME,
        entity_type_id,
        jurisdiction_id,
    )
    .await?;

    let (project_id, _) = plan_project(
        surreal,
        plan,
        mode,
        PROJECT_CODE,
        PROJECT_NAME,
        PROJECT_DESCRIPTION,
        entity_id,
        jurisdiction_id,
    )
    .await?;

    plan_participation(
        surreal,
        plan,
        mode,
        project_id,
        PROJECT_CODE,
        lawyer_id,
        "lawyer",
    )
    .await?;
    plan_participation(
        surreal,
        plan,
        mode,
        project_id,
        PROJECT_CODE,
        client_id,
        "client",
    )
    .await?;

    plan_document(
        surreal,
        storage,
        plan,
        mode,
        project_id,
        DOCUMENT_FILENAME,
        DOCUMENT_CONTENT,
        "onboarding",
        "Synthetic portfolio intake note",
    )
    .await?;

    plan_notation(surreal, plan, mode, project_id, template_id, lawyer_id).await?;

    plan_trust_deposit(surreal, plan, mode, project_id).await?;

    plan_invoice(surreal, plan, mode, project_id).await?;

    plan_iolta_account(surreal, plan, mode, jurisdiction_id).await?;

    plan_iolta_withdrawal(surreal, plan, mode).await?;

    plan_portal_bundle(storage, plan, mode).await?;

    Ok(lawyer_id)
}

async fn plan_person(
    surreal: &SurrealDb,
    plan: &mut PortfolioPlan,
    mode: Mode,
    name: &str,
    email: &str,
    role: crate::persons::Role,
) -> anyhow::Result<Uuid> {
    let existing = crate::persons::find_by_email_ci(surreal, email).await?;
    plan.record("person", email, action_for(existing.is_some()));
    if mode.is_apply() {
        let row = crate::persons::find_or_create(
            surreal,
            &crate::persons::NewPerson::with_role(name, email, role),
        )
        .await?;
        Ok(row.id)
    } else {
        Ok(existing.map_or(Uuid::nil(), |row| row.id))
    }
}

async fn plan_entity(
    surreal: &SurrealDb,
    plan: &mut PortfolioPlan,
    mode: Mode,
    name: &str,
    entity_type_id: Uuid,
    jurisdiction_id: Uuid,
) -> anyhow::Result<Uuid> {
    let existing = crate::entities::find_by_name_and_type(surreal, name, entity_type_id).await?;
    plan.record("entity", name, action_for(existing.is_some()));
    if mode.is_apply() {
        if let Some(row) = existing {
            return Ok(row.id);
        }
        let row = crate::entities::create(
            surreal,
            &crate::entities::NewEntity {
                name: name.to_string(),
                entity_type_id,
                jurisdiction_id,
                phone: None,
                url: None,
                xero_id: None,
                firm_anchor_key: None,
            },
        )
        .await?;
        Ok(row.id)
    } else {
        Ok(existing.map_or(Uuid::nil(), |row| row.id))
    }
}

#[allow(clippy::too_many_arguments)]
/// Plan (and in [`Mode::Apply`] write) a matter's row, and report whether it
/// already existed before this call — `true` on every apply from the second
/// onward, since a code this fixture writes is otherwise unclaimed.
///
/// A [`LifecycleScenario`] that closes, archives, or reopens its matter reads
/// that flag to run its transitions exactly once: a reopened matter's
/// settled row looks identical to one that was never closed (`"open"`, no
/// `closed_at`), so nothing about its *current* fields can tell an already-
/// applied fixture apart from a fresh one — only "did this row already
/// exist" can.
async fn plan_project(
    surreal: &SurrealDb,
    plan: &mut PortfolioPlan,
    mode: Mode,
    code: &str,
    name: &str,
    description: &str,
    entity_id: Uuid,
    jurisdiction_id: Uuid,
) -> anyhow::Result<(Uuid, bool)> {
    let existing = crate::projects::find_by_code(surreal, code).await?;
    let already_existed = existing.is_some();
    plan.record("project", code, action_for(already_existed));
    if mode.is_apply() {
        // A fresh matter opens `"open"`; an existing one keeps whatever
        // status it already carries. This upsert runs on every apply, and
        // several `LIFECYCLE_SCENARIOS` below move their matter out of
        // `"open"` afterward — hardcoding `"open"` here would silently
        // reset a closed/archived/reopened matter's status back on the very
        // next apply, fighting the transition this module just wrote.
        let status = existing
            .as_ref()
            .map_or_else(|| "open".to_string(), |row| row.status.clone());
        let input = crate::projects::NewProject {
            code: code.to_string(),
            name: name.to_string(),
            status,
            brand: PROJECT_BRAND.to_string(),
            entity_id,
            firm_id: None,
            // The withdrawal/allocation step below needs the matter's
            // pooled IOLTA account, which resolves through this column
            // (`crate::iolta_accounts::for_project`) — unlike the dev
            // sample-matter fixture, this one must set it.
            jurisdiction_id: Some(jurisdiction_id),
            description: Some(description.to_string()),
        };
        let row = match existing {
            Some(row) => crate::projects::upsert_with_id(surreal, row.id, &input).await?,
            None => {
                crate::projects::find_or_create_by_code(surreal, Uuid::now_v7(), &input).await?
            }
        };
        Ok((row.id, already_existed))
    } else {
        Ok((existing.map_or(Uuid::nil(), |row| row.id), already_existed))
    }
}

async fn plan_participation(
    surreal: &SurrealDb,
    plan: &mut PortfolioPlan,
    mode: Mode,
    project_id: Uuid,
    project_code: &str,
    person_id: Uuid,
    participation: &str,
) -> anyhow::Result<()> {
    let existing =
        crate::projects::participation_for_person(surreal, person_id, project_id).await?;
    plan.record(
        "participation",
        format!("{project_code}:{participation}:{person_id}"),
        action_for(existing.is_some()),
    );
    if mode.is_apply() && existing.is_none() {
        crate::projects::add_participation(surreal, project_id, person_id, participation).await?;
    }
    Ok(())
}

/// Add the fixture's lawyer to a matter as its accountable lawyer DRI in one
/// step, through [`crate::projects::designate_dri_in_surreal`]: idempotent
/// on `(person, project)`, it creates the participation row when absent and
/// otherwise only flags an existing one. Several [`LifecycleScenario`]s need
/// a flagged, currently-licensed lawyer DRI — the Clerk supervision contract
/// in `docs/access-model.md#clerk` reads for one — and this fixture has
/// exactly one lawyer, so every new matter simply names them.
async fn plan_lawyer_dri(
    surreal: &SurrealDb,
    plan: &mut PortfolioPlan,
    mode: Mode,
    project_id: Uuid,
    project_code: &str,
    lawyer_id: Uuid,
) -> anyhow::Result<()> {
    let existing =
        crate::projects::participation_for_person(surreal, lawyer_id, project_id).await?;
    let already_dri = existing.is_some_and(|row| row.is_lawyer_dri);
    plan.record(
        "participation",
        format!("{project_code}:lawyer_dri:{lawyer_id}"),
        action_for(already_dri),
    );
    if mode.is_apply() && !already_dri {
        crate::projects::designate_dri_in_surreal(
            surreal,
            project_id,
            lawyer_id,
            crate::projects::DriSide::Lawyer,
        )
        .await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn plan_document(
    surreal: &SurrealDb,
    storage: &Arc<dyn cloud::StorageService>,
    plan: &mut PortfolioPlan,
    mode: Mode,
    project_id: Uuid,
    filename: &str,
    content: &str,
    kind: &str,
    description: &str,
) -> anyhow::Result<()> {
    let sha256_hex = crate::assets::sha256_hex(content.as_bytes());
    let existing =
        crate::assets::find_filed_copy(surreal, project_id, filename, &sha256_hex).await?;
    plan.record("document", filename, action_for(existing.is_some()));
    if mode.is_apply() {
        crate::documents::ingest_bytes_exactly_once(
            surreal,
            storage,
            &crate::documents::IngestArgs {
                project_id,
                source: "generated",
                filename,
                kind,
                content_type: "text/markdown",
                description: Some(description),
                secondary_storage_key: None,
                visibility: crate::documents::visibility::CLIENT,
            },
            content.as_bytes(),
        )
        .await?;
    }
    Ok(())
}

/// Close a matter with `reason`, unless `project_already_existed`.
///
/// A reopened matter's settled row (`"open"`, no `closed_at`) is
/// indistinguishable from one that was never closed, so the *current* row
/// cannot tell a fresh apply apart from a repeat one — only whether this
/// code already named a row before [`plan_project`] ran this pass can, which
/// is exactly what `project_already_existed` carries. Skipping the repeat
/// also sidesteps a real refusal: closing an already-`archived` matter is
/// rejected by [`crate::projects::transition_project_with_reason`] as a
/// transition out of a terminal state.
async fn plan_close(
    surreal: &SurrealDb,
    plan: &mut PortfolioPlan,
    mode: Mode,
    project_id: Uuid,
    project_code: &str,
    reason: crate::projects::ClosureReason,
    project_already_existed: bool,
) -> anyhow::Result<()> {
    plan.record(
        "project_close",
        format!("{project_code}:{}", reason.as_str()),
        action_for(project_already_existed),
    );
    if mode.is_apply() && !project_already_existed {
        crate::projects::transition_project_with_reason(
            surreal,
            project_id,
            crate::projects::Transition::Close,
            Some(reason),
            None,
        )
        .await?;
    }
    Ok(())
}

/// Archive an already-closed matter, unless `project_already_existed` — see
/// [`plan_close`] for why the matter's current row cannot answer that on its
/// own for a scenario that also reopens.
async fn plan_archive(
    surreal: &SurrealDb,
    plan: &mut PortfolioPlan,
    mode: Mode,
    project_id: Uuid,
    project_code: &str,
    project_already_existed: bool,
) -> anyhow::Result<()> {
    plan.record(
        "project_archive",
        project_code,
        action_for(project_already_existed),
    );
    if mode.is_apply() && !project_already_existed {
        crate::projects::transition_project_with_reason(
            surreal,
            project_id,
            crate::projects::Transition::Archive,
            None,
            None,
        )
        .await?;
    }
    Ok(())
}

/// Reopen a closed matter, clearing `closed_at` and `closure_reason`, unless
/// `project_already_existed` — see [`plan_close`] for why the matter's
/// current row cannot answer that on its own once it is back to `"open"`.
async fn plan_reopen(
    surreal: &SurrealDb,
    plan: &mut PortfolioPlan,
    mode: Mode,
    project_id: Uuid,
    project_code: &str,
    project_already_existed: bool,
) -> anyhow::Result<()> {
    plan.record(
        "project_reopen",
        project_code,
        action_for(project_already_existed),
    );
    if mode.is_apply() && !project_already_existed {
        crate::projects::transition_project_with_reason(
            surreal,
            project_id,
            crate::projects::Transition::Reopen,
            None,
            None,
        )
        .await?;
    }
    Ok(())
}

/// Plan and (in [`Mode::Apply`]) write one [`LifecycleScenario`]: its
/// client, entity, and matter; the fixture lawyer as that matter's DRI; any
/// onboarding/offboarding artifacts; the close/archive/reopen sequence the
/// scenario declares; and a supervised Clerk once the matter has reached its
/// final state.
#[allow(clippy::too_many_arguments)]
async fn plan_lifecycle_scenario(
    surreal: &SurrealDb,
    storage: &Arc<dyn cloud::StorageService>,
    plan: &mut PortfolioPlan,
    mode: Mode,
    lawyer_id: Uuid,
    entity_type_id: Uuid,
    jurisdiction_id: Uuid,
    scenario: &LifecycleScenario,
) -> anyhow::Result<()> {
    let client_id = plan_person(
        surreal,
        plan,
        mode,
        scenario.client_name,
        scenario.client_email,
        crate::persons::Role::Client,
    )
    .await?;
    let entity_id = plan_entity(
        surreal,
        plan,
        mode,
        scenario.entity_name,
        entity_type_id,
        jurisdiction_id,
    )
    .await?;
    let (project_id, project_already_existed) = plan_project(
        surreal,
        plan,
        mode,
        scenario.project_code,
        scenario.project_name,
        scenario.project_description,
        entity_id,
        jurisdiction_id,
    )
    .await?;

    plan_lawyer_dri(
        surreal,
        plan,
        mode,
        project_id,
        scenario.project_code,
        lawyer_id,
    )
    .await?;
    plan_participation(
        surreal,
        plan,
        mode,
        project_id,
        scenario.project_code,
        client_id,
        "client",
    )
    .await?;

    plan_lifecycle_transitions(
        surreal,
        storage,
        plan,
        mode,
        project_id,
        project_already_existed,
        scenario,
    )
    .await
}

/// The artifact-filing, close/archive/reopen, and Clerk-assignment half of
/// [`plan_lifecycle_scenario`], split out only to keep each function under
/// this workspace's line-count lint.
async fn plan_lifecycle_transitions(
    surreal: &SurrealDb,
    storage: &Arc<dyn cloud::StorageService>,
    plan: &mut PortfolioPlan,
    mode: Mode,
    project_id: Uuid,
    project_already_existed: bool,
    scenario: &LifecycleScenario,
) -> anyhow::Result<()> {
    if let Some((filename, content)) = scenario.onboarding {
        plan_document(
            surreal,
            storage,
            plan,
            mode,
            project_id,
            filename,
            content,
            "onboarding",
            "Synthetic onboarding artifact",
        )
        .await?;
    }
    if let Some((filename, content)) = scenario.offboarding {
        plan_document(
            surreal,
            storage,
            plan,
            mode,
            project_id,
            filename,
            content,
            "offboarding",
            "Synthetic offboarding artifact",
        )
        .await?;
    }
    if let Some(reason) = scenario.close_reason {
        plan_close(
            surreal,
            plan,
            mode,
            project_id,
            scenario.project_code,
            reason,
            project_already_existed,
        )
        .await?;
    }
    if scenario.archive {
        plan_archive(
            surreal,
            plan,
            mode,
            project_id,
            scenario.project_code,
            project_already_existed,
        )
        .await?;
    }
    if scenario.reopen {
        plan_reopen(
            surreal,
            plan,
            mode,
            project_id,
            scenario.project_code,
            project_already_existed,
        )
        .await?;
    }
    if let Some((clerk_name, clerk_email)) = scenario.clerk {
        let clerk_id = plan_person(
            surreal,
            plan,
            mode,
            clerk_name,
            clerk_email,
            crate::persons::Role::Clerk,
        )
        .await?;
        plan_participation(
            surreal,
            plan,
            mode,
            project_id,
            scenario.project_code,
            clerk_id,
            "clerk",
        )
        .await?;
    }

    Ok(())
}

async fn plan_notation(
    surreal: &SurrealDb,
    plan: &mut PortfolioPlan,
    mode: Mode,
    project_id: Uuid,
    template_id: Uuid,
    lawyer_id: Uuid,
) -> anyhow::Result<()> {
    let existing = crate::notations::find_by_project_template_person(
        surreal,
        project_id,
        template_id,
        lawyer_id,
    )
    .await?;
    plan.record(
        "notation",
        format!("{ONBOARDING_TEMPLATE_CODE}:{project_id}"),
        action_for(existing.is_some()),
    );
    if mode.is_apply() && existing.is_none() {
        crate::notations::create(
            surreal,
            &crate::notations::NewNotation::new(template_id, lawyer_id, project_id, NOTATION_STATE),
        )
        .await?;
    }
    Ok(())
}

async fn plan_trust_deposit(
    surreal: &SurrealDb,
    plan: &mut PortfolioPlan,
    mode: Mode,
    project_id: Uuid,
) -> anyhow::Result<()> {
    let existing = crate::trust::has_external_ref(surreal, project_id, TRUST_DEPOSIT_EXTERNAL_REF)
        .await
        .map_err(|error| anyhow!("synthetic portfolio: trust deposit lookup: {error}"))?;
    plan.record(
        "trust_movement",
        TRUST_DEPOSIT_EXTERNAL_REF,
        action_for(existing),
    );
    if mode.is_apply() {
        let occurred_at = chrono::Utc::now().to_rfc3339();
        let mut deposit = crate::trust::Movement::deposit(
            project_id,
            "USD",
            usd_amount_string(INVOICE_CENTS),
            INVOICE_CENTS,
            occurred_at,
        );
        deposit.external_ref = Some(TRUST_DEPOSIT_EXTERNAL_REF.to_string());
        crate::trust::record_project_movement(surreal, project_id, &deposit)
            .await
            .map_err(|error| anyhow!("synthetic portfolio: trust deposit: {error}"))?;
    }
    Ok(())
}

fn usd_amount_string(cents: i64) -> String {
    format!("{}.{:02}", cents / 100, cents % 100)
}

async fn plan_invoice(
    surreal: &SurrealDb,
    plan: &mut PortfolioPlan,
    mode: Mode,
    project_id: Uuid,
) -> anyhow::Result<()> {
    let existing = crate::xero_invoices::find_for_allocation(surreal, XERO_INVOICE_ID).await?;
    plan.record("invoice", XERO_INVOICE_ID, action_for(existing.is_some()));
    if mode.is_apply() {
        crate::xero_invoices::upsert(
            surreal,
            &crate::xero_invoices::UpsertXeroInvoice {
                project_id,
                xero_invoice_id: XERO_INVOICE_ID.to_string(),
                reference: XERO_INVOICE_REFERENCE.to_string(),
                status: "AUTHORISED".to_string(),
                amount_cents: INVOICE_CENTS,
                currency: "USD".to_string(),
                issued_at: chrono::Utc::now(),
                due_at: None,
            },
        )
        .await?;
    }
    Ok(())
}

async fn plan_iolta_account(
    surreal: &SurrealDb,
    plan: &mut PortfolioPlan,
    mode: Mode,
    jurisdiction_id: Uuid,
) -> anyhow::Result<()> {
    let existing = crate::iolta_accounts::for_jurisdiction(surreal, jurisdiction_id)
        .await?
        .filter(|account| account.xero_account_id == IOLTA_XERO_ACCOUNT_ID);
    plan.record(
        "iolta_account",
        IOLTA_XERO_ACCOUNT_ID,
        action_for(existing.is_some()),
    );
    if mode.is_apply() {
        crate::iolta_accounts::upsert(
            surreal,
            &crate::iolta_accounts::UpsertIoltaAccount {
                jurisdiction_id,
                xero_account_id: IOLTA_XERO_ACCOUNT_ID.to_string(),
                xero_account_code: None,
                name: IOLTA_ACCOUNT_NAME.to_string(),
                currency: "USD".to_string(),
                balance_cents: INVOICE_CENTS,
                mirrored_at: chrono::Utc::now(),
            },
        )
        .await?;
    }
    Ok(())
}

async fn plan_iolta_withdrawal(
    surreal: &SurrealDb,
    plan: &mut PortfolioPlan,
    mode: Mode,
) -> anyhow::Result<()> {
    let existing = crate::iolta_withdrawals::find(surreal, IOLTA_XERO_TRANSACTION_ID).await?;
    plan.record(
        "iolta_withdrawal",
        IOLTA_XERO_TRANSACTION_ID,
        action_for(existing.is_some()),
    );
    if mode.is_apply() {
        crate::iolta_withdrawals::apply(
            surreal,
            &crate::iolta_withdrawals::WithdrawalInput {
                xero_transaction_id: IOLTA_XERO_TRANSACTION_ID.to_string(),
                xero_account_id: IOLTA_XERO_ACCOUNT_ID.to_string(),
                total_cents: INVOICE_CENTS,
                currency: "USD".to_string(),
                occurred_at: chrono::Utc::now(),
                lines: vec![crate::iolta_withdrawals::AllocationInput {
                    invoice_reference: XERO_INVOICE_ID.to_string(),
                    amount_cents: INVOICE_CENTS,
                }],
            },
        )
        .await
        .map_err(|error| anyhow!("synthetic portfolio: iolta withdrawal: {error}"))?;
    }
    Ok(())
}

async fn plan_portal_bundle(
    storage: &Arc<dyn cloud::StorageService>,
    plan: &mut PortfolioPlan,
    mode: Mode,
) -> anyhow::Result<()> {
    let key = format!(
        "{}/{}",
        crate::sample_project::portal_prefix(PROJECT_CODE),
        crate::sample_project::ENTRY_DOCUMENT
    );
    let existing = storage.exists(&key).await?;
    plan.record("portal_bundle", &key, action_for(existing));
    if mode.is_apply() {
        storage
            .put_cached(
                &key,
                PORTAL_INDEX.as_bytes(),
                "text/html; charset=utf-8",
                crate::sample_project::ENTRY_CACHE_CONTROL,
            )
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::mem_surreal;
    use crate::DeploymentEnvironment::{Dev, Production};

    /// One row of [`apply_refuses_every_unsafe_target_before_any_write`]'s
    /// table: the target, the environment, and the disclosure value it
    /// reads.
    type RefusalCase = (
        Option<&'static str>,
        crate::DeploymentEnvironment,
        fn(&str) -> Option<String>,
    );

    /// A filesystem-backed storage under a fresh, uniquely-named temp dir
    /// per call. Unlike [`crate::seed`]'s tests — whose documents are
    /// content-addressed and vary per test — this fixture's portal-bundle
    /// key is fixed, so a shared directory would leak one test's write into
    /// another's "does this already exist" read; each test gets its own.
    async fn fs_storage() -> Arc<dyn cloud::StorageService> {
        Arc::new(
            cloud::FsStorage::new(std::env::temp_dir().join(format!(
                "navigator-synthetic-portfolio-test-{}",
                Uuid::now_v7()
            )))
            .await
            .expect("temp FsStorage"),
        )
    }

    /// Every fixture item this module writes resolves against the
    /// canonical seed's Nevada jurisdiction, C-Corp entity type, and
    /// onboarding-letter template — present after
    /// [`crate::seed::seed_canonical`], which every deployment profile runs
    /// on every boot (unlike the disposable sample-matter layer).
    async fn canonical(surreal: &SurrealDb, storage: &Arc<dyn cloud::StorageService>) {
        crate::seed::seed_canonical(surreal, storage)
            .await
            .expect("canonical seed");
    }

    fn discloses(value: &'static str) -> impl Fn(&str) -> Option<String> {
        move |_| Some(value.to_string())
    }

    fn undisclosed(_: &str) -> Option<String> {
        None
    }

    /// The whole refusal boundary, pure and cluster/store-free: only the
    /// exact literal `staging` is ever accepted, and even that is refused
    /// unless the deployment already discloses simulated matters.
    #[test]
    fn verify_target_accepts_only_the_exact_staging_literal_and_requires_disclosure() {
        let disclosed = discloses("true");

        assert_eq!(
            verify_target(None, Production, &disclosed),
            Err(PortfolioTargetError::MissingTarget)
        );
        assert_eq!(
            verify_target(Some(""), Production, &disclosed),
            Err(PortfolioTargetError::MissingTarget)
        );
        assert_eq!(
            verify_target(Some("   "), Production, &disclosed),
            Err(PortfolioTargetError::MissingTarget)
        );
        for other in ["production", "Staging", "STAGING", "dev", "staging "] {
            // Note: "staging " is trimmed to "staging" and therefore accepted;
            // every other near-miss above is refused.
            if other == "staging " {
                continue;
            }
            assert_eq!(
                verify_target(Some(other), Production, &disclosed),
                Err(PortfolioTargetError::UnrecognizedTarget(other.to_string())),
                "{other} must be refused"
            );
        }
        assert!(verify_target(Some(STAGING_TARGET), Production, &disclosed).is_ok());
        assert!(verify_target(Some(" staging "), Production, &disclosed).is_ok());
        assert!(verify_target(Some(STAGING_TARGET), Dev, &disclosed).is_ok());

        // The right target with no disclosure is still refused — disclosure
        // is required, but is not by itself the trigger (see module docs).
        assert_eq!(
            verify_target(Some(STAGING_TARGET), Production, undisclosed),
            Err(PortfolioTargetError::NotDisclosedSimulated)
        );
        assert_eq!(
            verify_target(Some(STAGING_TARGET), Production, discloses("false")),
            Err(PortfolioTargetError::NotDisclosedSimulated)
        );
    }

    #[test]
    fn verify_target_rejects_an_invalid_disclosure_value_rather_than_guessing() {
        assert!(matches!(
            verify_target(Some(STAGING_TARGET), Production, discloses("yes")),
            Err(PortfolioTargetError::SampleMatters(_))
        ));
    }

    #[tokio::test]
    async fn dry_run_reports_a_full_create_plan_and_reaches_no_write() {
        let surreal = mem_surreal().await;
        let storage = fs_storage().await;
        canonical(&surreal, &storage).await;

        let plan = dry_run_with(
            &surreal,
            &storage,
            Production,
            Some(STAGING_TARGET),
            discloses("true"),
        )
        .await
        .expect("dry run");

        assert!(!plan.items.is_empty());
        assert_eq!(
            plan.unchanged(),
            0,
            "a clean checkout has nothing to leave unchanged"
        );
        assert_eq!(plan.created(), plan.items.len());

        assert!(crate::persons::find_by_email_ci(&surreal, LAWYER_EMAIL)
            .await
            .unwrap()
            .is_none());
        assert!(crate::projects::find_by_code(&surreal, PROJECT_CODE)
            .await
            .unwrap()
            .is_none());
        assert!(
            crate::xero_invoices::find_for_allocation(&surreal, XERO_INVOICE_ID)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            crate::iolta_withdrawals::find(&surreal, IOLTA_XERO_TRANSACTION_ID)
                .await
                .unwrap()
                .is_none()
        );
        assert!(!storage
            .exists(&format!(
                "{}/{}",
                crate::sample_project::portal_prefix(PROJECT_CODE),
                crate::sample_project::ENTRY_DOCUMENT
            ))
            .await
            .unwrap());
    }

    /// Two independent clean checkouts plan the identical fixture: same
    /// items, same order, same natural keys. The manifest is fixed Rust
    /// data, so this is the version/identity determinism the acceptance
    /// criteria asks for, made concrete.
    #[tokio::test]
    async fn the_same_version_produces_the_same_plan_from_two_clean_checkouts() {
        let first_surreal = mem_surreal().await;
        let first_storage = fs_storage().await;
        canonical(&first_surreal, &first_storage).await;
        let first = dry_run_with(
            &first_surreal,
            &first_storage,
            Production,
            Some(STAGING_TARGET),
            discloses("true"),
        )
        .await
        .expect("first dry run");

        let second_surreal = mem_surreal().await;
        let second_storage = fs_storage().await;
        canonical(&second_surreal, &second_storage).await;
        let second = dry_run_with(
            &second_surreal,
            &second_storage,
            Production,
            Some(STAGING_TARGET),
            discloses("true"),
        )
        .await
        .expect("second dry run");

        assert_eq!(first.version, second.version);
        assert_eq!(first.items, second.items);
    }

    #[tokio::test]
    async fn apply_is_idempotent_across_every_domain_the_fixture_touches() {
        let surreal = mem_surreal().await;
        let storage = fs_storage().await;
        canonical(&surreal, &storage).await;

        let first = apply_with(
            &surreal,
            &storage,
            Production,
            Some(STAGING_TARGET),
            discloses("true"),
        )
        .await
        .expect("first apply");
        assert!(first.created() > 0);
        assert_eq!(first.unchanged(), 0);

        let second = apply_with(
            &surreal,
            &storage,
            Production,
            Some(STAGING_TARGET),
            discloses("true"),
        )
        .await
        .expect("second apply");
        assert_eq!(
            second.created(),
            0,
            "a repeat apply must insert nothing new"
        );
        assert_eq!(second.unchanged(), first.items.len());
        let mut expected = first.items.clone();
        for item in &mut expected {
            item.action = PlannedAction::Unchanged;
        }
        assert_eq!(
            second.items, expected,
            "same plan shape the second time, every item now Unchanged"
        );

        let project = crate::projects::find_by_code(&surreal, PROJECT_CODE)
            .await
            .unwrap()
            .expect("project exists after apply");
        assert_eq!(
            project.jurisdiction_id,
            Some(
                crate::jurisdictions::find_by_name(&surreal, JURISDICTION_NAME)
                    .await
                    .unwrap()
                    .unwrap()
                    .id
            )
        );

        assert_eq!(
            crate::projects::participations_for_project(&surreal, project.id)
                .await
                .unwrap()
                .len(),
            2,
            "lawyer and client, no duplicates on a repeat apply"
        );
        assert_eq!(
            crate::assets::for_project(&surreal, project.id)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            crate::notations::list_by_project(&surreal, project.id)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            crate::xero_invoices::for_projects(&surreal, &[project.id])
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            crate::iolta_withdrawals::lines_for(&surreal, IOLTA_XERO_TRANSACTION_ID)
                .await
                .unwrap()
                .len(),
            1
        );

        let position = crate::trust::position_for_project(&surreal, project.id)
            .await
            .unwrap();
        assert_eq!(
            position.held_cents(),
            0,
            "the withdrawal drew exactly the deposit; a second apply must not draw again"
        );
    }

    /// One resolved lifecycle scenario, read back by its stable code.
    struct ResolvedScenario {
        project: crate::projects::Project,
        client_id: uuid::Uuid,
    }

    async fn resolve_scenario(
        surreal: &SurrealDb,
        scenario: &LifecycleScenario,
    ) -> ResolvedScenario {
        let project = crate::projects::find_by_code(surreal, scenario.project_code)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("{} must exist after apply", scenario.project_code));
        let client_id = crate::persons::find_by_email_ci(surreal, scenario.client_email)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("{} must exist after apply", scenario.client_email))
            .id;
        ResolvedScenario { project, client_id }
    }

    /// Status and the closure fields each scenario's transitions must leave
    /// behind — the direct proof of the LAW-36 reason contract and of the
    /// reopen-clears-both-fields rule.
    async fn assert_lifecycle_status_and_closure(surreal: &SurrealDb, s: &LifecycleScenarios) {
        assert_eq!(s.pitch.project.status, "open");
        assert_eq!(s.pitch.project.closure_reason, None);

        assert_eq!(s.closed_active.project.status, "closed");
        assert_eq!(
            s.closed_active.project.closure_reason.as_deref(),
            Some("engagement_completed")
        );

        assert_eq!(s.closed_pitch.project.status, "closed");
        assert_eq!(
            s.closed_pitch.project.closure_reason.as_deref(),
            Some("pitch_declined")
        );
        let (_, has_closing) = crate::projects::matter_lifecycle_sets(
            surreal,
            std::slice::from_ref(&s.closed_pitch.project),
        )
        .await
        .unwrap();
        assert!(
            has_closing.contains(&s.closed_pitch.project.id),
            "the closed pitch must carry the offboarding artifact LAW-36 requires before it \
             could close at all"
        );

        assert_eq!(s.archived.project.status, "archived");

        // Reopen clears both the closed timestamp and the reason, per the
        // lifecycle contract — not just one of the two.
        assert_eq!(s.reopened.project.status, "open");
        assert_eq!(s.reopened.project.closed_at, None);
        assert_eq!(s.reopened.project.closure_reason, None);
    }

    /// The list pill every scenario renders, computed the same way
    /// `webapp::project_list::project_row` does — proven rather than
    /// assumed, since an archived matter's pill is not itself "closed" under
    /// the current shipped derivation (`matter_lifecycle` only special-cases
    /// the literal `"closed"` status; archived matters still branch on
    /// `missing_onboarding`, same as an open one).
    async fn assert_lifecycle_pill_labels(surreal: &SurrealDb, s: &LifecycleScenarios) {
        for resolved in s.all_scenarios() {
            let (has_engagement, has_closing) = crate::projects::matter_lifecycle_sets(
                surreal,
                std::slice::from_ref(&resolved.project),
            )
            .await
            .unwrap();
            let (missing_onboarding, missing_offboarding_letter) = crate::projects::matter_flags(
                has_engagement.contains(&resolved.project.id),
                &resolved.project.status,
                has_closing.contains(&resolved.project.id),
            );
            let lifecycle = crate::projects::matter_lifecycle(
                &resolved.project.status,
                missing_onboarding,
                missing_offboarding_letter,
            );
            let expected = if resolved.project.status == "closed" {
                "closed"
            } else if missing_onboarding {
                "pitch"
            } else {
                "active"
            };
            assert_eq!(
                lifecycle.label(),
                expected,
                "{} lifecycle label",
                resolved.project.code
            );
        }
    }

    /// Every synthetic client sees only their own matter, never another
    /// scenario's — the participation gate the access model documents,
    /// exercised across the whole scenario set rather than one pair.
    async fn assert_clients_fail_closed_across_matters(
        surreal: &SurrealDb,
        s: &LifecycleScenarios,
    ) {
        let all = s.all_projects();
        for owner in s.all_scenarios() {
            for candidate in all {
                let viewer = crate::access::matter_viewer(
                    surreal,
                    Some(owner.client_id),
                    crate::persons::Role::Client,
                    candidate.id,
                )
                .await
                .unwrap();
                if candidate.id == owner.project.id {
                    assert!(
                        matches!(viewer, Some(crate::access::MatterViewer::Client)),
                        "{} must see their own matter",
                        candidate.code
                    );
                } else {
                    assert!(
                        viewer.is_none(),
                        "{} must not see {}'s matter (cross-project fail-closed)",
                        candidate.code,
                        owner.project.code
                    );
                }
            }
        }
    }

    /// The supervised Clerk: sees the one matter it was added to as
    /// `MatterViewer::Clerk`, and fails closed on every other one.
    async fn assert_supervised_clerk_fails_closed(surreal: &SurrealDb, s: &LifecycleScenarios) {
        let clerk_id =
            crate::persons::find_by_email_ci(surreal, "riley.doyle@synthetic-portfolio.example")
                .await
                .unwrap()
                .expect("clerk exists")
                .id;
        for candidate in s.all_projects() {
            let viewer = crate::access::matter_viewer(
                surreal,
                Some(clerk_id),
                crate::persons::Role::Clerk,
                candidate.id,
            )
            .await
            .unwrap();
            if candidate.id == s.reopened.project.id {
                assert!(
                    matches!(viewer, Some(crate::access::MatterViewer::Clerk)),
                    "the supervised Clerk must resolve on the reopened matter it was added to"
                );
            } else {
                assert!(
                    viewer.is_none(),
                    "the supervised Clerk must not resolve on {} (cross-project fail-closed)",
                    candidate.code
                );
            }
        }
    }

    /// The unassigned Admin: participates in nothing, so `matter_viewer`
    /// answers `None` on every one of these matters — the same ENG-81
    /// participation-only shape the detail dispatcher renders instead of the
    /// row this predicate alone would otherwise deny.
    async fn assert_unassigned_admin_resolves_nowhere(surreal: &SurrealDb, s: &LifecycleScenarios) {
        let admin_id = crate::persons::find_by_email_ci(surreal, ADMIN_EMAIL)
            .await
            .unwrap()
            .expect("admin exists")
            .id;
        for candidate in s.all_projects() {
            assert_eq!(
                crate::projects::participation_for_person(surreal, admin_id, candidate.id)
                    .await
                    .unwrap(),
                None,
                "the unassigned Admin must hold no participation row on {}",
                candidate.code
            );
            let viewer = crate::access::matter_viewer(
                surreal,
                Some(admin_id),
                crate::persons::Role::Admin,
                candidate.id,
            )
            .await
            .unwrap();
            assert!(
                viewer.is_none(),
                "the unassigned Admin must not resolve to a matter viewer on {}",
                candidate.code
            );
        }
    }

    /// The five resolved [`LifecycleScenario`] rows, in `LIFECYCLE_SCENARIOS`
    /// order, threaded through the assertion helpers above rather than five
    /// loose local bindings.
    struct LifecycleScenarios {
        pitch: ResolvedScenario,
        closed_active: ResolvedScenario,
        closed_pitch: ResolvedScenario,
        archived: ResolvedScenario,
        reopened: ResolvedScenario,
    }

    impl LifecycleScenarios {
        fn all_scenarios(&self) -> [&ResolvedScenario; 5] {
            [
                &self.pitch,
                &self.closed_active,
                &self.closed_pitch,
                &self.archived,
                &self.reopened,
            ]
        }

        fn all_projects(&self) -> [&crate::projects::Project; 5] {
            self.all_scenarios().map(|s| &s.project)
        }
    }

    /// Every lifecycle/participation scenario `LIFECYCLE_SCENARIOS` declares,
    /// proven against a single apply: the matter status and closure fields
    /// each scenario's transitions must leave behind, the lifecycle pill each
    /// renders through the same [`crate::projects::matter_lifecycle`] the
    /// Projects list uses, the supervised-Clerk and unassigned-Admin
    /// participation shapes, and that `store::access::matter_viewer` fails
    /// closed across every pair of these matters — no synthetic client, the
    /// Clerk, or the Admin resolves to anything on a matter they do not
    /// participate in.
    #[tokio::test]
    async fn apply_produces_every_lifecycle_and_participation_scenario() {
        let surreal = mem_surreal().await;
        let storage = fs_storage().await;
        canonical(&surreal, &storage).await;

        apply_with(
            &surreal,
            &storage,
            Production,
            Some(STAGING_TARGET),
            discloses("true"),
        )
        .await
        .expect("apply");

        let scenarios = LifecycleScenarios {
            pitch: resolve_scenario(&surreal, &LIFECYCLE_SCENARIOS[0]).await,
            closed_active: resolve_scenario(&surreal, &LIFECYCLE_SCENARIOS[1]).await,
            closed_pitch: resolve_scenario(&surreal, &LIFECYCLE_SCENARIOS[2]).await,
            archived: resolve_scenario(&surreal, &LIFECYCLE_SCENARIOS[3]).await,
            reopened: resolve_scenario(&surreal, &LIFECYCLE_SCENARIOS[4]).await,
        };

        assert_lifecycle_status_and_closure(&surreal, &scenarios).await;
        assert_lifecycle_pill_labels(&surreal, &scenarios).await;
        assert_clients_fail_closed_across_matters(&surreal, &scenarios).await;
        assert_supervised_clerk_fails_closed(&surreal, &scenarios).await;
        assert_unassigned_admin_resolves_nowhere(&surreal, &scenarios).await;
    }

    /// The persistent-staging distinction the acceptance criteria calls
    /// out by name: the ordinary boot seed — even under the `Production`
    /// profile with disclosure already `true`, exactly what persistent
    /// staging runs — never applies this fixture. Only the explicit apply
    /// path does.
    #[tokio::test]
    async fn a_production_profile_boot_does_not_auto_apply_the_synthetic_portfolio() {
        let surreal = mem_surreal().await;
        let storage = fs_storage().await;

        crate::seed::seed_environment_with(
            &surreal,
            &storage,
            Production,
            crate::seed::BrandSeed::Neon,
        )
        .await
        .expect("ordinary boot seed");
        assert!(
            crate::projects::find_by_code(&surreal, PROJECT_CODE)
                .await
                .unwrap()
                .is_none(),
            "a boot must never auto-apply the synthetic portfolio"
        );

        let plan = apply_with(
            &surreal,
            &storage,
            Production,
            Some(STAGING_TARGET),
            discloses("true"),
        )
        .await
        .expect("explicit apply");
        assert!(plan.created() > 0);
        assert!(crate::projects::find_by_code(&surreal, PROJECT_CODE)
            .await
            .unwrap()
            .is_some());
    }

    /// The write gate refuses a real-matter or ambiguous target before the
    /// first write, under every combination this module accepts as input.
    #[tokio::test]
    async fn apply_refuses_every_unsafe_target_before_any_write() {
        let surreal = mem_surreal().await;
        let storage = fs_storage().await;
        canonical(&surreal, &storage).await;

        let cases: [RefusalCase; 4] = [
            (None, Production, undisclosed),
            (Some("production"), Production, |_| Some("true".to_string())),
            (Some(STAGING_TARGET), Production, undisclosed),
            (Some(STAGING_TARGET), Dev, |_| Some("false".to_string())),
        ];
        for (target, environment, get) in cases {
            assert!(
                apply_with(&surreal, &storage, environment, target, get)
                    .await
                    .is_err(),
                "{target:?} under {environment:?} must be refused"
            );
        }

        assert!(
            crate::persons::find_by_email_ci(&surreal, LAWYER_EMAIL)
                .await
                .unwrap()
                .is_none(),
            "no refused attempt may reach a write"
        );
        assert!(crate::projects::find_by_code(&surreal, PROJECT_CODE)
            .await
            .unwrap()
            .is_none());
    }
}
