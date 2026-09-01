//! The bot's private-channel address is persisted on the authoritative Project row.

use store::projects::{create, set_internal_slack_channel_id, NewProject, ProjectCommandError};
use store::test_support::{mem_surreal, seed_entity};

#[tokio::test]
async fn a_slack_channel_id_is_written_for_the_project() {
    let surreal = mem_surreal().await;
    let project = create(
        &surreal,
        &NewProject {
            code: "slack-channel".to_string(),
            name: "Slack channel".to_string(),
            status: "open".to_string(),
            entity_id: seed_entity(&surreal).await,
            ..Default::default()
        },
    )
    .await
    .expect("create Project");

    let updated = set_internal_slack_channel_id(&surreal, project.id, "C123456")
        .await
        .expect("set Slack channel")
        .expect("Project exists");
    assert_eq!(
        updated.internal_slack_channel_id.as_deref(),
        Some("C123456")
    );
}

#[tokio::test]
async fn an_invalid_slack_channel_id_is_refused() {
    let surreal = mem_surreal().await;
    let project = create(
        &surreal,
        &NewProject {
            code: "slack-invalid".to_string(),
            name: "Slack invalid".to_string(),
            status: "open".to_string(),
            entity_id: seed_entity(&surreal).await,
            ..Default::default()
        },
    )
    .await
    .expect("create Project");

    assert!(matches!(
        set_internal_slack_channel_id(&surreal, project.id, "C-123").await,
        Err(ProjectCommandError::Invalid(_))
    ));
}
