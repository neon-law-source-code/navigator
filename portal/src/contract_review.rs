//! ContractReviewer seam — analyze an inbound third-party contract against
//! a client's [playbook](store::playbooks) and produce a deviation report.
//!
//! This is the first review-*in* analysis step. It mirrors
//! [`crate::signature`]: a trait with two implementations selected at
//! [`crate::AppState`] build time, exactly like
//! [`crate::agent_router`]-style trait selection.
//!
//! - [`GeminiContractReviewer`] — production. Calls Vertex AI Gemini's
//!   `generateContent` with the playbook positions and the contract text,
//!   asking for a strict-JSON deviation report. Auth via Workload Identity,
//!   a GKE-metadata access token
//!   uses — no new credential.
//! - [`StubContractReviewer`] — KIND / tests. Deterministic: it flags every
//!   playbook position as a finding the attorney must act on. Not a real
//!   analysis, but it exercises the whole pipeline (intake → findings →
//!   attorney review → memo) with no LLM, since KIND has no Vertex access.
//!
//! The report reuses the persisted types — [`store::contract_reviews::Finding`]
//! and [`store::playbooks::Position`] — so there is one shape from analysis
//! through storage to the rendered memo.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use store::contract_reviews::Finding;
use store::playbooks::{Position, SEVERITY_HIGH, SEVERITY_MEDIUM};

/// The deviation report an analysis produces: a plain-language risk summary
/// plus the per-clause findings the reviewing attorney will act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewReport {
    pub risk_summary: String,
    pub findings: Vec<Finding>,
}

#[derive(Debug, thiserror::Error)]
pub enum ReviewError {
    /// No reviewer configured (should not happen — selection always falls
    /// back to the stub).
    #[error("contract reviewer not configured")]
    NotConfigured,
    /// Transport / HTTP failure (metadata server, Vertex AI).
    #[error("contract reviewer transport: {0}")]
    Transport(String),
    /// The model returned something we couldn't parse into a report.
    #[error("contract reviewer returned invalid response: {0}")]
    InvalidResponse(String),
}

#[async_trait]
pub trait ContractReviewer: Send + Sync {
    /// Review `contract_text` against the client's playbook `positions`
    /// (labelled `playbook_name`) and return the deviation report. Every
    /// finding comes back with `accepted = false` — nothing is accepted
    /// until the reviewing attorney acts.
    ///
    /// # Errors
    ///
    /// Returns [`ReviewError`] on transport or parse failure.
    async fn review(
        &self,
        playbook_name: &str,
        positions: &[Position],
        contract_text: &str,
    ) -> Result<ReviewReport, ReviewError>;
}

// ---------------------------------------------------------------------------
// StubContractReviewer — deterministic, no LLM. KIND / tests.
// ---------------------------------------------------------------------------

/// Deterministic reviewer used where Vertex is unavailable (KIND, tests).
/// Flags each playbook position as a finding to review, deriving the
/// severity and a fallback redline from the position itself.
#[derive(Debug, Default)]
pub struct StubContractReviewer;

#[async_trait]
impl ContractReviewer for StubContractReviewer {
    async fn review(
        &self,
        playbook_name: &str,
        positions: &[Position],
        contract_text: &str,
    ) -> Result<ReviewReport, ReviewError> {
        let findings: Vec<Finding> = positions
            .iter()
            .enumerate()
            .map(|(i, p)| Finding {
                clause_ref: format!("Position {}: {}", i + 1, p.topic),
                deviation: format!(
                    "Automated screen flagged \"{}\" for attorney review against the \
                     walk-away line: {}",
                    p.topic, p.walkaway
                ),
                severity: p.severity.clone(),
                suggested_redline: Some(p.fallback.clone()),
                attorney_note: None,
                accepted: false,
            })
            .collect();
        let high = findings
            .iter()
            .filter(|f| f.severity == SEVERITY_HIGH)
            .count();
        let medium = findings
            .iter()
            .filter(|f| f.severity == SEVERITY_MEDIUM)
            .count();
        let risk_summary = format!(
            "Automated screen against playbook \"{playbook_name}\" over {chars} characters \
             of contract text: {total} position(s) flagged for attorney review \
             ({high} high, {medium} medium). This is a deterministic placeholder screen, \
             not legal analysis — every finding requires attorney judgment.",
            chars = contract_text.chars().count(),
            total = findings.len(),
        );
        Ok(ReviewReport {
            risk_summary,
            findings,
        })
    }
}

