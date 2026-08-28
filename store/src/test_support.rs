//! Shared test infrastructure for every workspace crate that exercises
//! the `store` schema.
//!
//! # The pattern
//!
//! Each test opens its own embedded, memory-backed SurrealDB inside the
//! test process — [`mem_surreal`] — and applies the `DEFINE` schema to it.
//! Nothing is shared between tests and nothing outlives the process, so
//! the whole suite runs in parallel with no cross-test collisions, no
//! container, and no cleanup. `cargo test` needs no configuration and no
//! Docker.
//!
//! The one exception is a test that spawns the `navigator` binary: a
//! subprocess cannot reach an in-process engine, so [`server_surreal`]
//! opens a *server-mode* database instead and hands back the coordinates
//! to point that process at. It skips when no endpoint is configured,
//! which `NAV_REQUIRE_SURREAL=1` turns into a failure so CI stays honest.
//!
//! The rest of this module is seeds: the small fixtures — an entity, a
//! matter, a person, a notation — that most tests need before they can
//! assert anything.

use uuid::Uuid;

/// A private embedded SurrealDB with the `DEFINE` schema applied — one
/// engine per test, inside the test process.
pub async fn mem_surreal() -> crate::surreal::SurrealDb {
    crate::surreal::test_support::mem().await
}

/// A SurrealDB reachable from *another process*, plus the coordinates to
/// point that process at it — see [`server_surreal`].
pub struct ServerSurreal {
    /// The connected handle, on a database of the caller's own naming,
    /// with the `DEFINE` schema applied.
    pub db: crate::surreal::SurrealDb,
    /// The endpoint, namespace, database, and any credentials, ready to
    /// be handed to a subprocess as environment variables.
    pub config: crate::surreal::SurrealConfig,
}

/// Open a *server-mode* SurrealDB for a test that spawns a subprocess.
///
/// [`mem_surreal`] lives inside the test process, so a `navigator`
/// invocation cannot reach it. A test that runs the binary — and every
/// `navigator db` subcommand touches the person directory, if only through
/// the canonical seed — needs a real server instead:
///
/// - **`NAVIGATOR_SURREAL_ENDPOINT` set** → connect, on `database` so two
///   tests on one engine cannot collide, and apply the schema.
/// - **unset** → return `None`, keeping `cargo test` zero-config on a
///   laptop with no dependency tier running.
///
/// `NAV_REQUIRE_SURREAL=1` turns that skip into a failure, which is what
/// keeps CI honest. A half-configured environment (an endpoint with no
/// namespace) always panics: that is a mistake, not an opt-out.
///
/// # Panics
///
/// When the environment is half-configured, when `NAV_REQUIRE_SURREAL=1`
/// and no endpoint is set, or when connecting or applying the schema
/// fails.
pub async fn server_surreal(database: &str) -> Option<ServerSurreal> {
    let config = match crate::surreal::SurrealConfig::from_env() {
        Ok(config) => crate::surreal::SurrealConfig {
            database: database.to_string(),
            ..config
        },
        Err(crate::surreal::SurrealConfigError::MissingEnv(name)) => {
            assert!(
                std::env::var("NAV_REQUIRE_SURREAL").as_deref() != Ok("1"),
                "NAV_REQUIRE_SURREAL=1 but {name} is unset: this test spawns `navigator`, \
                 which resolves the person directory in SurrealDB and cannot reach an \
                 in-process engine.",
            );
            eprintln!("skipping the server-mode SurrealDB lane: {name} is unset");
            return None;
        }
        Err(err) => panic!("the SurrealDB environment is half-configured: {err}"),
    };
    let db = crate::surreal::connect(&config)
        .await
        .expect("connect to the person store");
    crate::schema::apply(&db).await.expect("apply the schema");
    Some(ServerSurreal { db, config })
}

/// The synthetic `jurisdiction_id` every [`seed_entity`] row carries.
///
/// The entity's `jurisdiction_id` is a `record<jurisdiction>` link now
/// (ENG-120), and the engine does **not** validate a link, so this
/// fixture still deliberately points it at no row: nothing here needs
/// the jurisdiction resolved, and seeding one would cost every
/// project-focused test a reference row it never reads. A test that
/// *renders* jurisdiction names must seed its own row through
/// `store::jurisdictions` and use that id instead.
pub const SEED_ENTITY_JURISDICTION_ID: Uuid =
    Uuid::from_u128(0x0199_0000_0000_7000_8000_0000_0000_0075);

