//! Compatibility gate for the accepted Regorus policy-engine replacement.
//!
//! The portal-owned policy and its checked-in Rego rules share one behavioral
//! oracle under the deployed Regorus engine.

use std::path::{Path, PathBuf};

use regorus::{Engine, Value};
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cli crate has a parent (the workspace root)")
        .to_path_buf()
}

fn test_rule_names(tests: &str) -> Vec<String> {
    tests
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            line.strip_prefix("test_")
                .and_then(|rest| rest.split_whitespace().next())
                .map(|name| format!("test_{name}"))
        })
        .collect()
}

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn regorus_matches_every_checked_in_policy_decision() {
    assert_send_sync::<regorus::CompiledPolicy>();

    let root = repo_root();
    let policy = std::fs::read_to_string(root.join("portal/policy/navigator.rego"))
        .expect("read portal-owned policy");
    let tests = include_str!("../../portal/policy/navigator_test.rego");
    let test_names = test_rule_names(tests);

    // 178 + 9 for `/app/docs`: four admitted tiers at the hub, two of them
    // again one document deeper, and the three denials that matter — a client
    // at the hub, a client at a document, and an anonymous request.
    //
    // + 4 for `/app/team`, the firm team home: each firm tier is admitted, and
    // the client and anonymous requests are denied.
    // + 3 for a Project's client portal at `/app/projects/{code}/portal`: a
    // client and a firm-side caller admitted, and an anonymous request denied.
    // Admission only — the portal is *participation*-scoped, and the ledger this
    // policy cannot read is what the handler consults, so a client who is not on
    // the matter is admitted here and answered 404 there. These three exist
    // because the portal is the first surface four path elements deep, and the
    // rule that admits it matches on the first two: a policy that silently
    // denied that depth would make the route unreachable in production while
    // every passthrough-policy test in the workspace still passed.
    // + 4 for the Owner/Admin matter directory at `/app/admin/projects`
    // (ENG-221): both admin tiers admitted, and Lawyer, Clerk, and
    // Client each denied. The path carries no rule of its own — it rides the
    // Owner/Admin route bypass, exactly as `/app/admin` does — so these
    // assertions are the only thing holding the deny-by-omission in place. The
    // Lawyer denial is the load-bearing one: a rule copied from `/app/lawyer`
    // would hand every lawyer the firm's whole matter list.
    //
    // The base is 178 rather than 183 because ENG-144 removed the reprice
    // command: `POST /app/api/projects/{id}/price-events` and its five decisions
    // (lawyer and admin admitted; client, clerk, and anonymous denied) went
    // with the price journal the route appended to.
    //
    // + 33 for consolidating the API under the private `/app/api` prefix, in
    // three groups.
    //
    // **Retired paths (3).** The two former top-level doc paths (`/api-docs`,
    // `/openapi.json`) must not keep an exemption now that nothing mounts
    // there, and the A2A agent card left the anonymous allowlist and must not
    // be re-opened by a rule here.
    //
    // **The documentation gate (13).** ENG-83's audience, decided by this
    // policy rather than by routing: the four operating tiers admitted at each
    // of the two documentation paths, plus `client` and anonymous denied at
    // each, plus the pair asserting a Clerk reads the reference plane but not
    // the directory it describes.
    //
    // **Per-resource reads (17).** The blanket any-authenticated
    // `GET /app/api/*` grant is gone, so each read is now named with its tier:
    // the CRM directory and the reference vocabularies at lawyer, raw Template
    // markdown deliberately wide, and — the property the blanket grant made
    // impossible — an unnamed read denied, so adding a GET endpoint fails
    // closed. Two decisions here *replace* rather than add: the former
    // `test_authenticated_get_api_allowed` and `test_clerk_get_api_allowed`
    // asserted a client and a clerk reaching the people directory, which is
    // exactly the behaviour removed.
    //
    // − 7 for making workshop reads public while keeping the certificate POST
    // policy-gated: the ten old workshop/presentation spillover cases became
    // three certificate-claim matrix cases.
    //
    // − 5 for retiring the `/portal` landing: the client, clerk, owner, admin,
    // and anonymous decisions on the exact `["portal"]` path went with the
    // route. The two functional `/portal/*` surfaces moved rather than
    // retired — `/portal/forms/*` → `/app/forms/*` and
    // `/portal/notations/.../documents` → `/app/notations/.../documents` — so
    // those decisions kept their count, only their path changed. (Base is now
    // 233.)
    //
    // + 125 for the API-parity doors, each proved by its five-case authorization
    // matrix (client-writable and admin-only doors vary which tier is admitted):
    //   + 5   DRI designate/clear (PUT/DELETE .../participants/{role_id}/dri)
    //   + 5   close matter (POST .../projects/{id}/close)
    //   + 10  notation request-changes + reask
    //   + 5   approve-plan (client-writable)
    //   + 5   conversation message (client-writable)
    //   + 10  expunge authorize (admin-only) + deny (lawyer)
    //   + 10  playbook create + update
    //   + 20  contract-review workbench (findings/summary/approve/reject)
    //   + 15  document upload + notation transcript coverage + estate transcript
    //   + 40  the #866 GET read clusters (five matter reads authenticated +
    //         handler-scoped; three firm-tool reads lawyer-tier)
    //   + 20  the remaining #866 GET reads: matter documents + conversation
    //         (authenticated, client-visible-filtered in the handler) and the
    //         expunge queue + a notation's review documents (lawyer-tier)
    // 233 + 134 = 367.
    //
    // + 6 for the admin-only reconciliation read (GET
    //   /app/api/project-repositories): the five-case tier matrix, plus one
    //   case pinning *why* it carries its own noun — the `projects` GET rule
    //   admits any authenticated caller up to five segments, so the same report
    //   nested there would be policy-reachable by a client.
    // 367 + 6 = 373.
    //
    // + 6 for the admin-only surfaces reconcile (POST
    //   /app/api/project-surfaces/{id}): the five-case tier matrix, plus one
    //   noun-isolation case matching project-repositories.
    // 373 + 6 = 379.
    //
    // + 4 for firm-administration listings under `/app/admin`: Lawyer reaches
    //   the named reference resources, and is denied Person CRUD, analytics,
    //   Clerk, and client on those same paths.
    // 379 + 4 = 383.
    //
    // + 2 for the Harvard-outline narration stage at `/lawyer/outline`: Lawyer
    //   is admitted by the `/lawyer/*` prefix, Clerk is denied.
    // 383 + 2 = 385.
    assert_eq!(
        test_names.len(),
        385,
        "the policy decision inventory changed; review every new or removed rule"
    );

    let mut engine = Engine::new();
    engine
        .add_policy("navigator.rego".to_owned(), policy)
        .expect("Regorus parses the deployed policy");
    engine
        .add_policy("navigator_test.rego".to_owned(), tests.to_owned())
        .expect("Regorus parses the policy tests");

    let mut failures = Vec::new();
    for name in test_names {
        engine.set_input(Value::Undefined);
        let result = engine
            .eval_rule(format!("data.navigator.authz_test.{name}"))
            .unwrap_or_else(|error| panic!("Regorus could not evaluate {name}: {error}"));
        if result != Value::from(true) {
            failures.push(format!("{name} returned {result:?}"));
        }
    }

    assert!(
        failures.is_empty(),
        "Regorus policy parity failed:\n{}",
        failures.join("\n")
    );
}
