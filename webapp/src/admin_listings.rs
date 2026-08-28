//! The migrated generic read-only admin listings (#641 Phase 3, admin cluster).
//!
//! Each page is a thin pair built on [`crate::admin_listing`]: a `#[server]`
//! function that gates, reads, and projects its rows, and a component that
//! renders the result through [`crate::admin_listing::render_resource`]. The
//! chrome, table, and loading/error states live once in `admin_listing`; only
//! the read and the projection are per-page.
//!
//! Server-only entity paths stay fully qualified inside the `#[server]` bodies
//! so the wasm client build (which stubs those bodies) carries no unused
//! `store`/`SeaORM` imports.

use dioxus::prelude::*;

use crate::admin_listing::{render_resource, AdminListingView};

/// The `?sort=` a sortable listing reads back to render its header direction.
#[derive(serde::Deserialize, Default)]
pub struct SortQuery {
    #[serde(default)]
    pub sort: Option<String>,
}

/// Read the validated `?sort=` for a sortable listing. The route's pre-handler
/// has already rejected an unadvertised key with a `400`, so whatever arrives
/// here is safe to order by.
#[cfg(feature = "server")]
async fn requested_sort() -> String {
    dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Query<SortQuery>, _>()
        .await
        .ok()
        .and_then(|axum::extract::Query(q)| q.sort)
        .unwrap_or_default()
}

/// Lawyer jurisdictions directory — the reference table of jurisdictions
/// (name + code), ordered by code as the page was. Gate first, then read,
/// then project.
#[server]
pub async fn list_jurisdictions() -> Result<AdminListingView, ServerFnError> {
    // Gate before touching the query, so a non-lawyer caller never
    // triggers it.
    let role = crate::admin_listing::require_lawyer().await?;

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let mut rows = store::jurisdictions::list_all(&surreal)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    rows.sort_by(|a, b| a.code.cmp(&b.code));

    Ok(crate::admin_listing::view(
        role,
        "Lawyer | Jurisdictions",
        "Jurisdictions",
        &["Name", "Code"],
        rows.into_iter().map(|j| vec![j.name, j.code]).collect(),
    )
    .await)
}

/// Lawyer jurisdictions directory component.
#[component]
pub fn LawyerJurisdictions() -> Element {
    let resource = use_server_future(list_jurisdictions)?;
    render_resource(&resource)
}

/// Lawyer git-repositories directory — the tracked repositories (remote hash +
/// last commit SHA). Gate first, then read, then project.
#[server]
pub async fn list_git_repositories() -> Result<AdminListingView, ServerFnError> {
    // Gate before touching the query, so a non-lawyer caller never
    // triggers it.
    let role = crate::admin_listing::require_lawyer().await?;

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let rows = store::git_repositories::list_all(&surreal)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(crate::admin_listing::view(
        role,
        "Lawyer | Git repositories",
        "Git repositories",
        &["Remote hash", "Last commit SHA"],
        rows.into_iter()
            .map(|g| vec![g.remote_hash, g.last_commit_sha])
            .collect(),
    )
    .await)
}

/// Lawyer git-repositories directory component.
#[component]
pub fn LawyerGitRepositories() -> Element {
    let resource = use_server_future(list_git_repositories)?;
    render_resource(&resource)
}

/// Lawyer person-entity roles directory — the person↔entity role assignments
/// (person, entity, role).
///
/// **Firm-wide on purpose, for the whole lawyer tier including a lawyer on no
/// matters.** Do not scope this for consistency with `/lawyer/answers` and
/// `/lawyer/assets`. `entity_role` is one of the two edges
/// `store::conflicts::check_new_matter` traverses (`<->entity_role` and
/// `<->relationship`), and ABA Model Rule 1.10 imputes one lawyer's conflict to
/// the entire firm — a conflict that surfaces through a matter the checker is
/// not on is still the firm's conflict. Scoping these rows would narrow the
/// traversal to the checker's own caseload. Pinned by
/// `conflict_graph_listings_stay_firm_wide_for_an_unparticipating_lawyer`.
///
/// The ties live in the `entity_role` relation (ENG-120). Gate first, then
/// read, then project.
#[server]
pub async fn list_person_entity_roles() -> Result<AdminListingView, ServerFnError> {
    // Gate before touching the query, so a non-lawyer caller never
    // triggers it.
    let role = crate::admin_listing::require_lawyer().await?;

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let rows = store::entity_roles::all(&surreal)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(crate::admin_listing::view(
        role,
        "Lawyer | Person-entity roles",
        "Person-entity roles",
        &["Person", "Entity", "Role"],
        rows.into_iter()
            .map(|tie| {
                vec![
                    tie.person_id.to_string(),
                    tie.entity_id.to_string(),
                    tie.role,
                ]
            })
            .collect(),
    )
    .await)
}

