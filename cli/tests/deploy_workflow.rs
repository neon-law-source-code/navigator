use std::path::Path;

fn deploy_workflow() -> String {
    repo_file(".github/workflows/deploy.yml")
}

fn repo_file(path: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(path),
    )
    .unwrap_or_else(|error| panic!("read {path}: {error}"))
}

/// The trigger block, parsed. `on` is the YAML 1.1 boolean `true`, so
/// `serde_yaml` keys it as a bool rather than the string "on" — reading it by
/// name silently finds nothing and every assertion below would pass vacuously.
fn deploy_triggers() -> serde_yaml::Mapping {
    let workflow: serde_yaml::Value =
        serde_yaml::from_str(&deploy_workflow()).expect("deploy.yml parses as YAML");
    let triggers = workflow
        .get(serde_yaml::Value::Bool(true))
        .or_else(|| workflow.get("on"))
        .expect("deploy.yml must declare a trigger block");
    triggers
        .as_mapping()
        .expect("the trigger block must be a mapping")
        .clone()
}

fn has_trigger(name: &str) -> bool {
    deploy_triggers().contains_key(serde_yaml::Value::String(name.to_string()))
}

#[test]
fn deploy_workflow_has_no_pull_request_trigger() {
    let workflow = deploy_workflow();

    assert!(
        !workflow.contains("\n  pull_request:\n"),
        "deploy.yml must not trigger on pull_request — UI/browser proof runs on the \
         release train and locally, never on a PR"
    );
}

/// One job's `needs`, normalised — the key is written as a scalar, a flow list,
/// and a block list across this file's jobs.
fn job_needs(workflow: &serde_yaml::Value, job: &str) -> Vec<String> {
    match &workflow["jobs"][job]["needs"] {
        serde_yaml::Value::String(need) => vec![need.clone()],
        serde_yaml::Value::Sequence(needs) => needs
            .iter()
            .map(|need| {
                need.as_str()
                    .unwrap_or_else(|| panic!("{job} has a non-string need"))
                    .to_string()
            })
            .collect(),
        serde_yaml::Value::Null => panic!("{job} declares no needs"),
        other => panic!("{job} needs must be a string or list, got {other:?}"),
    }
}

/// THE WORKFLOW DEPLOYS NOTHING, AND HOLDS NO CLOUD CREDENTIAL. It ends at the
/// registry: every rollout is `navigator ops ship`, run by a person against
/// their own short-lived credentials.
///
/// This is a security boundary, not a preference. A pipeline that can roll a
/// cluster is a pipeline whose compromise rolls that cluster, and a ship job
/// added back here would restore that reach silently — the run would go green
/// and nobody would read the diff that did it. Two things are asserted, because
/// a job can reach a cloud provider without being called `ship-*`: no such job
/// exists, and no step federates an identity into one.
#[test]
fn deploy_workflow_ships_nothing_and_holds_no_cloud_credential() {
    let workflow: serde_yaml::Value =
        serde_yaml::from_str(&deploy_workflow()).expect("deploy.yml parses as YAML");
    let jobs = workflow["jobs"]
        .as_mapping()
        .expect("deploy.yml must declare jobs");
    let names: Vec<String> = jobs
        .keys()
        .map(|key| key.as_str().unwrap_or("<non-string job key>").to_string())
        .collect();

    let shipping: Vec<&String> = names
        .iter()
        .filter(|name| name.starts_with("ship"))
        .collect();
    assert!(
        shipping.is_empty(),
        "deploy.yml must contain no ship job: {shipping:?}. Publishing is automated; deploying is \
         a human act run from `navigator ops ship`, so nothing here may roll a cluster"
    );

    // Comments are stripped first. The header explains at length that this
    // workflow federates into no cloud provider, and naming the thing it does
    // not do must not read as doing it.
    let source = deploy_workflow();
    let effective: String = source
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");

    for forbidden in ["google-github-actions/auth", "workload_identity_provider"] {
        assert!(
            !effective.contains(forbidden),
            "deploy.yml still references `{forbidden}` — this workflow reaches no cloud provider, \
             and a surviving credential exchange is reach it does not need"
        );
    }
}

/// The `inputs` context cannot exist without a declared input, so any reference
/// to one is dead. This is ENG-182 as a guard.
#[test]
fn deploy_workflow_references_no_workflow_inputs() {
    let workflow = deploy_workflow();

    assert!(
        !workflow.contains("inputs."),
        "deploy.yml declares no inputs, so `inputs.<name>` always evaluates empty — a reference \
         to one is dead and reads as a knob that exists (ENG-182)"
    );
}

/// A prerelease must not masquerade as the latest release on the one surface
/// that decides what a browsing reader gets by default.
///
/// The GitHub Release is flagged so it stops being reported as "Latest". That is
/// the ONLY place a prerelease behaves differently: `brew` resolves exactly one
/// version and needs it to be the newest good build, so the tap follows every
/// publishable version. `a_hotfix_is_dispatched_to_the_tap` holds that half.
///
/// Which versions ARE prereleases is no longer a spelling rule this workflow
/// knows. `ops release check` reports `prerelease` from `Version::pre`, so any
/// semver prerelease — `-hotfix.3`, `-rc.1` — is flagged, and the workflow reads
/// the answer instead of matching a suffix.
#[test]
fn a_prerelease_publishes_as_a_prerelease() {
    let workflow: serde_yaml::Value =
        serde_yaml::from_str(&deploy_workflow()).expect("deploy.yml parses as YAML");

    let outputs = &workflow["jobs"]["release-version"]["outputs"];
    assert!(
        outputs["prerelease"].as_str().is_some(),
        "`release-version` must publish a `prerelease` output for downstream jobs to gate on"
    );

    assert!(
        deploy_workflow().contains("flags+=(--prerelease)"),
        "the GitHub Release for a prerelease must be created with --prerelease so it is not \
         reported as the latest release"
    );
}

