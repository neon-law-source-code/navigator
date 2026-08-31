//! `aida_create_project` MCP tool.
//!
//! Opens a new Project (a [Matter] in client English) without
//! attaching a Notation yet. Use this when onboarding a matter
//! whose Template doesn't exist in Neon Law Navigator (a one-off settlement,
//! a custom expungement petition, an entity-management container) —
//! the Project is the durable home for Persons and Documents;
//! Notations attach later as Templates ship.
//!
//! [Matter]: ../../../docs/glossary.md#matter

use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::principal::Principal;

use super::ToolError;

#[must_use]
pub fn descriptor() -> Value {
    json!({
        "name": "aida_create_project",
        "description": "Open a new Project (matter) in Neon Law Navigator. A matter always opens against a \
                        pre-existing Entity — pass its uuid as `entity_id` (create the Entity \
                        first if needed). Every open requires the attorney's conflict \
                        `attestation`. A matter always opens `open`. Returns the new id, name, \
                        status, and entity_id so the caller can reference the row.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Human-readable matter name (e.g. \"Sison — mutual release\", \"ShookEstate\")."
                },
                "code": {
                    "type": "string",
                    "description": "The stem of the matter code (e.g. `sison-mutual-release`) — the \
                                    base of its eventual git repository name. Required. Lowercase \
                                    letters, digits, and single hyphens; must start and end with a \
                                    letter or digit; at most 80 characters. This is not the stored \
                                    code: Navigator appends a short generated suffix so the stem \
                                    alone can never collide with another matter's, since a code is \
                                    chosen once at matter-open and never changes. Ask the requesting \
                                    attorney for their preferred stem rather than inventing one."
                },
                "attestation": {
                    "type": "boolean",
                    "description": "The opening attorney's conflict attestation. Must be `true`: opening \
                                    a matter through AIDA affirms the requesting attorney (the lawyer \
                                    principal on the call) has checked for and cleared conflicts. The \
                                    open is refused without it — it is never defaulted."
                },
                "entity_id": {
                    "type": "string",
                    "format": "uuid",
                    "description": "Uuid of the pre-existing Entity this matter opens against (the \
                                    LLC, trust, estate, or a `Human` entity for a solo person)."
                },
                "client_dri_person_id": {
                    "type": "string",
                    "format": "uuid",
                    "description": "Uuid of the pre-existing client Person this matter is opened \
                                    for — its client-side Directly Responsible Individual. Must be \
                                    an existing person with role `client` (create the client \
                                    first). The matter's client of record is a client, never a \
                                    firm attorney."
                }
            },
            "required": ["name", "code", "entity_id", "client_dri_person_id", "attestation"],
            "additionalProperties": false
        }
    })
}

// `deny_unknown_fields` matches the descriptor's `additionalProperties: false`
// at the deserialize layer (`decode_args` does not itself validate the schema),
// so an obsolete or hallucinated argument — e.g. the removed `status` — is a
// hard `InvalidArguments` error rather than a silently dropped field on a
// legally-consequential write.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    name: String,
    /// The stem of the matter code. Required — see the descriptor: this is
    /// not the stored code, which gets a generated suffix appended.
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    attestation: Option<bool>,
    #[serde(default)]
    entity_id: Option<Uuid>,
    #[serde(default)]
    client_dri_person_id: Option<Uuid>,
}

