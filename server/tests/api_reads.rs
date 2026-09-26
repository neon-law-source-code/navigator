#![allow(clippy::doc_markdown)]
//! Integration tests for the #866 read clusters — the matter-centric GETs the
//! portal pages load:
//! `GET /app/api/projects`, `/app/api/projects/{id}`,
//! `/app/api/projects/{id}/{participants,notations}`, `/app/api/notations/{id}`,
//! `/app/api/playbooks`, `/app/api/playbooks/{id}`, `/app/api/contract-reviews/{id}`.
//!
//! The read functions are shared `store` reads; these tests focus on what the API
//! adds: `visible_projects` self-scoping, the by-id 404-on-out-of-scope gate,
//! and the tier split (matter reads are client-readable, firm tools are lawyer).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::session::SessionData;
use portal::{AppState, SessionStore};
use store::persons::Role;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use uuid::Uuid;

const KEY: &str = "api-reads-test-key";
/// Synthetic Firm-private coordinates the fixture matter carries so the
/// client-payload redaction has something to redact.
const PRIVATE_NOTION: &str = "https://notion.example/private-workup";
const INTERNAL_SLACK_ID: &str = "C0FIRMONLY1";

struct Fixture {
    app: axum::Router,
    project_id: Uuid,
    project_code: String,
    notation_id: Uuid,
    lawyer: String,
    client: String,
    outsider: String,
    /// An Owner with no participation row on the fixture matter.
    unassigned_owner: String,
    /// An Admin with no participation row on the fixture matter.
    unassigned_admin: String,
    /// An Admin who *is* seeded onto the matter, so the firm-side gate is
    /// proved to be about the participation row rather than about the tier.
    participating_admin: String,
    /// A Clerk: the supervised non-lawyer tier, which `LawyerSession` refuses.
    clerk: String,
}

fn bearer(person_id: Uuid, role: Role) -> String {
    let mut s = SessionData::fresh("api-reads-sub", role);
    s.person_id = Some(person_id);
    format!("Bearer {}", SessionStore::new(KEY).encode(&s))
}

/// The privileged and supervised tiers the participation gate has to answer
/// for. None of them is needed as a `Person` elsewhere in the fixture, so they
/// are seeded straight to the bearer their session presents.
struct TierBearers {
    unassigned_owner: String,
    unassigned_admin: String,
    participating_admin: String,
    clerk: String,
}

/// Seed an Owner and an Admin who hold no row on `project_id`, an Admin who
/// does, and a Clerk. The unassigned pair is the point: it is what proves the
/// privileged tiers reach matter *content* through the participation ledger
/// like everybody else (ENG-81) rather than through their tier.
async fn seed_tier_bearers(surreal: &store::surreal::SurrealDb, project_id: Uuid) -> TierBearers {
    let mut seeded = Vec::new();
    for (name, email, role, seated) in [
        ("Owner", "owner@example.com", Role::Owner, false),
        ("Admin", "admin@example.com", Role::Admin, false),
        (
            "Seated Admin",
            "seated-admin@example.com",
            Role::Admin,
            true,
        ),
        ("Clerk", "clerk@example.com", Role::Clerk, false),
    ] {
        let person = store::persons::create(
            surreal,
            &store::persons::NewPerson::with_role(name, email, role),
        )
        .await
        .unwrap();
        if seated {
            store::projects::add_participation(surreal, project_id, person.id, "lawyer")
                .await
                .unwrap();
        }
        seeded.push(bearer(person.id, role));
    }
    let mut seeded = seeded.into_iter();
    TierBearers {
        unassigned_owner: seeded.next().unwrap(),
        unassigned_admin: seeded.next().unwrap(),
        participating_admin: seeded.next().unwrap(),
        clerk: seeded.next().unwrap(),
    }
}