/// A LANDED VERSION BUMP IS THE ONLY WAY TO PUBLISH, and that is what makes an
/// image's version trustworthy.
///
/// Three triggers have owned this workflow. The clock and the dispatch both
/// derived a version from `date`, so the name an image carried stood behind no
/// Git ref: `Cargo.toml` sat at one version while published images marched on
/// under another. The pushed tag fixed that but paid in ceremony — four
/// validations existed to re-establish facts a bare ref cannot carry, each of
/// which spent an immutable name when it failed late.
///
/// Reading the version out of the merged manifest answers all four by
/// construction. All three retired triggers are asserted absent rather than
/// merely unused: any of them surviving would publish under a version this
/// repository's source does not name, and would go green doing it.
#[test]
fn deploy_workflow_publishes_only_from_a_landed_version_bump() {
    assert!(
        !has_trigger("schedule"),
        "deploy.yml must not publish on a clock: a cron can only derive a version, which is \
         exactly the drift that made the manifest and the published images disagree"
    );
    assert!(
        !has_trigger("workflow_dispatch"),
        "deploy.yml must not publish on demand: a dispatch would publish a version nobody \
         reviewed. Merge the bump instead"
    );
    assert!(
        has_trigger("push"),
        "deploy.yml publishes from a push to `main`, so it must keep its `push` trigger"
    );

    let triggers = deploy_triggers();
    let push = &triggers[&serde_yaml::Value::String("push".into())];
    assert!(
        push["tags"].is_null(),
        "deploy.yml must carry NO tag trigger. A pushed tag was the publish path until the \
         version became the trigger, and keeping it would mean keeping the four validations a \
         bare ref needs — shape, date, manifest equality, and provenance — for a second path \
         nobody uses"
    );
}

/// THE VERSION IS THE MANIFEST, and the manifest is read by one Rust guard.
///
/// This replaces four workflow-level checks with one call, and the deletion is
/// the point rather than a side effect. Shape was a `grep -E` transcribing a
/// grammar; date was UTC arithmetic that could only fail a bump for having been
/// reviewed slowly; manifest equality compared a tag against the value the tag
/// is now derived FROM; and provenance proved an ancestry that a push to `main`
/// asserts. What survives is the one question none of them asked: is this
/// version newer than every version already published?
#[test]
fn the_release_decision_is_one_rust_guard_over_the_manifest() {
    let workflow: serde_yaml::Value =
        serde_yaml::from_str(&deploy_workflow()).expect("deploy.yml parses as YAML");
    let steps = workflow["jobs"]["release-version"]["steps"]
        .as_sequence()
        .expect("release-version must declare steps");

    let script: String = steps
        .iter()
        .filter_map(|step| step["run"].as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        script.contains("ops release check --github-output"),
        "release-version must decide the release by running `ops release check`, which reads \
         `[workspace.package].version` and compares it against the release tags"
    );

    // The retired checks, asserted GONE. Each one left behind would be a second
    // rule the release has to satisfy, and the date check in particular would
    // refuse a bump merged on a different day than it was authored.
    let source = deploy_workflow();
    for (forbidden, why) in [
        (
            "TZ=UTC date",
            "the release must not read a clock: `YY.M.D` is a naming convention, and a bump is \
             authored days before it merges",
        ),
        (
            "ops release-provenance",
            "a push to `main` IS the provenance, so the ancestry guard has nothing left to prove",
        ),
    ] {
        assert!(
            !source.contains(forbidden),
            "deploy.yml still references `{forbidden}` — {why}"
        );
    }

    // Reading the manifest and building the Rust guard require the source on
    // disk, and the decision is made against the tags — so the checkout has to
    // carry them.
    let checkout = steps
        .iter()
        .find(|step| {
            step["uses"]
                .as_str()
                .unwrap_or_default()
                .starts_with("actions/checkout")
        })
        .expect("release-version must check out the tree it reads");
    assert!(
        checkout["with"]["sparse-checkout"].is_null(),
        "release-version must check out the full source: the guard that makes the decision is a \
         Rust binary in this tree"
    );
    assert_eq!(
        checkout["with"]["fetch-depth"].as_u64(),
        Some(0),
        "the release decision is made against every release tag, so the checkout must carry them"
    );
}

