//! Billing-provider seam for the matter lifecycle.
//!
//! When a matter closes (the firm countersignature on the closing letter,
//! see the onboarding/closing walk) the firm bills the flat fee. Rather
//! than couple the workflow to one accounting vendor we target this small
//! trait. The shipped [`StubBillingProvider`] is what dev and tests use —
//! it records every call to an internal Mutex so tests can assert the
//! step fired with the right invoice.
//!
//! Production plugs in [`XeroBillingProvider`] behind the same trait — it
//! POSTs an `ACCREC` invoice to the Xero Accounting API and returns the
//! `InvoiceID` as the [`InvoiceId`]. Auth is the client-credentials grant
//! ([`xero_auth::XeroClientCredentials`]); tokens are minted and cached
//! with a refresh-before-expiry path, mirroring the DocuSign signature
//! provider in `web`.
//!
//! Unconfigured → the stub, so a fork boots and self-tests without a Xero
//! account (the third-party "one vendor account per environment"
//! convention — `docs/third-party-integrations.md`).
//!
//! Lives in its own crate (not `web`) so the worker-side billing
//! workflows in `billing-workflows` can reach the same provider seam —
//! `web` re-exports it as `portal::billing` / `portal::xero_auth`.

pub mod gcp_cost;
pub mod xero_auth;

pub use gcp_cost::{
    adc_token_provider, billing_account_from_table, billing_export_tables, format_cost_sql,
    parse_cost_rows, BillingClient, CostReport, CostRow, ProjectCost, StaticToken, TokenProvider,
};

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use crate::xero_auth::XeroClientCredentials;

/// Opaque identifier returned by the billing provider for a created
/// invoice (Xero's `InvoiceID` GUID). Used to correlate later events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceId(pub String);

/// Opaque identifier for a billing contact (Xero's `ContactID` GUID).
/// Resolved once per billed party via find-or-create on email; the same
/// email always returns the same contact, never duplicated, so resolution
/// is idempotent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactId(pub String);

/// An invoice's reconcilable state, read back from the provider. `status`
/// is the provider's own status string (`AUTHORISED`, `PAID`, `VOIDED`,
/// …); `amount_paid_cents` is minor units, parsed from the provider's
/// decimal at the wire boundary and rounded to whole cents immediately so
/// no float enters our money path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceStatus {
    pub status: String,
    pub amount_paid_cents: i64,
}

/// One line on an invoice. Money is carried in **minor units (cents)** —
/// never a float — and rendered to a decimal string only at the wire
/// boundary. Quantity is whole units (a flat-fee matter bills quantity 1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceLine {
    pub description: String,
    pub quantity: u32,
    pub unit_amount_cents: i64,
    /// Xero chart-of-accounts code the revenue posts to (e.g. `"200"`).
    pub account_code: String,
}

/// The invoice to raise for a matter: who is billed and the line items.
/// `reference` is the human-facing matter reference shown on the invoice;
/// idempotency is keyed separately on the matter id (see
/// [`BillingProvider::create_invoice`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceRequest {
    pub contact_name: String,
    pub contact_email: String,
    pub reference: String,
    pub line_items: Vec<InvoiceLine>,
}

/// The billed party to resolve (find-or-create) in the accounting system
/// before an invoice is raised against it. `email` is the **match key** —
/// the payer's stable identity that find-or-create looks up on — and
/// `name` is the display name, set as Xero's required unique `Name` when
/// the contact is first created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactRequest {
    pub name: String,
    pub email: String,
}

/// One captured `create_invoice` invocation. Tests assert on the contents
/// of [`StubBillingProvider::calls`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BillingCall {
    pub matter_id: Uuid,
    pub request: InvoiceRequest,
}

#[derive(Debug, thiserror::Error)]
pub enum BillingError {
    #[error("provider error: {0}")]
    Provider(String),
}