#[allow(clippy::too_many_lines)] // one matter-opening flow onto the shared command
pub async fn call(
    surreal: &store::surreal::SurrealDb,
    principal: Option<&Principal>,
    arguments: &Value,
) -> Result<Value, ToolError> {
    let args: Args = super::decode_args(arguments)?;

    let name = args.name.trim().to_string();
    if name.is_empty() {
        return Err(ToolError::InvalidArguments("name must not be empty".into()));
    }

    // The stem is required and passed through to `open_matter`, which appends
    // a generated suffix — an AIDA-invented stem is fine (unlike an invented
    // final code once was), but ask the attorney for their preferred one when
    // it is available, per the descriptor.
    let code = args
        .code
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .ok_or_else(|| ToolError::InvalidArguments("code is required".into()))?
        .to_string();

    // A matter always opens against a pre-existing entity (projects.entity_id
    // is NOT NULL). Require it and validate it exists before opening.
    let entity_id = args.entity_id.ok_or_else(|| {
        ToolError::InvalidArguments(
            "entity_id is required — open the matter against an existing entity".into(),
        )
    })?;
    if store::entities::find_by_id(surreal, entity_id)
        .await?
        .is_none()
    {
        return Err(ToolError::NotFound(format!("entity_id={entity_id}")));
    }

    // The client side is the pre-existing client this matter is opened for —
    // required, and a real `Role::Client` person (the client of record is a
    // client, never a firm attorney). The lawyer side defaults to the firm
    // principal (resolved by role).
    let client_dri_id = args.client_dri_person_id.ok_or_else(|| {
        ToolError::InvalidArguments(
            "client_dri_person_id is required — open the matter for an existing client".into(),
        )
    })?;
    let client = store::persons::find_by_id(surreal, client_dri_id)
        .await?
        .ok_or_else(|| ToolError::NotFound(format!("client_dri_person_id={client_dri_id}")))?;
    if client.role != store::persons::Role::Client {
        return Err(ToolError::InvalidArguments(format!(
            "the client DRI must be a client person, not {}",
            client.role.as_str()
        )));
    }
    // The firm-side DRI is the authenticated caller — the attorney opening
    // the matter through AIDA — so the matter lands in their workbench, the
    // same identity the lawyer web form uses (`portal::admin::resolve_lawyer_dri`).
    // A `client`-role principal must never own a matter from the firm side, so
    // the caller is required to be lawyer-tier.
    //
    // `default_firm_dri` remains the no-principal fallback. That is the
    // pass-through dev path (KIND, where no auth layer ran); a deployed
    // environment cannot reach it, because `GOOGLE_OAUTH_CLIENT_IDS` is a
    // boot invariant (`store::deployment::WEB_REQUIREMENTS`), so
    // `portal::google_oauth` is always enforced and `portal::mcp_principal`
    // always injects — the same reasoning `create_notation` documents.
    let lawyer_dri = match principal {
        Some(principal) => {
            let actor = store::persons::find_by_email_ci(surreal, &principal.email)
                .await?
                .ok_or_else(|| {
                    ToolError::NotFound(format!("person with email `{}`", principal.email))
                })?;
            if !actor.role.is_lawyer_tier() {
                return Err(ToolError::Forbidden(format!(
                    "{} is not lawyer or admin and cannot be a matter's lawyer DRI",
                    principal.email
                )));
            }
            actor.id
        }
        None => store::persons::default_firm_dri(surreal)
            .await?
            .ok_or_else(|| {
                ToolError::InvalidArguments(
                    "no firm principal to assign as lawyer DRI — seed a lawyer/admin person first"
                        .into(),
                )
            })?,
    };

    // Open the matter through the shared command — the same boundary the web
    // form and CLI (`project create`) use (#355). It owns the conflict block,
    // the attestation audit row, both DRI designations, and repo provisioning
    // in one transaction; this tool is one more adapter that resolves ids and
    // renders the outcome. `attestation` must be `true` (the AIDA caller
    // affirms the requesting attorney has cleared conflicts); soft findings
    // proceed on it, a blocking conflict is refused.
    let created = store::projects::open_matter(
        surreal,
        &store::projects::OpenMatterCommand {
            name,
            code,
            client_id: client.id,
            entity_id,
            description: None,
            attestation: args.attestation.unwrap_or(false),
            acting_person_id: lawyer_dri,
        },
    )
    .await
    .map_err(open_matter_tool_error)?;

    store::project_surfaces::reconcile_after_open(surreal, created.id).await;

    let summary = format!(
        "Created project id={} ({}, status={}, entity_id={}).",
        created.id, created.name, created.status, created.entity_id
    );

    Ok(json!({
        "content": [{ "type": "text", "text": summary }],
        "structuredContent": {
            "id": created.id,
            "name": created.name,
            "status": created.status,
            "entity_id": created.entity_id,
        }
    }))
}