/// NO MERGE THAT CARRIES NO BUMP MAY BUILD ANYTHING.
///
/// Every merge to `main` starts this workflow and almost none of them are
/// releases. `release-version` answers in seconds, but `build` and `integration`
/// are a four-image cold compile and the whole KIND suite — so without a
/// publishability gate on those two, the trigger change would bill the entire
/// release pipeline on every merge and publish nothing. That failure is silent:
/// the run goes green.
///
/// The `kind-ci/**` leg has to survive the same gate, because a branch iteration
/// reports `publishable=false` by design and building the images is the entire
/// point of that trigger.
#[test]
fn only_a_release_or_a_branch_iteration_builds() {
    let workflow: serde_yaml::Value =
        serde_yaml::from_str(&deploy_workflow()).expect("deploy.yml parses as YAML");

    for job in ["build", "integration"] {
        let condition = workflow["jobs"][job]["if"]
            .as_str()
            .unwrap_or_else(|| panic!("{job} must carry an `if`"));
        let condition: String = condition.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            condition.contains("needs.release-version.outputs.publishable == 'true'"),
            "{job} must be gated on publishability, or every merge to `main` runs it: {condition}"
        );
        assert!(
            condition.contains("startsWith(github.ref_name, 'kind-ci/')"),
            "{job} must still run for a `kind-ci/**` branch iteration, which reports \
             publishable=false by design: {condition}"
        );
    }
}

/// THE TAG IS CREATED AFTER THE TREE IS PROVED, AND BEFORE ANYTHING PUBLISHES.
///
/// The ordering is the whole value of moving the tag into the pipeline. While a
/// person pushed it, the ref existed before a single image was built, so a
/// release that then went red had already spent its name and had to be recovered
/// under a different version. Created here, a failure above the line costs
/// nothing but a re-run.
///
/// Both halves are asserted. `release-tag` must wait for `integration`, or the
/// ref would again precede the proof; and every publisher must wait for
/// `release-tag`, or an artifact could exist under a version no ref names.
#[test]
fn the_release_tag_is_created_between_the_proof_and_the_publish() {
    let workflow: serde_yaml::Value =
        serde_yaml::from_str(&deploy_workflow()).expect("deploy.yml parses as YAML");

    let tag_job = &workflow["jobs"]["release-tag"];
    assert!(
        !tag_job.is_null(),
        "deploy.yml must declare the `release-tag` job that creates the release ref"
    );
    assert!(
        job_needs(&workflow, "release-tag").contains(&"integration".to_string()),
        "release-tag must wait for integration: a ref created before the proof is a name spent on \
         an unproved tree"
    );
    assert_eq!(
        tag_job["if"].as_str(),
        Some("needs.release-version.outputs.publishable == 'true'"),
        "release-tag must create nothing on a `kind-ci/**` iteration or an ordinary merge"
    );

    for publisher in [
        "publish-service",
        "publish-triggers",
        "release-windows-cli-build",
        "release-cli-build-linux",
        "release-cli-build-macos",
    ] {
        assert!(
            job_needs(&workflow, publisher).contains(&"release-tag".to_string()),
            "{publisher} must wait for the release tag, or it could publish an artifact under a \
             version no Git ref names"
        );
    }

    // The Release attaches to the ref this run created, so it inherits the wait
    // transitively through the archive builds — asserted explicitly because
    // "transitively" is exactly the kind of claim that stops being true.
    let release_needs = job_needs(&workflow, "release-windows-cli-publish");
    assert!(
        release_needs
            .iter()
            .any(|need| need == "release-cli-build-linux"),
        "the GitHub Release job must wait for the archive builds that wait for the tag"
    );
}

