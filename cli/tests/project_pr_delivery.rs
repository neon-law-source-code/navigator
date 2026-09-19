//! End-to-end coverage for the Rust-owned Project pull-request delivery lane.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};

use assert_cmd::Command;
use predicates::{prelude::PredicateBooleanExt as _, str};
use serde_json::{json, Value};
use tempfile::TempDir;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const BRANCH: &str = "dev/project-delivery";

struct Fixture {
    root: TempDir,
    repository: PathBuf,
    body: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().expect("tempdir");
        let repository = root.path().join("example-project");
        fs::create_dir(&repository).expect("repository directory");
        Command::cargo_bin("navigator")
            .expect("navigator")
            .args([
                "project",
                "repository",
                "scaffold",
                "example-project",
                "--dir",
            ])
            .arg(&repository)
            .args([
                "--action-version",
                "26.9.19",
                "--host",
                "staging.neonlaw.com",
            ])
            .assert()
            .success();

        git(&repository, &["init", "--initial-branch=main"]);
        git(&repository, &["add", "-A"]);
        git(
            &repository,
            &[
                "-c",
                "user.name=Dev",
                "-c",
                "user.email=dev@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "chore: scaffold",
            ],
        );

        let remote = root.path().join("remote.git");
        git(
            root.path(),
            &[
                "init",
                "--bare",
                "--initial-branch=main",
                remote.to_str().unwrap(),
            ],
        );
        git(
            &repository,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(&repository, &["push", "--set-upstream", "origin", "main"]);

        let readme = repository.join("README.md");
        let mut contents = fs::read_to_string(&readme).expect("README");
        contents.push_str("\nDelivered through Navigator.\n");
        fs::write(&readme, contents).expect("write README");
        git(&repository, &["add", "README.md"]);
        let tree = git_output(&repository, &["write-tree"]);
        let parent = git_output(&repository, &["rev-parse", "HEAD"]);
        let signed = format!(
            "tree {tree}\nparent {parent}\nauthor Dev <dev@example.com> 1 +0000\ncommitter Dev <dev@example.com> 1 +0000\ngpgsig -----BEGIN SSH SIGNATURE-----\n U1NIU0lHAAAAAQ==\n -----END SSH SIGNATURE-----\n\nfeat: deliver change\n"
        );
        let sha = hash_commit(&repository, &signed);
        git(
            &repository,
            &["update-ref", &format!("refs/heads/{BRANCH}"), &sha],
        );
        git(&repository, &["reset", "--hard"]);
        git(&repository, &["switch", BRANCH]);

        let body = root.path().join("pr-body.md");
        fs::write(&body, "A synthetic Project repository change.\n").expect("body");
        Self {
            root,
            repository,
            body,
        }
    }

