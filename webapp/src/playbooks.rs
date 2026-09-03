//! Lawyer contract-negotiation playbooks as Dioxus components (#956 Phase 4) —
//! the list, the create form, and the edit-positions form.
//!
//! A **playbook** is the set of negotiating positions a client Entity has
//! decided it wants — the yardstick the inbound-contract review measures a
//! third-party contract against. Each position is a row of
//! `topic | preferred | fallback | walk-away | severity`. Both forms carry the
//! whole position set in one textarea (one position per line, pipe-delimited)
//! so an attorney edits the playbook as a block.
//!
//! The successor to the `views::pages::admin::playbooks`. The three `GET`
//! renders live here; `POST /app/admin/playbooks` (create) and `POST
//! /app/admin/playbooks/{id}` (update) stay on `portal::admin_playbooks`, which axum
//! merges onto the same paths. Those handlers follow post/redirect/get: a
//! refusal redirects back to the form carrying its message as `?error=` **and
//! the rejected positions text**, which these loaders overlay onto the stored
//! row. A position set is dozens of hand-authored lines, so reloading the stored
//! row after a typo'd severity would silently discard the whole block.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{
    Choice, Column, DataTable, Field, FormCard, Heading, RowActions, SortState,
};
use crate::people::ViewerRole;

/// The pipe-delimited textarea contract, shown as form help so an attorney
/// knows the line shape.
pub const POSITIONS_HELP: &str =
    "One position per line: Topic | Preferred | Fallback | Walk-away | severity \
     (severity is low, medium, or high).";

/// One `<select>` option on the create form: the Entity id and its name.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct CompanyChoice {
    pub id: String,
    pub name: String,
}

// ---- list -----------------------------------------------------------------

/// One playbook row, in a wasm-safe shape: the id (for the edit URL), the
/// resolved company name, the playbook name, how many positions it holds, and
/// whether it is applied to new reviews.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct PlaybookRow {
    pub id: String,
    pub entity_name: String,
    pub name: String,
    pub position_count: usize,
    pub active: bool,
}

/// The rendered playbooks list: the rows, the active `?sort=`, and the viewer's
/// tier. No CSRF token — the rows carry only an Edit link, no `POST` form.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct PlaybookListView {
    /// The resolved brand's tokens stylesheet href, so the page wears
    /// its own palette rather than the firm's on a non-default host.
    #[serde(default)]
    pub tokens_href: String,
    pub rows: Vec<PlaybookRow>,
    pub sort: String,
    pub role: ViewerRole,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// The playbooks list `?sort=` query. The route's pre-handler has already
/// refused anything outside the advertised set with a `400`.
#[derive(Deserialize, Default)]
pub struct PlaybookListQuery {
    #[serde(default)]
    pub sort: Option<String>,
}

/// Load the playbooks list: refuse non-lawyer, read the validated `?sort=`, load
/// every playbook and the company-name lookup, and sort in memory.
///
/// The company name is resolved from another table and the position count from
/// the JSONB blob, so neither is a database column to order by — one in-memory
/// comparator keeps the first requested `?sort=` field primary, as the JSON:API
/// `SortSpec` contract requires.
#[server]
pub async fn get_playbook_list() -> Result<PlaybookListView, ServerFnError> {
    let role = crate::admin_listing::require_lawyer().await?;
    let axum::extract::Query(query) = dioxus_fullstack_core::FullstackContext::extract::<
        axum::extract::Query<PlaybookListQuery>,
        _,
    >()
    .await?;
    let sort = query.sort.unwrap_or_default();

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let names = company_names(&surreal).await?;
    let mut rows: Vec<PlaybookRow> = store::playbooks::all(&surreal)
        .await
        .map_err(|e| server_error(&e))?
        .into_iter()
        .map(|p| PlaybookRow {
            entity_name: names
                .get(&p.entity_id)
                .cloned()
                .unwrap_or_else(|| "(unknown)".to_string()),
            position_count: store::playbooks::positions_of(&p).map_or(0, |v| v.len()),
            id: p.id.to_string(),
            name: p.name,
            active: p.active,
        })
        .collect();

    // Company then name is the default reading order — the page always
    // applied it. A `?sort=` the headers advertise now actually reorders the
    // table rather than only re-rendering the header arrow.
    let parsed = parse_sort(&sort);
    rows.sort_by(|a, b| {
        let requested = parsed
            .iter()
            .fold(std::cmp::Ordering::Equal, |acc, (key, descending)| {
                acc.then_with(|| {
                    let ordering = match key.as_str() {
                        "entity" => a.entity_name.cmp(&b.entity_name),
                        "name" => a.name.cmp(&b.name),
                        _ => std::cmp::Ordering::Equal,
                    };
                    if *descending {
                        ordering.reverse()
                    } else {
                        ordering
                    }
                })
            });
        requested
            .then_with(|| a.entity_name.cmp(&b.entity_name))
            .then_with(|| a.name.cmp(&b.name))
    });

    Ok(PlaybookListView {
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
        firm_name: crate::app_chrome::firm_name_from_context().await,
        rows,
        sort,
        role,
    })
}