/// The tag this workflow CREATES must be one the `release-tags` ruleset
/// protects.
///
/// `cli/src/devx/github_setup.rs` restricts `refs/tags/[0-9]*.[0-9]*.[0-9]*`
/// against deletion, update, and non-fast-forward with no bypass actor. That is
/// what makes a published version's ref immutable — and it only binds if the
/// name this job creates matches the glob. A version that fell outside it would
/// publish under a MOVABLE ref, which makes every artifact carrying that version
/// rewritable after the fact.
#[test]
fn the_created_tag_matches_the_protected_glob() {
    let glob = "refs/tags/[0-9]*.[0-9]*.[0-9]*";
    let setup = repo_file("cli/src/devx/github_setup.rs");
    assert!(
        setup.contains(glob),
        "the `release-tags` ruleset must still protect `{glob}`"
    );

    // The Rust side owns the shape, so the glob and the parser live together —
    // this asserts the workflow creates a ref under the namespace that glob
    // covers rather than re-deriving the shape here.
    let workflow = deploy_workflow();
    assert!(
        workflow.contains("ref=refs/tags/${TAG}"),
        "release-tag must create the ref under `refs/tags/`, the namespace the ruleset protects"
    );
    let release = repo_file("cli/src/release.rs");
    assert!(
        release.contains(r#"pub const RELEASE_TAG_GLOB: &str = "[0-9]*.[0-9]*.[0-9]*";"#),
        "cli/src/release.rs must hold the same glob the ruleset protects, so the one place that \
         lists tags and the one place that protects them cannot drift"
    );
}

/// Creating the tag must be idempotent, because a release run gets re-run.
///
/// A tag already pointing at this commit is this run's own, and saying so is
/// what lets a failed release be re-run instead of recovered under a new
/// version. A tag pointing at a DIFFERENT commit must be refused rather than
/// forced: that is someone else's release wearing this version.
#[test]
fn creating_the_release_tag_is_idempotent_but_never_forced() {
    let workflow = deploy_workflow();

    assert!(
        workflow.contains(r#"if [ "${existing}" = "${SHA}" ]; then"#),
        "release-tag must recognise its own tag on a re-run instead of failing"
    );
    // The two ways a ref could be MOVED, asserted absent. A blanket `--force`
    // search would be wrong here: `kubectl apply --force-conflicts` and the
    // Intel-Mac `cargo install --force` fallback both live in this file and
    // neither touches a ref.
    let effective: String = workflow
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    for forbidden in [
        "git push --force",
        "git push -f",
        "--method PATCH",
        "--method PUT",
    ] {
        assert!(
            !effective.contains(forbidden),
            "deploy.yml references `{forbidden}` — nothing here may move a ref. The \
             `release-tags` ruleset forbids it, and a moved tag makes every artifact already \
             carrying that version a lie"
        );
    }
}

/// A CONTAINER MUST REPORT THE VERSION ITS IMAGE IS TAGGED WITH.
///
/// Two independent things hold this, and both are needed. The
/// tag-equals-`Cargo.toml` check above makes the *source* carry the release
/// name, so a plain `cargo build` of the tagged tree self-reports correctly.
/// This one makes the *image* carry it: the tag is passed as the `RELEASE_TAG`
/// build-arg, each Containerfile turns it into a runtime
/// `ENV NAVIGATOR_RELEASE_TAG`, and `main.rs` reads that override.
///
/// Drop the build-arg and nothing fails: images still publish, and every one of
/// them silently reports whatever the manifest happened to say. That silence is
/// why this is asserted.
#[test]
fn every_image_is_stamped_with_the_release_tag() {
    let workflow = deploy_workflow();

    assert!(
        workflow.contains("printf 'RELEASE_TAG=%s"),
        "deploy.yml must pass the derived version to every image build as the `RELEASE_TAG` \
         build-arg — without it a published container reports the wrong version and nothing fails"
    );

    let containerfile = repo_file("images/Containerfile.neon");
    assert!(
        containerfile.contains("ARG RELEASE_TAG"),
        "Containerfile.neon must accept the RELEASE_TAG build-arg deploy.yml passes it"
    );
    assert!(
        containerfile.contains("ENV NAVIGATOR_RELEASE_TAG=$RELEASE_TAG"),
        "Containerfile.neon must expose RELEASE_TAG as the runtime NAVIGATOR_RELEASE_TAG override \
         `main.rs` reads — a build-arg that never becomes an ENV stamps nothing"
    );
}

/// Nothing in the release pipeline may move a Git ref. `release-version` held
/// `contents: write` to cut the nightly tag; a human pushes the tag now, so the
/// permission is gone and the whole workflow is read-only against the
/// repository.
#[test]
fn no_job_can_write_repository_contents() {
    let workflow: serde_yaml::Value =
        serde_yaml::from_str(&deploy_workflow()).expect("deploy.yml parses as YAML");

    assert_eq!(
        workflow["permissions"]["contents"].as_str(),
        Some("read"),
        "the workflow-level contents permission must stay `read`"
    );

    let jobs = workflow["jobs"]
        .as_mapping()
        .expect("deploy.yml must declare jobs");
    let mut writers = Vec::new();
    for (name, job) in jobs {
        if job["permissions"]["contents"].as_str() == Some("write") {
            writers.push(name.as_str().unwrap_or("<non-string job key>").to_string());
        }
    }

    // TWO JOBS, AND THE LIST IS THE POINT. `release-tag` creates the release ref
    // once integration has proved the tree; `release-windows-cli-publish`
    // creates the GitHub Release that hangs off it and uploads the CLI archives.
    // Neither can MOVE a ref — see
    // `creating_the_release_tag_is_idempotent_but_never_forced` — and
    // `release-version`, which makes the decision, holds `contents: read`: the
    // job that decides is not the job that writes.
    assert_eq!(
        writers,
        ["release-tag", "release-windows-cli-publish"],
        "exactly two jobs may hold `contents: write`: the one that creates the release ref and \
         the one that creates the Release against it"
    );

    assert_eq!(
        workflow["jobs"]["release-version"]["permissions"]["contents"].as_str(),
        Some("read"),
        "the job that decides whether this is a release must not be able to write one"
    );
}

/// The browser gate builds every image it then audits.
///
/// It used to clone the deployed pod on a second port and run a second brand
/// binary beside it, because the two faces used to be separate images.
/// One binary serves both faces now, so that whole apparatus is gone — and what
/// survives is the part that always mattered: the images the gate exercises are
/// the ones a deployment rolls, not a route substitution.
#[test]
fn browser_accessibility_uses_the_shipped_images() {
    let workflow = deploy_workflow();

    for required in [
        "          - image: neon-server\n            dockerfile: images/Containerfile.neon",
        "for img in navigator-web neon-server navigator-workflows-service navigator-gateway; do",
    ] {
        assert!(
            workflow.contains(required),
            "deploy.yml must keep browser accessibility proof `{required}`"
        );
    }

    // The retired second-host apparatus must not come back: it cloned the web
    // Deployment onto port 3002 to run a second brand image, and there is no
    // second brand image to run.
    for retired in [
        "neon-browser-a11y",
        "NAV_BASE_URL: http://localhost:3002",
        ".image = \"neon-server:dev\"",
    ] {
        assert!(
            !workflow.contains(retired),
            "deploy.yml still carries the retired second-host clone `{retired}`; \
             one binary serves both faces, so the gate audits one host"
        );
    }
}

/// The public-host image compiles the full server and Dioxus web bundle. A
/// stock `ubuntu-latest` runner timed out building `neon-server` at the release
/// job's 90-minute wedge detector (run 32185875546), so that leg needs a
/// Blacksmith machine. Four vCPU is the width the workspace standardised on
/// after run 32536363029 measured the merge gate on both images, and Blacksmith
/// bills linearly in cores — so a leg left at eight is paying twice the rate
/// for the same core-minutes.
///
/// Keep the one heavy leg on the metered four-vCPU runner and every other leg
/// on `ubuntu-latest`, which is free on a public repository. The smaller
/// service images do not compile the workspace and do not earn a metered
/// machine.
#[test]
fn the_public_host_image_builds_on_the_blacksmith_four_vcpu_runner() {
    let workflow: serde_yaml::Value =
        serde_yaml::from_str(&deploy_workflow()).expect("deploy.yml parses as YAML");
    let matrix = workflow["jobs"]["build"]["strategy"]["matrix"]["include"]
        .as_sequence()
        .expect("the build job must declare an include matrix");

    let leg = matrix
        .iter()
        .find(|leg| leg["image"].as_str() == Some("neon-server"))
        .expect("the build matrix must include neon-server");
    assert_eq!(
        leg["runner"].as_str(),
        Some("blacksmith-4vcpu-ubuntu-2404"),
        "neon-server compiles the full Rust and Dioxus application and must use the four-vCPU \
         Blacksmith runner the workspace standardised on"
    );

    for leg in matrix
        .iter()
        .filter(|leg| leg["image"].as_str() != Some("neon-server"))
    {
        assert_eq!(
            leg["runner"].as_str(),
            Some("ubuntu-latest"),
            "{:?} does not compile the workspace and must not hold a metered runner",
            leg["image"].as_str().unwrap_or("?")
        );
    }
}

/// `publish-service` recompiles the same Containerfile the `build` job does,
/// so it needs the same machine and the same clock.
///
/// The two jobs feed `images/Containerfile.neon` different inputs — `build`
/// stubs `server/public` for the KIND gate — so `publish-service` cannot read
/// `build`'s cache and always compiles the workspace cold. That is the whole
/// reason its `neon-server` leg carries no `ci_cache_scope`. A cold compile is
/// exactly the work the four-vCPU guard above exists for, and this leg was left
/// on the free two-vCPU `ubuntu-latest` machine anyway: run 33256595378 built
/// the identical image in 21m0s under `build` and 44m17s here, then died 37
/// seconds into the alias step on a 45-minute cap. It had been landing inside
/// that cap by seconds for a month — 44m35s, 44m56s, 44m12s — so the cap was
/// not detecting a wedge, it was rationing a build.
///
/// Both halves are asserted together because either alone leaves the release
/// one slow minute from losing its `navigator-web` alias.
#[test]
fn the_public_host_image_publishes_on_the_same_runner_and_clock_as_it_builds() {
    let workflow: serde_yaml::Value =
        serde_yaml::from_str(&deploy_workflow()).expect("deploy.yml parses as YAML");
    let build = &workflow["jobs"]["build"];
    let publish = &workflow["jobs"]["publish-service"];

    let matrix = publish["strategy"]["matrix"]["include"]
        .as_sequence()
        .expect("the publish-service job must declare an include matrix");
    let leg = matrix
        .iter()
        .find(|leg| leg["image"].as_str() == Some("neon-server"))
        .expect("the publish-service matrix must include neon-server");
    assert_eq!(
        leg["runner"].as_str(),
        Some("blacksmith-4vcpu-ubuntu-2404"),
        "publish-service compiles neon-server cold from the same Containerfile `build` does, so \
         it must hold the same four-vCPU Blacksmith runner rather than a free two-vCPU machine"
    );

    for leg in matrix
        .iter()
        .filter(|leg| leg["image"].as_str() != Some("neon-server"))
    {
        assert_eq!(
            leg["runner"].as_str(),
            Some("ubuntu-latest"),
            "{:?} reads a warm cache and must not hold a metered runner",
            leg["image"].as_str().unwrap_or("?")
        );
    }

    let build_timeout = build["timeout-minutes"]
        .as_u64()
        .expect("the build job must declare timeout-minutes");
    let publish_timeout = publish["timeout-minutes"]
        .as_u64()
        .expect("the publish-service job must declare timeout-minutes");
    assert!(
        publish_timeout >= build_timeout,
        "publish-service compiles what build compiles and then pushes it, so its \
         {publish_timeout}-minute cap must not sit below build's {build_timeout}-minute one — a \
         cap a successful build routinely finishes inside by seconds detects no wedge"
    );
}

/// `navigator-web` is a TAG on the image `neon-server`'s leg builds, never a
/// second build of it.
///
/// Both names have only ever come from `images/Containerfile.neon`, so the
/// alias leg was paying a second ~26-minute metered build for bytes the run
/// already had: measured across five days, 263 minutes of `navigator-web`
/// beside 272 of `neon-server`, half of this workflow's entire runner bill.
/// `publish` collapsed the same pair into an `alias` earlier; this is `build`
/// catching up.
///
/// What the saving must not cost is the KIND gate. `integration` loads one
/// tarball per image NAME — `for img in navigator-web neon-server ...` — so the
/// alias has to keep arriving as its own `ci-image-*` artifact. That coupling
/// is invisible from either file alone, which is why it is asserted here.
#[test]
fn navigator_web_is_an_alias_tag_rather_than_a_second_metered_build() {
    let raw = deploy_workflow();
    let workflow: serde_yaml::Value =
        serde_yaml::from_str(&raw).expect("deploy.yml parses as YAML");
    let build = &workflow["jobs"]["build"];
    let matrix = build["strategy"]["matrix"]["include"]
        .as_sequence()
        .expect("the build job must declare an include matrix");

    assert!(
        !matrix
            .iter()
            .any(|leg| leg["image"].as_str() == Some("navigator-web")),
        "navigator-web is byte-identical to neon-server; a matrix leg of its own buys a second \
         metered build of an image the run already holds"
    );

    let neon = matrix
        .iter()
        .find(|leg| leg["image"].as_str() == Some("neon-server"))
        .expect("the build matrix must include neon-server");
    assert_eq!(
        neon["alias"].as_str(),
        Some("navigator-web"),
        "the deployed GKE manifests still pull navigator-web, so neon-server's leg must publish \
         that tag as an alias"
    );

    let steps = build["steps"]
        .as_sequence()
        .expect("the build job must declare steps");
    assert!(
        steps
            .iter()
            .filter_map(|step| step["run"].as_str())
            .any(|script| script.contains("docker tag") && script.contains("matrix.alias")),
        "the alias must be produced by tagging the built image"
    );
    assert!(
        steps
            .iter()
            .any(|step| { step["with"]["name"].as_str() == Some("ci-image-${{ matrix.alias }}") }),
        "integration loads one tarball per image name, so the alias needs its own ci-image-* \
         artifact — without it the KIND gate fails at `docker load` on navigator-web.tar"
    );
}

/// Slack is an optional reporting surface, not a publication gate. Progress
/// posts already notice-and-skip when the webhook is absent; the terminal
/// success report and failure alert must follow the same contract. Otherwise a
/// fully published release ends red solely because this public repository has
/// no Slack secret configured (run 32148764921).
#[test]
fn missing_slack_webhook_does_not_fail_the_release_workflow() {
    let workflow: serde_yaml::Value =
        serde_yaml::from_str(&deploy_workflow()).expect("deploy.yml parses as YAML");

    for job in ["notify", "notify-failure"] {
        let steps = workflow["jobs"][job]["steps"]
            .as_sequence()
            .unwrap_or_else(|| panic!("{job} must declare steps"));
        for script in steps.iter().filter_map(|step| step["run"].as_str()) {
            if script.contains("SLACK_WEBHOOK_URL is unset") {
                assert!(
                    script.contains("::notice::SLACK_WEBHOOK_URL is unset"),
                    "{job} must report an absent optional webhook as a notice"
                );
                assert!(
                    script.contains("exit 0"),
                    "{job} must skip successfully when the optional webhook is absent"
                );
                assert!(
                    !script.contains("::error::SLACK_WEBHOOK_URL is unset"),
                    "{job} must not turn an absent optional webhook into a release failure"
                );
            }
        }
    }
}

/// THE 502 RACE, kept as a guard against its return. A one-shot
/// `curl --fail .../readyz` under `set -e`, fired while a load balancer is
/// still swapping backends, went red on `neon-production` in run 154026811
/// AFTER the roll had succeeded.
///
/// This workflow no longer probes a deployed host at all — it publishes images
/// and stops — so the assertion is now the absence of the shape rather than the
/// presence of the fix. If a probe is ever added back here, it must poll to a
/// deadline with the curl inside an `if`, never bare under `set -e`.
#[test]
fn no_readyz_probe_is_one_shot_curled() {
    let workflow = deploy_workflow();

    let bare: Vec<&str> = workflow
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("curl ") && line.contains("/readyz"))
        .collect();
    assert!(
        bare.is_empty(),
        "a /readyz probe must never be a bare command under `set -e` — a transient 502 during \
         the load-balancer swap then fails a step that already succeeded. Put the curl in an \
         `if` condition and poll to a deadline. Found: {bare:#?}"
    );
}

/// NO CLUSTER MANIFEST IS FETCHED AT RUN TIME. Every `kubectl apply` in the
/// KIND job reads a file this repository vendors.
///
/// `raw.githubusercontent.com` rate-limits by runner IP, and `kubectl` turns
/// its 429 into a hard error rather than a retry: run 32040810491 lost a
/// release four minutes into the integration job, after an hour of image
/// builds, because the ingress manifest happened to be unreachable in that
/// minute. Vendoring is also what makes the version pin real — a URL pinned
/// to a tag still trusts whatever bytes that tag serves today, while
/// `cli::devx::ingress_manifest_tests` holds the vendored copy to a recorded
/// digest.
///
/// The assertion is on the shape, not on the two known URLs, because the next
/// manifest added here would reintroduce the outage silently: the run would go
/// green on every attempt where the third party happened to answer.
#[test]
fn every_kubectl_apply_reads_a_vendored_manifest() {
    let workflow = deploy_workflow();

    // Backslash continuations are folded first: the Restate CRD apply carries
    // its `-f` argument on the following line, so a plain line scan sees a
    // `kubectl apply` with no URL and a URL with no `kubectl apply`, and passes
    // while the fetch is still there.
    let folded = workflow.replace("\\\n", " ");
    let remote: Vec<String> = folded
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.starts_with('#'))
        .filter(|line| line.contains("kubectl apply") && line.contains("://"))
        .collect();
    assert!(
        remote.is_empty(),
        "a release must not depend on a third party serving a manifest in the minute it runs — \
         vendor it under `k8s/vendor/` and apply the file, as `cli::devx::orchestrate` does. \
         Found: {remote:#?}"
    );

    // Both vendored roots must be named here, and every artifact must be
    // present in the tree — otherwise the apply trades a 429 for a missing
    // file and nothing is gained. The Restate CRDs are applied through a shell
    // loop, so the directory is what appears literally.
    for named in [
        "k8s/vendor/ingress-nginx-controller-v1.11.2.yaml",
        "k8s/vendor/restate-operator-v2.8.1/",
    ] {
        assert!(
            workflow.contains(named),
            "deploy.yml must apply the vendored `{named}`, keeping the KIND job on the same \
             manifests `dev up` applies locally"
        );
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for vendored in [
        "k8s/vendor/ingress-nginx-controller-v1.11.2.yaml",
        "k8s/vendor/restate-operator-v2.8.1/restateclusters.yaml",
        "k8s/vendor/restate-operator-v2.8.1/restatedeployments.yaml",
        "k8s/vendor/restate-operator-v2.8.1/restatecloudenvironments.yaml",
    ] {
        assert!(
            root.join(vendored).exists(),
            "`{vendored}` is named by deploy.yml but is missing from the tree"
        );
    }
}

#[test]
fn standalone_wasm_workflow_stays_retired() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    assert!(
        !root.join(".github/workflows/webapp-wasm.yml").exists(),
        "the deploy image build is the one Dioxus wasm proof path"
    );
}

