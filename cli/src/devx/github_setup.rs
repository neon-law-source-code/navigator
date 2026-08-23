//! Reconciliation for repository-side GitHub policy.
//!
//! `ops github setup [repository]` keeps the merge gate reviewable in Rust
//! instead of in GitHub's settings UI, and it governs every repository the Firm
//! administers rather than a hard-coded pair. The target comes from the
//! argument, else `GITHUB_REPOSITORY`, else the checkout's `origin` remote.
//!
//! # The boundary is a `(host, organization)` pair
//!
//! Governance used to be scoped to the **host** alone, which ENG-58 chose for a
//! good reason: on a private tenant, *every repository on this host* and *every
//! repository the Firm owns* were the same set, so a host check needed no list
//! of organization names to keep true. On a public forge they are not the same
//! set at all — a host check there admits any repository on GitHub whose
//! checkout happens to be the working directory.
//!
//! So the boundary took the organization back, and is strictly tighter than the
//! host check it replaces. Two organizations are admissible: the public one
//! holding `navigator` itself, and the deployment's own
//! [`cloud::workspace::NAVIGATOR_GITHUB_ORG`]. Both halves come from
//! configuration through [`GovernedForge`], which supplies the public half from
//! source because Navigator's own URL is not configuration.
//!
//! There is a public-forge default now, and it is the common case rather than a
//! convenience: [`cloud::workspace::DEFAULT_GIT_HOST`] is where Navigator lives,
//! so a fresh clone, a laptop, or a CI job that sourced no deployment config can
//! run this command. What the old refusal was protecting against — a governance
//! write aimed at a host nobody chose — is now caught by the organization half.
//!
//! Policy is explicit and forks by organization: see [`policy_for`].

use std::collections::HashMap;
use std::env;
use std::fmt;
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use reqwest::header;
use serde::{Deserialize, Serialize};

/// The `(host, organization)` pair this command may write governance to.
///
/// This is the whole authorization boundary. `ops github setup` takes neither
/// half from the caller: it derives the repository, the organization, and the
/// API base from configuration and from a remote that must fall inside the
/// pair, so an incidental checkout of somebody else's repository cannot become
/// a write target by being the current directory.
#[derive(Debug)]
struct GovernedForge {
    /// The forge host, from [`cloud::workspace::WorkspaceConfig`].
    host: String,
    /// The organizations whose repositories this run may reconcile.
    ///
    /// Always the public organization holding `navigator`; additionally the
    /// deployment's own organization when a deployment is configured. Never a
    /// list from the caller.
    organizations: Vec<String>,
}

impl GovernedForge {
    /// Resolve the pair from configuration.
    ///
    /// A process that operates **no deployment** is the ordinary case here — a
    /// fresh clone, a laptop, a CI job reconciling the public repositories — and
    /// it governs the public organization on the default host. A process
    /// operating a **misconfigured** deployment is not the same thing and does
    /// not fall back: the error names the key, because silently governing only
    /// the public organization would look like success while doing less than
    /// asked.
    ///
    /// # Errors
    ///
    /// When a deployment is named but its coordinates do not resolve.
    fn from_lookup<F: Fn(&str) -> Option<String>>(get: F) -> Result<Self> {
        match cloud::workspace::WorkspaceConfig::from_lookup(get) {
            Ok(config) => Ok(Self {
                host: config.host,
                organizations: vec![public_organization().to_string(), config.organization],
            }),
            Err(cloud::workspace::WorkspaceConfigError::MissingDeployment) => Ok(Self {
                host: cloud::workspace::DEFAULT_GIT_HOST.to_string(),
                organizations: vec![public_organization().to_string()],
            }),
            Err(other) => Err(anyhow!(
                "resolve the (host, organization) pair `ops github setup` governs: {other}"
            )),
        }
    }

    /// The REST base for this host.
    ///
    /// Composed rather than spelled, so the API base and the boundary cannot
    /// disagree: a run reconciling a repository on one host can only ever talk
    /// to that host's API.
    fn api_base(&self) -> String {
        format!("https://api.{}", self.host)
    }

    /// Whether an `owner/name` slug falls inside the pair's organizations.
    fn admits(&self, slug: &str) -> bool {
        let owner = slug.split('/').next().unwrap_or_default();
        self.organizations
            .iter()
            .any(|org| org.eq_ignore_ascii_case(owner))
    }

    /// The refusal, spelling both halves so an operator can see which one they
    /// are outside.
    fn refuse(&self, what: &str, slug: &str) -> anyhow::Error {
        anyhow!(
            "{what} names {slug}, which is in none of the organizations `ops github setup` \
             governs on {}: {}",
            self.host,
            self.organizations.join(", ")
        )
    }
}

/// The one repository carrying more than [`COMMON_POLICY`].
const NAVIGATOR_SLUG: &str = "neon-law-source-code/navigator";

/// The organization Navigator itself lives in, and the one organization
/// admissible on every run.
///
/// Derived from [`NAVIGATOR_SLUG`] rather than written twice: Navigator's own
/// public URL is a source constant, so the organization holding it cannot
/// disagree with the slug naming it.
fn public_organization() -> &'static str {
    NAVIGATOR_SLUG
        .split('/')
        .next()
        .expect("NAVIGATOR_SLUG is an owner/name slug")
}

/// The Homebrew tap, the one administered repository outside the merge gate.
///
/// See [`TAP_POLICY`] for why a publication surface cannot carry one.
const TAP_SLUG: &str = "neon-law-source-code/homebrew-navigator";

const REPOSITORY_ENV: &str = "GITHUB_REPOSITORY";
const API_BASE_ENV: &str = "NAVIGATOR_GITHUB_API_BASE";
const TOKEN_ENV: &str = "GITHUB_TOKEN";
const APP_ID_ENV: &str = "NAVIGATOR_GITHUB_APP_ID";
const USER_AGENT: &str = concat!("neon-law-navigator/", env!("CARGO_PKG_VERSION"));
const API_VERSION: &str = "2022-11-28";
const BRANCH_RULESET_NAME: &str = "production";

const TAG_RULESET_NAME: &str = "release-tags";
const REVIEW_RULESET_NAME: &str = "production-review";
const LABEL_COLOR: &str = "6f42c1";

/// GitHub's own Actions App, which produces the `ci` check run the `production`
/// ruleset requires. The **slug** is a property of GitHub's App rather than of
/// any one host, so it is the same word on every deployment; only the numeric id
/// behind it differs.
const ACTIONS_APP_SLUG: &str = "github-actions";

/// The single status check every administered repository is gated on.
///
/// Repositories do not agree on what CI *does* — this workspace runs `cargo
/// test --workspace` on a large runner, `neon-law/ui` runs its own `lint`/`verify`
/// pair — and binding the gate to whatever the slowest job
/// happened to be called made the required context per-repository. That is
/// fragile in the one direction that fails silently: renaming a job renames
/// its check run, the ruleset keeps requiring the old spelling, and a context
/// nothing posts is a gate that never goes green.
///
/// So the contract is inverted. Every repository terminates its `ci` workflow
/// in one aggregating job spelled exactly `ci`, which succeeds only when the
/// real jobs behind it did. Those jobs stay free to differ per repository and
/// to be renamed at will; the required context never moves.
///
/// [`assert_required_check_job`] refuses to bind this gate on a repository
/// whose workflows do not actually define that job, so adopting the convention
/// is checked rather than assumed.
const REQUIRED_CHECK: &str = "ci";

/// Workflow files that may terminate in the [`REQUIRED_CHECK`] job, in the
/// order they are looked for.
///
/// Two spellings are live at once, and both are correct. A repository the Firm
/// has always administered carries `ci.yml`; a Project repository written by
/// `navigator projects repository scaffold` carries `gate.yml`. What they share
/// is the invariant that actually matters — a job whose check run is named
/// `ci` — so the gate accepts either filename and refuses only when neither
/// file exists or neither defines the job.
///
/// Accepting both is deliberate rather than transitional. The scaffold names
/// the file for what it is, and renaming it in every Project repository would
/// buy nothing: the required context is matched by job name, never by path.
const CI_WORKFLOW_PATHS: &[&str] = &[".github/workflows/ci.yml", ".github/workflows/gate.yml"];

/// The merge gate every repository the Firm *develops in* carries, with the
/// public organization's posture.
///
/// The gate half of this constant is common to both organizations; see
/// [`CLIENT_POLICY`] for the same gate under client-confidential defaults.
///
/// There is one lighter tier and one repository in it — the Homebrew tap, whose
/// `main` no human writes. Every other repository the Firm administers is held
/// to the same integrity rules and the same code-owner review as Navigator
/// itself; what varies is only the automation Navigator alone runs. Adding a
/// second exception is a policy decision, not a config change: see
/// [`TAP_POLICY`] for the test the tap passes and a source repository does not.
///
/// `assert_codeowners` is part of the common policy rather than a Navigator
/// extra because `review_gate` is meaningless without it: `require_code_owner_review`
/// against an unresolvable — or absent — CODEOWNERS silently passes anyone's
/// approval. The two ship together or neither means anything.
const COMMON_POLICY: RepositoryPolicy = RepositoryPolicy {
    default_visibility: Visibility::Public,
    open_source_governance: true,
    release_tags: false,
    labels: &[],
    assert_codeowners: true,
    assert_devx_app: false,
    review_gate: true,
    branch_protections: true,
};

/// The same gate, with client-confidential defaults: what a repository in the
/// deployment's own organization carries.
///
/// The gate does not change with the organization — a client matter's source is
/// a repository the Firm develops in, and it is held to the same integrity rules
/// and the same code-owner review as anything else. What changes is everything
/// about *publication*, and the two fields that differ are the two the fork
/// exists for.
///
/// The host check alone could never have expressed this. On a private tenant
/// there was one organization and therefore one posture; on a public forge the
/// same host holds both the repository whose whole point is to be readable by
/// anyone and the repository whose whole point is that it is not.
const CLIENT_POLICY: RepositoryPolicy = RepositoryPolicy {
    default_visibility: Visibility::Private,
    open_source_governance: false,
    release_tags: false,
    labels: &[],
    assert_codeowners: true,
    assert_devx_app: false,
    review_gate: true,
    branch_protections: true,
};

/// Navigator's own policy: the common gate plus the three things only this
/// repository does — cut release tags, drive `DevX` automation off labels, and
/// host the App installation that automation authenticates as.
const NAVIGATOR_POLICY: RepositoryPolicy = RepositoryPolicy {
    default_visibility: Visibility::Public,
    open_source_governance: true,
    release_tags: true,
    labels: &DEVX_LABELS,
    assert_codeowners: true,
    assert_devx_app: true,
    review_gate: true,
    branch_protections: true,
};

/// The Homebrew tap's policy: nothing on `main` at all.
///
/// A tap is not a repository the Firm develops in — it is the published output
/// of a release. Its `main` holds one mechanical file and grows by one commit
/// per Navigator release, written by its own `bump` workflow only after that
/// workflow has computed the digests, installed the formula, and tested the
/// binary it is about to publish. The verification a reviewer would perform has
/// already run, by machine, against the actual bytes.
///
/// Every rule in [`desired_branch_ruleset`] refuses that write rather than
/// governing it: `pull_request` admits no direct push, and `required_signatures`
/// rejects a runner's `git commit`, which GitHub verifies only for commits made
/// through the API or the web editor. A gated tap therefore reports a stale
/// version to everyone who installed through it while every check stays green —
/// which is exactly what happened, for three consecutive releases.
///
/// The assertions are off for the same reason and not as a convenience: the tap
/// has no CODEOWNERS to resolve and no `ci` job to bind, because it has no
/// reviewer and no test gate of the shape [`CI_WORKFLOW_PATHS`] describes. It
/// still receives the merge settings, which govern its occasional human pull
/// request and cannot block the bump.
const TAP_POLICY: RepositoryPolicy = RepositoryPolicy {
    default_visibility: Visibility::Public,
    open_source_governance: true,
    release_tags: false,
    labels: &[],
    assert_codeowners: false,
    assert_devx_app: false,
    review_gate: false,
    branch_protections: false,
};