// ---------------------------------------------------------------------------
// GeminiContractReviewer — Vertex AI Gemini via Workload Identity.
// ---------------------------------------------------------------------------

/// Vertex AI–backed contract reviewer. Reuses the same region/model/auth
/// shape Vertex AI expects.
pub struct GeminiContractReviewer {
    project_id: String,
    location: String,
    model: String,
    auth: Auth,
    vertex_base_url: String,
    http: reqwest::Client,
}

/// Default GKE metadata-server URL for a Workload Identity access
/// token. The metadata server lives on a link-local address inside
/// every GKE pod and the `Metadata-Flavor: Google` header is
/// mandatory. Overridable via `GOOGLE_METADATA_URL` so tests can point
/// it at a mock.
pub const DEFAULT_METADATA_TOKEN_URL: &str =
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";

/// Vertex AI region. Must be one where the chosen model is served.
pub const DEFAULT_VERTEX_LOCATION: &str = "us-west4";

/// Vertex AI model backing the review.
pub const DEFAULT_VERTEX_MODEL: &str = "gemini-2.5-flash";

/// Where the Vertex access token comes from: the GKE metadata server in
/// production, or a baked-in string in tests.
enum Auth {
    Metadata(String),
    Static(String),
}

impl GeminiContractReviewer {
    /// Production constructor. `None` when `NAVIGATOR_GCP_PROJECT_ID` is
    /// unset — caller falls back to [`StubContractReviewer`].
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let project_id = std::env::var("NAVIGATOR_GCP_PROJECT_ID").ok()?;
        if project_id.is_empty() {
            return None;
        }
        let location = std::env::var("NAVIGATOR_GCP_LOCATION")
            .unwrap_or_else(|_| DEFAULT_VERTEX_LOCATION.to_string());
        let model = std::env::var("NAVIGATOR_CONTRACT_REVIEW_MODEL")
            .unwrap_or_else(|_| DEFAULT_VERTEX_MODEL.to_string());
        let metadata_url = std::env::var("GOOGLE_METADATA_URL")
            .unwrap_or_else(|_| DEFAULT_METADATA_TOKEN_URL.to_string());
        let vertex_base_url = format!("https://{location}-aiplatform.googleapis.com");
        Some(Self {
            project_id,
            location,
            model,
            auth: Auth::Metadata(metadata_url),
            vertex_base_url,
            http: reqwest::Client::new(),
        })
    }

    /// Test constructor — static token, Vertex base URL pointed at a
    /// wiremock server.
    #[must_use]
    pub fn for_test(
        project_id: impl Into<String>,
        location: impl Into<String>,
        model: impl Into<String>,
        static_token: impl Into<String>,
        vertex_base_url: impl Into<String>,
    ) -> Self {
        Self {
            project_id: project_id.into(),
            location: location.into(),
            model: model.into(),
            auth: Auth::Static(static_token.into()),
            vertex_base_url: vertex_base_url.into(),
            http: reqwest::Client::new(),
        }
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/v1/projects/{}/locations/{}/publishers/google/models/{}:generateContent",
            self.vertex_base_url, self.project_id, self.location, self.model
        )
    }

    async fn token(&self) -> Result<String, ReviewError> {
        match &self.auth {
            Auth::Static(t) => Ok(t.clone()),
            Auth::Metadata(url) => {
                let resp = self
                    .http
                    .get(url)
                    .header("Metadata-Flavor", "Google")
                    .send()
                    .await
                    .map_err(|e| ReviewError::Transport(format!("metadata GET: {e}")))?;
                let status = resp.status();
                let bytes = resp
                    .bytes()
                    .await
                    .map_err(|e| ReviewError::Transport(format!("metadata body: {e}")))?;
                if !status.is_success() {
                    return Err(ReviewError::Transport(format!("metadata http {status}")));
                }
                let parsed: MetadataToken = serde_json::from_slice(&bytes)
                    .map_err(|e| ReviewError::InvalidResponse(format!("metadata parse: {e}")))?;
                Ok(parsed.access_token)
            }
        }
    }
}

