#![allow(clippy::doc_markdown)] // DNS prose is dense with acronyms (DKIM, DMARC, SPF, CNAME, DNSimple, SendGrid).
//! DNS provider abstraction + DNSimple implementation.
//!
//! `ops dns setup` provisions the full public-deploy record set for a
//! domain — reachability, the apex→www redirect, and both mail lanes
//! (human mail on the apex, application mail on a `parse.` subdomain) —
//! behind the [`DnsProvider`] trait so a cutover (Cloud DNS, Route 53)
//! drops in without touching the orchestration layer.
//!
//! ## What we ensure
//!
//! Every record group is opt-in via a flag, and each is idempotent
//! (existing matching record → no-op, single-valued drift → patched in
//! place, missing → created). We never delete: a record the command
//! does not know about is left alone.
//!
//! | Group                | Records                                             | Flag                         |
//! |----------------------|-----------------------------------------------------|------------------------------|
//! | Reachability         | `www` / `workflows` `A` → gateway IP                | `--gateway-ip`               |
//! | Apex→www redirect    | apex `URL` record → `https://www.<zone>`            | `--redirect-apex-to-www`     |
//! | Human mail           | apex `MX` `smtp.google.com` + SPF `_spf.google.com` | `--google-workspace`         |
//! | Domain verification  | apex `google-site-verification` `TXT`               | `--google-site-verification` |
//! | Application inbound   | `parse.` `MX` `mx.sendgrid.net` + SPF `sendgrid.net`| `--sendgrid`                |
//! | Application outbound | `s{1,2}._domainkey` DKIM + link-branding `CNAME`s   | `--dkim-target` / `--sendgrid-link-brand` |
//! | DMARC                | `_dmarc` `TXT`                                       | `--dmarc`                    |
//!
//! The apex→www redirect is a DNSimple `URL` record: DNSimple's
//! redirector answers the bare domain with a 301 to `https://www.<zone>`,
//! so the GKE ingress only ever serves `www` (and `workflows`). The
//! redirect serves HTTPS only once a certificate exists for the apex —
//! DNSimple does not auto-issue one for a `URL` record — so a public
//! deploy pairs `--redirect-apex-to-www` with a Let's Encrypt certificate
//! for the domain (issued from DNSimple's certificate API; see
//! `docs/dns.md`). The command prints that reminder after it runs.
//!
//! The SendGrid DKIM/link-branding targets and the site-verification
//! token are issued per-domain by SendGrid's Domain Authentication
//! wizard and Google's Admin console; the command takes them as flags —
//! it never invents them. The Google Workspace routing rule that
//! forwards `support@` into SendGrid Inbound Parse is a mail-provider
//! setting, not a DNS record; see `docs/dns.md`.
//!
//! ## Auth
//!
//! DNSimple v2 uses a Personal Access Token as `Authorization: Bearer …`.
//! Three env vars — the first two provider-agnostic, the token
//! DNSimple-specific:
//!
//! - `DNS_ZONE` — the domain to configure (or pass `--domain`)
//! - `DNS_ACCT` — numeric account id (look it up once with
//!   `dnsimple accounts list`)
//! - `DNS_SIMPLE` — the bearer token (an operator-local credential)
//! - `DNSIMPLE_API_TOKEN` — legacy alias for forks
//!
//! Tests stand up a `wiremock` server and point a constructor override
//! at it — no real HTTP, no real account.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// DNS record types we manipulate. We don't need the full enumeration —
/// just the five that drive a public deploy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordType {
    A,
    Aaaa,
    Mx,
    Txt,
    Cname,
    /// DNSimple `URL` record — the apex→www redirect. `content` is the
    /// target URL; DNSimple's redirector 301s the name to it.
    Url,
}

impl RecordType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::Aaaa => "AAAA",
            Self::Mx => "MX",
            Self::Txt => "TXT",
            Self::Cname => "CNAME",
            Self::Url => "URL",
        }
    }

    /// Parse a DNSimple record `type` string, or `None` for a type we
    /// don't manage (`SOA`, `NS`, `DNSKEY`, …).
    #[must_use]
    pub fn from_api(raw: &str) -> Option<Self> {
        match raw {
            "A" => Some(Self::A),
            "AAAA" => Some(Self::Aaaa),
            "MX" => Some(Self::Mx),
            "TXT" => Some(Self::Txt),
            "CNAME" => Some(Self::Cname),
            "URL" => Some(Self::Url),
            _ => None,
        }
    }
}

/// DMARC policy for the `_dmarc` `TXT` record. Rendered lowercase into
/// `p=<policy>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum DmarcPolicy {
    None,
    Quarantine,
    Reject,
}

impl DmarcPolicy {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Quarantine => "quarantine",
            Self::Reject => "reject",
        }
    }
}

/// A desired DNS record. `name` is the relative-to-zone label
/// (`""` for root, `"_dmarc"` for the DMARC TXT). `priority` is
/// honored only for MX.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredRecord {
    pub record_type: RecordType,
    pub name: String,
    pub content: String,
    pub priority: Option<u32>,
    pub ttl: u32,
}

/// What reconciling a record did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnsureOutcome {
    Created,
    Updated,
    Unchanged,
}

#[derive(Debug, Error)]
pub enum DnsError {
    #[error("http error: {0}")]
    Http(String),
    #[error("unexpected status {status} from {url}: {body}")]
    Status {
        status: u16,
        url: String,
        body: String,
    },
    #[error("missing env var: {0}")]
    MissingEnv(&'static str),
    #[error("conflicting records at {name}: {message}")]
    Conflict { name: String, message: String },
}

/// Pluggable DNS backend. The two write methods plus a list cover the
/// entire surface [`run_setup`] needs: list (for idempotency) and write.
#[async_trait]
pub trait DnsProvider: Send + Sync {
    /// List every record currently in the zone. The caller filters.
    async fn list_records(&self, zone: &str) -> Result<Vec<ExistingRecord>, DnsError>;