/// The policy a repository is reconciled against, forked by organization first.
///
/// The organization is the question that decides publication, so it is asked
/// first: a repository in the public organization carries [`COMMON_POLICY`], and
/// one in the deployment's own carries [`CLIENT_POLICY`]. Only then do the two
/// repositories that carry something other than their organization's posture get
/// their own answer, and both of them live in the public organization —
/// `navigator`, which runs release automation nothing else runs, and the tap,
/// whose `main` no human writes.
///
/// Splitting on the slug alone was right while one hard-coded slug was the only
/// thing that differed. With two organizations it would answer the wrong
/// question: it would give a client matter's repository the posture of a
/// published one by default, which is the direction that fails badly.
fn policy_for(slug: &str) -> RepositoryPolicy {
    if slug.eq_ignore_ascii_case(NAVIGATOR_SLUG) {
        NAVIGATOR_POLICY
    } else if slug.eq_ignore_ascii_case(TAP_SLUG) {
        TAP_POLICY
    } else if in_public_organization(slug) {
        COMMON_POLICY
    } else {
        CLIENT_POLICY
    }
}

/// Whether a slug names a repository in the public organization.
fn in_public_organization(slug: &str) -> bool {
    slug.split('/')
        .next()
        .is_some_and(|owner| owner.eq_ignore_ascii_case(public_organization()))
}

/// A resolved repository inside the governed `(host, organization)` pair, with
/// the policy it will be reconciled against.
///
/// This used to be a two-variant enum, which meant a repository could not be
/// governed until someone added its name to the CLI. The authorization boundary
/// is now the pair: anything in an admissible organization on the configured
/// host is in scope and anything else is refused before a token is read, so a
/// checkout of somebody else's repository cannot become a write target by being
/// the current directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryTarget {
    slug: String,
    api_base: String,
    policy: RepositoryPolicy,
}

impl RepositoryTarget {
    /// Resolve the repository to reconcile, in precedence order: the explicit
    /// argument, then `GITHUB_REPOSITORY` (how a workflow names itself), then
    /// the `origin` remote of the current checkout.
    ///
    /// Only the remote carries a host, so only the remote path can fail the host
    /// half of the boundary. Every path yields an owner, so **all three** are
    /// held to the organization half — including the explicit argument, which
    /// under a host-only boundary was checked no further than its shape.
    ///
    /// Idempotency starts here: resolving twice from the same inputs yields the
    /// same target and the same policy, so a re-run reconciles rather than
    /// re-creating. `run` then converges each ruleset and label by reading the
    /// live state first and writing only a difference.
    pub fn resolve(explicit: Option<String>) -> Result<Self> {
        Self::resolve_within(
            explicit.or_else(|| optional_env(REPOSITORY_ENV)),
            &GovernedForge::from_lookup(|key| env::var(key).ok())?,
            optional_env(API_BASE_ENV),
        )
    }

    /// The resolution itself, with every environment read already done.
    ///
    /// Split out so the tests can drive the whole path — including the shape a
    /// fresh clone has, where nothing is configured — without mutating process
    /// environment that a parallel test is also reading.
    fn resolve_within(
        named: Option<String>,
        forge: &GovernedForge,
        api_base_override: Option<String>,
    ) -> Result<Self> {
        let (what, slug) = match named {
            Some(value) => ("the repository named", validate_slug(&value)?),
            None => ("the `origin` remote", origin_slug(forge)?),
        };
        if !forge.admits(&slug) {
            return Err(forge.refuse(what, &slug));
        }
        // A caller that overrides the base has already named the endpoint, so
        // the host's own base is not composed.
        let api_base = api_base_override.unwrap_or_else(|| forge.api_base());
        Ok(Self {
            api_base,
            policy: policy_for(&slug),
            slug,
        })
    }

    const fn policy(&self) -> RepositoryPolicy {
        self.policy
    }
}

impl fmt::Display for RepositoryTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.slug)
    }
}

/// Accept an `owner/name` slug and nothing else. A value carrying a scheme,
/// host, or extra path segment is a caller who meant a URL, and silently
/// reading the first two segments of one would reconcile the wrong repository.
fn validate_slug(value: &str) -> Result<String> {
    let value = value.trim().trim_matches('/');
    let mut parts = value.split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(owner), Some(name), None) if !owner.is_empty() && !name.is_empty() => {
            Ok(format!("{owner}/{name}"))
        }
        _ => bail!("expected a repository as `owner/name`, got {value:?}"),
    }
}