#[async_trait]
impl ContractReviewer for GeminiContractReviewer {
    async fn review(
        &self,
        playbook_name: &str,
        positions: &[Position],
        contract_text: &str,
    ) -> Result<ReviewReport, ReviewError> {
        let token = self.token().await?;
        let prompt = build_prompt(playbook_name, positions, contract_text);
        let body = json!({
            "contents": [{ "role": "user", "parts": [{ "text": prompt }] }],
            // Force strict JSON back so parsing is deterministic.
            "generationConfig": { "responseMimeType": "application/json" },
            "systemInstruction": { "parts": [{ "text": SYSTEM_INSTRUCTION }] }
        });
        let resp = self
            .http
            .post(self.endpoint())
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ReviewError::Transport(format!("vertex POST: {e}")))?;
        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| ReviewError::Transport(format!("vertex body: {e}")))?;
        if !status.is_success() {
            return Err(ReviewError::Transport(format!(
                "vertex http {status}: {body}",
                body = String::from_utf8_lossy(&bytes)
            )));
        }
        let parsed: VertexResponse = serde_json::from_slice(&bytes).map_err(|e| {
            ReviewError::InvalidResponse(format!(
                "vertex response parse: {e}; body={body}",
                body = String::from_utf8_lossy(&bytes)
            ))
        })?;
        let text = parsed.first_text().ok_or_else(|| {
            ReviewError::InvalidResponse("vertex response had no text part".to_string())
        })?;
        let report: WireReport = serde_json::from_str(&text)
            .map_err(|e| ReviewError::InvalidResponse(format!("report JSON parse: {e}")))?;
        Ok(report.into_report())
    }
}

/// System instruction: the model is the firm's contract screener. It must
/// return strict JSON and never approve a clause — only flag deviations for
/// the attorney. `accepted` is always false in its output.
const SYSTEM_INSTRUCTION: &str =
    "You are a contract-review assistant for a law firm. Compare the inbound contract \
     against the client's playbook positions and return ONLY a JSON object with keys \
     `risk_summary` (string) and `findings` (array). Each finding has `clause_ref` \
     (string, where in the contract), `deviation` (string, how it departs from the \
     playbook), `severity` (one of `low`, `medium`, `high`), and `suggested_redline` \
     (string or null). Flag deviations only — never state that a clause is acceptable, \
     and never set anything as accepted; a licensed attorney makes that call. If a \
     position is not addressed by the contract, that silence may itself be a finding.";

fn build_prompt(playbook_name: &str, positions: &[Position], contract_text: &str) -> String {
    let positions_json =
        serde_json::to_string_pretty(positions).unwrap_or_else(|_| "[]".to_string());
    format!(
        "Client playbook \"{playbook_name}\" positions (JSON):\n{positions_json}\n\n\
         Inbound contract text:\n{contract_text}\n\n\
         Return the JSON deviation report now."
    )
}

#[derive(Debug, Deserialize)]
struct MetadataToken {
    access_token: String,
}

#[derive(Debug, Deserialize)]
struct VertexResponse {
    #[serde(default)]
    candidates: Vec<VertexCandidate>,
}

#[derive(Debug, Deserialize)]
struct VertexCandidate {
    content: VertexContent,
}

#[derive(Debug, Deserialize)]
struct VertexContent {
    #[serde(default)]
    parts: Vec<VertexPart>,
}

#[derive(Debug, Deserialize)]
struct VertexPart {
    #[serde(default)]
    text: Option<String>,
}

impl VertexResponse {
    fn first_text(self) -> Option<String> {
        self.candidates
            .into_iter()
            .flat_map(|c| c.content.parts)
            .find_map(|p| p.text)
    }
}

/// The model's JSON, before we normalise it into a [`ReviewReport`]. We
/// re-stamp `accepted = false` regardless of what the model says, so the
/// "nothing accepted until the attorney acts" invariant holds at the seam.
#[derive(Debug, Deserialize)]
struct WireReport {
    #[serde(default)]
    risk_summary: String,
    #[serde(default)]
    findings: Vec<WireFinding>,
}

#[derive(Debug, Deserialize)]
struct WireFinding {
    #[serde(default)]
    clause_ref: String,
    #[serde(default)]
    deviation: String,
    #[serde(default)]
    severity: String,
    #[serde(default)]
    suggested_redline: Option<String>,
}