/// Lawyer person-entity roles directory component.
#[component]
pub fn LawyerPersonEntityRoles() -> Element {
    let resource = use_server_future(list_person_entity_roles)?;
    render_resource(&resource)
}

/// Lawyer notations directory. Gate first, then read, then project.
#[server]
pub async fn list_notations() -> Result<AdminListingView, ServerFnError> {
    // Gate before touching the query, so a non-lawyer caller never
    // triggers it.
    let role = crate::admin_listing::require_lawyer().await?;

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let rows = store::notations::list_all(&surreal)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(crate::admin_listing::view(
        role,
        "Lawyer | Notations",
        "Notations",
        &["Template", "Person", "Entity", "State"],
        rows.into_iter()
            .map(|n| {
                vec![
                    n.template_id.to_string(),
                    n.person_id.to_string(),
                    n.entity_id.map_or("—".into(), |x| x.to_string()),
                    n.state,
                ]
            })
            .collect(),
    )
    .await)
}

/// Lawyer notations directory component.
#[component]
pub fn LawyerNotations() -> Element {
    let resource = use_server_future(list_notations)?;
    render_resource(&resource)
}

/// Lawyer answers directory — **matter content**, scoped to the caller's
/// participation ledger (ENG-303). A raw questionnaire answer is the client's
/// own words, so a lawyer reads only the matters they are on; Owner and Admin
/// read every row.
///
/// Gate first, then read, then scope, then project.
#[server]
pub async fn list_answers() -> Result<AdminListingView, ServerFnError> {
    // Resolve the handle first, because the gate needs it to read the
    // participation ledger — but the gate still runs before the listing's own
    // query, so a non-lawyer caller never triggers it.
    let surreal = consume_context::<store::surreal::SurrealDb>();
    let (role, scope) = crate::admin_listing::require_lawyer_in_matters(&surreal).await?;

    let mut rows = store::answers::list_all(&surreal)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    // `answer` carries no `project_id`. It reaches a matter only through
    // `notation_id → notation.project_id` — the hop `store::contract_reviews`
    // documents for the same reason. Resolve the caller's visible notations
    // once rather than per row; an answer whose `notation_id` is NONE has no
    // path to a matter at all and so fails closed.
    if let crate::admin_listing::MatterScope::Participating(visible) = &scope {
        let project_ids: Vec<uuid::Uuid> = visible.iter().copied().collect();
        let project_of_notation: std::collections::HashMap<uuid::Uuid, uuid::Uuid> =
            store::notations::list_by_projects(&surreal, &project_ids)
                .await
                .map_err(|e| ServerFnError::new(e.to_string()))?
                .into_iter()
                .map(|notation| (notation.id, notation.project_id))
                .collect();
        scope.retain(&mut rows, |answer| {
            answer
                .notation_id
                .and_then(|id| project_of_notation.get(&id).copied())
        });
    }

    Ok(crate::admin_listing::view(
        role,
        "Lawyer | Answers",
        "Answers",
        &["Question", "Person", "Value"],
        rows.into_iter()
            .map(|a| {
                vec![
                    a.question_id.to_string(),
                    a.person_id.to_string(),
                    store::answers::display_value(&a.value),
                ]
            })
            .collect(),
    )
    .await)
}

/// Lawyer answers directory component.
#[component]
pub fn LawyerAnswers() -> Element {
    let resource = use_server_future(list_answers)?;
    render_resource(&resource)
}