/// Read `origin` from the current checkout and reduce it to an `owner/name` on
/// the governed host.
fn origin_slug(forge: &GovernedForge) -> Result<String> {
    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .output()
        .context("run `git remote get-url origin`")?;
    if !output.status.success() {
        bail!(
            "`git remote get-url origin` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let url = String::from_utf8(output.stdout).context("decode the `origin` remote URL")?;
    slug_from_remote(url.trim(), &forge.host)
}

fn slug_from_remote(url: &str, governed_host: &str) -> Result<String> {
    let (host, path) =
        split_remote(url).ok_or_else(|| anyhow!("cannot parse the `origin` remote {url:?}"))?;
    if !host.eq_ignore_ascii_case(governed_host) {
        bail!(
            "`origin` points at {host}, not {governed_host}; `ops github setup` only \
             reconciles repositories on {governed_host}"
        );
    }
    let path = path.trim_matches('/');
    validate_slug(path.strip_suffix(".git").unwrap_or(path))
        .with_context(|| format!("read a repository out of the `origin` remote {url:?}"))
}

/// Split either remote spelling into `(host, path)`: the URL form
/// `scheme://[user@]host[:port]/path`, and Git's scp-like `[user@]host:path`.
fn split_remote(url: &str) -> Option<(&str, &str)> {
    if let Some((_scheme, rest)) = url.split_once("://") {
        let rest = rest.rsplit_once('@').map_or(rest, |(_, after)| after);
        let (authority, path) = rest.split_once('/')?;
        Some((
            authority
                .split_once(':')
                .map_or(authority, |(host, _)| host),
            path,
        ))
    } else {
        let rest = url.rsplit_once('@').map_or(url, |(_, after)| after);
        rest.split_once(':')
    }
}

/// Whether a repository is published or held to a client matter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Visibility {
    Public,
    Private,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(clippy::struct_excessive_bools)] // One independent switch per policy half.
struct RepositoryPolicy {
    /// Visibility a repository in this organization is **created** with.
    ///
    /// A creation-time default rather than a setting this command reconciles.
    /// `ops github setup` converges the policy of repositories that already
    /// exist, and flipping a live repository's visibility is not a convergence
    /// in either direction: it either publishes a client matter or unpublishes
    /// the product. Repository creation reads this — ENG-282 — and this command
    /// only carries it, which is why no [`Action`] writes it.
    default_visibility: Visibility,
    /// Whether this repository publishes the Firm's source-available
    /// governance file set: `LICENSE` carrying the `BUSL-1.1` text with its
    /// parameters filled in and its terms otherwise unaltered, `NOTICE` carrying
    /// the Firm's own statements, and a `CONTRIBUTING.md` stating that
    /// contributions are closed.
    ///
    /// False for a client matter, and not as an omission: a repository holding
    /// one client's confidential material publishes nothing and grants nobody
    /// anything, so a licence file there would describe rights that do not
    /// exist.
    open_source_governance: bool,
    release_tags: bool,
    labels: &'static [DesiredLabel],
    assert_codeowners: bool,
    assert_devx_app: bool,
    /// Whether merges additionally require a code owner's approval, enforced
    /// by the separate [`REVIEW_RULESET_NAME`] ruleset.
    review_gate: bool,
    /// Whether `main` carries the integrity half of the gate at all — the
    /// [`BRANCH_RULESET_NAME`] ruleset.
    ///
    /// True everywhere a person opens pull requests. False only for a surface
    /// whose `main` is written by a machine, where the same rules refuse the
    /// write instead of reviewing it.
    branch_protections: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DesiredLabel {
    name: &'static str,
    description: &'static str,
}

const DEVX_LABELS: [DesiredLabel; 4] = [
    DesiredLabel {
        name: "triage",
        description: "DevX: notify engineering that this issue is ready for triage",
    },
    DesiredLabel {
        name: "triaged",
        description: "DevX: issue grounded and planned; ready to implement",
    },
    DesiredLabel {
        name: "devx:paused",
        description: "DevX: automation stopped; needs a human",
    },
    DesiredLabel {
        name: "devx:failed",
        description: "DevX: last automated run failed; see the linked comment",
    },
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RulesetPayload {
    name: String,
    target: String,
    enforcement: String,
    bypass_actors: Vec<serde_json::Value>,
    conditions: Conditions,
    rules: Vec<Rule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Conditions {
    ref_name: RefName,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RefName {
    exclude: Vec<String>,
    include: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Rule {
    #[serde(rename = "type")]
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parameters: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct RulesetSummary {
    id: u64,
    name: String,
}

#[derive(Debug, Deserialize)]
#[allow(clippy::struct_excessive_bools)] // Mirrors GitHub's merge-settings fields.
struct Repository {
    allow_squash_merge: bool,
    allow_merge_commit: bool,
    allow_rebase_merge: bool,
    allow_auto_merge: bool,
    delete_branch_on_merge: bool,
    squash_merge_commit_title: String,
    squash_merge_commit_message: String,
}

#[derive(Debug, Serialize)]
#[allow(clippy::struct_excessive_bools)] // Mirrors GitHub's merge-settings payload.
struct MergeSettings {
    allow_squash_merge: bool,
    allow_merge_commit: bool,
    allow_rebase_merge: bool,
    allow_auto_merge: bool,
    delete_branch_on_merge: bool,
    squash_merge_commit_title: &'static str,
    squash_merge_commit_message: &'static str,
}

#[derive(Debug, Serialize, Deserialize)]
struct Label {
    name: String,
    description: Option<String>,
}

/// One App, as `GET /apps/{slug}` returns it. Only the id is read.
#[derive(Debug, Deserialize)]
struct App {
    id: u64,
}

#[derive(Debug, Deserialize)]
struct Installation {
    app_id: u64,
}

/// A planned remote change. Assertions do not appear here: they either hold
/// or stop the command before any write can happen.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    UpdateMergeSettings,
    CreateRuleset { name: String },
    UpdateRuleset { name: String },
    CreateLabel { name: String },
    UpdateLabel { name: String },
}

impl Action {
    fn description(&self) -> String {
        match self {
            Self::UpdateMergeSettings => "update merge settings".to_string(),
            Self::CreateRuleset { name } => format!("create ruleset {name}"),
            Self::UpdateRuleset { name } => format!("update ruleset {name}"),
            Self::CreateLabel { name } => format!("create label {name}"),
            Self::UpdateLabel { name } => format!("update label {name}"),
        }
    }
}

/// Every policy payload this command writes, in the order it reconciles them.
/// Kept as typed Rust data so a review sees every protected rule.
fn desired_rulesets(policy: RepositoryPolicy, actions_app_id: u64) -> Vec<RulesetPayload> {
    let mut rulesets = Vec::new();
    if policy.branch_protections {
        rulesets.push(desired_branch_ruleset(actions_app_id));
    }
    if policy.release_tags {
        rulesets.push(desired_tag_ruleset());
    }
    if policy.review_gate {
        rulesets.push(desired_review_ruleset());
    }
    rulesets
}

/// The integrity half of the merge gate: what is true of `main` for everyone,
/// with no exceptions.
///
/// This ruleset deliberately carries no bypass actor, so every rule in it binds
/// the Firm's own administrator too. It is the reason the review gate lives in
/// a *separate* ruleset ([`desired_review_ruleset`]) rather than as one more
/// parameter here — bypass in GitHub is granted per ruleset, never per rule, so
/// a single ruleset holding both halves would have forced one choice for both:
/// either nobody can merge their own work, or the administrator who can also
/// skips signing, linear history, and the test gate.
///
/// Splitting them buys the exact asymmetry the Firm wants. The administrator
/// bypasses approval and nothing else; `required_status_checks` and
/// `required_signatures` still apply to them, because those rules are here.
fn desired_branch_ruleset(actions_app_id: u64) -> RulesetPayload {
    RulesetPayload {
        name: BRANCH_RULESET_NAME.to_string(),
        target: "branch".to_string(),
        enforcement: "active".to_string(),
        // No actor bypasses the pull-request or status-check gate. The release
        // workflow only creates a tag at an already-reviewed `main` commit, so
        // it never needs branch-write authority.
        bypass_actors: Vec::new(),
        conditions: Conditions {
            ref_name: RefName {
                exclude: Vec::new(),
                include: vec!["~DEFAULT_BRANCH".to_string()],
            },
        },
        rules: vec![
            rule("deletion", None),
            rule("non_fast_forward", None),
            rule("required_linear_history", None),
            rule("required_signatures", None),
            // Coverage has no context of its own: `cargo llvm-cov
            // --fail-under-lines` runs inside `cargo test (workspace)` and fails
            // that check, so the floor is already required here.
            rule(
                "required_status_checks",
                Some(serde_json::json!({
                    "strict_required_status_checks_policy": false,
                    "do_not_enforce_on_create": false,
                    "required_status_checks": [
                        {"context": REQUIRED_CHECK, "integration_id": actions_app_id}
                    ]
                })),
            ),
            // The floor every merge clears regardless of who is merging: a
            // pull request at all, squash-only, with every review thread
            // resolved. Approval count stays 0 *here* on purpose — requiring
            // it in this un-bypassable ruleset would deadlock the sole code
            // owner, who cannot approve their own pull request. The approval
            // requirement is layered on top by `desired_review_ruleset`, which
            // the administrator bypasses.
            //
            // Rules of the same type in two rulesets do not replace one
            // another; GitHub applies the union and the most restrictive value
            // wins. A contributor is therefore held to 1 approval by the other
            // ruleset while the administrator falls back to this 0 — and both
            // are still held to squash, thread resolution, and the checks
            // above.
            rule(
                "pull_request",
                Some(serde_json::json!({
                    "required_approving_review_count": 0,
                    "dismiss_stale_reviews_on_push": true,
                    "required_reviewers": [],
                    "require_code_owner_review": false,
                    "dismissal_restriction": {"enabled": false, "allowed_actors": []},
                    "require_last_push_approval": false,
                    "required_review_thread_resolution": true,
                    "allowed_merge_methods": ["squash"]
                })),
            ),
        ],
    }
}

/// Release tags are write-once. Nothing may move or delete one.
///
/// A `YY.M.D` tag is the record of which tree a release shipped from. Deploys,
/// image tags, and `navigator --version` all resolve through it, so a tag that
/// can be moved is a release whose contents can be rewritten after the fact —
/// and an incident review that cannot trust what it is reading.
fn desired_tag_ruleset() -> RulesetPayload {
    RulesetPayload {
        name: TAG_RULESET_NAME.to_string(),
        target: "tag".to_string(),
        enforcement: "active".to_string(),
        // Deliberately empty. Creating a tag is
        // not restricted here, so the nightly cut needs nothing beyond the
        // built-in GITHUB_TOKEN; an actor that
        // could delete or move one afterwards is exactly the actor this
        // ruleset exists to have none of.
        bypass_actors: Vec::new(),
        conditions: Conditions {
            ref_name: RefName {
                exclude: Vec::new(),
                // The calendar release tags `deploy.yml` cuts nightly and
                // fires on. Each component is one or more digits
                // with no leading zeros, so June is `6`.
                include: vec!["refs/tags/[0-9]*.[0-9]*.[0-9]*".to_string()],
            },
        },
        rules: vec![
            rule("deletion", None),
            // `update` blocks repointing the tag and `non_fast_forward` blocks
            // forcing it backwards. Both, because either one alone leaves a
            // way to change what a released version means.
            rule("update", None),
            rule("non_fast_forward", None),
        ],
    }
}

/// The review half of the merge gate: nobody lands code in a Firm repository
/// without a code owner's approval, except the code owner.
///
/// # Why this is a second ruleset
///
/// The requirement is asymmetric by necessity, not by preference. GitHub will
/// not let anyone approve their own pull request, so a code-owner requirement
/// applied uniformly does not mean "everything is reviewed" — it means the sole
/// code owner can never merge anything again, and auto-merge stops working for
/// the one person who maintains the repository. The requirement has to bind
/// contributors and release the owner.
///
/// Bypass is the mechanism GitHub provides for that, and it is scoped to a
/// whole ruleset. Hence the split: this ruleset holds *only* the approval
/// requirement, so the administrator's bypass buys them exactly one exemption.
/// Everything that must hold universally — the required `ci` check, signed
/// commits, linear history, squash-only, resolved threads — lives in
/// [`desired_branch_ruleset`], which no one bypasses.
///
/// # Why `OrganizationAdmin`
///
/// The bypass names a role, not a person. `OrganizationAdmin` resolves to
/// whoever owns the organization at evaluation time, so the policy survives the
/// administrator changing without a code edit, and it cannot silently widen the
/// way a hardcoded username or a `write`-role bypass would. GitHub stores this
/// actor with a null `actor_id`; the payload spells that out so a read-back
/// compares equal and the reconcile converges instead of rewriting the ruleset
/// on every run.
fn desired_review_ruleset() -> RulesetPayload {
    RulesetPayload {
        name: REVIEW_RULESET_NAME.to_string(),
        target: "branch".to_string(),
        enforcement: "active".to_string(),
        bypass_actors: vec![serde_json::json!({
            "actor_id": null,
            "actor_type": "OrganizationAdmin",
            "bypass_mode": "always"
        })],
        conditions: Conditions {
            ref_name: RefName {
                exclude: Vec::new(),
                include: vec!["~DEFAULT_BRANCH".to_string()],
            },
        },
        rules: vec![rule(
            "pull_request",
            Some(serde_json::json!({
                // One approval, and `require_code_owner_review` makes it the
                // approval of whoever `.github/CODEOWNERS` names for the
                // touched paths — not just any colleague's. `assert_codeowners`
                // refuses to write this ruleset onto a repository whose owners
                // do not resolve, because an unresolvable owner leaves every
                // path unowned and quietly turns this back into "any one
                // approval".
                "required_approving_review_count": 1,
                "require_code_owner_review": true,
                // A push after approval re-opens the review. Without this, an
                // approval collected on a benign diff carries over to whatever
                // is force-pushed onto the branch afterwards.
                "dismiss_stale_reviews_on_push": true,
                // Belt and braces on the same window: the head commit must
                // itself be approved, so a contributor cannot approve a
                // colleague's branch and then append their own final commit.
                "require_last_push_approval": true,
                "required_reviewers": [],
                "dismissal_restriction": {"enabled": false, "allowed_actors": []},
                "required_review_thread_resolution": true,
                "allowed_merge_methods": ["squash"]
            })),
        )],
    }
}

/// Compare a desired ruleset against the live one, ignoring the order GitHub
/// happens to return the rules in.
///
/// The API preserves whatever order a ruleset's rules were first stored in, and
/// that order is not stable across repositories: one created by hand through
/// REST comes back with `pull_request` ahead of `required_status_checks`, one
/// created by this command comes back the other way round. Comparing the
/// vectors positionally therefore reports drift forever on the hand-made ones —
/// every run issues a PUT, every following run still sees drift, and "no drift"
/// becomes unreachable, which is exactly when drift detection stops being worth
/// anything. Order carries no meaning in the API, so normalize it away.
fn ruleset_matches(desired: &RulesetPayload, live: &RulesetPayload) -> bool {
    fn by_kind(payload: &RulesetPayload) -> Vec<&Rule> {
        let mut rules: Vec<&Rule> = payload.rules.iter().collect();
        rules.sort_by(|left, right| left.kind.cmp(&right.kind));
        rules
    }
    desired.name == live.name
        && desired.target == live.target
        && desired.enforcement == live.enforcement
        && desired.bypass_actors == live.bypass_actors
        && desired.conditions == live.conditions
        && by_kind(desired) == by_kind(live)
}

fn rule(kind: &str, parameters: Option<serde_json::Value>) -> Rule {
    Rule {
        kind: kind.to_string(),
        parameters,
    }
}

fn ruleset_by_name(
    policy: RepositoryPolicy,
    actions_app_id: u64,
    name: &str,
) -> Result<RulesetPayload> {
    desired_rulesets(policy, actions_app_id)
        .into_iter()
        .find(|ruleset| ruleset.name == name)
        .ok_or_else(|| anyhow!("no desired ruleset named {name}"))
}

/// `live_rulesets` is positional: entry `i` is what the repository currently
/// holds for `desired_rulesets(target)[i]`, or `None` when that ruleset is missing.
fn plan(
    policy: RepositoryPolicy,
    actions_app_id: u64,
    merge_settings_match: bool,
    live_rulesets: &[Option<RulesetPayload>],
    labels: &[Label],
) -> Vec<Action> {
    let mut actions = Vec::new();
    if !merge_settings_match {
        actions.push(Action::UpdateMergeSettings);
    }
    for (desired, live) in desired_rulesets(policy, actions_app_id)
        .iter()
        .zip(live_rulesets)
    {
        match live {
            None => actions.push(Action::CreateRuleset {
                name: desired.name.clone(),
            }),
            Some(live) if !ruleset_matches(desired, live) => actions.push(Action::UpdateRuleset {
                name: desired.name.clone(),
            }),
            Some(_) => {}
        }
    }
    for desired in policy.labels {
        match labels.iter().find(|label| label.name == desired.name) {
            None => actions.push(Action::CreateLabel {
                name: desired.name.to_string(),
            }),
            Some(label) if label.description.as_deref() != Some(desired.description) => {
                actions.push(Action::UpdateLabel {
                    name: desired.name.to_string(),
                });
            }
            Some(_) => {}
        }
    }
    actions
}

struct GitHubClient {
    http: reqwest::Client,
    api_base: String,
    repository: String,
}

impl GitHubClient {
    fn from_env(repository: &RepositoryTarget) -> Result<Self> {
        let token = required_env(TOKEN_ENV)?;
        let api_base = repository.api_base.clone();
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            header::HeaderName::from_static("x-github-api-version"),
            header::HeaderValue::from_static(API_VERSION),
        );
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}")
                .parse()
                .context("encode GitHub authorization header")?,
        );
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .user_agent(USER_AGENT)
            .build()
            .context("build GitHub HTTP client")?;
        Ok(Self {
            http,
            api_base: api_base.trim_end_matches('/').to_string(),
            repository: repository.slug.clone(),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.api_base, path)
    }

    fn repo_path(&self, suffix: &str) -> String {
        format!("/repos/{}{}", self.repository, suffix)
    }

    async fn get_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T> {
        let url = self.url(path);
        let response = self.http.get(&url).send().await?;
        parse_json(response, &url).await
    }

    /// Fetch every label, following pagination. A single 100-item page would
    /// hide a `DevX` label that sorts onto a later page, and `plan()` would then
    /// try to recreate it — a duplicate-label error that breaks the otherwise
    /// idempotent reconcile.
    async fn get_all_labels(&self) -> Result<Vec<Label>> {
        let mut labels = Vec::new();
        let mut page = 1u32;
        loop {
            let mut chunk: Vec<Label> = self
                .get_json(&self.repo_path(&format!("/labels?per_page=100&page={page}")))
                .await?;
            let full_page = chunk.len() == 100;
            labels.append(&mut chunk);
            if !full_page {
                return Ok(labels);
            }
            page += 1;
        }
    }

    /// Whether a resource exists, distinguishing "absent" from "the request
    /// failed". A 404 is the answer; anything else that is not a success is an
    /// error, so a revoked token cannot be read as a missing account.
    async fn exists(&self, path: &str) -> Result<bool> {
        let url = self.url(path);
        let response = self.http.get(&url).send().await?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!("GitHub GET {url} returned {}: {body}", status.as_u16());
        }
        Ok(true)
    }

    /// A file's contents, distinguishing "absent" from "the request failed",
    /// on the same principle as [`Self::exists`]: a 404 is the answer, and
    /// anything else that is not a success is an error. Reading a 500 or a
    /// revoked token as a missing file would let this bind a gate on a
    /// repository whose workflows were never actually read.
    async fn get_optional_text(&self, path: &str) -> Result<Option<String>> {
        let url = self.url(path);
        let response = self
            .http
            .get(&url)
            .header(header::ACCEPT, "application/vnd.github.raw+json")
            .send()
            .await?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("GitHub GET {url} returned {}: {body}", status.as_u16());
        }
        Ok(Some(body))
    }

    async fn get_text(&self, path: &str) -> Result<String> {
        let url = self.url(path);
        let response = self
            .http
            .get(&url)
            .header(header::ACCEPT, "application/vnd.github.raw+json")
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("GitHub GET {url} returned {}: {body}", status.as_u16());
        }
        Ok(body)
    }

    async fn put_json(&self, path: &str, body: &impl Serialize) -> Result<()> {
        self.send_json(reqwest::Method::PUT, path, body).await
    }

    async fn post_json(&self, path: &str, body: &impl Serialize) -> Result<()> {
        self.send_json(reqwest::Method::POST, path, body).await
    }

    async fn patch_json(&self, path: &str, body: &impl Serialize) -> Result<()> {
        self.send_json(reqwest::Method::PATCH, path, body).await
    }

    async fn send_json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: &impl Serialize,
    ) -> Result<()> {
        let url = self.url(path);
        let response = self.http.request(method, &url).json(body).send().await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!("GitHub write {url} returned {}: {body}", status.as_u16());
        }
        Ok(())
    }
}

