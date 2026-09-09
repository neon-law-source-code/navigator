//! Signature-provider seam for the retainer-intake workflow.
//!
//! Step 3 of the workflow sends the rendered retainer out for
//! external signature. Google Workspace eSignature has no public
//! API today, so the workflow targets this small trait rather than
//! a concrete vendor. The shipped [`StubSignatureProvider`] is what
//! dev and tests use — it records every call to an internal Mutex
//! so tests can assert that the step fired with the right inputs.
//!
//! Production plugs in a real provider (DocuSign, Dropbox Sign, …)
//! behind the same trait — no changes to the workflow or handler.
//! [`DocuSignSignatureProvider`] is the shipped concrete impl: it
//! POSTs an envelope-create request to the DocuSign eSignature REST
//! API and returns the `envelopeId` as the [`SignatureRequestId`]. The
//! provider's later "completed" callback is received by
//! [`crate::esignature_webhook`], which resolves the envelope id back
//! to a notation and signals `signature_received`.

use std::sync::{Arc, Mutex};
use uuid::Uuid;

use async_trait::async_trait;
use base64::Engine;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;

use crate::docusign_auth::DocuSignJwtAuth;

/// Opaque identifier returned by the signature provider for a sent
/// document. Used to correlate webhook callbacks later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureRequestId(pub String);

/// The executed document set for a completed signature request: the
/// signed PDF (all signatures applied) and the Certificate of
/// Completion — the ESIGN evidentiary record of who signed, when, and
/// from where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedDocuments {
    pub signed_pdf: Vec<u8>,
    pub certificate_pdf: Vec<u8>,
}

/// One captured `send_for_signature` invocation. Tests assert on
/// the contents of [`StubSignatureProvider::calls`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureCall {
    pub notation_id: Uuid,
    pub pdf_bytes_len: usize,
    /// The manifest the step asked us to place — so tests can assert
    /// the right anchors/recipients threaded through.
    pub manifest: SignatureManifest,
}

/// The kind of signature field an anchor expands into — each maps to a
/// distinct DocuSign tab collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignatureFieldKind {
    Signature,
    Initials,
    Date,
}

impl SignatureFieldKind {
    /// Every recognized field kind, in placement order.
    pub const ALL: [SignatureFieldKind; 3] = [
        SignatureFieldKind::Signature,
        SignatureFieldKind::Initials,
        SignatureFieldKind::Date,
    ];

    /// DocuSign's tab-collection key for this kind.
    #[must_use]
    pub fn tabs_key(self) -> &'static str {
        match self {
            SignatureFieldKind::Signature => "signHereTabs",
            SignatureFieldKind::Initials => "initialHereTabs",
            SignatureFieldKind::Date => "dateSignedTabs",
        }
    }
}

/// One person who signs the document, with their routing position.
/// Identities are resolved by the caller (the respondent Person, the
/// attorney of record, or `person__<role>` for any other declared
/// signer) — the provider never hardcodes who a role is. `routing_order`
/// is load-bearing: recipients sign in the template's `signers:`
/// declaration order (default client then firm).
///
/// `client_user_id`, when set, makes the recipient **captive** (embedded):
/// DocuSign does NOT email them a signing link; instead the app requests
/// a short-lived signing URL via [`SignatureProvider::create_recipient_view`]
/// and shows it inside Neon Law Navigator. The same id must be replayed on the
/// recipient-view request, so it is the caller's stable handle for this
/// signer (we key it on the notation). `None` keeps the recipient
/// non-captive — DocuSign emails them the usual link (the firm
/// countersignature path).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureRecipient {
    pub role: String,
    pub email: String,
    pub name: String,
    pub routing_order: u32,
    #[serde(default)]
    pub client_user_id: Option<String>,
}

/// One typed field anchored into the document text, bound to the role
/// that fills it. `anchor` is the exact string the renderer emitted as
/// invisible text in the PDF (see `signature_render`, added in step 2b).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureField {
    pub recipient_role: String,
    pub kind: SignatureFieldKind,
    pub anchor: String,
}

/// The full placement plan for one envelope: who signs (recipients) and
/// what typed fields anchor onto the document (fields). Empty means
/// "no anchored signing plan" — the provider falls back to its single
/// configured recipient with no tabs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureManifest {
    pub recipients: Vec<SignatureRecipient>,
    pub fields: Vec<SignatureField>,
}

impl SignatureManifest {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.recipients.is_empty()
    }
}

/// The inputs for an embedded-signing URL request. The
/// `email`/`name`/`client_user_id` triple must exactly match a captive
/// recipient created on the envelope (see [`SignatureRecipient::client_user_id`]) —
/// DocuSign resolves the recipient from those, then issues a one-shot
/// URL that redirects the browser to `return_url` once signing finishes
/// (or is declined / times out).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipientView {
    pub return_url: String,
    pub email: String,
    pub name: String,
    pub client_user_id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SignatureError {
    #[error("provider error: {0}")]
    Provider(String),
}