    /// Create a record. Returns the new record's provider-assigned id.
    async fn create_record(&self, zone: &str, record: &DesiredRecord) -> Result<u64, DnsError>;

    /// Update an existing record's content (and, for MX, priority).
    async fn update_record(
        &self,
        zone: &str,
        record_id: u64,
        record: &DesiredRecord,
    ) -> Result<(), DnsError>;
}

/// A record as returned by the provider. Carries enough to decide
/// "same as desired" or "drifted, needs update".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingRecord {
    pub id: u64,
    pub record_type: RecordType,
    pub name: String,
    pub content: String,
    pub priority: Option<u32>,
}

/// Normalize record content for comparison. DNSimple returns `TXT`
/// content wrapped in double quotes; we send it unquoted, so strip the
/// wrapper before comparing or every run would look like drift.
fn normalize(record_type: RecordType, content: &str) -> &str {
    if record_type == RecordType::Txt {
        content.trim_matches('"')
    } else {
        content
    }
}

/// True when an existing record already satisfies a desired one.
fn matches(existing: &ExistingRecord, desired: &DesiredRecord) -> bool {
    normalize(existing.record_type, &existing.content)
        == normalize(desired.record_type, &desired.content)
        && existing.priority == desired.priority
}

fn is_spf(record_type: RecordType, content: &str) -> bool {
    record_type == RecordType::Txt && normalize(record_type, content).starts_with("v=spf1 ")
}

/// True when two desired records are the same `(type, name, content,
/// priority)` — used to skip duplicates within one run.
fn same_desired(a: &DesiredRecord, b: &DesiredRecord) -> bool {
    a.record_type == b.record_type
        && a.name == b.name
        && normalize(a.record_type, &a.content) == normalize(b.record_type, &b.content)
        && a.priority == b.priority
}

/// One line of the [`run_setup`] report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnsureReport {
    pub record_type: RecordType,
    pub name: String,
    pub content: String,
    pub outcome: EnsureOutcome,
}

/// Reconcile a desired record set against the zone, additively.
///
/// Idempotency is per `(type, name)` group. A group with a single
/// desired record is *single-valued*: an existing record with the same
/// `(type, name)` but drifted content is patched in place. A group with
/// several desired records (the apex `A`/`AAAA` sets, the two apex
/// `TXT`s) is *multi-valued*: each missing content is created and
/// existing extras are left untouched. Nothing is ever deleted.
pub async fn run_setup(
    provider: &dyn DnsProvider,
    zone: &str,
    desired: &[DesiredRecord],
) -> Result<Vec<EnsureReport>, DnsError> {
    let existing = provider.list_records(zone).await?;
    let mut reports = Vec::with_capacity(desired.len());
    // Records already handled this run — the `existing` snapshot is fetched
    // once and never refreshed, so a duplicate in `desired` would otherwise
    // create a second identical record instead of no-op'ing.
    let mut applied: Vec<&DesiredRecord> = Vec::new();
    for record in desired {
        // Address records cannot safely coexist with a CNAME or DNSimple URL
        // redirect at the same owner name. This matters during migrations:
        // neonlaw.com historically carried `www URL`, while the GKE ingress
        // needs `www A`. The reconciler is intentionally additive, so refuse
        // and tell the operator to remove the old redirect rather than report
        // a deployable host that still redirects elsewhere.
        if matches!(record.record_type, RecordType::A | RecordType::Aaaa) {
            let conflicting: Vec<&str> = existing
                .iter()
                .filter(|e| {
                    e.name == record.name
                        && matches!(e.record_type, RecordType::Cname | RecordType::Url)
                })
                .map(|e| e.record_type.as_str())
                .collect();
            if !conflicting.is_empty() {
                return Err(DnsError::Conflict {
                    name: if record.name.is_empty() {
                        "the apex".to_string()
                    } else {
                        record.name.clone()
                    },
                    message: format!(
                        "cannot add an {} record where {} record(s) already exist — delete them \
                         first (see docs/dns.md)",
                        record.record_type.as_str(),
                        conflicting.join("/")
                    ),
                });
            }
        }
        // A `URL` record (DNSimple's redirector) cannot coexist with
        // address records on the same name: DNSimple rejects the pair, and
        // a stale forwarding `A`/`AAAA` left from a prior setup would keep
        // answering instead of the redirect. `run_setup` never deletes, so
        // surface the conflict as an actionable error rather than creating
        // a broken apex — migrating from address-based forwarding means
        // removing those records first (see `docs/dns.md`).
        if record.record_type == RecordType::Url {
            let conflicting: Vec<&str> = existing
                .iter()
                .filter(|e| {
                    e.name == record.name
                        && matches!(
                            e.record_type,
                            RecordType::A | RecordType::Aaaa | RecordType::Cname
                        )
                })
                .map(|e| e.record_type.as_str())
                .collect();
            if !conflicting.is_empty() {
                return Err(DnsError::Conflict {
                    name: if record.name.is_empty() {
                        "the apex".to_string()
                    } else {
                        record.name.clone()
                    },
                    message: format!(
                        "cannot add a URL redirect where {} record(s) already exist — delete them \
                         first (see docs/dns.md)",
                        conflicting.join("/")
                    ),
                });
            }
        }
        let here: Vec<&ExistingRecord> = existing
            .iter()
            .filter(|e| e.record_type == record.record_type && e.name == record.name)
            .collect();
        let outcome = if applied.iter().any(|d| same_desired(d, record))
            || here.iter().any(|e| matches(e, record))
        {
            EnsureOutcome::Unchanged
        } else {
            let group_size = desired
                .iter()
                .filter(|d| d.record_type == record.record_type && d.name == record.name)
                .count();
            if is_spf(record.record_type, &record.content) {
                if let Some(existing_spf) = here.iter().find(|e| is_spf(e.record_type, &e.content))
                {
                    provider
                        .update_record(zone, existing_spf.id, record)
                        .await?;
                    EnsureOutcome::Updated
                } else {
                    provider.create_record(zone, record).await?;
                    EnsureOutcome::Created
                }
            } else if group_size == 1 && !here.is_empty() {
                provider.update_record(zone, here[0].id, record).await?;
                EnsureOutcome::Updated
            } else {
                provider.create_record(zone, record).await?;
                EnsureOutcome::Created
            }
        };
        applied.push(record);
        reports.push(EnsureReport {
            record_type: record.record_type,
            name: record.name.clone(),
            content: record.content.clone(),
            outcome,
        });
    }
    Ok(reports)
}

