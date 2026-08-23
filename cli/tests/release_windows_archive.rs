//! Guard the three CLI release artifacts — the Linux archive CI installs, and
//! the Windows and macOS archives humans download — and the install commands
//! advertised in the successful-release Slack message.

use std::fs;
use std::path::PathBuf;

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
    let job = deploy_job(name);
    serde_yaml::from_value(job["needs"].clone())
        .unwrap_or_else(|error| panic!("`{name}` must declare a `needs` list: {error}"))
}

/// The composite gate every Project repository runs. It downloads the archive
/// `deploy.yml` publishes, and the two files never reference each other — so
/// the asset name is a contract only a test can hold.
fn validate_action() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(".github")
        .join("actions")
        .join("validate")
        .join("action.yml");
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

#[test]
fn releases_build_and_attach_a_windows_cli_archive() {
    let workflow = deploy_workflow();

    for required in [
        "release-windows-cli-build:",
        "runs-on: windows-latest",
        "NAVIGATOR_RELEASE_TAG: ${{ needs.release-version.outputs.tag }}",
        "Copy-Item \"target/release/navigator.exe\"",
        "Copy-Item \"LICENSE\"",
        "Compress-Archive -Path \"dist/navigator-windows/*\"",
        "release-windows-cli-publish:",
        "gh release create \"${TAG}\"",
        "gh release upload \"${TAG}\" dist/navigator-*-windows.zip",
    ] {
        assert!(
            workflow.contains(required),
            "deploy.yml must retain the Windows CLI release contract `{required}`"
        );
    }
}

/// The three CLI archives compile in the SAME stage that publishes the images to
/// GHCR: each waits on `integration` — the KIND e2e, interop, and
/// browser/accessibility suite — exactly as `publish-service` and
/// `publish-triggers` do. One gate decides whether a release produces artifacts
/// at all, and it is the e2e run.
///
/// That ordering is what makes a CLI archive mean the same thing an image tag
/// means. A release ships one version across four surfaces — the GHCR images,
/// the three archives, `navigator --version`, and the Release page — and a
/// stranger reading that version has no way to tell which surfaces the e2e
/// suite actually stood behind. Compiling the archives before the gate made
/// "e2e-proven" true of the images and merely coincidental for the CLI. It also
/// occupied three runners, one a 90-minute Windows compile, on every release
/// whose integration job then went red — building an archive for a Release that
/// never gets cut.
///
/// `publishable` stays as the second half of the gate — the SAME condition the
/// publish jobs carry — so a run that publishes images always ships all three
/// archives, and a `kind-ci/**` branch iteration stops at integration and
/// compiles none of them.
fn assert_builds_after_the_e2e_gate(job: &str) {
    let gate = deploy_job(job)["if"]
        .as_str()
        .unwrap_or_else(|| panic!("{job} must declare an `if:` gate"))
        .trim()
        .to_string();
    assert_eq!(
        gate, "needs.release-version.outputs.publishable == 'true'",
        "{job} must carry the same `publishable` gate the publish jobs do, so every run that \
         publishes images also ships a CLI, got: {gate:?}"
    );

    let needs = job_needs(job);
    for required in ["integration", "release-version"] {
        assert!(
            needs.iter().any(|entry| entry == required),
            "{job} must need `{required}` so the CLI is compiled in the same stage that pushes \
             the images to GHCR — after the e2e run, never beside it. Got: {needs:?}"
        );
    }
}

/// Every release that publishes images also builds the Windows CLI, and starts
/// that build only once the e2e suite is green.
#[test]
fn every_published_release_builds_the_windows_cli() {
    assert_builds_after_the_e2e_gate("release-windows-cli-build");
}

/// Every run that reaches this job is a tag release, so the archive is built
/// from the commit its tag will name.
///
/// There is no tag to check out: `release-windows-cli-publish` CUTS the tag,
/// at this same SHA, after both archives are built. So the archive and the tag
/// naming it describe one commit rather than two that happened to be close —
/// which is the property the old `ref: <tag>` was protecting, held from the
/// other end.
#[test]
fn the_windows_build_checks_out_the_commit_it_claims() {
    assert_builds_from_the_sha("release-windows-cli-build");
}