async fn parse_json<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
    url: &str,
) -> Result<T> {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("GitHub GET {url} returned {}: {body}", status.as_u16());
    }
    serde_json::from_str(&body).with_context(|| format!("decode GitHub response from {url}"))
}

/// Reconcile GitHub settings for the explicitly named repository.
pub fn run(target: &RepositoryTarget, dry_run: bool) -> Result<()> {
    tracing_subscriber::fmt::try_init().ok();
    let client = GitHubClient::from_env(target)?;
    let policy = target.policy();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    runtime.block_on(async move { reconcile(policy, &client, dry_run).await })
}

async fn reconcile(policy: RepositoryPolicy, client: &GitHubClient, dry_run: bool) -> Result<()> {
    eprintln!("==> reconciling {}", client.repository);
    let repository: Repository = client.get_json(&client.repo_path("")).await?;

    // Assertions run before any write, so a repository that cannot satisfy the
    // policy is left exactly as it was rather than half-reconciled.
    //
    // This one exists to stop `required_status_checks` binding a context that
    // nothing posts, so it is owed only where that rule is written. A policy
    // carrying no branch protections has no context to bind and no workflow to
    // demand — asserting anyway would refuse a repository this command is
    // configured for.
    if policy.branch_protections {
        assert_required_check_job(client).await?;
    }

    // Read before planning, and before any write, for the same reason: the
    // required-check rule is built from this id, so a host that cannot answer
    // must stop the reconcile rather than have one guessed for it.
    let actions_app_id = actions_integration_id(client).await?;
    eprintln!("==> {ACTIONS_APP_SLUG:?} App on this host is id {actions_app_id}");

    if policy.assert_codeowners {
        let codeowners = client
            .get_text(&client.repo_path("/contents/.github/CODEOWNERS"))
            .await?;
        let owners = assert_codeowners(&codeowners)?;
        assert_owners_resolve(client, &owners).await?;
        eprintln!("==> CODEOWNERS verified");
    }

    if policy.assert_devx_app {
        assert_app_installation(client).await?;
    }

    let summaries: Vec<RulesetSummary> = client.get_json(&client.repo_path("/rulesets")).await?;
    let mut ruleset_ids = HashMap::new();
    let mut live_rulesets = Vec::new();
    for desired in desired_rulesets(policy, actions_app_id) {
        let Some(summary) = summaries
            .iter()
            .find(|summary| summary.name == desired.name)
        else {
            live_rulesets.push(None);
            continue;
        };
        ruleset_ids.insert(desired.name.clone(), summary.id);
        live_rulesets.push(Some(
            client
                .get_json::<RulesetPayload>(&client.repo_path(&format!("/rulesets/{}", summary.id)))
                .await?,
        ));
    }
    let labels = if policy.labels.is_empty() {
        Vec::new()
    } else {
        client.get_all_labels().await?
    };
    let actions = plan(
        policy,
        actions_app_id,
        merge_settings_match(&repository),
        &live_rulesets,
        &labels,
    );

    if actions.is_empty() {
        eprintln!("==> GitHub settings already match the desired state");
        return Ok(());
    }
    for action in &actions {
        eprintln!(
            "{} {}",
            if dry_run { "would" } else { "will" },
            action.description()
        );
    }
    if dry_run {
        return Ok(());
    }
    for action in actions {
        apply(policy, actions_app_id, client, &ruleset_ids, action).await?;
    }
    eprintln!("==> GitHub settings reconciled");
    Ok(())
}

async fn apply(
    policy: RepositoryPolicy,
    actions_app_id: u64,
    client: &GitHubClient,
    ruleset_ids: &HashMap<String, u64>,
    action: Action,
) -> Result<()> {
    match action {
        Action::UpdateMergeSettings => {
            client
                .patch_json(&client.repo_path(""), &desired_merge_settings())
                .await
        }
        Action::CreateRuleset { name } => {
            client
                .post_json(
                    &client.repo_path("/rulesets"),
                    &ruleset_by_name(policy, actions_app_id, &name)?,
                )
                .await
        }
        Action::UpdateRuleset { name } => {
            let id = ruleset_ids
                .get(&name)
                .ok_or_else(|| anyhow!("no live id for ruleset {name}"))?;
            client
                .put_json(
                    &client.repo_path(&format!("/rulesets/{id}")),
                    &ruleset_by_name(policy, actions_app_id, &name)?,
                )
                .await
        }
        Action::CreateLabel { name } => {
            let desired = label_by_name(policy, &name)?;
            client
                .post_json(
                    &client.repo_path("/labels"),
                    &serde_json::json!({
                        "name": desired.name,
                        "description": desired.description,
                        "color": LABEL_COLOR,
                    }),
                )
                .await
        }
        Action::UpdateLabel { name } => {
            let desired = label_by_name(policy, &name)?;
            client
                .patch_json(
                    &client.repo_path(&format!(
                        "/labels/{}",
                        url::form_urlencoded::byte_serialize(name.as_bytes()).collect::<String>()
                    )),
                    &serde_json::json!({
                        "new_name": desired.name,
                        "description": desired.description,
                    }),
                )
                .await
        }
    }
}

fn label_by_name(policy: RepositoryPolicy, name: &str) -> Result<&'static DesiredLabel> {
    policy
        .labels
        .iter()
        .find(|label| label.name == name)
        .ok_or_else(|| anyhow!("no desired label named {name}"))
}

fn desired_merge_settings() -> MergeSettings {
    MergeSettings {
        allow_squash_merge: true,
        allow_merge_commit: false,
        allow_rebase_merge: false,
        allow_auto_merge: true,
        delete_branch_on_merge: true,
        squash_merge_commit_title: "PR_TITLE",
        squash_merge_commit_message: "PR_BODY",
    }
}

fn merge_settings_match(repository: &Repository) -> bool {
    let desired = desired_merge_settings();
    repository.allow_squash_merge == desired.allow_squash_merge
        && repository.allow_merge_commit == desired.allow_merge_commit
        && repository.allow_rebase_merge == desired.allow_rebase_merge
        && repository.allow_auto_merge == desired.allow_auto_merge
        && repository.delete_branch_on_merge == desired.delete_branch_on_merge
        && repository.squash_merge_commit_title == desired.squash_merge_commit_title
        && repository.squash_merge_commit_message == desired.squash_merge_commit_message
}

/// Every distinct owner named by a CODEOWNERS file, in first-seen order.
///
/// A rule is `<pattern> <owner>...`, so the first token is the path pattern and
/// every token after it is an owner. Comments and blank lines carry none.
fn codeowners_owners(contents: &str) -> Vec<String> {
    let mut owners: Vec<String> = Vec::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        for owner in line.split_whitespace().skip(1) {
            if !owners.iter().any(|seen| seen == owner) {
                owners.push(owner.to_string());
            }
        }
    }
    owners
}

fn assert_codeowners(contents: &str) -> Result<Vec<String>> {
    let owners = codeowners_owners(contents);
    if owners.is_empty() {
        bail!(".github/CODEOWNERS must contain at least one ownership rule");
    }
    Ok(owners)
}

/// Confirm every owner the file names actually exists on this host.
///
/// This is the assertion that makes `require_code_owner_review` mean something.
/// GitHub does not reject a CODEOWNERS entry it cannot resolve — it drops the
/// rule and leaves the matched paths unowned, which looks identical to having
/// no CODEOWNERS file at all. A repository can therefore sit for months with a
/// review gate switched on, a CODEOWNERS file committed, and no owner on any
/// path.
///
/// It is not a hypothetical. A handle carried over from a checkout on one forge
/// resolves nowhere on another that shares no account namespace with it — an
/// EMU-provisioned tenant and github.com being the pair that produced this
/// failure — and it fails exactly that quietly.
async fn assert_owners_resolve(client: &GitHubClient, owners: &[String]) -> Result<()> {
    for owner in owners {
        // An email owner cannot be resolved through the API — GitHub matches it
        // against the commit author address, not an account. Accept it.
        let Some(handle) = owner.strip_prefix('@') else {
            continue;
        };
        let (path, kind) = match handle.split_once('/') {
            Some((org, team)) => (format!("/orgs/{org}/teams/{team}"), "team"),
            None => (format!("/users/{handle}"), "user"),
        };
        if !client.exists(&path).await? {
            bail!(
                "CODEOWNERS names {owner}, which does not resolve to a {kind} on this host \
                 ({}). GitHub ignores an owner it cannot resolve, so the paths it covers \
                 would be left unowned and `require_code_owner_review` would pass anyone's \
                 review. Correct the handle to one that exists here.",
                client.api_base,
            );
        }
    }
    eprintln!("==> CODEOWNERS owners resolve: {}", owners.join(", "));
    Ok(())
}

/// Effective check-run names of every job in a workflow file.
///
/// A job reports under its `name:` when it sets one and under its key
/// otherwise, which is the same rule GitHub applies when it creates the check
/// run.
fn workflow_job_check_names(workflow: &str) -> Result<Vec<String>> {
    let document: serde_yaml::Value =
        serde_yaml::from_str(workflow).context("parse the CI workflow as YAML")?;
    let Some(jobs) = document.get("jobs").and_then(serde_yaml::Value::as_mapping) else {
        bail!("the CI workflow defines no `jobs:` mapping");
    };
    Ok(jobs
        .iter()
        .filter_map(|(key, job)| {
            job.get("name")
                .and_then(serde_yaml::Value::as_str)
                .map(str::to_string)
                .or_else(|| key.as_str().map(str::to_string))
        })
        .collect())
}

/// Refuse to require a status check the repository never posts.
///
/// A `required_status_checks` rule naming a context that no job produces does
/// not fail loudly — GitHub simply waits for a check run that never arrives, so
/// every pull request sits permanently "Expected". Binding the gate to a
/// standard name is only safe if the standard is actually adopted, so this
/// checks the workflow before the ruleset is written rather than after someone
/// notices nothing can merge.
async fn assert_required_check_job(client: &GitHubClient) -> Result<()> {
    let mut inspected: Vec<(&str, Vec<String>)> = Vec::new();
    for path in CI_WORKFLOW_PATHS {
        let Some(workflow) = client
            .get_optional_text(&client.repo_path(&format!("/contents/{path}")))
            .await
            .with_context(|| format!("read {path} from {}", client.repository))?
        else {
            continue;
        };
        let names = workflow_job_check_names(&workflow)?;
        if names.iter().any(|name| name == REQUIRED_CHECK) {
            eprintln!("==> {path} defines the required {REQUIRED_CHECK:?} job");
            return Ok(());
        }
        inspected.push((path, names));
    }

    // Two different problems with two different fixes, so they are not one
    // message: a repository with no workflow at all has nothing to aggregate
    // yet, while one whose workflow exists but ends in some other job name has
    // adopted CI without adopting the convention the gate is matched by.
    if inspected.is_empty() {
        bail!(
            "none of {} exist in {}. The gate would then require a context nothing posts, \
             leaving every pull request stuck on an expected check. Add a workflow ending in \
             an aggregating job named {REQUIRED_CHECK:?} that `needs:` the real jobs, then \
             re-run.",
            CI_WORKFLOW_PATHS.join(" or "),
            client.repository,
        );
    }
    let found = inspected
        .iter()
        .map(|(path, names)| format!("{path}: {}", names.join(", ")))
        .collect::<Vec<_>>()
        .join("; ");
    bail!(
        "no workflow defines a job whose check run is named {REQUIRED_CHECK:?} \
         (found: {found}). The gate would then require a context nothing posts, leaving every \
         pull request stuck on an expected check. Add an aggregating job named \
         {REQUIRED_CHECK:?} that `needs:` the real jobs, then re-run.",
    )
}

