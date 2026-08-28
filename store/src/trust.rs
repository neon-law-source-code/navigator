//! Trust-ledger movements — the immutable, per-engagement record of client
//! funds held in trust, earned as work is performed, and refunded when a
//! matter closes early.
//!
//! # Why this exists
//!
//! Every retainer bills a **prorated first month at signing**, held in the
//! client trust account and *earned as the work is performed* (NRPC 1.5 /
//! Cal. Rule 1.15 — advance fees are **not** earned on receipt). Whatever is
//! unearned when the engagement ends is **refunded**. That earned/unearned
//! split, the proration formula, and the refund are a bar-rules obligation no
//! bank or payment processor computes for us — so they live here, as pure,
//! provider- and asset-agnostic logic.
//!
//! # What this is NOT (the deferred banking seam)
//!
//! Navigator does **not** custody funds. A real banking/trust-ledger provider
//! (Modern Treasury / Increase / Column) will own USD settlement and the
//! bank-statement leg of the IOLTA three-way reconciliation; on-chain rails
//! own crypto settlement. This module owns only the *legal-meaning overlay*:
//! which engagement a movement belongs to, what it means (deposit / earned
//! draw / refund), and the running trust position per matter.
//!
//! Because the provider — and whether a client pays in USD, USDC, or BTC — is
//! not chosen yet, we commit to **no financial schema**. Each movement is
//! recorded as an immutable JSON event on the existing append-only
//! [`notation_events`](crate::notation_events) journal (append-only by the
//! module's own shape — see its header — which makes it tamper-evident) under a
//! [`MACHINE_TRUST_LEDGER`] machine-kind, anchored to the engagement's
//! retainer notation (which already carries the `project_id` / `person_id` /
//! `entity_id` links). When a provider lands, its posting id / on-chain tx
//! hash mirrors into [`Movement::external_ref`] and these postings replay onto
//! the provider's ledger — no migration to unwind.
//!
//! # Double-entry
//!
//! Each movement is a double-entry posting: value flows from one account to
//! another, mirrored onto the journal row's `from_state` → `to_state`. The
//! accounts are:
//!
//! - [`client_account`]`(project)` — the client's own funds (external).
//! - [`trust_account`]`(project)` — this matter's individual client trust
//!   ledger (the per-matter balance IOLTA reconciliation requires).
//! - [`OPERATING_ACCOUNT`] — the firm's earned revenue.
//!
//! A **deposit** moves client → trust; an **earned draw** moves trust →
//! operating; a **refund** moves trust → client. The trust balance for a
//! matter is everything that flowed into `trust:<project>` minus everything
//! that flowed out — and, since earned money is drawn out, whatever remains in
//! trust is by definition still *unearned* and refundable.
//!
//! # Assets (USD and crypto)
//!
//! [`Movement::asset`] + [`Movement::amount`] record the asset actually moved
//! (`"USD"`, `"USDC"`, `"BTC"`) as a decimal *string* — never a float, and
//! never assuming USD cents (a wei-denominated ETH amount overflows `i64`).
//! Note the legal frame differs by asset: pooled *USD* held for clients is an
//! IOLTA concern, whereas client *crypto* is **safekeeping of client property**
//! (RPC 1.15's property branch), tracked in kind. The fee obligation itself is
//! denominated in USD, so trust accounting runs on the common denominator
//! [`Movement::usd_value_cents`] — the USD value credited at receipt.

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use String;

use crate::notation_events::{append_event, TransitionRecord};
use crate::surreal::SurrealDb;

/// Machine-kind discriminator for trust-ledger events on the
/// `notation_events` journal. Distinct from `questionnaire` / `workflow`, so a
/// matter's trust postings never mix with its state-machine transitions.
pub const MACHINE_TRUST_LEDGER: &str = "trust_ledger";

/// The firm's earned-revenue account — the destination of an earned draw.
/// A single logical account today; a provider maps it to the operating bank
/// account later.
pub const OPERATING_ACCOUNT: &str = "operating";

/// The client-funds account for a matter (source of a deposit, destination of
/// a refund).
#[must_use]
pub fn client_account(project_id: Uuid) -> String {
    format!("client:{project_id}")
}

/// This matter's individual client trust ledger — the per-matter balance the
/// IOLTA three-way reconciliation is built on.
#[must_use]
pub fn trust_account(project_id: Uuid) -> String {
    format!("trust:{project_id}")
}

/// What a [`Movement`] means. Serialized snake_case into the journal payload
/// and mirrored onto the row's `condition`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MovementKind {
    /// Client funds arriving into trust (the prorated first month at signing,
    /// or a later replenishment).
    Deposit,
    /// Earned fee moving out of trust to the firm's operating account.
    EarnedDraw,
    /// Unearned funds returned to the client (early close).
    Refund,
}

