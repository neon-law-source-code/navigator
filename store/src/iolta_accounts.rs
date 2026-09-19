//! The read-only mirror of one pooled IOLTA bank account per US state.
//!
//! IOLTA is pooled **by jurisdiction**: every client whose matter is governed
//! by Nevada law shares one Nevada trust account at the bank, and the firm's
//! obligation is to know, at any moment, how much of that pooled balance
//! belongs to each matter. Xero is the books for that account — the firm's
//! bookkeeper records deposits and the withdrawal there, and Navigator reads.
//! Navigator opens no account, moves no money, and writes nothing back.
//!
//! A row's identity is its [`IoltaAccount::jurisdiction_id`], UNIQUE in the
//! schema: one pooled account per state, so a second Xero account claiming
//! Nevada is a refused write ([`IoltaAccountError::JurisdictionTaken`])
//! rather than a silent second master balance. The row's own record id is the
//! Xero `AccountID`, the same choice [`crate::xero_invoices`] makes for the
//! same reason — two nightly reads of one Xero account address one key rather
//! than minting two rows the index would only notice afterwards.
//!
//! # Which account a matter belongs to
//!
//! [`for_project`] reads `project.jurisdiction_id` — the matter's **general**
//! governing jurisdiction, not an IOLTA-specific column — and returns that
//! state's account. It answers `Ok(None)`, never a guess, when the matter has
//! no jurisdiction, when the jurisdiction is not a state (a country-scoped
//! matter has no pooled state account), or when that state has no mirrored
//! account yet. Callers count those skips; nothing infers a trust account
//! from the client entity or the owning firm.
//!
//! # What this module does not hold
//!
//! The per-matter side of the three-way reconciliation. That stays in
//! [`crate::trust`], folded from the journal on read, so the master balance
//! and the per-matter balances can never drift apart through a stale column
//! here. [`IoltaAccount::balance_cents`] is only what Xero last reported for
//! the bank account itself.

use chrono::{DateTime, Utc};
use serde::Serialize;
use surrealdb::types::{RecordId, SurrealValue};
use thiserror::Error;
use uuid::Uuid;

use crate::jurisdictions::JURISDICTION_TYPE_STATE;
use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

/// The prefix a Xero bank account's name carries to declare which state's
/// IOLTA pool it is — the account-side twin of the `Matter <code>` reference
/// invoices use ([`crate::xero_invoices::project_scope_from_reference`]).
///
/// The firm names the account `IOLTA NV — Trust` in Xero and Navigator reads
/// the state code out of it. Xero has no field for "which US state's client
/// funds pool is this", so the mapping has to be declared somewhere; a
/// convention in the account name keeps it in the bookkeeper's own tool
/// rather than in a Navigator-only table an operator would have to remember
/// to edit.
pub const IOLTA_ACCOUNT_NAME_PREFIX: &str = "IOLTA";

/// Read the jurisdiction code out of a Xero bank-account name.
///
/// `"IOLTA NV — Trust"` → `Some("NV")`. `None` for any account that does not
/// open with the prefix and a plausible code — the firm's operating and
/// payroll accounts are listed by the same Xero read, and an account that
/// does not declare a state is skipped and counted, never attached to one.
#[must_use]
pub fn jurisdiction_scope_from_account_name(name: &str) -> Option<String> {
    let rest = name
        .trim()
        .strip_prefix(IOLTA_ACCOUNT_NAME_PREFIX)
        .or_else(|| {
            name.trim()
                .strip_prefix(&IOLTA_ACCOUNT_NAME_PREFIX.to_lowercase())
        })?;
    let code = rest
        .trim_start()
        .split(|c: char| c.is_whitespace() || c == '-')
        .next()?
        .trim();
    let plausible =
        !code.is_empty() && code.len() <= 3 && code.chars().all(|c| c.is_ascii_alphabetic());
    plausible.then(|| code.to_ascii_uppercase())
}

/// Resolve a Xero bank-account name to the state jurisdiction it declares.
///
/// `Ok(None)` when the name declares no code, the code names no
/// jurisdiction, or that jurisdiction is not a state. The caller counts
/// those as unscoped and writes nothing.
///
/// # Errors
///
/// [`IoltaAccountError::Jurisdictions`] when the lookup itself fails.
pub async fn resolve_jurisdiction_scope(
    db: &SurrealDb,
    account_name: &str,
) -> Result<Option<Uuid>, IoltaAccountError> {
    let Some(code) = jurisdiction_scope_from_account_name(account_name) else {
        return Ok(None);
    };
    let Some(jurisdiction) = crate::jurisdictions::find_by_code(db, &code).await? else {
        return Ok(None);
    };
    if jurisdiction.jurisdiction_type != JURISDICTION_TYPE_STATE {
        return Ok(None);
    }
    Ok(Some(jurisdiction.id))
}

