//! `navigator projects drift` — reconcile Project repositories against live
//! Project rows.
//!
//! One `projects.code` names both a repository and a row, and nothing makes
//! the two agree. A repository declares its code in `navigator.yaml`; a row
//! records its repository in `projects.repository_url`. Either side can be
//! written without the other, and neither side complains — so a repository can
//! publish a portal under `<code>/portal/` that no row mounts, and a row can
//! carry a repository URL that names nothing.
//!
//! Both failures are silent by construction. A portal publish under an
//! unmounted prefix *succeeds*; the mount then 404s for a reason that reads as
//! a missing bundle. And `repository_url` is `Option<String>`
//! ([`store::projects::Project::repository_url`]), so a row that never
//! recorded one is indistinguishable at a glance from a row that did — which
//! matters because a repository is resolved to its row *through* that column.
//!
//! # Why the live rows come from the JSON route
//!
//! `GET /app/projects.csv` — the route `site projects list` uses — emits
//! `id, code, name, status, entity_name` (`portal::admin::projects_csv`) and
//! carries no `repository_url`. It can only answer "does this repository have
//! a row?", never "does this row have a repository?". `GET /app/api/projects`
//! serializes the whole [`store::projects::Project`], so the reverse direction
//! is answerable without widening a CSV contract other readers depend on.
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

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use comfy_table::{presets::UTF8_FULL, Cell, ContentArrangement, Table};

use crate::palette;
use crate::projects::doctor::Status;

/// The manifest a Project repository declares its Project in. Distinct from
/// `store::sample_project::MANIFEST_FILE` (`navigator.yml`, keyed `name:`),
/// which is a bundle's publish manifest and a different contract.
pub const MANIFEST: &str = "navigator.yaml";
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
    no_live_row: Option<String>,
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

/// One live Project row, as `GET /app/api/projects` serializes it.
///
/// A deliberately narrow view of [`store::projects::Project`]: this command
/// reconciles codes and repository URLs, and reading the rest of a matter's
/// row — its entity, its Drive folder, its Slack channels — would pull client
/// detail into a report that does not need it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct LiveProject {
    pub code: String,
    #[serde(default)]
    pub repository_url: Option<String>,
}