/// Seed a notation on the matter and a playbook for the firm, so the reads
/// under test return content rather than empty arrays. Returns the notation id.
async fn seed_matter_content(
    surreal: &store::surreal::SurrealDb,
    project_id: Uuid,
    entity_id: Uuid,
    respondent_id: Uuid,
) -> Uuid {
    let tmpl = store::templates::save_version(
        surreal,
        None,
        "test__read_walk",
        store::templates::Version {
            title: "Read walk".into(),
            respondent_type: "person".into(),
            asset_id: None,
            form_code: None,
            kind: None,
            source_commit_sha: None,
        },
    )
    .await
    .unwrap()
    .into_model();
    let notation_id = store::notations::create(
        surreal,
        &store::notations::NewNotation::new(tmpl.id, respondent_id, project_id, "BEGIN"),
    )
    .await
    .unwrap()
    .id;
    store::playbooks::create(
        surreal,
        &store::playbooks::NewPlaybook {
            entity_id,
            name: "Vendor MSA",
            positions: &[store::playbooks::Position {
                topic: "Liability".into(),
                preferred: "cap".into(),
                fallback: "2x".into(),
                walkaway: "uncapped".into(),
                severity: store::playbooks::SEVERITY_HIGH.into(),
            }],
        },
    )
    .await
    .unwrap();
    notation_id
}

async fn build_fixture() -> Fixture {
    let surreal = mem_surreal().await;
    let entity_id = store::test_support::seed_entity(&surreal).await;
    let project = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: format!("matter-{}", Uuid::now_v7()),
            name: "Matter".into(),
            status: "open".into(),
            entity_id,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    store::projects::set_private_notion_page_url(&surreal, project.id, PRIVATE_NOTION)
        .await
        .unwrap()
        .expect("the fixture matter records a private Notion page");
    store::projects::set_internal_slack_channel_id(&surreal, project.id, INTERNAL_SLACK_ID)
        .await
        .unwrap()
        .expect("the fixture matter records a firm-only Slack channel");
    let lawyer = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Lawyer", "lawyer@example.com", Role::Lawyer),
    )
    .await
    .unwrap();
    store::projects::add_participation(&surreal, project.id, lawyer.id, "lawyer")
        .await
        .unwrap();
    let client = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Client", "client@example.com", Role::Client),
    )
    .await
    .unwrap();
    store::projects::add_participation(&surreal, project.id, client.id, "client")
        .await
        .unwrap();
    let outsider = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Outsider", "outsider@example.com", Role::Lawyer),
    )
    .await
    .unwrap();
    let tiers = seed_tier_bearers(&surreal, project.id).await;

    let notation_id = seed_matter_content(&surreal, project.id, entity_id, client.id).await;

    let state = AppState {
        sessions: SessionStore::new(KEY),
        ..portal::test_support::app_state(surreal.clone()).await
    };
    Fixture {
        app: server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        project_id: project.id,
        project_code: project.code.clone(),
        notation_id,
        lawyer: bearer(lawyer.id, Role::Lawyer),
        client: bearer(client.id, Role::Client),
        outsider: bearer(outsider.id, Role::Lawyer),
        unassigned_owner: tiers.unassigned_owner,
        unassigned_admin: tiers.unassigned_admin,
        participating_admin: tiers.participating_admin,
        clerk: tiers.clerk,
    }
}