impl WireReport {
    fn into_report(self) -> ReviewReport {
        let findings = self
            .findings
            .into_iter()
            .map(|f| Finding {
                clause_ref: f.clause_ref,
                deviation: f.deviation,
                severity: normalise_severity(&f.severity),
                suggested_redline: f.suggested_redline,
                attorney_note: None,
                accepted: false,
            })
            .collect();
        ReviewReport {
            risk_summary: self.risk_summary,
            findings,
        }
    }
}

/// Map a model-supplied severity onto a known value, defaulting unknown
/// strings to `medium` so a finding is never silently dropped.
fn normalise_severity(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        store::playbooks::SEVERITY_LOW => store::playbooks::SEVERITY_LOW.to_string(),
        store::playbooks::SEVERITY_HIGH => store::playbooks::SEVERITY_HIGH.to_string(),
        _ => store::playbooks::SEVERITY_MEDIUM.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use store::playbooks::SEVERITY_HIGH;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn positions() -> Vec<Position> {
        vec![
            Position {
                topic: "Limitation of liability".into(),
                preferred: "Mutual cap at 12 months' fees".into(),
                fallback: "Cap at 24 months' fees".into(),
                walkaway: "Uncapped liability".into(),
                severity: SEVERITY_HIGH.into(),
            },
            Position {
                topic: "Auto-renewal".into(),
                preferred: "No auto-renewal".into(),
                fallback: "Auto-renewal with 60-day notice".into(),
                walkaway: "Auto-renewal with no exit".into(),
                severity: store::playbooks::SEVERITY_MEDIUM.into(),
            },
        ]
    }

    #[tokio::test]
    async fn stub_flags_every_position_deterministically() {
        let r = StubContractReviewer;
        let report = r
            .review("SaaS vendor MSA", &positions(), "some contract text")
            .await
            .unwrap();
        assert_eq!(report.findings.len(), 2);
        // Severity carries through from the position.
        assert_eq!(report.findings[0].severity, SEVERITY_HIGH);
        // Nothing is accepted out of analysis.
        assert!(report.findings.iter().all(|f| !f.accepted));
        // Fallback becomes the suggested redline.
        assert_eq!(
            report.findings[0].suggested_redline.as_deref(),
            Some("Cap at 24 months' fees")
        );
        assert!(report.risk_summary.contains("1 high"));
        // Deterministic: same inputs, same output.
        let again = r
            .review("SaaS vendor MSA", &positions(), "some contract text")
            .await
            .unwrap();
        assert_eq!(report, again);
    }

    #[tokio::test]
    async fn gemini_parses_a_json_report() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/v1/projects/p/locations/us-west4/publishers/google/models/\
                 {DEFAULT_VERTEX_MODEL}:generateContent"
            )))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{
                    "content": {
                        "parts": [{
                            "text": "{\"risk_summary\":\"One high-severity deviation.\",\
                                \"findings\":[{\"clause_ref\":\"§7.2\",\"deviation\":\"Uncapped liability.\",\
                                \"severity\":\"HIGH\",\"suggested_redline\":\"Add a mutual cap.\",\"accepted\":true}]}"
                        }]
                    }
                }]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let r = GeminiContractReviewer::for_test(
            "p",
            "us-west4",
            DEFAULT_VERTEX_MODEL,
            "test-token",
            server.uri(),
        );
        let report = r
            .review("SaaS vendor MSA", &positions(), "contract")
            .await
            .unwrap();
        assert_eq!(report.risk_summary, "One high-severity deviation.");
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].clause_ref, "§7.2");
        // Severity normalised from "HIGH".
        assert_eq!(report.findings[0].severity, SEVERITY_HIGH);
        // The seam re-stamps accepted=false even though the model said true.
        assert!(!report.findings[0].accepted);
    }

    #[tokio::test]
    async fn gemini_surfaces_transport_error_on_5xx() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503).set_body_string("down"))
            .expect(1)
            .mount(&server)
            .await;
        let r = GeminiContractReviewer::for_test(
            "p",
            "us-west4",
            DEFAULT_VERTEX_MODEL,
            "t",
            server.uri(),
        );
        let err = r.review("pb", &positions(), "contract").await.unwrap_err();
        assert!(matches!(err, ReviewError::Transport(_)), "got {err:?}");
    }
}
