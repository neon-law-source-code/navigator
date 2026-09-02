//! `update_project` is always a patch: an absent field is never written.
//!
//! The rule these tests pin is the one that keeps a partial update from
//! destroying data the caller never mentioned. A client that reads a matter,
//! edits one field, and sends its own narrow body must not be able to erase the
//! columns it does not know about — so absence means *leave alone*, and
//! clearing is always an explicit empty string.
//!
//! `null` and absent are deliberately the same thing. `serde` cannot tell them
//! apart on a plain `Option`, and collapsing them toward "leave alone" is the
//! safe direction: the other choice fails by wiping a column silently.

use store::projects::{
    create, update_project, NewProject, ProjectCommandError, UpdateProjectCommand,
};
use store::test_support::{mem_surreal, seed_entity};
use uuid::Uuid;

const A_REPOSITORY: &str = "https://forge.example/an-organization/a-matter";
const A_SLACK_CHANNEL: &str = "https://slack.example/archives/C0FIXTURE";

async fn matter(surreal: &store::surreal::SurrealDb) -> store::projects::Project {
    let code = format!("patch-{}", Uuid::now_v7().simple());
    let created = create(
        surreal,
        &NewProject {
            code: code.clone(),
            name: "Original Name".to_string(),
            status: "open".to_string(),
            entity_id: seed_entity(surreal).await,
            ..Default::default()
        },
    )
    .await
    .expect("create matter");

    // Give every optional column a value, so a later patch that wrongly wrote
    // an absent field would be visible rather than indistinguishable from a
    // column that was already empty.
    update_project(
        surreal,
        created.id,
        &UpdateProjectCommand {
            name: Some("Original Name".into()),
            description: Some("Original description".into()),
            repository_url: Some(A_REPOSITORY.into()),
            internal_slack_channel_url: Some(A_SLACK_CHANNEL.into()),
            ..Default::default()
        },
    )
    .await
    .expect("seed the optional columns")
}

/// The central rule. One field is sent; every other column survives.
#[tokio::test]
async fn a_patch_touching_one_field_leaves_every_other_column_alone() {
    let surreal = mem_surreal().await;
    let before = matter(&surreal).await;

    let after = update_project(
        &surreal,
        before.id,
        &UpdateProjectCommand {
            description: Some("Edited description".into()),
            ..Default::default()
        },
    )
    .await
    .expect("a one-field patch applies");

    assert_eq!(after.description.as_deref(), Some("Edited description"));
    assert_eq!(
        after.name, before.name,
        "an absent name must not be written"
    );
    assert_eq!(
        after.repository_url, before.repository_url,
        "an absent repository_url must not be cleared"
    );
    assert_eq!(
        after.internal_slack_channel_url, before.internal_slack_channel_url,
        "an absent Slack channel must not be cleared"
    );
}

/// The regression this fixes. `name` used to be a required `String`, so a JSON
/// body carrying only `repository_url` could not deserialize at all — a caller
/// had to resend the name to change anything else, which is the opposite of
/// "specify the fields you want to update".
#[tokio::test]
async fn a_json_body_naming_one_field_deserializes_and_applies() {
    let surreal = mem_surreal().await;
    let before = matter(&surreal).await;

    let body = serde_json::json!({
        "repository_url": "https://forge.example/an-organization/moved"
    });
    let command: UpdateProjectCommand =
        serde_json::from_value(body).expect("a one-field JSON patch deserializes");

    let after = update_project(&surreal, before.id, &command)
        .await
        .expect("it applies");

    assert_eq!(
        after.repository_url.as_deref(),
        Some("https://forge.example/an-organization/moved")
    );
    assert_eq!(
        after.name, before.name,
        "the name is untouched by a body that never mentioned it"
    );
}