/// The numeric id of the Actions App **on the host this run addresses**, read
/// from that host.
///
/// The id is per-host — the same App is a different number on one forge than on
/// another — and requiring the `ci` context under the wrong one binds the rule
/// to an App that never posts there. GitHub accepts such a rule
/// and reports it as present, so the gate silently matches nothing and every
/// pull request looks guarded while being unguarded. That is worse than no
/// ruleset, because nothing looks wrong.
///
/// So it is **asked for rather than asserted**. A table of host-to-id mappings
/// would be a second coordinate to keep true, and the failure mode of getting it
/// wrong is silent; the host already knows the answer. This is also the check
/// a human would run by hand:
///
/// ```text
/// gh api repos/<owner>/<repo>/commits/<sha>/check-runs \
///     --jq '.check_runs[] | "\(.name) \(.app.id) \(.app.slug)"'
/// ```
///
/// A failure here stops the reconcile rather than falling back to a guess:
/// there is no default, because a wrong id writes a gate that does not gate.
async fn actions_integration_id(client: &GitHubClient) -> Result<u64> {
    let app: App = client
        .get_json(&format!("/apps/{ACTIONS_APP_SLUG}"))
        .await
        .with_context(|| {
            format!(
                "read the {ACTIONS_APP_SLUG:?} App id from this host; the required \
                 {REQUIRED_CHECK:?} context is bound to it, and a wrong id would leave the \
                 ruleset matching nothing"
            )
        })?;
    Ok(app.id)
}

async fn assert_app_installation(client: &GitHubClient) -> Result<()> {
    let Some(app_id) = optional_env(APP_ID_ENV) else {
        eprintln!("warning: {APP_ID_ENV} is unset; skipping DevX App installation assertion");
        return Ok(());
    };
    let expected = app_id
        .parse::<u64>()
        .with_context(|| format!("{APP_ID_ENV} must be numeric"))?;
    let installation: Installation = client.get_json(&client.repo_path("/installation")).await?;
    if installation.app_id == expected {
        Ok(())
    } else {
        bail!(
            "repository installation app_id {} does not match {APP_ID_ENV}={expected}",
            installation.app_id
        )
    }
}

fn required_env(name: &'static str) -> Result<String> {
    optional_env(name).ok_or_else(|| anyhow!("missing env var: {name}"))
}