/// Lawyer addresses directory. The table lives in `SurrealDB` (ENG-20).
/// Gate first, then read, then project.
#[server]
pub async fn list_addresses() -> Result<AdminListingView, ServerFnError> {
    // Gate before touching the query, so a non-lawyer caller never
    // triggers it.
    let role = crate::admin_listing::require_lawyer().await?;

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let rows = store::addresses::list_all(&surreal)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(crate::admin_listing::view(
        role,
        "Lawyer | Addresses",
        "Addresses",
        &["Owner", "Line 1", "City", "Region", "Country"],
        rows.into_iter()
            .map(|a| {
                let owner = a.person_id.map_or_else(
                    || a.entity_id.map_or("—".into(), |id| format!("entity/{id}")),
                    |id| format!("person/{id}"),
                );
                vec![owner, a.line1, a.city, a.region, a.country]
            })
            .collect(),
    )
    .await)
}

/// Lawyer addresses directory component.
#[component]
pub fn LawyerAddresses() -> Element {
    let resource = use_server_future(list_addresses)?;
    render_resource(&resource)
}

/// Lawyer assets directory — **matter content**, scoped to the caller's
/// participation ledger (ENG-303). A storage key carries its matter's prefix
/// and a filename names the document, so a lawyer reads only the matters they
/// are on; Owner and Admin read every row.
///
/// Gate first, then read, then scope, then project.
#[server]
pub async fn list_assets() -> Result<AdminListingView, ServerFnError> {
    // Resolve the handle first, because the gate needs it to read the
    // participation ledger — but the gate still runs before the listing's own
    // query, so a non-lawyer caller never triggers it.
    let surreal = consume_context::<store::surreal::SurrealDb>();
    let (role, scope) = crate::admin_listing::require_lawyer_in_matters(&surreal).await?;

    let mut rows = store::assets::list_all(&surreal)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    // `asset.project_id` is `option<record<project>>`: a bare content asset
    // belongs to no matter, and a scoped read leaves it out rather than
    // guessing.
    scope.retain(&mut rows, |asset| asset.project_id);

    Ok(crate::admin_listing::view(
        role,
        "Lawyer | Assets",
        "Assets",
        &[
            "Storage key",
            "Filename",
            "Kind",
            "Content type",
            "Bytes",
            "SHA-256",
        ],
        rows.into_iter()
            .map(|a| {
                vec![
                    a.storage_key,
                    a.filename.unwrap_or_default(),
                    a.kind.unwrap_or_default(),
                    a.content_type,
                    a.byte_size.to_string(),
                    a.sha256_hex,
                ]
            })
            .collect(),
    )
    .await)
}

/// Lawyer assets directory component.
#[component]
pub fn LawyerAssets() -> Element {
    let resource = use_server_future(list_assets)?;
    render_resource(&resource)
}

/// Lawyer person-project roles directory.
#[server]
pub async fn list_person_project_roles() -> Result<AdminListingView, ServerFnError> {
    let role = crate::admin_listing::require_lawyer().await?;
    let surreal = consume_context::<store::surreal::SurrealDb>();
    let rows = store::projects::all_participations(&surreal)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(crate::admin_listing::view(
        role,
        "Lawyer | Person-project roles",
        "Person-project roles",
        &["Person", "Project", "Participation"],
        rows.into_iter()
            .map(|r| {
                vec![
                    r.person_id.to_string(),
                    r.project_id.to_string(),
                    r.participation,
                ]
            })
            .collect(),
    )
    .await)
}

/// Lawyer person-project roles directory component.
#[component]
pub fn LawyerPersonProjectRoles() -> Element {
    let resource = use_server_future(list_person_project_roles)?;
    render_resource(&resource)
}

