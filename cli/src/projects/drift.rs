//! `navigator projects drift` — reconcile Project *repositories* against the
//! live Project rows.
//!
//! One `projects.code` names both a repository and a row, and nothing makes the
//! two agree. A repository declares its code in its root manifest; a row
//! records its repository in `projects.repository_url`. Either side can be
//! written without the other, and neither side complains — so a repository can
//! publish a portal under `<code>/portal/` that no row mounts.
//!
//! # This is the repository half only
//!
//! The row half lives in [`store::project_reconcile`], behind
//! `GET /app/api/project-repositories`, and the split is not arbitrary: the two
//! halves need different things and fail differently.
//!
//! A row can be checked against itself. A Project code *is* its repository name
//! ([`cloud::workspace::is_valid_slug`]), so a row whose `repository_url` names
//! a different repository is drift provable from that one row — no checkout, no
//! fleet, no configuration. That belongs on the server, where every row is
//! visible and no local state is assumed.
//!
//! A repository cannot. "Does this checkout's declared code name a live row?"
//! needs the checkout, and only a machine holding the clones can answer it.
//! That is this command, and it is why the command still exists.
//!
//! # Why it reads codes rather than rows
//!
//! The one thing this half needs from the server is the set of live codes.
//! Findings cannot supply it: a row that is entirely fine produces no finding,
//! so an absence in the finding set means "reconciled" and "does not exist"
//! indistinguishably. `project_codes` on the reconciliation report carries the
//! codes themselves, which is exactly the question and nothing more.
//!
//! It is deliberately *not* `GET /app/api/projects`. That route returns the
//! caller's own matters — `store::access::visible_projects` scopes to
//! participation rows for every firm tier, Owner and Admin included — so a
//! repository whose row exists but the caller does not participate in would
//! have read as a repository with no row at all, which is the loudest failure
//! this command emits.
//!
//! # Why a repository declares its own absence
//!
//! Not every repository without a row is drift. Some matters are closed and
//! deliberately carry no row, and a tool that reports known-good repositories
//! as failures is a tool nobody runs twice.
//!
//! The suppression cannot live here. A Project code *is* a client identifier —
//! it names who retained the firm — and this repository is public, so a
//! constant list of the codes to skip would publish exactly what `AGENTS.md`
//! forbids. A `--ignore` flag only moves that list into a runbook or a CI
//! invocation, where it is written down just the same and reviewed less.
//! Inferring intent from shape — "no `portal/`, no `seeds/`, so nothing is
//! load-bearing" — is worse than either: an empty repository is also what a
//! brand-new *unreconciled* Project looks like, so that rule would go quiet
//! about precisely the gaps this command exists to find.
//!
//! So the repository declares it, in the manifest it already carries, with a
//! reason rather than a flag ([`MANIFEST_ROWLESS_KEY`]). The fact lives beside
//! the matter it is about, is reviewed by whoever knows that matter, and is
//! deleted when the repository is. A boolean would have let a red line be
//! dismissed without anyone writing down why.
//!
//! This is the opposite of the call [`super::repository`]'s allowed-root list
//! makes, and deliberately so. That list governs a rule that is *identical for
//! every repository* — which paths may sit at a root — so centralizing it costs
//! nothing and keeps the gate from going advisory. Whether one matter is meant
//! to have a row is a per-matter fact only that matter knows, and centralizing
//! it costs a client identifier in a public tree.
//!
//! Declared row-less repositories are still *counted*, and `--all` lists them.
//! Suppressed and silent are different things: a report that hides repositories
//! without saying so fails the same way as one that cries wolf about them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use comfy_table::{presets::UTF8_FULL, Cell, ContentArrangement, Table};

use crate::palette;
use crate::projects::doctor::Status;

/// The manifest a Project repository declares its Project in.
///
/// Taken from [`super::repository`] rather than spelled again: two constants for
/// one filename is a rename waiting to leave one of them stale, and the layout
/// gate that admits the file and the command that reads it are exactly the pair
/// that must agree. Distinct from `store::sample_project::MANIFEST_FILE`
/// (`navigator.yml`, keyed `name:`), which is a bundle's publish manifest and a
/// different contract.
use super::repository::PROJECT_MANIFEST as MANIFEST;

/// The manifest key naming the Project this repository belongs to.
pub const MANIFEST_CODE_KEY: &str = "project";
/// The manifest key declaring that this repository is *meant* to have no live
/// Project row. Its value is the reason, which is why it is a string: a
/// boolean records that someone silenced a finding without recording why.
pub const MANIFEST_ROWLESS_KEY: &str = "no_live_row";

