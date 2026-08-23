//! Guard the hand-off from a release to the Homebrew tap.
//!
//! `brew install neon-law-source-code/navigator/navigator` is the macOS install
//! path — the released binary is unsigned, and Gatekeeper blocks an unsigned
//! Mach-O downloaded through a browser but not one brew fetched with curl. The
//! formula stays current because `deploy.yml` tells the tap that a release
//! landed.
//!
//! **This is the same invisible-breakage shape the CLI archive tests guard.**
//! `deploy.yml` and the tap repository never reference each other, so if this
//! dispatch stops firing, `brew upgrade` keeps resolving the previous release
//! and nothing in this repository goes red. The contract is only holdable by a
//! test.
//!
//! It has already broken once in exactly that way, which is why
//! `a_hotfix_is_dispatched_to_the_tap` exists: the dispatch fired for ordinary
//! releases only, three ordinary releases failed at the KIND gate in a row, and
//! `brew install` served a 404 for days with every check green.

use std::fs;
use std::path::PathBuf;

/// The job that fires the dispatch.
const JOB: &str = "release-homebrew-tap";

/// The tap the formula lives in. A separate repository because a tap is cloned
/// and re-read on every `brew update`, and its formula changes once per
/// release with no review to add.
const TAP_REPO: &str = "neon-law-source-code/homebrew-navigator";

/// The one file a bump changes, and the one the release reads back to prove the
/// bump landed.
const FORMULA_PATH: &str = "Formula/navigator.rb";

/// How long the release waits for the tap to publish the formula. The bump
/// itself runs in under a minute; the rest of the budget absorbs a queued
/// runner and the tap's `bump-formula` concurrency group.
const POLL_BUDGET_MINUTES: u64 = 15;

fn deploy_workflow() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(".github")
        .join("workflows")
        .join("deploy.yml");
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn deploy_job(name: &str) -> serde_yaml::Value {
    let workflow: serde_yaml::Value =
        serde_yaml::from_str(&deploy_workflow()).expect("deploy.yml parses as YAML");
    workflow
        .get("jobs")
        .and_then(|jobs| jobs.get(name))
        .cloned()
        .unwrap_or_else(|| panic!("deploy.yml must define the `{name}` job"))
}

fn job_needs(name: &str) -> Vec<String> {
    serde_yaml::from_value(deploy_job(name)["needs"].clone())
        .unwrap_or_else(|error| panic!("`{name}` must declare a `needs` list: {error}"))
}

/// The dispatch must land after the archives it tells the tap to digest.
///
/// The bump downloads each attached asset to compute its sha256. Dispatching
/// before `release-windows-cli-publish` finishes would race a Release that
/// exists but carries nothing, and the tap would fail on a 404 for bytes that
/// were seconds away.
#[test]
fn the_tap_is_told_only_after_the_archives_are_attached() {
    let needs = job_needs(JOB);
    for required in ["release-windows-cli-publish", "release-version"] {
        assert!(
            needs.iter().any(|entry| entry == required),
            "`{JOB}` must not dispatch before the Release carries its archives, so it needs \
             `{required}`"
        );
    }
}

/// Only a real release may move the formula.
///
/// A `kind-ci/**` branch iteration publishes nothing and stands behind no tag,
/// so a dispatch from one would point the tap at a Release that does not exist.
#[test]
fn only_a_publishable_run_dispatches_to_the_tap() {
    let gate = deploy_job(JOB)["if"]
        .as_str()
        .expect("`release-homebrew-tap` must declare an `if:` gate")
        .to_string();

    assert!(
        gate.contains("needs.release-version.outputs.publishable == 'true'"),
        "the tap dispatch must stay gated on a publishable run, got: {gate:?}"
    );
}

