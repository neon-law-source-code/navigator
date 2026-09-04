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
/// `code` is the matter's code, required, and is stored exactly as given —
/// `open_matter` never generates or appends anything to it. A code is chosen
/// once, at matter-open, and never changes (`docs/glossary.md#project`); a
/// code already in use by another matter is refused rather than
/// disambiguated.
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
                "no entity named `{needle}` — run `navigator db list entities` to see \
                 what's seeded"
            )
        })?;
    // The client of record: the pre-existing client this matter is opened for,
    // resolved by `--client-email`. The command re-checks the role, but
    // resolving it here lets the CLI name the email in a friendlier error.
    let client = store::persons::find_by_email_ci(surreal, client_email)
        .await?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no person with email `{client_email}` — the client of record must \
                 exist before the matter opens: add it to the canonical seed under \
                 `seeds/`, or import it into a running deployment with \
                 `navigator site import person <seed-file>`"
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
            // The CLI runs firm-side with no per-request host to resolve; it
            // always opens against the firm's own default brand.
            brand: "neon".to_string(),
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

#[cfg(test)]
mod tests {
    use super::create;
    use store::entities::NewEntity;
    use store::persons::{NewPerson, Role};
    use store::test_support::{mem_surreal, SEED_ENTITY_JURISDICTION_ID, SEED_ENTITY_TYPE_ID};

    /// `cli project create` is the third adapter (alongside the web form and
    /// `POST /app/api/projects`) onto the shared `store::projects::open_matter`
    /// command. It resolves the human-facing `--entity-name` and
    /// `--client-email` flags to ids, the firm principal to the attester, and
    /// always opens against the firm's own default brand — there is no
    /// per-request host to resolve outside a browser, so `neon` is not a
    /// narrowing, it is the only value that makes sense here.
    #[tokio::test]
    async fn create_opens_a_matter_against_the_firms_default_brand() {
        let db = mem_surreal().await;
        let entity = store::entities::create(
            &db,
            &NewEntity {
                name: "Acme Anchor".to_string(),
                entity_type_id: SEED_ENTITY_TYPE_ID,
                jurisdiction_id: SEED_ENTITY_JURISDICTION_ID,
                phone: None,
                url: None,
                firm_anchor_key: None,
            },
        )
        .await
        .unwrap();
        store::persons::create(
            &db,
            &NewPerson::with_role("Firm Lawyer", "lawyer@example.com", Role::Lawyer),
        )
        .await
        .unwrap();
        let client = store::persons::create(
            &db,
            &NewPerson::with_role("Client of Record", "client@example.com", Role::Client),
        )
        .await
        .unwrap();

        let created = create(
            &db,
            "CLI-opened matter",
            "cli-opened-matter",
            Some(&entity.name),
            &client.email,
            true,
        )
        .await
        .unwrap();

        assert_eq!(created.code, "cli-opened-matter");
        assert_eq!(created.status, "open");
        let stored = store::projects::find_by_id(&db, created.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.brand, "neon",
            "the CLI door always opens against the firm's own default brand"
        );
    }

    /// The client-of-record check is the CLI's own friendlier error over the
    /// same refusal `open_matter` enforces — a firm attorney can never stand
    /// in as their own client.
    #[tokio::test]
    async fn create_refuses_a_non_client_of_record() {
        let db = mem_surreal().await;
        let entity = store::entities::create(
            &db,
            &NewEntity {
                name: "Acme Anchor".to_string(),
                entity_type_id: SEED_ENTITY_TYPE_ID,
                jurisdiction_id: SEED_ENTITY_JURISDICTION_ID,
                phone: None,
                url: None,
                firm_anchor_key: None,
            },
        )
        .await
        .unwrap();
        store::persons::create(
            &db,
            &NewPerson::with_role("Firm Lawyer", "lawyer2@example.com", Role::Lawyer),
        )
        .await
        .unwrap();
        let lawyer_as_client = store::persons::create(
            &db,
            &NewPerson::with_role("Not A Client", "not-a-client@example.com", Role::Lawyer),
        )
        .await
        .unwrap();

        let err = create(
            &db,
            "Bad client matter",
            "bad-client-matter",
            Some(&entity.name),
            &lawyer_as_client.email,
            true,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("must be a client person"));
    }
}