#[async_trait]
pub trait SignatureProvider: Send + Sync {
    /// Submit the rendered retainer PDF for the given notation, placing
    /// the fields described by `manifest`. Returns a provider-issued id
    /// correlating future events.
    async fn send_for_signature(
        &self,
        notation_id: Uuid,
        pdf: &[u8],
        manifest: &SignatureManifest,
    ) -> Result<SignatureRequestId, SignatureError>;

    /// Request a short-lived, single-use **recipient-view URL** for a captive
    /// recipient of an already-created envelope. DocuSign suppresses the
    /// signing email for a captive recipient, so this URL is the only way in:
    /// Navigator redirects the signer to it and the ceremony happens on the
    /// provider's own site. Only valid for recipients sent with a
    /// `client_user_id`; the URL expires in minutes.
    async fn create_recipient_view(
        &self,
        request_id: &SignatureRequestId,
        view: &RecipientView,
    ) -> Result<String, SignatureError>;

    /// Fetch the executed document set for a completed request — the
    /// signed PDF and the Certificate of Completion. Called on the
    /// completion webhook so the ESIGN record is archived to storage.
    async fn fetch_completed_documents(
        &self,
        request_id: &SignatureRequestId,
    ) -> Result<CompletedDocuments, SignatureError>;
}

/// In-process stub. Records every call and hands back synthetic
/// `stub-<notation_id>-<seq>` ids that are unique within the process.
#[derive(Default)]
pub struct StubSignatureProvider {
    calls: Mutex<Vec<SignatureCall>>,
    recipient_views: Mutex<Vec<RecipientView>>,
}

impl StubSignatureProvider {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of every call so far. Cheap clone — callers can
    /// hold onto it across `await` points.
    #[must_use]
    pub fn calls(&self) -> Vec<SignatureCall> {
        self.calls.lock().expect("stub provider lock").clone()
    }

    /// Snapshot of every [`RecipientView`] requested so far.
    ///
    /// The synthetic signing URL the stub returns is built from the request
    /// id and the `clientUserId` alone, so without this the `return_url` and
    /// the `email`/`userName` legs would leave no trace and a test could not
    /// tell an absolute return URL from a relative one — which is exactly how
    /// a relative one reached the wire unnoticed (ENG-557).
    #[must_use]
    pub fn recipient_views(&self) -> Vec<RecipientView> {
        self.recipient_views
            .lock()
            .expect("stub provider lock")
            .clone()
    }
}

#[async_trait]
impl SignatureProvider for StubSignatureProvider {
    async fn send_for_signature(
        &self,
        notation_id: Uuid,
        pdf: &[u8],
        manifest: &SignatureManifest,
    ) -> Result<SignatureRequestId, SignatureError> {
        let mut calls = self.calls.lock().expect("stub provider lock");
        let seq = calls.len() + 1;
        calls.push(SignatureCall {
            notation_id,
            pdf_bytes_len: pdf.len(),
            manifest: manifest.clone(),
        });
        Ok(SignatureRequestId(format!("stub-{notation_id}-{seq}")))
    }

    async fn create_recipient_view(
        &self,
        request_id: &SignatureRequestId,
        view: &RecipientView,
    ) -> Result<String, SignatureError> {
        self.recipient_views
            .lock()
            .expect("stub provider lock")
            .push(view.clone());
        // A deterministic fake signing URL so dev / KIND can exercise the
        // embedded-signing route without a real DocuSign account.
        Ok(format!(
            "https://stub.docusign.local/signing/{}/{}",
            request_id.0, view.client_user_id
        ))
    }

    async fn fetch_completed_documents(
        &self,
        _request_id: &SignatureRequestId,
    ) -> Result<CompletedDocuments, SignatureError> {
        Ok(CompletedDocuments {
            signed_pdf: b"%PDF-1.7 stub-signed".to_vec(),
            certificate_pdf: b"%PDF-1.7 stub-certificate".to_vec(),
        })
    }
}

/// Build DocuSign's `tabs` object for one recipient `role` from the
/// manifest's fields, grouping each anchored field under its DocuSign
/// tab-collection key. Returns `None` when the role anchors no fields,
/// so the signer JSON simply omits `tabs`.
fn tabs_for_role(role: &str, fields: &[SignatureField]) -> Option<serde_json::Value> {
    let mut tabs = serde_json::Map::new();
    for kind in SignatureFieldKind::ALL {
        let placed: Vec<serde_json::Value> = fields
            .iter()
            .filter(|f| f.recipient_role == role && f.kind == kind)
            .map(|f| {
                serde_json::json!({
                    "anchorString": f.anchor,
                    "anchorUnits": "pixels",
                    "anchorYOffset": "-6",
                    "anchorIgnoreIfNotPresent": "false",
                })
            })
            .collect();
        if !placed.is_empty() {
            tabs.insert(
                kind.tabs_key().to_string(),
                serde_json::Value::Array(placed),
            );
        }
    }
    (!tabs.is_empty()).then_some(serde_json::Value::Object(tabs))
}