/// Lawyer disclosures directory — **firm-wide on purpose, for the whole lawyer
/// tier including a lawyer on no matters.**
///
/// Do not scope this for consistency with `/lawyer/answers` and
/// `/lawyer/assets`. The disclosures table feeds
/// `store::conflicts::check_new_matter`, and ABA Model Rule 1.10 imputes one
/// lawyer's conflict to the entire firm — so a lawyer running a conflict check
/// must be able to see a conflict arising out of a matter they are not on.
/// Filtering these rows by the checker's own participation would narrow the
/// conflict check to the checker's own caseload, which is the exact failure the
/// imputation rule exists to prevent. Pinned by
/// `conflict_graph_listings_stay_firm_wide_for_an_unparticipating_lawyer`.
#[server]
pub async fn list_disclosures() -> Result<AdminListingView, ServerFnError> {
    crate::admin_listing::load_surreal(
        |surreal| async move {
            store::disclosures::all(&surreal)
                .await
                .map_err(|e| ServerFnError::new(e.to_string()))
        },
        "Lawyer | Disclosures",
        "Disclosures",
        &["Entity", "Project", "Kind", "Summary"],
        |d| {
            vec![
                d.entity_id.map_or("—".into(), |x| x.to_string()),
                d.project_id.map_or("—".into(), |x| x.to_string()),
                d.kind,
                d.summary,
            ]
        },
    )
    .await
}

/// Lawyer disclosures directory component.
#[component]
pub fn LawyerDisclosures() -> Element {
    let resource = use_server_future(list_disclosures)?;
    render_resource(&resource)
}

/// Lawyer relationship logs directory — **matter content**, scoped to the
/// caller's participation ledger (ENG-303).
///
/// This one is not a conflict-graph input, and that is what decides it.
/// `store::relationship_logs` says the trail is one-sided and "the conflict
/// traversal never reads it", so the Model Rule 1.10 imputation that keeps
/// `/lawyer/disclosures` and `/lawyer/person-entity-roles` firm-wide has
/// nothing to say here. What the trail *does* hold is per-matter: every live
/// writer — `store::projects`, `store::project_modules`, and
/// `store::participation` — stamps `subject_type = "project"` with the matter's
/// id, and the matter-open writer puts `conflict.summary_lines()` in `detail`,
/// which names adverse parties in prose. So it is scoped, and the
/// `subject_id → project` link the writers already set is what scopes it, with
/// no schema change.
///
/// An entry whose `subject_type` is something other than `"project"` names no
/// matter and fails closed.
///
/// The trail reads newest-first. Gate first, then read, then scope, then
/// project.
#[server]
pub async fn list_relationship_logs() -> Result<AdminListingView, ServerFnError> {
    // Resolve the handle first, because the gate needs it to read the
    // participation ledger — but the gate still runs before the listing's own
    // query, so a non-lawyer caller never triggers it.
    let surreal = consume_context::<store::surreal::SurrealDb>();
    let (role, scope) = crate::admin_listing::require_lawyer_in_matters(&surreal).await?;

    let mut rows = store::relationship_logs::all(&surreal)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    scope.retain(&mut rows, |log| {
        (log.subject_type == "project").then_some(log.subject_id)
    });

    Ok(crate::admin_listing::view(
        role,
        "Lawyer | Relationship logs",
        "Relationship logs",
        &["Actor", "Subject type", "Subject", "Action", "Detail"],
        rows.into_iter()
            .map(|log| {
                vec![
                    log.actor_person_id
                        .map_or("—".into(), |actor| actor.to_string()),
                    log.subject_type,
                    log.subject_id.to_string(),
                    log.action,
                    log.detail,
                ]
            })
            .collect(),
    )
    .await)
}

/// Lawyer relationship logs directory component.
#[component]
pub fn LawyerRelationshipLogs() -> Element {
    let resource = use_server_future(list_relationship_logs)?;
    render_resource(&resource)
}

/// Lawyer mailrooms directory. Each row resolves its address through an in-memory
/// join (the handler did the same), so it builds rows itself and hands them
/// to `admin_listing::view` rather than the single-entity `load`.
#[server]
pub async fn list_mailrooms() -> Result<AdminListingView, ServerFnError> {
    let role = crate::admin_listing::require_lawyer().await?;
    let surreal = consume_context::<store::surreal::SurrealDb>();
    let mailrooms = store::mailrooms::list_all(&surreal)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    // Both tables are single-engine now, so the in-memory join is one
    // engine's data rather than two.
    let addresses = store::addresses::list_all(&surreal)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let by_address = |id| {
        addresses.iter().find(|a| a.id == id).map_or_else(
            || format!("(unknown address #{id})"),
            |a| format!("{}, {}, {}", a.line1, a.city, a.region),
        )
    };
    let rows = mailrooms
        .into_iter()
        .map(|m| vec![m.name, by_address(m.address_id)])
        .collect();
    Ok(crate::admin_listing::view(
        role,
        "Lawyer | Mailrooms",
        "Mailrooms",
        &["Name", "Address"],
        rows,
    )
    .await)
}