#[async_trait]
pub trait BillingProvider: Send + Sync {
    /// Resolve the billed party to a provider contact id, creating the
    /// contact when it does not yet exist. Idempotent: a contact with the
    /// same email is returned, never duplicated.
    async fn ensure_contact(&self, request: &ContactRequest) -> Result<ContactId, BillingError>;

    /// Raise an invoice for the given matter. Must be idempotent on retry
    /// (Restate replays the closing step): the same `matter_id` never
    /// creates two invoices.
    async fn create_invoice(
        &self,
        matter_id: Uuid,
        request: &InvoiceRequest,
    ) -> Result<InvoiceId, BillingError>;

    /// Read an invoice's current status + amount paid, by provider
    /// invoice id. Used by the nightly reconcile to fold Xero's payment
    /// state back onto the local mirror. Read-only.
    async fn get_invoice(&self, invoice_id: &str) -> Result<InvoiceStatus, BillingError>;
}

/// In-process stub. Records every call and hands back synthetic
/// `stub-invoice-<matter_id>-<seq>` ids unique within the process.
#[derive(Default)]
pub struct StubBillingProvider {
    calls: Mutex<Vec<BillingCall>>,
    contact_calls: Mutex<Vec<ContactRequest>>,
    /// Canned `get_invoice` responses, keyed on invoice id. Unset ids
    /// resolve to `AUTHORISED` / nothing paid — the create-time default.
    invoice_statuses: Mutex<std::collections::HashMap<String, InvoiceStatus>>,
}

impl StubBillingProvider {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Pre-seed what `get_invoice` returns for an id, so a reconcile test
    /// can simulate Xero reporting an invoice paid.
    pub fn set_invoice_status(&self, invoice_id: &str, status: InvoiceStatus) {
        self.invoice_statuses
            .lock()
            .expect("stub provider lock")
            .insert(invoice_id.to_string(), status);
    }

    /// Snapshot of every call so far. Cheap clone — callers can hold onto
    /// it across `await` points.
    #[must_use]
    pub fn calls(&self) -> Vec<BillingCall> {
        self.calls.lock().expect("stub provider lock").clone()
    }

    /// Snapshot of every `ensure_contact` call so far.
    #[must_use]
    pub fn contact_calls(&self) -> Vec<ContactRequest> {
        self.contact_calls
            .lock()
            .expect("stub provider lock")
            .clone()
    }
}

#[async_trait]
impl BillingProvider for StubBillingProvider {
    async fn ensure_contact(&self, request: &ContactRequest) -> Result<ContactId, BillingError> {
        self.contact_calls
            .lock()
            .expect("stub provider lock")
            .push(request.clone());
        // Deterministic on email, mirroring the real provider's match on
        // the payer's email — a repeated resolve is stable.
        Ok(ContactId(format!("stub-contact-{}", request.email)))
    }

    async fn create_invoice(
        &self,
        matter_id: Uuid,
        request: &InvoiceRequest,
    ) -> Result<InvoiceId, BillingError> {
        let mut calls = self.calls.lock().expect("stub provider lock");
        let seq = calls.len() + 1;
        calls.push(BillingCall {
            matter_id,
            request: request.clone(),
        });
        Ok(InvoiceId(format!("stub-invoice-{matter_id}-{seq}")))
    }

    async fn get_invoice(&self, invoice_id: &str) -> Result<InvoiceStatus, BillingError> {
        Ok(self
            .invoice_statuses
            .lock()
            .expect("stub provider lock")
            .get(invoice_id)
            .cloned()
            .unwrap_or(InvoiceStatus {
                status: "AUTHORISED".into(),
                amount_paid_cents: 0,
            }))
    }
}

/// Render minor units (cents) as a Xero decimal amount string — pure, so
/// no float ever touches the money path. `1_111_00` → `"1111.00"`.
#[must_use]
fn format_cents(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let abs = cents.unsigned_abs();
    format!("{sign}{}.{:02}", abs / 100, abs % 100)
}

/// Re-mint a token this many seconds before Xero's stated expiry, so an
/// in-flight request never carries a token that lapses mid-call.
const TOKEN_REFRESH_LEAD_SECS: u64 = 120;