impl MovementKind {
    /// The stable snake_case token stored on the journal row's `condition`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            MovementKind::Deposit => "deposit",
            MovementKind::EarnedDraw => "earned_draw",
            MovementKind::Refund => "refund",
        }
    }
}

/// Which rail settled a movement. A free-form token (`"manual"`, `"bank"`,
/// `"crypto"`) rather than an enum, so a new provider or chain is additive.
pub const RAIL_MANUAL: &str = "manual";

/// One immutable trust-ledger posting. The full record lives in the journal
/// row's JSON `payload`; the double-entry legs and kind are mirrored onto the
/// row's `from_state` / `to_state` / `condition` for at-a-glance reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Movement {
    /// What this posting means.
    pub kind: MovementKind,
    /// Debit (source) account — where value flows *from*.
    pub from_account: String,
    /// Credit (destination) account — where value flows *to*.
    pub to_account: String,
    /// The asset actually moved: `"USD"`, `"USDC"`, `"BTC"`, `"ETH"`.
    pub asset: String,
    /// Native amount as a decimal string — never a float, never assumed to be
    /// USD cents (`"230.00"`, `"0.05"`).
    pub amount: String,
    /// USD value credited to the engagement's fee obligation, in cents. For a
    /// USD movement this equals the dollar amount; for crypto it is the
    /// USD-equivalent recorded at receipt. Trust accounting runs on this.
    pub usd_value_cents: i64,
    /// Which rail settled it (see [`RAIL_MANUAL`]) — the provider seam.
    pub rail: String,
    /// External settlement reference (bank posting id / on-chain tx hash),
    /// filled once a provider or chain rail is wired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_ref: Option<String>,
    /// The matter phase this fee belongs to, when the monthly fee flexes by
    /// phase (litigation: `"pleadings"`, `"discovery"`, `"trial"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// RFC 3339 receipt/effective time.
    pub occurred_at: String,
    /// Optional human memo. Must not carry client content beyond what the
    /// journal already holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum TrustLedgerError {
    #[error("{kind:?} movement has a negative USD value: {usd_value_cents}")]
    NegativeUsdValue {
        kind: MovementKind,
        usd_value_cents: i64,
    },
    #[error("{kind:?} movement has an invalid native asset amount: {amount}")]
    InvalidAssetAmount { kind: MovementKind, amount: String },
    #[error(
        "{kind:?} movement has invalid account legs: expected {expected_from} -> {expected_to}, got {actual_from} -> {actual_to}"
    )]
    InvalidLegs {
        kind: MovementKind,
        expected_from: String,
        expected_to: String,
        actual_from: String,
        actual_to: String,
    },
    #[error(
        "trust movements mix projects: expected {expected_project_id}, got {actual_project_id}"
    )]
    MixedProjects {
        expected_project_id: Uuid,
        actual_project_id: Uuid,
    },
    #[error(
        "trust event {event_id} journal mirrors do not match payload: expected {expected_from} -> {expected_to} / {expected_condition}, got {actual_from} -> {actual_to} / {actual_condition}"
    )]
    JournalMirrorMismatch {
        event_id: Uuid,
        expected_from: String,
        expected_to: String,
        expected_condition: &'static str,
        actual_from: String,
        actual_to: String,
        actual_condition: String,
    },
}

pub type TrustLedgerResult<T> = Result<T, Box<TrustLedgerError>>;

impl Movement {
    /// A deposit of client funds into this matter's trust account.
    #[must_use]
    pub fn deposit(
        project_id: Uuid,
        asset: impl Into<String>,
        amount: impl Into<String>,
        usd_value_cents: i64,
        occurred_at: impl Into<String>,
    ) -> Self {
        Movement {
            kind: MovementKind::Deposit,
            from_account: client_account(project_id),
            to_account: trust_account(project_id),
            asset: asset.into(),
            amount: amount.into(),
            usd_value_cents,
            rail: RAIL_MANUAL.to_string(),
            external_ref: None,
            phase: None,
            occurred_at: occurred_at.into(),
            memo: None,
        }
    }

    /// An earned draw of `usd_value_cents` from trust to the firm's operating
    /// account, as work is performed. Denominated in USD by nature.
    #[must_use]
    pub fn earned_draw(
        project_id: Uuid,
        usd_value_cents: i64,
        occurred_at: impl Into<String>,
    ) -> Self {
        Movement {
            kind: MovementKind::EarnedDraw,
            from_account: trust_account(project_id),
            to_account: OPERATING_ACCOUNT.to_string(),
            asset: "USD".to_string(),
            amount: usd_string(usd_value_cents),
            usd_value_cents,
            rail: RAIL_MANUAL.to_string(),
            external_ref: None,
            phase: None,
            occurred_at: occurred_at.into(),
            memo: None,
        }
    }