/// Lawyer mailrooms directory component.
#[component]
pub fn LawyerMailrooms() -> Element {
    let resource = use_server_future(list_mailrooms)?;
    render_resource(&resource)
}

/// Letters directory — **Owner/Admin only** (ENG-303). Each row resolves its
/// mailroom through an in-memory join, so it builds rows itself and hands them
/// to `admin_listing::view`.
///
/// This is matter content — sender, recipient, and summary of correspondence in
/// both directions — but `letter` carries no link to a project to scope it by.
/// Its only link is `mailroom_id`, and a mailroom is a physical address, not a
/// matter. So the interim close is the admin gate rather than participation
/// scoping: it stops the disclosure today with no schema change, at the cost of
/// a firm-wide view a Lawyer arguably never should have had. Adding
/// `letter.project_id` plus a backfill is the real fix and is tracked
/// separately.
#[server]
pub async fn list_letters() -> Result<AdminListingView, ServerFnError> {
    let role = crate::admin_listing::require_admin().await?;
    let surreal = consume_context::<store::surreal::SurrealDb>();
    let letters = store::letters::list_all(&surreal)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    // Both tables are single-engine now, so the in-memory join is one
    // engine's data rather than two.
    let mailrooms = store::mailrooms::list_all(&surreal)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let by_mailroom = |id| {
        mailrooms
            .iter()
            .find(|m| m.id == id)
            .map_or_else(|| format!("(unknown #{id})"), |m| m.name.clone())
    };
    let rows = letters
        .into_iter()
        .map(|l| {
            vec![
                by_mailroom(l.mailroom_id),
                l.direction,
                l.sender,
                l.recipient,
                l.summary,
            ]
        })
        .collect();
    Ok(crate::admin_listing::view(
        role,
        "Lawyer | Letters",
        "Letters",
        &["Mailroom", "Direction", "Sender", "Recipient", "Summary"],
        rows,
    )
    .await)
}

/// Lawyer letters directory component.
#[component]
pub fn LawyerLetters() -> Element {
    let resource = use_server_future(list_letters)?;
    render_resource(&resource)
}

/// The email-log `?page=` query. 1-indexed; defaults to page 1.
#[derive(serde::Deserialize, Default)]
pub struct EmailLogQuery {
    #[serde(default)]
    pub page: Option<u64>,
}

/// How many `sent_emails` rows the email log shows per page.
#[cfg(feature = "server")]
const EMAIL_LOG_PER_PAGE: u64 = 50;

/// Email log — **Owner/Admin only** (ENG-303). A read-only,
/// `?page=`-paginated audit view over `sent_emails`, newest first, metadata
/// only (the body is intentionally not shown). Unlike the other listings it
/// carries pagination, so it sets the view's `PageState` after building its
/// rows.
///
/// Recipient, subject, and sender of every message the deployment has sent is
/// matter content, but `sent_email` carries no project link at all — not even
/// an indirect one — so there is nothing to scope by. Same interim close as
/// `/app/admin/letters`: the admin gate now, a real `project_id` and backfill in
/// its own issue.
#[server]
pub async fn list_email_log() -> Result<AdminListingView, ServerFnError> {
    let role = crate::admin_listing::require_admin().await?;
    let axum::extract::Query(query) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Query<EmailLogQuery>, _>(
        )
        .await?;
    let requested_page = query.page.unwrap_or(1).max(1);

    let db = consume_context::<store::surreal::SurrealDb>();
    // The count, the clamp, and the fetch are one statement batch inside
    // `store::sent_emails::page`, which SurrealDB runs as one transaction, so
    // all three read one snapshot. Without it, a row logged between the count
    // and the fetch pushes
    // onto page 1 and shifts the rest down, stranding the oldest row on an
    // unreachable page N+1 under a pager that shows no Next link.
    let store::sent_emails::Page {
        rows: rows_raw,
        total_pages,
        page,
    } = store::sent_emails::page(&db, requested_page, EMAIL_LOG_PER_PAGE)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let rows = rows_raw
        .into_iter()
        .map(|r| {
            vec![
                r.sent_at.to_rfc3339(),
                r.recipient,
                r.subject,
                r.sender,
                r.template_slug.unwrap_or_else(|| "—".to_string()),
                r.outcome,
            ]
        })
        .collect();

    let mut view = crate::admin_listing::view(
        role,
        "Lawyer | Email log",
        "Email log",
        &[
            "Sent at",
            "Recipient",
            "Subject",
            "From",
            "Template",
            "Outcome",
        ],
        rows,
    )
    .await;
    view.subtitle = Some(
        "Every outbound message that went through the SendGrid path. Gmail mail \
         from Workspace mailboxes is intentionally not logged here."
            .to_string(),
    );
    view.pagination = Some(crate::admin_listing::PageState {
        current: u32::try_from(page).unwrap_or(u32::MAX),
        total: u32::try_from(total_pages).unwrap_or(u32::MAX),
        base_path: "/app/admin/email-log".to_string(),
    });
    Ok(view)
}