/// `null` reads as absent, not as a clear. The failure mode of the other
/// choice is a caller wiping a column it did not intend to touch, so the
/// collapse goes toward leaving it alone.
#[tokio::test]
async fn an_explicit_null_leaves_the_column_alone() {
    let surreal = mem_surreal().await;
    let before = matter(&surreal).await;

    let command: UpdateProjectCommand = serde_json::from_value(serde_json::json!({
        "description": "Edited",
        "repository_url": null,
    }))
    .expect("null deserializes");

    let after = update_project(&surreal, before.id, &command)
        .await
        .expect("it applies");

    assert_eq!(
        after.repository_url, before.repository_url,
        "an explicit null must read as absent, never as a clear"
    );
}

/// Clearing is explicit, and it is the empty string — the same value an HTML
/// form posts for a text input a person emptied, so the form caller and the
/// JSON caller converge on one rule.
#[tokio::test]
async fn an_empty_string_is_how_a_column_is_cleared() {
    let surreal = mem_surreal().await;
    let before = matter(&surreal).await;
    assert!(before.repository_url.is_some(), "the fixture set one");

    let after = update_project(
        &surreal,
        before.id,
        &UpdateProjectCommand {
            repository_url: Some(String::new()),
            ..Default::default()
        },
    )
    .await
    .expect("an explicit clear applies");

    assert_eq!(after.repository_url, None);
    assert_eq!(
        after.description, before.description,
        "clearing one column does not disturb another"
    );
}

/// The one field that refuses to be blanked. A matter with no name is not a
/// state a patch may produce, so `""` is an error rather than a clear —
/// while omitting it is still just "leave it alone".
#[tokio::test]
async fn a_blank_name_is_refused_but_an_absent_one_is_fine() {
    let surreal = mem_surreal().await;
    let before = matter(&surreal).await;

    let blank = update_project(
        &surreal,
        before.id,
        &UpdateProjectCommand {
            name: Some("   ".into()),
            ..Default::default()
        },
    )
    .await;
    assert!(
        matches!(blank, Err(ProjectCommandError::Invalid(_))),
        "a blank name is refused, not applied: {blank:?}"
    );

    let absent = update_project(&surreal, before.id, &UpdateProjectCommand::default())
        .await
        .expect("an empty patch is a no-op, not an error");
    assert_eq!(absent.name, before.name);
}

/// Lifecycle status is a supported PATCH field, but its write must use the
/// transition command so `closed_at` follows the status invariant.
#[tokio::test]
async fn a_status_patch_transitions_the_matter_and_stamps_closed_at() {
    let surreal = mem_surreal().await;
    let before = matter(&surreal).await;
    let command: UpdateProjectCommand = serde_json::from_value(serde_json::json!({
        "status": "closed"
    }))
    .expect("status is a supported patch field");

    let after = update_project(&surreal, before.id, &command)
        .await
        .expect("status patch applies");

    assert_eq!(after.status, "closed");
    assert!(after.closed_at.is_some(), "closing must stamp closed_at");
}

/// The close date is derived by the lifecycle transition and cannot be
/// supplied independently by a PATCH caller.
#[test]
fn closed_at_is_not_a_settable_patch_field() {
    let error = serde_json::from_value::<UpdateProjectCommand>(serde_json::json!({
        "closed_at": "2026-08-15T00:00:00Z"
    }))
    .expect_err("closed_at must be rejected rather than silently ignored");
    assert!(error.to_string().contains("unknown field"), "{error}");
}

/// A status outside the lifecycle vocabulary must fail instead of returning
/// a successful no-op.
#[tokio::test]
async fn an_unknown_status_is_refused() {
    let surreal = mem_surreal().await;
    let before = matter(&surreal).await;
    let command: UpdateProjectCommand = serde_json::from_value(serde_json::json!({
        "status": "zzz-not-a-status"
    }))
    .expect("status reaches command validation");

    let error = update_project(&surreal, before.id, &command)
        .await
        .expect_err("unknown status must be rejected");
    assert!(
        matches!(error, ProjectCommandError::Invalid(_)),
        "{error:?}"
    );
    let unchanged = store::projects::find_by_id(&surreal, before.id)
        .await
        .expect("reload")
        .expect("matter still exists");
    assert_eq!(unchanged.status, "open");
    assert!(unchanged.closed_at.is_none());
}
