//! Lawyer entity-types directory as a Dioxus component (#641 Phase 3, admin
//! cluster).
//!
//! The successor to the `views::pages::admin::entity_types` read view — the
//! first admin list page migrated to the Dioxus stack, following the
//! `/app/admin/people` (Tranche 1) seam: a `#[server]` function reads `?sort=` and
//! the injected `SurrealDb`, queries the shared `store::entity_types` command,
//! and `use_server_future` renders the sorted rows into the SSR HTML, readable
//! before hydration, with the sort header a real anchor. The URL contract holds:
//! the route pre-handler `400`s a `?sort=` naming anything but `name`.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Column, DataTable, SortState};
use crate::people::ViewerRole;

/// The entity-types list URL contract: JSON:API `?sort=` (only `name`).
#[derive(Deserialize, Default)]
pub struct EntityTypesQuery {
    #[serde(default)]
    pub sort: Option<String>,
}

/// One entity-type row, in a wasm-safe shape (no `store`/`SeaORM` types leak to
/// the client build).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct EntityTypeRow {
    pub name: String,
}

/// The rendered view: the rows, the active sort (for the header anchor), and the
/// viewer's tier.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct EntityTypesView {
    /// The resolved brand's tokens stylesheet href, so the page wears
    /// its own palette rather than the firm's on a non-default host.
    #[serde(default)]
    pub tokens_href: String,
    pub rows: Vec<EntityTypeRow>,
    pub sort: String,
    pub role: ViewerRole,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// Fetch the entity-types list for the current request: read `?sort=` and the
/// injected `store::surreal::SurrealDb` (the table lives in `SurrealDB` since
/// its ENG-20 slice), and query the shared `store::entity_types` command —
/// the same command boundary the REST API uses.
#[server]
pub async fn list_entity_types() -> Result<EntityTypesView, ServerFnError> {
    let axum::extract::Query(query) = dioxus_fullstack_core::FullstackContext::extract::<
        axum::extract::Query<EntityTypesQuery>,
        _,
    >()
    .await?;
    let sort = query.sort.unwrap_or_default();

    let role = dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<ViewerRole>, _>()
        .await
        .map(|axum::Extension(role)| role)
        .unwrap_or_default();

    // Defense in depth: the entity-types directory is lawyer-only. The page
    // router gates the route with `require_auth` + `require_policy` and injects
    // the viewer tier, but a direct request to the generated `#[server]`
    // endpoint need not carry that gate, and the injected role then defaults to
    // the least-privileged `Client`. Refuse any non-lawyer caller here, before
    // the query runs, so the loader never discloses the rows on its own
    // authority rather than trusting the route layers alone.
    if !role.is_lawyer_tier() {
        return Err(ServerFnError::new("lawyer access required"));
    }

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let rows = store::entity_types::list(&surreal, &parse_sort(&sort))
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(EntityTypesView {
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
        firm_name: crate::app_chrome::firm_name_from_context().await,
        rows: rows
            .into_iter()
            .map(|r| EntityTypeRow { name: r.name })
            .collect(),
        sort,
        role,
    })
}

/// Parse a JSON:API `sort` value into `(key, descending)` pairs. Server-only —
/// the client build stubs the server function that is its sole caller.
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

/// The lawyer entity-types directory. Server-side rendered with the rows already
/// in the markup (via [`use_server_future`]), readable before hydration; the
/// sort header is a real anchor.
#[component]
pub fn LawyerEntityTypes() -> Element {
    let resource = use_server_future(list_entity_types)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "lawyer-entity-types", p { "Failed to load entity types." } }
            }
        }
        None => {
            return rsx! {
                main { id: "lawyer-entity-types", p { "Loading…" } }
            }
        }
    };

    let sort = SortState::parse(Some(&view.sort));
    let columns = vec![Column::sortable("name", "Name")];
    let is_empty = view.rows.is_empty();

    rsx! {
        document::Title { "{view.firm_name} | Lawyer | Entity types" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        nav { class: "lawyer-nav",
            a { class: "nav-link", href: "/app/projects", "Projects" }
            a { class: "nav-link", href: "/auth/logout", "Sign out" }
        }
        main { id: "lawyer-entity-types", class: "nav-theme",
            h1 { "Entity types" }
            DataTable {
                columns,
                sort,
                base_path: "/app/admin/entity-types".to_string(),
                if is_empty {
                    tr {
                        td { class: "entity-types-empty", "No entity types yet." }
                    }
                }
                for row in view.rows.iter() {
                    tr { class: "entity-type-row",
                        td { class: "entity-type-name", "{row.name}" }
                    }
                }
            }
        }
    }
}