/// The ref must be the run's SHA and nothing else. Shared by both CLI builds
/// because the requirement is identical: the archive's name carries the version,
/// so its bytes must come from the commit that version is cut at.
fn assert_builds_from_the_sha(job: &str) {
    let build = deploy_job(job);
    let steps = build["steps"]
        .as_sequence()
        .unwrap_or_else(|| panic!("{job} must declare steps"));
    let checkout = steps
        .iter()
        .find(|step| {
            step.get("uses")
                .and_then(serde_yaml::Value::as_str)
                .is_some_and(|uses| uses.starts_with("actions/checkout@"))
        })
        .unwrap_or_else(|| panic!("{job} must check the tree out"));
    let git_ref = checkout["with"]["ref"]
        .as_str()
        .expect("the checkout must pin a ref");

    assert!(
        !git_ref.contains('\n'),
        "the ref expression must be ONE line, got: {git_ref:?}"
    );
    assert_eq!(
        git_ref.trim(),
        "${{ github.sha }}",
        "{job} must build the run's own SHA — on a tag push that IS the tagged commit, and the \
         Release these archives attach to hangs off that same tag, so checking out anything else \
         would let an archive named for a version be compiled from a different commit"
    );
}

/// A GitHub Release hangs off an immutable Git tag, and `publishable` is true
/// only for a validated tag ref, so that one output is the whole gate. The
/// trigger-shaped clauses this carried existed to stop a clock- or
/// dispatch-driven run claiming a Release no tag stood behind; neither trigger
/// exists now, and naming a retired one here would be a gate on a condition that
/// can never be true.
#[test]
fn only_tagged_releases_attach_the_archive_to_a_github_release() {
    let gate = deploy_job("release-windows-cli-publish")["if"]
        .as_str()
        .expect("release-windows-cli-publish must declare an `if:` gate")
        .to_string();

    assert!(
        gate.contains("needs.release-version.outputs.publishable == 'true'"),
        "attaching to a GitHub Release must stay gated on a publishable run, got: {gate:?}"
    );
    for retired in ["github.event_name == 'schedule'", "workflow_dispatch"] {
        assert!(
            !gate.contains(retired),
            "the gate must not name the retired trigger `{retired}` — a release is a tag push"
        );
    }
}

/// #navigator's install message offers a download per platform, and keeps the
/// build-from-source path for the one Mac no archive covers.
///
/// The source build used to be the *only* macOS instruction, because there was
/// no macOS archive to point at. Now it is the Intel fallback — `macos-latest`
/// is arm64, so that is what ships. Both halves are asserted: dropping the
/// download would send every Mac operator back to a 20-minute compile, and
/// dropping the fallback would leave an Intel Mac with an instruction that
/// produces a binary it cannot execute.
#[test]
fn the_slack_message_offers_a_download_for_every_published_archive() {
    let workflow = deploy_workflow();

    for required in [
        "navigator-${TAG}-windows.zip",
        "navigator-${TAG}-macos.tar.gz",
        "navigator-${TAG}-linux.tar.gz",
    ] {
        assert!(
            workflow.contains(required),
            "#navigator's install instructions must name the published archive `{required}`"
        );
    }

    assert!(
        workflow.contains("On an Intel Mac there is no prebuilt archive"),
        "the message must say which Mac the download does not cover"
    );
    for required in [
        "git clone --depth 1 --branch",
        "NAVIGATOR_RELEASE_TAG=",
        "cargo install --locked --path",
        "/tmp/navigator.XXXXXX",
    ] {
        assert!(
            workflow.contains(required),
            "the Intel-Mac fallback must build the immutable source tag: `{required}`"
        );
    }
}

