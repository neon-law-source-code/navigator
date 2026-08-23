//! Guard the GHCR retention sweep — the one workflow in this repository that
//! DELETES published artifacts.
//!
//! Every other workflow adds: images, archives, check runs. This one removes,
//! unattended, on a clock, and what it removes cannot be recovered — a deleted
//! container version is gone, and `ops ship` refuses a tag the registry no
//! longer holds. So the properties that keep it from deleting something
//! load-bearing are asserted here rather than left to review, because a sweep
//! that deleted too much would report success doing it.
//!
//! Three of those properties are the safety floor, and they are independent:
//!
//!   1. an AGE bound, so a version has to be genuinely old to qualify;
//!   2. a COUNT floor, so the newest versions survive however old they are —
//!      this is what stops a quiet month from deleting the version production is
//!      running;
//!   3. a `latest` exemption, so the mutable pointer every published image
//!      carries is never orphaned.
//!
//! Retention was count-only before this workflow existed, enforced by Artifact
//! Registry `cleanupPolicies` that GHCR never reads (`docs/gitops.md` → "Image
//! retention"). The count floor is carried forward deliberately: it is the half
//! that cannot expire.

use std::fs;
use std::path::PathBuf;

use serde_yaml::Value;

/// Delete nothing newer than this. Mirrors `CUTOFF_DAYS` in the workflow.
const CUTOFF_DAYS: u32 = 30;
/// Keep at least this many of every image's newest versions, whatever their age.
const RETAINED_VERSIONS: u32 = 10;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root is cli/'s parent")
        .to_path_buf()
}