fn optional_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_json, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Two arbitrary App ids. They are deliberately *not* either real
    /// deployment's number: what these tests prove is that the id is carried
    /// from wherever it was read to the rule that requires the check, and a
    /// fixture pinning a real one would assert a coordinate instead of a
    /// behaviour.
    const TEST_ACTIONS_APP_ID: u64 = 4242;
    const TEST_OTHER_APP_ID: u64 = 9931;

    #[tokio::test]
    async fn the_actions_app_id_is_read_from_the_host() {
        // The id is per-host and the host is the only authority on it, so it is
        // asked for rather than mapped from a name. A table of host-to-id pairs
        // would be a second coordinate to keep true, and getting it wrong fails
        // silently: the rule stays present and matches nothing.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/apps/{ACTIONS_APP_SLUG}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"id": TEST_ACTIONS_APP_ID})),
            )
            .mount(&server)
            .await;
        let client = test_client(&server);

        assert_eq!(
            actions_integration_id(&client).await.unwrap(),
            TEST_ACTIONS_APP_ID
        );
    }

    #[tokio::test]
    async fn a_host_that_cannot_name_its_actions_app_stops_the_reconcile() {
        // No fallback id. A guessed one writes a rule bound to an App that never
        // posts here, which reads as configured while gating nothing — strictly
        // worse than failing loudly.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/apps/{ACTIONS_APP_SLUG}")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let client = test_client(&server);

        let error = actions_integration_id(&client)
            .await
            .expect_err("a host that cannot answer must not yield a default id");
        assert!(
            format!("{error:#}").contains(ACTIONS_APP_SLUG),
            "the failure must name what it could not read, got: {error:#}"
        );
    }

    #[test]
    fn the_branch_ruleset_requires_ci_under_the_app_id_it_is_given() {
        // The id is threaded, not constant: the same policy reconciled against
        // two hosts must require the check under each host's own App.
        for id in [TEST_ACTIONS_APP_ID, TEST_OTHER_APP_ID] {
            let value = serde_json::to_value(desired_branch_ruleset(id)).unwrap();
            let checks = value["rules"]
                .as_array()
                .unwrap()
                .iter()
                .find(|rule| rule["type"] == "required_status_checks")
                .expect("a required_status_checks rule");
            assert_eq!(
                checks["parameters"]["required_status_checks"][0]["integration_id"],
                serde_json::json!(id),
            );
        }
    }

    fn live_ruleset() -> RulesetPayload {
        let mut ruleset = desired_branch_ruleset(TEST_ACTIONS_APP_ID);
        let pull_request = ruleset
            .rules
            .iter_mut()
            .find(|rule| rule.kind == "pull_request")
            .unwrap();
        pull_request.parameters = Some(serde_json::json!({
            "required_approving_review_count": 0,
            "dismiss_stale_reviews_on_push": false,
            "required_reviewers": [],
            "require_code_owner_review": false,
            "dismissal_restriction": {"enabled": false, "allowed_actors": []},
            "require_last_push_approval": false,
            "required_review_thread_resolution": true,
            "allowed_merge_methods": ["squash"]
        }));
        ruleset
    }

    fn matching_repository() -> Repository {
        Repository {
            allow_squash_merge: true,
            allow_merge_commit: false,
            allow_rebase_merge: false,
            allow_auto_merge: true,
            delete_branch_on_merge: true,
            squash_merge_commit_title: "PR_TITLE".to_string(),
            squash_merge_commit_message: "PR_BODY".to_string(),
        }
    }

    /// No actor may bypass the integrity gate — including the administrator who
    /// bypasses the review gate.
    ///
    /// This is the invariant the two-ruleset split exists to protect. Folding
    /// the approval requirement back into `production` would mean granting the
    /// administrator's bypass here, and a bypass here is a bypass of signing,
    /// linear history, and the `ci` check as well, because GitHub scopes bypass
    /// to the ruleset and not to the rule.
    #[test]
    fn branch_ruleset_has_no_bypass_actors() {
        assert_eq!(
            desired_branch_ruleset(TEST_ACTIONS_APP_ID).bypass_actors,
            Vec::<serde_json::Value>::new(),
        );
    }

    /// The review gate requires a code owner's approval and exempts exactly one
    /// actor: whoever owns the organization.
    #[test]
    fn review_ruleset_requires_code_owner_approval() {
        let value = serde_json::to_value(desired_review_ruleset()).unwrap();
        assert_eq!(value["name"], "production-review");
        assert_eq!(value["target"], "branch");
        assert_eq!(value["enforcement"], "active");
        assert_eq!(
            value["conditions"]["ref_name"]["include"],
            serde_json::json!(["~DEFAULT_BRANCH"])
        );
        let parameters = &value["rules"][0]["parameters"];
        assert_eq!(parameters["required_approving_review_count"], 1);
        assert_eq!(parameters["require_code_owner_review"], true);
        assert_eq!(parameters["dismiss_stale_reviews_on_push"], true);
        assert_eq!(parameters["require_last_push_approval"], true);
    }

    /// The bypass names the organization-owner *role*, never a person.
    ///
    /// A username would have to be re-edited when the administrator changes, and
    /// a `RepositoryRole` bypass would silently widen the exemption to everyone
    /// holding that role. The null `actor_id` is what GitHub stores for this
    /// actor type; spelling it in the desired payload is what lets a read-back
    /// compare equal so the reconcile converges instead of rewriting the ruleset
    /// on every run.
    #[test]
    fn review_ruleset_is_bypassed_only_by_the_organization_owner() {
        assert_eq!(
            desired_review_ruleset().bypass_actors,
            vec![serde_json::json!({
                "actor_id": null,
                "actor_type": "OrganizationAdmin",
                "bypass_mode": "always"
            })],
        );
    }

    /// Every governed repository carries both halves of the gate. Only the
    /// release-tag ruleset is Navigator's alone, because it is the only
    /// repository that cuts a release.
    #[test]
    fn every_repository_carries_both_halves_of_the_gate() {
        let names = |policy| {
            desired_rulesets(policy, TEST_ACTIONS_APP_ID)
                .into_iter()
                .map(|ruleset| ruleset.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(NAVIGATOR_POLICY),
            vec!["production", "release-tags", "production-review"]
        );
        assert_eq!(
            names(COMMON_POLICY),
            vec!["production", "production-review"]
        );
    }

    /// The Homebrew tap is governed by writing to it, and a rule that stops the
    /// write governs nothing.
    ///
    /// Its `main` is a machine-written log: one commit per Navigator release,
    /// pushed by the tap's own `bump` workflow after that workflow has installed
    /// and tested the formula it is about to publish. `pull_request` and
    /// `required_signatures` in the `production` ruleset each refuse that push
    /// outright — a runner's `git commit` is unsigned, and there is no reviewer
    /// for a mechanical digest bump — so a gated tap is a tap whose formula
    /// silently stops following releases. Three releases were lost that way
    /// before the ruleset came off.
    ///
    /// So the tap receives no rulesets at all. `desired_rulesets` never emits a
    /// `DeleteRuleset`, so an empty desired set also means a reconcile aimed
    /// here is a no-op rather than a removal — it cannot restore the gate, and
    /// it cannot take away one a human deliberately adds.
    #[test]
    fn the_tap_is_governed_by_writing_to_it_and_carries_no_rulesets() {
        assert_eq!(
            policy_for(TAP_SLUG),
            TAP_POLICY,
            "the tap must resolve to its own policy, not the common gate"
        );
        assert_eq!(
            policy_for("NEON-LAW-SOURCE-CODE/Homebrew-Navigator"),
            TAP_POLICY,
            "slugs are matched case-insensitively, as they are for Navigator itself"
        );
        assert!(
            desired_rulesets(TAP_POLICY, TEST_ACTIONS_APP_ID).is_empty(),
            "a ruleset on the tap refuses the bump push that is the tap's whole purpose"
        );
    }

    /// The tap must not trip an assertion written for a source repository.
    ///
    /// `reconcile` fails closed on a missing CODEOWNERS and on a missing `ci`
    /// job, and the tap has neither: it holds one formula and two workflows. Any
    /// policy that asserts them would abort a reconcile aimed here — which reads
    /// as the command refusing a repository it is in fact configured for.
    #[test]
    fn the_tap_asserts_nothing_a_publication_surface_cannot_have() {
        // Const blocks: every field is known at compile time, so these hold at
        // compile time too rather than waiting for the suite to run.
        const {
            assert!(
                !TAP_POLICY.assert_codeowners,
                "the tap has no CODEOWNERS, and no reviewer to name in one"
            );
        }
        const {
            assert!(
                !TAP_POLICY.assert_devx_app,
                "DevX automation runs against Navigator, not the tap"
            );
        }
        const {
            assert!(
                !TAP_POLICY.branch_protections,
                "the `production` ruleset is what refuses the bump push"
            );
        }
        const {
            assert!(
                TAP_POLICY.labels.is_empty() && !TAP_POLICY.release_tags,
                "the tap cuts no release and runs no label automation"
            );
        }
    }

    /// `require_code_owner_review` against an absent or unresolvable
    /// CODEOWNERS silently accepts anyone's approval, so a policy that gates
    /// on code owners without asserting the file is a gate that does nothing.
    #[test]
    fn the_review_gate_never_ships_without_the_codeowners_assertion() {
        for policy in [NAVIGATOR_POLICY, COMMON_POLICY, TAP_POLICY] {
            assert!(
                !policy.review_gate || policy.assert_codeowners,
                "{policy:?} gates on code owners without asserting CODEOWNERS"
            );
        }
    }

    #[test]
    fn codeowners_parses_every_owner_after_the_pattern() {
        let owners = codeowners_owners(
            "# comment\n\n* @nick\n/docs/ @nick @neon-law/counsel legal@example.com\n",
        );
        assert_eq!(
            owners,
            vec!["@nick", "@neon-law/counsel", "legal@example.com"],
            "owners are de-duplicated in first-seen order"
        );
    }

    #[test]
    fn codeowners_without_an_ownership_rule_is_rejected() {
        assert!(assert_codeowners("# only a comment\n\n").is_err());
        // A pattern with no owner is not an ownership rule.
        assert!(assert_codeowners("*\n").is_err());
    }

    /// The regression that motivated this assertion: a handle from one forge
    /// committed to a repository on another that shares no account namespace
    /// with it, where it resolves to nothing and GitHub therefore leaves every
    /// path unowned.
    #[tokio::test]
    async fn unresolvable_codeowner_fails_the_reconcile() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users/shicholas"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let client = test_client(&server);
        let error = assert_owners_resolve(&client, &["@shicholas".to_string()])
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("@shicholas"), "{error}");
        assert!(error.contains("does not resolve"), "{error}");
    }

    /// Users, teams, and email owners each resolve the way GitHub matches them.
    #[tokio::test]
    async fn resolvable_codeowners_pass() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users/nick"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/orgs/neon-law/teams/counsel"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;
        let client = test_client(&server);
        // The email owner is accepted without a request: GitHub matches it
        // against the commit author address, not against an account.
        assert_owners_resolve(
            &client,
            &[
                "@nick".to_string(),
                "@neon-law/counsel".to_string(),
                "legal@example.com".to_string(),
            ],
        )
        .await
        .unwrap();
    }

    /// A 500 is not an absent account. Treating every non-200 as "missing"
    /// would turn an expired token into a confident, wrong claim that the
    /// repository's code owner does not exist.
    #[tokio::test]
    async fn a_failed_lookup_is_not_reported_as_a_missing_owner() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users/nick"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let client = test_client(&server);
        let error = assert_owners_resolve(&client, &["@nick".to_string()])
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("500"), "{error}");
        assert!(!error.contains("does not resolve"), "{error}");
    }

    /// `required_signatures` applies to everyone because the branch gate has no
    /// bypass actor.
    #[test]
    fn branch_ruleset_still_requires_signatures_of_everyone() {
        let ruleset = desired_branch_ruleset(TEST_ACTIONS_APP_ID);
        assert!(ruleset
            .rules
            .iter()
            .any(|rule| rule.kind == "required_signatures"));
    }

    /// Release tags cannot be deleted, moved, or forced, and no actor is
    /// exempt. See `desired_tag_ruleset` for why that matters to a licensee.
    #[test]
    fn release_tag_ruleset_is_immutable_and_has_no_bypass() {
        let value = serde_json::to_value(desired_tag_ruleset()).unwrap();
        assert_eq!(value["name"], "release-tags");
        assert_eq!(value["target"], "tag");
        assert_eq!(value["enforcement"], "active");
        assert_eq!(value["bypass_actors"], serde_json::json!([]));
        assert_eq!(
            value["conditions"]["ref_name"]["include"],
            serde_json::json!(["refs/tags/[0-9]*.[0-9]*.[0-9]*"])
        );
        let kinds: Vec<&str> = value["rules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|rule| rule["type"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, vec!["deletion", "update", "non_fast_forward"]);
    }

    /// The ruleset must protect a `-hotfix.N` tag too, and it does so with no
    /// pattern change: GitHub matches `ref_name` conditions with fnmatch, whose
    /// `*` matches any character including `.` and `-`. That looseness is a
    /// liability for the workflow's own shape check — which is why `deploy.yml`
    /// anchors a real regex — but here it is exactly right: every tag the release
    /// workflow will ever accept is immutable.
    ///
    /// This is the property a licensee depends on. A hotfix publishes images and
    /// archives under its name, so a movable hotfix tag would let those bytes be
    /// relabelled after the fact.
    /// fnmatch semantics, narrowed to the `[0-9]` classes and `*` the
    /// release-tag pattern uses — enough to prove which tag shapes GitHub's
    /// `ref_name` condition covers.
    fn fnmatch(pattern: &[u8], name: &[u8]) -> bool {
        match pattern.first() {
            None => name.is_empty(),
            Some(b'*') => (0..=name.len()).any(|skip| fnmatch(&pattern[1..], &name[skip..])),
            Some(b'[') => {
                let close = pattern.iter().position(|&byte| byte == b']');
                match (close, name.first()) {
                    (Some(close), Some(&candidate)) => {
                        let class = &pattern[1..close];
                        let matched = class.windows(3).any(|window| {
                            window[1] == b'-' && candidate >= window[0] && candidate <= window[2]
                        });
                        matched && fnmatch(&pattern[close + 1..], &name[1..])
                    }
                    _ => false,
                }
            }
            Some(&literal) => match name.first() {
                Some(&candidate) if candidate == literal => fnmatch(&pattern[1..], &name[1..]),
                _ => false,
            },
        }
    }

    #[test]
    fn release_tag_ruleset_also_covers_hotfix_prereleases() {
        let value = serde_json::to_value(desired_tag_ruleset()).unwrap();
        let pattern = value["conditions"]["ref_name"]["include"][0]
            .as_str()
            .expect("the ruleset must include a tag pattern");
        let pattern = pattern
            .strip_prefix("refs/tags/")
            .expect("the pattern is scoped to tags");

        for tag in ["26.8.17", "26.8.18-hotfix.17", "26.12.25-hotfix.0"] {
            assert!(
                fnmatch(pattern.as_bytes(), tag.as_bytes()),
                "the release-tags ruleset must make {tag} immutable"
            );
        }
    }

    #[test]
    fn desired_ruleset_serializes_to_github_put_payload() {
        let value = serde_json::to_value(desired_branch_ruleset(TEST_ACTIONS_APP_ID)).unwrap();
        assert_eq!(value["name"], "production");
        assert_eq!(
            value["conditions"]["ref_name"]["include"],
            serde_json::json!(["~DEFAULT_BRANCH"])
        );
        assert_eq!(
            value["rules"][5]["parameters"]["required_approving_review_count"],
            0
        );
        assert_eq!(
            value["rules"][5]["parameters"]["require_code_owner_review"],
            false
        );
        assert_eq!(
            value["rules"][5]["parameters"]["required_review_thread_resolution"],
            true
        );
        assert_eq!(
            value["rules"].as_array().unwrap().len(),
            6,
            "ruleset carries exactly the six protected rules"
        );
    }

    /// The merge gate is the CI test job, bound to the App that actually posts
    /// it on this host.
    ///
    /// Both halves matter. A drifted *context* stops requiring the job that
    /// proves the workspace; a drifted *integration id* is worse, because the
    /// rule still looks present in the API while matching an App that never
    /// posts a check here — the gate reads as configured and enforces nothing.
    #[test]
    fn branch_ruleset_gates_on_the_ci_test_check() {
        let value = serde_json::to_value(desired_branch_ruleset(TEST_ACTIONS_APP_ID)).unwrap();
        let checks = value["rules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|rule| rule["type"] == "required_status_checks")
            .expect("the production ruleset must require a status check")["parameters"]
            ["required_status_checks"]
            .clone();

        assert_eq!(
            checks,
            serde_json::json!([
                {"context": REQUIRED_CHECK, "integration_id": TEST_ACTIONS_APP_ID}
            ]),
            "the merge gate is the `ci` aggregating job posted by GitHub Actions \
             ({TEST_ACTIONS_APP_ID}); the context must match a job's check-run \
             name in .github/workflows/ci.yml exactly"
        );
    }

    /// The required context is the same string on every administered
    /// repository. Per-repository check names are what let a rename unbind the
    /// gate silently.
    #[test]
    fn every_repository_requires_the_same_check_context() {
        for policy in [NAVIGATOR_POLICY, COMMON_POLICY] {
            let contexts = desired_rulesets(policy, TEST_ACTIONS_APP_ID)
                .into_iter()
                .flat_map(|ruleset| ruleset.rules)
                .filter(|rule| rule.kind == "required_status_checks")
                .map(|rule| {
                    rule.parameters.unwrap()["required_status_checks"][0]["context"].clone()
                })
                .collect::<Vec<_>>();
            assert_eq!(contexts, vec![serde_json::json!("ci")], "{policy:?}");
        }
    }

    /// The repository's own workflow satisfies the convention the policy
    /// requires. Without this the two drift apart in the direction that fails
    /// silently — the gate waits forever on a check nothing posts.
    #[test]
    fn this_repository_defines_the_required_check_job() {
        let workflow = include_str!("../../../.github/workflows/ci.yml");
        let names = workflow_job_check_names(workflow).unwrap();
        assert!(
            names.iter().any(|name| name == REQUIRED_CHECK),
            "ci.yml must define a job whose check run is named {REQUIRED_CHECK:?}; found {names:?}"
        );
    }

    /// A job reports under its `name:` when it sets one, and under its key
    /// otherwise — the same rule GitHub applies when it names the check run.
    #[test]
    fn workflow_job_names_prefer_the_explicit_name() {
        let names = workflow_job_check_names(
            "jobs:\n  rust:\n    name: cargo test (workspace)\n  ci:\n    runs-on: ubuntu-latest\n",
        )
        .unwrap();
        assert_eq!(names, vec!["cargo test (workspace)", "ci"]);
    }

    #[test]
    fn workflow_without_the_required_job_is_rejected() {
        let names = workflow_job_check_names(
            "jobs:\n  lint:\n    name: lint\n  verify:\n    name: verify\n",
        )
        .unwrap();
        assert!(!names.iter().any(|name| name == REQUIRED_CHECK));
    }

    /// GitHub returns rules in stored order, which differs between a ruleset
    /// this command created and one created by hand through REST. A permuted
    /// order is the same policy and must not read as drift, or reconciliation
    /// never converges.
    #[test]
    fn rule_order_is_not_drift() {
        let desired = desired_branch_ruleset(TEST_ACTIONS_APP_ID);
        let mut permuted = desired.clone();
        permuted.rules.reverse();
        assert!(ruleset_matches(&desired, &permuted));
        assert!(plan(
            COMMON_POLICY,
            TEST_ACTIONS_APP_ID,
            true,
            &[Some(permuted)],
            &[]
        )
        .is_empty());
    }

    /// Reordering is forgiven; a changed rule is not.
    #[test]
    fn a_changed_rule_is_still_drift() {
        let desired = desired_branch_ruleset(TEST_ACTIONS_APP_ID);
        let mut weakened = desired.clone();
        weakened
            .rules
            .retain(|rule| rule.kind != "required_signatures");
        assert!(!ruleset_matches(&desired, &weakened));
    }

    #[test]
    fn planner_is_empty_for_identical_state() {
        let labels = DEVX_LABELS
            .iter()
            .map(|label| Label {
                name: label.name.to_string(),
                description: Some(label.description.to_string()),
            })
            .collect::<Vec<_>>();
        let live = desired_rulesets(NAVIGATOR_POLICY, TEST_ACTIONS_APP_ID)
            .into_iter()
            .map(Some)
            .collect::<Vec<_>>();
        assert!(plan(NAVIGATOR_POLICY, TEST_ACTIONS_APP_ID, true, &live, &labels).is_empty());
    }

    #[test]
    fn planner_reconciles_merge_settings_before_other_drift() {
        let live = desired_rulesets(COMMON_POLICY, TEST_ACTIONS_APP_ID)
            .into_iter()
            .map(Some)
            .collect::<Vec<_>>();
        assert_eq!(
            plan(COMMON_POLICY, TEST_ACTIONS_APP_ID, false, &live, &[]),
            vec![Action::UpdateMergeSettings]
        );
    }

    /// A repository that has never had the tag gate reconciled onto it plans a
    /// create rather than silently skipping the ruleset it could not find.
    #[test]
    fn planner_creates_a_missing_ruleset() {
        let live = vec![
            Some(desired_branch_ruleset(TEST_ACTIONS_APP_ID)),
            None,
            None,
        ];
        assert_eq!(
            plan(NAVIGATOR_POLICY, TEST_ACTIONS_APP_ID, true, &live, &[]),
            vec![
                Action::CreateRuleset {
                    name: "release-tags".to_string()
                },
                Action::CreateRuleset {
                    name: "production-review".to_string()
                },
                Action::CreateLabel {
                    name: "triage".to_string()
                },
                Action::CreateLabel {
                    name: "triaged".to_string()
                },
                Action::CreateLabel {
                    name: "devx:paused".to_string()
                },
                Action::CreateLabel {
                    name: "devx:failed".to_string()
                },
            ]
        );
    }

    #[test]
    fn planner_limits_drift_to_ruleset_and_label() {
        let labels = vec![Label {
            name: "triaged".to_string(),
            description: Some("old description".to_string()),
        }];
        let live = vec![
            Some(live_ruleset()),
            Some(desired_tag_ruleset()),
            Some(desired_review_ruleset()),
        ];
        assert_eq!(
            plan(NAVIGATOR_POLICY, TEST_ACTIONS_APP_ID, true, &live, &labels),
            vec![
                Action::UpdateRuleset {
                    name: "production".to_string()
                },
                Action::CreateLabel {
                    name: "triage".to_string()
                },
                Action::UpdateLabel {
                    name: "triaged".to_string()
                },
                Action::CreateLabel {
                    name: "devx:paused".to_string()
                },
                Action::CreateLabel {
                    name: "devx:failed".to_string()
                },
            ]
        );
    }

    /// Navigator keeps its own policy; every other repository in the public
    /// organization resolves to the common one rather than being refused.
    #[test]
    fn policy_is_navigator_only_for_navigator() {
        assert_eq!(
            policy_for("neon-law-source-code/navigator"),
            NAVIGATOR_POLICY
        );
        assert_eq!(
            policy_for("NEON-LAW-SOURCE-CODE/Navigator"),
            NAVIGATOR_POLICY
        );
        for slug in ["neon-law-source-code/other", "NEON-LAW-SOURCE-CODE/ux"] {
            assert_eq!(policy_for(slug), COMMON_POLICY, "{slug}");
        }
    }

    /// The two organizations get different policies, and the client
    /// organization's default is private.
    ///
    /// This is the fork the move to a public forge made necessary. The same host
    /// now holds the repository whose whole point is to be readable by anyone
    /// and the repository whose whole point is that it is not, so the
    /// organization has to be the question that decides publication — and the
    /// default has to be the safe direction, because the unsafe one publishes a
    /// client matter.
    #[test]
    fn the_two_organizations_get_different_policies() {
        let public = policy_for("neon-law-source-code/some-repository");
        let client = policy_for("a-deployment-org/cruller-v-prine");

        assert_ne!(public, client);
        assert_eq!(public.default_visibility, Visibility::Public);
        assert_eq!(client.default_visibility, Visibility::Private);
        assert!(public.open_source_governance);
        assert!(!client.open_source_governance);

        // The gate itself does not vary with the organization: a client
        // matter's source is a repository the Firm develops in, held to the
        // same integrity rules and the same code-owner review.
        assert_eq!(client.branch_protections, public.branch_protections);
        assert_eq!(client.review_gate, public.review_gate);
        assert_eq!(client.assert_codeowners, public.assert_codeowners);
        assert_eq!(client.release_tags, public.release_tags);
        assert!(client.labels.is_empty());
        assert!(!client.assert_devx_app);
    }

    /// Every repository in the public organization defaults to public, and
    /// nothing outside it does.
    ///
    /// The case-insensitive halves matter: GitHub slugs are compared without
    /// case, and a lookalike owner that merely *starts with* the public
    /// organization is a different organization.
    #[test]
    fn only_the_public_organization_defaults_to_public() {
        for slug in [NAVIGATOR_SLUG, TAP_SLUG, "NEON-LAW-SOURCE-CODE/anything"] {
            assert_eq!(
                policy_for(slug).default_visibility,
                Visibility::Public,
                "{slug}"
            );
        }
        for slug in [
            "ux/core",
            "a-deployment-org/cruller-v-prine",
            "neon-law-source-code-evil/navigator",
        ] {
            assert_eq!(
                policy_for(slug).default_visibility,
                Visibility::Private,
                "{slug}"
            );
        }
    }

    /// No planned action writes visibility.
    ///
    /// The field is a creation-time default this command carries, not one it
    /// converges: `ops github setup` is idempotent over repositories that
    /// already exist, and a run that flipped visibility would either publish a
    /// client matter or unpublish the product. Both are one-way doors, and
    /// neither is a policy difference to write.
    #[test]
    fn no_planned_action_writes_visibility() {
        // The planner's whole vocabulary, matched exhaustively: none of it is a
        // visibility write. Adding a variant that was one would stop compiling
        // here rather than shipping a one-way door.
        let writes_visibility = |action: &Action| match action {
            Action::UpdateMergeSettings
            | Action::CreateRuleset { .. }
            | Action::UpdateRuleset { .. }
            | Action::CreateLabel { .. }
            | Action::UpdateLabel { .. } => false,
        };
        for action in [
            Action::UpdateMergeSettings,
            Action::CreateRuleset { name: "x".into() },
            Action::UpdateRuleset { name: "x".into() },
            Action::CreateLabel { name: "x".into() },
            Action::UpdateLabel { name: "x".into() },
        ] {
            assert!(!writes_visibility(&action), "{action:?}");
        }

        // And the two organizations' policies differ in nothing the planner
        // reads, so a reconcile writes the same thing in either one.
        assert_eq!(
            plan(COMMON_POLICY, TEST_ACTIONS_APP_ID, false, &[], &[]),
            plan(CLIENT_POLICY, TEST_ACTIONS_APP_ID, false, &[], &[]),
        );
    }

    /// The governed host every test here supplies.
    ///
    /// Synthetic: the host is configuration, so this file spells no real forge
    /// host. What the tests prove is that the *configured* host is the boundary,
    /// which is a stronger claim than pinning one spelling of it.
    const A_GOVERNED_HOST: &str = "forge.example";

    /// A deployment's own organization. Synthetic for the same reason.
    const A_DEPLOYMENT_ORGANIZATION: &str = "a-deployment-org";

    /// The pair a configured deployment governs: the public organization from
    /// source, plus its own from configuration.
    fn a_governed_forge() -> GovernedForge {
        GovernedForge {
            host: A_GOVERNED_HOST.to_string(),
            organizations: vec![
                public_organization().to_string(),
                A_DEPLOYMENT_ORGANIZATION.to_string(),
            ],
        }
    }

    #[test]
    fn origin_remote_reduces_to_an_owner_name_slug() {
        for remote in [
            "https://forge.example/ux/core",
            "https://forge.example/ux/core.git",
            "https://nick@forge.example/ux/core.git",
            "git@forge.example:ux/core.git",
            "ssh://git@forge.example/ux/core",
        ] {
            assert_eq!(
                slug_from_remote(remote, A_GOVERNED_HOST).unwrap(),
                "ux/core",
                "{remote}"
            );
        }
    }

    /// The host is half the authorization boundary, so a remote off the governed
    /// host is refused before a token is read — including a lookalike that
    /// merely *contains* it.
    #[test]
    fn origin_remote_off_the_governed_host_is_refused() {
        for remote in [
            "https://elsewhere.example/neon-law-source-code/navigator.git",
            "git@elsewhere.example:neon-law-source-code/navigator.git",
            "https://forge.example.evil.test/neon-law-source-code/navigator.git",
        ] {
            let error = slug_from_remote(remote, A_GOVERNED_HOST)
                .unwrap_err()
                .to_string();
            assert!(error.contains(A_GOVERNED_HOST), "{remote}: {error}");
        }
    }

    /// A remote on *another* configured host is refused by the same rule.
    ///
    /// This is what makes the boundary configuration rather than a literal: the
    /// same remote is in scope under one configured host and refused under
    /// another, with no name written down in this file.
    #[test]
    fn the_boundary_follows_the_configured_host() {
        let remote = "https://one.example/ux/core.git";
        assert_eq!(slug_from_remote(remote, "one.example").unwrap(), "ux/core");
        assert!(slug_from_remote(remote, "another.example").is_err());
    }

    /// The organization is the other half, and it is the half a public forge
    /// made necessary.
    ///
    /// A host check alone admitted every repository on GitHub whose checkout
    /// happened to be the working directory. Both admissible organizations are
    /// accepted; a third is refused, and the refusal names the host and the
    /// organizations so an operator can see which half they are outside.
    #[test]
    fn a_repository_outside_both_organizations_is_refused() {
        let forge = a_governed_forge();

        for slug in [
            NAVIGATOR_SLUG,
            TAP_SLUG,
            "NEON-LAW-SOURCE-CODE/Navigator",
            "a-deployment-org/cruller-v-prine",
            "A-DEPLOYMENT-ORG/cruller-v-prine",
        ] {
            assert!(forge.admits(slug), "{slug} must be admitted");
        }

        for slug in [
            "ux/core",
            "some-other-org/navigator",
            "neon-law-source-code-evil/navigator",
        ] {
            assert!(!forge.admits(slug), "{slug} must be refused");
            let error = forge.refuse("the repository named", slug).to_string();
            assert!(error.contains(slug), "{slug}: {error}");
            assert!(error.contains(A_GOVERNED_HOST), "{slug}: {error}");
            assert!(error.contains(A_DEPLOYMENT_ORGANIZATION), "{slug}: {error}");
        }
    }

    /// A checkout operating no deployment governs the public organization
    /// alone, on the default host.
    ///
    /// This is the ordinary case for this command — a fresh clone, a laptop, a
    /// CI job reconciling the public repositories — and it is the case the old
    /// host-only boundary could not run at all, because it had no default and
    /// nothing had set the key.
    #[test]
    fn with_no_deployment_the_public_organization_is_governed_on_the_default_host() {
        let forge = GovernedForge {
            host: cloud::workspace::DEFAULT_GIT_HOST.to_string(),
            organizations: vec![public_organization().to_string()],
        };
        assert!(forge.admits(NAVIGATOR_SLUG));
        assert!(forge.admits(TAP_SLUG));
        assert!(!forge.admits("a-deployment-org/cruller-v-prine"));
        // Composed from the constant rather than spelled: `forge_coordinate_retired`
        // admits exactly one spelling of a forge host in this tree, and it is
        // that constant's own declaration.
        assert_eq!(
            forge.api_base(),
            format!("https://api.{}", cloud::workspace::DEFAULT_GIT_HOST)
        );
    }

    /// `ops github setup` runs in a fresh clone that configures nothing, and
    /// targets the public forge.
    ///
    /// This is the case the command could not run at all before: `NAVIGATOR_GIT_HOST`
    /// had no default, so a laptop or a CI job that had not sourced a deployment
    /// config was refused by its own authorization boundary — while the one
    /// repository the boundary existed to protect was a repository on that same
    /// public forge, named by a constant in this file.
    #[test]
    fn a_fresh_clone_configures_nothing_and_targets_the_public_forge() {
        let forge = GovernedForge::from_lookup(|_| None)
            .expect("a checkout that configures nothing must resolve");
        assert_eq!(forge.host, cloud::workspace::DEFAULT_GIT_HOST);
        assert_eq!(forge.organizations, vec![public_organization().to_string()]);

        let target =
            RepositoryTarget::resolve_within(Some(NAVIGATOR_SLUG.to_string()), &forge, None)
                .expect("Navigator's own repository is governed from a fresh clone");
        assert_eq!(target.slug, NAVIGATOR_SLUG);
        assert_eq!(target.policy, NAVIGATOR_POLICY);
        assert_eq!(
            target.api_base,
            format!("https://api.{}", cloud::workspace::DEFAULT_GIT_HOST)
        );

        // And the same fresh clone refuses a repository outside the one
        // organization it governs, before any token is read.
        let error =
            RepositoryTarget::resolve_within(Some("some-other-org/navigator".into()), &forge, None)
                .expect_err("a foreign organization is refused");
        assert!(error.to_string().contains("some-other-org/navigator"));
    }

    /// A misconfigured deployment does not quietly become a fresh clone.
    ///
    /// Falling back to the public organization here would look like success
    /// while governing less than asked, so the error names the key instead.
    #[test]
    fn a_misconfigured_deployment_fails_closed_naming_the_key() {
        let error = GovernedForge::from_lookup(|key| {
            (key == cloud::workspace::NAVIGATOR_GCP_PROJECT_ID).then(|| "neon-law".to_string())
        })
        .expect_err("a named deployment with no organization must not resolve")
        .to_string();
        assert!(
            error.contains(cloud::workspace::NAVIGATOR_GITHUB_ORG),
            "{error}"
        );
    }

    /// The public organization is the owner of [`NAVIGATOR_SLUG`], never a
    /// second constant that could drift from it.
    #[test]
    fn the_public_organization_is_derived_from_navigators_own_slug() {
        assert_eq!(
            NAVIGATOR_SLUG,
            format!("{}/navigator", public_organization())
        );
        assert!(TAP_SLUG.starts_with(public_organization()));
    }

    /// The API base follows the governed host, so a run can only ever talk to
    /// the host it is reconciling on.
    #[test]
    fn the_api_base_is_composed_from_the_governed_host() {
        assert_eq!(a_governed_forge().api_base(), "https://api.forge.example");
    }

    /// A URL passed where a slug belongs is an error, not the first two path
    /// segments of somebody else's repository.
    #[test]
    fn slug_argument_rejects_anything_but_owner_name() {
        assert_eq!(validate_slug("ux/core").unwrap(), "ux/core");
        assert_eq!(validate_slug("/ux/core/").unwrap(), "ux/core");
        for value in [
            "https://forge.example/neon-law-source-code/navigator",
            "neon-law",
            "a/b/c",
            "",
        ] {
            assert!(validate_slug(value).is_err(), "{value}");
        }
    }

    /// The common policy is the full gate minus only the release automation.
    #[test]
    fn the_common_policy_is_the_full_gate_without_release_automation() {
        let rulesets = desired_rulesets(COMMON_POLICY, TEST_ACTIONS_APP_ID);
        assert_eq!(rulesets.len(), 2);
        assert!(rulesets[0].bypass_actors.is_empty());
        let checks = serde_json::to_value(&rulesets[0]).unwrap()["rules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|rule| rule["type"] == "required_status_checks")
            .unwrap()["parameters"]["required_status_checks"]
            .clone();
        // The same `ci` context as every other administered repository. What
        // that job runs there is that repository's business; what it is called
        // is the Firm's contract, so the gate never has to be re-pointed.
        assert_eq!(
            checks,
            serde_json::json!([{"context": REQUIRED_CHECK, "integration_id": TEST_ACTIONS_APP_ID}])
        );
        assert!(COMMON_POLICY.labels.is_empty());
        const { assert!(!COMMON_POLICY.release_tags) };
        const { assert!(COMMON_POLICY.review_gate) };
    }

    #[tokio::test]
    async fn reconcile_writes_only_drifted_ruleset_and_label() {
        let server = MockServer::start().await;
        let client = test_client(&server);
        mount_reads(
            &server,
            &live_ruleset(),
            vec![Label {
                name: "triaged".to_string(),
                description: Some("old description".to_string()),
            }],
        )
        .await;
        Mock::given(method("PUT"))
            .and(path("/repos/acme/navigator/rulesets/7"))
            .and(header("authorization", "Bearer token"))
            // The id the mounted `/apps` read answers with, which is what
            // `reconcile` will have carried into the rule. Asserting the written
            // body against it is what proves the read reaches the write.
            .and(body_json(
                serde_json::to_value(desired_branch_ruleset(TEST_ACTIONS_APP_ID)).unwrap(),
            ))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/repos/acme/navigator/labels/triaged"))
            .and(body_json(
                serde_json::json!({"new_name":"triaged","description":DEVX_LABELS[1].description}),
            ))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        for label in [&DEVX_LABELS[0], &DEVX_LABELS[2], &DEVX_LABELS[3]] {
            Mock::given(method("POST"))
                .and(path("/repos/acme/navigator/labels"))
                .and(body_json(serde_json::json!({"name":label.name,"description":label.description,"color":LABEL_COLOR})))
                .respond_with(ResponseTemplate::new(201))
                .expect(1)
                .mount(&server)
                .await;
        }
        reconcile(NAVIGATOR_POLICY, &client, false).await.unwrap();
    }

    #[tokio::test]
    async fn dry_run_never_writes() {
        let server = MockServer::start().await;
        let client = test_client(&server);
        mount_reads(&server, &live_ruleset(), Vec::new()).await;
        reconcile(NAVIGATOR_POLICY, &client, true).await.unwrap();
        let requests = server.received_requests().await.unwrap();
        assert!(requests
            .iter()
            .all(|request| request.method == wiremock::http::Method::GET));
    }

    /// A reconcile aimed at the tap must succeed, write nothing, and never even
    /// ask for what a tap does not have.
    ///
    /// This is the behaviour the carve-out exists for. Only three reads are
    /// mounted — the Actions App, the repository, and its ruleset list — so a
    /// request for CODEOWNERS or for `ci.yml` would 404 against this server and
    /// fail the reconcile. Passing therefore proves those assertions are skipped
    /// rather than merely satisfied.
    ///
    /// The live `production` ruleset in the list is the other half: the command
    /// emits no `DeleteRuleset`, so a gate a human deliberately adds here is
    /// left alone. Pointing this command at the tap is a no-op, not a fight.
    #[tokio::test]
    async fn a_reconcile_aimed_at_the_tap_writes_nothing_and_asserts_nothing() {
        let server = MockServer::start().await;
        let client = test_client(&server);
        Mock::given(method("GET"))
            .and(path(format!("/apps/{ACTIONS_APP_SLUG}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"id": TEST_ACTIONS_APP_ID})),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/navigator"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "allow_squash_merge": true,
                "allow_merge_commit": false,
                "allow_rebase_merge": false,
                "allow_auto_merge": true,
                "delete_branch_on_merge": true,
                "squash_merge_commit_title": "PR_TITLE",
                "squash_merge_commit_message": "PR_BODY",
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/navigator/rulesets"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"id":7,"name":"production"},
            ])))
            .mount(&server)
            .await;

        reconcile(TAP_POLICY, &client, false).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        assert!(
            requests
                .iter()
                .all(|request| request.method == wiremock::http::Method::GET),
            "a tap reconcile must write nothing"
        );
        assert!(
            !requests
                .iter()
                .any(|request| request.url.path().contains("CODEOWNERS")
                    || request.url.path().contains("workflows/")),
            "a tap reconcile must not read a CODEOWNERS or a CI workflow it has no reason to have"
        );
    }

    fn test_client(server: &MockServer) -> GitHubClient {
        let http = reqwest::Client::builder()
            .default_headers({
                let mut headers = header::HeaderMap::new();
                headers.insert(
                    header::AUTHORIZATION,
                    header::HeaderValue::from_static("Bearer token"),
                );
                headers
            })
            .build()
            .unwrap();
        GitHubClient {
            http,
            api_base: server.uri(),
            repository: "acme/navigator".to_string(),
        }
    }

    async fn mount_reads(server: &MockServer, ruleset: &RulesetPayload, labels: Vec<Label>) {
        // The host names its own Actions App, and the required-check rule is
        // built from that id.
        Mock::given(method("GET"))
            .and(path(format!("/apps/{ACTIONS_APP_SLUG}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"id": TEST_ACTIONS_APP_ID})),
            )
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/navigator"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "allow_squash_merge": true,
                "allow_merge_commit": false,
                "allow_rebase_merge": false,
                "allow_auto_merge": true,
                "delete_branch_on_merge": true,
                "squash_merge_commit_title": "PR_TITLE",
                "squash_merge_commit_message": "PR_BODY",
            })))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/navigator/contents/.github/CODEOWNERS"))
            .respond_with(ResponseTemplate::new(200).set_body_string("* @owner\n"))
            .mount(server)
            .await;
        // The owner named above must resolve, or the review gate would be
        // written onto a repository where every path is in fact unowned.
        Mock::given(method("GET"))
            .and(path("/users/owner"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(server)
            .await;
        // The workflow must actually define the `ci` job the gate requires.
        Mock::given(method("GET"))
            .and(path(
                "/repos/acme/navigator/contents/.github/workflows/ci.yml",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                "jobs:\n  rust:\n    name: cargo test (workspace)\n  ci:\n    name: ci\n",
            ))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/navigator/rulesets"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"id":7,"name":"production"},
                {"id":8,"name":"release-tags"},
                {"id":9,"name":"production-review"},
            ])))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/navigator/rulesets/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ruleset))
            .mount(server)
            .await;
        // Already reconciled, so only the branch gate above should ever be
        // written: this is what makes the test's name load-bearing.
        Mock::given(method("GET"))
            .and(path("/repos/acme/navigator/rulesets/8"))
            .respond_with(ResponseTemplate::new(200).set_body_json(desired_tag_ruleset()))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/navigator/rulesets/9"))
            .respond_with(ResponseTemplate::new(200).set_body_json(desired_review_ruleset()))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/navigator/labels"))
            .respond_with(ResponseTemplate::new(200).set_body_json(labels))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn required_check_job_accepts_the_scaffolded_gate_workflow() {
        // A Project repository written by `navigator projects repository
        // scaffold` has no ci.yml at all. Before both spellings were accepted
        // this read as "the repository has no CI" and refused to govern it.
        let server = MockServer::start().await;
        let client = test_client(&server);
        Mock::given(method("GET"))
            .and(path(
                "/repos/acme/navigator/contents/.github/workflows/ci.yml",
            ))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(
                "/repos/acme/navigator/contents/.github/workflows/gate.yml",
            ))
            .respond_with(
                ResponseTemplate::new(200).set_body_string("jobs:\n  ci:\n    name: ci\n"),
            )
            .mount(&server)
            .await;
        assert_required_check_job(&client).await.unwrap();
    }

    #[tokio::test]
    async fn required_check_job_names_both_spellings_when_neither_exists() {
        let server = MockServer::start().await;
        let client = test_client(&server);
        for workflow in ["ci.yml", "gate.yml"] {
            Mock::given(method("GET"))
                .and(path(format!(
                    "/repos/acme/navigator/contents/.github/workflows/{workflow}"
                )))
                .respond_with(ResponseTemplate::new(404))
                .mount(&server)
                .await;
        }
        let error = assert_required_check_job(&client)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("ci.yml"), "{error}");
        assert!(error.contains("gate.yml"), "{error}");
    }

    #[tokio::test]
    async fn required_check_job_refuses_a_workflow_without_the_aggregating_job() {
        let server = MockServer::start().await;
        let client = test_client(&server);
        Mock::given(method("GET"))
            .and(path(
                "/repos/acme/navigator/contents/.github/workflows/ci.yml",
            ))
            .respond_with(
                ResponseTemplate::new(200).set_body_string("jobs:\n  build:\n    name: build\n"),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(
                "/repos/acme/navigator/contents/.github/workflows/gate.yml",
            ))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let error = assert_required_check_job(&client)
            .await
            .unwrap_err()
            .to_string();
        // The job it did find is reported, because that is the fix: rename it or
        // add an aggregator, not "write some CI".
        assert!(error.contains("build"), "{error}");
    }

    #[tokio::test]
    async fn required_check_job_does_not_read_a_server_error_as_a_missing_workflow() {
        // A 500 or a revoked token must not look like "no workflow here", or the
        // gate would be bound on a repository whose workflows were never read.
        let server = MockServer::start().await;
        let client = test_client(&server);
        Mock::given(method("GET"))
            .and(path(
                "/repos/acme/navigator/contents/.github/workflows/ci.yml",
            ))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        // `{:#}` walks the context chain; the outermost layer is only
        // "read <path> from <repo>", so the status code lives one level down.
        let error = format!(
            "{:#}",
            assert_required_check_job(&client).await.unwrap_err()
        );
        assert!(error.contains("500"), "{error}");
        assert!(
            !error.contains("none of"),
            "a 500 must not read as an absent workflow: {error}"
        );
    }

    #[tokio::test]
    async fn get_all_labels_follows_pagination_past_the_first_page() {
        let server = MockServer::start().await;
        let client = test_client(&server);
        let page_one = (0..100)
            .map(|index| Label {
                name: format!("filler-{index}"),
                description: None,
            })
            .collect::<Vec<_>>();
        Mock::given(method("GET"))
            .and(path("/repos/acme/navigator/labels"))
            .and(query_param("page", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&page_one))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/navigator/labels"))
            .and(query_param("page", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"name": "triaged", "description": DEVX_LABELS[1].description}
            ])))
            .mount(&server)
            .await;
        let labels = client.get_all_labels().await.unwrap();
        assert_eq!(labels.len(), 101);
        assert!(labels.iter().any(|label| label.name == "triaged"));
    }

    #[test]
    fn merge_settings_detects_drift() {
        let mut repository = matching_repository();
        repository.allow_rebase_merge = true;
        assert!(!merge_settings_match(&repository));
    }
}