/// A Project repository's root manifest.
///
/// Unknown keys pass through untouched — `navigator.yaml` also carries `host:`,
/// and a repository is free to add its own keys without this command refusing
/// to read the two it cares about.
#[derive(Debug, Clone, Default, serde::Deserialize)]
struct Manifest {
    project: Option<String>,
    /// Deliberately the raw YAML value rather than `Option<String>`. Deserializing
    /// straight into a string is *lenient*: `no_live_row: true` coerces to the
    /// string `"true"` and reads as a perfectly good reason, which is exactly the
    /// boolean-shaped suppression [`MANIFEST_ROWLESS_KEY`] exists to refuse. The
    /// value is held untyped here and required to be a genuine string below.
    no_live_row: Option<serde_yaml::Value>,
}

/// One checkout found under the scan root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedRepository {
    /// The checkout's directory name. This is the repository name, and
    /// `super::repository::validate` takes the Project code from it.
    pub directory: String,
    /// The manifest's `project:` value, when the manifest parsed.
    pub declared_code: Option<String>,
    /// The manifest's `no_live_row:` reason, when it carries one.
    pub rowless_reason: Option<String>,
    /// Why the manifest could not be read, when it could not be.
    pub manifest_error: Option<String>,
}

impl ScannedRepository {
    /// The Project code this repository is about.
    ///
    /// The manifest wins where it declares one, because that is the value the
    /// repository asserts about itself; the directory name is the fallback
    /// `super::repository::validate` already uses. A disagreement between them
    /// is reported separately rather than resolved silently.
    #[must_use]
    pub fn code(&self) -> &str {
        self.declared_code.as_deref().unwrap_or(&self.directory)
    }
}

/// What the two sides disagree about.
///
/// Serialized internally tagged, so every variant's fields survive onto the
/// wire beside its `kind` — the same shape `store::project_reconcile::Finding`
/// emits, because a gate reading both halves should not need two parsers. The
/// alternative, flattening each finding into a `detail` sentence, makes the
/// prose the contract: a consumer wanting the repository a finding is about has
/// to pull it back out of English.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Finding {
    /// A repository whose code no live row carries.
    RepositoryHasNoRow { repository: String, code: String },
    /// A repository that declares it is meant to have no row, and why.
    RowlessByDeclaration {
        repository: String,
        code: String,
        reason: String,
    },
    /// A repository whose manifest declares a code other than its own name.
    /// Legal today — nothing derives one from the other — and worth seeing,
    /// because it is the shape that lets a repository assert the wrong code as
    /// settled fact.
    ManifestDisagreesWithName {
        repository: String,
        declared: String,
    },
    /// Two checkouts claiming one Project code.
    DuplicateCode {
        code: String,
        repositories: Vec<String>,
    },
    /// A manifest that is present but unreadable, or names an invalid code.
    UnreadableManifest { repository: String, detail: String },
    /// A checkout carrying no manifest. Reported rather than skipped: a
    /// Project repository that never got one is invisible to every check
    /// above, which is indistinguishable from having no drift.
    NoManifest { repository: String },
}

impl Finding {
    /// How loudly to report this.
    ///
    /// `Warn` is for a state that is legal today and worth a human's attention;
    /// `Fail` is for one side asserting something the other contradicts.
    #[must_use]
    pub fn status(&self) -> Status {
        match self {
            Self::RowlessByDeclaration { .. } => Status::Ok,
            Self::ManifestDisagreesWithName { .. } | Self::NoManifest { .. } => Status::Warn,
            Self::RepositoryHasNoRow { .. }
            | Self::DuplicateCode { .. }
            | Self::UnreadableManifest { .. } => Status::Fail,
        }
    }