    fn command(&self, forge: &MockServer) -> Command {
        let mut command = Command::cargo_bin("navigator").expect("navigator");
        let coverage_profile = std::env::var_os("LLVM_PROFILE_FILE")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| self.root.path().join("navigator-%p-%m.profraw"));
        command
            .env_remove("NAVIGATOR_GITHUB_TOKEN")
            // GitHub Actions names Navigator itself here. Delivery must gate
            // the explicit Project coordinate instead of inheriting the host
            // workflow's repository identity.
            .env("GITHUB_REPOSITORY", "neon-law-source-code/navigator")
            // A coverage-instrumented child without an inherited absolute
            // profile path writes into its current directory. Keep that test
            // artifact outside the Project checkout that delivery must gate.
            .env("LLVM_PROFILE_FILE", coverage_profile)
            .env("GITHUB_TOKEN", "test-token")
            .env("NAVIGATOR_GITHUB_API_BASE", forge.uri())
            .env(
                "NAVIGATOR_GITHUB_GRAPHQL_API",
                format!("{}/graphql", forge.uri()),
            )
            .args(["project", "repository", "deliver", "--dir"])
            .arg(&self.repository)
            .args([
                "--repository",
                "example/example-project",
                "--branch",
                BRANCH,
                "--title",
                "feat: deliver Project change",
                "--body-file",
            ])
            .arg(&self.body)
            .args(["--timeout-seconds", "0", "--poll-seconds", "0"]);
        command
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn delivery_opens_arms_reads_back_and_reaches_merged() {
    let fixture = Fixture::new();
    let forge = MockServer::start().await;
    mount_forge(
        &forge,
        false,
        status(
            true,
            true,
            "CLEAN",
            None,
            &[check("ci / ci", "SUCCESS"), check("publish", "FAILURE")],
        ),
    )
    .await;

    fixture
        .command(&forge)
        .assert()
        .success()
        .stdout(str::contains("merged"))
        .stdout(str::contains(
            "https://forge.example/example-project/pull/7",
        ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_successful_arm_that_reads_back_null_stops_unarmed_on_an_adopted_pr() {
    let fixture = Fixture::new();
    let forge = MockServer::start().await;
    mount_forge(&forge, true, status(false, false, "CLEAN", None, &[])).await;

    fixture
        .command(&forge)
        .assert()
        .failure()
        .code(1)
        .stderr(str::contains("reported successful but is not armed"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_required_check_is_named_but_an_optional_failure_is_not() {
    let fixture = Fixture::new();
    let forge = MockServer::start().await;
    mount_forge(
        &forge,
        false,
        status(
            false,
            true,
            "UNSTABLE",
            None,
            &[check("ci / ci", "FAILURE"), check("publish", "FAILURE")],
        ),
    )
    .await;

    fixture
        .command(&forge)
        .assert()
        .failure()
        .code(1)
        .stderr(str::contains("required check(s) failed"))
        .stderr(str::contains("ci / ci"))
        .stderr(str::contains("publish").not());
}

#[tokio::test(flavor = "multi_thread")]
async fn required_review_is_a_distinct_stop() {
    let fixture = Fixture::new();
    let forge = MockServer::start().await;
    mount_forge(
        &forge,
        false,
        status(false, true, "BLOCKED", Some("REVIEW_REQUIRED"), &[]),
    )
    .await;

    fixture
        .command(&forge)
        .assert()
        .failure()
        .code(1)
        .stderr(str::contains("required review holds"));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_outdated_branch_is_a_distinct_stop() {
    let fixture = Fixture::new();
    let forge = MockServer::start().await;
    mount_forge(&forge, false, status(false, true, "BEHIND", None, &[])).await;

    fixture
        .command(&forge)
        .assert()
        .failure()
        .code(1)
        .stderr(str::contains("branch is behind `main`"));
}

#[tokio::test(flavor = "multi_thread")]
async fn pending_required_checks_end_at_the_bound() {
    let fixture = Fixture::new();
    let forge = MockServer::start().await;
    mount_forge(&forge, false, status(false, true, "UNSTABLE", None, &[])).await;

    fixture
        .command(&forge)
        .assert()
        .failure()
        .code(1)
        .stderr(str::contains("delivery timed out"))
        .stderr(str::contains("ci / ci"));
}

async fn mount_forge(server: &MockServer, adopted: bool, snapshot: Value) {
    let pull = json!({
        "number": 7,
        "node_id": "PR_node",
        "html_url": "https://forge.example/example-project/pull/7"
    });
    Mock::given(method("GET"))
        .and(path("/repos/example/example-project/pulls"))
        .respond_with(ResponseTemplate::new(200).set_body_json(if adopted {
            json!([pull.clone()])
        } else {
            json!([])
        }))
        .mount(server)
        .await;
    if !adopted {
        Mock::given(method("POST"))
            .and(path("/repos/example/example-project/pulls"))
            .respond_with(ResponseTemplate::new(201).set_body_json(pull))
            .mount(server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/repos/example/example-project/rules/branches/main"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "type": "required_status_checks",
            "parameters": {
                "required_status_checks": [{ "context": "ci / ci" }]
            }
        }])))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_partial_json(
            json!({ "operationName": "EnableAutoMerge" }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "enablePullRequestAutoMerge": { "pullRequest": { "id": "PR_node" } } }
        })))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_partial_json(
            json!({ "operationName": "PullRequestStatus" }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(snapshot))
        .mount(server)
        .await;
}

fn status(
    merged: bool,
    armed: bool,
    merge_state: &str,
    review_decision: Option<&str>,
    checks: &[Value],
) -> Value {
    json!({
        "data": {
            "repository": {
                "pullRequest": {
                    "merged": merged,
                    "autoMergeRequest": armed.then(|| json!({ "enabledAt": "2026-09-19T00:00:00Z" })),
                    "mergeStateStatus": merge_state,
                    "reviewDecision": review_decision,
                    "commits": {
                        "nodes": [{
                            "commit": {
                                "statusCheckRollup": {
                                    "contexts": { "nodes": checks }
                                }
                            }
                        }]
                    }
                }
            }
        }
    })
}

fn check(name: &str, conclusion: &str) -> Value {
    json!({
        "__typename": "CheckRun",
        "name": name,
        "status": "COMPLETED",
        "conclusion": conclusion
    })
}

fn hash_commit(repository: &Path, object: &str) -> String {
    let mut child = ProcessCommand::new("git")
        .args(["-C"])
        .arg(repository)
        .args(["hash-object", "-t", "commit", "-w", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hash-object");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(object.as_bytes())
        .expect("write commit");
    let output = child.wait_with_output().expect("wait hash-object");
    assert!(
        output.status.success(),
        "hash-object failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf8")
        .trim()
        .to_string()
}

fn git(repository: &Path, args: &[&str]) {
    let output = ProcessCommand::new("git")
        .args(["-C"])
        .arg(repository)
        .args(args)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output(repository: &Path, args: &[&str]) -> String {
    let output = ProcessCommand::new("git")
        .args(["-C"])
        .arg(repository)
        .args(args)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf8")
        .trim()
        .to_string()
}