async fn get(fx: &Fixture, path: &str, auth: Option<&str>) -> axum::http::Response<Body> {
    let mut req = Request::builder().method("GET").uri(path);
    if let Some(auth) = auth {
        req = req.header("authorization", auth);
    }
    fx.app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

/// The caller's own view of the fixture matter, read through the list door
/// so the assertion covers the payload a portal actually receives.
async fn project_json(fx: &Fixture, auth: Option<&str>) -> serde_json::Value {
    let resp = get(fx, "/app/api/projects", auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .unwrap()
        .as_array()
        .unwrap()
        .first()
        .cloned()
        .expect("the caller sees the fixture matter")
}

async fn json_array_len(resp: axum::http::Response<Body>) -> usize {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .unwrap()
        .as_array()
        .unwrap()
        .len()
}

#[tokio::test]
async fn list_projects_is_scoped_to_the_caller() {
    let fx = build_fixture().await;

    let mine = get(&fx, "/app/api/projects", Some(&fx.lawyer)).await;
    assert_eq!(mine.status(), StatusCode::OK);
    assert_eq!(
        json_array_len(mine).await,
        1,
        "the participant sees their matter"
    );

    let theirs = get(&fx, "/app/api/projects", Some(&fx.outsider)).await;
    assert_eq!(theirs.status(), StatusCode::OK);
    assert_eq!(
        json_array_len(theirs).await,
        0,
        "a non-participant sees none"
    );

    assert_eq!(
        get(&fx, "/app/api/projects", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

/// The Firm-private integration coordinates are the reason
/// `/app/api/projects` serializes a narrower struct for a client caller.
/// Navigator stores an address only, so a leaked private Notion URL or a
/// private Slack channel id is a live handle to firm-only work product on a
/// matter the client is otherwise entitled to read — the row is in scope and
/// the coordinate is not. The paired assertion on the lawyer payload is what
/// keeps this a redaction rather than a column nobody reads.
#[tokio::test]
async fn a_client_project_payload_redacts_the_firm_private_coordinates() {
    let fx = build_fixture().await;

    let client_view = project_json(&fx, Some(&fx.client)).await;
    for field in [
        "private_notion_page_url",
        "internal_slack_channel_id",
        "internal_slack_channel_url",
        "firm_id",
        "repository_url",
        "drive_folder_id",
        "git_initialized_at",
        "forge_provisioned_at",
        "closed_at",
    ] {
        assert!(
            client_view.get(field).is_none(),
            "a client payload must not carry `{field}`: {client_view}"
        );
    }
    assert_eq!(
        client_view.get("code").and_then(serde_json::Value::as_str),
        Some(fx.project_code.as_str()),
        "the client still reads the matter itself"
    );

    let firm_view = project_json(&fx, Some(&fx.lawyer)).await;
    assert_eq!(
        firm_view
            .get("private_notion_page_url")
            .and_then(serde_json::Value::as_str),
        Some(PRIVATE_NOTION),
        "the firm tier still reads the private page: {firm_view}"
    );
    assert_eq!(
        firm_view
            .get("internal_slack_channel_id")
            .and_then(serde_json::Value::as_str),
        Some(INTERNAL_SLACK_ID),
        "the firm tier still reads the private channel id: {firm_view}"
    );
}

#[tokio::test]
async fn matter_reads_are_scoped() {
    let fx = build_fixture().await;
    let pid = fx.project_id;

    for path in [
        format!("/app/api/projects/{pid}"),
        format!("/app/api/projects/{pid}/participants"),
        format!("/app/api/projects/{pid}/notations"),
        format!("/app/api/notations/{}", fx.notation_id),
    ] {
        assert_eq!(
            get(&fx, &path, Some(&fx.lawyer)).await.status(),
            StatusCode::OK,
            "{path}"
        );
        assert_eq!(
            get(&fx, &path, Some(&fx.outsider)).await.status(),
            StatusCode::NOT_FOUND,
            "{path} out of scope"
        );
        assert_eq!(
            get(&fx, &path, None).await.status(),
            StatusCode::UNAUTHORIZED,
            "{path} anon"
        );
    }

    // A client on the matter reads it too.
    assert_eq!(
        get(&fx, &format!("/app/api/projects/{pid}"), Some(&fx.client))
            .await
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn notation_answers_are_lawyer_only_and_matter_scoped() {
    let fx = build_fixture().await;
    let path = format!("/app/api/notations/{}/answers", fx.notation_id);

    assert_eq!(
        get(&fx, &path, Some(&fx.lawyer)).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        get(&fx, &path, Some(&fx.client)).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(&fx, &path, Some(&fx.outsider)).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&fx, &path, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn notation_inventory_is_lawyer_only_and_matter_scoped() {
    let fx = build_fixture().await;
    let path = format!("/app/api/projects/{}/notation-inventory", fx.project_id);

    let lawyer_inventory = get(&fx, &path, Some(&fx.lawyer)).await;
    assert_eq!(lawyer_inventory.status(), StatusCode::OK);
    let body = lawyer_inventory
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    let inventory: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(inventory.as_array().unwrap().len(), 1);
    assert_eq!(inventory[0]["notation_id"], fx.notation_id.to_string());
    assert_eq!(inventory[0]["template_code"], "test__read_walk");
    assert_eq!(inventory[0]["respondent_email"], "client@example.com");

    assert_eq!(
        get(&fx, &path, Some(&fx.client)).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(&fx, &path, Some(&fx.outsider)).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&fx, &path, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

/// The notation inventory and the filed answers are matter *content*, so the
/// participation ledger scopes them for every tier — Owner and Admin included
/// (ENG-81). Route admission is where a privileged tier skips project-scoping;
/// it is not a key to a matter's work product. An unassigned Owner or Admin
/// therefore gets the same 404 an unassigned Lawyer gets, and the fact that
/// they could name the matter's UUID changes nothing.
#[tokio::test]
async fn private_notation_reads_require_participation_from_every_tier() {
    let fx = build_fixture().await;
    let inventory = format!("/app/api/projects/{}/notation-inventory", fx.project_id);
    let answers = format!("/app/api/notations/{}/answers", fx.notation_id);

    for path in [&inventory, &answers] {
        for (label, auth) in [
            ("unassigned owner", &fx.unassigned_owner),
            ("unassigned admin", &fx.unassigned_admin),
            ("unassigned lawyer", &fx.outsider),
        ] {
            assert_eq!(
                get(&fx, path, Some(auth)).await.status(),
                StatusCode::NOT_FOUND,
                "{label} must not read {path} without a participation row"
            );
        }

        assert_eq!(
            get(&fx, path, Some(&fx.participating_admin)).await.status(),
            StatusCode::OK,
            "an admin seated on the matter still reads {path}"
        );
    }
}

/// Clerk is the supervised non-lawyer tier, not a narrower Lawyer, so it is
/// refused at the extractor rather than scoped at the matter — 403, not 404.
#[tokio::test]
async fn private_notation_reads_refuse_the_clerk_tier() {
    let fx = build_fixture().await;

    for path in [
        format!("/app/api/projects/{}/notation-inventory", fx.project_id),
        format!("/app/api/notations/{}/answers", fx.notation_id),
    ] {
        assert_eq!(
            get(&fx, &path, Some(&fx.clerk)).await.status(),
            StatusCode::FORBIDDEN,
            "{path} is lawyer-tier, and a clerk is not lawyer tier"
        );
    }
}

#[tokio::test]
async fn playbook_reads_are_lawyer_only() {
    let fx = build_fixture().await;

    let list = get(&fx, "/app/api/playbooks", Some(&fx.lawyer)).await;
    assert_eq!(list.status(), StatusCode::OK);
    assert_eq!(json_array_len(list).await, 1);

    assert_eq!(
        get(&fx, "/app/api/playbooks", Some(&fx.client))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(&fx, "/app/api/playbooks", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn contract_review_read_is_gated() {
    let fx = build_fixture().await;
    // No review seeded; an unknown id is 404 for a lawyer, 403 for a client, 401
    // for anonymous — exercising the tier gate and the non-disclosing miss.
    let path = format!("/app/api/contract-reviews/{}", Uuid::now_v7());
    assert_eq!(
        get(&fx, &path, Some(&fx.lawyer)).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&fx, &path, Some(&fx.client)).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(&fx, &path, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
}
