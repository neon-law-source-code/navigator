//! Lawyer entities list as a Dioxus component (#641 Phase 3, admin cluster) — the
//! entity CRUD read view, completing the entities cluster.
//!
//! A sortable table of entities (name, resolved type, resolved jurisdiction)
//! with a per-row Edit link and, for deletable rows, a Delete `POST` form
//! carrying the session CSRF token — combining the sortable-listing seam (a
//! `SortSpec`-validated `?sort=`) with the row-action form seam. The resolved
//! type/jurisdiction name columns are not database columns, so all three sort
//! columns (`name` included) are composed in one in-memory comparator that
//! keeps the first requested `?sort=` field primary. The firm anchor (the
//! bootstrap company) cannot be deleted, so its row shows no Delete action.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Column, DataTable, SortState};
use crate::people::ViewerRole;

/// One entity row, in a wasm-safe shape: the id (for the action URLs), the
/// display fields, and whether the row may be deleted.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct EntityRow {
    pub id: String,
    pub name: String,
    pub entity_type: String,
    pub jurisdiction: String,
    pub can_delete: bool,
}

/// The rendered entities list: the rows, the active `?sort=`, the session CSRF
/// token (for the per-row Delete forms), and the viewer's tier.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct EntityListView {
    /// The resolved brand's tokens stylesheet href, so the page wears
    /// its own palette rather than the firm's on a non-default host.
    #[serde(default)]
    pub tokens_href: String,
    pub rows: Vec<EntityRow>,
    pub sort: String,
    pub csrf_token: String,
    pub role: ViewerRole,
    /// The `?error=` flash surfaced above the table — set when a per-row Delete
    /// is refused (a dependent record still references the entity) and the
    /// handler redirects back here. `None` on a plain visit.
    #[serde(default)]
    pub error: Option<String>,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// Whether `name` is the firm anchor (bootstrap company), which may not be