fn source() -> String {
    let path = repo_root().join(".github/workflows/ghcr-retention.yml");
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn workflow() -> Value {
    serde_yaml::from_str(&source()).expect("ghcr-retention.yml parses as YAML")
}

/// `on` is the YAML 1.1 boolean `true`, so `serde_yaml` keys it as a bool.
/// Reading it by name silently finds nothing and every assertion passes
/// vacuously.
fn triggers() -> serde_yaml::Mapping {
    let workflow = workflow();
    workflow
        .get(Value::Bool(true))
        .or_else(|| workflow.get("on"))
        .expect("ghcr-retention.yml must declare a trigger block")
        .as_mapping()
        .expect("the trigger block must be a mapping")
        .clone()
}

fn sweep_script() -> String {
    let workflow = workflow();
    workflow["jobs"]["sweep"]["steps"]
        .as_sequence()
        .expect("the sweep job must declare steps")
        .iter()
        .filter_map(|step| step["run"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 01:11 UTC, and 1:11 rather than 1:00 on purpose: GitHub delays scheduled runs
/// when the hosted-runner queue is deep, and the top of the hour is when it is
/// deepest. The old nightly release held this slot; the sweep inherits it now
/// that publishing runs from a tag.
#[test]
fn the_sweep_runs_nightly_at_0111_utc() {
    let crons: Vec<String> = triggers()
        .get(Value::String("schedule".into()))
        .expect("the sweep must run on a clock — retention nobody triggers is retention nobody has")
        .as_sequence()
        .expect("the schedule trigger must be a sequence")
        .iter()
        .filter_map(|entry| entry["cron"].as_str().map(str::to_string))
        .collect();

    assert!(
        crons.iter().any(|cron| cron == "11 1 * * *"),
        "the GHCR sweep must run at 01:11 UTC. Got: {crons:?}"
    );
}

/// A destructive unattended job must be rehearsable. The dispatch exists so the
/// sweep can be watched reporting what it WOULD delete before a night deletes
/// it — the only way to prove a change to this workflow without waiting for the
/// clock and finding out from the registry.
#[test]
fn the_sweep_can_be_rehearsed_without_deleting() {
    assert!(
        triggers().contains_key(Value::String("workflow_dispatch".into())),
        "the sweep must keep `workflow_dispatch`: a delete job you cannot rehearse is one whose \
         first proof is a registry you cannot restore"
    );

    let script = sweep_script();
    assert!(
        script.contains("DRY_RUN"),
        "the sweep must honour a dry-run mode, and the dispatch must be able to set it"
    );
}

/// THE COUNT FLOOR. The property an age-only rule cannot provide.
///
/// Publishing runs from a tag, so nothing guarantees a release this month. An
/// age-only sweep would then delete the exact versions production is running:
/// serving pods survive it, because they already pulled, but a restart, a
/// reschedule, or a node replacement cannot pull an image that is gone, and
/// `ops ship --tag <previous>` — the documented rollback — refuses a tag the
/// registry no longer holds. A count cannot expire.
#[test]
fn the_newest_versions_survive_any_age() {
    let script = sweep_script();

    assert!(
        script.contains(&format!("RETAINED_VERSIONS={RETAINED_VERSIONS}")),
        "the sweep must keep the newest {RETAINED_VERSIONS} versions of every image whatever their \
         age — the floor that stops a quiet month deleting what production runs"
    );
    assert!(
        script.contains(&format!("CUTOFF_DAYS={CUTOFF_DAYS}")),
        "the sweep must bound deletion by age as well as count"
    );
}

/// The `latest` pointer is published on every image and must never be orphaned.
/// Deleting the version it points at leaves a tag resolving to nothing, which
/// fails at pull time rather than at sweep time.
#[test]
fn the_latest_pointer_is_never_deleted() {
    assert!(
        sweep_script().contains("latest"),
        "the sweep must exempt the version tagged `latest`: it is a published pointer, and \
         deleting what it points at breaks a pull rather than the sweep"
    );
}

/// THE PACKAGE LIST IS EXACTLY WHAT `deploy.yml` PUBLISHES.
///
/// The sweep names the packages it may touch instead of discovering them, for
/// two reasons that happen to point the same way.
///
/// **It has to.** `GET /orgs/{org}/packages` — the discovery call — is reachable
/// only by a classic PAT holding `read:packages`; `GITHUB_TOKEN` is answered 403
/// however the permissions block is written. The sweep failed every night it
/// ever ran on exactly that call, and no permissions change could have fixed it.
/// The per-package version LISTING is a different lane, and the run's own token
/// reaches it. Deleting needs more — see
/// [`a_sweep_that_deletes_nothing_names_the_missing_admin_grant`].
///
/// **It should.** A GHCR package is owned by the ORG, and the org owns packages
/// other repositories push. A named list cannot widen on its own — no repository
/// link has to be trusted, and a link that changed shape cannot hand this
/// workflow someone else's images to delete on a clock.
///
/// What a named list CAN do is drift, so this test removes that: the list must
/// equal the images `deploy.yml` publishes — every `publish-service` leg's
/// `image` and `alias`, and every `publish-triggers` leg's `image`. A new image
/// therefore joins the sweep in the same commit that starts publishing it, and
/// a retired one cannot linger here aimed at a package this repository no
/// longer owns.
#[test]
fn the_swept_packages_are_exactly_what_deploy_publishes() {
    let script = sweep_script();

    // The literal block, read out of the workflow.
    let listed: Vec<String> = script
        .split("PACKAGES=\"")
        .nth(1)
        .expect("the sweep must declare its PACKAGES list")
        .split('"')
        .next()
        .expect("the PACKAGES list is quote-delimited")
        .split_whitespace()
        .map(str::to_string)
        .collect();

    // What deploy.yml actually pushes.
    let deploy: Value = {
        let path = repo_root().join(".github/workflows/deploy.yml");
        let source =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        serde_yaml::from_str(&source).expect("deploy.yml parses as YAML")
    };

    let mut published: Vec<String> = Vec::new();
    for job in ["publish-service", "publish-triggers"] {
        let legs = deploy["jobs"][job]["strategy"]["matrix"]["include"]
            .as_sequence()
            .unwrap_or_else(|| panic!("deploy.yml's `{job}` declares a matrix"));
        for leg in legs {
            for key in ["image", "alias"] {
                if let Some(name) = leg[key].as_str() {
                    published.push(name.to_string());
                }
            }
        }
    }

    let mut listed_sorted = listed.clone();
    listed_sorted.sort();
    listed_sorted.dedup();
    let mut published_sorted = published.clone();
    published_sorted.sort();
    published_sorted.dedup();

    assert_eq!(
        listed_sorted, published_sorted,
        "the sweep's PACKAGES list must equal the images deploy.yml publishes. An image \
         missing here is never pruned and accumulates forever; an image here that deploy.yml \
         does not publish is a name this workflow would delete versions of without owning it"
    );

    // A list that parsed to nothing would satisfy the equality above only if
    // deploy.yml also published nothing, but an empty sweep must never be the
    // quiet outcome of a formatting change.
    assert!(
        !listed.is_empty(),
        "the sweep must name at least one package; an empty list prunes nothing and says so \
         to no one"
    );
}

/// The unreachable discovery call must not come back.
///
/// It is the specific line that failed every scheduled run, and it fails in a
/// way retrying cannot fix. Naming it here means a future edit that reaches for
/// the "obvious" org listing is refused at the gate rather than at 01:45 UTC.
#[test]
fn the_sweep_does_not_enumerate_the_organizations_packages() {
    let script = sweep_script();

    assert!(
        !script.contains("/orgs/${OWNER}/packages?"),
        "`GET /orgs/{{org}}/packages` needs a classic PAT with `read:packages`; GITHUB_TOKEN is \
         answered 403, so a sweep built on it cannot run at all. Name the packages instead"
    );
    assert!(
        !script.contains("if ! packages="),
        "the sweep must not reintroduce the org-listing call: it is answered 403 for GITHUB_TOKEN \
         and needs a classic PAT, which naming the packages removes the need for"
    );
}

/// The sweep is still bound to container packages and to this repository.
#[test]
fn the_sweep_only_touches_this_repositorys_packages() {
    let script = sweep_script();

    assert!(
        script.contains("${PACKAGES}"),
        "the sweep must iterate the packages it names — the org owns packages this repository \
         did not publish, and only a named list is bounded by construction"
    );
    assert!(
        script.contains("container"),
        "the sweep must scope itself to container packages"
    );
}

/// Deleting a package version is the whole grant, and it is the narrowest one
/// that does it. In particular this workflow must not be able to move a ref: a
/// sweep with `contents: write` could rewrite the repository it is pruning
/// images for.
#[test]
fn the_sweep_holds_only_the_packages_grant() {
    let workflow = workflow();
    let permissions = &workflow["jobs"]["sweep"]["permissions"];

    assert_eq!(
        permissions["packages"].as_str(),
        Some("write"),
        "the sweep needs `packages: write` to delete a version, and GITHUB_TOKEN is the whole \
         credential — no PAT to rotate"
    );
    assert_ne!(
        permissions["contents"].as_str(),
        Some("write"),
        "the sweep must not be able to write repository contents: it prunes a registry, and \
         nothing in this repository's automation may move a ref"
    );
    assert!(
        !source().contains("google-github-actions/auth"),
        "the sweep reaches no cloud provider — GHCR is the only registry, and a surviving \
         credential exchange is reach it does not need"
    );
}

/// TELL THE TWO 404s APART.
///
/// `DELETE .../versions/{id}` answers `404 Package not found` for two unrelated
/// reasons: a version another run already removed — benign, and the reason one
/// failed delete must not abandon the rest of the sweep — and a credential that
/// may not delete anything at all. Publishing an image with `GITHUB_TOKEN`
/// inherits the `write` role on the resulting package, and `write` can upload
/// and download but not delete; only `admin` deletes. That role is granted per
/// package under "Manage Actions access", it has no REST API, and so it is the
/// one precondition of this sweep that lives outside the repository and cannot
/// be asserted here.
///
/// Which is exactly how it went unnoticed: the sweep listed fine, decided fine,
/// then emitted one indistinguishable warning per version and a summary that
/// counted them — reading as the benign 404 the code comments predict. It ran
/// that way for five consecutive nights, deleting nothing.
///
/// So the arithmetic carries the diagnosis. Some deletes failing is ordinary;
/// EVERY delete failing is not a registry that raced, it is a role that was
/// never granted, and the run must say so in the words that name the fix.
#[test]
fn a_sweep_that_deletes_nothing_names_the_missing_admin_grant() {
    let script = sweep_script();

    assert!(
        script.contains(r#""${deleted}" -eq 0"#),
        "the sweep must branch on having deleted NOTHING. `failed > 0` alone cannot tell a \
         stale version from a credential that may not delete, and those need different hands"
    );
    assert!(
        script.contains("Manage Actions access"),
        "a total delete failure must name where the `admin` role is granted. The sweep cannot \
         set it and no test can assert it, so the run that trips over it is the only place \
         the remedy can be written"
    );
    assert!(
        script.contains("cannot delete"),
        "the error must say why `packages: write` was not enough — otherwise the next reader \
         re-derives it from a 404 that also means something harmless"
    );
}

/// An unattended destructive job that says nothing is indistinguishable from one
/// that never ran. A silent nightly failure went unnoticed for four consecutive
/// nights once (`docs/gitops.md` → "What detects a broken pipeline"), and this
/// job's failure mode is quieter still: nothing goes red, images just stop being
/// pruned, or worse, are pruned wrongly.
#[test]
fn the_sweep_reports_to_navigator() {
    let source = source();

    assert!(
        source.contains("SLACK_WEBHOOK_URL"),
        "the sweep must post its result to #navigator through the prod ops webhook"
    );
    assert!(
        source.contains("if: failure()"),
        "the sweep must page #navigator when it fails — on a clock, nobody is reading the run"
    );
}

/// Identifiers and counts, never content. The rule binds this surface as hard as
/// it binds the release reports: a sweep summary names images and totals.
#[test]
fn the_sweep_summary_carries_no_client_bearing_field() {
    let source = source();

    for forbidden in ["persons", "matters", "projects/", "@neonlaw.com"] {
        assert!(
            !source.contains(forbidden),
            "the sweep summary must carry identifiers and counts only; found `{forbidden}`"
        );
    }
}
