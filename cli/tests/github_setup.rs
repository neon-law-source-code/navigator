//! Binary wiring for `navigator ops github setup <repository> --dry-run`.

use assert_cmd::Command;
use predicates::str::contains;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn dry_run_prints_plan_without_writes() {
    let server = MockServer::start().await;
    mount_reads(&server).await;
    Command::cargo_bin("navigator")
        .unwrap()
        .args([
            "ops",
            "github",
            "setup",
            "neon-law-source-code/navigator",
            "--dry-run",
        ])
        .env("GITHUB_TOKEN", "test-token")
        .env_remove("GITHUB_REPOSITORY")
        .env("NAVIGATOR_GITHUB_API_BASE", server.uri())
        .env_remove("NAVIGATOR_GITHUB_APP_ID")
        .assert()
        .success()
        .stderr(contains("would update ruleset production"))
        // The repository below has never had the tag gate applied, so the plan
        // creates it rather than skipping the ruleset it could not find.
        .stderr(contains("would create ruleset release-tags"))
        // Nor the review gate, which is the ruleset that makes a code owner's
        // approval a precondition of merging.
        .stderr(contains("would create ruleset production-review"))
        .stderr(contains("would create label triage"));
    let requests = server.received_requests().await.unwrap();
    assert!(requests
        .iter()
        .all(|request| request.method == wiremock::http::Method::GET));
}

async fn mount_reads(server: &MockServer) {
    // The required-check rule is bound to the Actions App id, and the host is
    // the only authority on what that id is, so the reconcile reads it before
    // it plans. Without this the dry run stops on a 404 rather than printing a
    // plan — deliberately, because a guessed id writes a gate that does not
    // gate.
    Mock::given(method("GET"))
        .and(path("/apps/github-actions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": 4242})))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/neon-law-source-code/navigator"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "allow_squash_merge": true,
            "allow_merge_commit": false,
            "allow_rebase_merge": false,
            "allow_auto_merge": true,
            "delete_branch_on_merge": true,
            "squash_merge_commit_title": "PR_TITLE",
            "squash_merge_commit_message": "PR_BODY",
            "has_issues": false,
            "has_projects": false,
            "has_wiki": false,
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/repos/neon-law-source-code/navigator/contents/.github/CODEOWNERS",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_string("* @owner\n"))
        .mount(server)
        .await;
    // Every owner the file names is resolved against the host before the review
    // gate is written, because GitHub silently ignores one that does not exist.
    Mock::given(method("GET"))
        .and(path("/users/owner"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(server)
        .await;
    // Existing is not owning: GitHub honors a code owner only where that owner
    // can write, so the reconcile asks this too.
    Mock::given(method("GET"))
        .and(path(
            "/repos/neon-law-source-code/navigator/collaborators/owner/permission",
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"permission": "admin"})),
        )
        .mount(server)
        .await;
    // The required `ci` context is only bound to a repository whose workflow
    // actually defines that job.
    Mock::given(method("GET"))
        .and(path(
            "/repos/neon-law-source-code/navigator/contents/.github/workflows/ci.yml",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "jobs:\n  rust:\n    name: cargo test (workspace)\n  ci:\n    name: ci\n",
        ))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/neon-law-source-code/navigator/rulesets"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!([{"id":7,"name":"production"}])),
        )
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/neon-law-source-code/navigator/rulesets/7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "name": "production",
            "target": "branch",
            "enforcement": "active",
            "bypass_actors": [],
            "conditions": {"ref_name": {"exclude": [], "include": ["~DEFAULT_BRANCH"]}},
            "rules": []
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/neon-law-source-code/navigator/labels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(server)
        .await;
}