/// The macOS archive, and the 404 it closes.
///
/// `.github/actions/validate` has always mapped a macOS runner to
/// `platform=macos` and downloaded `navigator-<tag>-macos.tar.gz`. Nothing
/// built one, so the notation gate failed on any Project repository that ran
/// it on a macOS runner — the same breakage the Linux job's comment describes,
/// one platform over, and invisible from this repository because the failure
/// lands in the consumer's CI.
#[test]
fn releases_build_and_attach_a_macos_cli_archive() {
    let workflow = deploy_workflow();

    for required in [
        "release-cli-build-macos:",
        "runs-on: macos-latest",
        "install -m 0755 target/release/navigator dist/navigator-macos/navigator",
        "install -m 0644 LICENSE dist/navigator-macos/LICENSE",
        "-C dist/navigator-macos navigator LICENSE",
        "name: navigator-macos-cli",
        "gh release upload \"${TAG}\" dist/navigator-*-macos.tar.gz",
    ] {
        assert!(
            workflow.contains(required),
            "deploy.yml must retain the macOS CLI release contract `{required}`"
        );
    }
}

/// The same two-file contract the Linux archive is held to, for the platform
/// whose absence was the reason to write this test.
#[test]
fn the_macos_archive_name_matches_what_the_validate_action_downloads() {
    assert!(
        validate_action().contains("macOS)  platform=macos"),
        "the validate action must still map a macOS runner to the `macos` platform"
    );
    assert!(
        deploy_workflow().contains("dist/navigator-${TAG}-macos.tar.gz"),
        "deploy.yml must build the exact asset name the validate action downloads"
    );
}

/// Same rule as the other two builds: gated on `publishable`, and queued behind
/// the e2e run.
#[test]
fn every_published_release_builds_the_macos_cli() {
    assert_builds_after_the_e2e_gate("release-cli-build-macos");
}

#[test]
fn the_macos_build_checks_out_the_commit_it_claims() {
    assert_builds_from_the_sha("release-cli-build-macos");
}

/// A build that fails must page, and only the jobs `notify-failure` lists can.
///
/// The list is hand-maintained and the CLI builds are the easiest rows to
/// forget: they are peers of the publishes rather than dependencies of them, so
/// a green publish reads like a green release right up until the Release carries
/// two archives instead of three.
#[test]
fn a_failed_cli_build_pages_engineering() {
    let needs = job_needs("notify-failure");
    for required in [
        "release-windows-cli-build",
        "release-cli-build-linux",
        "release-cli-build-macos",
    ] {
        assert!(
            needs.iter().any(|entry| entry == required),
            "notify-failure cannot report a failure in `{required}` unless it needs it"
        );
    }
}

/// The Linux archive is the one CI actually consumes, and it went missing for
/// long enough that no repository had a working notation gate. `deploy.yml`
/// built and attached Windows only, while `.github/actions/validate` asks for
/// `navigator-<tag>-linux.tar.gz` on every runner.
#[test]
fn releases_build_and_attach_a_linux_cli_archive() {
    let workflow = deploy_workflow();

    for required in [
        "release-cli-build-linux:",
        "runs-on: ubuntu-latest",
        "install -m 0755 target/release/navigator dist/navigator-linux/navigator",
        "install -m 0644 LICENSE dist/navigator-linux/LICENSE",
        "-C dist/navigator-linux navigator LICENSE",
        "name: navigator-linux-cli",
        "gh release upload \"${TAG}\" dist/navigator-*-linux.tar.gz",
    ] {
        assert!(
            workflow.contains(required),
            "deploy.yml must retain the Linux CLI release contract `{required}`"
        );
    }
}

/// The archive name is a contract between two files that never reference each
/// other. `.github/actions/validate` composes
/// `navigator-${VERSION}-${platform}.tar.gz` with `platform=linux`; `deploy.yml`
/// has to produce exactly that. Nothing else in the tree ties them together,
/// and the drift cost every consuming repository its `ci` job.
#[test]
fn the_linux_archive_name_matches_what_the_validate_action_downloads() {
    let action = validate_action();

    assert!(
        action.contains("navigator-${VERSION}-${platform}.tar.gz"),
        "the validate action must still compose its asset name from VERSION and platform"
    );
    assert!(
        action.contains("Linux)  platform=linux"),
        "the validate action must still map a Linux runner to the `linux` platform"
    );
    assert!(
        deploy_workflow().contains("dist/navigator-${TAG}-linux.tar.gz"),
        "deploy.yml must build the exact asset name the validate action downloads"
    );
}