/// Map a matter-open command failure to the tool's error vocabulary. A missing
/// attestation or a bad reference is a caller-correctable `InvalidArguments`; a
/// blocking conflict or a non-lawyer attester is `Forbidden`; a provisioning
/// failure is `Internal` (the forge is an infrastructure dependency); a
/// database error propagates.
fn open_matter_tool_error(err: store::projects::OpenMatterError) -> ToolError {
    use store::projects::OpenMatterError as E;
    match err {
        E::AttestationRequired => ToolError::InvalidArguments(
            "this matter open requires attestation — pass attestation=true to affirm the \
             requesting attorney has checked for and cleared conflicts"
                .into(),
        ),
        E::BlockingConflict(findings) => ToolError::Forbidden(format!(
            "conflict check refused this matter — it is adverse to a current client:\n{}",
            findings.join("\n")
        )),
        E::ClientNotAllowed => ToolError::InvalidArguments(
            "the client of record must be a client, not a firm attorney".into(),
        ),
        E::AttesterNotAllowed => {
            ToolError::Forbidden("the attesting attorney must be a firm lawyer".into())
        }
        E::NotFound(what) => ToolError::NotFound(what.into()),
        E::CodeConflict => {
            ToolError::InvalidArguments("that project code is already in use".into())
        }
        E::Invalid(m) => ToolError::InvalidArguments(m.into()),
        // The matter is already open when this fires — only its audit
        // entry failed — so it is a server-side fault, not something the
        // caller can correct by retrying with different arguments.
        E::Attestation(e) => ToolError::Internal(e.to_string()),
        E::Db(e) => ToolError::Database(e),
    }
}

#[cfg(test)]
mod tests {
    use super::{call, descriptor};
    use crate::principal::Principal;
    use crate::tools::ToolError;
    use serde_json::json;
    use uuid::Uuid;

    use store::test_support::mem_surreal;
    async fn db() -> store::surreal::SurrealDb {
        let surreal = mem_surreal().await;
        surreal
    }

    /// Seed a `Role::Client` person and return its id — the matter's
    /// client of record, required as `client_dri_person_id`.
    async fn seed_client(surreal: &store::surreal::SurrealDb) -> Uuid {
        store::persons::create(
            surreal,
            &store::persons::NewPerson::with_role(
                "Libra",
                "libra@example.com",
                store::persons::Role::Client,
            ),
        )
        .await
        .unwrap()
        .id
    }

    /// Seed a `Role::Admin` person so `default_firm_dri` can resolve a
    /// lawyer-side DRI for the project. Returns the id so a test can assert
    /// the caller — not this firm default — became the lawyer DRI.
    async fn seed_firm_principal(surreal: &store::surreal::SurrealDb) -> Uuid {
        store::persons::create(
            surreal,
            &store::persons::NewPerson::with_role(
                "Firm Principal",
                "principal@example.com",
                store::persons::Role::Admin,
            ),
        )
        .await
        .unwrap()
        .id
    }

    /// Seed a `Role::Lawyer` person — a firm attorney who can open matters —
    /// and return its id. This is the caller the fix must record as the
    /// lawyer DRI when it opens a matter through AIDA.
    async fn seed_lawyer(surreal: &store::surreal::SurrealDb, email: &str) -> Uuid {
        store::persons::create(
            surreal,
            &store::persons::NewPerson::with_role(email, email, store::persons::Role::Lawyer),
        )
        .await
        .unwrap()
        .id
    }

    async fn seed_entity(surreal: &store::surreal::SurrealDb) -> Uuid {
        // The engine does not validate a `record<>` link and nothing in
        // this tool resolves either reference, so the fixture points them
        // at rows it never writes.
        store::entities::create(
            surreal,
            &store::entities::NewEntity {
                name: "shook.family".into(),
                entity_type_id: Uuid::now_v7(),
                jurisdiction_id: Uuid::now_v7(),
                phone: None,
                url: None,
                firm_anchor_key: None,
            },
        )
        .await
        .unwrap()
        .id
    }