// --- The record set -----------------------------------------------

/// Google Workspace's single inbound mail exchanger.
pub const WORKSPACE_MX: &str = "smtp.google.com";
/// SendGrid Inbound Parse's mail exchanger (lives on the `parse.` subdomain).
pub const SENDGRID_PARSE_MX: &str = "mx.sendgrid.net";

/// Which record groups to provision, and the per-domain values that
/// only the operator can supply.
#[derive(Debug, Clone, Default)]
pub struct DnsSetupConfig {
    /// `A` records for `hosts` (default `www`, `workflows`) → this IP.
    pub gateway_ip: Option<String>,
    /// Host labels to point at `gateway_ip`. Empty → `www` + `workflows`.
    pub hosts: Vec<String>,
    /// Apex `URL` record → `https://www.<zone>` (DNSimple redirector).
    pub redirect_apex_to_www: bool,
    /// Apex `MX` `smtp.google.com` + `_spf.google.com` in the SPF record.
    pub google_workspace: bool,
    /// Apex `google-site-verification=<token>` `TXT`.
    pub google_site_verification: Option<String>,
    /// `parse.` `MX` `mx.sendgrid.net` + `sendgrid.net` in the SPF record.
    pub sendgrid: bool,
    /// SendGrid DKIM targets → `s1._domainkey`, `s2._domainkey`, … in order.
    pub dkim_targets: Vec<String>,
    /// SendGrid link-branding CNAMEs as `(label, target)`.
    pub sendgrid_link_brand: Vec<(String, String)>,
    /// Extra SPF `include:` mechanisms (e.g. `amazonses.com`), ordered
    /// between the Google and SendGrid includes.
    pub spf_includes: Vec<String>,
    /// DMARC policy. `Some` → a `_dmarc` `TXT` is emitted.
    pub dmarc: Option<DmarcPolicy>,
    /// DMARC aggregate-report address. Default `mailto:postmaster@<zone>`.
    pub dmarc_rua: Option<String>,
}

impl DnsSetupConfig {
    /// Reject flag combinations that silently produce no record. `--host`
    /// labels have nothing to point at without `--gateway-ip`, so that
    /// pairing is an error rather than a no-op.
    pub fn validate(&self) -> Result<(), String> {
        if !self.hosts.is_empty() && self.gateway_ip.is_none() {
            return Err(
                "--host requires --gateway-ip (host labels need an IP to point at)".to_string(),
            );
        }
        Ok(())
    }
}

fn record(
    record_type: RecordType,
    name: &str,
    content: String,
    priority: Option<u32>,
    ttl: u32,
) -> DesiredRecord {
    DesiredRecord {
        record_type,
        name: name.to_string(),
        content,
        priority,
        ttl,
    }
}

/// Translate the operator's spelling of the apex into the provider's.
///
/// DNSimple names the apex with the **empty** string, but an empty
/// `--host ""` does not survive the command line — the shell and clap
/// between them drop it, so the run silently produced no record at all
/// rather than an error. `@` is how a zone file and every DNS tool spells
/// the apex, so that is what the flag accepts, and it is translated here so
/// the internal model keeps the provider's own convention.
fn apex_label(host: &str) -> &str {
    if host == "@" {
        ""
    } else {
        host
    }
}

/// Build the full desired record set for `zone` from `config`. The order
/// mirrors the groups in the module table; only enabled groups appear.
#[must_use]
pub fn desired_records(zone: &str, config: &DnsSetupConfig) -> Vec<DesiredRecord> {
    let mut out = Vec::new();

    // Reachability.
    if let Some(ip) = &config.gateway_ip {
        let default_hosts = ["www".to_string(), "workflows".to_string()];
        let hosts: &[String] = if config.hosts.is_empty() {
            &default_hosts
        } else {
            &config.hosts
        };
        for host in hosts {
            out.push(record(
                RecordType::A,
                apex_label(host),
                ip.clone(),
                None,
                300,
            ));
        }
    }

    // Apex → www redirect via a DNSimple URL record. The redirector 301s
    // the bare domain to `https://www.<zone>`, keeping the GKE ingress a
    // single `www` entry point. HTTPS needs a cert for the apex — issued
    // separately (see the module docs and `docs/dns.md`).
    if config.redirect_apex_to_www {
        out.push(record(
            RecordType::Url,
            "",
            format!("https://www.{zone}"),
            None,
            300,
        ));
    }

    // Human mail — Google Workspace at the apex.
    if config.google_workspace {
        out.push(record(
            RecordType::Mx,
            "",
            WORKSPACE_MX.to_string(),
            Some(1),
            3600,
        ));
    }
    if let Some(token) = &config.google_site_verification {
        out.push(record(
            RecordType::Txt,
            "",
            format!("google-site-verification={token}"),
            None,
            300,
        ));
    }

    // Application inbound — SendGrid Inbound Parse on the `parse.` subdomain.
    if config.sendgrid {
        out.push(record(
            RecordType::Mx,
            "parse",
            SENDGRID_PARSE_MX.to_string(),
            Some(10),
            3600,
        ));
    }

    // Application outbound — SendGrid sender authentication.
    for (idx, target) in config.dkim_targets.iter().enumerate() {
        out.push(record(
            RecordType::Cname,
            &format!("s{}._domainkey", idx + 1),
            target.clone(),
            None,
            3600,
        ));
    }
    for (label, target) in &config.sendgrid_link_brand {
        out.push(record(RecordType::Cname, label, target.clone(), None, 3600));
    }

    // SPF — one record authorizing every enabled sender, in order.
    let mut includes = Vec::new();
    if config.google_workspace {
        includes.push("_spf.google.com".to_string());
    }
    includes.extend(config.spf_includes.iter().cloned());
    if config.sendgrid {
        includes.push("sendgrid.net".to_string());
    }
    if !includes.is_empty() {
        let mechanisms: Vec<String> = includes.iter().map(|i| format!("include:{i}")).collect();
        out.push(record(
            RecordType::Txt,
            "",
            format!("v=spf1 {} -all", mechanisms.join(" ")),
            None,
            3600,
        ));
    }

    // DMARC.
    if let Some(policy) = config.dmarc {
        let rua = config
            .dmarc_rua
            .clone()
            .unwrap_or_else(|| format!("mailto:postmaster@{zone}"));
        out.push(record(
            RecordType::Txt,
            "_dmarc",
            format!("v=DMARC1; p={}; rua={rua}", policy.as_str()),
            None,
            3600,
        ));
    }

    out
}