    /// The short category name, stable enough to grep a CI log for.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::RepositoryHasNoRow { .. } => "repository-has-no-row",
            Self::RowlessByDeclaration { .. } => "rowless-by-declaration",
            Self::ManifestDisagreesWithName { .. } => "manifest-disagrees-with-name",
            Self::DuplicateCode { .. } => "duplicate-code",
            Self::UnreadableManifest { .. } => "unreadable-manifest",
            Self::NoManifest { .. } => "no-manifest",
        }
    }

    /// The Project code or repository this finding is about, for the report's
    /// first column.
    #[must_use]
    pub fn subject(&self) -> &str {
        match self {
            Self::RepositoryHasNoRow { code, .. }
            | Self::RowlessByDeclaration { code, .. }
            | Self::DuplicateCode { code, .. } => code,
            Self::ManifestDisagreesWithName { repository, .. }
            | Self::UnreadableManifest { repository, .. }
            | Self::NoManifest { repository } => repository,
        }
    }

    /// One sentence naming what disagrees with what.
    ///
    /// No longer takes the scan root. The only finding that needed it was the
    /// row-side "no such repository under this directory", which was true but
    /// uninteresting whenever the root held part of the fleet — the whole
    /// conditional-on-a-machine shape that moved to the server.
    #[must_use]
    pub fn detail(&self) -> String {
        match self {
            Self::RepositoryHasNoRow { repository, code } => format!(
                "`{repository}` declares Project `{code}`, which no live row carries; \
                 a portal published under `{code}/portal/` would mount nowhere"
            ),
            Self::RowlessByDeclaration { reason, .. } => {
                format!("declared to have no live row: {reason}")
            }
            Self::ManifestDisagreesWithName {
                repository,
                declared,
            } => format!(
                "`{repository}` declares Project `{declared}`; the repository name is the code \
                 everywhere else, and nothing makes the two agree"
            ),
            Self::DuplicateCode { code, repositories } => {
                format!("Project `{code}` is claimed by {}", repositories.join(", "))
            }
            Self::UnreadableManifest { detail, .. } => {
                format!("{MANIFEST} could not be read: {detail}")
            }
            Self::NoManifest { repository } => format!(
                "`{repository}` carries no {MANIFEST}, so it declares no Project and cannot \
                 be reconciled"
            ),
        }
    }
}

/// Every disagreement found, with the counts a reader needs to trust it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub findings: Vec<Finding>,
    /// Checkouts examined. Printed because every row-side finding is only
    /// meaningful against a scan of the whole fleet.
    pub repositories: usize,
    /// Live Project codes read from the host. Printed because a repository
    /// reported as having no row is only trustworthy against a known number of
    /// live codes — nought would mean the read failed, not that the fleet is
    /// entirely unreconciled.
    pub live_codes: usize,
}

impl Report {
    /// True when nothing failed. Warnings do not make a fleet drifted, in the
    /// same sense [`super::doctor::Diagnosis::is_healthy`] uses.
    #[must_use]
    pub fn is_reconciled(&self) -> bool {
        !self
            .findings
            .iter()
            .any(|finding| finding.status() == Status::Fail)
    }

    /// Findings of one status, in report order.
    #[must_use]
    pub fn of(&self, status: Status) -> Vec<&Finding> {
        self.findings
            .iter()
            .filter(|finding| finding.status() == status)
            .collect()
    }
}

/// Compare each repository against the set of live Project codes. Pure: every
/// input is already read.
///
/// `live_codes` is the sorted `project_codes` from the reconciliation report.
/// A set rather than a row list, because that is the entire question this half
/// asks of the server — whether a declared code names a live row — and taking
/// anything wider would pull matter detail into a command that has no use for
/// it and prints its findings to a terminal.
///
/// Order is deliberate — per-repository findings in scan order, then
/// whole-fleet integrity — so two runs over the same checkouts read the same.
pub fn analyze(repositories: &[ScannedRepository], live_codes: &[String]) -> Report {
    let mut findings = Vec::new();

    let live: BTreeSet<&str> = live_codes.iter().map(String::as_str).collect();

    // Which repositories claim which code, so a duplicate is reported rather
    // than letting one checkout silently shadow another.
    let mut claimants: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for repository in repositories {
        if repository.manifest_error.is_none() {
            claimants
                .entry(repository.code())
                .or_default()
                .push(repository.directory.as_str());
        }
    }

    // ── Repository side ────────────────────────────────────────────────────
    for repository in repositories {
        if let Some(detail) = &repository.manifest_error {
            findings.push(Finding::UnreadableManifest {
                repository: repository.directory.clone(),
                detail: detail.clone(),
            });
            continue;
        }
        match &repository.declared_code {
            None => findings.push(Finding::NoManifest {
                repository: repository.directory.clone(),
            }),
            Some(declared) if declared != &repository.directory => {
                findings.push(Finding::ManifestDisagreesWithName {
                    repository: repository.directory.clone(),
                    declared: declared.clone(),
                });
            }
            Some(_) => {}
        }
        let code = repository.code();
        if live.contains(code) {
            continue;
        }
        match &repository.rowless_reason {
            Some(reason) => findings.push(Finding::RowlessByDeclaration {
                repository: repository.directory.clone(),
                code: code.to_string(),
                reason: reason.clone(),
            }),
            None => findings.push(Finding::RepositoryHasNoRow {
                repository: repository.directory.clone(),
                code: code.to_string(),
            }),
        }
    }

    // ── Whole-fleet integrity ──────────────────────────────────────────────
    for (code, claiming) in claimants {
        if claiming.len() > 1 {
            findings.push(Finding::DuplicateCode {
                code: code.to_string(),
                repositories: claiming.iter().map(|name| (*name).to_string()).collect(),
            });
        }
    }

    Report {
        findings,
        repositories: repositories.len(),
        live_codes: live.len(),
    }
}