    #[test]
    fn descriptor_names_the_tool_and_requires_name_code_and_entity() {
        let d = descriptor();
        assert_eq!(d["name"], "aida_create_project");
        let required = d["inputSchema"]["required"].as_array().unwrap();
        let names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        // `code` is required on the schema, not just in `call`: AIDA must be
        // told to ask for a stem rather than discovering the refusal by
        // trying an open.
        assert_eq!(
            names,
            vec![
                "name",
                "code",
                "entity_id",
                "client_dri_person_id",
                "attestation"
            ]
        );
        assert_eq!(d["inputSchema"]["additionalProperties"], false);
    }

    #[tokio::test]
    async fn a_missing_code_is_refused_rather_than_derived() {
        let surreal = db().await;
        let eid = seed_entity(&surreal).await;
        let cid = seed_client(&surreal).await;
        seed_firm_principal(&surreal).await;
        let err = call(
            &surreal,
            None,
            &json!({ "name": "No Code", "entity_id": eid, "client_dri_person_id": cid, "attestation": true }),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, ToolError::InvalidArguments(ref m) if m.contains("code is required")),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn happy_path_inserts_with_defaults() {
        let surreal = db().await;
        let eid = seed_entity(&surreal).await;
        let cid = seed_client(&surreal).await;
        seed_firm_principal(&surreal).await;
        let r = call(
            &surreal,
            None,
            &json!({ "name": "Sison", "code": "sison", "entity_id": eid, "client_dri_person_id": cid, "attestation": true }),
        )
        .await
        .unwrap();
        assert_eq!(r["structuredContent"]["name"], "Sison");
        assert_eq!(r["structuredContent"]["status"], "open");
        assert_eq!(r["structuredContent"]["entity_id"], eid.to_string());
        let all = store::projects::all(&surreal).await.unwrap();
        assert_eq!(all.len(), 1);
    }

    #[tokio::test]
    async fn authenticated_lawyer_becomes_the_lawyer_dri() {
        // The bug: a matter opened through AIDA was assigned to
        // `default_firm_dri` (the first admin), so it never appeared in the
        // workbench of the attorney who opened it. Seed an admin *and* the
        // acting lawyer attorney; the caller — not the admin — must be the
        // recorded lawyer DRI.
        let surreal = db().await;
        let eid = seed_entity(&surreal).await;
        let cid = seed_client(&surreal).await;
        let admin = seed_firm_principal(&surreal).await;
        let lawyer = seed_lawyer(&surreal, "attorney@example.com").await;
        let r = call(
            &surreal,
            Some(&Principal::new("attorney@example.com")),
            &json!({ "name": "Sison", "code": "sison", "entity_id": eid, "client_dri_person_id": cid, "attestation": true }),
        )
        .await
        .unwrap();
        let id: Uuid = serde_json::from_value(r["structuredContent"]["id"].clone()).unwrap();
        let row = store::projects::find_by_id(&surreal, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            store::projects::participations_for_project(&surreal, row.id)
                .await
                .unwrap()
                .into_iter()
                .find(|p| p.is_lawyer_dri)
                .map(|p| p.person_id),
            Some(lawyer),
            "the caller must be the lawyer DRI"
        );
        assert_ne!(
            store::projects::participations_for_project(&surreal, row.id)
                .await
                .unwrap()
                .into_iter()
                .find(|p| p.is_lawyer_dri)
                .map(|p| p.person_id),
            Some(admin),
            "the firm default must not win over the caller"
        );
    }

    #[tokio::test]
    async fn caller_email_resolves_case_insensitively() {
        // The verified principal email may differ in case from the stored
        // row; resolution matches on lowercase, like `create_notation`.
        let surreal = db().await;
        let eid = seed_entity(&surreal).await;
        let cid = seed_client(&surreal).await;
        seed_firm_principal(&surreal).await;
        let lawyer = seed_lawyer(&surreal, "attorney@example.com").await;
        let r = call(
            &surreal,
            Some(&Principal::new("Attorney@Example.com")),
            &json!({ "name": "Sison", "code": "sison", "entity_id": eid, "client_dri_person_id": cid, "attestation": true }),
        )
        .await
        .unwrap();
        let id: Uuid = serde_json::from_value(r["structuredContent"]["id"].clone()).unwrap();
        let row = store::projects::find_by_id(&surreal, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            store::projects::participations_for_project(&surreal, row.id)
                .await
                .unwrap()
                .into_iter()
                .find(|p| p.is_lawyer_dri)
                .map(|p| p.person_id),
            Some(lawyer)
        );
    }

    #[tokio::test]
    async fn no_principal_falls_back_to_default_firm_dri() {
        // The dev/local path with no auth layer keeps the firm default.
        let surreal = db().await;
        let eid = seed_entity(&surreal).await;
        let cid = seed_client(&surreal).await;
        let admin = seed_firm_principal(&surreal).await;
        let r = call(
            &surreal,
            None,
            &json!({ "name": "Sison", "code": "sison", "entity_id": eid, "client_dri_person_id": cid, "attestation": true }),
        )
        .await
        .unwrap();
        let id: Uuid = serde_json::from_value(r["structuredContent"]["id"].clone()).unwrap();
        let row = store::projects::find_by_id(&surreal, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            store::projects::participations_for_project(&surreal, row.id)
                .await
                .unwrap()
                .into_iter()
                .find(|p| p.is_lawyer_dri)
                .map(|p| p.person_id),
            Some(admin)
        );
    }

    #[tokio::test]
    async fn client_role_principal_cannot_be_lawyer_dri() {
        // A `client`-role principal must never own a matter from the firm
        // side, even if it somehow reaches this door.
        let surreal = db().await;
        let eid = seed_entity(&surreal).await;
        let cid = seed_client(&surreal).await;
        seed_firm_principal(&surreal).await;
        let err = call(
            &surreal,
            Some(&Principal::new("libra@example.com")),
            &json!({ "name": "Sison", "code": "sison", "entity_id": eid, "client_dri_person_id": cid, "attestation": true }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ToolError::Forbidden(_)), "got {err:?}");
        assert!(
            store::projects::all(&surreal).await.unwrap().is_empty(),
            "no matter is opened for a non-lawyer caller"
        );
    }

    #[tokio::test]
    async fn unknown_principal_email_is_not_found() {
        let surreal = db().await;
        let eid = seed_entity(&surreal).await;
        let cid = seed_client(&surreal).await;
        seed_firm_principal(&surreal).await;
        let err = call(
            &surreal,
            Some(&Principal::new("ghost@example.com")),
            &json!({ "name": "Sison", "code": "sison", "entity_id": eid, "client_dri_person_id": cid, "attestation": true }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ToolError::NotFound(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn binds_entity_when_provided_and_exists() {
        let surreal = db().await;
        let eid = seed_entity(&surreal).await;
        let cid = seed_client(&surreal).await;
        seed_firm_principal(&surreal).await;
        let r = call(
            &surreal,
            None,
            &json!({ "name": "ShookEstate", "code": "shookestate", "entity_id": eid, "client_dri_person_id": cid, "attestation": true }),
        )
        .await
        .unwrap();
        assert_eq!(r["structuredContent"]["entity_id"], eid.to_string());
    }

    // Repo-provisioning failure and its rollback are now owned by the shared
    // `open_matter` command (which selects the forge from the environment, not
    // an injected one), so the in-process forge-injection test that used to
    // live here moved with the logic: the CLI's subprocess test
    // `create_project_rolls_back_when_repo_provisioning_fails` exercises the
    // rollback with an isolated `NAVIGATOR_FORGE_BACKEND`.

    #[tokio::test]
    async fn missing_attestation_is_refused() {
        // Every matter open requires the attorney's conflict attestation. Omit
        // it and the shared command refuses the open, opening nothing — the
        // AIDA door onto the same gate the web form and CLI enforce (#355).
        let surreal = db().await;
        let eid = seed_entity(&surreal).await;
        let cid = seed_client(&surreal).await;
        seed_firm_principal(&surreal).await;
        let err = call(
            &surreal,
            None,
            // Attestation deliberately omitted — every other field resolves.
            &json!({ "name": "Unattested", "code": "unattested", "entity_id": eid, "client_dri_person_id": cid }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)), "got {err:?}");
        assert!(
            store::projects::all(&surreal).await.unwrap().is_empty(),
            "a refused open writes no matter",
        );
    }

    #[tokio::test]
    async fn unknown_entity_id_returns_not_found() {
        let surreal = db().await;
        let missing = Uuid::now_v7();
        let err = call(
            &surreal,
            None,
            &json!({ "name": "X", "code": "x", "entity_id": missing }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ToolError::NotFound(_)));
    }

    #[tokio::test]
    async fn empty_name_is_invalid() {
        let surreal = db().await;
        let err = call(&surreal, None, &json!({ "name": "   ", "code": "matter" }))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }

    #[tokio::test]
    async fn obsolete_status_argument_is_rejected() {
        // `status` was removed when the tool started routing through the
        // matter-open command (a matter always opens `open`). A caller still
        // sending it must be told — `deny_unknown_fields` rejects it rather
        // than silently opening the matter `open` while the caller believes it
        // set `closed`.
        let surreal = db().await;
        let eid = seed_entity(&surreal).await;
        let cid = seed_client(&surreal).await;
        seed_firm_principal(&surreal).await;
        let err = call(
            &surreal,
            None,
            &json!({
                "name": "Stale", "code": "stale",
                "entity_id": eid,
                "client_dri_person_id": cid,
                "attestation": true,
                "status": "closed"
            }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)), "got {err:?}");
        assert!(
            store::projects::all(&surreal).await.unwrap().is_empty(),
            "a rejected request opens no matter",
        );
    }

    /// An AIDA-opened matter must be reachable by its own accountable
    /// lawyer. Designating a DRI writes the membership row and the marker
    /// together, so this is now structural — there is no way to open a matter
    /// naming a lawyer who is not on it.
    #[tokio::test]
    async fn open_writes_the_dri_participation_rows() {
        let surreal = db().await;
        let eid = seed_entity(&surreal).await;
        let cid = seed_client(&surreal).await;
        seed_firm_principal(&surreal).await;
        call(
            &surreal,
            None,
            &json!({ "name": "Disclosed", "code": "disclosed", "entity_id": eid, "client_dri_person_id": cid, "attestation": true }),
        )
        .await
        .unwrap();

        let project_row = store::projects::all(&surreal).await.unwrap().pop().unwrap();
        let rows = store::projects::participations_for_project(&surreal, project_row.id)
            .await
            .unwrap()
            .into_iter()
            .map(|r| (r.person_id, r.participation))
            .collect::<Vec<_>>();
        let lawyer_dri = store::projects::participations_for_project(&surreal, project_row.id)
            .await
            .unwrap()
            .into_iter()
            .find(|p| p.is_lawyer_dri)
            .map(|p| p.person_id)
            .expect("lawyer DRI assigned");
        assert!(
            rows.contains(&(lawyer_dri, "attorney".to_string())),
            "the lawyer DRI needs its membership disclosure: {rows:?}"
        );
        assert!(
            rows.contains(&(cid, "client".to_string())),
            "the client DRI needs its participation row: {rows:?}"
        );
        assert!(
            store::projects::can_access_as_lawyer_in_surreal(
                &surreal,
                Some(lawyer_dri),
                store::persons::Role::Lawyer,
                project_row.id,
            )
            .await
            .unwrap(),
            "the accountable lawyer must reach the matter they were assigned"
        );
    }

    /// The door no longer collects a product code. A caller still sending
    /// one is told so by `deny_unknown_fields`, rather than having it
    /// silently ignored while they believe the matter was correlated.
    #[tokio::test]
    async fn a_posted_product_code_is_rejected() {
        let surreal = db().await;
        let eid = seed_entity(&surreal).await;
        let cid = seed_client(&surreal).await;
        seed_firm_principal(&surreal).await;
        let err = call(
            &surreal,
            None,
            &json!({
                "name": "Bad Product", "code": "bad-product",
                "entity_id": eid,
                "client_dri_person_id": cid,
                "attestation": true,
                "product_code": "retired-product"
            }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)), "got {err:?}");
        assert!(
            store::projects::all(&surreal).await.unwrap().is_empty(),
            "a rejected request opens no matter",
        );
    }
}