/// What the two sides disagree about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// A repository whose code no live row carries.
    RepositoryHasNoRow { repository: String, code: String },
    /// A repository that declares it is meant to have no row, and why.
    RowlessByDeclaration {
        repository: String,
        code: String,
        reason: String,
    },
    /// A row recording no repository at all. The column is optional, so this
    /// is the state that fails silently: nothing resolves a repository back to
    /// this row, and nothing says so.
    RowHasNoRepositoryUrl { code: String },
    /// A row whose `repository_url` names a repository not present under the
    /// scan root. A forge rename leaves a redirect, so the stale URL keeps
    /// resolving over HTTP and this is the only thing that notices.
    RowRepositoryAbsent {
        code: String,
        url: String,
        named: String,
    },
    /// A row and the repository its URL names declare different codes.
    CodeMismatch {
        repository: String,
        declared: String,
        row: String,
    },
    /// A repository whose manifest declares a code other than its own name.
    /// Legal today — nothing derives one from the other — and worth seeing,
    /// because it is the shape that lets a repository assert the wrong code as
    /// settled fact.
    ManifestDisagreesWithName { repository: String, declared: String },
    /// Two checkouts claiming one Project code.
    DuplicateCode { code: String, repositories: Vec<String> },
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
            | Self::RowHasNoRepositoryUrl { .. }
            | Self::RowRepositoryAbsent { .. }
            | Self::CodeMismatch { .. }
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
            Self::RowHasNoRepositoryUrl { .. } => "row-has-no-repository-url",
            Self::RowRepositoryAbsent { .. } => "row-repository-absent",
            Self::CodeMismatch { .. } => "code-mismatch",
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
            | Self::RowHasNoRepositoryUrl { code }
            | Self::RowRepositoryAbsent { code, .. }
            | Self::DuplicateCode { code, .. } => code,
            Self::CodeMismatch { repository, .. }
            | Self::ManifestDisagreesWithName { repository, .. }
            | Self::UnreadableManifest { repository, .. }
            | Self::NoManifest { repository } => repository,
        }
    }

    /// One sentence naming what disagrees with what.
    ///
    /// `scan_root` is quoted into the repository-absent case on purpose: run
    /// against a directory holding part of the fleet, that finding is true but
    /// uninteresting, and naming the directory is what lets a reader tell the
    /// two apart without a flag.
    #[must_use]
    pub fn detail(&self, scan_root: &str) -> String {
        match self {
            Self::RepositoryHasNoRow { repository, code } => format!(
                "`{repository}` declares Project `{code}`, which no live row carries; \
                 a portal published under `{code}/portal/` would mount nowhere"
            ),
            Self::RowlessByDeclaration { reason, .. } => {
                format!("declared to have no live row: {reason}")
            }
            Self::RowHasNoRepositoryUrl { code } => format!(
                "row `{code}` records no repository_url, so nothing resolves a repository \
                 back to it"
            ),
            Self::RowRepositoryAbsent { url, named, .. } => format!(
                "row names `{url}`, and no repository `{named}` is present under {scan_root}; \
                 a forge rename leaves a redirect, so the stale URL still resolves"
            ),
            Self::CodeMismatch {
                repository,
                declared,
                row,
            } => format!(
                "`{repository}` declares Project `{declared}`, but the row naming it as its \
                 repository is `{row}`"
            ),
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
    /// Live rows read from the host.
    pub rows: usize,
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

/// The repository a `repository_url` names.
///
/// The last path segment, with a trailing `.git` removed. A whole URL is
/// stored rather than a name ([`store::projects::Project::repository_url`]),
/// and that segment is what a checkout on disk is named for — the same
/// assumption `super::repository::validate` makes when it takes the Project
/// code from the directory name.
#[must_use]
pub fn repository_named_by(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let segment = trimmed.rsplit('/').next()?;
    let segment = segment.strip_suffix(".git").unwrap_or(segment);
    // A URL with no path at all leaves the host as its last segment. That is
    // not special-cased: no checkout is named `github.com`, so it reports as
    // naming nothing, which is both true and what a reader needs to see.
    if segment.is_empty() {
        return None;
    }
    Some(segment.to_string())
}

/// Compare the two sides. Pure: every input is already read.
///
/// Order is deliberate — repository-side findings first, then row-side, then
/// whole-fleet integrity — so a reader walks the same path the drift takes.
#[must_use]
pub fn analyze(repositories: &[ScannedRepository], rows: &[LiveProject]) -> Report {
    let mut findings = Vec::new();

    let rows_by_code: BTreeMap<&str, &LiveProject> =
        rows.iter().map(|row| (row.code.as_str(), row)).collect();

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
        if rows_by_code.contains_key(code) {
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

    // ── Row side ───────────────────────────────────────────────────────────
    let by_directory: BTreeMap<&str, &ScannedRepository> = repositories
        .iter()
        .map(|repository| (repository.directory.as_str(), repository))
        .collect();

    for row in rows {
        let Some(url) = row
            .repository_url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
        else {
            findings.push(Finding::RowHasNoRepositoryUrl {
                code: row.code.clone(),
            });
            continue;
        };
        let named = repository_named_by(url).unwrap_or_default();
        match by_directory.get(named.as_str()) {
            None => findings.push(Finding::RowRepositoryAbsent {
                code: row.code.clone(),
                url: url.to_string(),
                named,
            }),
            Some(repository) => {
                if repository.manifest_error.is_none() && repository.code() != row.code {
                    findings.push(Finding::CodeMismatch {
                        repository: repository.directory.clone(),
                        declared: repository.code().to_string(),
                        row: row.code.clone(),
                    });
                }
            }
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
        rows: rows.len(),
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
    let reason = manifest
        .no_live_row
        .map(|reason| reason.trim().to_string())
        .filter(|reason| !reason.is_empty());
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
    let entries =
        std::fs::read_dir(root).with_context(|| format!("read the scan root {}", root.display()))?;
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

/// Read the live rows. `GET /app/api/projects` rather than the CSV route,
/// because the CSV carries no `repository_url`.
async fn fetch_rows(base: &str, token: &str) -> Result<Vec<LiveProject>> {
    let response = reqwest::Client::new()
        .get(format!("{base}/app/api/projects"))
        .bearer_auth(token)
        .send()
        .await
        .context("GET /app/api/projects")?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!("listing live projects failed: {status}"));
    }
    serde_json::from_str(&body).context("parse the live project rows")
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
                Cell::new(finding.detail(scan_root)),
            ]);
        }
        println!("{table}");
    }

    let declared = report.of(Status::Ok).len();
    println!(
        "{}",
        palette::dim(format!(
            "{} repositories under {scan_root}, {} live rows: {} drifted, {} to review, \
             {declared} declared row-less",
            report.repositories,
            report.rows,
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
        "rows": report.rows,
        "reconciled": report.is_reconciled(),
        "findings": report
            .findings
            .iter()
            .map(|finding| serde_json::json!({
                "status": match finding.status() {
                    Status::Ok => "ok",
                    Status::Warn => "warn",
                    Status::Fail => "fail",
                },
                "kind": finding.kind(),
                "subject": finding.subject(),
                "detail": finding.detail(scan_root),
            }))
            .collect::<Vec<_>>(),
    })
}

/// `navigator projects drift [--host h] [--dir d] [--all] [--json]`.
///
/// Read-only on both sides: it reads manifests off the local disk and lists
/// live rows over a bearer token. Nothing is created, patched, or closed —
/// reconciling a repository to a row is a decision about a matter, not a
/// mechanical fix, so this command reports and stops.
pub async fn run(host: Option<&str>, dir: &Path, all: bool, json: bool) -> ExitCode {
    let scan_root = dir.display().to_string();
    let repositories = match scan(dir) {
        Ok(repositories) => repositories,
        Err(error) => {
            eprintln!("navigator: {error:#}");
            return ExitCode::from(2);
        }
    };
    let rows = match crate::remote::resolve(host) {
        Ok((base, token)) => match fetch_rows(&base, &token).await {
            Ok(rows) => rows,
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

    let report = analyze(&repositories, &rows);
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
    use super::{
        analyze, repository_named_by, scan, Finding, LiveProject, ScannedRepository, MANIFEST,
    };
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

    fn row(code: &str, url: Option<&str>) -> LiveProject {
        LiveProject {
            code: code.to_string(),
            repository_url: url.map(str::to_string),
        }
    }

    fn kinds(findings: &[Finding]) -> Vec<&'static str> {
        findings.iter().map(Finding::kind).collect()
    }

    fn kinds_of(findings: &[&Finding]) -> Vec<&'static str> {
        findings.iter().map(|finding| finding.kind()).collect()
    }

    #[test]
    fn a_repository_matched_to_its_row_reports_nothing() {
        let report = analyze(
            &[repository("acme")],
            &[row("acme", Some("https://github.com/an-org/acme"))],
        );

        assert_eq!(kinds(&report.findings), Vec::<&str>::new());
        assert!(report.is_reconciled());
    }

    #[test]
    fn a_repository_whose_code_no_row_carries_is_drift() {
        let report = analyze(&[repository("acme")], &[]);

        assert_eq!(kinds(&report.findings), vec!["repository-has-no-row"]);
        assert!(!report.is_reconciled());
    }

    // ── The design decision: intentional absence is declared, not inferred ──

    /// The whole point of the `no_live_row:` key. A repository that declares it
    /// is meant to have no row is not drift, and the fleet stays reconciled.
    #[test]
    fn a_repository_declaring_no_live_row_is_not_drift() {
        let report = analyze(&[declared_rowless("acme")], &[]);

        assert_eq!(kinds(&report.findings), vec!["rowless-by-declaration"]);
        assert_eq!(report.findings[0].status(), Status::Ok);
        assert!(report.is_reconciled());
    }

    /// Suppressed is not silent. The declaration removes a *failure*, not the
    /// repository: it is still counted, and `Report::of(Ok)` is what the footer
    /// and `--all` list from.
    #[test]
    fn a_declared_rowless_repository_is_still_counted() {
        let report = analyze(&[declared_rowless("acme"), repository("beta")], &[]);

        assert_eq!(report.repositories, 2);
        assert_eq!(report.of(Status::Ok).len(), 1);
        assert_eq!(report.of(Status::Fail).len(), 1);
    }

    /// The declaration only speaks to the absence of a row. A repository that
    /// declares it *and* has one is fully reconciled, and says nothing.
    #[test]
    fn a_declaration_is_inert_once_the_row_exists() {
        let report = analyze(
            &[declared_rowless("acme")],
            &[row("acme", Some("https://github.com/an-org/acme"))],
        );

        assert_eq!(kinds(&report.findings), Vec::<&str>::new());
    }

    // ── The row side, which is the direction nothing checked ───────────────

    /// The state that failed across a whole fleet without reporting anything:
    /// the column is optional, so a row that never recorded a repository reads
    /// exactly like one that did.
    #[test]
    fn a_row_recording_no_repository_url_is_its_own_finding() {
        let report = analyze(&[repository("acme")], &[row("acme", None)]);

        assert_eq!(kinds(&report.findings), vec!["row-has-no-repository-url"]);
        assert!(!report.is_reconciled());
    }

    /// An empty string is the same failure as a missing value, and must not
    /// fall through to "names a repository called nothing".
    #[test]
    fn a_row_recording_a_blank_repository_url_is_the_same_finding() {
        let report = analyze(&[repository("acme")], &[row("acme", Some("   "))]);

        assert_eq!(kinds(&report.findings), vec!["row-has-no-repository-url"]);
    }

    /// The reverse direction, and the reason it needs a command at all: a forge
    /// rename leaves a redirect, so the stale URL keeps resolving over HTTP and
    /// nothing goes red. Here the checkout has been renamed and the row still
    /// names the repository under its old name.
    #[test]
    fn a_renamed_repository_leaves_the_row_naming_a_repository_that_is_gone() {
        let report = analyze(
            &[repository("acme-studio")],
            // A row's code is immutable, so it was the repository that moved;
            // the row's URL was not updated with it.
            &[row("acme-studio", Some("https://github.com/an-org/acme"))],
        );

        assert_eq!(kinds(&report.findings), vec!["row-repository-absent"]);
        assert!(!report.is_reconciled());
        assert!(
            report.findings[0].detail("/fleet").contains("acme"),
            "the finding must name the repository the row points at"
        );
    }

    /// Before the rename the same pair reconciles: the checkout is still named
    /// for the old repository and the manifest already declares the new code.
    /// The disagreement is worth seeing, but it is not a failure.
    #[test]
    fn the_same_pair_reconciles_before_the_rename() {
        let report = analyze(
            &[declaring("acme", "acme-studio")],
            &[row("acme-studio", Some("https://github.com/an-org/acme"))],
        );

        assert_eq!(kinds(&report.findings), vec!["manifest-disagrees-with-name"]);
        assert!(report.is_reconciled());
    }

    #[test]
    fn a_row_and_the_repository_it_names_may_disagree_about_the_code() {
        let report = analyze(
            &[declaring("acme", "beta")],
            &[row("acme", Some("https://github.com/an-org/acme"))],
        );

        assert!(
            kinds(&report.findings).contains(&"code-mismatch"),
            "{:?}",
            kinds(&report.findings)
        );
        assert!(!report.is_reconciled());
    }

    // ── Repository integrity ───────────────────────────────────────────────

    #[test]
    fn a_manifest_disagreeing_with_the_repository_name_warns_but_does_not_fail() {
        let report = analyze(&[declaring("acme", "beta")], &[row("beta", None)]);

        assert_eq!(
            kinds_of(&report.of(Status::Warn)),
            vec!["manifest-disagrees-with-name"]
        );
    }

    #[test]
    fn two_checkouts_claiming_one_code_is_a_failure() {
        let report = analyze(
            &[repository("acme"), declaring("acme-fork", "acme")],
            &[row("acme", Some("https://github.com/an-org/acme"))],
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

        let report = analyze(&[broken], &[]);

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

        let report = analyze(
            &[bare],
            &[row("acme", Some("https://github.com/an-org/acme"))],
        );

        assert_eq!(kinds(&report.findings), vec!["no-manifest"]);
        assert_eq!(report.findings[0].status(), Status::Warn);
        assert!(report.is_reconciled());
    }

    // ── The URL to repository rule ─────────────────────────────────────────

    #[test]
    fn a_repository_url_names_its_last_path_segment() {
        assert_eq!(
            repository_named_by("https://github.com/an-org/acme").as_deref(),
            Some("acme")
        );
        assert_eq!(
            repository_named_by("https://github.com/an-org/acme.git").as_deref(),
            Some("acme")
        );
        assert_eq!(
            repository_named_by("https://github.com/an-org/acme/").as_deref(),
            Some("acme")
        );
        assert_eq!(
            repository_named_by("https://gitlab.example/a-group/a-project").as_deref(),
            Some("a-project")
        );
    }

    /// A URL with no path leaves the host as its last segment. That is not
    /// special-cased: no checkout is named for a forge host, so it reports as
    /// naming nothing present, which is exactly right.
    #[test]
    fn a_url_with_no_path_names_no_repository_present() {
        let report = analyze(
            &[repository("acme")],
            &[row("acme", Some("https://forge.example"))],
        );

        assert_eq!(kinds(&report.findings), vec!["row-repository-absent"]);
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
        assert_eq!(found[1].rowless_reason.as_deref(), Some("the matter closed"));
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

    /// The live route serializes the whole `store::projects::Project`. The
    /// narrow view here must read that payload and ignore the rest of a
    /// matter's row rather than failing on an unexpected field.
    #[test]
    fn a_live_row_deserializes_from_the_whole_project_payload() {
        let payload = r#"[{
            "id": "6e8b6f2e-0f1e-4f0a-9c3a-2b7d5a1c9e44",
            "code": "acme",
            "name": "Acme Widgets",
            "status": "open",
            "entity_id": "0f9b1d5c-9a2e-4d3b-8c1f-7e6a4b2d0c11",
            "description": null,
            "drive_folder_id": null,
            "repository_url": "https://github.com/an-org/acme",
            "git_initialized_at": null,
            "forge_provisioned_at": null,
            "closed_at": null,
            "internal_slack_channel_url": null,
            "external_slack_channel_url": null,
            "private_notion_page_url": null,
            "shared_notion_page_url": null,
            "inserted_at": "2026-08-25T00:00:00Z",
            "updated_at": "2026-08-25T00:00:00Z"
        }]"#;

        let rows: Vec<LiveProject> = serde_json::from_str(payload).unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].code, "acme");
        assert_eq!(
            rows[0].repository_url.as_deref(),
            Some("https://github.com/an-org/acme")
        );
    }
}