/// Re-mint a JWT-grant token this many seconds before DocuSign's stated
/// expiry, so an in-flight request never carries a token that lapses
/// mid-call.
const TOKEN_REFRESH_LEAD_SECS: u64 = 300;

/// Current unix time in seconds. The JWT assertion builder takes `now`
/// as a parameter for determinism; the provider's runtime token cache
/// reads the real clock here.
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A minted JWT-grant token and the unix second at/after which it must
/// be re-minted (DocuSign's stated expiry, less [`TOKEN_REFRESH_LEAD_SECS`]).
#[derive(Clone)]
struct CachedToken {
    token: String,
    refresh_at: u64,
}

impl CachedToken {
    /// The token if it is still fresh at `now` (strictly before
    /// `refresh_at`); `None` means the caller must re-mint.
    fn fresh_at(&self, now: u64) -> Option<&str> {
        (now < self.refresh_at).then_some(self.token.as_str())
    }
}

/// How [`DocuSignSignatureProvider`] obtains the OAuth bearer for each
/// REST call. `Static` is the simple local/demo path (a `DOCUSIGN_ACCESS_TOKEN`
/// that DocuSign expires in ~8h); `Jwt` is the durable production path
/// that mints + caches short-lived tokens via [`DocuSignJwtAuth`],
/// re-minting before expiry. The cache is shared across clones of the
/// provider so a cloned handle reuses a token already minted elsewhere.
#[derive(Clone)]
enum TokenSource {
    Static(String),
    Jwt {
        auth: Arc<DocuSignJwtAuth>,
        cached: Arc<AsyncMutex<Option<CachedToken>>>,
    },
}

/// Production [`SignatureProvider`] backed by the DocuSign eSignature
/// REST API. Creates one envelope per notation (the rendered retainer
/// PDF, sent to the client for signature) and returns DocuSign's
/// `envelopeId` as the [`SignatureRequestId`].
///
/// Config is `.env`-driven so OSS forks plug in their own DocuSign
/// account (or swap the whole impl): `base_url` is the account's
/// eSignature base path (e.g. `https://demo.docusign.net/restapi` for
/// the developer sandbox), `account_id` the API account GUID, and
/// `access_token` an OAuth bearer token. We hit the REST surface
/// directly with `reqwest` rather than pulling a DocuSign SDK — the
/// surface we need is one POST, mirroring [`workflows::email`]'s
/// SendGrid backend.
///
/// `signer_email` / `signer_name` address the envelope. v1 sends to a
/// single configured recipient (the firm's own signing inbox in the
/// reference deploy); per-client routing belongs to a later change.
#[derive(Clone)]
pub struct DocuSignSignatureProvider {
    http: reqwest::Client,
    base_url: String,
    account_id: String,
    token: TokenSource,
    signer_email: String,
    signer_name: String,
}

