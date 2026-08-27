//! Reconcile Project rows against the repositories they record.
//!
//! One `projects.code` names a matter, a Drive folder, a route segment, an
//! object-storage prefix, and a repository. Nothing keeps the last of those
//! agreeing with the first, because `repository_url` is a free column written
//! by a person.
//!
//! # Why this reads rows and not checkouts
//!
//! The obvious way to check a repository against a row is to hold both — scan
//! a directory of clones, list the rows, compare. That shape has two defects
//! that are not fixable by writing it more carefully:
//!
//! - **A scan root is an assumption about a machine.** Every row-side answer it
//!   gives is conditional on the operator having cloned the whole fleet, and it
//!   is silently wrong when they have not. In a CI run — a Project repository's
//!   own workflow, the caller this exists for — exactly one checkout is
//!   present, so *every other matter* reads as a repository that is gone.
//! - **An HTTP project list is a lens, not an inventory.** `visible_projects`
//!   returns the caller's own matters ([`crate::access`]); every firm tier,
//!   Owner and Admin included, sees only matters it holds a participation row
//!   on. A read through it cannot distinguish "no such row" from "not yours".
//!
//! So this module reconciles what a row says against what a row must be true
//! of. Every failing finding here is computable from one row and a rule — no
//! checkout, no network, no fleet.
//!
//! # The rule that makes that possible
//!
//! A Project code *is* its repository name. That is enforced rather than
//! documented: [`cloud::workspace::is_valid_slug`] admits exactly the shape
//! that is simultaneously a URL segment, a repository name, and a Drive folder
//! name, and `crate::projects::is_valid_code` is that function plus the
//! reserved-code refusal. So a recorded URL whose last segment is not the row's
//! own code is drift, whatever forge or organization holds it.
//!
//! # What configuration adds, and what it must never add
//!
//! A deployment knows one `(host, organization)` pair — its creation target —
//! so it can compose the repository it *would* have created for a code
//! ([`cloud::workspace::WorkspaceConfig::expected_repository_url`]). Compared
//! against a recorded URL, that answers a genuinely useful question: is this
//! matter's source where this deployment puts things?
//!
//! It is a [`Severity::Warn`] and nothing more, for the reason
//! `Project::repository_url` became a stored URL in the first place: a
//! Project's source may live on any forge, in an organization the Firm does
//! not own. A deployment that has no pair configured — the local loop, the
//! test suite — simply skips that comparison and still gets every failure.
//! Nothing here composes a URL to stand in for one a row does not have.

use cloud::workspace::WorkspaceConfig;

use crate::projects::{is_valid_repository_url, Project};

/// How loudly to report a finding.
///
/// The split is not "how bad" but *who decided*: a [`Self::Fail`] contradicts a
/// rule Navigator enforces, and a [`Self::Warn`] describes a choice a person is
/// allowed to make and might not have meant to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    Warn,
    Fail,
}

/// The two columns reconciliation reads.
///
/// A deliberately narrow view of [`Project`]. A matter's row carries its
/// entity, its Drive folder, and its Slack and Notion addresses; a report about
/// repository naming needs none of that, and a report is a thing that gets
/// pasted into an issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowUnderReview<'a> {
    pub code: &'a str,
    pub repository_url: Option<&'a str>,
}

impl<'a> From<&'a Project> for RowUnderReview<'a> {
    fn from(project: &'a Project) -> Self {
        Self {
            code: &project.code,
            repository_url: project.repository_url.as_deref(),
        }
    }
}