/// An Actions-cache export nobody reads is pure quota. The 10 GB per-repository
/// budget is shared with `ci.yml`'s Rust dependency cache, and `mode=max` over a
/// Rust builder stage exports the whole `target` directory. Joining each `build`
/// leg's exported layer digests against the cache listing for run 32487939326
/// measured 3.78 GB for `neon-server`, 1.81 GB for `navigator-workflows-service`
/// and 0.54 GB for `navigator-gateway` — and `publish-service` gives
/// `neon-server` no `ci_cache_scope`, so the largest of the three was read by
/// nothing. Together they left no room for the gate's ~1.7 GB dependency cache,
/// which uploaded at the end of a `main` run and was evicted before the next PR
/// could restore it, so every gate run logged `No cache found.`
///
/// `build` therefore exports a scope only where `publish-service` reads one
/// back. The two matrices are edited independently and the coupling is invisible
/// from either one alone, which is why it is asserted here.
#[test]
fn build_exports_an_actions_cache_scope_only_where_publish_service_reads_it() {
    let workflow: serde_yaml::Value =
        serde_yaml::from_str(&deploy_workflow()).expect("deploy.yml parses as YAML");

    let legs = |job: &str| -> Vec<serde_yaml::Value> {
        workflow["jobs"][job]["strategy"]["matrix"]["include"]
            .as_sequence()
            .unwrap_or_else(|| panic!("the {job} job must declare an include matrix"))
            .clone()
    };
    let scopes = |job: &str| -> Vec<String> {
        legs(job)
            .iter()
            .filter_map(|leg| leg["ci_cache_scope"].as_str().map(str::to_string))
            .collect()
    };

    let exported = scopes("build");
    let read_back = scopes("publish-service");

    assert!(
        !exported.is_empty(),
        "at least one build leg must still export a scope publish-service reads, or a release \
         compiles the same crates twice"
    );
    for scope in &exported {
        assert!(
            read_back.contains(scope),
            "the build job exports the Actions-cache scope {scope}, which publish-service does \
             not list as a ci_cache_scope — an export with no reader spends the shared 10 GB \
             quota and evicts ci.yml's Rust dependency cache"
        );
    }

    // The export must be reachable ONLY through that key, so a leg without one
    // touches the Actions cache not at all.
    let build = &workflow["jobs"]["build"];
    let cache_to = build["steps"]
        .as_sequence()
        .expect("the build job must declare steps")
        .iter()
        .find_map(|step| step["with"]["cache-to"].as_str())
        .expect("the build job must declare a cache-to");
    assert!(
        cache_to.contains("matrix.ci_cache_scope") && !cache_to.contains("matrix.image"),
        "cache-to must be gated on matrix.ci_cache_scope, not derived from matrix.image — \
         deriving it exports a scope for every leg, including the one nothing reads"
    );

    // The reason this job cannot simply follow publish-service into Artifact
    // Registry: its `if:` admits `kind-ci/**` branch pushes, which no review
    // gates, so it must never hold a registry credential.
    assert!(
        build["permissions"].is_null(),
        "build must not declare permissions of its own — it runs for kind-ci/** branch pushes \
         and must hold no registry credential (no id-token: write, no WIF)"
    );
    assert!(
        build["if"]
            .as_str()
            .expect("the build job must declare an if:")
            .contains("kind-ci/"),
        "the kind-ci/** trigger is why build holds no registry credential; if it stops running \
         for branch pushes, revisit where this job caches"
    );
}