impl DocuSignSignatureProvider {
    /// Build a provider that authenticates with a static OAuth bearer
    /// (`DOCUSIGN_ACCESS_TOKEN`). The simple local/demo path; the token
    /// expires in ~8h and is not refreshed.
    #[must_use]
    pub fn new(
        base_url: impl Into<String>,
        account_id: impl Into<String>,
        access_token: impl Into<String>,
        signer_email: impl Into<String>,
        signer_name: impl Into<String>,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into(),
            account_id: account_id.into(),
            token: TokenSource::Static(access_token.into()),
            signer_email: signer_email.into(),
            signer_name: signer_name.into(),
        }
    }

    /// Build a provider that authenticates via JWT grant — minting and
    /// caching short-lived tokens from `auth`, re-minting before expiry.
    /// The durable production token path.
    #[must_use]
    pub fn with_jwt_auth(
        base_url: impl Into<String>,
        account_id: impl Into<String>,
        auth: DocuSignJwtAuth,
        signer_email: impl Into<String>,
        signer_name: impl Into<String>,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into(),
            account_id: account_id.into(),
            token: TokenSource::Jwt {
                auth: Arc::new(auth),
                cached: Arc::new(AsyncMutex::new(None)),
            },
            signer_email: signer_email.into(),
            signer_name: signer_name.into(),
        }
    }

    /// Build from the environment, returning `None` when the required
    /// vars are absent (dev / KIND, where the stub is used instead).
    ///
    /// Required: `DOCUSIGN_BASE_URL`, `DOCUSIGN_ACCOUNT_ID`. Optional:
    /// `DOCUSIGN_SIGNER_EMAIL` / `DOCUSIGN_SIGNER_NAME` (the envelope
    /// recipient), defaulting to the firm's support inbox.
    ///
    /// Prefers JWT grant when [`DocuSignJwtAuth::from_env`] is satisfied
    /// (integration key + impersonated user + RSA key present) — the
    /// self-refreshing production path — and otherwise falls back to a
    /// static `DOCUSIGN_ACCESS_TOKEN` for a quick local/demo smoke test.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let get = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
        let base_url = get("DOCUSIGN_BASE_URL")?;
        let account_id = get("DOCUSIGN_ACCOUNT_ID")?;
        let signer_email =
            get("DOCUSIGN_SIGNER_EMAIL").unwrap_or_else(|| "support@neonlaw.com".to_string());
        let signer_name = get("DOCUSIGN_SIGNER_NAME").unwrap_or_else(|| "Neon Law".to_string());
        if let Some(auth) = DocuSignJwtAuth::from_env() {
            Some(Self::with_jwt_auth(
                base_url,
                account_id,
                auth,
                signer_email,
                signer_name,
            ))
        } else {
            Some(Self::new(
                base_url,
                account_id,
                get("DOCUSIGN_ACCESS_TOKEN")?,
                signer_email,
                signer_name,
            ))
        }
    }

    /// The OAuth bearer for the next REST call. `Static` sources return
    /// the configured token; `Jwt` sources return a cached token,
    /// minting a fresh one when none is cached or the current one is
    /// within [`TOKEN_REFRESH_LEAD_SECS`] of expiry.
    async fn bearer_token(&self) -> Result<String, SignatureError> {
        match &self.token {
            TokenSource::Static(t) => Ok(t.clone()),
            TokenSource::Jwt { auth, cached } => {
                let now = now_unix();
                let mut guard = cached.lock().await;
                if let Some(token) = guard.as_ref().and_then(|c| c.fresh_at(now)) {
                    return Ok(token.to_string());
                }
                let minted = auth.mint(now).await?;
                *guard = Some(CachedToken {
                    token: minted.access_token.clone(),
                    refresh_at: now + minted.expires_in.saturating_sub(TOKEN_REFRESH_LEAD_SECS),
                });
                Ok(minted.access_token)
            }
        }
    }

    /// Build the DocuSign envelope-create JSON body. Pure — exposed for
    /// unit-testing the request shape without an HTTP round-trip.
    ///
    /// When `manifest` carries recipients, each becomes a signer with
    /// its anchored `tabs` and routing order. When it's empty, we fall
    /// back to the single configured recipient with no tabs (the
    /// pre-anchor behavior) — a deliberate transitional state until the
    /// caller supplies a real manifest.
    #[must_use]
    pub fn build_envelope_body(
        &self,
        notation_id: Uuid,
        pdf: &[u8],
        manifest: &SignatureManifest,
    ) -> serde_json::Value {
        let document_b64 = base64::engine::general_purpose::STANDARD.encode(pdf);
        let signers: Vec<serde_json::Value> = if manifest.is_empty() {
            vec![serde_json::json!({
                "email": self.signer_email,
                "name": self.signer_name,
                "recipientId": "1",
                "routingOrder": "1",
            })]
        } else {
            manifest
                .recipients
                .iter()
                .map(|r| {
                    let mut signer = serde_json::json!({
                        "email": r.email,
                        "name": r.name,
                        "recipientId": r.routing_order.to_string(),
                        "routingOrder": r.routing_order.to_string(),
                    });
                    // A captive recipient carries `clientUserId`: DocuSign
                    // then suppresses the email and the signer must be
                    // driven through an embedded recipient view.
                    if let Some(client_user_id) = &r.client_user_id {
                        signer["clientUserId"] = serde_json::Value::String(client_user_id.clone());
                    }
                    if let Some(tabs) = tabs_for_role(&r.role, &manifest.fields) {
                        signer["tabs"] = tabs;
                    }
                    signer
                })
                .collect()
        };
        serde_json::json!({
            "emailSubject": format!("Document for signature ({notation_id})"),
            "status": "sent",
            "documents": [{
                "documentBase64": document_b64,
                "name": "Document",
                "fileExtension": "pdf",
                "documentId": "1",
            }],
            "recipients": { "signers": signers },
        })
    }

    /// GET one envelope document as raw bytes. `which` is the DocuSign
    /// document selector — `combined` (all signed docs merged) or
    /// `certificate` (the Certificate of Completion).
    async fn get_envelope_document(
        &self,
        envelope_id: &str,
        which: &str,
    ) -> Result<Vec<u8>, SignatureError> {
        let url = format!(
            "{}/v2.1/accounts/{}/envelopes/{}/documents/{}",
            self.base_url.trim_end_matches('/'),
            self.account_id,
            envelope_id,
            which
        );
        let resp = self
            .http
            .get(&url)
            .bearer_auth(self.bearer_token().await?)
            .send()
            .await
            .map_err(|e| SignatureError::Provider(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SignatureError::Provider(format!(
                "docusign document {which} responded {status}: {body}"
            )));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| SignatureError::Provider(e.to_string()))?;
        Ok(bytes.to_vec())
    }
}