// --- DNSimple implementation --------------------------------------

/// DNSimple v2 base URL. Override in tests via
/// [`DnsimpleProvider::with_base_url`].
pub const DNSIMPLE_BASE_URL: &str = "https://api.dnsimple.com";

/// Production HTTP client for DNSimple v2.
#[derive(Clone)]
pub struct DnsimpleProvider {
    http: reqwest::Client,
    api_token: String,
    account_id: String,
    base_url: String,
    dry_run: bool,
    recorded: Arc<Mutex<Vec<RecordedCall>>>,
}

/// Recorded request (dry-run audit log).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedCall {
    pub method: &'static str,
    pub url: String,
    pub body: Option<String>,
}

impl DnsimpleProvider {
    /// Production constructor. Bearer token + numeric account id.
    #[must_use]
    pub fn new(api_token: impl Into<String>, account_id: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_token: api_token.into(),
            account_id: account_id.into(),
            base_url: DNSIMPLE_BASE_URL.into(),
            dry_run: false,
            recorded: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Load from `DNS_SIMPLE` + `DNS_ACCT`; retain `DNSIMPLE_API_TOKEN` as a
    /// backwards-compatible alias for existing forks.
    pub fn from_env() -> Result<Self, DnsError> {
        let api_token = std::env::var("DNS_SIMPLE")
            .or_else(|_| std::env::var("DNSIMPLE_API_TOKEN"))
            .map_err(|_| DnsError::MissingEnv("DNS_SIMPLE (or DNSIMPLE_API_TOKEN)"))?;
        let account_id = std::env::var("DNS_ACCT").map_err(|_| DnsError::MissingEnv("DNS_ACCT"))?;
        Ok(Self::new(api_token, account_id))
    }

    /// Override the base URL — tests only.
    #[cfg(test)]
    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Switch into dry-run mode. List remains live (we need the real
    /// state to decide create/update), but `create_record` /
    /// `update_record` are recorded instead of sent.
    #[must_use]
    pub fn with_dry_run(mut self) -> Self {
        self.dry_run = true;
        self
    }

    /// Snapshot of dry-run records.
    #[must_use]
    pub fn recorded_calls(&self) -> Vec<RecordedCall> {
        self.recorded
            .lock()
            .expect("recorded lock poisoned")
            .clone()
    }

    fn url(&self, path: &str) -> String {
        format!("{}/v2/{}{path}", self.base_url, self.account_id)
    }

    fn record(&self, method: &'static str, url: &str, body: Option<String>) {
        tracing::info!(
            target: "devx::dns::dry_run",
            method = method,
            url = url,
            body = body.as_deref().unwrap_or(""),
            "[dry-run] would call DNSimple",
        );
        self.recorded
            .lock()
            .expect("recorded lock poisoned")
            .push(RecordedCall {
                method,
                url: url.to_string(),
                body,
            });
    }
}

#[derive(Debug, Deserialize)]
struct DnsimpleListResponse {
    data: Vec<DnsimpleRecord>,
    #[serde(default)]
    pagination: Option<DnsimplePagination>,
}

#[derive(Debug, Deserialize)]
struct DnsimplePagination {
    total_pages: u32,
}

#[derive(Debug, Deserialize)]
struct DnsimpleSingleResponse {
    data: DnsimpleRecord,
}

#[derive(Debug, Deserialize)]
struct DnsimpleRecord {
    id: u64,
    #[serde(rename = "type")]
    record_type: String,
    name: String,
    content: String,
    #[serde(default)]
    priority: Option<u32>,
}

#[derive(Debug, Serialize)]
struct DnsimpleWriteBody<'a> {
    name: &'a str,
    #[serde(rename = "type")]
    record_type: &'a str,
    content: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    priority: Option<u32>,
    ttl: u32,
}

#[async_trait]
impl DnsProvider for DnsimpleProvider {
    async fn list_records(&self, zone: &str) -> Result<Vec<ExistingRecord>, DnsError> {
        // Page through the whole zone: reconciliation (and the apex
        // URL/address conflict guard) must see every record, not just the
        // first 100. DNSimple caps `per_page` at 100 and reports
        // `pagination.total_pages`.
        let mut out = Vec::new();
        let mut page = 1u32;
        loop {
            let url = self.url(&format!("/zones/{zone}/records?per_page=100&page={page}"));
            let resp = self
                .http
                .get(&url)
                .bearer_auth(&self.api_token)
                .header("Accept", "application/json")
                .send()
                .await
                .map_err(|e| DnsError::Http(e.to_string()))?;
            let status = resp.status().as_u16();
            let body_bytes = resp
                .bytes()
                .await
                .map_err(|e| DnsError::Http(e.to_string()))?;
            if !(200..300).contains(&status) {
                return Err(DnsError::Status {
                    status,
                    url,
                    body: String::from_utf8_lossy(&body_bytes).into_owned(),
                });
            }
            let parsed: DnsimpleListResponse = serde_json::from_slice(&body_bytes)
                .map_err(|e| DnsError::Http(format!("decode list response: {e}")))?;
            out.extend(parsed.data.into_iter().filter_map(|r| {
                let rt = RecordType::from_api(&r.record_type)?;
                Some(ExistingRecord {
                    id: r.id,
                    record_type: rt,
                    name: r.name,
                    content: r.content,
                    priority: r.priority,
                })
            }));
            let total_pages = parsed.pagination.map_or(1, |p| p.total_pages);
            if page >= total_pages {
                break;
            }
            page += 1;
        }
        Ok(out)
    }