/// Current unix time in seconds — the provider's runtime token cache
/// reads the real clock here.
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A minted token and the unix second at/after which it must be re-minted
/// (Xero's stated expiry, less [`TOKEN_REFRESH_LEAD_SECS`]).
#[derive(Clone)]
struct CachedToken {
    token: String,
    refresh_at: u64,
}

impl CachedToken {
    /// The token if still fresh at `now` (strictly before `refresh_at`);
    /// `None` means the caller must re-mint.
    fn fresh_at(&self, now: u64) -> Option<&str> {
        (now < self.refresh_at).then_some(self.token.as_str())
    }
}

/// How [`XeroBillingProvider`] obtains the bearer for each REST call.
/// `Static` is the simple local/demo path (a `XERO_ACCESS_TOKEN` Xero
/// expires in 30m); `ClientCredentials` is the durable path that mints +
/// caches short-lived tokens, re-minting before expiry. The cache is
/// shared across clones so a cloned handle reuses an already-minted token.
#[derive(Clone)]
enum TokenSource {
    Static(String),
    ClientCredentials {
        auth: Arc<XeroClientCredentials>,
        cached: Arc<AsyncMutex<Option<CachedToken>>>,
    },
}

/// Production [`BillingProvider`] backed by the Xero Accounting REST API.
/// Creates one `ACCREC` invoice per matter and returns Xero's `InvoiceID`.
///
/// Config is `.env`-driven so OSS forks plug in their own Xero custom
/// connection (or swap the impl): `base_url` is the Accounting API base
/// (`https://api.xero.com/api.xro/2.0`), and `tenant_id` is the connected
/// organisation's id sent as `Xero-Tenant-Id`. A custom connection binds
/// to one organisation, so `tenant_id` is fixed per environment.
#[derive(Clone)]
pub struct XeroBillingProvider {
    http: reqwest::Client,
    base_url: String,
    tenant_id: String,
    token: TokenSource,
}