/// Read one checkout's manifest.
///
/// A checkout with no manifest is still returned — that is a finding, not a
/// reason to skip a directory. Staying silent about it would make an
/// unmanifested Project repository indistinguishable from a reconciled one.
fn scan_repository(directory: &Path) -> ScannedRepository {
    let name = directory
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();
    let unreadable = |detail: String| ScannedRepository {
        directory: name.clone(),
        declared_code: None,
        rowless_reason: None,
        manifest_error: Some(detail),
    };

    let manifest_path = directory.join(MANIFEST);
    if !manifest_path.is_file() {
        return ScannedRepository {
            directory: name,
            declared_code: None,
            rowless_reason: None,
            manifest_error: None,
        };
    }
    let contents = match std::fs::read_to_string(&manifest_path) {
        Ok(contents) => contents,
        Err(error) => return unreadable(error.to_string()),
    };
    let manifest: Manifest = match serde_yaml::from_str(&contents) {
        Ok(manifest) => manifest,
        Err(error) => return unreadable(format!("not valid YAML: {error}")),
    };
    let declared = manifest
        .project
        .map(|code| code.trim().to_string())
        .filter(|code| !code.is_empty());
    // The manifest cannot introduce a code the rest of Navigator would refuse,
    // the same rule `store::sample_project::project_code_from_manifest` applies.
    if let Some(code) = &declared {
        if !store::projects::is_valid_code(code) {
            return unreadable(format!(
                "`{MANIFEST_CODE_KEY}: {code}` is not a valid Project code"
            ));
        }
    }
    // A declaration must carry prose. Anything else — a boolean, a number, a
    // list — records that a finding was silenced without recording why, which
    // is the one thing this key must not allow.
    let reason = match manifest.no_live_row {
        None => None,
        Some(serde_yaml::Value::String(reason)) => {
            let reason = reason.trim().to_string();
            if reason.is_empty() {
                return unreadable(format!(
                    "`{MANIFEST_ROWLESS_KEY}:` must give the reason this repository has no live row"
                ));
            }
            Some(reason)
        }
        Some(_) => {
            return unreadable(format!(
                "`{MANIFEST_ROWLESS_KEY}:` must be the reason this repository has no live row, \
                 written as text"
            ))
        }
    };
    ScannedRepository {
        directory: name,
        declared_code: declared,
        rowless_reason: reason,
        manifest_error: None,
    }
}

/// Every checkout directly under `root`, in name order.
///
/// Immediate children only, and only those that are git checkouts: the scan
/// root is a directory of sibling clones, and descending further would read a
/// vendored copy or a nested fixture as a Project repository.
pub fn scan(root: &Path) -> Result<Vec<ScannedRepository>> {
    let mut found = Vec::new();
    let entries = std::fs::read_dir(root)
        .with_context(|| format!("read the scan root {}", root.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("read an entry under {}", root.display()))?;
        let path = entry.path();
        if !path.is_dir() || !path.join(".git").exists() {
            continue;
        }
        found.push(scan_repository(&path));
    }
    found.sort_by(|a, b| a.directory.cmp(&b.directory));
    Ok(found)
}