    /// A refund of `usd_value_cents` of unearned funds from trust back to the
    /// client at early close.
    #[must_use]
    pub fn refund(project_id: Uuid, usd_value_cents: i64, occurred_at: impl Into<String>) -> Self {
        Movement {
            kind: MovementKind::Refund,
            from_account: trust_account(project_id),
            to_account: client_account(project_id),
            asset: "USD".to_string(),
            amount: usd_string(usd_value_cents),
            usd_value_cents,
            rail: RAIL_MANUAL.to_string(),
            external_ref: None,
            phase: None,
            occurred_at: occurred_at.into(),
            memo: None,
        }
    }

    /// Tag this movement with the matter phase whose fee it settles.
    #[must_use]
    pub fn with_phase(mut self, phase: impl Into<String>) -> Self {
        self.phase = Some(phase.into());
        self
    }

    /// Attach the provider/chain settlement reference (bank posting id / tx
    /// hash).
    #[must_use]
    pub fn with_external_ref(mut self, external_ref: impl Into<String>) -> Self {
        self.external_ref = Some(external_ref.into());
        self
    }

    /// Attach a human memo.
    #[must_use]
    pub fn with_memo(mut self, memo: impl Into<String>) -> Self {
        self.memo = Some(memo.into());
        self
    }

    /// Validate that this movement is a canonical posting for `project_id`.
    ///
    /// Trust ledger entries are append-only; reject impossible signs and account
    /// legs before they become durable facts.
    ///
    /// # Errors
    ///
    /// Returns a [`TrustLedgerError`] when the value is negative, the native
    /// amount is empty/negative, or the double-entry legs do not match the
    /// movement kind for the supplied project.
    pub fn validate_for_project(&self, project_id: Uuid) -> TrustLedgerResult<()> {
        validate_non_negative_values(self)?;
        let (expected_from, expected_to) = expected_legs(self.kind, project_id);
        if self.from_account == expected_from && self.to_account == expected_to {
            return Ok(());
        }
        Err(Box::new(TrustLedgerError::InvalidLegs {
            kind: self.kind,
            expected_from,
            expected_to,
            actual_from: self.from_account.clone(),
            actual_to: self.to_account.clone(),
        }))
    }
}

fn validate_non_negative_values(movement: &Movement) -> TrustLedgerResult<()> {
    if movement.usd_value_cents < 0 {
        return Err(Box::new(TrustLedgerError::NegativeUsdValue {
            kind: movement.kind,
            usd_value_cents: movement.usd_value_cents,
        }));
    }
    let amount = movement.amount.trim();
    if amount.is_empty() || amount.starts_with('-') {
        return Err(Box::new(TrustLedgerError::InvalidAssetAmount {
            kind: movement.kind,
            amount: movement.amount.clone(),
        }));
    }
    Ok(())
}

fn expected_legs(kind: MovementKind, project_id: Uuid) -> (String, String) {
    match kind {
        MovementKind::Deposit => (client_account(project_id), trust_account(project_id)),
        MovementKind::EarnedDraw => (trust_account(project_id), OPERATING_ACCOUNT.to_string()),
        MovementKind::Refund => (trust_account(project_id), client_account(project_id)),
    }
}

fn parse_account_project(account: &str, prefix: &str) -> Option<Uuid> {
    account
        .strip_prefix(prefix)
        .and_then(|id| Uuid::parse_str(id).ok())
}

fn movement_project_id(movement: &Movement) -> TrustLedgerResult<Uuid> {
    validate_non_negative_values(movement)?;
    let project_id = match movement.kind {
        MovementKind::Deposit => {
            let Some(client_project) = parse_account_project(&movement.from_account, "client:")
            else {
                return Err(Box::new(invalid_self_contained_legs(movement)));
            };
            let Some(trust_project) = parse_account_project(&movement.to_account, "trust:") else {
                return Err(Box::new(invalid_self_contained_legs(movement)));
            };
            if client_project != trust_project {
                return Err(Box::new(TrustLedgerError::MixedProjects {
                    expected_project_id: client_project,
                    actual_project_id: trust_project,
                }));
            }
            client_project
        }
        MovementKind::EarnedDraw => {
            if movement.to_account != OPERATING_ACCOUNT {
                return Err(Box::new(invalid_self_contained_legs(movement)));
            }
            let Some(trust_project) = parse_account_project(&movement.from_account, "trust:")
            else {
                return Err(Box::new(invalid_self_contained_legs(movement)));
            };
            trust_project
        }
        MovementKind::Refund => {
            let Some(trust_project) = parse_account_project(&movement.from_account, "trust:")
            else {
                return Err(Box::new(invalid_self_contained_legs(movement)));
            };
            let Some(client_project) = parse_account_project(&movement.to_account, "client:")
            else {
                return Err(Box::new(invalid_self_contained_legs(movement)));
            };
            if trust_project != client_project {
                return Err(Box::new(TrustLedgerError::MixedProjects {
                    expected_project_id: trust_project,
                    actual_project_id: client_project,
                }));
            }
            trust_project
        }
    };
    movement.validate_for_project(project_id)?;
    Ok(project_id)
}