/// The release decision is answered by a PUBLISHED binary, not by a compile.
///
/// `release-version` reads one field out of `[workspace.package].version`,
/// compares it against the tag list, and on the overwhelming majority of merges
/// answers "no" and skips every downstream job. Compiling the CLI to do that
/// cost 7m54s of latency on every merge to `main` (run 32531667235) — the job
/// runs on a free runner, so the price is wall-clock rather than money, paid at
/// the very front of the release train where nothing else can start.
///
/// The published Linux archive is the same code, already built. Downloading it
/// keeps the rule in exactly one place — `cli/src/release_check.rs` — while
/// removing the compile.
///
/// **The bootstrap caveat is real and accepted.** The checker is release N-1's,
/// so a change to `ops release check` itself governs from the release AFTER the
/// one that lands it. That is tolerable because the binary carries the rule
/// while the run supplies the data — the manifest and the tags are both read
/// fresh — and because `ci.yml` runs the in-tree `ops release check` on every
/// pull request, so a change to the rule is proved on the branch that makes it.
#[test]
fn the_release_decision_runs_a_published_binary_rather_than_a_compile() {
    let workflow = deploy_workflow();
    let job = release_version_job(&workflow);

    assert!(
        job.contains("gh release download"),
        "`release-version` must install the published `navigator` binary rather than build one; \
         compiling the CLI to answer a yes/no question is ~8 minutes of latency on every merge"
    );
    assert!(
        job.contains("navigator ops release check --github-output"),
        "the downloaded binary must answer the question through the same `ops release check` \
         command, so the release rule lives in one place"
    );
}