/// What a row and its recorded repository disagree about.
///
/// Serialized internally tagged, so every variant's fields survive onto the
/// wire beside its `kind`. A consumer reads the field it needs by name; it
/// never parses a sentence, and adding a variant cannot silently change what an
/// existing one means.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Finding {
    /// The row records no repository at all.
    ///
    /// A warning, not a failure: the column is optional by design and most
    /// matters never get a repository. It is reported because *nothing* else
    /// reports it — a repository resolves back to its row through this column,
    /// so an unset one fails every resolution silently.
    NoRepositoryUrl { code: String },
    /// The recorded value is not a URL Navigator would accept today.
    ///
    /// Rows are validated on write ([`is_valid_repository_url`]), so this can
    /// only be a row written before that gate, or one written around it.
    RepositoryUrlInvalid { code: String, recorded: String },
    /// The recorded URL's last segment is not this row's code.
    ///
    /// The one failure that needs no configuration and no checkout. The code is
    /// the repository name, the portal mount, and the object prefix at once, so
    /// a row pointing at a differently-named repository has already broken the
    /// correspondence every one of those depends on.
    RepositoryNameIsNotCode {
        code: String,
        recorded: String,
        named: String,
    },
    /// The recorded URL is not the one this deployment would have created.
    ///
    /// Only ever a warning. A Project's source may live on another forge or in
    /// an organization the Firm does not own, and that is a supported state,
    /// not an error. Emitted only when a deployment has a configured pair to
    /// compare against.
    RepositoryOutsideDeploymentForge {
        code: String,
        recorded: String,
        expected: String,
    },
    /// Two or more matters record one repository.
    ///
    /// A failure, and the most consequential one here: a repository publishes
    /// its portal under the code it declares, so two matters sharing a
    /// repository means one matter's client can be served the other's bundle.
    DuplicateRepositoryUrl { url: String, codes: Vec<String> },
    /// The row records Navigator's own repository rather than a matter's.
    ///
    /// A Project is never Navigator. That repository is one fixed URL for every
    /// deployment ([`cloud::workspace::NAVIGATOR_REPOSITORY_URL`]) while a
    /// Project's differs for every matter, so the two are never the same value
    /// and a row holding it is a paste, not a configuration choice.
    ///
    /// Reported separately from [`Self::RepositoryNameIsNotCode`], which would
    /// otherwise absorb it into a generic naming complaint, because the
    /// consequence is specific: every rule that treats a Project repository as
    /// client-adjacent — the layout gate, the seed roots, a portal publish —
    /// would be pointed at the product's own source.
    RecordsNavigatorItself { code: String, recorded: String },
}

impl Finding {
    /// How loudly to report this.
    #[must_use]
    pub fn severity(&self) -> Severity {
        match self {
            Self::NoRepositoryUrl { .. } | Self::RepositoryOutsideDeploymentForge { .. } => {
                Severity::Warn
            }
            Self::RepositoryUrlInvalid { .. }
            | Self::RepositoryNameIsNotCode { .. }
            | Self::DuplicateRepositoryUrl { .. }
            | Self::RecordsNavigatorItself { .. } => Severity::Fail,
        }
    }

    /// The short category name, stable enough to key a gate on.
    ///
    /// The same string `serde` writes as `kind`, kept as a method so a caller
    /// that has not serialized the finding can still branch on it.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::NoRepositoryUrl { .. } => "no-repository-url",
            Self::RepositoryUrlInvalid { .. } => "repository-url-invalid",
            Self::RepositoryNameIsNotCode { .. } => "repository-name-is-not-code",
            Self::RepositoryOutsideDeploymentForge { .. } => "repository-outside-deployment-forge",
            Self::DuplicateRepositoryUrl { .. } => "duplicate-repository-url",
            Self::RecordsNavigatorItself { .. } => "records-navigator-itself",
        }
    }
}

/// One finding as it goes onto the wire: its severity beside its own fields.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReportedFinding<'a> {
    pub severity: Severity,
    #[serde(flatten)]
    pub finding: &'a Finding,
}

/// Every disagreement found, with the count a reader needs to trust it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub findings: Vec<Finding>,
    /// Rows examined. Printed because a report over a subset of the matters is
    /// a different claim from one over all of them.
    pub rows: usize,
    /// Whether a deployment forge pair was available to compare against. False
    /// means every [`Finding::RepositoryOutsideDeploymentForge`] was skipped,
    /// which a reader must be told rather than left to infer from an absence.
    pub compared_against_deployment_forge: bool,
}