/// deleted — the same rule the handler applied, self-contained here from
/// `store::seed::FIRM_ENTITY_NAME` and the `NAVIGATOR_BOOTSTRAP_COMPANY`
/// override rather than portal's `AdminState`.
#[cfg(feature = "server")]
fn is_bootstrap_company(name: &str) -> bool {
    let default = store::seed::FIRM_ENTITY_NAME;
    let configured = std::env::var("NAVIGATOR_BOOTSTRAP_COMPANY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_string());
    configured.eq_ignore_ascii_case(name) || default.eq_ignore_ascii_case(name)
}

/// Fetch the entities list for the current request: refuse non-lawyer, read the
/// injected CSRF token and `?sort=` (validated by the route pre-handler), load
/// the entities and the type/jurisdiction lookups, resolve the two foreign-key
/// name columns, sort in memory (one composite comparator so the first
/// requested `?sort=` field is primary), and mark each row deletable unless it
/// is the firm anchor.
#[server]
pub async fn get_entity_list() -> Result<EntityListView, ServerFnError> {
    let role = crate::admin_listing::require_lawyer().await?;
    let csrf_token = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::csrf::CsrfToken>,
        _,
    >()
    .await
    .map(|axum::Extension(token)| token.0)
    .unwrap_or_default();
    let axum::extract::Query(query) = dioxus_fullstack_core::FullstackContext::extract::<
        axum::extract::Query<EntityListQuery>,
        _,
    >()
    .await?;
    let sort = query.sort.unwrap_or_default();
    let parsed = parse_sort(&sort);

    // Both reference tables live in SurrealDB (ENG-20).
    let surreal = consume_context::<store::surreal::SurrealDb>();
    let types = store::entity_types::list(&surreal, &[])
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let jurs = store::jurisdictions::list_all(&surreal)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    // The resolved type/jurisdiction names are not database columns, so `name`
    // is sorted in memory alongside them (below) under one comparator rather
    // than pushed into the store separately. A database sort followed by
    // per-field in-memory sorts would make the last field primary and reverse
    // the requested precedence.
    let mut rows_raw = store::entities::all(&surreal)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let by_type = |id| {
        types
            .iter()
            .find(|t| t.id == id)
            .map_or("?", |t| t.name.as_str())
            .to_string()
    };
    let by_jur = |id| {
        jurs.iter()
            .find(|j| j.id == id)
            .map_or("?", |j| j.name.as_str())
            .to_string()
    };
    // Build one composite comparator so the first requested field is primary and
    // later fields only break ties, per the JSON:API `SortSpec` contract. A
    // sequence of separate stable sorts would instead make the last field
    // primary, reversing precedence.
    rows_raw.sort_by(|a, b| {
        parsed
            .iter()
            .fold(std::cmp::Ordering::Equal, |acc, (key, descending)| {
                acc.then_with(|| {
                    let ordering = match key.as_str() {
                        "name" => a.name.cmp(&b.name),
                        "entity_type" => by_type(a.entity_type_id).cmp(&by_type(b.entity_type_id)),
                        "jurisdiction" => by_jur(a.jurisdiction_id).cmp(&by_jur(b.jurisdiction_id)),
                        _ => std::cmp::Ordering::Equal,
                    };
                    if *descending {
                        ordering.reverse()
                    } else {
                        ordering
                    }
                })
            })
    });

    let rows = rows_raw
        .into_iter()
        .map(|e| EntityRow {
            entity_type: by_type(e.entity_type_id),
            jurisdiction: by_jur(e.jurisdiction_id),
            can_delete: !is_bootstrap_company(&e.name),
            id: e.id.to_string(),
            name: e.name,
        })
        .collect();

    Ok(EntityListView {
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
        firm_name: crate::app_chrome::firm_name_from_context().await,
        rows,
        sort,
        csrf_token,
        role,
        error: query.error.filter(|message| !message.is_empty()),
    })
}

/// The entities list `?sort=` query, plus the `?error=` flash a refused
/// per-row Delete redirects back with.
#[derive(Deserialize, Default)]
pub struct EntityListQuery {
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
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

/// The lawyer entities list. Server-side rendered with the sorted rows already in
/// the markup; the sort headers are real anchors, the Edit link and the Delete
/// `POST` form are native (work without JavaScript).
#[component]
pub fn LawyerEntityList() -> Element {
    let resource = use_server_future(get_entity_list)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "entities", p { "Failed to load entities." } }
            }
        }
        None => {
            return rsx! {
                main { id: "entities", p { "Loading…" } }
            }
        }
    };

    let sort = SortState::parse(Some(&view.sort));
    let columns = vec![
        Column::sortable("name", "Name"),
        Column::sortable("entity_type", "Type"),
        Column::sortable("jurisdiction", "Jurisdiction"),
        Column::fixed("actions", "Actions"),
    ];
    let csrf = view.csrf_token.clone();
    let error = view.error.clone();
    let is_empty = view.rows.is_empty();

    rsx! {
        document::Title { "{view.firm_name} | Lawyer | Entities" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        nav { class: "lawyer-nav",
            a { class: "nav-link", href: "/app/projects", "Projects" }
            a { class: "nav-link", href: "/auth/logout", "Sign out" }
        }
        main { id: "entities", class: "nav-theme",
            header { class: "page-header",
                h1 { "Entities" }
                p { a { class: "nav-btn nav-btn--primary", href: "/app/admin/entities/new", "Add entity" } }
            }
            if let Some(error) = error.as_ref() {
                p { class: "nav-form-error", role: "alert", "{error}" }
            }
            if is_empty {
                p { class: "entities-empty",
                    "No entities yet. "
                    a { href: "/app/admin/entities/new", "Add the first." }
                }
            } else {
                DataTable {
                    columns,
                    sort,
                    base_path: "/app/admin/entities".to_string(),
                    for row in view.rows.iter() {
                        tr { class: "entity-row",
                            td { class: "entity-name", "{row.name}" }
                            td { class: "entity-type", "{row.entity_type}" }
                            td { class: "entity-jurisdiction", "{row.jurisdiction}" }
                            td { class: "entity-actions",
                                a { class: "nav-link", href: "/app/admin/entities/{row.id}/edit", "Edit" }
                                if row.can_delete {
                                    " "
                                    form {
                                        class: "d-inline",
                                        method: "post",
                                        action: "/app/admin/entities/{row.id}/delete",
                                        input { r#type: "hidden", name: "_csrf", value: "{csrf}" }
                                        button {
                                            class: "nav-btn nav-btn--danger",
                                            r#type: "submit",
                                            "Delete"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
