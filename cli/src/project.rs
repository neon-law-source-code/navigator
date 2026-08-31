//! `project` subcommand: write-side primitives for the `projects`
//! table.
//!
//! Today the only operation is `create`, which inserts one row with
//! a required Entity link. The caller is expected to have already run
//! seed against the target store so the Entity it names
//! actually exists.

use uuid::Uuid;

/// Outcome of a `project create` run, returned so tests can assert
/// on the inserted row without re-querying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedProject {
    pub id: Uuid,
    /// The stable, lawyer-facing matter code — the handle a later
    /// `notation create --project <code>` refers to.
    pub code: String,
    pub name: String,
    pub status: String,
    pub entity_id: Uuid,
}

/// Open a new matter through the shared `store::projects::open_matter`
/// command — the same command boundary the web form and `POST /app/api/projects`
/// use (navigator#355). The CLI is one adapter: it resolves the human-facing
/// `--entity-name` and `--client-email` to ids and the
/// firm principal to the attesting attorney, then hands the command the
/// conflict block, the attestation audit row, and both DRI designations.
/// The Drive ingest folder and source repository are then created or adopted
/// best-effort; a Drive or forge fault leaves the matter open.
///
/// `attest` is the operator's explicit `--attest` affirmation that the
/// attorney has checked for and cleared conflicts; the command refuses the
/// open without it (there is no default). A matter always opens as `open`, so
/// there is no status argument — lifecycle transitions are their own commands
/// (navigator#770).
///
/// `code` is the stem of the matter's code, required. It is not the stored
/// code: `open_matter` appends a short generated suffix so the stem alone can
/// never collide with another matter's — a code is chosen once, at
/// matter-open, and never changes (`docs/glossary.md#project`).
#[allow(clippy::too_many_arguments)] // the human-facing open flags
pub async fn create(
    surreal: &store::surreal::SurrealDb,
    name: &str,
    code: &str,
    entity_name: Option<&str>,
    client_email: &str,
    attest: bool,
) -> anyhow::Result<CreatedProject> {
    // A matter always opens against a pre-existing entity
    // (`projects.entity_id` is NOT NULL). Require `--entity-name` and
    // resolve it strictly to an id for the command.
    let needle = entity_name.ok_or_else(|| {
        anyhow::anyhow!("an entity is required — pass --entity-name (create the entity first)")
    })?;
    let entity_id = store::entities::find_by_name(surreal, needle)
        .await?
        .map(|e| e.id)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no entity named `{needle}` — run `cli list entities` to see what's seeded"
            )
        })?;
    // The client of record: the pre-existing client this matter is opened for,
    // resolved by `--client-email`. The command re-checks the role, but
    // resolving it here lets the CLI name the email in a friendlier error.
    let client = store::persons::find_by_email_ci(surreal, client_email)
        .await?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no person with email `{client_email}` — create the client first \
                 (`cli person create` / bulk import)"
            )
        })?;
    if client.role != store::persons::Role::Client {
        anyhow::bail!(
            "the client DRI `{client_email}` must be a client person, not {}",
            client.role.as_str()
        );
    }
    // The attesting attorney and lawyer-side DRI: the firm principal. It must be
    // lawyer-tier (an attorney), which `default_firm_dri` guarantees (admin, or
    // lawyer) — the command enforces the same invariant.
    let attester = store::persons::default_firm_dri(surreal)
        .await?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no firm principal for the lawyer DRI — seed a lawyer/admin person first"
            )
        })?;

    let created = store::projects::open_matter(
        surreal,
        &store::projects::OpenMatterCommand {
            name: name.to_string(),
            code: code.to_string(),
            client_id: client.id,
            entity_id,
            description: None,
            attestation: attest,
            acting_person_id: attester,
        },
    )
    .await
    .map_err(|e| match e {
        store::projects::OpenMatterError::AttestationRequired => anyhow::anyhow!(
            "this matter open requires attestation — pass --attest to affirm the attorney \
             has checked for and cleared conflicts for this matter"
        ),
        store::projects::OpenMatterError::BlockingConflict(findings) => anyhow::anyhow!(
            "conflict check refused this matter — it is adverse to a current client:\n{}",
            findings.join("\n")
        ),
        other => anyhow::Error::new(other),
    })?;

    store::project_surfaces::reconcile_after_open(surreal, created.id).await;

    Ok(CreatedProject {
        id: created.id,
        code: created.code,
        name: created.name,
        status: created.status,
        entity_id: created.entity_id,
    })
}