    async fn create_record(&self, zone: &str, record: &DesiredRecord) -> Result<u64, DnsError> {
        let url = self.url(&format!("/zones/{zone}/records"));
        let body = DnsimpleWriteBody {
            name: &record.name,
            record_type: record.record_type.as_str(),
            content: &record.content,
            priority: record.priority,
            ttl: record.ttl,
        };
        let body_json = serde_json::to_string(&body)
            .map_err(|e| DnsError::Http(format!("serialize body: {e}")))?;
        if self.dry_run {
            self.record("POST", &url, Some(body_json));
            return Ok(0);
        }
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.api_token)
            .header("Accept", "application/json")
            .body(body_json)
            .header("Content-Type", "application/json")
            .send()
            .await
            .map_err(|e| DnsError::Http(e.to_string()))?;
        let status = resp.status().as_u16();
        let body_bytes = resp
            .bytes()
            .await
            .map_err(|e| DnsError::Http(e.to_string()))?;
        if !(200..300).contains(&status) {
            return Err(DnsError::Status {
                status,
                url,
                body: String::from_utf8_lossy(&body_bytes).into_owned(),
            });
        }
        let parsed: DnsimpleSingleResponse = serde_json::from_slice(&body_bytes)
            .map_err(|e| DnsError::Http(format!("decode create response: {e}")))?;
        Ok(parsed.data.id)
    }