/// Lawyer email log component.
#[component]
pub fn LawyerEmailLog() -> Element {
    let resource = use_server_future(list_email_log)?;
    render_resource(&resource)
}

/// Lawyer templates catalog — the public template catalog (code / title /
/// respondent type), sortable by code, title, and respondent type.
///
/// Project-scoped templates are deliberately hidden: this is the shared
/// catalog, and a matter's own templates belong to that matter.
#[server]
pub async fn list_templates() -> Result<AdminListingView, ServerFnError> {
    // Gate before touching the query, so a non-lawyer caller never
    // triggers it.
    let role = crate::admin_listing::require_lawyer().await?;
    let sort = requested_sort().await;

    let surreal = consume_context::<store::surreal::SurrealDb>();
    // `list_current` is already the shared catalog plus every Project's own
    // current rows; the filter below drops the scoped ones, which is what
    // the `project_id IS NULL` predicate did.
    let rows = store::templates::list_current(&surreal)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(crate::admin_listing::sorted_view(
        role,
        "Lawyer | Templates",
        "Templates",
        &["Code", "Title", "Respondent type"],
        &crate::admin_listing::PortedSort {
            keys: &["code", "title", "respondent_type"],
            active: &sort,
            base_path: "/app/admin/templates",
        },
        rows.into_iter()
            .filter(|t| t.project_id.is_none())
            .map(|t| vec![t.code, t.title, t.respondent_type])
            .collect(),
    )
    .await)
}

/// Lawyer templates catalog component.
#[component]
pub fn LawyerTemplates() -> Element {
    let resource = use_server_future(list_templates)?;
    render_resource(&resource)
}

/// Lawyer questions directory — the seeded questionnaire questions, sortable by
/// code and answer type.
///
/// Questions are seeded from template frontmatter by `cli import`, so this is a
/// transparency surface only: no add / edit / delete.
/// The view is assembled through [`crate::admin_listing::sorted_view`]: gate
/// first, then read, then project, then order.
#[server]
pub async fn list_questions() -> Result<AdminListingView, ServerFnError> {
    // Gate before touching the query, so a non-lawyer caller never
    // triggers it.
    let role = crate::admin_listing::require_lawyer().await?;
    let sort = requested_sort().await;

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let rows = store::questions::list_all(&surreal)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(crate::admin_listing::sorted_view(
        role,
        "Lawyer | Questions",
        "Questions",
        &["Code", "Prompt", "Answer type"],
        &crate::admin_listing::PortedSort {
            // The prompt is free prose; sorting by it says nothing useful,
            // so its key is empty and the header stays fixed.
            keys: &["code", "", "answer_type"],
            active: &sort,
            base_path: "/app/admin/questions",
        },
        rows.into_iter()
            .map(|q| vec![q.code, q.prompt, q.answer_type])
            .collect(),
    )
    .await)
}

/// Lawyer questions directory component.
#[component]
pub fn LawyerQuestions() -> Element {
    let resource = use_server_future(list_questions)?;
    render_resource(&resource)
}