fn invalid_self_contained_legs(movement: &Movement) -> TrustLedgerError {
    let marker = "<same project>";
    let (expected_from, expected_to) = match movement.kind {
        MovementKind::Deposit => ("client:<project>", "trust:<project>"),
        MovementKind::EarnedDraw => ("trust:<project>", OPERATING_ACCOUNT),
        MovementKind::Refund => ("trust:<project>", "client:<project>"),
    };
    TrustLedgerError::InvalidLegs {
        kind: movement.kind,
        expected_from: expected_from.replace("<project>", marker),
        expected_to: expected_to.replace("<project>", marker),
        actual_from: movement.from_account.clone(),
        actual_to: movement.to_account.clone(),
    }
}

fn trust_error(error: &TrustLedgerError) -> String {
    format!("invalid trust movement: {error}")
}

/// Render a USD cent amount as a plain decimal string (`230_00` → `"230.00"`).
/// Negative values keep a leading `-`.
#[must_use]
fn usd_string(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let abs = cents.unsigned_abs();
    format!("{sign}{}.{:02}", abs / 100, abs % 100)
}

/// The number of days in the calendar month containing `date`.
#[must_use]
fn days_in_month(date: NaiveDate) -> i64 {
    let (year, month) = (date.year(), date.month());
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let first_of_this = NaiveDate::from_ymd_opt(year, month, 1).expect("valid first-of-month");
    let first_of_next =
        NaiveDate::from_ymd_opt(next_year, next_month, 1).expect("valid first-of-next-month");
    (first_of_next - first_of_this).num_days()
}

/// Prorate the first partial month, due at signing:
/// `monthly_fee_cents × (days_remaining_after_signing ÷ days_in_month)`, where
/// `days_remaining_after_signing = days_in_month − day_of_month` (the balance
/// of the signing month *after* the signing date; the signing day itself is
/// carried by the deposit). Rounds to the nearest cent (half up).
///
/// Signed **July 8** (31-day month) → `(31 − 8) / 31` of the fee. Signed on the
/// last day of the month → `0` (the first full month is billed at the next
/// period boundary). A phase change mid-engagement prorates the *new* phase's
/// monthly fee the same way from its effective date.
///
/// # Panics
///
/// Panics only if the month arithmetic in [`days_in_month`] fails, which cannot
/// happen for a valid `signing` date.
#[must_use]
pub fn prorate_first_month(monthly_fee_cents: i64, signing: NaiveDate) -> i64 {
    let days_in = days_in_month(signing);
    let days_remaining = days_in - i64::from(signing.day());
    if days_remaining <= 0 {
        return 0;
    }
    // i128 so a large fee × up-to-30 days can't overflow; round half up.
    let numerator = i128::from(monthly_fee_cents) * i128::from(days_remaining);
    let denominator = i128::from(days_in);
    let rounded = (numerator * 2 + denominator) / (denominator * 2);
    i64::try_from(rounded).expect("prorated cents fit in i64")
}

/// The running trust position for one engagement, folded from its movements.
/// All amounts are USD cents.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Position {
    /// Total deposited into trust.
    pub deposited_cents: i64,
    /// Total earned and drawn to the firm's operating account.
    pub earned_cents: i64,
    /// Total refunded to the client.
    pub refunded_cents: i64,
}

impl Position {
    /// Funds still held in trust: `deposited − earned − refunded`. Because
    /// earned money has been drawn out, this held balance is exactly the
    /// **unearned** amount.
    #[must_use]
    pub fn held_cents(&self) -> i64 {
        self.deposited_cents - self.earned_cents - self.refunded_cents
    }

    /// The unearned balance — funds paid in advance and not yet earned.
    /// Identical to [`Self::held_cents`]; named for the legal concept.
    #[must_use]
    pub fn unearned_cents(&self) -> i64 {
        self.held_cents()
    }

    /// What must be refunded if the engagement closes now: the entire unearned
    /// (still-held) balance.
    #[must_use]
    pub fn refund_on_close_cents(&self) -> i64 {
        self.held_cents()
    }
}