/// `/releases/latest` must not be how the checker is found.
///
/// It excludes prereleases, and every release this repository cuts is one —
/// `YY.M.D-hotfix.N` carries a semver pre-release segment, and
/// `release_check::Outcome::prerelease` is what marks the GitHub Release as
/// such. The endpoint therefore answers 404 here. A checker resolved through it
/// would be missing on every run, silently falling back to the compile this
/// change exists to remove.
#[test]
fn the_checker_is_not_resolved_through_the_latest_release_endpoint() {
    let workflow = deploy_workflow();
    let job = release_version_job(&workflow);

    assert!(
        !job.contains("releases/latest"),
        "`/releases/latest` excludes prereleases and this repository publishes only \
         prereleases, so it answers 404 — enumerate releases and take the newest one that \
         carries a Linux archive instead"
    );
}

/// An unavailable binary falls back to the source of truth in this tree.
///
/// This is the shape that matters more than the speed. A release lost because
/// nothing published it is a failure this pipeline has already paid for once,
/// and a checker that could not be downloaded must never be allowed to answer
/// "not a release" by default. The fallback compiles, which is slow — and slow
/// is the correct price for a release that still happens.
#[test]
fn an_unavailable_checker_falls_back_to_building_from_source() {
    let workflow = deploy_workflow();
    let job = release_version_job(&workflow);

    assert!(
        job.contains("cargo run --locked -p cli --quiet -- ops release check --github-output"),
        "`release-version` must keep the in-tree fallback: a download that failed must compile \
         the checker, never answer the release question by default"
    );

    // The toolchain has to be installed on the fallback path and only there,
    // or the fallback cannot run at all.
    let parsed: serde_yaml::Value =
        serde_yaml::from_str(&workflow).expect("deploy.yml parses as YAML");
    let toolchain = parsed["jobs"]["release-version"]["steps"]
        .as_sequence()
        .expect("release-version declares steps")
        .iter()
        .find(|step| step["name"].as_str() == Some("install rust toolchain"))
        .expect("release-version must keep a toolchain step for the fallback");
    let guard = toolchain["if"].as_str().expect(
        "the toolchain step must be conditional; installing it always pays for a \
                 fallback that almost never fires",
    );
    assert!(
        guard.contains("steps.checker.outputs.ready != 'true'"),
        "the toolchain must install only when the published checker could not be had"
    );
}