impl Report {
    /// True when nothing failed. Warnings do not make a fleet drifted.
    #[must_use]
    pub fn is_reconciled(&self) -> bool {
        !self
            .findings
            .iter()
            .any(|finding| finding.severity() == Severity::Fail)
    }

    /// The findings of one severity, in report order.
    #[must_use]
    pub fn of(&self, severity: Severity) -> Vec<&Finding> {
        self.findings
            .iter()
            .filter(|finding| finding.severity() == severity)
            .collect()
    }

    /// The findings paired with their severities, for serialization.
    #[must_use]
    pub fn reported(&self) -> Vec<ReportedFinding<'_>> {
        self.findings
            .iter()
            .map(|finding| ReportedFinding {
                severity: finding.severity(),
                finding,
            })
            .collect()
    }
}

impl serde::Serialize for Report {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Report", 4)?;
        state.serialize_field("rows", &self.rows)?;
        state.serialize_field("reconciled", &self.is_reconciled())?;
        state.serialize_field(
            "compared_against_deployment_forge",
            &self.compared_against_deployment_forge,
        )?;
        state.serialize_field("findings", &self.reported())?;
        state.end()
    }
}

/// The repository a `repository_url` names.
///
/// The last path segment, with a trailing `.git` removed. A whole URL is
/// stored rather than a name, and that segment is the repository — which, by
/// [`cloud::workspace::is_valid_slug`], is also what the Project code must be.
///
/// Only meaningful for a URL [`is_valid_repository_url`] accepts; that gate
/// already requires a non-empty path, so a bare host never reaches here and
/// there is no "the host is the repository" case to special-case.
#[must_use]
pub fn repository_named_by(url: &str) -> Option<&str> {
    let trimmed = url.trim().trim_end_matches('/');
    let segment = trimmed.rsplit('/').next()?;
    let segment = segment.strip_suffix(".git").unwrap_or(segment);
    (!segment.is_empty()).then_some(segment)
}