/// Fold a set of movements into the engagement's [`Position`]. Order does not
/// matter — the position is the sum of the parts.
///
/// # Errors
///
/// Returns a [`TrustLedgerError`] if any movement has negative value, invalid
/// double-entry legs for its kind, or mixes projects with the other movements.
pub fn position(movements: &[Movement]) -> TrustLedgerResult<Position> {
    let mut pos = Position::default();
    let mut project_id = None;
    for movement in movements {
        let movement_project_id = movement_project_id(movement)?;
        if let Some(expected_project_id) = project_id {
            if movement_project_id != expected_project_id {
                return Err(Box::new(TrustLedgerError::MixedProjects {
                    expected_project_id,
                    actual_project_id: movement_project_id,
                }));
            }
        } else {
            project_id = Some(movement_project_id);
        }
        match movement.kind {
            MovementKind::Deposit => pos.deposited_cents += movement.usd_value_cents,
            MovementKind::EarnedDraw => pos.earned_cents += movement.usd_value_cents,
            MovementKind::Refund => pos.refunded_cents += movement.usd_value_cents,
        }
    }
    Ok(pos)
}

/// Append one trust movement to the immutable journal, anchored to the
/// engagement's retainer `notation_id` and attributed to `acting_person_id`.
/// Inherits the append-only trigger and actor FK from `notation_events`.
///
/// # Errors
///
/// Returns a [`String`] if the movement cannot be serialized or the row cannot
/// be inserted (e.g. the notation does not exist).
pub async fn record_movement(
    surreal: &SurrealDb,
    notation_id: Uuid,
    acting_person_id: Uuid,
    movement: &Movement,
) -> Result<crate::notation_events::NotationEvent, String> {
    let notation = crate::notations::find_by_id(surreal, notation_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("notation {notation_id}"))?;
    movement
        .validate_for_project(notation.project_id)
        .map_err(|e| trust_error(&e))?;
    let payload =
        serde_json::to_string(movement).map_err(|e| format!("serialize trust movement: {e}"))?;
    append_event(
        surreal,
        TransitionRecord {
            notation_id,
            acting_person_id: Some(acting_person_id),
            machine_kind: MACHINE_TRUST_LEDGER,
            from_state: &movement.from_account,
            to_state: &movement.to_account,
            condition: movement.kind.as_str(),
            payload_json: Some(payload),
            recorded_at: &movement.occurred_at,
        },
    )
    .await
    .map_err(|error| error.to_string())
}

/// Read every trust movement recorded against a retainer `notation_id`,
/// oldest → newest (by the time-sortable event id).
///
/// # Errors
///
/// Returns a [`String`] on a query failure, or if a `trust_ledger` row carries a
/// missing or unparseable payload — a corruption we surface loudly rather than
/// silently skip.
pub async fn movements_for(
    surreal: &SurrealDb,
    notation_id: Uuid,
) -> Result<Vec<Movement>, String> {
    let notation = crate::notations::find_by_id(surreal, notation_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("notation {notation_id}"))?;
    let rows: Vec<_> = crate::notation_events::for_notation(surreal, notation_id)
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|row| row.machine_kind == MACHINE_TRUST_LEDGER)
        .collect();
    rows.into_iter()
        .map(|row| {
            let payload = row
                .payload
                .ok_or_else(|| format!("trust_ledger event {} has no payload", row.id))?;
            let movement = serde_json::from_str::<Movement>(&payload)
                .map_err(|e| format!("parse trust movement {}: {e}", row.id))?;
            movement
                .validate_for_project(notation.project_id)
                .map_err(|e| trust_error(&e))?;
            if row.from_state != movement.from_account
                || row.to_state != movement.to_account
                || row.condition != movement.kind.as_str()
            {
                let error = TrustLedgerError::JournalMirrorMismatch {
                    event_id: row.id,
                    expected_from: movement.from_account,
                    expected_to: movement.to_account,
                    expected_condition: movement.kind.as_str(),
                    actual_from: row.from_state,
                    actual_to: row.to_state,
                    actual_condition: row.condition,
                };
                return Err(trust_error(&error));
            }
            Ok(movement)
        })
        .collect()
}

/// The current trust [`Position`] for an engagement — [`movements_for`] folded
/// through [`position`].
///
/// # Errors
///
/// Propagates any [`String`] from [`movements_for`].
pub async fn position_for(surreal: &SurrealDb, notation_id: Uuid) -> Result<Position, String> {
    position(&movements_for(surreal, notation_id).await?).map_err(|e| trust_error(&e))
}

#[cfg(test)]
mod tests {
    use super::{
        client_account, days_in_month, position, prorate_first_month, trust_account, usd_string,
        Movement, MovementKind, TrustLedgerError, OPERATING_ACCOUNT,
    };
    use chrono::NaiveDate;
    use uuid::Uuid;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn days_in_month_covers_lengths_and_leap_february() {
        assert_eq!(days_in_month(date(2026, 7, 8)), 31, "July");
        assert_eq!(days_in_month(date(2026, 4, 15)), 30, "April");
        assert_eq!(days_in_month(date(2026, 2, 1)), 28, "Feb non-leap");
        assert_eq!(days_in_month(date(2024, 2, 1)), 29, "Feb leap");
        assert_eq!(
            days_in_month(date(2026, 12, 31)),
            31,
            "December rolls the year"
        );
    }