/// Parse a JSON:API `sort` value into `(key, descending)` pairs. Server-only.
#[cfg(feature = "server")]
fn parse_sort(raw: &str) -> Vec<(String, bool)> {
    raw.split(',')
        .map(str::trim)
        .filter(|segment| !segment.is_empty() && *segment != "-")
        .map(|segment| match segment.strip_prefix('-') {
            Some(key) => (key.to_string(), true),
            None => (segment.to_string(), false),
        })
        .collect()
}

/// The lawyer playbooks list. Server-side rendered with the sorted rows already
/// in the markup; the sort headers and the per-row Edit link are real anchors.
#[component]
pub fn LawyerPlaybookList() -> Element {
    let resource = use_server_future(get_playbook_list)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "playbooks", p { "Failed to load playbooks." } }
            }
        }
        None => {
            return rsx! {
                main { id: "playbooks", p { "Loading…" } }
            }
        }
    };

    playbook_list_body(&view)
}

/// The loaded list. Split from the component so the tests render a fixed view
/// without standing up the server function.
fn playbook_list_body(view: &PlaybookListView) -> Element {
    let sort = SortState::parse(Some(&view.sort));
    let columns = vec![
        Column::sortable("entity", "Company"),
        Column::sortable("name", "Playbook"),
        Column::fixed("positions", "Positions"),
        Column::fixed("active", "Active"),
        Column::fixed("actions", ""),
    ];
    let rows = view.rows.clone();
    let role = view.role;

    rsx! {
        document::Title { "{view.firm_name} | Lawyer | Playbooks" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        LawyerNav { role }
        main { id: "playbooks", class: "nav-theme",
            header { class: "page-header",
                h1 { "Playbooks" }
                p { class: "nav-muted",
                    "The negotiating positions a Company's inbound contracts are measured against."
                }
                p {
                    a { class: "nav-btn nav-btn--primary", href: "/app/admin/playbooks/new",
                        "Add playbook"
                    }
                }
            }
            if rows.is_empty() {
                p { class: "playbooks-empty",
                    "No playbooks yet. "
                    a { href: "/app/admin/playbooks/new", "Add the first." }
                }
            } else {
                DataTable {
                    columns,
                    sort,
                    base_path: "/app/admin/playbooks".to_string(),
                    for row in rows.iter() {
                        tr { class: "playbook-row",
                            td { class: "playbook-entity", "{row.entity_name}" }
                            td { class: "playbook-name", "{row.name}" }
                            td { class: "playbook-positions", "{row.position_count}" }
                            td { class: "playbook-active",
                                if row.active { "Yes" } else { "No" }
                            }
                            td { class: "playbook-actions",
                                RowActions {
                                    edit_href: format!("/app/admin/playbooks/{}/edit", row.id),
                                    row_label: row.name.clone(),
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ---- create form ----------------------------------------------------------

/// The rendered "add playbook" form: the company options, the session CSRF
/// token, the viewer's tier, and any values a refused create bounced back.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct PlaybookNewView {
    /// The resolved brand's tokens stylesheet href, so the page wears
    /// its own palette rather than the firm's on a non-default host.
    #[serde(default)]
    pub tokens_href: String,
    pub companies: Vec<CompanyChoice>,
    pub csrf_token: String,
    pub role: ViewerRole,
    /// The `?error=` flash rendered above the form — set when `POST
    /// /app/admin/playbooks` refuses the create. `None` on a plain visit.
    #[serde(default)]
    pub error: Option<String>,
    /// The rejected submission, echoed back so a refusal costs one correction
    /// rather than retyping the whole position set.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub entity_id: String,
    #[serde(default)]
    pub positions: String,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// The "add playbook" form query: the `?error=` flash a refused create
/// redirects back with, plus the values it submitted.
#[derive(Deserialize, Default)]
pub struct PlaybookNewQuery {
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub entity_id: Option<String>,
    #[serde(default)]
    pub positions: Option<String>,
}

/// Load the "add playbook" form: refuse non-lawyer, read the injected CSRF token
/// and the bounced-back submission, and list the companies as `<select>`
/// options.
#[server]
pub async fn get_playbook_new_form() -> Result<PlaybookNewView, ServerFnError> {
    let role = crate::admin_listing::require_lawyer().await?;
    let csrf_token = csrf_token().await;
    let axum::extract::Query(query) = dioxus_fullstack_core::FullstackContext::extract::<
        axum::extract::Query<PlaybookNewQuery>,
        _,
    >()
    .await?;

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let companies = company_choices(&surreal).await?;

    Ok(PlaybookNewView {
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
        firm_name: crate::app_chrome::firm_name_from_context().await,
        companies,
        csrf_token,
        role,
        error: query.error.filter(|message| !message.is_empty()),
        name: query.name.unwrap_or_default(),
        entity_id: query.entity_id.unwrap_or_default(),
        positions: query.positions.unwrap_or_default(),
    })
}

/// The lawyer "add playbook" form. Server-side rendered as a native `POST` to
/// `/app/admin/playbooks` carrying the CSRF token, so it works without JavaScript.
#[component]
pub fn LawyerPlaybookNew() -> Element {
    let resource = use_server_future(get_playbook_new_form)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "playbook-new", p { "Failed to load the form." } }
            }
        }
        None => {
            return rsx! {
                main { id: "playbook-new", p { "Loading…" } }
            }
        }
    };

    playbook_new_body(&view)
}

/// The loaded create form. Split from the component so the tests render a fixed
/// view without standing up the server function.
fn playbook_new_body(view: &PlaybookNewView) -> Element {
    let mut options = vec![Choice::new("", "Choose…")];
    options.extend(
        view.companies
            .iter()
            .map(|c| Choice::new(c.id.clone(), c.name.clone())),
    );
    let selected = (!view.entity_id.is_empty()).then(|| view.entity_id.clone());
    let fields = vec![
        Field::select("Company", "entity_id", options, selected).required(),
        Field::text("Playbook name", "name", view.name.clone()).required(),
        // Never hand-roll the textarea: Dioxus renders it as RCDATA, so
        // hydration markers placed inside would be saved as playbook text.
        Field::textarea("Positions", "positions", view.positions.clone(), 10)
            .help(POSITIONS_HELP)
            .required(),
    ];
    let role = view.role;
    let error = view.error.clone();

    rsx! {
        document::Title { "{view.firm_name} | Lawyer | Playbooks | Add playbook" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        LawyerNav { role }
        main { id: "playbook-new", class: "nav-theme",
            if let Some(error) = error.as_ref() {
                p { class: "nav-form-error", role: "alert", "{error}" }
            }
            FormCard {
                title: "Add playbook".to_string(),
                action: "/app/admin/playbooks".to_string(),
                submit_label: "Create".to_string(),
                csrf_token: Some(view.csrf_token.clone()),
                fields,
            }
            p { a { href: "/app/admin/playbooks", "← Cancel" } }
        }
    }
}

// ---- edit form ------------------------------------------------------------

/// The playbook being edited: the fixed context (company + name) and the
/// position set the attorney is replacing.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct PlaybookFields {
    pub entity_name: String,
    pub name: String,
    pub positions: String,
}

/// The rendered "edit playbook" form: the playbook id (for the form action),
/// its fields (`None` when the id resolves to no row), the CSRF token, the
/// viewer's tier, and any `?error=` flash.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct PlaybookEditView {
    /// The resolved brand's tokens stylesheet href, so the page wears
    /// its own palette rather than the firm's on a non-default host.
    #[serde(default)]
    pub tokens_href: String,
    pub id: String,
    pub fields: Option<PlaybookFields>,
    pub csrf_token: String,
    pub role: ViewerRole,
    #[serde(default)]
    pub error: Option<String>,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// The "edit playbook" form query: the `?error=` flash a refused update
/// redirects back with, plus the positions it submitted.
#[derive(Deserialize, Default)]
pub struct PlaybookEditQuery {
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub positions: Option<String>,
}

/// Load the "edit playbook" form for the `{id}` in the request path: refuse
/// non-lawyer, read the CSRF token, load the playbook and its company name, and
/// overlay any positions a refused update bounced back.
#[server]
pub async fn get_playbook_edit_form() -> Result<PlaybookEditView, ServerFnError> {
    let role = crate::admin_listing::require_lawyer().await?;
    let axum::extract::Path(id) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Path<uuid::Uuid>, _>()
            .await?;
    let csrf_token = csrf_token().await;
    let axum::extract::Query(query) = dioxus_fullstack_core::FullstackContext::extract::<
        axum::extract::Query<PlaybookEditQuery>,
        _,
    >()
    .await?;

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let row = store::playbooks::by_id(&surreal, id)
        .await
        .map_err(|e| server_error(&e))?;

    let fields = match row {
        Some(row) => {
            let stored = store::playbooks::positions_of(&row).unwrap_or_default();
            let entity_name = company_names(&surreal)
                .await?
                .get(&row.entity_id)
                .cloned()
                .unwrap_or_else(|| "(unknown)".to_string());
            Some(PlaybookFields {
                entity_name,
                name: row.name,
                positions: query
                    .positions
                    .unwrap_or_else(|| store::playbooks::positions_to_text(&stored)),
            })
        }
        None => None,
    };

    // A valid UUID that resolves to no row is a missing resource: commit the
    // same 404 the retired handler returned, so a `#[server]` fallback
    // does not quietly serve it as a successful page.
    if fields.is_none() {
        dioxus_fullstack_core::FullstackContext::commit_http_status(
            axum::http::StatusCode::NOT_FOUND,
            None,
        );
    }

    Ok(PlaybookEditView {
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
        firm_name: crate::app_chrome::firm_name_from_context().await,
        id: id.to_string(),
        fields,
        csrf_token,
        role,
        error: query.error.filter(|message| !message.is_empty()),
    })
}

/// The lawyer "edit playbook" form — the company and name are fixed context; the
/// attorney replaces the position set. A native `POST` to
/// `/app/admin/playbooks/{id}` carrying the CSRF token.
#[component]
pub fn LawyerPlaybookEdit() -> Element {
    let resource = use_server_future(get_playbook_edit_form)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "playbook-edit", p { "Failed to load the playbook." } }
            }
        }
        None => {
            return rsx! {
                main { id: "playbook-edit", p { "Loading…" } }
            }
        }
    };

    playbook_edit_body(&view)
}

/// The loaded edit form (or the not-found state). Split from the component so
/// the tests render a fixed view without standing up the server function.
fn playbook_edit_body(view: &PlaybookEditView) -> Element {
    let view = view.clone();
    let role = view.role;
    let error = view.error.clone();

    rsx! {
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        LawyerNav { role }
        main { id: "playbook-edit", class: "nav-theme",
            if let Some(error) = error.as_ref() {
                p { class: "nav-form-error", role: "alert", "{error}" }
            }
            match view.fields {
                Some(fields) => {
                    let context = format!("{} — {}", fields.entity_name, fields.name);
                    let action = format!("/app/admin/playbooks/{}", view.id);
                    let form_fields = vec![
                        Field::textarea("Positions", "positions", fields.positions, 14)
                            .help(POSITIONS_HELP)
                            .required(),
                    ];
                    rsx! {
                        document::Title { "{view.firm_name} | Lawyer | Playbooks | Edit playbook" }
                        header { class: "page-header",
                            h1 { "Edit playbook" }
                            p { class: "nav-muted", "{context}" }
                        }
                        FormCard {
                            title: "Positions".to_string(),
                            action,
                            submit_label: "Save".to_string(),
                            heading: Heading::H2,
                            csrf_token: Some(view.csrf_token.clone()),
                            fields: form_fields,
                        }
                        p { a { href: "/app/admin/playbooks", "← Cancel" } }
                    }
                }
                None => rsx! {
                    document::Title { "{view.firm_name} | Lawyer | Playbooks | Not found" }
                    h1 { "Playbook not found" }
                    p { "No playbook exists with id " code { "{view.id}" } "." }
                    p { a { href: "/app/admin/playbooks", "← Back to playbooks" } }
                },
            }
        }
    }
}

// ---- shared ---------------------------------------------------------------

/// The lawyer chrome the three pages share.
#[component]
fn LawyerNav(role: ViewerRole) -> Element {
    rsx! {
        nav { class: "lawyer-nav",
            a { class: "nav-link", href: "/app/projects", "Portal" }
            if role.is_lawyer_tier() {
                a { class: "nav-link", href: "/app/lawyer", "Lawyer" }
            }
            if role.is_admin_tier() {
                a { class: "nav-link", href: "/app/admin", "Admin" }
            }
            a { class: "nav-link", href: "/auth/logout", "Sign out" }
        }
    }
}

/// The session CSRF token injected by the route's `inject_csrf_token` layer.
#[cfg(feature = "server")]
async fn csrf_token() -> String {
    dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<crate::csrf::CsrfToken>, _>()
        .await
        .map(|axum::Extension(token)| token.0)
        .unwrap_or_default()
}

/// Every Entity id to its name, for resolving each playbook's company.
#[cfg(feature = "server")]
async fn company_names(
    surreal: &store::surreal::SurrealDb,
) -> Result<std::collections::HashMap<uuid::Uuid, String>, ServerFnError> {
    Ok(store::entities::all(surreal)
        .await
        .map_err(|e| server_error(&e))?
        .into_iter()
        .map(|e| (e.id, e.name))
        .collect())
}

/// Every Entity as a `<select>` option for the create form.
#[cfg(feature = "server")]
async fn company_choices(
    surreal: &store::surreal::SurrealDb,
) -> Result<Vec<CompanyChoice>, ServerFnError> {
    Ok(store::entities::all(surreal)
        .await
        .map_err(|e| server_error(&e))?
        .into_iter()
        .map(|e| CompanyChoice {
            id: e.id.to_string(),
            name: e.name,
        })
        .collect())
}

/// A failed query is a server fault, not an empty playbook set: commit a real
/// `500` so the page cannot render "No playbooks yet." over a database that
/// never answered. An attorney reading that emptiness as "this Company has no
/// playbook on file" would be acting on a lie.
#[cfg(feature = "server")]
fn server_error(e: &dyn std::fmt::Display) -> ServerFnError {
    tracing::error!(error = %e, "lawyer: playbook query failed");
    dioxus_fullstack_core::FullstackContext::commit_http_status(
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        None,
    );
    ServerFnError::new(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::form::assert_forms_accessible;

    const ID: &str = "00000000-0000-0000-0000-000000000001";
    const COMPANY: &str = "00000000-0000-0000-0000-000000000009";

    fn row(entity_name: &str, name: &str, count: usize, active: bool) -> PlaybookRow {
        PlaybookRow {
            id: ID.to_string(),
            entity_name: entity_name.to_string(),
            name: name.to_string(),
            position_count: count,
            active,
        }
    }

    #[test]
    fn the_empty_list_points_at_the_create_form() {
        let html = dioxus_ssr::render_element(playbook_list_body(&PlaybookListView {
            tokens_href: String::new(),
            firm_name: "Neon Law".to_string(),
            rows: vec![],
            sort: String::new(),
            role: ViewerRole::Lawyer,
        }));
        assert!(html.contains("No playbooks yet."), "{html}");
        assert!(html.contains("/app/admin/playbooks/new"), "{html}");
    }

    #[test]
    fn a_listed_playbook_shows_its_company_count_and_edit_link() {
        let html = dioxus_ssr::render_element(playbook_list_body(&PlaybookListView {
            tokens_href: String::new(),
            firm_name: "Neon Law".to_string(),
            rows: vec![row("Acme Inc", "Vendor MSA", 4, true)],
            sort: String::new(),
            role: ViewerRole::Lawyer,
        }));
        // Dioxus SSR puts hydration markers between an attribute and its text,
        // so assert on the rendered text and the href separately.
        assert!(html.contains("Acme Inc"), "{html}");
        assert!(html.contains("Vendor MSA"), "{html}");
        assert!(html.contains(">4<"), "{html}");
        assert!(html.contains(">Yes<"), "{html}");
        assert!(
            html.contains(&format!("href=\"/app/admin/playbooks/{ID}/edit\"")),
            "{html}",
        );
    }

    #[test]
    fn the_create_form_meets_the_page_accessibility_invariants() {
        let html = dioxus_ssr::render_element(playbook_new_body(&PlaybookNewView {
            companies: vec![CompanyChoice {
                id: COMPANY.to_string(),
                name: "Acme Inc".to_string(),
            }],
            csrf_token: "TOK".to_string(),
            role: ViewerRole::Lawyer,
            ..PlaybookNewView::default()
        }));
        assert_forms_accessible(&html, "playbooks::LawyerPlaybookNew");
        assert!(html.contains("action=\"/app/admin/playbooks\""), "{html}");
        assert!(html.contains("One position per line"), "{html}");
        assert!(html.contains("value=\"TOK\""), "{html}");
        assert!(!html.contains("nav-form-error"), "{html}");
    }

    #[test]
    fn a_refused_create_shows_its_message_over_the_typed_positions() {
        // The whole position set is hand-authored, so a refusal that reloaded a
        // blank textarea would cost the attorney every line they entered.
        let typed = "Liability | mutual cap | 2x fees | uncapped | critical";
        let html = dioxus_ssr::render_element(playbook_new_body(&PlaybookNewView {
            tokens_href: String::new(),
            firm_name: "Neon Law".to_string(),
            companies: vec![CompanyChoice {
                id: COMPANY.to_string(),
                name: "Acme Inc".to_string(),
            }],
            csrf_token: "TOK".to_string(),
            role: ViewerRole::Lawyer,
            error: Some("Line 1: severity must be low, medium, or high.".to_string()),
            name: "Vendor MSA".to_string(),
            entity_id: COMPANY.to_string(),
            positions: typed.to_string(),
        }));
        assert!(html.contains("nav-form-error"), "{html}");
        assert!(html.contains("Line 1: severity must be"), "{html}");
        assert!(html.contains(typed), "{html}");
        assert!(html.contains("value=\"Vendor MSA\""), "{html}");
        // The company they picked is still selected, so only the severity needs
        // fixing.
        assert!(
            html.contains(&format!("value=\"{COMPANY}\" selected")),
            "{html}",
        );
    }

    #[test]
    fn the_edit_form_prefills_the_positions_under_fixed_context() {
        let stored = "Liability | mutual cap | 2x fees | uncapped | high";
        let html = dioxus_ssr::render_element(playbook_edit_body(&PlaybookEditView {
            tokens_href: String::new(),
            firm_name: "Neon Law".to_string(),
            id: ID.to_string(),
            fields: Some(PlaybookFields {
                entity_name: "Acme Inc".to_string(),
                name: "Vendor MSA".to_string(),
                positions: stored.to_string(),
            }),
            csrf_token: "TOK".to_string(),
            role: ViewerRole::Lawyer,
            error: None,
        }));
        assert_forms_accessible(&html, "playbooks::LawyerPlaybookEdit");
        assert!(
            html.contains(&format!("action=\"/app/admin/playbooks/{ID}\"")),
            "{html}",
        );
        assert!(html.contains("Acme Inc — Vendor MSA"), "{html}");
        assert!(html.contains(stored), "{html}");
        assert!(html.contains("value=\"TOK\""), "{html}");
    }

    #[test]
    fn an_unresolvable_id_offers_no_form_to_submit() {
        let html = dioxus_ssr::render_element(playbook_edit_body(&PlaybookEditView {
            tokens_href: String::new(),
            firm_name: "Neon Law".to_string(),
            id: ID.to_string(),
            fields: None,
            csrf_token: "TOK".to_string(),
            role: ViewerRole::Lawyer,
            error: None,
        }));
        assert!(html.contains("Playbook not found"), "{html}");
        assert!(!html.contains("<form"), "{html}");
    }
}