/// The synthetic `entity_type_id` every [`seed_entity`] row carries.
/// Same contract as [`SEED_ENTITY_JURISDICTION_ID`].
pub const SEED_ENTITY_TYPE_ID: Uuid = Uuid::from_u128(0x0199_0000_0000_7000_8000_0000_0000_0076);

/// Create a minimal entity and return its id. `project.entity_id` is
/// required, so every test that opens a matter needs a pre-existing
/// entity; this is the one-liner that supplies it.
///
/// Its references are the
/// synthetic [`SEED_ENTITY_JURISDICTION_ID`] and [`SEED_ENTITY_TYPE_ID`],
/// and it never carries a firm-anchor key — a fixture that minted one
/// would make the *second* fixture in the same database fail on the
/// `entity_firm_anchor` index.
pub async fn seed_entity(surreal: &crate::surreal::SurrealDb) -> Uuid {
    crate::entities::create(
        surreal,
        &crate::entities::NewEntity {
            name: format!("Test Entity {}", Uuid::now_v7()),
            entity_type_id: SEED_ENTITY_TYPE_ID,
            jurisdiction_id: SEED_ENTITY_JURISDICTION_ID,
            phone: None,
            url: None,
            firm_anchor_key: None,
        },
    )
    .await
    .expect("seed entity")
    .id
}