    /// The exact `(days_remaining / days_in_month)` formula across a table of
    /// signing dates — the issue's July-8 case, month lengths, both February
    /// variants, a mid-range $10–15k litigation fee, and the boundaries.
    #[test]
    fn prorate_first_month_matches_the_formula() {
        // Issue's worked example: July 8, $2,000/mo → (31-8)/31 × 200_000.
        // 200_000 × 23 / 31 = 148_387.09… → 148_387 (half up).
        assert_eq!(prorate_first_month(200_000, date(2026, 7, 8)), 148_387);

        // A $15,000/mo litigation fee signed July 8: 1_500_000 × 23 / 31.
        // = 1_112_903.2… → 1_112_903.
        assert_eq!(prorate_first_month(1_500_000, date(2026, 7, 8)), 1_112_903);

        // Signed the 1st → nearly the whole month: (31-1)/31 × fee.
        // 1_000_000 × 30 / 31 = 967_741.9… → 967_742.
        assert_eq!(prorate_first_month(1_000_000, date(2026, 7, 1)), 967_742);

        // Signed the last day → zero prorated (first full month bills next
        // period boundary). July 31 and Feb 28 (non-leap) both → 0.
        assert_eq!(prorate_first_month(1_000_000, date(2026, 7, 31)), 0);
        assert_eq!(prorate_first_month(1_000_000, date(2026, 2, 28)), 0);

        // February proration differs by leap year for the same day-of-month.
        // Feb 14 non-leap: 280_000 × (28-14)/28 = 140_000 exactly.
        assert_eq!(prorate_first_month(280_000, date(2026, 2, 14)), 140_000);
        // Feb 14 leap: 280_000 × (29-14)/29 = 144_827.5… → 144_828.
        assert_eq!(prorate_first_month(280_000, date(2024, 2, 14)), 144_828);

        // A zero fee prorates to zero on any date.
        assert_eq!(prorate_first_month(0, date(2026, 7, 8)), 0);
    }

    #[test]
    fn usd_string_renders_cents_as_plain_decimal() {
        assert_eq!(usd_string(23_000), "230.00");
        assert_eq!(usd_string(5), "0.05");
        assert_eq!(usd_string(0), "0.00");
        assert_eq!(usd_string(1_500_000), "15000.00");
        assert_eq!(usd_string(-450), "-4.50");
    }

    #[test]
    fn movement_constructors_set_the_double_entry_legs() {
        let project = Uuid::now_v7();

        let deposit = Movement::deposit(project, "USD", "1483.87", 148_387, "2026-07-08T00:00:00Z");
        assert_eq!(deposit.kind, MovementKind::Deposit);
        assert_eq!(deposit.from_account, client_account(project));
        assert_eq!(deposit.to_account, trust_account(project));

        let earned = Movement::earned_draw(project, 148_387, "2026-08-01T00:00:00Z");
        assert_eq!(earned.from_account, trust_account(project));
        assert_eq!(earned.to_account, OPERATING_ACCOUNT);
        assert_eq!(earned.amount, "1483.87");

        let refund = Movement::refund(project, 50_000, "2026-09-15T00:00:00Z");
        assert_eq!(refund.from_account, trust_account(project));
        assert_eq!(refund.to_account, client_account(project));
    }

    /// Held == unearned == refund-on-close, folded from a full lifecycle:
    /// a prorated deposit, a phase-two deposit, one earned draw, and a
    /// partial refund — every movement kind in one position.
    #[test]
    fn position_folds_deposits_earns_and_refunds() {
        let project = Uuid::now_v7();
        let movements = vec![
            Movement::deposit(project, "USD", "1483.87", 148_387, "2026-07-08T00:00:00Z")
                .with_phase("pleadings"),
            Movement::deposit(project, "USD", "2000.00", 200_000, "2026-09-01T00:00:00Z")
                .with_phase("discovery"),
            Movement::earned_draw(project, 148_387, "2026-09-01T00:00:00Z"),
            Movement::refund(project, 50_000, "2026-10-01T00:00:00Z"),
        ];
        let pos = position(&movements).unwrap();
        assert_eq!(pos.deposited_cents, 348_387);
        assert_eq!(pos.earned_cents, 148_387);
        assert_eq!(pos.refunded_cents, 50_000);
        // Still held (and therefore unearned, and therefore the refund owed on
        // an immediate close): 348_387 − 148_387 − 50_000 = 150_000.
        assert_eq!(pos.held_cents(), 150_000);
        assert_eq!(pos.unearned_cents(), 150_000);
        assert_eq!(pos.refund_on_close_cents(), 150_000);
    }

    #[test]
    fn position_of_no_movements_is_zero() {
        let pos = position(&[]).unwrap();
        assert_eq!(pos.held_cents(), 0);
        assert_eq!(pos.unearned_cents(), 0);
        assert_eq!(pos.refund_on_close_cents(), 0);
    }