/// Every CLI archive carries the licence and the notice.
///
/// A recipient holds the archive and not the repository — that is the whole
/// point of shipping a binary — so BUSL's condition that a conveyor display
/// every recipient this License along with the work is met by the archive or not
/// at all. § 13 can oblige that recipient to pass the corresponding source on in
/// turn, and nobody honours an obligation from terms they were never shown.
///
/// Both files, because they carry different halves of the answer. `LICENSE` is
/// the Free Software Foundation's text unaltered, which says nothing about this
/// work in particular; `NOTICE` carries the copyright line, the marks the grant
/// does not reach, and § 13 in the Foundation's own voice.
///
/// Asserted per platform rather than once over the file, because the packaging
/// steps are written in different shells against different paths and a fix to
/// one has already missed another.
#[test]
fn every_cli_archive_carries_the_licence_and_the_notice() {
    let workflow = deploy_workflow();

    for (platform, staged, archived) in [
        (
            "Windows",
            [
                "Copy-Item \"LICENSE\" \"dist/navigator-windows/LICENSE\"",
                "Copy-Item \"NOTICE\" \"dist/navigator-windows/NOTICE\"",
            ],
            "Compress-Archive -Path \"dist/navigator-windows/*\"",
        ),
        (
            "Linux",
            [
                "install -m 0644 LICENSE dist/navigator-linux/LICENSE",
                "install -m 0644 NOTICE dist/navigator-linux/NOTICE",
            ],
            "-C dist/navigator-linux navigator LICENSE NOTICE",
        ),
        (
            "macOS",
            [
                "install -m 0644 LICENSE dist/navigator-macos/LICENSE",
                "install -m 0644 NOTICE dist/navigator-macos/NOTICE",
            ],
            "-C dist/navigator-macos navigator LICENSE NOTICE",
        ),
    ] {
        for step in staged {
            assert!(
                workflow.contains(step),
                "the {platform} archive must stage both terms files; `{step}` is \
                 missing"
            );
        }
        assert!(
            workflow.contains(archived),
            "the {platform} archive must carry both terms files into the archive \
             itself — staging them beside the binary and then packing only the \
             binary ships an executable with no terms"
        );
    }
}

/// Same rule as the Windows build: the `publishable` gate is what makes a
/// publishing release ship a CLI for CI to install, so a run cannot publish
/// images while quietly shipping none.
#[test]
fn every_published_release_builds_the_linux_cli() {
    assert_builds_after_the_e2e_gate("release-cli-build-linux");
}

/// The archives and the images are peers in stage 2, so neither is cut into a
/// Release by the other finishing. `release-windows-cli-publish` is the first
/// job in the run that publishes anything a stranger can fetch, and the CLI it
/// attaches is only half a release without the images the same tag names — so it
/// waits on both publish jobs as well as the three builds.
#[test]
fn the_release_is_cut_only_after_the_images_publish() {
    let needs = job_needs("release-windows-cli-publish");
    for required in ["publish-service", "publish-triggers"] {
        assert!(
            needs.iter().any(|entry| entry == required),
            "the Release must not exist before the images it accompanies, so the attach job \
             needs `{required}`"
        );
    }
}

/// One publish job attaches all three archives. Several would each run
/// `gh release create` behind a check-then-act `if ! gh release view` guard and
/// race on the same tag.
#[test]
fn one_publish_job_attaches_every_cli_archive() {
    let needs = job_needs("release-windows-cli-publish");
    for required in [
        "release-windows-cli-build",
        "release-cli-build-linux",
        "release-cli-build-macos",
    ] {
        assert!(
            needs.iter().any(|entry| entry == required),
            "the publish job attaches every archive, so it needs `{required}`"
        );
    }

    // Count real invocations, not prose: this file's own comments discuss the
    // command, and an earlier version of this assertion counted them.
    let invocations = deploy_workflow()
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !trimmed.starts_with('#') && trimmed.starts_with("gh release create")
        })
        .count();
    assert_eq!(
        invocations, 1,
        "exactly one job may create the Release, or two runs race on the same tag"
    );
}

/// The Linux archive is the one CI installs, so it is held to the same rule as
/// the Windows one: built from the tag it is named for.
#[test]
fn the_linux_build_checks_out_the_commit_it_claims() {
    assert_builds_from_the_sha("release-cli-build-linux");
}