/// Rename the firm anchor aside and surrender its `firm_anchor_key`,
/// opening the "white-label window" in which the protected name exists in
/// configuration but no row carries it.
///
/// This deliberately goes around `store::entities::update`, which refuses
/// to rename a row carrying the key — that refusal is the invariant the
/// tests using this helper are there to prove, so a fixture that went
/// through the seam could not set the scene. It lives here rather than in
/// a test file so there is exactly one documented way around the guard.
///
/// It drops the `firm_anchor` claim as well as the column. Clearing only
/// the column would leave the claim standing, and the window it is
/// opening would be shut to every later mint.
///
/// # Panics
///
/// If the write fails, or the entity does not exist.
pub async fn release_firm_anchor(
    surreal: &crate::surreal::SurrealDb,
    entity_id: Uuid,
    aside_name: &str,
) {
    surreal
        .query(
            "BEGIN TRANSACTION; \
             DELETE firm_anchor WHERE entity_id = $id; \
             UPDATE $id SET name = $name, firm_anchor_key = NONE; \
             COMMIT TRANSACTION;",
        )
        .bind(("id", crate::surreal::record_id("entity", entity_id)))
        .bind(("name", aside_name.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)
        .expect("move the seeded anchor aside");
}

/// Create a minimal open matter in the projects cluster. Tests that need
/// a different lifecycle state or product can call
/// [`crate::projects::create`] directly.
///
/// # Panics
///
/// If seeding the entity or the project fails.
pub async fn seed_project(
    surreal: &crate::surreal::SurrealDb,
    name: &str,
) -> crate::projects::Project {
    seed_project_row(surreal, name).await
}

/// [`seed_project`] returning just the id — what most fixtures want.
///
/// # Panics
///
/// If seeding the entity or the project fails.
pub async fn seed_project_surreal(surreal: &crate::surreal::SurrealDb, name: &str) -> Uuid {
    seed_project_row(surreal, name).await.id
}

async fn seed_project_row(
    surreal: &crate::surreal::SurrealDb,
    name: &str,
) -> crate::projects::Project {
    let entity_id = seed_entity(surreal).await;
    let code_id = Uuid::now_v7();
    crate::projects::create(
        surreal,
        &crate::projects::NewProject {
            code: crate::projects::code_from_name(name, code_id),
            name: name.to_string(),
            status: "open".to_string(),
            entity_id,
            ..Default::default()
        },
    )
    .await
    .expect("seed project")
}

/// Find-or-create a single throwaway person to stand as a project's
/// lawyer and client DRI in tests. Every matter needs a real `person` id
/// on each side; tests that don't exercise DRI semantics point both at
/// this one fixture row. Idempotent (keyed on a fixed email) so repeated
/// calls in one test return the same id instead of colliding on
/// `person_email_lower`.
///
/// Find-or-create the person `input` describes, keyed on its email.
///
/// A scenario that says "there is a client called Libra" wants the row to
/// exist, not to be *created*. The difference matters wherever one engine
/// outlives one test: the cucumber suites share a single process-wide
/// engine (dropping one per scenario panics inside the async runtime), so
/// the second scenario to blind-insert the same mailbox is refused by the
/// `person_email_lower` index — a collision that says nothing about the
/// behaviour under test.
///
/// [`crate::persons::find_or_create`] settles identity, including the
/// case where two scenarios race to create the same mailbox. On top of
/// that, this brings the name and role up to date when an existing row
/// differs, so a caller gets the person it asked for rather than whatever
/// an earlier scenario happened to seed. [`dri_person`] is the same
/// shape, fixed to one well-known fixture identity.
///
/// # Panics
///
/// If the lookup or the write fails.
pub async fn ensure_person(
    surreal: &crate::surreal::SurrealDb,
    input: &crate::persons::NewPerson,
) -> crate::persons::Person {
    use crate::persons::{self, PersonEdit};

    let created = persons::find_by_email_ci(surreal, &input.email)
        .await
        .expect("person lookup")
        .is_none();
    let mut person = persons::find_or_create(surreal, input)
        .await
        .expect("seed the person");
    if created {
        return person;
    }

    if person.role != input.role {
        person = persons::set_role(surreal, person.id, input.role)
            .await
            .expect("align the seeded person's role")
            .expect("the seeded person is still there");
    }
    if person.name != input.name {
        person = persons::edit(
            surreal,
            person.id,
            &PersonEdit {
                name: Some(input.name.clone()),
                ..PersonEdit::default()
            },
        )
        .await
        .expect("align the seeded person's name")
        .expect("the seeded person is still there");
    }
    person
}

pub async fn dri_person(surreal: &crate::surreal::SurrealDb) -> Uuid {
    use crate::persons::{self, NewPerson};

    const EMAIL: &str = "dri-fixture@test.invalid";
    // `find_or_create`, because the cucumber suites share one engine and
    // run their scenarios concurrently: look-then-create interleaves and
    // the loser is refused by the unique email index.
    persons::find_or_create(surreal, &NewPerson::new("DRI Fixture", EMAIL))
        .await
        .expect("seed dri fixture person")
        .id
}

/// Seed one notation (with its template, person, and project) and
/// return the notation id. Shared by the helper-module tests that need
/// a matter to hang rows off (`review_documents`, `document_comments`).
/// The template declares `kind: onboarding`. The code is a unique synthetic
/// id (`sitting__transcript-…`), not a catalog template — see [`seed_notation_with_kind`] for a
/// test that needs a different (or absent) declared kind.
///
/// See [`seed_project`] for the same shape.
pub async fn seed_notation(surreal: &crate::surreal::SurrealDb) -> Uuid {
    seed_notation_with_kind_surreal(surreal, Some("onboarding")).await
}

/// Same as [`seed_notation`], but the template's declared `kind:` is
/// `kind` instead of the default `onboarding` — `None` seeds a template
/// with no declared kind at all, for the "generate_pdf on a kindless
/// template errors" case.
pub async fn seed_notation_with_kind(
    surreal: &crate::surreal::SurrealDb,
    kind: Option<&str>,
) -> Uuid {
    seed_notation_with_kind_surreal(surreal, kind).await
}

async fn seed_notation_with_kind_surreal(
    surreal: &crate::surreal::SurrealDb,
    kind: Option<&str>,
) -> Uuid {
    use crate::persons::{self, NewPerson};
    use crate::projects::{designate_dri_in_surreal, DriSide, NewProject};

    let entity_id = seed_entity(surreal).await;

    let tmpl = crate::templates::save_version(
        surreal,
        None,
        &format!("sitting__transcript-{}", Uuid::now_v7()),
        crate::templates::Version {
            title: "Estate Plan".into(),
            respondent_type: "person".into(),
            asset_id: None,
            form_code: None,
            kind: kind.map(str::to_string),
            source_commit_sha: None,
        },
    )
    .await
    .expect("seed template")
    .into_model();
    // A fixed mailbox on an engine that outlives one test, so this seeds
    // the same way [`dri_person`] does — see [`ensure_person`].
    let person = persons::find_or_create(surreal, &NewPerson::new("Libra", "libra@example.com"))
        .await
        .expect("seed person");
    let dri = dri_person(surreal).await;
    let proj = crate::projects::create(
        surreal,
        &NewProject {
            code: format!("libra-estate-{}", Uuid::now_v7()),
            name: "Libra estate plan".into(),
            status: "open".into(),
            entity_id,
            ..Default::default()
        },
    )
    .await
    .expect("seed project");
    designate_dri_in_surreal(surreal, proj.id, dri, DriSide::Lawyer)
        .await
        .expect("designate lawyer DRI");
    designate_dri_in_surreal(surreal, proj.id, dri, DriSide::Client)
        .await
        .expect("designate client DRI");
    crate::notations::create(
        surreal,
        &crate::notations::NewNotation::new(tmpl.id, person.id, proj.id, "BEGIN"),
    )
    .await
    .expect("seed notation")
    .id
}
