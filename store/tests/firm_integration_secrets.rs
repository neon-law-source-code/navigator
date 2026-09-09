use cloud::{FakeKms, KmsContext, RuntimeKms};
use serde_json::Value;
use store::firm_capability::FirmCapability;
use store::firm_secrets::{
    metadata_for_firm, put, resolve_project_credential, revoke, IntegrationProvider,
    IntegrationSecretKind, SecretPutRequest, SecretStoreError,
};
use store::firms::{self, FirmMembership, NewFirm, NewPersonFirmRole};
use store::persons::{self, NewPerson, Role};
use store::projects::{self, NewProject};
use store::test_support::{mem_surreal, seed_entity};
use uuid::Uuid;

async fn person(db: &store::surreal::SurrealDb, label: &str, role: Role) -> persons::Person {
    persons::create(
        db,
        &NewPerson::with_role(
            format!("{label} Person"),
            format!("{label}-{}@example.com", Uuid::now_v7()),
            role,
        ),
    )
    .await
    .expect("synthetic person creates")
}

async fn firm_with_dri(
    db: &store::surreal::SurrealDb,
    label: &str,
) -> (firms::Firm, persons::Person) {
    let admin = person(db, label, Role::Admin).await;
    let firm = firms::create(
        db,
        &NewFirm {
            name: format!("{label} Firm"),
            status: "active".to_string(),
            entity_id: seed_entity(db).await,
            admin_dri_person_id: admin.id,
        },
    )
    .await
    .expect("synthetic firm creates");
    (firm, admin)
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn typed_secret_rotation_is_metadata_only_and_resolves_by_project_firm() {
    let db = mem_surreal().await;
    let (firm_a, dri_a) = firm_with_dri(&db, "alpha").await;
    let (firm_b, dri_b) = firm_with_dri(&db, "beta").await;
    let entity_a = seed_entity(&db).await;
    let entity_b = seed_entity(&db).await;
    let project_a = projects::create(
        &db,
        &NewProject {
            code: "alpha-project".to_string(),
            name: "Alpha Project".to_string(),
            entity_id: entity_a,
            firm_id: Some(firm_a.id),
            ..Default::default()
        },
    )
    .await
    .expect("synthetic project creates");
    let project_b = projects::create(
        &db,
        &NewProject {
            code: "beta-project".to_string(),
            name: "Beta Project".to_string(),
            entity_id: entity_b,
            firm_id: Some(firm_b.id),
            ..Default::default()
        },
    )
    .await
    .expect("synthetic project creates");
    let kms =
        FakeKms::new("projects/fixture/locations/global/keyRings/runtime/cryptoKeys/integration");

    let first = put(
        &db,
        SecretPutRequest {
            actor_role: Role::Admin,
            actor_person_id: Some(dri_a.id),
            firm_id: firm_a.id,
            provider: IntegrationProvider::Notion,
            kind: IntegrationSecretKind::NotionToken,
            value: "notion-token-one",
        },
        &kms,
    )
    .await
    .expect("Admin DRI writes the first version");
    assert_eq!(first.version, 1);
    assert_eq!(first.provider, IntegrationProvider::Notion);
    assert_eq!(first.kind, IntegrationSecretKind::NotionToken);

    let second = put(
        &db,
        SecretPutRequest {
            actor_role: Role::Admin,
            actor_person_id: Some(dri_a.id),
            firm_id: firm_a.id,
            provider: IntegrationProvider::Notion,
            kind: IntegrationSecretKind::NotionToken,
            value: "notion-token-two",
        },
        &kms,
    )
    .await
    .expect("rotation writes a new version");
    assert_eq!(second.version, 2);

    let metadata = metadata_for_firm(
        &db,
        Role::Owner,
        None,
        firm_a.id,
        IntegrationProvider::Notion,
        IntegrationSecretKind::NotionToken,
    )
    .await
    .expect("Owner can inspect metadata");
    let json = serde_json::to_value(&metadata).expect("metadata serializes");
    assert_eq!(json["version"], 2);
    assert!(json.get("value").is_none());
    assert!(!format!("{json}").contains("notion-token"));

    let resolved = resolve_project_credential(
        &db,
        project_a.id,
        IntegrationProvider::Notion,
        IntegrationSecretKind::NotionToken,
        &kms,
    )
    .await
    .expect("Project resolves its own Firm credential");
    assert_eq!(resolved.expose_for_provider_call(), "notion-token-two");

    let cross_firm = resolve_project_credential(
        &db,
        project_b.id,
        IntegrationProvider::Notion,
        IntegrationSecretKind::NotionToken,
        &kms,
    )
    .await;
    assert!(matches!(
        cross_firm,
        Err(SecretStoreError::NotConfigured { .. })
    ));

    let mut raw = db
        .query("SELECT ciphertext, wrapped_dek, kms_context FROM firm_integration_secret")
        .await
        .expect("raw secret query succeeds")
        .check()
        .expect("raw secret query checks");
    let rows: Vec<Value> = raw.take(0).expect("raw rows deserialize");
    let rendered = serde_json::to_string(&rows).expect("rows serialize");
    assert!(!rendered.contains("notion-token-one"));
    assert!(!rendered.contains("notion-token-two"));

    revoke(
        &db,
        Role::Admin,
        Some(dri_a.id),
        firm_a.id,
        IntegrationProvider::Notion,
        IntegrationSecretKind::NotionToken,
    )
    .await
    .expect("Admin DRI revokes");
    assert!(matches!(
        resolve_project_credential(
            &db,
            project_a.id,
            IntegrationProvider::Notion,
            IntegrationSecretKind::NotionToken,
            &kms,
        )
        .await,
        Err(SecretStoreError::NotConfigured { .. })
    ));

    let _ = dri_b;
}

#[tokio::test]
async fn only_the_firm_admin_dri_writes_and_owner_never_resolves_plaintext() {
    let db = mem_surreal().await;
    let (firm, dri) = firm_with_dri(&db, "alpha").await;
    let other_admin = person(&db, "other-admin", Role::Admin).await;
    firms::add_membership(
        &db,
        &NewPersonFirmRole {
            person_id: other_admin.id,
            firm_id: firm.id,
            membership: FirmMembership::Admin,
            is_dri: false,
        },
    )
    .await
    .expect("second Admin membership creates");
    let kms =
        FakeKms::new("projects/fixture/locations/global/keyRings/runtime/cryptoKeys/integration");

    let refused = put(
        &db,
        SecretPutRequest {
            actor_role: Role::Admin,
            actor_person_id: Some(other_admin.id),
            firm_id: firm.id,
            provider: IntegrationProvider::Slack,
            kind: IntegrationSecretKind::SlackBotToken,
            value: "secret-value",
        },
        &kms,
    )
    .await;
    assert!(matches!(refused, Err(SecretStoreError::NotAuthorized)));

    let owner_refused = put(
        &db,
        SecretPutRequest {
            actor_role: Role::Owner,
            actor_person_id: None,
            firm_id: firm.id,
            provider: IntegrationProvider::Slack,
            kind: IntegrationSecretKind::SlackBotToken,
            value: "owner-secret",
        },
        &kms,
    )
    .await;
    assert!(matches!(
        owner_refused,
        Err(SecretStoreError::NotAuthorized)
    ));

    put(
        &db,
        SecretPutRequest {
            actor_role: Role::Admin,
            actor_person_id: Some(dri.id),
            firm_id: firm.id,
            provider: IntegrationProvider::Slack,
            kind: IntegrationSecretKind::SlackBotToken,
            value: "secret-value",
        },
        &kms,
    )
    .await
    .expect("DRI writes");
    assert!(FirmCapability::ViewIntegrationSecretMetadata
        .resolve(&db, Role::Owner, None, firm.id)
        .await
        .expect("Owner metadata decision")
        .is_allowed());
}

#[tokio::test]
async fn kms_context_mismatch_and_unavailability_fail_closed() {
    let kms =
        FakeKms::new("projects/fixture/locations/global/keyRings/runtime/cryptoKeys/integration");
    let one = KmsContext::new(Uuid::now_v7(), "notion", "notion_token");
    let two = KmsContext::new(Uuid::now_v7(), "notion", "notion_token");
    let wrapped = kms
        .wrap_data_key(b"data-key", &one)
        .await
        .expect("fake KMS wraps");
    assert!(kms.unwrap_data_key(&wrapped, &two).await.is_err());
    kms.set_available(false);
    assert!(kms.wrap_data_key(b"data-key", &one).await.is_err());
}