/// EVERY publishable tag reaches the tap, hotfixes included.
///
/// This is the assertion the 404 was made of. While the gate also excluded
/// prereleases, the tap could only be bumped by an ordinary `YY.M.D` release —
/// and when three of those failed at the KIND gate in a row, the formula sat on
/// its seeded placeholder for days while `brew install` served a 404 for a
/// Release that was never cut. Nothing here went red, because a skipped job is
/// not a failed one.
///
/// The stranding this exclusion was protecting against is real but belongs to
/// the tap: Homebrew's comparator orders `26.8.20-hotfix.4` ABOVE `26.8.20`, so
/// a formula walked from a hotfix to its own base version looks like a
/// downgrade and `brew` refuses to upgrade it. `scripts/bump.sh` in the tap
/// handles that with `version_scheme`, which is the mechanism Homebrew provides
/// for precisely this case. Fixing it there rather than by refusing the bump is
/// what lets `brew` resolve the newest build that exists.
#[test]
fn a_hotfix_is_dispatched_to_the_tap() {
    let gate = deploy_job(JOB)["if"]
        .as_str()
        .expect("`release-homebrew-tap` must declare an `if:` gate")
        .to_string();

    assert!(
        !gate.contains("prerelease"),
        "`{JOB}` must not gate on `prerelease`: excluding hotfixes leaves `brew` resolving \
         whatever the last SUCCESSFUL ordinary release was, and a run of failed ordinary \
         releases silently strands the formula. Got: {gate:?}"
    );
}

/// The payload carries a tag and nothing else.
///
/// Digests belong to whoever downloads the bytes. Shipping them in the payload
/// would let a malformed dispatch pin the formula to bytes nobody verified, and
/// would leave the tap unable to repair a bad bump from a bare tag — which
/// matters because `YY.M.D` admits no second ordinary release the same UTC day,
/// so a bump that went wrong cannot be fixed by re-cutting the release.
///
/// This once forbade the job to name `Formula/` at all, which read as the same
/// invariant while the job only ever spoke to `/dispatches`. It is not the same
/// invariant: the job now reads the formula back to prove the bump landed, and
/// reading a file is how a release learns its own outcome. What must stay true
/// is that the tap remains the only writer — so the ban is on the digesting and
/// the writing, which is what "the tap owns its formula" actually means.
#[test]
fn the_dispatch_carries_the_tag_and_computes_no_digest() {
    let workflow = deploy_workflow();

    for required in [
        &format!("TAP_REPO: {TAP_REPO}"),
        "-f \"event_type=navigator-release\"",
        "-f \"client_payload[tag]=${TAG}\"",
        "TAG: ${{ needs.release-version.outputs.tag }}",
    ] {
        assert!(
            workflow.contains(required),
            "the tap dispatch must retain `{required}`"
        );
    }

    let job = serde_yaml::to_string(&deploy_job(JOB)).expect("the job serializes");
    for forbidden in ["sha256", "shasum"] {
        assert!(
            !job.contains(forbidden),
            "`{JOB}` must not compute or carry `{forbidden}` — the tap digests the published \
             bytes itself"
        );
    }
    for forbidden in ["--method PUT", "--method PATCH", "git commit", "git push"] {
        assert!(
            !job.contains(forbidden),
            "`{JOB}` must not `{forbidden}` — it reads the tap's formula to confirm the bump and \
             never writes it, so the tap stays the only author of its own history"
        );
    }
    assert!(
        !job.contains(&format!("--method POST \"/repos/{TAP_REPO}/contents")),
        "`{JOB}` must not create the formula — its only write to the tap is the dispatch event"
    );
}

/// A missing token must fail the release, not skip the bump.
///
/// A tap that silently stops updating reports a stale version to everyone who
/// installed through it, and nothing anywhere goes red. That is the same
/// failure shape as the Project-CI download 404 the archive jobs exist to
/// prevent, and it is why this job has no `continue-on-error` and no
/// secret-presence `if:`.
#[test]
fn a_tap_that_cannot_be_reached_fails_the_release() {
    let job = deploy_job(JOB);

    assert!(
        job.get("continue-on-error").is_none(),
        "`{JOB}` must not swallow its own failure — a silent tap is a stale tap"
    );

    let workflow = deploy_workflow();
    assert!(
        workflow.contains("HOMEBREW_TAP_TOKEN is unset"),
        "the job must say what broke when the cross-repository token is missing"
    );
    assert!(
        workflow.contains("GH_TOKEN: ${{ secrets.HOMEBREW_TAP_TOKEN }}"),
        "the dispatch must authenticate with the tap-scoped token, not the run's own GITHUB_TOKEN, \
         which cannot reach another repository"
    );
}

/// The cross-repository grant lives in one secret, not in the workflow's
/// permissions.
///
/// `no_job_can_write_repository_contents` already pins this repository's
/// write surface to the Release-attach job. This asserts the other half: the
/// tap job reaches another repository without widening anything here.
#[test]
fn the_tap_job_writes_nothing_in_this_repository() {
    let job = deploy_job(JOB);

    assert_eq!(
        job["permissions"]["contents"].as_str(),
        Some("read"),
        "`{JOB}` writes nothing here — its grant is the tap-scoped token alone"
    );
}