/// DocuSign's envelope-create response. We need only the id.
#[derive(Deserialize)]
struct EnvelopeSummary {
    #[serde(rename = "envelopeId")]
    envelope_id: String,
}

/// DocuSign's `views/recipient` response. We need only the signing URL.
#[derive(Deserialize)]
struct RecipientViewResponse {
    url: String,
}

#[async_trait]
impl SignatureProvider for DocuSignSignatureProvider {
    async fn send_for_signature(
        &self,
        notation_id: Uuid,
        pdf: &[u8],
        manifest: &SignatureManifest,
    ) -> Result<SignatureRequestId, SignatureError> {
        let url = format!(
            "{}/v2.1/accounts/{}/envelopes",
            self.base_url.trim_end_matches('/'),
            self.account_id
        );
        let resp = self
            .http
            .post(&url)
            // Idempotency at the provider boundary: the same notation
            // never creates two envelopes, even under a concurrent
            // double-send. DocuSign honors this key for 24h.
            .header("X-DocuSign-Idempotency-Key", notation_id.to_string())
            .bearer_auth(self.bearer_token().await?)
            .json(&self.build_envelope_body(notation_id, pdf, manifest))
            .send()
            .await
            .map_err(|e| SignatureError::Provider(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SignatureError::Provider(format!(
                "docusign responded {status}: {body}"
            )));
        }
        let summary: EnvelopeSummary = resp
            .json()
            .await
            .map_err(|e| SignatureError::Provider(e.to_string()))?;
        Ok(SignatureRequestId(summary.envelope_id))
    }

    async fn create_recipient_view(
        &self,
        request_id: &SignatureRequestId,
        view: &RecipientView,
    ) -> Result<String, SignatureError> {
        let url = format!(
            "{}/v2.1/accounts/{}/envelopes/{}/views/recipient",
            self.base_url.trim_end_matches('/'),
            self.account_id,
            request_id.0
        );
        // `authenticationMethod` is the audit label DocuSign records for
        // how the signer was authenticated upstream; "none" means the app
        // (Neon Law Navigator, behind its own OIDC login) vouched for them. The
        // email/userName/clientUserId triple resolves the captive
        // recipient created at envelope time.
        let body = serde_json::json!({
            "returnUrl": view.return_url,
            "authenticationMethod": "none",
            "email": view.email,
            "userName": view.name,
            "clientUserId": view.client_user_id,
        });
        let resp = self
            .http
            .post(&url)
            .bearer_auth(self.bearer_token().await?)
            .json(&body)
            .send()
            .await
            .map_err(|e| SignatureError::Provider(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SignatureError::Provider(format!(
                "docusign recipient view responded {status}: {body}"
            )));
        }
        let view: RecipientViewResponse = resp
            .json()
            .await
            .map_err(|e| SignatureError::Provider(e.to_string()))?;
        Ok(view.url)
    }