/// Compare each row against what its code requires, and against what this
/// deployment would have created.
///
/// Pure: every input is already read. `deployment` is the configured forge
/// pair, or `None` where none is configured — the local loop and the test
/// suite, where every failure is still detected and only the
/// "would this deployment have put it here" warning is skipped.
///
/// Order is deliberate — per-row findings in row order, then the whole-set
/// integrity check — so two runs over the same rows produce the same report.
#[must_use]
pub fn reconcile(rows: &[RowUnderReview<'_>], deployment: Option<&WorkspaceConfig>) -> Report {
    let mut findings = Vec::new();

    for row in rows {
        let Some(recorded) = row
            .repository_url
            .map(str::trim)
            .filter(|url| !url.is_empty())
        else {
            findings.push(Finding::NoRepositoryUrl {
                code: row.code.to_string(),
            });
            continue;
        };

        if !is_valid_repository_url(recorded) {
            findings.push(Finding::RepositoryUrlInvalid {
                code: row.code.to_string(),
                recorded: recorded.to_string(),
            });
            continue;
        }

        // Checked before the naming rule, which would otherwise absorb this
        // into a generic "named something else" complaint. Navigator's own
        // repository is a specific mistake with a specific consequence, and a
        // reader needs to be told which one they made.
        if cloud::workspace::is_navigator_repository(recorded) {
            findings.push(Finding::RecordsNavigatorItself {
                code: row.code.to_string(),
                recorded: recorded.to_string(),
            });
            continue;
        }

        // The rule that needs nothing but the row: the code is the repository
        // name. Checked before the deployment comparison because it holds on
        // every forge, and a row that fails it is drifted regardless of where
        // the deployment would have put it.
        match repository_named_by(recorded) {
            Some(named) if named == row.code => {}
            Some(named) => findings.push(Finding::RepositoryNameIsNotCode {
                code: row.code.to_string(),
                recorded: recorded.to_string(),
                named: named.to_string(),
            }),
            None => findings.push(Finding::RepositoryUrlInvalid {
                code: row.code.to_string(),
                recorded: recorded.to_string(),
            }),
        }

        if let Some(config) = deployment {
            let expected = config.expected_repository_url(row.code);
            if expected != recorded {
                findings.push(Finding::RepositoryOutsideDeploymentForge {
                    code: row.code.to_string(),
                    recorded: recorded.to_string(),
                    expected,
                });
            }
        }
    }

    findings.extend(duplicate_repository_urls(rows));

    Report {
        findings,
        rows: rows.len(),
        compared_against_deployment_forge: deployment.is_some(),
    }
}

/// Matters recording one repository between them, in URL order.
///
/// Grouped on the trimmed URL rather than the repository name: two matters
/// whose URLs differ only by an organization are two repositories, and saying
/// otherwise would report a collision that is not one.
fn duplicate_repository_urls(rows: &[RowUnderReview<'_>]) -> Vec<Finding> {
    let mut by_url: std::collections::BTreeMap<&str, Vec<&str>> = std::collections::BTreeMap::new();
    for row in rows {
        if let Some(url) = row
            .repository_url
            .map(str::trim)
            .filter(|url| !url.is_empty())
        {
            by_url.entry(url).or_default().push(row.code);
        }
    }
    by_url
        .into_iter()
        .filter(|(_, codes)| codes.len() > 1)
        .map(|(url, codes)| Finding::DuplicateRepositoryUrl {
            url: url.to_string(),
            codes: codes.into_iter().map(str::to_string).collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{reconcile, repository_named_by, Finding, RowUnderReview, Severity};
    use cloud::workspace::{
        WorkspaceConfig, NAVIGATOR_GCP_PROJECT_ID, NAVIGATOR_GITHUB_ORG, NAVIGATOR_GIT_HOST,
    };
    use std::collections::HashMap;

    /// A synthetic organization, for the same reason `cloud::workspace`'s own
    /// fixtures use one: which organization holds a deployment's Project
    /// repositories is configuration, never a constant in this workspace.
    const AN_ORGANIZATION: &str = "an-organization";
    const A_HOST: &str = "forge.example";

    fn deployment() -> WorkspaceConfig {
        let pairs: HashMap<String, String> = [
            (NAVIGATOR_GCP_PROJECT_ID, "neon-law-stg"),
            (NAVIGATOR_GITHUB_ORG, AN_ORGANIZATION),
            (NAVIGATOR_GIT_HOST, A_HOST),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
        WorkspaceConfig::from_lookup(move |key| pairs.get(key).cloned())
            .expect("the fixture deployment resolves")
    }

    /// A row recording the repository this deployment would have created.
    fn row<'a>(code: &'a str, url: Option<&'a str>) -> RowUnderReview<'a> {
        RowUnderReview {
            code,
            repository_url: url,
        }
    }

    fn kinds(findings: &[Finding]) -> Vec<&'static str> {
        findings.iter().map(Finding::kind).collect()
    }

    fn kinds_of(findings: &[&Finding]) -> Vec<&'static str> {
        findings.iter().map(|finding| finding.kind()).collect()
    }

    // ── The rule that needs no configuration ───────────────────────────────

    /// The whole point of the module. A matter whose row points at a
    /// differently-named repository is drift, and proving it needs no
    /// checkout, no network, and no deployment configuration.
    #[test]
    fn a_repository_named_something_other_than_the_code_fails_with_no_configuration() {
        let report = reconcile(
            &[row(
                "acme-studio",
                Some("https://forge.example/an-organization/acme"),
            )],
            None,
        );

        assert_eq!(kinds(&report.findings), vec!["repository-name-is-not-code"]);
        assert!(!report.is_reconciled());
        assert!(!report.compared_against_deployment_forge);
    }

    /// The same failure on a forge the deployment has never heard of. The rule
    /// is about the code, not about where the repository is hosted.
    #[test]
    fn the_naming_rule_holds_on_a_forge_the_deployment_does_not_own() {
        let report = reconcile(
            &[row(
                "acme-studio",
                Some("https://gitlab.example/someone-else/acme"),
            )],
            Some(&deployment()),
        );

        assert!(
            kinds(&report.findings).contains(&"repository-name-is-not-code"),
            "{:?}",
            kinds(&report.findings)
        );
        assert!(!report.is_reconciled());
    }

    /// A matched row says nothing, and a `.git` suffix does not make it drift.
    #[test]
    fn a_row_naming_its_own_repository_reports_nothing() {
        for url in [
            "https://forge.example/an-organization/acme",
            "https://forge.example/an-organization/acme.git",
            "https://forge.example/an-organization/acme/",
        ] {
            let report = reconcile(&[row("acme", Some(url))], None);
            assert_eq!(kinds(&report.findings), Vec::<&str>::new(), "{url}");
            assert!(report.is_reconciled(), "{url}");
        }
    }

    // ── What a row alone cannot decide ─────────────────────────────────────

    /// Optional by design, so its absence is a warning. Reported because
    /// nothing else reports it: a repository resolves back to its row through
    /// this column.
    #[test]
    fn a_row_recording_no_repository_warns_rather_than_failing() {
        let report = reconcile(&[row("acme", None)], None);

        assert_eq!(kinds(&report.findings), vec!["no-repository-url"]);
        assert_eq!(report.findings[0].severity(), Severity::Warn);
        assert!(report.is_reconciled());
    }

    /// A blank string is the same state as a missing value, and must not fall
    /// through into the naming rule as a repository called nothing.
    #[test]
    fn a_blank_repository_url_is_the_same_warning() {
        let report = reconcile(&[row("acme", Some("   "))], None);

        assert_eq!(kinds(&report.findings), vec!["no-repository-url"]);
    }

    /// A Project's source may live anywhere, so being outside the deployment's
    /// own pair is a warning — never a failure. This is the finding that would
    /// have been wrong to grade any harder: it is exactly the state
    /// `repository_url` became a stored URL in order to permit.
    #[test]
    fn a_repository_in_another_organization_warns_but_does_not_fail() {
        let report = reconcile(
            &[row(
                "acme",
                Some("https://gitlab.example/someone-else/acme"),
            )],
            Some(&deployment()),
        );

        assert_eq!(
            kinds_of(&report.of(Severity::Warn)),
            vec!["repository-outside-deployment-forge"]
        );
        assert!(report.is_reconciled());
        assert!(report.compared_against_deployment_forge);
    }

    /// The comparison the deployment pair buys, in the case it is meant for:
    /// the row is where this deployment puts things, so it says nothing.
    #[test]
    fn a_repository_on_the_deployment_pair_is_silent() {
        let report = reconcile(
            &[row(
                "acme",
                Some("https://forge.example/an-organization/acme"),
            )],
            Some(&deployment()),
        );

        assert_eq!(kinds(&report.findings), Vec::<&str>::new());
    }

    /// Without a configured pair the comparison is skipped, and the report says
    /// so rather than leaving a reader to read the absence as agreement.
    #[test]
    fn an_unconfigured_deployment_skips_the_forge_comparison_and_says_so() {
        let elsewhere = row("acme", Some("https://gitlab.example/someone-else/acme"));

        let unconfigured = reconcile(&[elsewhere], None);
        assert_eq!(kinds(&unconfigured.findings), Vec::<&str>::new());
        assert!(!unconfigured.compared_against_deployment_forge);

        let configured = reconcile(&[elsewhere], Some(&deployment()));
        assert_eq!(
            kinds(&configured.findings),
            vec!["repository-outside-deployment-forge"]
        );
        assert!(configured.compared_against_deployment_forge);
    }

    // ── Whole-set integrity ────────────────────────────────────────────────

    /// The most consequential failure: a repository publishes its portal under
    /// the code it declares, so two matters on one repository can serve one
    /// client the other's bundle.
    #[test]
    fn two_matters_recording_one_repository_is_a_failure() {
        let report = reconcile(
            &[
                row("acme", Some("https://forge.example/an-organization/acme")),
                row("beta", Some("https://forge.example/an-organization/acme")),
            ],
            None,
        );

        let duplicates: Vec<&Finding> = report
            .findings
            .iter()
            .filter(|finding| finding.kind() == "duplicate-repository-url")
            .collect();
        assert_eq!(
            duplicates,
            vec![&Finding::DuplicateRepositoryUrl {
                url: "https://forge.example/an-organization/acme".to_string(),
                codes: vec!["acme".to_string(), "beta".to_string()],
            }],
            "the finding must name every matter claiming the repository"
        );
        assert!(!report.is_reconciled());
    }

    /// Two organizations are two repositories. Grouping on the name rather than
    /// the URL would report a collision that does not exist.
    #[test]
    fn the_same_repository_name_in_two_organizations_is_not_a_duplicate() {
        let report = reconcile(
            &[
                row("acme", Some("https://forge.example/an-organization/acme")),
                row("acme", Some("https://gitlab.example/another-org/acme")),
            ],
            None,
        );

        assert_eq!(kinds(&report.findings), Vec::<&str>::new());
    }

    // ── A Project is never Navigator ───────────────────────────────────────

    /// Navigator's own repository is one fixed URL for every deployment, and a
    /// Project's differs for every matter, so the two are never the same value.
    /// A row holding it is a paste — and it gets its own finding rather than
    /// the generic naming one, because the consequence is specific: every rule
    /// that treats a Project repository as client-adjacent would be pointed at
    /// the product's own source.
    #[test]
    fn a_row_recording_navigators_own_repository_is_its_own_failure() {
        let report = reconcile(
            &[row(
                "acme",
                Some("https://github.com/neon-law-source-code/navigator"),
            )],
            None,
        );

        assert_eq!(kinds(&report.findings), vec!["records-navigator-itself"]);
        assert!(!report.is_reconciled());
    }

    /// The point is to catch the paste, not to reward a tidy one. Every spelling
    /// a person plausibly pastes is the same repository.
    #[test]
    fn navigators_repository_is_recognized_however_it_was_pasted() {
        for recorded in [
            "https://github.com/neon-law-source-code/navigator",
            "https://github.com/neon-law-source-code/navigator/",
            "https://github.com/neon-law-source-code/navigator.git",
            "  https://github.com/neon-law-source-code/navigator  ",
            "https://GitHub.com/neon-law-source-code/navigator",
        ] {
            let report = reconcile(&[row("acme", Some(recorded))], None);
            assert_eq!(
                kinds(&report.findings),
                vec!["records-navigator-itself"],
                "{recorded}"
            );
        }
    }

    /// A different repository in the same organization is a Project's business.
    /// The rule is about one repository, not about a namespace — the Firm's own
    /// organization is a perfectly ordinary place for a matter to live.
    #[test]
    fn another_repository_in_navigators_organization_is_not_navigator() {
        let report = reconcile(
            &[row(
                "acme",
                Some("https://github.com/neon-law-source-code/acme"),
            )],
            None,
        );

        assert_eq!(kinds(&report.findings), Vec::<&str>::new());
        assert!(report.is_reconciled());
    }

    /// The other half of the same rule, enforced a step earlier: a matter can
    /// never be *coded* `navigator`, so the composed creation target can never
    /// be Navigator's repository either.
    #[test]
    fn navigator_is_not_an_acceptable_project_code() {
        assert!(
            !crate::projects::is_valid_code("navigator"),
            "a Project code is its repository name, so `navigator` would name Navigator itself"
        );
        assert!(
            cloud::workspace::is_valid_slug("navigator"),
            "the refusal is the reserved list, not the shape — otherwise this test proves nothing"
        );
    }

    // ── Values a row should never have held ────────────────────────────────

    /// Rows are validated on write, so this is a row written before that gate
    /// or around it. Refused rather than parsed, because the naming rule read
    /// off a malformed URL would be a guess.
    #[test]
    fn a_url_that_would_not_be_accepted_today_fails_without_being_parsed() {
        for recorded in [
            "https://forge.example",
            "forge.example/an-organization/acme",
            "file:///etc/passwd",
            "https://user:token@forge.example/an-organization/acme",
        ] {
            let report = reconcile(&[row("acme", Some(recorded))], None);
            assert_eq!(
                kinds(&report.findings),
                vec!["repository-url-invalid"],
                "{recorded}"
            );
            assert!(!report.is_reconciled(), "{recorded}");
        }
    }

    #[test]
    fn a_repository_url_names_its_last_path_segment() {
        assert_eq!(
            repository_named_by("https://forge.example/an-organization/acme"),
            Some("acme")
        );
        assert_eq!(
            repository_named_by("https://forge.example/a-group/a-subgroup/a-project"),
            Some("a-project")
        );
    }

    // ── The wire shape ─────────────────────────────────────────────────────

    /// The contract a gate reads. Every field of a finding survives beside its
    /// `kind` and its `severity`, so a consumer looks up what it needs by name
    /// instead of parsing a sentence.
    #[test]
    fn a_finding_serializes_with_its_own_fields_beside_its_kind() {
        let report = reconcile(
            &[row(
                "acme-studio",
                Some("https://forge.example/an-organization/acme"),
            )],
            None,
        );

        let json = serde_json::to_value(&report).expect("the report serializes");

        assert_eq!(json["rows"], 1);
        assert_eq!(json["reconciled"], false);
        assert_eq!(json["compared_against_deployment_forge"], false);
        assert_eq!(json["findings"][0]["severity"], "fail");
        assert_eq!(json["findings"][0]["kind"], "repository-name-is-not-code");
        assert_eq!(json["findings"][0]["code"], "acme-studio");
        assert_eq!(
            json["findings"][0]["recorded"],
            "https://forge.example/an-organization/acme"
        );
        assert_eq!(json["findings"][0]["named"], "acme");
    }

    /// Every variant's `kind` method and its serialized tag are the same
    /// string. They are written in two places, so a test holds them together
    /// rather than a comment asking the next author to remember.
    #[test]
    fn every_findings_kind_matches_its_serialized_tag() {
        let findings = [
            Finding::NoRepositoryUrl {
                code: "acme".into(),
            },
            Finding::RepositoryUrlInvalid {
                code: "acme".into(),
                recorded: "nope".into(),
            },
            Finding::RepositoryNameIsNotCode {
                code: "acme".into(),
                recorded: "https://forge.example/an-organization/beta".into(),
                named: "beta".into(),
            },
            Finding::RepositoryOutsideDeploymentForge {
                code: "acme".into(),
                recorded: "https://gitlab.example/other/acme".into(),
                expected: "https://forge.example/an-organization/acme".into(),
            },
            Finding::DuplicateRepositoryUrl {
                url: "https://forge.example/an-organization/acme".into(),
                codes: vec!["acme".into(), "beta".into()],
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

    /// A report over many rows keeps its count and orders per-row findings
    /// before the whole-set one, so two runs read the same.
    #[test]
    fn the_report_counts_every_row_it_examined() {
        let report = reconcile(
            &[
                row("acme", None),
                row("beta", Some("https://forge.example/an-organization/acme")),
                row("gamma", Some("https://forge.example/an-organization/gamma")),
            ],
            None,
        );

        assert_eq!(report.rows, 3);
        assert_eq!(
            kinds(&report.findings),
            vec!["no-repository-url", "repository-name-is-not-code"]
        );
    }
}