    #[test]
    fn position_rejects_negative_values() {
        let project = Uuid::now_v7();
        let movements = vec![Movement::deposit(
            project,
            "USD",
            "500.00",
            -50_000,
            "2026-07-08T00:00:00Z",
        )];

        let error = position(&movements).unwrap_err();
        assert!(matches!(*error, TrustLedgerError::NegativeUsdValue { .. }));
    }

    #[test]
    fn position_rejects_kind_and_leg_disagreement() {
        let project = Uuid::now_v7();
        let mut movement =
            Movement::deposit(project, "USD", "500.00", 50_000, "2026-07-08T00:00:00Z");
        movement.from_account = trust_account(project);
        movement.to_account = client_account(project);

        let error = position(&[movement]).unwrap_err();
        assert!(matches!(*error, TrustLedgerError::InvalidLegs { .. }));
    }

    #[test]
    fn position_rejects_mixed_projects() {
        let a = Uuid::now_v7();
        let b = Uuid::now_v7();

        let error = position(&[
            Movement::deposit(a, "USD", "500.00", 50_000, "2026-07-08T00:00:00Z"),
            Movement::deposit(b, "USD", "500.00", 50_000, "2026-07-08T00:00:00Z"),
        ])
        .unwrap_err();
        assert!(matches!(*error, TrustLedgerError::MixedProjects { .. }));
    }

    /// A crypto deposit records the token amount and rail but still credits a
    /// USD value the ledger math runs on.
    #[test]
    fn crypto_deposit_round_trips_through_serde() {
        let project = Uuid::now_v7();
        let movement = Movement::deposit(
            project,
            "USDC",
            "1483.870000",
            148_387,
            "2026-07-08T00:00:00Z",
        )
        .with_external_ref("0xabc123")
        .with_memo("USDC on Base");
        let json = serde_json::to_string(&movement).unwrap();
        let back: Movement = serde_json::from_str(&json).unwrap();
        assert_eq!(back, movement);
        assert_eq!(back.asset, "USDC");
        assert_eq!(back.amount, "1483.870000");
        assert_eq!(back.usd_value_cents, 148_387);
        assert_eq!(back.external_ref.as_deref(), Some("0xabc123"));
    }

    #[test]
    fn optional_fields_are_omitted_when_absent() {
        let project = Uuid::now_v7();
        let json =
            serde_json::to_string(&Movement::earned_draw(project, 100, "2026-08-01T00:00:00Z"))
                .unwrap();
        assert!(
            !json.contains("external_ref"),
            "no external_ref key when None"
        );
        assert!(!json.contains("phase"), "no phase key when None");
        assert!(!json.contains("memo"), "no memo key when None");
    }
}

#[cfg(test)]
mod db_tests {
    use super::{
        append_event, client_account, movements_for, position_for, record_movement, trust_account,
        Movement, TransitionRecord, MACHINE_TRUST_LEDGER,
    };
    use crate::persons::{self, NewPerson};
    use crate::surreal::SurrealDb;
    use crate::test_support::mem_surreal;
    use uuid::Uuid;

    /// Seed the person/template/project/notation graph a trust movement is
    /// anchored to. Returns `(notation_id, project_id, acting_person_id)`.
    async fn seed_engagement(surreal: &SurrealDb) -> (Uuid, Uuid, Uuid) {
        // Unique per call so a single test can seed more than one engagement
        // without colliding on the person-email / template-code constraints.
        let tag = Uuid::now_v7().simple().to_string();
        let libra = persons::create(
            surreal,
            &NewPerson::new("Libra", format!("libra-{tag}@example.com")),
        )
        .await
        .unwrap();
        let tmpl = crate::templates::save_version(
            surreal,
            None,
            &format!("onboarding__letter_{tag}"),
            crate::templates::Version {
                title: "Retainer".into(),
                respondent_type: "person_and_entity".into(),
                asset_id: None,
                form_code: None,
                kind: None,
                source_commit_sha: None,
            },
        )
        .await
        .unwrap()
        .into_model();
        let proj = crate::test_support::seed_project(surreal, "Libra litigation").await;
        let notation_id = crate::notations::create(
            surreal,
            &crate::notations::NewNotation::new(tmpl.id, libra.id, proj.id, "BEGIN"),
        )
        .await
        .unwrap()
        .id;
        (notation_id, proj.id, libra.id)
    }