impl XeroBillingProvider {
    /// Build a provider that authenticates with a static bearer
    /// (`XERO_ACCESS_TOKEN`). The simple local/demo path; Xero expires
    /// the token in 30m and it is not refreshed.
    #[must_use]
    pub fn new(
        base_url: impl Into<String>,
        tenant_id: impl Into<String>,
        access_token: impl Into<String>,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into(),
            tenant_id: tenant_id.into(),
            token: TokenSource::Static(access_token.into()),
        }
    }

    /// Build a provider that authenticates via the client-credentials
    /// grant — minting and caching short-lived tokens from `auth`,
    /// re-minting before expiry. The durable production path.
    #[must_use]
    pub fn with_client_credentials(
        base_url: impl Into<String>,
        tenant_id: impl Into<String>,
        auth: XeroClientCredentials,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into(),
            tenant_id: tenant_id.into(),
            token: TokenSource::ClientCredentials {
                auth: Arc::new(auth),
                cached: Arc::new(AsyncMutex::new(None)),
            },
        }
    }

    /// Build from the environment, returning `None` when the required
    /// vars are absent (dev / KIND, where the stub is used instead).
    ///
    /// Required: `XERO_TENANT_ID`. Optional: `XERO_BASE_URL` (defaults to
    /// the Xero Accounting API base).
    ///
    /// Prefers the client-credentials grant when
    /// [`XeroClientCredentials::from_env`] is satisfied (client id +
    /// secret present) — the self-refreshing production path — and
    /// otherwise falls back to a static `XERO_ACCESS_TOKEN`.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let get = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
        let tenant_id = get("XERO_TENANT_ID")?;
        let base_url =
            get("XERO_BASE_URL").unwrap_or_else(|| "https://api.xero.com/api.xro/2.0".to_string());
        if let Some(auth) = XeroClientCredentials::from_env() {
            Some(Self::with_client_credentials(base_url, tenant_id, auth))
        } else {
            Some(Self::new(base_url, tenant_id, get("XERO_ACCESS_TOKEN")?))
        }
    }

    /// The bearer for the next REST call. `Static` returns the configured
    /// token; `ClientCredentials` returns a cached token, minting a fresh
    /// one when none is cached or the current one is within
    /// [`TOKEN_REFRESH_LEAD_SECS`] of expiry.
    async fn bearer_token(&self) -> Result<String, BillingError> {
        match &self.token {
            TokenSource::Static(t) => Ok(t.clone()),
            TokenSource::ClientCredentials { auth, cached } => {
                let now = now_unix();
                let mut guard = cached.lock().await;
                if let Some(token) = guard.as_ref().and_then(|c| c.fresh_at(now)) {
                    return Ok(token.to_string());
                }
                let minted = auth.mint().await?;
                *guard = Some(CachedToken {
                    token: minted.access_token.clone(),
                    refresh_at: now + minted.expires_in.saturating_sub(TOKEN_REFRESH_LEAD_SECS),
                });
                Ok(minted.access_token)
            }
        }
    }

    /// Build the Xero Accounting `Invoices` POST body. Pure — exposed for
    /// unit-testing the request shape without an HTTP round-trip. Type
    /// `ACCREC` (accounts-receivable, a sales invoice) and status
    /// `AUTHORISED` so the invoice is immediately billable.
    #[must_use]
    pub fn build_invoice_body(&self, request: &InvoiceRequest) -> serde_json::Value {
        let line_items: Vec<serde_json::Value> = request
            .line_items
            .iter()
            .map(|l| {
                serde_json::json!({
                    "Description": l.description,
                    "Quantity": l.quantity,
                    "UnitAmount": format_cents(l.unit_amount_cents),
                    "AccountCode": l.account_code,
                })
            })
            .collect();
        serde_json::json!({
            "Type": "ACCREC",
            "Status": "AUTHORISED",
            "Reference": request.reference,
            "Contact": {
                "Name": request.contact_name,
                "EmailAddress": request.contact_email,
            },
            "LineItems": line_items,
        })
    }

    /// Build the Xero `Contacts` POST body for a find-or-create. Pure —
    /// exposed for unit-testing the request shape without a round-trip.
    #[must_use]
    pub fn build_contact_body(&self, request: &ContactRequest) -> serde_json::Value {
        serde_json::json!({
            "Name": request.name,
            "EmailAddress": request.email,
        })
    }

    /// Look a contact up by email — the payer's stable identity — returning
    /// its id when one already exists. Xero does not force email
    /// uniqueness, so if several contacts share an address we take the
    /// first; the create path still sets a Xero-unique `Name`.
    async fn find_contact_by_email(&self, email: &str) -> Result<Option<ContactId>, BillingError> {
        let url = format!("{}/Contacts", self.base_url.trim_end_matches('/'));
        // Xero's `where` is a double-quoted string match; escape any inner
        // quote so an address can't break the predicate.
        let predicate = format!("EmailAddress==\"{}\"", email.replace('"', "\\\""));
        let resp = self
            .http
            .get(&url)
            .bearer_auth(self.bearer_token().await?)
            .header("Xero-Tenant-Id", &self.tenant_id)
            .header(reqwest::header::ACCEPT, "application/json")
            .query(&[("where", predicate)])
            .send()
            .await
            .map_err(|e| BillingError::Provider(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(BillingError::Provider(format!(
                "xero responded {status}: {body}"
            )));
        }
        let parsed: ContactsResponse = resp
            .json()
            .await
            .map_err(|e| BillingError::Provider(e.to_string()))?;
        Ok(parsed
            .contacts
            .into_iter()
            .next()
            .map(|c| ContactId(c.contact_id)))
    }

    /// Create a new Xero contact and return its id.
    async fn create_contact(&self, request: &ContactRequest) -> Result<ContactId, BillingError> {
        let url = format!("{}/Contacts", self.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .post(&url)
            .bearer_auth(self.bearer_token().await?)
            .header("Xero-Tenant-Id", &self.tenant_id)
            .header(reqwest::header::ACCEPT, "application/json")
            .json(&self.build_contact_body(request))
            .send()
            .await
            .map_err(|e| BillingError::Provider(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(BillingError::Provider(format!(
                "xero responded {status}: {body}"
            )));
        }
        let parsed: ContactsResponse = resp
            .json()
            .await
            .map_err(|e| BillingError::Provider(e.to_string()))?;
        parsed
            .contacts
            .into_iter()
            .next()
            .map(|c| ContactId(c.contact_id))
            .ok_or_else(|| BillingError::Provider("xero returned no contact".to_string()))
    }
}

/// Xero's invoice-create response. Accounting API wraps results in a
/// top-level `Invoices` array; we need only the first id.
#[derive(Deserialize)]
struct InvoicesResponse {
    #[serde(rename = "Invoices")]
    invoices: Vec<InvoiceSummary>,
}

#[derive(Deserialize)]
struct InvoiceSummary {
    #[serde(rename = "InvoiceID")]
    invoice_id: String,
    /// Present on `GET /Invoices/{id}` (the reconcile read); absent on the
    /// create response, which returns only the id.
    #[serde(rename = "Status", default)]
    status: Option<String>,
    #[serde(rename = "AmountPaid", default)]
    amount_paid: Option<f64>,
}

/// Xero's contact lookup/create response — the Accounting API wraps
/// results in a top-level `Contacts` array (empty when nothing matched).
#[derive(Deserialize)]
struct ContactsResponse {
    #[serde(rename = "Contacts", default)]
    contacts: Vec<ContactSummary>,
}

#[derive(Deserialize)]
struct ContactSummary {
    #[serde(rename = "ContactID")]
    contact_id: String,
}

#[async_trait]
impl BillingProvider for XeroBillingProvider {
    async fn ensure_contact(&self, request: &ContactRequest) -> Result<ContactId, BillingError> {
        if let Some(existing) = self.find_contact_by_email(&request.email).await? {
            return Ok(existing);
        }
        self.create_contact(request).await
    }

    async fn create_invoice(
        &self,
        matter_id: Uuid,
        request: &InvoiceRequest,
    ) -> Result<InvoiceId, BillingError> {
        let url = format!("{}/Invoices", self.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .post(&url)
            .bearer_auth(self.bearer_token().await?)
            .header("Xero-Tenant-Id", &self.tenant_id)
            // The Accounting API returns XML unless asked for JSON.
            .header(reqwest::header::ACCEPT, "application/json")
            // Idempotency at the provider boundary: the same matter never
            // raises two invoices, even under a concurrent double-send or
            // a Restate replay. Xero honours this key for 24h.
            .header("Idempotency-Key", matter_id.to_string())
            .json(&self.build_invoice_body(request))
            .send()
            .await
            .map_err(|e| BillingError::Provider(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(BillingError::Provider(format!(
                "xero responded {status}: {body}"
            )));
        }
        let parsed: InvoicesResponse = resp
            .json()
            .await
            .map_err(|e| BillingError::Provider(e.to_string()))?;
        let id = parsed
            .invoices
            .into_iter()
            .next()
            .ok_or_else(|| BillingError::Provider("xero returned no invoice".to_string()))?;
        Ok(InvoiceId(id.invoice_id))
    }

    async fn get_invoice(&self, invoice_id: &str) -> Result<InvoiceStatus, BillingError> {
        let url = format!(
            "{}/Invoices/{}",
            self.base_url.trim_end_matches('/'),
            invoice_id
        );
        let resp = self
            .http
            .get(&url)
            .bearer_auth(self.bearer_token().await?)
            .header("Xero-Tenant-Id", &self.tenant_id)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|e| BillingError::Provider(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(BillingError::Provider(format!(
                "xero responded {status}: {body}"
            )));
        }
        let parsed: InvoicesResponse = resp
            .json()
            .await
            .map_err(|e| BillingError::Provider(e.to_string()))?;
        let inv = parsed
            .invoices
            .into_iter()
            .next()
            .ok_or_else(|| BillingError::Provider("xero returned no invoice".to_string()))?;
        Ok(InvoiceStatus {
            status: inv.status.unwrap_or_default(),
            amount_paid_cents: dollars_to_cents(inv.amount_paid.unwrap_or(0.0)),
        })
    }
}

/// Convert a provider decimal amount (e.g. `333.30`) to whole cents. The
/// one float touch is at this wire boundary, rounded immediately to an
/// integer so no float arithmetic enters the money path.
#[must_use]
fn dollars_to_cents(amount: f64) -> i64 {
    (amount * 100.0).round() as i64
}

#[cfg(test)]
mod tests {
    use super::{
        dollars_to_cents, format_cents, BillingProvider, CachedToken, ContactId, ContactRequest,
        InvoiceId, InvoiceLine, InvoiceRequest, InvoiceStatus, StubBillingProvider,
        XeroBillingProvider,
    };
    use crate::xero_auth::XeroClientCredentials;
    use uuid::Uuid;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ID1: Uuid = Uuid::from_u128(1);
    const ID2: Uuid = Uuid::from_u128(2);
    const ID42: Uuid = Uuid::from_u128(42);

    /// A flat-fee estate matter: one line, billed once.
    fn northstar_invoice() -> InvoiceRequest {
        InvoiceRequest {
            contact_name: "Libra".into(),
            contact_email: "libra@example.com".into(),
            reference: "NL-NORTHSTAR-0001".into(),
            line_items: vec![InvoiceLine {
                description: "Estate plan (flat fee)".into(),
                quantity: 1,
                // $3,333.00 in cents.
                unit_amount_cents: 333_300,
                account_code: "200".into(),
            }],
        }
    }

    /// The billed party for an estate matter — an organisation Xero
    /// dedupes on its unique name.
    fn acme_contact() -> ContactRequest {
        ContactRequest {
            name: "Acme LLC".into(),
            email: "ap@acme.example".into(),
        }
    }

    #[test]
    fn format_cents_renders_two_decimal_places() {
        assert_eq!(format_cents(333_300), "3333.00");
        assert_eq!(format_cents(111_105), "1111.05");
        assert_eq!(format_cents(7), "0.07");
        assert_eq!(format_cents(0), "0.00");
        assert_eq!(format_cents(-250), "-2.50");
    }

    #[tokio::test]
    async fn stub_records_each_call_with_matter_id_and_request() {
        let stub = StubBillingProvider::new();
        let id1 = stub
            .create_invoice(ID42, &northstar_invoice())
            .await
            .expect("stub never errors");
        let id2 = stub
            .create_invoice(ID1, &northstar_invoice())
            .await
            .unwrap();

        assert_ne!(id1, id2, "each call gets a distinct id");
        assert_eq!(id1.0, format!("stub-invoice-{ID42}-1"));
        assert_eq!(id2.0, format!("stub-invoice-{ID1}-2"));

        let calls = stub.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].matter_id, ID42);
        // The request threads through verbatim so the step can assert it.
        assert_eq!(calls[0].request, northstar_invoice());
    }

    #[tokio::test]
    async fn stub_calls_snapshot_is_shareable_after_more_calls() {
        let stub = StubBillingProvider::new();
        stub.create_invoice(ID1, &northstar_invoice())
            .await
            .unwrap();
        let snap = stub.calls();
        stub.create_invoice(ID2, &northstar_invoice())
            .await
            .unwrap();
        assert_eq!(snap.len(), 1, "snapshot doesn't see later calls");
        assert_eq!(stub.calls().len(), 2);
    }

    fn xero(base_url: String) -> XeroBillingProvider {
        XeroBillingProvider::new(base_url, "tenant-guid", "TOKEN")
    }

    #[test]
    fn cached_token_is_fresh_strictly_before_refresh_at() {
        let ct = CachedToken {
            token: "tok".into(),
            refresh_at: 100,
        };
        assert_eq!(ct.fresh_at(99), Some("tok"), "fresh before refresh_at");
        assert_eq!(ct.fresh_at(100), None, "must re-mint at the boundary");
        assert_eq!(ct.fresh_at(101), None, "must re-mint past expiry");
    }

    #[test]
    fn invoice_body_is_accrec_authorised_with_decimal_amount() {
        let body = xero("https://api.xero.com/api.xro/2.0".into())
            .build_invoice_body(&northstar_invoice());
        assert_eq!(body["Type"], "ACCREC");
        assert_eq!(body["Status"], "AUTHORISED");
        assert_eq!(body["Reference"], "NL-NORTHSTAR-0001");
        assert_eq!(body["Contact"]["Name"], "Libra");
        let lines = body["LineItems"].as_array().unwrap();
        assert_eq!(lines.len(), 1);
        // Money is a decimal string rendered from cents — never a float.
        assert_eq!(lines[0]["UnitAmount"], "3333.00");
        assert_eq!(lines[0]["Quantity"], 1);
        assert_eq!(lines[0]["AccountCode"], "200");
    }

    #[tokio::test]
    async fn xero_posts_tenant_and_idempotency_headers_and_returns_invoice_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/Invoices"))
            .and(header("authorization", "Bearer TOKEN"))
            .and(header("xero-tenant-id", "tenant-guid"))
            .and(header("idempotency-key", ID42.to_string().as_str()))
            .and(header("accept", "application/json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Invoices": [ { "InvoiceID": "inv-789", "Status": "AUTHORISED" } ],
            })))
            .expect(1)
            .mount(&server)
            .await;

        let id = xero(server.uri())
            .create_invoice(ID42, &northstar_invoice())
            .await
            .expect("create succeeds");
        assert_eq!(id, InvoiceId("inv-789".into()));
    }

    #[tokio::test]
    async fn client_credentials_provider_mints_once_then_serves_from_cache() {
        let server = MockServer::start().await;
        // The token endpoint is hit exactly once across both invoices:
        // the second create must reuse the cached, still-fresh token.
        Mock::given(method("POST"))
            .and(path("/connect/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "minted-xero",
                "token_type": "Bearer",
                "expires_in": 1800,
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/Invoices"))
            .and(header("authorization", "Bearer minted-xero"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Invoices": [ { "InvoiceID": "inv-1" } ],
            })))
            .mount(&server)
            .await;

        let auth = XeroClientCredentials::new(
            "ck",
            "cs",
            format!("{}/connect/token", server.uri()),
            "accounting.transactions",
        );
        let provider =
            XeroBillingProvider::with_client_credentials(server.uri(), "tenant-guid", auth);

        let id1 = provider
            .create_invoice(ID1, &northstar_invoice())
            .await
            .expect("first create mints a token");
        let id2 = provider
            .create_invoice(ID2, &northstar_invoice())
            .await
            .expect("second create reuses the cached token");
        assert_eq!(id1, InvoiceId("inv-1".into()));
        assert_eq!(id2, InvoiceId("inv-1".into()));
        // The `.expect(1)` on the token mock, verified on drop, proves
        // exactly one mint backed both creates.
    }

    #[tokio::test]
    async fn xero_maps_non_2xx_to_provider_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/Invoices"))
            .respond_with(ResponseTemplate::new(400).set_body_string("ValidationException"))
            .mount(&server)
            .await;

        let err = xero(server.uri())
            .create_invoice(ID42, &northstar_invoice())
            .await
            .expect_err("a 400 is a provider error");
        assert!(err.to_string().contains("400"), "status surfaces: {err}");
    }

    #[tokio::test]
    async fn xero_errors_when_response_carries_no_invoice() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/Invoices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Invoices": [],
            })))
            .mount(&server)
            .await;

        let err = xero(server.uri())
            .create_invoice(ID42, &northstar_invoice())
            .await
            .expect_err("empty Invoices is an error");
        assert!(err.to_string().contains("no invoice"), "surfaces: {err}");
    }

    #[tokio::test]
    async fn stub_ensure_contact_records_calls_and_is_deterministic_on_name() {
        let stub = StubBillingProvider::new();
        let a = stub.ensure_contact(&acme_contact()).await.unwrap();
        let b = stub.ensure_contact(&acme_contact()).await.unwrap();
        assert_eq!(a, b, "same email resolves to the same contact id");
        assert_eq!(a, ContactId("stub-contact-ap@acme.example".into()));

        let calls = stub.contact_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], acme_contact());
    }

    #[test]
    fn contact_body_carries_name_and_email() {
        let body =
            xero("https://api.xero.com/api.xro/2.0".into()).build_contact_body(&acme_contact());
        assert_eq!(body["Name"], "Acme LLC");
        assert_eq!(body["EmailAddress"], "ap@acme.example");
    }

    #[tokio::test]
    async fn ensure_contact_returns_existing_id_without_creating() {
        let server = MockServer::start().await;
        // Looked up by the payer's email (the match key).
        Mock::given(method("GET"))
            .and(path("/Contacts"))
            .and(query_param("where", "EmailAddress==\"ap@acme.example\""))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Contacts": [ { "ContactID": "c-existing", "Name": "Acme LLC" } ],
            })))
            .expect(1)
            .mount(&server)
            .await;
        // No POST mock is mounted: a create attempt would 404 and fail.
        let id = xero(server.uri())
            .ensure_contact(&acme_contact())
            .await
            .expect("resolve succeeds");
        assert_eq!(id, ContactId("c-existing".into()));
    }

    #[tokio::test]
    async fn ensure_contact_creates_when_none_exists() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Contacts"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "Contacts": [] })),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/Contacts"))
            .and(header("xero-tenant-id", "tenant-guid"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Contacts": [ { "ContactID": "c-new" } ],
            })))
            .expect(1)
            .mount(&server)
            .await;

        let id = xero(server.uri())
            .ensure_contact(&acme_contact())
            .await
            .expect("create succeeds");
        assert_eq!(id, ContactId("c-new".into()));
    }

    #[tokio::test]
    async fn ensure_contact_maps_lookup_error_to_provider_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Contacts"))
            .respond_with(ResponseTemplate::new(401).set_body_string("Unauthorized"))
            .mount(&server)
            .await;

        let err = xero(server.uri())
            .ensure_contact(&acme_contact())
            .await
            .expect_err("a 401 is a provider error");
        assert!(err.to_string().contains("401"), "status surfaces: {err}");
    }

    #[test]
    fn dollars_to_cents_rounds_two_dp_at_the_wire_boundary() {
        assert_eq!(dollars_to_cents(333.30), 33_330);
        assert_eq!(dollars_to_cents(0.0), 0);
        assert_eq!(dollars_to_cents(1111.0), 111_100);
    }

    #[tokio::test]
    async fn stub_get_invoice_defaults_then_honors_canned_status() {
        let stub = StubBillingProvider::new();
        // Unknown id → the create-time default.
        let def = stub.get_invoice("nope").await.unwrap();
        assert_eq!(def.status, "AUTHORISED");
        assert_eq!(def.amount_paid_cents, 0);

        stub.set_invoice_status(
            "inv-1",
            InvoiceStatus {
                status: "PAID".into(),
                amount_paid_cents: 333_300,
            },
        );
        let paid = stub.get_invoice("inv-1").await.unwrap();
        assert_eq!(paid.status, "PAID");
        assert_eq!(paid.amount_paid_cents, 333_300);
    }

    #[tokio::test]
    async fn xero_get_invoice_parses_status_and_amount_paid() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Invoices/inv-42"))
            .and(header("Xero-Tenant-Id", "tenant-guid"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Invoices": [{
                    "InvoiceID": "inv-42",
                    "Status": "PAID",
                    "AmountPaid": 3333.00
                }]
            })))
            .mount(&server)
            .await;

        let got = xero(server.uri()).get_invoice("inv-42").await.unwrap();
        assert_eq!(got.status, "PAID");
        assert_eq!(got.amount_paid_cents, 333_300);
    }
}