/// The SurrealDB table holding the pooled-account mirror.
pub(crate) const TABLE: &str = "iolta_account";
const SELECT: &str = "jurisdiction_id, xero_account_id, xero_account_code, name, currency, \
                      balance_cents, mirrored_at, inserted_at, updated_at";

/// One state's pooled IOLTA account, as Xero last reported it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct IoltaAccount {
    /// The US state this pooled account serves. One account per state.
    pub jurisdiction_id: Uuid,
    /// Xero `AccountID` (a GUID), and this row's own record id.
    pub xero_account_id: String,
    /// Xero's short chart-of-accounts code (`"090"`), when the account has
    /// one. Display only — [`Self::xero_account_id`] is the identity.
    pub xero_account_code: Option<String>,
    /// The account name as Xero holds it (`"IOLTA Trust — Nevada"`).
    pub name: String,
    /// ISO currency of the account, as Xero reports it.
    pub currency: String,
    /// The bank balance Xero last reported, in minor units. This is the
    /// bank-statement leg; the per-matter leg is folded from
    /// [`crate::trust`].
    pub balance_cents: i64,
    /// When the nightly mirror last read this account. `None` on a row
    /// created before its first successful read.
    pub mirrored_at: Option<DateTime<Utc>>,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(serde::Deserialize, SurrealValue)]