    /// A full lifecycle persists to the journal and folds back to the same
    /// position, with movements returned oldest → newest.
    #[tokio::test]
    async fn record_and_read_back_a_trust_lifecycle() {
        let surreal = mem_surreal().await;
        let (notation_id, project, actor) = seed_engagement(&surreal).await;

        let deposit = Movement::deposit(project, "USD", "1483.87", 148_387, "2026-07-08T00:00:00Z")
            .with_phase("pleadings");
        let earned = Movement::earned_draw(project, 100_000, "2026-09-01T00:00:00Z");
        let refund = Movement::refund(project, 48_387, "2026-10-01T00:00:00Z");

        record_movement(&surreal, notation_id, actor, &deposit)
            .await
            .unwrap();
        record_movement(&surreal, notation_id, actor, &earned)
            .await
            .unwrap();
        record_movement(&surreal, notation_id, actor, &refund)
            .await
            .unwrap();

        let movements = movements_for(&surreal, notation_id).await.unwrap();
        assert_eq!(movements.len(), 3, "oldest → newest");
        assert_eq!(movements[0].phase.as_deref(), Some("pleadings"));
        assert_eq!(movements[1], earned);
        assert_eq!(movements[2], refund);

        let pos = position_for(&surreal, notation_id).await.unwrap();
        assert_eq!(pos.deposited_cents, 148_387);
        assert_eq!(pos.earned_cents, 100_000);
        assert_eq!(pos.refunded_cents, 48_387);
        assert_eq!(pos.held_cents(), 0, "earned + refunded exhaust the deposit");
    }

    /// The event carries the double-entry legs on `from_state`/`to_state` and
    /// the kind on `condition`, so a trust posting reads at a glance on the
    /// journal.
    #[tokio::test]
    async fn movement_mirrors_its_legs_onto_the_journal_row() {
        let surreal = mem_surreal().await;
        let (notation_id, project, actor) = seed_engagement(&surreal).await;
        let deposit = Movement::deposit(project, "USD", "500.00", 50_000, "2026-07-08T00:00:00Z");
        let row = record_movement(&surreal, notation_id, actor, &deposit)
            .await
            .unwrap();

        assert_eq!(row.machine_kind, MACHINE_TRUST_LEDGER);
        assert_eq!(row.condition, "deposit");
        assert_eq!(row.from_state, deposit.from_account);
        assert_eq!(row.to_state, deposit.to_account);
        assert_eq!(row.acting_person_id, actor);

        // Only trust_ledger rows come back from movements_for.
        let count = crate::notation_events::for_notation(&surreal, notation_id)
            .await
            .unwrap()
            .into_iter()
            .filter(|e| e.machine_kind == MACHINE_TRUST_LEDGER)
            .count();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn record_rejects_a_movement_for_another_project() {
        let surreal = mem_surreal().await;
        let (notation_id, _project, actor) = seed_engagement(&surreal).await;
        let other_project = Uuid::now_v7();
        let deposit = Movement::deposit(
            other_project,
            "USD",
            "500.00",
            50_000,
            "2026-07-08T00:00:00Z",
        );

        assert!(record_movement(&surreal, notation_id, actor, &deposit)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn read_rejects_payload_that_disagrees_with_journal_legs() {
        let surreal = mem_surreal().await;
        let (notation_id, project, actor) = seed_engagement(&surreal).await;
        let deposit = Movement::deposit(project, "USD", "500.00", 50_000, "2026-07-08T00:00:00Z");
        append_event(
            &surreal,
            TransitionRecord {
                notation_id,
                acting_person_id: Some(actor),
                machine_kind: MACHINE_TRUST_LEDGER,
                from_state: &trust_account(project),
                to_state: &client_account(project),
                condition: "deposit",
                payload_json: Some(serde_json::to_string(&deposit).unwrap()),
                recorded_at: "2026-07-08T00:00:00Z",
            },
        )
        .await
        .unwrap();

        assert!(movements_for(&surreal, notation_id).await.is_err());
    }

    /// A `trust_ledger` row with a missing or unparseable payload is a
    /// corruption `movements_for` surfaces loudly rather than silently drops.
    #[tokio::test]
    async fn malformed_trust_events_surface_as_errors() {
        let surreal = mem_surreal().await;

        let raw = |notation_id, actor, payload_json| TransitionRecord {
            notation_id,
            acting_person_id: Some(actor),
            machine_kind: MACHINE_TRUST_LEDGER,
            from_state: "client:x",
            to_state: "trust:x",
            condition: "deposit",
            payload_json,
            recorded_at: "2026-07-08T00:00:00Z",
        };

        // Missing payload.
        let (empty, _project, actor) = seed_engagement(&surreal).await;
        append_event(&surreal, raw(empty, actor, None))
            .await
            .unwrap();
        assert!(
            movements_for(&surreal, empty).await.is_err(),
            "a payload-less trust event must error"
        );

        // Unparseable payload.
        let (bad, _project, actor) = seed_engagement(&surreal).await;
        append_event(&surreal, raw(bad, actor, Some("{ not json".to_string())))
            .await
            .unwrap();
        assert!(
            movements_for(&surreal, bad).await.is_err(),
            "an unparseable trust event must error"
        );
    }
}