/// A failed hand-off must page, and only the jobs `notify-failure` lists can.
///
/// The list is hand-maintained, and this row is easy to forget for the reason
/// the whole file exists: a green publish reads like a green release right up
/// until someone runs `brew upgrade` and gets last week's binary.
#[test]
fn a_failed_tap_dispatch_pages_engineering() {
    let needs = job_needs("notify-failure");
    assert!(
        needs.iter().any(|entry| entry == JOB),
        "notify-failure cannot report a failure in `{JOB}` unless it needs it"
    );
}

/// #navigator's install message offers the brew path it now maintains.
///
/// The three download instructions stay — Windows has no tap, and a reader
/// without Homebrew needs the archive. What this adds is the one line that
/// works on a Mac without a Gatekeeper fight.
#[test]
fn the_slack_message_offers_the_homebrew_install() {
    let workflow = deploy_workflow();

    assert!(
        workflow.contains("brew install neon-law-source-code/navigator/navigator"),
        "the #navigator install message must name the tap install command"
    );
    assert!(
        workflow.contains("Gatekeeper"),
        "the message must say why brew is the recommended path on a Mac — an unsigned binary \
         downloaded through a browser is blocked, and a reader who does not know that concludes \
         the release is broken"
    );
}

/// An accepted dispatch is not a completed bump.
///
/// `POST /dispatches` answers 204 the moment GitHub queues the event, and it
/// carries no run id. So the dispatch step alone reports success for a tap that
/// then fails to bump — which is what happened: three consecutive releases
/// (`26.8.21-hotfix.10`, `.11`, and `.12`) were dispatched, all three bumps died
/// at their final `git push`, and every one of those releases stayed green while
/// `brew` served `26.8.20-hotfix.4`. That is the exact silence this file's
/// header calls "only holdable by a test", one repository downstream of the
/// break it already guards.
///
/// The job therefore reads the formula back and fails the release unless it
/// reports this tag. Verifying the OUTCOME rather than the tap's run conclusion
/// is deliberate on three counts: the fine-grained `HOMEBREW_TAP_TOKEN` needs no
/// `actions: read` widening to read a file it can already write; there is no run
/// to discover and therefore no race with a concurrent manual re-run; and a
/// green bump that committed nothing is still caught.
#[test]
fn an_accepted_dispatch_is_not_proof_the_formula_moved() {
    let workflow = deploy_workflow();

    assert!(
        workflow.contains(&format!("/repos/${{TAP_REPO}}/contents/{FORMULA_PATH}")),
        "`{JOB}` must read the tap's formula back — a 204 from `POST /dispatches` only says \
         GitHub queued the event, not that the bump landed"
    );

    let steps: Vec<String> = deploy_job(JOB)["steps"]
        .as_sequence()
        .expect("the job declares steps")
        .iter()
        .filter_map(|step| step.get("name")?.as_str().map(str::to_owned))
        .collect();
    let dispatch = steps
        .iter()
        .position(|name| name.contains("dispatch"))
        .expect("the job dispatches the bump");
    let confirm = steps
        .iter()
        .position(|name| name.contains("confirm"))
        .unwrap_or_else(|| {
            panic!("`{JOB}` must confirm the formula moved; its steps are {steps:?}")
        });
    assert!(
        confirm > dispatch,
        "the confirmation must follow the dispatch, not precede it: {steps:?}"
    );
}

/// The confirmation must be able to fail, and must say what it saw.
///
/// A poll that gives up quietly, or one whose timeout is generous enough to
/// outlive the job, restores the silence it exists to remove. The error names
/// the version the formula actually reports so the reader does not have to open
/// the tap to learn whether the bump ran late or died.
#[test]
fn a_tap_that_never_bumps_fails_the_release() {
    let workflow = deploy_workflow();

    assert!(
        workflow.contains("the tap never published"),
        "the timeout must state that the formula never reached this tag"
    );

    let job = deploy_job(JOB);
    let timeout = job["timeout-minutes"]
        .as_u64()
        .unwrap_or_else(|| panic!("`{JOB}` must declare `timeout-minutes`"));
    assert!(
        timeout > POLL_BUDGET_MINUTES,
        "`{JOB}`'s {timeout}-minute timeout must outlast its {POLL_BUDGET_MINUTES}-minute poll, \
         or the job dies on the clock instead of reporting the stale formula"
    );
}