    async fn fetch_completed_documents(
        &self,
        request_id: &SignatureRequestId,
    ) -> Result<CompletedDocuments, SignatureError> {
        // `combined` is every signed document merged into one PDF;
        // `certificate` is the Certificate of Completion.
        let signed_pdf = self
            .get_envelope_document(&request_id.0, "combined")
            .await?;
        let certificate_pdf = self
            .get_envelope_document(&request_id.0, "certificate")
            .await?;
        Ok(CompletedDocuments {
            signed_pdf,
            certificate_pdf,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CachedToken, DocuSignSignatureProvider, RecipientView, SignatureField, SignatureFieldKind,
        SignatureManifest, SignatureProvider, SignatureRecipient, SignatureRequestId,
        StubSignatureProvider,
    };
    use crate::docusign_auth::{DocuSignJwtAuth, TEST_PRIV_PEM};
    use base64::Engine;
    use uuid::Uuid;
    use wiremock::matchers::{body_string_contains, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ID1: Uuid = Uuid::from_u128(1);
    const ID2: Uuid = Uuid::from_u128(2);
    const ID42: Uuid = Uuid::from_u128(42);
    const ID43: Uuid = Uuid::from_u128(43);

    /// Empty manifest — the pre-anchor fallback path.
    fn no_manifest() -> SignatureManifest {
        SignatureManifest::default()
    }

    /// A two-party retainer manifest: the client (routing 1) signs and
    /// dates; the firm (routing 2) countersigns.
    fn retainer_manifest() -> SignatureManifest {
        SignatureManifest {
            recipients: vec![
                SignatureRecipient {
                    role: "client".into(),
                    email: "libra@example.com".into(),
                    name: "Libra".into(),
                    routing_order: 1,
                    // The client signs embedded (in-portal), so they are
                    // captive — keyed on a stable per-notation handle.
                    client_user_id: Some("notation-client".into()),
                },
                SignatureRecipient {
                    role: "firm".into(),
                    email: "attorney@neonlaw.com".into(),
                    name: "Attorney".into(),
                    routing_order: 2,
                    // The firm countersigns from its inbox (emailed).
                    client_user_id: None,
                },
            ],
            fields: vec![
                SignatureField {
                    recipient_role: "client".into(),
                    kind: SignatureFieldKind::Signature,
                    anchor: "nlsig-client-signature-1".into(),
                },
                SignatureField {
                    recipient_role: "client".into(),
                    kind: SignatureFieldKind::Date,
                    anchor: "nlsig-client-date-1".into(),
                },
                SignatureField {
                    recipient_role: "firm".into(),
                    kind: SignatureFieldKind::Signature,
                    anchor: "nlsig-firm-signature-1".into(),
                },
            ],
        }
    }

    #[tokio::test]
    async fn stub_records_each_call_with_notation_id_and_pdf_len() {
        let stub = StubSignatureProvider::new();
        let id1 = stub
            .send_for_signature(ID42, b"<pdf bytes>", &retainer_manifest())
            .await
            .expect("stub never errors");
        let id2 = stub
            .send_for_signature(ID43, b"<longer pdf bytes here>", &no_manifest())
            .await
            .unwrap();

        assert_ne!(id1, id2, "each call gets a distinct id");
        assert_eq!(id1.0, format!("stub-{ID42}-1"));
        assert_eq!(id2.0, format!("stub-{ID43}-2"));

        let calls = stub.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].notation_id, ID42);
        assert_eq!(calls[0].pdf_bytes_len, b"<pdf bytes>".len());
        // The manifest threads through verbatim so the step can assert it.
        assert_eq!(calls[0].manifest, retainer_manifest());
        assert_eq!(calls[1].notation_id, ID43);
        assert_eq!(calls[1].pdf_bytes_len, b"<longer pdf bytes here>".len());
        assert!(calls[1].manifest.is_empty());
    }

    #[tokio::test]
    async fn stub_calls_snapshot_is_shareable_after_more_calls() {
        let stub = StubSignatureProvider::new();
        stub.send_for_signature(ID1, b"a", &no_manifest())
            .await
            .unwrap();
        let snap = stub.calls();
        stub.send_for_signature(ID2, b"b", &no_manifest())
            .await
            .unwrap();
        assert_eq!(snap.len(), 1, "snapshot doesn't see later calls");
        assert_eq!(stub.calls().len(), 2);
    }

    fn docusign(base_url: String) -> DocuSignSignatureProvider {
        DocuSignSignatureProvider::new(
            base_url,
            "acct-guid",
            "TOKEN",
            "signer@example.com",
            "Signer",
        )
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

    #[tokio::test]
    async fn jwt_backed_provider_mints_once_then_serves_from_cache() {
        let server = MockServer::start().await;
        // The OAuth token endpoint is hit exactly once across both sends:
        // the second send must reuse the cached, still-fresh token.
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "minted-abc",
                "token_type": "Bearer",
                "expires_in": 3600,
            })))
            .expect(1)
            .mount(&server)
            .await;
        // Every envelope POST must carry the minted token as its bearer.
        Mock::given(method("POST"))
            .and(path("/v2.1/accounts/acct-guid/envelopes"))
            .and(header("authorization", "Bearer minted-abc"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "envelopeId": "env-1",
            })))
            .mount(&server)
            .await;

        let auth = DocuSignJwtAuth::new(
            "ik",
            "user",
            server.uri(),
            TEST_PRIV_PEM.as_bytes().to_vec(),
        );
        let provider = DocuSignSignatureProvider::with_jwt_auth(
            server.uri(),
            "acct-guid",
            auth,
            "signer@example.com",
            "Signer",
        );

        let id1 = provider
            .send_for_signature(ID1, b"%PDF-1.7 fake", &no_manifest())
            .await
            .expect("first send mints a token and creates the envelope");
        let id2 = provider
            .send_for_signature(ID2, b"%PDF-1.7 fake", &no_manifest())
            .await
            .expect("second send reuses the cached token");
        assert_eq!(id1, SignatureRequestId("env-1".into()));
        assert_eq!(id2, SignatureRequestId("env-1".into()));
        // The `.expect(1)` on the oauth mock, verified on server drop,
        // proves exactly one mint backed both sends.
    }

    #[test]
    fn docusign_envelope_body_base64s_the_pdf_and_falls_back_to_single_signer() {
        // Empty manifest → the pre-anchor behavior: one configured
        // signer, no tabs.
        let ds = docusign("https://demo.docusign.net/restapi".into());
        let body = ds.build_envelope_body(ID42, b"%PDF-1.7 fake", &no_manifest());
        assert_eq!(body["status"], "sent");
        let signers = body["recipients"]["signers"].as_array().unwrap();
        assert_eq!(signers.len(), 1);
        assert_eq!(signers[0]["email"], "signer@example.com");
        assert!(signers[0]["tabs"].is_null(), "no tabs without a manifest");
        let doc_b64 = body["documents"][0]["documentBase64"].as_str().unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(doc_b64)
            .unwrap();
        assert_eq!(decoded, b"%PDF-1.7 fake");
    }

    #[test]
    fn docusign_envelope_body_emits_anchored_tabs_and_routing_from_manifest() {
        let ds = docusign("https://demo.docusign.net/restapi".into());
        let body = ds.build_envelope_body(ID42, b"%PDF", &retainer_manifest());
        let signers = body["recipients"]["signers"].as_array().unwrap();
        assert_eq!(signers.len(), 2, "one signer per manifest recipient");

        // Client signs first (routingOrder 1) with a signHere + dateSigned tab.
        let client = &signers[0];
        assert_eq!(client["email"], "libra@example.com");
        assert_eq!(client["routingOrder"], "1");
        // Captive: the client signs embedded, so DocuSign gets a
        // clientUserId and suppresses their email.
        assert_eq!(client["clientUserId"], "notation-client");
        assert_eq!(
            client["tabs"]["signHereTabs"][0]["anchorString"],
            "nlsig-client-signature-1"
        );
        assert_eq!(
            client["tabs"]["dateSignedTabs"][0]["anchorString"],
            "nlsig-client-date-1"
        );
        assert!(
            client["tabs"]["initialHereTabs"].is_null(),
            "client placed no initials"
        );

        // Firm countersigns second (routingOrder 2) — engagement forms here.
        // Non-captive: no clientUserId, so DocuSign emails the firm a link.
        let firm = &signers[1];
        assert_eq!(firm["email"], "attorney@neonlaw.com");
        assert_eq!(firm["routingOrder"], "2");
        assert!(
            firm.get("clientUserId").is_none(),
            "the firm signer is emailed, not captive"
        );
        assert_eq!(
            firm["tabs"]["signHereTabs"][0]["anchorString"],
            "nlsig-firm-signature-1"
        );
    }

    #[tokio::test]
    async fn docusign_create_recipient_view_returns_the_signing_url() {
        // The embedded-signing path: POST views/recipient with the
        // captive recipient's identifying triple, get back a one-shot URL.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/v2.1/accounts/acct-guid/envelopes/env-1/views/recipient",
            ))
            .and(header("authorization", "Bearer TOKEN"))
            .and(body_string_contains("notation-client"))
            .and(body_string_contains("https://app.example/return"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "url": "https://demo.docusign.net/signing/abc123",
            })))
            .expect(1)
            .mount(&server)
            .await;

        let ds = docusign(server.uri());
        let url = ds
            .create_recipient_view(
                &SignatureRequestId("env-1".into()),
                &RecipientView {
                    return_url: "https://app.example/return".into(),
                    email: "libra@example.com".into(),
                    name: "Libra".into(),
                    client_user_id: "notation-client".into(),
                },
            )
            .await
            .expect("recipient view succeeds");
        assert_eq!(url, "https://demo.docusign.net/signing/abc123");
    }

    #[tokio::test]
    async fn docusign_recipient_view_maps_non_2xx_to_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/v2.1/accounts/acct-guid/envelopes/env-1/views/recipient",
            ))
            .respond_with(ResponseTemplate::new(400).set_body_string("UNKNOWN_RECIPIENT"))
            .mount(&server)
            .await;

        let ds = docusign(server.uri());
        let err = ds
            .create_recipient_view(
                &SignatureRequestId("env-1".into()),
                &RecipientView {
                    return_url: "https://app.example/return".into(),
                    email: "libra@example.com".into(),
                    name: "Libra".into(),
                    client_user_id: "notation-client".into(),
                },
            )
            .await
            .expect_err("a 400 is a provider error");
        assert!(
            err.to_string().contains("400"),
            "error surfaces status: {err}"
        );
    }

    #[tokio::test]
    async fn stub_create_recipient_view_returns_a_deterministic_url() {
        let stub = StubSignatureProvider::new();
        let url = stub
            .create_recipient_view(
                &SignatureRequestId("env-9".into()),
                &RecipientView {
                    return_url: "https://app.example/return".into(),
                    email: "libra@example.com".into(),
                    name: "Libra".into(),
                    client_user_id: "notation-client".into(),
                },
            )
            .await
            .expect("stub view succeeds");
        assert_eq!(
            url,
            "https://stub.docusign.local/signing/env-9/notation-client"
        );
    }

    #[tokio::test]
    async fn docusign_posts_bearer_authed_envelope_and_returns_envelope_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2.1/accounts/acct-guid/envelopes"))
            .and(header("authorization", "Bearer TOKEN"))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(serde_json::json!({"envelopeId": "env-789", "status": "sent"})),
            )
            .expect(1)
            .mount(&server)
            .await;

        let ds = docusign(server.uri());
        let id = ds
            .send_for_signature(ID42, b"<pdf>", &no_manifest())
            .await
            .expect("send succeeds");
        assert_eq!(id, SignatureRequestId("env-789".into()));
    }

    #[tokio::test]
    async fn docusign_posts_the_manifest_anchor_strings_to_the_wire() {
        // The contract layer: assert the anchored tab actually reaches
        // DocuSign in the POST body, not just our in-memory JSON.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2.1/accounts/acct-guid/envelopes"))
            .and(body_string_contains("nlsig-client-signature-1"))
            .and(body_string_contains("nlsig-firm-signature-1"))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(serde_json::json!({"envelopeId": "env-anchored"})),
            )
            .expect(1)
            .mount(&server)
            .await;

        let ds = docusign(server.uri());
        let id = ds
            .send_for_signature(ID42, b"<pdf>", &retainer_manifest())
            .await
            .expect("send succeeds");
        assert_eq!(id, SignatureRequestId("env-anchored".into()));
    }

    #[tokio::test]
    async fn docusign_sends_an_idempotency_key_keyed_on_the_notation() {
        // Provider-level guard: the envelope POST carries
        // X-DocuSign-Idempotency-Key = notation id, so a concurrent
        // double-send never creates two envelopes.
        let idem_key = ID42.to_string();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2.1/accounts/acct-guid/envelopes"))
            .and(header("x-docusign-idempotency-key", idem_key.as_str()))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(serde_json::json!({"envelopeId": "env-idem"})),
            )
            .expect(1)
            .mount(&server)
            .await;

        let ds = docusign(server.uri());
        let id = ds
            .send_for_signature(ID42, b"<pdf>", &no_manifest())
            .await
            .expect("send succeeds");
        assert_eq!(id, SignatureRequestId("env-idem".into()));
    }

    #[tokio::test]
    async fn docusign_maps_non_2xx_to_provider_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2.1/accounts/acct-guid/envelopes"))
            .respond_with(ResponseTemplate::new(401).set_body_string("bad token"))
            .mount(&server)
            .await;

        let ds = docusign(server.uri());
        let err = ds
            .send_for_signature(ID42, b"<pdf>", &no_manifest())
            .await
            .unwrap_err();
        match err {
            super::SignatureError::Provider(msg) => {
                assert!(msg.contains("401"), "expected 401 in error, got: {msg}");
            }
        }
    }

    #[tokio::test]
    async fn docusign_fetches_signed_pdf_and_certificate() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(
                "/v2.1/accounts/acct-guid/envelopes/env-1/documents/combined",
            ))
            .and(header("authorization", "Bearer TOKEN"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"%PDF-signed".to_vec()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(
                "/v2.1/accounts/acct-guid/envelopes/env-1/documents/certificate",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"%PDF-cert".to_vec()))
            .mount(&server)
            .await;

        let ds = docusign(server.uri());
        let docs = ds
            .fetch_completed_documents(&SignatureRequestId("env-1".into()))
            .await
            .expect("fetch succeeds");
        assert_eq!(docs.signed_pdf, b"%PDF-signed");
        assert_eq!(docs.certificate_pdf, b"%PDF-cert");
    }

    #[tokio::test]
    async fn docusign_document_download_maps_non_2xx_to_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(
                "/v2.1/accounts/acct-guid/envelopes/env-x/documents/combined",
            ))
            .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
            .mount(&server)
            .await;

        let ds = docusign(server.uri());
        let err = ds
            .fetch_completed_documents(&SignatureRequestId("env-x".into()))
            .await
            .unwrap_err();
        match err {
            super::SignatureError::Provider(msg) => {
                assert!(msg.contains("404"), "expected 404 in error, got: {msg}");
            }
        }
    }

    #[tokio::test]
    async fn stub_returns_canned_completed_documents() {
        let stub = StubSignatureProvider::new();
        let docs = stub
            .fetch_completed_documents(&SignatureRequestId("stub-1".into()))
            .await
            .expect("stub never errors");
        assert!(docs.signed_pdf.starts_with(b"%PDF"));
        assert!(docs.certificate_pdf.starts_with(b"%PDF"));
    }
}
