//! A Project's source repository is a stored URL on any forge.
//!
//! The column replaced a coordinate composed from one deployment-wide forge
//! host, so what these tests pin is the *storage* contract rather than a
//! formatting one: any forge round-trips, an unsafe or unclonable value is
//! refused at the write instead of being stored, and absence is a legitimate
//! state that nothing fills in.

use store::projects::{
    create, set_repository_url, update_project, NewProject, ProjectCommandError,
    UpdateProjectCommand,
};
use store::test_support::{mem_surreal, seed_entity};
use uuid::Uuid;

async fn project(surreal: &store::surreal::SurrealDb, code: &str) -> store::projects::Project {
    create(
        surreal,
        &NewProject {
            code: code.to_string(),
            name: code.to_string(),
            status: "open".to_string(),
            entity_id: seed_entity(surreal).await,
            ..Default::default()
        },
    )
    .await
    .expect("create matter")
}

/// A matter opens with no repository, and nothing invents one.
#[tokio::test]
async fn a_new_matter_records_no_repository() {
    let surreal = mem_surreal().await;
    let row = project(&surreal, "repo-absent").await;
    assert_eq!(
        row.repository_url, None,
        "a matter opens before its source exists, so the column starts empty"
    );
}

/// Any forge round-trips: the point of storing a URL instead of composing one.
#[tokio::test]
async fn any_forge_and_organization_round_trips() {
    let surreal = mem_surreal().await;
    let row = project(&surreal, "repo-forges").await;

    for url in [
        "https://github.com/an-org/a-project",
        "https://gitlab.example/a-group/a-subgroup/a-project.git",
        "https://forge.example:8443/another-org/x",
        "http://forge.internal/an-org/y",
    ] {
        let updated = set_repository_url(&surreal, row.id, Some(url))
            .await
            .expect("write")
            .expect("the matter exists");
        assert_eq!(
            updated.repository_url.as_deref(),
            Some(url),
            "{url} must round-trip verbatim"
        );
    }
}

/// Clearing is explicit and distinct from never having had one.
#[tokio::test]
async fn a_recorded_url_can_be_cleared() {
    let surreal = mem_surreal().await;
    let row = project(&surreal, "repo-clear").await;
    set_repository_url(&surreal, row.id, Some("https://forge.example/o/r"))
        .await
        .expect("write")
        .expect("the matter exists");

    let cleared = set_repository_url(&surreal, row.id, None)
        .await
        .expect("clear")
        .expect("the matter exists");
    assert_eq!(cleared.repository_url, None);

    // Blank and whitespace are the same instruction as `None`, because an HTML
    // form posts an empty field rather than omitting it.
    for blank in ["", "   "] {
        set_repository_url(&surreal, row.id, Some("https://forge.example/o/r"))
            .await
            .expect("write")
            .expect("the matter exists");
        let cleared = set_repository_url(&surreal, row.id, Some(blank))
            .await
            .expect("clear")
            .expect("the matter exists");
        assert_eq!(cleared.repository_url, None, "{blank:?} must clear");
    }
}

/// An unclonable or unsafe URL is refused at the write, not stored.
///
/// This value is handed to `git clone` and rendered to a lawyer as a link, so
/// the refusal is the whole reason the column is validated rather than trimmed.
#[tokio::test]
async fn an_unsafe_or_unclonable_url_is_refused_rather_than_stored() {
    let surreal = mem_surreal().await;
    let row = project(&surreal, "repo-refuse").await;

    for bad in [
        "not-a-url",
        "ssh://git@forge.example/o/r",
        "file:///etc/passwd",
        "https://forge.example",
        "https://user:token@forge.example/o/r",
    ] {
        let error = set_repository_url(&surreal, row.id, Some(bad))
            .await
            .expect_err("must be refused");
        assert!(
            matches!(error, ProjectCommandError::Invalid(_)),
            "{bad} must be caller-correctable, got {error:?}"
        );
    }

    // And nothing landed.
    let reread = store::projects::find_by_id(&surreal, row.id)
        .await
        .expect("read")
        .expect("the matter exists");
    assert_eq!(
        reread.repository_url, None,
        "a refused write must leave the column untouched"
    );
}

/// Writing to a matter that no longer exists is `Ok(None)`, not an error.
#[tokio::test]
async fn setting_the_url_on_a_missing_matter_is_not_an_error() {
    let surreal = mem_surreal().await;
    assert!(
        set_repository_url(&surreal, Uuid::now_v7(), Some("https://forge.example/o/r"))
            .await
            .expect("no error")
            .is_none(),
        "a vanished matter is reported as absent, not as a failure"
    );
}

/// The descriptive-edit command carries the same validation and the same
/// blank-clears behaviour, so the admin form and the store setter cannot drift.
#[tokio::test]
async fn the_edit_command_validates_and_clears_the_repository_url() {
    let surreal = mem_surreal().await;
    let row = project(&surreal, "repo-command").await;

    let command = |url: Option<&str>| UpdateProjectCommand {
        name: Some("Repo Command".to_string()),
        repository_url: url.map(str::to_string),
        ..Default::default()
    };

    let updated = update_project(
        &surreal,
        row.id,
        &command(Some("https://forge.example/o/r")),
    )
    .await
    .expect("write");
    assert_eq!(
        updated.repository_url.as_deref(),
        Some("https://forge.example/o/r")
    );

    let error = update_project(&surreal, row.id, &command(Some("file:///etc/passwd")))
        .await
        .expect_err("must be refused");
    assert!(
        matches!(error, ProjectCommandError::Invalid(_)),
        "{error:?}"
    );

    let cleared = update_project(&surreal, row.id, &command(Some("")))
        .await
        .expect("clear");
    assert_eq!(cleared.repository_url, None, "a blank field clears");

    // An omitted field leaves the column alone, which is what lets a caller
    // edit only the name.
    set_repository_url(&surreal, row.id, Some("https://forge.example/o/r"))
        .await
        .expect("write")
        .expect("the matter exists");
    let untouched = update_project(&surreal, row.id, &command(None))
        .await
        .expect("write");
    assert_eq!(
        untouched.repository_url.as_deref(),
        Some("https://forge.example/o/r"),
        "an omitted repository_url must not clear a recorded one"
    );
}