    async fn update_record(
        &self,
        zone: &str,
        record_id: u64,
        record: &DesiredRecord,
    ) -> Result<(), DnsError> {
        let url = self.url(&format!("/zones/{zone}/records/{record_id}"));
        let body = DnsimpleWriteBody {
            name: &record.name,
            record_type: record.record_type.as_str(),
            content: &record.content,
            priority: record.priority,
            ttl: record.ttl,
        };
        let body_json = serde_json::to_string(&body)
            .map_err(|e| DnsError::Http(format!("serialize body: {e}")))?;
        if self.dry_run {
            self.record("PATCH", &url, Some(body_json));
            return Ok(());
        }
        let resp = self
            .http
            .patch(&url)
            .bearer_auth(&self.api_token)
            .header("Accept", "application/json")
            .body(body_json)
            .header("Content-Type", "application/json")
            .send()
            .await
            .map_err(|e| DnsError::Http(e.to_string()))?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let body = resp.text().await.unwrap_or_default();
            return Err(DnsError::Status { status, url, body });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn record_type_round_trips_the_api_strings() {
        for rt in [
            RecordType::A,
            RecordType::Aaaa,
            RecordType::Mx,
            RecordType::Txt,
            RecordType::Cname,
            RecordType::Url,
        ] {
            assert_eq!(RecordType::from_api(rt.as_str()), Some(rt));
        }
        assert_eq!(RecordType::from_api("SOA"), None);
    }

    fn neonlaw_config() -> DnsSetupConfig {
        DnsSetupConfig {
            gateway_ip: Some("8.232.102.111".into()),
            redirect_apex_to_www: true,
            google_workspace: true,
            google_site_verification: Some("token123".into()),
            sendgrid: true,
            dkim_targets: vec![
                "s1.domainkey.u36914952.wl203.sendgrid.net".into(),
                "s2.domainkey.u36914952.wl203.sendgrid.net".into(),
            ],
            sendgrid_link_brand: vec![("em7475".into(), "u36914952.wl203.sendgrid.net".into())],
            spf_includes: vec!["amazonses.com".into()],
            dmarc: Some(DmarcPolicy::None),
            ..Default::default()
        }
    }

    /// `@` reaches the provider as the empty apex name.
    ///
    /// The bug this pins is a silent one: `--host ""` is dropped between the
    /// shell and clap, so the run reported success and created nothing at
    /// all. `@` is the spelling every zone file uses, and it has to arrive
    /// as `""`, because that is what DNSimple calls the apex.
    #[test]
    fn an_apex_host_is_spelled_at_sign_and_arrives_empty() {
        let config = DnsSetupConfig {
            gateway_ip: Some("203.0.113.10".to_string()),
            hosts: vec!["@".to_string(), "www".to_string()],
            ..DnsSetupConfig::default()
        };
        let desired = desired_records("example.test", &config);

        let apex = desired
            .iter()
            .find(|record| record.record_type == RecordType::A && record.name.is_empty())
            .expect("the apex A record");
        assert_eq!(apex.content, "203.0.113.10");
        assert!(
            !desired.iter().any(|record| record.name == "@"),
            "`@` must never reach the provider verbatim"
        );
        assert!(
            desired
                .iter()
                .any(|record| record.record_type == RecordType::A && record.name == "www"),
            "an ordinary label is untouched"
        );
    }

    #[test]
    fn desired_records_reproduces_the_full_topology() {
        let recs = desired_records("neonlaw.com", &neonlaw_config());
        let find = |rt: RecordType, name: &str| {
            recs.iter()
                .filter(|r| r.record_type == rt && r.name == name)
                .collect::<Vec<_>>()
        };

        // Reachability: www + workflows → gateway IP.
        assert_eq!(find(RecordType::A, "www")[0].content, "8.232.102.111");
        assert_eq!(find(RecordType::A, "workflows")[0].content, "8.232.102.111");
        // Apex redirect: one URL record at the root → https://www.<zone>.
        let apex_url = find(RecordType::Url, "");
        assert_eq!(apex_url.len(), 1);
        assert_eq!(apex_url[0].content, "https://www.neonlaw.com");
        // Human mail: apex MX smtp.google.com priority 1.
        let apex_mx = find(RecordType::Mx, "");
        assert_eq!(apex_mx[0].content, "smtp.google.com");
        assert_eq!(apex_mx[0].priority, Some(1));
        // Application inbound: parse MX priority 10.
        let parse_mx = find(RecordType::Mx, "parse");
        assert_eq!(parse_mx[0].content, "mx.sendgrid.net");
        assert_eq!(parse_mx[0].priority, Some(10));
        // DKIM + link branding.
        assert_eq!(
            find(RecordType::Cname, "s1._domainkey")[0].content,
            "s1.domainkey.u36914952.wl203.sendgrid.net"
        );
        assert_eq!(find(RecordType::Cname, "s2._domainkey").len(), 1);
        assert_eq!(
            find(RecordType::Cname, "em7475")[0].content,
            "u36914952.wl203.sendgrid.net"
        );
    }

    #[test]
    fn spf_composes_all_enabled_senders_in_order() {
        let recs = desired_records("neonlaw.com", &neonlaw_config());
        let spf = recs
            .iter()
            .find(|r| {
                r.record_type == RecordType::Txt
                    && r.name.is_empty()
                    && r.content.starts_with("v=spf1")
            })
            .expect("spf record present");
        assert_eq!(
            spf.content,
            "v=spf1 include:_spf.google.com include:amazonses.com include:sendgrid.net -all"
        );
    }

    #[test]
    fn dmarc_defaults_rua_to_postmaster_at_zone() {
        let cfg = DnsSetupConfig {
            dmarc: Some(DmarcPolicy::Reject),
            ..Default::default()
        };
        let recs = desired_records("example.com", &cfg);
        let dmarc = recs
            .iter()
            .find(|r| r.name == "_dmarc")
            .expect("dmarc present");
        assert_eq!(
            dmarc.content,
            "v=DMARC1; p=reject; rua=mailto:postmaster@example.com"
        );
    }

    #[test]
    fn empty_config_emits_nothing() {
        assert!(desired_records("example.com", &DnsSetupConfig::default()).is_empty());
    }

    /// Programmable fake that records calls and returns canned data.
    #[derive(Default)]
    struct FakeProvider {
        existing: Vec<ExistingRecord>,
        creates: Arc<Mutex<Vec<DesiredRecord>>>,
        updates: Arc<Mutex<Vec<(u64, DesiredRecord)>>>,
    }

    #[async_trait]
    impl DnsProvider for FakeProvider {
        async fn list_records(&self, _zone: &str) -> Result<Vec<ExistingRecord>, DnsError> {
            Ok(self.existing.clone())
        }
        async fn create_record(
            &self,
            _zone: &str,
            record: &DesiredRecord,
        ) -> Result<u64, DnsError> {
            self.creates.lock().unwrap().push(record.clone());
            Ok(42)
        }
        async fn update_record(
            &self,
            _zone: &str,
            id: u64,
            record: &DesiredRecord,
        ) -> Result<(), DnsError> {
            self.updates.lock().unwrap().push((id, record.clone()));
            Ok(())
        }
    }

    #[tokio::test]
    async fn run_setup_creates_everything_on_an_empty_zone() {
        let fake = FakeProvider::default();
        let creates = fake.creates.clone();
        let desired = desired_records("neonlaw.com", &neonlaw_config());
        let report = run_setup(&fake, "neonlaw.com", &desired).await.unwrap();
        assert_eq!(report.len(), desired.len());
        assert!(report.iter().all(|r| r.outcome == EnsureOutcome::Created));
        assert_eq!(creates.lock().unwrap().len(), desired.len());
    }

    #[tokio::test]
    async fn run_setup_is_idempotent_including_quoted_txt() {
        // The zone already holds an SPF TXT — quoted, as DNSimple returns it.
        let fake = FakeProvider {
            existing: vec![ExistingRecord {
                id: 7,
                record_type: RecordType::Txt,
                name: String::new(),
                content: "\"v=spf1 include:sendgrid.net -all\"".into(),
                priority: None,
            }],
            ..Default::default()
        };
        let creates = fake.creates.clone();
        let updates = fake.updates.clone();
        let desired = vec![record(
            RecordType::Txt,
            "",
            "v=spf1 include:sendgrid.net -all".into(),
            None,
            3600,
        )];
        let report = run_setup(&fake, "neonlaw.com", &desired).await.unwrap();
        assert_eq!(report[0].outcome, EnsureOutcome::Unchanged);
        assert!(creates.lock().unwrap().is_empty());
        assert!(updates.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn run_setup_patches_single_valued_drift_in_place() {
        let fake = FakeProvider {
            existing: vec![ExistingRecord {
                id: 9,
                record_type: RecordType::Mx,
                name: String::new(),
                content: "mx.sendgrid.net".into(), // drifted away from Workspace
                priority: Some(10),
            }],
            ..Default::default()
        };
        let updates = fake.updates.clone();
        let creates = fake.creates.clone();
        let desired = vec![record(
            RecordType::Mx,
            "",
            "smtp.google.com".into(),
            Some(1),
            3600,
        )];
        let report = run_setup(&fake, "neonlaw.com", &desired).await.unwrap();
        assert_eq!(report[0].outcome, EnsureOutcome::Updated);
        assert_eq!(updates.lock().unwrap()[0].0, 9);
        assert!(creates.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn run_setup_updates_spf_without_duplicating_other_apex_txt_records() {
        let fake = FakeProvider {
            existing: vec![
                ExistingRecord {
                    id: 11,
                    record_type: RecordType::Txt,
                    name: String::new(),
                    content: "google-site-verification=token123".into(),
                    priority: None,
                },
                ExistingRecord {
                    id: 12,
                    record_type: RecordType::Txt,
                    name: String::new(),
                    content: "\"v=spf1 include:sendgrid.net -all\"".into(),
                    priority: None,
                },
            ],
            ..Default::default()
        };
        let updates = fake.updates.clone();
        let creates = fake.creates.clone();
        let desired = desired_records(
            "neonlaw.com",
            &DnsSetupConfig {
                google_workspace: true,
                google_site_verification: Some("token123".into()),
                sendgrid: true,
                spf_includes: vec!["amazonses.com".into()],
                ..Default::default()
            },
        );
        let report = run_setup(&fake, "neonlaw.com", &desired).await.unwrap();
        let spf_report = report
            .iter()
            .find(|r| is_spf(r.record_type, &r.content))
            .expect("spf report present");
        assert_eq!(spf_report.outcome, EnsureOutcome::Updated);

        let updates = updates.lock().unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].0, 12);
        assert_eq!(
            updates[0].1.content,
            "v=spf1 include:_spf.google.com include:amazonses.com include:sendgrid.net -all"
        );
        assert!(!creates
            .lock()
            .unwrap()
            .iter()
            .any(|r| is_spf(r.record_type, &r.content)));
    }

    #[test]
    fn validate_rejects_host_without_gateway_ip() {
        let orphan = DnsSetupConfig {
            hosts: vec!["workflows".into()],
            ..Default::default()
        };
        assert!(orphan.validate().is_err());
        let ok = DnsSetupConfig {
            hosts: vec!["workflows".into()],
            gateway_ip: Some("1.2.3.4".into()),
            ..Default::default()
        };
        assert!(ok.validate().is_ok());
    }

    #[tokio::test]
    async fn run_setup_dedupes_identical_desired_records() {
        // Duplicate `--host www` flags → two identical desired A records. The
        // `existing` snapshot is fetched once, so the second must no-op rather
        // than create a second identical record.
        let fake = FakeProvider::default();
        let creates = fake.creates.clone();
        let www = record(RecordType::A, "www", "1.2.3.4".into(), None, 300);
        let desired = vec![www.clone(), www];
        let report = run_setup(&fake, "example.com", &desired).await.unwrap();
        assert_eq!(report[0].outcome, EnsureOutcome::Created);
        assert_eq!(report[1].outcome, EnsureOutcome::Unchanged);
        assert_eq!(creates.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn run_setup_rejects_apex_url_redirect_conflicting_with_address_records() {
        // Migrating a zone whose apex still forwards via `A`/`AAAA`: the
        // command must refuse rather than create a conflicting apex or
        // silently leave the stale forwarding records answering.
        let existing = vec![
            ExistingRecord {
                id: 1,
                record_type: RecordType::A,
                name: String::new(),
                content: "216.239.32.21".into(),
                priority: None,
            },
            ExistingRecord {
                id: 2,
                record_type: RecordType::Aaaa,
                name: String::new(),
                content: "2001:4860:4802:32::15".into(),
                priority: None,
            },
        ];
        let fake = FakeProvider {
            existing,
            ..Default::default()
        };
        let creates = fake.creates.clone();
        let desired = desired_records(
            "example.com",
            &DnsSetupConfig {
                redirect_apex_to_www: true,
                ..Default::default()
            },
        );
        let err = run_setup(&fake, "example.com", &desired).await.unwrap_err();
        assert!(
            matches!(&err, DnsError::Conflict { name, .. } if name == "the apex"),
            "expected an apex conflict error, got {err:?}"
        );
        assert!(
            err.to_string().contains("A/AAAA"),
            "conflict should name the blocking address records, got {err}"
        );
        // Refused before creating anything.
        assert!(creates.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn run_setup_rejects_address_record_conflicting_with_a_url_redirect() {
        // The live neonlaw.com migration has a historical `www URL` redirect.
        // Creating `www A` beside it would either be rejected by DNSimple or
        // leave the redirect answering instead of the GKE ingress. The
        // additive reconciler must name the conflict rather than report that
        // the production host is ready.
        let fake = FakeProvider {
            existing: vec![ExistingRecord {
                id: 42,
                record_type: RecordType::Url,
                name: "www".into(),
                content: "https://www.neonlaw.com".into(),
                priority: None,
            }],
            ..Default::default()
        };
        let creates = fake.creates.clone();
        let desired = desired_records(
            "neonlaw.com",
            &DnsSetupConfig {
                gateway_ip: Some("203.0.113.15".into()),
                hosts: vec!["www".into()],
                ..Default::default()
            },
        );
        let err = run_setup(&fake, "neonlaw.com", &desired).await.unwrap_err();
        assert!(
            matches!(&err, DnsError::Conflict { name, .. } if name == "www"),
            "expected a www conflict error, got {err:?}"
        );
        assert!(err.to_string().contains("URL"));
        assert!(creates.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn run_setup_rejects_apex_url_redirect_conflicting_with_cname_records() {
        let fake = FakeProvider {
            existing: vec![ExistingRecord {
                id: 1,
                record_type: RecordType::Cname,
                name: String::new(),
                content: "old.example.com".into(),
                priority: None,
            }],
            ..Default::default()
        };
        let desired = desired_records(
            "example.com",
            &DnsSetupConfig {
                redirect_apex_to_www: true,
                ..Default::default()
            },
        );
        let err = run_setup(&fake, "example.com", &desired).await.unwrap_err();
        assert!(
            err.to_string().contains("CNAME"),
            "conflict should name the blocking CNAME record, got {err}"
        );
    }

    #[tokio::test]
    async fn run_setup_adds_missing_member_of_a_multi_valued_group() {
        // A multi-valued group — two apex `TXT` of the same `(type, name)`:
        // one already present → create only the missing one, patch nothing.
        let existing = vec![ExistingRecord {
            id: 0,
            record_type: RecordType::Txt,
            name: String::new(),
            content: "google-site-verification=already-here".into(),
            priority: None,
        }];
        let fake = FakeProvider {
            existing,
            ..Default::default()
        };
        let creates = fake.creates.clone();
        let updates = fake.updates.clone();
        let desired = vec![
            record(
                RecordType::Txt,
                "",
                "google-site-verification=already-here".into(),
                None,
                3600,
            ),
            record(
                RecordType::Txt,
                "",
                "google-site-verification=needs-creating".into(),
                None,
                3600,
            ),
        ];
        let report = run_setup(&fake, "example.com", &desired).await.unwrap();
        assert_eq!(report[0].outcome, EnsureOutcome::Unchanged);
        assert_eq!(report[1].outcome, EnsureOutcome::Created);
        // Multi-valued groups never patch in place.
        assert!(updates.lock().unwrap().is_empty());
        // Only the one missing apex TXT is created.
        assert_eq!(creates.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn dnsimple_list_records_decodes_v2_response_including_a_and_aaaa() {
        let server = MockServer::start().await;
        let body = serde_json::json!({
            "data": [
                {"id": 1, "type": "A", "name": "www", "content": "8.232.102.111"},
                {"id": 2, "type": "AAAA", "name": "", "content": "2001:4860:4802:32::15"},
                {"id": 3, "type": "MX", "name": "", "content": "smtp.google.com", "priority": 1},
                {"id": 4, "type": "SOA", "name": "", "content": "ns1.dnsimple.com ..."} // filtered out
            ]
        });
        Mock::given(method("GET"))
            .and(path("/v2/123/zones/neonlaw.com/records"))
            .and(header("authorization", "Bearer T"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;

        let provider = DnsimpleProvider::new("T", "123").with_base_url(server.uri());
        let records = provider.list_records("neonlaw.com").await.unwrap();
        // The SOA record is filtered out; A/AAAA/MX are kept.
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].record_type, RecordType::A);
        assert_eq!(records[1].record_type, RecordType::Aaaa);
        assert_eq!(records[2].priority, Some(1));
    }

    #[tokio::test]
    async fn dnsimple_list_records_pages_through_all_records() {
        // A zone with >100 records spans multiple pages; every page must
        // be fetched so reconciliation and the apex conflict guard see the
        // whole zone, not just the first 100.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/123/zones/big.example/records"))
            .and(query_param("page", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": 1, "type": "A", "name": "www", "content": "1.2.3.4"}],
                "pagination": {"total_pages": 2}
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v2/123/zones/big.example/records"))
            .and(query_param("page", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": 2, "type": "AAAA", "name": "", "content": "2001:db8::1"}],
                "pagination": {"total_pages": 2}
            })))
            .expect(1)
            .mount(&server)
            .await;

        let provider = DnsimpleProvider::new("T", "123").with_base_url(server.uri());
        let records = provider.list_records("big.example").await.unwrap();
        // Both pages merged: the page-1 `A` and the page-2 apex `AAAA`.
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].id, 1);
        assert_eq!(records[1].id, 2);
        assert_eq!(records[1].record_type, RecordType::Aaaa);
    }

    #[tokio::test]
    async fn dnsimple_create_record_posts_v2_body_and_returns_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2/123/zones/neonlaw.com/records"))
            .and(header("authorization", "Bearer T"))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(serde_json::json!({"data": {"id": 99, "type": "A", "name": "www", "content": "8.232.102.111"}})),
            )
            .expect(1)
            .mount(&server)
            .await;

        let provider = DnsimpleProvider::new("T", "123").with_base_url(server.uri());
        let desired = record(RecordType::A, "www", "8.232.102.111".into(), None, 300);
        let id = provider
            .create_record("neonlaw.com", &desired)
            .await
            .unwrap();
        assert_eq!(id, 99);
    }

    #[tokio::test]
    async fn dnsimple_dry_run_skips_create_traffic_and_records_call() {
        // Point at unreachable to prove no real HTTP happens.
        let provider = DnsimpleProvider::new("T", "123")
            .with_base_url("http://127.0.0.1:1")
            .with_dry_run();
        let desired = record(
            RecordType::Txt,
            "_dmarc",
            "v=DMARC1; p=none".into(),
            None,
            3600,
        );
        let id = provider
            .create_record("example.com", &desired)
            .await
            .unwrap();
        assert_eq!(id, 0); // dry-run synthetic id
        let calls = provider.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, "POST");
        assert!(calls[0].url.contains("/v2/123/zones/example.com/records"));
        assert!(calls[0].body.as_deref().unwrap().contains("_dmarc"));
    }

    #[tokio::test]
    async fn dnsimple_list_returns_status_error_on_4xx() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(403).set_body_string("Forbidden"))
            .mount(&server)
            .await;
        let provider = DnsimpleProvider::new("T", "123").with_base_url(server.uri());
        let err = provider.list_records("example.com").await.unwrap_err();
        match err {
            DnsError::Status { status, body, .. } => {
                assert_eq!(status, 403);
                assert!(body.contains("Forbidden"));
            }
            _ => panic!("expected Status, got {err:?}"),
        }
    }
}