struct IoltaAccountRow {
    jurisdiction_id: RecordId,
    xero_account_id: String,
    xero_account_code: Option<String>,
    name: String,
    currency: String,
    balance_cents: i64,
    mirrored_at: Option<surrealdb::types::Datetime>,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl IoltaAccountRow {
    fn into_account(self) -> Option<IoltaAccount> {
        Some(IoltaAccount {
            jurisdiction_id: record_uuid(&self.jurisdiction_id)?,
            xero_account_id: self.xero_account_id,
            xero_account_code: self.xero_account_code,
            name: self.name,
            currency: self.currency,
            balance_cents: self.balance_cents,
            mirrored_at: self.mirrored_at.map(Into::into),
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

/// Errors reading or writing the pooled-account mirror.
#[derive(Debug, Error)]
pub enum IoltaAccountError {
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    #[error(transparent)]
    Jurisdictions(#[from] crate::jurisdictions::JurisdictionError),
    #[error(transparent)]
    Projects(#[from] crate::projects::ProjectStoreError),
    /// The named jurisdiction does not exist.
    #[error("no jurisdiction {0}")]
    NoSuchJurisdiction(Uuid),
    /// IOLTA pools by **state**. A country (or any other jurisdiction type)
    /// has no pooled trust account, so attaching one is refused rather than
    /// mapped onto some nearby state.
    #[error("jurisdiction {code} is a {jurisdiction_type}, and IOLTA pools by state")]
    NotAState {
        code: String,
        jurisdiction_type: String,
    },
    /// Another Xero account is already mirrored for this state. One pooled
    /// account per state is the whole point of the model, so this is
    /// reported with both ids rather than silently replacing the incumbent.
    #[error("jurisdiction {jurisdiction_id} already mirrors Xero account {existing}")]
    JurisdictionTaken {
        jurisdiction_id: Uuid,
        existing: String,
    },
    #[error("writing an IOLTA account mirror returned no usable row")]
    WriteReturnedNothing,
}

/// Run a write under the crate's one retry policy. Unlike
/// [`crate::xero_invoices`] there is no create-race to classify: the mirror
/// writes with `UPSERT` onto the Xero account id, so a racing second read of
/// the same account converges on the one row instead of colliding.
async fn writing<F, Q>(attempt: F) -> Result<surrealdb::IndexedResults, IoltaAccountError>
where
    F: FnMut() -> Q,
    Q: std::future::IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retry::writing(attempt).await.map_err(IoltaAccountError::Db)
}

fn one(mut response: surrealdb::IndexedResults) -> Result<Option<IoltaAccount>, IoltaAccountError> {
    let row: Option<IoltaAccountRow> = response.take(0)?;
    Ok(row.and_then(IoltaAccountRow::into_account))
}

/// What the nightly mirror captures for one pooled account.
#[derive(Clone, Debug)]
pub struct UpsertIoltaAccount {
    /// The state this pooled account serves.
    pub jurisdiction_id: Uuid,
    /// Xero `AccountID`.
    pub xero_account_id: String,
    pub xero_account_code: Option<String>,
    pub name: String,
    pub currency: String,
    pub balance_cents: i64,
    /// When this read happened.
    pub mirrored_at: DateTime<Utc>,
}

/// Mirror one pooled account, creating the row or refreshing the one this
/// Xero account already has.
///
/// Refuses a jurisdiction that does not exist or is not a state, and refuses
/// to attach a **second** Xero account to a state that already mirrors a
/// different one — both as typed errors a caller reports, never as a silent
/// second master balance.
///
/// # Errors
///
/// [`IoltaAccountError::NoSuchJurisdiction`], [`IoltaAccountError::NotAState`],
/// [`IoltaAccountError::JurisdictionTaken`], or [`IoltaAccountError::Db`].
pub async fn upsert(
    db: &SurrealDb,
    input: &UpsertIoltaAccount,
) -> Result<IoltaAccount, IoltaAccountError> {
    let jurisdiction = crate::jurisdictions::find_by_id(db, input.jurisdiction_id)
        .await?
        .ok_or(IoltaAccountError::NoSuchJurisdiction(input.jurisdiction_id))?;
    if jurisdiction.jurisdiction_type != JURISDICTION_TYPE_STATE {
        return Err(IoltaAccountError::NotAState {
            code: jurisdiction.code,
            jurisdiction_type: jurisdiction.jurisdiction_type,
        });
    }
    if let Some(existing) = for_jurisdiction(db, input.jurisdiction_id).await? {
        if existing.xero_account_id != input.xero_account_id {
            return Err(IoltaAccountError::JurisdictionTaken {
                jurisdiction_id: input.jurisdiction_id,
                existing: existing.xero_account_id,
            });
        }
    }

    let response = writing(|| {
        db.query(format!(
            "UPSERT $id SET jurisdiction_id = $jurisdiction_id, \
             xero_account_id = $xero_account_id, xero_account_code = $xero_account_code, \
             name = $name, currency = $currency, balance_cents = $balance_cents, \
             mirrored_at = $mirrored_at, \
             inserted_at = IF inserted_at THEN inserted_at ELSE time::now() END, \
             updated_at = time::now() RETURN {SELECT}"
        ))
        .bind(("id", RecordId::new(TABLE, input.xero_account_id.clone())))
        .bind((
            "jurisdiction_id",
            record_id(crate::jurisdictions::TABLE, input.jurisdiction_id),
        ))
        .bind(("xero_account_id", input.xero_account_id.clone()))
        .bind(("xero_account_code", input.xero_account_code.clone()))
        .bind(("name", input.name.clone()))
        .bind(("currency", input.currency.clone()))
        .bind(("balance_cents", input.balance_cents))
        .bind((
            "mirrored_at",
            surrealdb::types::Datetime::from(input.mirrored_at),
        ))
    })
    .await?;
    one(response)?.ok_or(IoltaAccountError::WriteReturnedNothing)
}

/// The pooled account mirrored for one state, if any.
///
/// # Errors
///
/// [`IoltaAccountError::Db`] when the lookup fails.
pub async fn for_jurisdiction(
    db: &SurrealDb,
    jurisdiction_id: Uuid,
) -> Result<Option<IoltaAccount>, IoltaAccountError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM ONLY {TABLE} WHERE jurisdiction_id = $jurisdiction_id LIMIT 1"
        ))
        .bind((
            "jurisdiction_id",
            record_id(crate::jurisdictions::TABLE, jurisdiction_id),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// The pooled account a matter's client funds belong in, resolved from the
/// matter's **general** governing jurisdiction.
///
/// `Ok(None)` — never a guess — when the Project does not exist, carries no
/// jurisdiction, is governed by something other than a state, or names a
/// state with no mirrored account. Callers count those skips.
///
/// # Errors
///
/// [`IoltaAccountError::Db`] or a failure reading the Project.
pub async fn for_project(
    db: &SurrealDb,
    project_id: Uuid,
) -> Result<Option<IoltaAccount>, IoltaAccountError> {
    let Some(project) = crate::projects::find_by_id(db, project_id).await? else {
        return Ok(None);
    };
    let Some(jurisdiction_id) = project.jurisdiction_id else {
        return Ok(None);
    };
    let Some(jurisdiction) = crate::jurisdictions::find_by_id(db, jurisdiction_id).await? else {
        return Ok(None);
    };
    if jurisdiction.jurisdiction_type != JURISDICTION_TYPE_STATE {
        return Ok(None);
    }
    for_jurisdiction(db, jurisdiction_id).await
}

/// Every mirrored pooled account, oldest first — the firm-side list.
///
/// # Errors
///
/// [`IoltaAccountError::Db`] when the query fails.
pub async fn all(db: &SurrealDb) -> Result<Vec<IoltaAccount>, IoltaAccountError> {
    let mut response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} ORDER BY inserted_at ASC"
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<IoltaAccountRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(IoltaAccountRow::into_account)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{
        all, for_jurisdiction, for_project, jurisdiction_scope_from_account_name,
        resolve_jurisdiction_scope, upsert, IoltaAccountError, UpsertIoltaAccount,
    };
    use crate::jurisdictions::NewJurisdiction;
    use crate::surreal::test_support::mem;
    use crate::surreal::SurrealDb;
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    async fn state(db: &SurrealDb, name: &str, code: &str) -> Uuid {
        crate::jurisdictions::create(db, &NewJurisdiction::new(name, code, "state"))
            .await
            .unwrap()
            .id
    }

    async fn project_in(db: &SurrealDb, code: &str, jurisdiction_id: Option<Uuid>) -> Uuid {
        let entity_id = crate::test_support::seed_entity(db).await;
        crate::projects::create(
            db,
            &crate::projects::NewProject {
                code: code.to_string(),
                name: code.to_string(),
                status: "open".to_string(),
                entity_id,
                jurisdiction_id,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .id
    }

    fn mirror(jurisdiction_id: Uuid, xero_account_id: &str, name: &str) -> UpsertIoltaAccount {
        UpsertIoltaAccount {
            jurisdiction_id,
            xero_account_id: xero_account_id.to_string(),
            xero_account_code: Some("090".to_string()),
            name: name.to_string(),
            currency: "USD".to_string(),
            balance_cents: 500_000,
            mirrored_at: Utc.with_ymd_and_hms(2026, 9, 19, 3, 0, 0).unwrap(),
        }
    }

    /// The issue's first acceptance criterion: two states are two rows, each
    /// with its own Xero account.
    #[tokio::test]
    async fn two_states_carry_two_pooled_accounts() {
        let db = mem().await;
        let nevada = state(&db, "Nevada", "NV").await;
        let california = state(&db, "California", "CA").await;

        upsert(&db, &mirror(nevada, "xero-nv", "IOLTA NV — Trust"))
            .await
            .unwrap();
        upsert(&db, &mirror(california, "xero-ca", "IOLTA CA — Trust"))
            .await
            .unwrap();

        let nv = for_jurisdiction(&db, nevada).await.unwrap().unwrap();
        let ca = for_jurisdiction(&db, california).await.unwrap().unwrap();
        assert_eq!(nv.xero_account_id, "xero-nv");
        assert_eq!(ca.xero_account_id, "xero-ca");
        assert_eq!(all(&db).await.unwrap().len(), 2);
    }

    /// A re-run of the nightly mirror refreshes the one row rather than
    /// adding a second master balance for the same state.
    #[tokio::test]
    async fn re_mirroring_the_same_account_refreshes_one_row() {
        let db = mem().await;
        let nevada = state(&db, "Nevada", "NV").await;
        upsert(&db, &mirror(nevada, "xero-nv", "IOLTA NV — Trust"))
            .await
            .unwrap();

        let mut refreshed = mirror(nevada, "xero-nv", "IOLTA NV — Trust");
        refreshed.balance_cents = 750_000;
        upsert(&db, &refreshed).await.unwrap();

        let accounts = all(&db).await.unwrap();
        assert_eq!(accounts.len(), 1, "one state, one pooled account");
        assert_eq!(accounts[0].balance_cents, 750_000);
    }

    /// One pooled account per state: a *different* Xero account claiming a
    /// state that already has one is refused, naming the incumbent, rather
    /// than quietly replacing it.
    #[tokio::test]
    async fn a_second_xero_account_for_one_state_is_refused() {
        let db = mem().await;
        let nevada = state(&db, "Nevada", "NV").await;
        upsert(&db, &mirror(nevada, "xero-nv", "IOLTA NV — Trust"))
            .await
            .unwrap();

        let second = upsert(&db, &mirror(nevada, "xero-nv-2", "IOLTA NV — Second")).await;
        assert!(
            matches!(
                second,
                Err(IoltaAccountError::JurisdictionTaken { ref existing, .. })
                    if existing == "xero-nv"
            ),
            "unexpected upsert result shape"
        );
        assert_eq!(all(&db).await.unwrap().len(), 1);
    }

    /// IOLTA pools by state. A country jurisdiction has no pooled trust
    /// account, so attaching one is refused rather than mapped onto a state.
    #[tokio::test]
    async fn a_country_jurisdiction_cannot_hold_a_pooled_account() {
        let db = mem().await;
        let us = crate::jurisdictions::create(
            &db,
            &NewJurisdiction::new("United States", "US", "country"),
        )
        .await
        .unwrap()
        .id;

        let attached = upsert(&db, &mirror(us, "xero-us", "IOLTA US — Trust")).await;
        assert!(
            matches!(attached, Err(IoltaAccountError::NotAState { .. })),
            "got {attached:?}"
        );
    }

    #[tokio::test]
    async fn an_unknown_jurisdiction_is_refused() {
        let db = mem().await;
        let attached = upsert(&db, &mirror(Uuid::now_v7(), "xero-x", "IOLTA XX")).await;
        assert!(
            matches!(attached, Err(IoltaAccountError::NoSuchJurisdiction(_))),
            "got {attached:?}"
        );
    }

    /// A matter reaches its pooled account through its *general* governing
    /// jurisdiction — and a matter that names none is answered `None`, never
    /// attached to a nearby state.
    #[tokio::test]
    async fn a_matter_resolves_its_account_through_its_jurisdiction() {
        let db = mem().await;
        let nevada = state(&db, "Nevada", "NV").await;
        let california = state(&db, "California", "CA").await;
        upsert(&db, &mirror(nevada, "xero-nv", "IOLTA NV — Trust"))
            .await
            .unwrap();

        let nv_matter = project_in(&db, "nv-matter", Some(nevada)).await;
        let ca_matter = project_in(&db, "ca-matter", Some(california)).await;
        let unscoped_matter = project_in(&db, "no-jurisdiction", None).await;

        assert_eq!(
            for_project(&db, nv_matter)
                .await
                .unwrap()
                .map(|a| a.xero_account_id),
            Some("xero-nv".to_string())
        );
        assert_eq!(
            for_project(&db, ca_matter).await.unwrap(),
            None,
            "California has no mirrored account yet — nothing is invented"
        );
        assert_eq!(
            for_project(&db, unscoped_matter).await.unwrap(),
            None,
            "a matter with no jurisdiction has no pooled account"
        );
        assert_eq!(for_project(&db, Uuid::now_v7()).await.unwrap(), None);
    }

    /// A matter governed by a country — a federal matter — has no pooled
    /// state account, and is skipped rather than guessed at.
    #[tokio::test]
    async fn a_country_scoped_matter_has_no_pooled_account() {
        let db = mem().await;
        let us = crate::jurisdictions::create(
            &db,
            &NewJurisdiction::new("United States", "US", "country"),
        )
        .await
        .unwrap()
        .id;
        let federal_matter = project_in(&db, "federal", Some(us)).await;

        assert_eq!(for_project(&db, federal_matter).await.unwrap(), None);
    }

    #[test]
    fn the_account_name_convention_reads_the_state_code() {
        assert_eq!(
            jurisdiction_scope_from_account_name("IOLTA NV — Trust"),
            Some("NV".to_string())
        );
        assert_eq!(
            jurisdiction_scope_from_account_name("  IOLTA ca  "),
            Some("CA".to_string())
        );
        assert_eq!(
            jurisdiction_scope_from_account_name("IOLTA NV-Pooled"),
            Some("NV".to_string())
        );
        // The firm's other accounts are listed by the same Xero read and
        // must not be mistaken for a trust pool.
        assert_eq!(
            jurisdiction_scope_from_account_name("Business Checking"),
            None
        );
        assert_eq!(jurisdiction_scope_from_account_name("Payroll"), None);
        assert_eq!(jurisdiction_scope_from_account_name("IOLTA"), None);
        assert_eq!(
            jurisdiction_scope_from_account_name("IOLTA Nevada Pooled Trust"),
            None,
            "a spelled-out state is not a code; the convention is the code"
        );
    }

    #[tokio::test]
    async fn an_account_naming_an_unknown_or_non_state_code_is_unscoped() {
        let db = mem().await;
        state(&db, "Nevada", "NV").await;
        crate::jurisdictions::create(&db, &NewJurisdiction::new("United States", "US", "country"))
            .await
            .unwrap();

        assert!(resolve_jurisdiction_scope(&db, "IOLTA NV — Trust")
            .await
            .unwrap()
            .is_some());
        assert_eq!(
            resolve_jurisdiction_scope(&db, "IOLTA ZZ — Trust")
                .await
                .unwrap(),
            None,
            "a code no jurisdiction carries is unscoped"
        );
        assert_eq!(
            resolve_jurisdiction_scope(&db, "IOLTA US — Trust")
                .await
                .unwrap(),
            None,
            "a country code names no pooled state account"
        );
        assert_eq!(
            resolve_jurisdiction_scope(&db, "Business Checking")
                .await
                .unwrap(),
            None
        );
    }
}