/// No Actions cache in this job, deliberately.
///
/// It runs on `ubuntu-latest` with a different preinstalled Rust than the
/// Blacksmith merge gate, so it cannot read the gate's entry — it would have to
/// write its own into the shared 10 GB repository quota. That quota is the one
/// the release pipeline already starved once, evicting the Rust dependency cache
/// the PR gate uploads on every push. Downloading a built binary is the cheaper
/// answer and costs the quota nothing.
#[test]
fn the_release_decision_job_writes_no_actions_cache() {
    let workflow = deploy_workflow();
    let job = release_version_job(&workflow);

    for cache in ["actions/cache", "Swatinem/rust-cache", "type=gha"] {
        assert!(
            !job.contains(cache),
            "`release-version` must not use `{cache}`: it cannot read the Blacksmith gate's \
             entry and would write its own into the shared quota that starved that gate before"
        );
    }
}

/// The `release-version` job's CONFIGURATION, re-serialised from the parsed
/// YAML. Two properties matter: it is scoped to this one job, so an assertion
/// cannot accidentally read another job's steps; and YAML comments are dropped,
/// so the prose explaining what the job must not do is not itself searched.
/// Bash comments inside a `run:` block survive, which is correct — they are part
/// of the script that runs.
fn release_version_job(workflow: &str) -> String {
    let parsed: serde_yaml::Value =
        serde_yaml::from_str(workflow).expect("deploy.yml parses as YAML");
    let job = &parsed["jobs"]["release-version"];
    assert!(
        !job.is_null(),
        "deploy.yml must keep its `release-version` job"
    );
    serde_yaml::to_string(job).expect("the release-version job re-serialises")
}