/// Read the live Project codes from the reconciliation door.
///
/// `GET /app/api/project-repositories` rather than `GET /app/api/projects`. The
/// latter returns the *caller's* matters — `store::access::visible_projects`
/// scopes to participation rows for every firm tier, Owner and Admin included —
/// so a repository whose row exists but this caller does not participate in
/// would read as a repository with no row, which is the loudest finding here.
/// The door is admin-tier and reads every row.
///
/// Only `project_codes` is taken. The report's findings are the row side's
/// answer and this half has no business re-reporting them: a reader who wants
/// them asks the door.
async fn fetch_live_codes(base: &str, token: &str) -> Result<Vec<String>> {
    #[derive(serde::Deserialize)]
    struct Reconciliation {
        project_codes: Vec<String>,
    }

    let response = reqwest::Client::new()
        .get(format!("{base}/app/api/project-repositories"))
        .bearer_auth(token)
        .send()
        .await
        .context("GET /app/api/project-repositories")?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        // The body is quoted, not discarded: this door is admin-tier, so the
        // overwhelmingly likely failure is a caller who is merely lawyer-tier,
        // and a bare status leaves them guessing which of the two it was.
        return Err(anyhow!(
            "listing live Project codes failed: {status}: {}",
            first_line(&body)
        ));
    }
    Ok(serde_json::from_str::<Reconciliation>(&body)
        .context("parse the reconciliation report")?
        .project_codes)
}

/// The first line of a response body, for a one-line error.
fn first_line(body: &str) -> &str {
    body.lines().next().unwrap_or_default().trim()
}

fn render(report: &Report, scan_root: &str, all: bool) {
    let shown: Vec<&Finding> = report
        .findings
        .iter()
        .filter(|finding| all || finding.status() != Status::Ok)
        .collect();

    if shown.is_empty() {
        println!("{}", palette::dim("no drift"));
    } else {
        let mut table = Table::new();
        table
            .load_style(UTF8_FULL)
            .set_content_arrangement(ContentArrangement::Dynamic);
        table.set_header(
            ["", "subject", "finding", "detail"].map(|header| Cell::new(palette::header(header))),
        );
        for finding in &shown {
            let mark = match finding.status() {
                Status::Ok => palette::dim("ok"),
                Status::Warn => palette::header("warn"),
                Status::Fail => palette::highlight("fail"),
            };
            table.add_row([
                Cell::new(mark),
                Cell::new(finding.subject()),
                Cell::new(finding.kind()),
                Cell::new(finding.detail()),
            ]);
        }
        println!("{table}");
    }

    let declared = report.of(Status::Ok).len();
    println!(
        "{}",
        palette::dim(format!(
            "{} repositories under {scan_root}, {} live Project codes: {} drifted, \
             {} to review, {declared} declared row-less",
            report.repositories,
            report.live_codes,
            report.of(Status::Fail).len(),
            report.of(Status::Warn).len(),
        ))
    );
    if declared > 0 && !all {
        println!(
            "{}",
            palette::dim(format!(
                "  ({declared} suppressed by `{MANIFEST_ROWLESS_KEY}:` in {MANIFEST}; \
                 pass --all to list them)"
            ))
        );
    }
}

fn as_json(report: &Report, scan_root: &str) -> serde_json::Value {
    serde_json::json!({
        "scan_root": scan_root,
        "repositories": report.repositories,
        "live_codes": report.live_codes,
        "reconciled": report.is_reconciled(),
        "findings": report
            .findings
            .iter()
            .map(|finding| {
                // The variant's own fields, plus the two a consumer needs that
                // are not fields: how loudly to take it, and the sentence for a
                // human reading the JSON rather than the table.
                let mut value = serde_json::to_value(finding)
                    .unwrap_or_else(|_| serde_json::json!({"kind": finding.kind()}));
                if let Some(object) = value.as_object_mut() {
                    object.insert(
                        "severity".into(),
                        serde_json::json!(match finding.status() {
                            Status::Ok => "ok",
                            Status::Warn => "warn",
                            Status::Fail => "fail",
                        }),
                    );
                    object.insert("detail".into(), serde_json::json!(finding.detail()));
                }
                value
            })
            .collect::<Vec<_>>(),
    })
}

/// `navigator projects drift [--host h] [--dir d] [--all] [--json]`.
///
/// Read-only on both sides: it reads manifests off the local disk and lists the
/// live Project codes over a bearer token. Nothing is created, patched, or
/// closed — reconciling a repository to a row is a decision about a matter, not
/// a mechanical fix, so this command reports and stops.
///
/// # Exit codes
///
/// A gate reads these, so they are three values rather than two:
///
/// | Code | Meaning |
/// | --- | --- |
/// | `0` | Every repository reconciles. Warnings do not change this. |
/// | `1` | At least one failing finding. |
/// | `2` | The report could not be produced — the scan root, the login, the host, or the response. |
///
/// The split between `1` and `2` is the one that matters: a gate that treats
/// "drifted" and "could not ask" alike goes green on an expired token.
pub async fn run(host: Option<&str>, dir: &Path, all: bool, json: bool) -> ExitCode {
    let scan_root = dir.display().to_string();
    let repositories = match scan(dir) {
        Ok(repositories) => repositories,
        Err(error) => {
            eprintln!("navigator: {error:#}");
            return ExitCode::from(2);
        }
    };
    let live_codes = match crate::remote::resolve(host) {
        Ok((base, token)) => match fetch_live_codes(&base, &token).await {
            Ok(codes) => codes,
            Err(error) => {
                eprintln!("navigator: {error:#}");
                return ExitCode::from(2);
            }
        },
        Err(error) => {
            eprintln!("navigator: {error:#}");
            return ExitCode::from(2);
        }
    };

    let report = analyze(&repositories, &live_codes);
    if json {
        match serde_json::to_string_pretty(&as_json(&report, &scan_root)) {
            Ok(rendered) => println!("{rendered}"),
            Err(error) => {
                eprintln!("navigator: render the report: {error}");
                return ExitCode::from(2);
            }
        }
    } else {
        render(&report, &scan_root, all);
    }

    if report.is_reconciled() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

#[cfg(test)]
mod tests {
    use super::{analyze, as_json, scan, Finding, ScannedRepository, MANIFEST};
    use crate::projects::doctor::Status;
    use std::path::Path;

    /// A repository that parsed cleanly and declares its own name as its code —
    /// the shape every reconciled repository has.
    fn repository(name: &str) -> ScannedRepository {
        ScannedRepository {
            directory: name.to_string(),
            declared_code: Some(name.to_string()),
            rowless_reason: None,
            manifest_error: None,
        }
    }

    fn declared_rowless(name: &str) -> ScannedRepository {
        ScannedRepository {
            rowless_reason: Some("the matter closed; no row was opened".into()),
            ..repository(name)
        }
    }

    /// A checkout whose manifest declares a code other than its directory name.
    fn declaring(directory: &str, code: &str) -> ScannedRepository {
        ScannedRepository {
            directory: directory.to_string(),
            declared_code: Some(code.to_string()),
            rowless_reason: None,
            manifest_error: None,
        }
    }

    /// The live codes as the reconciliation door reports them: `project_codes`
    /// and nothing else, which is all this half asks for.
    fn live(codes: &[&str]) -> Vec<String> {
        codes.iter().map(|code| (*code).to_string()).collect()
    }

    fn kinds(findings: &[Finding]) -> Vec<&'static str> {
        findings.iter().map(Finding::kind).collect()
    }

    fn kinds_of(findings: &[&Finding]) -> Vec<&'static str> {
        findings.iter().map(|finding| finding.kind()).collect()
    }

    #[test]
    fn a_repository_matched_to_its_row_reports_nothing() {
        let report = analyze(&[repository("acme")], &live(&["acme"]));

        assert_eq!(kinds(&report.findings), Vec::<&str>::new());
        assert!(report.is_reconciled());
    }

    #[test]
    fn a_repository_whose_code_no_row_carries_is_drift() {
        let report = analyze(&[repository("acme")], &live(&[]));

        assert_eq!(kinds(&report.findings), vec!["repository-has-no-row"]);
        assert!(!report.is_reconciled());
    }

    // ── The design decision: intentional absence is declared, not inferred ──

    /// The whole point of the `no_live_row:` key. A repository that declares it
    /// is meant to have no row is not drift, and the fleet stays reconciled.
    #[test]
    fn a_repository_declaring_no_live_row_is_not_drift() {
        let report = analyze(&[declared_rowless("acme")], &live(&[]));

        assert_eq!(kinds(&report.findings), vec!["rowless-by-declaration"]);
        assert_eq!(report.findings[0].status(), Status::Ok);
        assert!(report.is_reconciled());
    }

    /// Suppressed is not silent. The declaration removes a *failure*, not the
    /// repository: it is still counted, and `Report::of(Ok)` is what the footer
    /// and `--all` list from.
    #[test]
    fn a_declared_rowless_repository_is_still_counted() {
        let report = analyze(&[declared_rowless("acme"), repository("beta")], &live(&[]));

        assert_eq!(report.repositories, 2);
        assert_eq!(report.of(Status::Ok).len(), 1);
        assert_eq!(report.of(Status::Fail).len(), 1);
    }

    /// The declaration only speaks to the absence of a row. A repository that
    /// declares it *and* has one is fully reconciled, and says nothing.
    #[test]
    fn a_declaration_is_inert_once_the_row_exists() {
        let report = analyze(&[declared_rowless("acme")], &live(&["acme"]));

        assert_eq!(kinds(&report.findings), Vec::<&str>::new());
    }

    // ── The row side, which is the direction nothing checked ───────────────

    // ── Repository integrity ───────────────────────────────────────────────

    #[test]
    fn a_manifest_disagreeing_with_the_repository_name_warns_but_does_not_fail() {
        let report = analyze(&[declaring("acme", "beta")], &live(&["beta"]));

        assert_eq!(
            kinds_of(&report.of(Status::Warn)),
            vec!["manifest-disagrees-with-name"]
        );
    }

    #[test]
    fn two_checkouts_claiming_one_code_is_a_failure() {
        let report = analyze(
            &[repository("acme"), declaring("acme-fork", "acme")],
            &live(&["acme"]),
        );

        assert!(
            kinds(&report.findings).contains(&"duplicate-code"),
            "{:?}",
            kinds(&report.findings)
        );
        assert!(!report.is_reconciled());
    }

    #[test]
    fn an_unreadable_manifest_fails_rather_than_reading_as_absent() {
        let broken = ScannedRepository {
            manifest_error: Some("not valid YAML".into()),
            declared_code: None,
            ..repository("acme")
        };

        let report = analyze(&[broken], &live(&[]));

        assert_eq!(kinds(&report.findings), vec!["unreadable-manifest"]);
        assert!(!report.is_reconciled());
    }

    /// A checkout with no manifest declares no Project, so it cannot be
    /// reconciled — and it must not be skipped silently, or an unmanifested
    /// Project repository would read as a reconciled one.
    #[test]
    fn a_checkout_with_no_manifest_is_reported() {
        let bare = ScannedRepository {
            declared_code: None,
            ..repository("acme")
        };

        let report = analyze(&[bare], &live(&["acme"]));

        assert_eq!(kinds(&report.findings), vec!["no-manifest"]);
        assert_eq!(report.findings[0].status(), Status::Warn);
        assert!(report.is_reconciled());
    }

    // ── The URL to repository rule ─────────────────────────────────────────

    // ── The wire shape ─────────────────────────────────────────────────────

    /// The contract a gate reads, which previously had no test at all.
    ///
    /// Each finding carries its own fields beside `kind`, so a consumer asking
    /// "which repository?" reads `repository` rather than pulling it out of the
    /// `detail` sentence. `detail` is still there, for a human reading the JSON.
    #[test]
    fn a_finding_serializes_with_its_own_fields_beside_its_kind() {
        let report = analyze(&[repository("acme")], &live(&[]));

        let json = as_json(&report, "/fleet");

        assert_eq!(json["scan_root"], "/fleet");
        assert_eq!(json["repositories"], 1);
        assert_eq!(json["live_codes"], 0);
        assert_eq!(json["reconciled"], false);

        let finding = &json["findings"][0];
        assert_eq!(finding["kind"], "repository-has-no-row");
        assert_eq!(finding["severity"], "fail");
        assert_eq!(finding["repository"], "acme");
        assert_eq!(finding["code"], "acme");
        assert!(
            finding["detail"]
                .as_str()
                .is_some_and(|d| d.contains("acme")),
            "the sentence survives for a human: {finding}"
        );
    }

    /// A warning does not make the fleet drifted, and the JSON says so — the
    /// field a gate branches on rather than the exit code it may not see.
    #[test]
    fn the_json_reports_reconciled_when_only_warnings_were_found() {
        let report = analyze(&[declaring("acme", "beta")], &live(&["beta"]));

        let json = as_json(&report, "/fleet");

        assert_eq!(json["findings"][0]["severity"], "warn");
        assert_eq!(json["reconciled"], true);
    }

    /// A declared row-less repository is `ok`, and it reaches the JSON even
    /// though the table hides it without `--all`. Suppressed is not silent.
    #[test]
    fn a_declared_rowless_repository_reaches_the_json_as_ok() {
        let report = analyze(&[declared_rowless("acme")], &live(&[]));

        let json = as_json(&report, "/fleet");

        assert_eq!(json["findings"][0]["severity"], "ok");
        assert_eq!(json["findings"][0]["kind"], "rowless-by-declaration");
        assert!(
            json["findings"][0]["reason"].as_str().is_some(),
            "the written reason is a field, not only prose"
        );
        assert_eq!(json["reconciled"], true);
    }

    /// `kind()` and the serialized tag are written in two places, so a test
    /// holds them together rather than a comment asking the next author to
    /// remember. The same guard `store::project_reconcile` carries.
    #[test]
    fn every_findings_kind_matches_its_serialized_tag() {
        let findings = [
            Finding::RepositoryHasNoRow {
                repository: "acme".into(),
                code: "acme".into(),
            },
            Finding::RowlessByDeclaration {
                repository: "acme".into(),
                code: "acme".into(),
                reason: "the matter closed".into(),
            },
            Finding::ManifestDisagreesWithName {
                repository: "acme".into(),
                declared: "beta".into(),
            },
            Finding::DuplicateCode {
                code: "acme".into(),
                repositories: vec!["acme".into(), "acme-fork".into()],
            },
            Finding::UnreadableManifest {
                repository: "acme".into(),
                detail: "not valid YAML".into(),
            },
            Finding::NoManifest {
                repository: "acme".into(),
            },
        ];

        for finding in &findings {
            let json = serde_json::to_value(finding).expect("a finding serializes");
            assert_eq!(
                json["kind"],
                finding.kind(),
                "{finding:?} serializes a tag its kind() does not match"
            );
        }
    }

    // ── The scan ───────────────────────────────────────────────────────────

    fn checkout(root: &Path, name: &str, manifest: Option<&str>) {
        let directory = root.join(name);
        std::fs::create_dir_all(directory.join(".git")).unwrap();
        if let Some(contents) = manifest {
            std::fs::write(directory.join(MANIFEST), contents).unwrap();
        }
    }

    #[test]
    fn the_scan_reads_each_checkouts_declared_code_and_reason() {
        let root = tempfile::tempdir().unwrap();
        checkout(
            root.path(),
            "acme",
            Some("host: www.example.com\nproject: acme\n"),
        );
        checkout(
            root.path(),
            "beta",
            Some("project: beta\nno_live_row: the matter closed\n"),
        );

        let found = scan(root.path()).unwrap();

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].declared_code.as_deref(), Some("acme"));
        assert_eq!(found[0].rowless_reason, None);
        assert_eq!(found[1].declared_code.as_deref(), Some("beta"));
        assert_eq!(
            found[1].rowless_reason.as_deref(),
            Some("the matter closed")
        );
    }

    /// The scan root is a directory of sibling clones. A subdirectory that is
    /// not a checkout is not a Project repository, and reading it as one would
    /// report a scratch folder as a missing matter.
    #[test]
    fn the_scan_ignores_a_directory_that_is_not_a_checkout() {
        let root = tempfile::tempdir().unwrap();
        checkout(root.path(), "acme", Some("project: acme\n"));
        std::fs::create_dir_all(root.path().join("notes")).unwrap();

        let found = scan(root.path()).unwrap();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].directory, "acme");
    }

    #[test]
    fn the_scan_reports_a_checkout_with_an_unparsable_manifest() {
        let root = tempfile::tempdir().unwrap();
        checkout(root.path(), "acme", Some("project: [unclosed\n"));

        let found = scan(root.path()).unwrap();

        assert!(
            found[0]
                .manifest_error
                .as_deref()
                .is_some_and(|detail| detail.contains("not valid YAML")),
            "{:?}",
            found[0].manifest_error
        );
    }

    /// The reason is a string on purpose: a boolean records that someone
    /// silenced a finding without recording why. `no_live_row: true` is
    /// therefore a manifest error, not a suppression.
    #[test]
    fn a_boolean_rowless_declaration_is_refused() {
        let root = tempfile::tempdir().unwrap();
        checkout(
            root.path(),
            "acme",
            Some("project: acme\nno_live_row: true\n"),
        );

        let found = scan(root.path()).unwrap();

        assert!(found[0].manifest_error.is_some());
        assert!(!analyze(&found, &[]).is_reconciled());
    }

    /// A manifest cannot introduce a code the rest of Navigator would refuse,
    /// the same rule `store::sample_project::project_code_from_manifest`
    /// applies to a bundle.
    #[test]
    fn the_scan_refuses_a_manifest_naming_an_invalid_code() {
        let root = tempfile::tempdir().unwrap();
        checkout(root.path(), "acme", Some("project: Not A Code\n"));

        let found = scan(root.path()).unwrap();

        assert!(
            found[0]
                .manifest_error
                .as_deref()
                .is_some_and(|detail| detail.contains("not a valid Project code")),
            "{:?}",
            found[0].manifest_error
        );
    }
}
