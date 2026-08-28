//! The Owner/Admin matter directory at `/app/admin/projects` (ENG-221).
//!
//! Oversight, not membership. Owner and Admin are not invited to matters —
//! they hold no `person_project_roles` row, because a membership row is what
//! grants access to what a matter *contains*. This page is the other question:
//! which matters exist, and who is accountable for each. It renders exactly
//! `store::projects::matter_directory`'s four fields — code, name, status, and
//! the person on the matter's `is_lawyer_dri` row — and nothing else.
//!
//! A matter nobody has taken accountability for is the case the page exists to
//! surface, so an absent DRI renders as its own emphasized cell rather than as
//! a blank one.
//!
//! The path is deliberately `/app/admin/projects` while its two siblings still
//! sit at `/app/admin/people` and `/app/admin/analytics`: new admin surfaces are born
//! under `/app/admin`, and the older two move on their own schedule.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Column, DataTable, SortState};
use crate::people::ViewerRole;

/// The page's own path.
pub const MATTER_DIRECTORY_PATH: &str = "/app/admin/projects";

/// The `?sort=` keys the headers advertise. The route's pre-handler answers
/// anything else with a `400` before the render, so a header can never link to
/// a query the route refuses.
pub const MATTER_DIRECTORY_SORT: &[&str] = &["code", "name", "status", "dri"];

/// One matter as the directory lens shows it, in a wasm-safe shape (plain
/// fields — no `store` types cross to the client build).
///
/// No id: the lens carries no link into the matter, because reaching a matter
/// is membership's decision and this page is not membership. The code is the
/// handle, and it is unique.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct MatterRow {
    pub code: String,
    pub name: String,
    pub status: String,
    /// The accountable lawyers' names, alphabetical; empty when the matter has
    /// no `is_lawyer_dri` row at all.
    pub lawyer_dris: Vec<String>,
}

impl MatterRow {
    /// The DRI column as one string — the names joined, or empty for a matter
    /// nobody is accountable for. Sorting and rendering read the same value, so
    /// the column cannot sort by one thing and show another.
    #[must_use]
    pub fn dri_label(&self) -> String {
        self.lawyer_dris.join(", ")
    }
}

/// The rendered directory: the rows, the active `?sort=`, and the chrome the
/// viewer's tier resolves.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct MatterDirectoryView {
    pub rows: Vec<MatterRow>,
    pub sort: String,
    pub role: ViewerRole,
    /// The deploy's brand mark for the navbar. `None` when the mounted brand
    /// configures none.
    #[serde(default)]
    pub logo: Option<crate::components::AppLogo>,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// The directory's `?sort=` query.
#[derive(Deserialize, Default)]
pub struct MatterDirectoryQuery {
    #[serde(default)]
    pub sort: Option<String>,
}

/// Load the directory: refuse any non-admin caller, read the lens, and order
/// the rows.
///
/// `require_admin` commits a real `403` for a caller below the admin tier, so a
/// direct hit on the generated endpoint discloses no matter. The store
/// predicate refuses that tier a second time and reads nothing for it, which is
/// what keeps the two answers from drifting.
#[server]
pub async fn matter_directory_view() -> Result<MatterDirectoryView, ServerFnError> {
    let role = crate::admin_listing::require_admin().await?;
    let axum::extract::Query(query) = dioxus_fullstack_core::FullstackContext::extract::<
        axum::extract::Query<MatterDirectoryQuery>,
        _,
    >()
    .await?;
    let sort = query.sort.unwrap_or_default();

    let store_role = match role {
        ViewerRole::Owner => store::persons::Role::Owner,
        ViewerRole::Admin => store::persons::Role::Admin,
        ViewerRole::Lawyer => store::persons::Role::Lawyer,
        ViewerRole::Clerk => store::persons::Role::Clerk,
        ViewerRole::Client => store::persons::Role::Client,
    };
    let surreal = consume_context::<store::surreal::SurrealDb>();
    let entries = store::projects::matter_directory(&surreal, store_role)
        .await
        .map_err(|error| {
            // A directory that cannot be read is a server error, not a page
            // that quietly reports zero matters.
            dioxus_fullstack_core::FullstackContext::commit_http_status(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                None,
            );
            ServerFnError::new(error.to_string())
        })?;

    let mut rows: Vec<MatterRow> = entries
        .into_iter()
        .map(|entry| MatterRow {
            code: entry.code,
            name: entry.name,
            status: entry.status,
            lawyer_dris: entry.lawyer_dris,
        })
        .collect();
    sort_rows(&mut rows, &sort);

    Ok(MatterDirectoryView {
        firm_name: crate::app_chrome::firm_name_from_context().await,
        rows,
        sort,
        role,
        logo: crate::app_chrome::app_logo_from_context().await,
    })
}

/// Order the rows by the requested `?sort=`, first field primary.
///
/// One composite comparator, the JSON:API `SortSpec` precedence contract the
/// other sortable listings hold. An unassigned matter sorts as an empty DRI, so
/// ascending by `dri` groups exactly the matters this page exists to surface at
/// the top.
#[cfg(feature = "server")]
fn sort_rows(rows: &mut [MatterRow], sort: &str) {
    let parsed = crate::admin_listing::parse_sort(sort);
    rows.sort_by(|a, b| {
        parsed
            .iter()
            .fold(std::cmp::Ordering::Equal, |acc, (key, descending)| {
                acc.then_with(|| {
                    let ordering = match key.as_str() {
                        "code" => a.code.cmp(&b.code),
                        "name" => a.name.cmp(&b.name),
                        "status" => a.status.cmp(&b.status),
                        "dri" => a.dri_label().cmp(&b.dri_label()),
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
}

/// `/app/admin/projects` — the matter directory.
#[component]
pub fn AdminMatterDirectory() -> Element {
    let resource = use_server_future(matter_directory_view)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "matter-directory", p { "Failed to load the matter directory." } }
            }
        }
        None => {
            return rsx! {
                main { id: "matter-directory", p { "Loading…" } }
            }
        }
    };
    matter_directory_body(&view)
}

/// The loaded page. Prop-driven and free of any server future, so it
/// server-renders and unit-tests directly.
pub fn matter_directory_body(view: &MatterDirectoryView) -> Element {
    let view = view.clone();
    let sort = SortState::parse(Some(&view.sort));
    let columns = vec![
        Column::sortable("code", "Code"),
        Column::sortable("name", "Matter"),
        Column::sortable("status", "Status"),
        Column::sortable("dri", "Lawyer DRI"),
    ];
    let is_empty = view.rows.is_empty();

    rsx! {
        document::Title { "{view.firm_name} | Admin | Matters" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        crate::components::AppNavbar {
            destinations: crate::app_chrome::app_destinations(view.role),
            logo: view.logo.clone(),
        }
        main { id: "matter-directory", class: "nav-theme",
            header { class: "page-header",
                h1 { "Matters" }
                p { class: "nav-muted",
                    "Every matter the firm carries and the lawyer accountable for it."
                }
            }
            if is_empty {
                p { class: "nav-muted", role: "status", "No matters yet." }
            } else {
                DataTable {
                    columns,
                    sort,
                    base_path: MATTER_DIRECTORY_PATH.to_string(),
                    for row in view.rows.iter() {
                        tr { class: "matter-directory-row",
                            td { class: "project-code", code { "{row.code}" } }
                            td { class: "project-name", "{row.name}" }
                            td { class: "project-status", "{row.status}" }
                            td { class: "matter-directory-dri",
                                if row.lawyer_dris.is_empty() {
                                    span { class: "matter-flag", "Unassigned" }
                                } else {
                                    "{row.dri_label()}"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(code: &str, status: &str, dris: &[&str]) -> MatterRow {
        MatterRow {
            code: code.to_string(),
            name: format!("{code} matter"),
            status: status.to_string(),
            lawyer_dris: dris.iter().map(|d| (*d).to_string()).collect(),
        }
    }

    fn html(view: &MatterDirectoryView) -> String {
        dioxus_ssr::render_element(matter_directory_body(view))
    }

    fn directory(rows: Vec<MatterRow>, role: ViewerRole) -> MatterDirectoryView {
        MatterDirectoryView {
            firm_name: "Neon Law".to_string(),
            rows,
            sort: String::new(),
            role,
            logo: None,
        }
    }

    /// The four fields the lens carries, and no fifth.
    #[test]
    fn the_directory_renders_code_name_status_and_dri() {
        let out = html(&directory(
            vec![row("acme-llc", "open", &["Nick Shook"])],
            ViewerRole::Owner,
        ));
        assert!(out.contains("acme-llc"), "{out}");
        assert!(out.contains("acme-llc matter"), "{out}");
        assert!(out.contains("open"), "{out}");
        assert!(out.contains("Nick Shook"), "{out}");
    }

    /// A matter two lawyers answer for names both. Showing one and dropping the
    /// other would make the oversight lens quietly wrong about who is
    /// accountable, which is the one question it exists to answer.
    #[test]
    fn a_matter_with_several_lawyer_dris_names_them_all() {
        let out = html(&directory(
            vec![row("acme-llc", "open", &["Ada Counsel", "Nick Shook"])],
            ViewerRole::Owner,
        ));
        assert!(out.contains("Ada Counsel"), "{out}");
        assert!(out.contains("Nick Shook"), "{out}");
        assert!(!out.contains("Unassigned"), "{out}");
    }

    /// The case the page exists to surface reads as unassigned, and the row is
    /// still there — the failure mode is a matter that quietly does not list.
    #[test]
    fn a_matter_with_no_lawyer_dri_renders_as_unassigned() {
        let out = html(&directory(
            vec![row("orphan-llc", "open", &[])],
            ViewerRole::Admin,
        ));
        assert!(out.contains("orphan-llc"), "the matter still lists: {out}");
        assert!(out.contains("Unassigned"), "{out}");
    }

    /// The lens is a directory, not a door: no row links into a matter, because
    /// reaching one is membership's decision.
    #[test]
    fn no_row_links_into_a_matter() {
        let out = html(&directory(
            vec![
                row("acme-llc", "open", &["Nick Shook"]),
                row("orphan-llc", "open", &[]),
            ],
            ViewerRole::Owner,
        ));
        assert!(!out.contains(r#"href="/app/projects/"#), "{out}");
    }

    /// The headers sort on exactly the keys the route advertises, so a header
    /// anchor cannot link to a `?sort=` the pre-handler answers with a `400`.
    #[test]
    fn every_sortable_header_names_an_advertised_key() {
        let out = html(&directory(
            vec![row("acme-llc", "open", &["Nick Shook"])],
            ViewerRole::Owner,
        ));
        for key in MATTER_DIRECTORY_SORT {
            assert!(
                out.contains(&format!("sort={key}")),
                "header for {key}: {out}"
            );
        }
    }

    #[test]
    fn an_empty_directory_says_so() {
        let out = html(&directory(Vec::new(), ViewerRole::Owner));
        assert!(out.contains("No matters yet."), "{out}");
        assert!(!out.contains("<table"), "no empty table: {out}");
    }

    /// The page mounts the `/app` chrome, so an admin reading it keeps the same
    /// navbar every other `/app` page renders — the shared three, with the
    /// tier-gated doors left to the Team home's cards.
    #[test]
    fn the_page_carries_the_app_chrome() {
        let out = html(&directory(
            vec![row("acme-llc", "open", &["Nick Shook"])],
            ViewerRole::Owner,
        ));
        assert!(out.contains(r#"href="/app/projects""#), "{out}");
        assert!(out.contains(r#"href="/app/team""#), "team home: {out}");
        assert!(out.contains(r#"href="/auth/logout""#), "{out}");
        assert!(
            !out.contains(r#"href="/app/admin""#),
            "the admin desk is a Team-home card, not a navbar door: {out}"
        );
    }

    #[cfg(feature = "server")]
    #[test]
    fn the_unassigned_matters_sort_to_the_top_by_dri() {
        let mut rows = vec![
            row("acme-llc", "open", &["Nick Shook"]),
            row("orphan-llc", "open", &[]),
            row("beta-llc", "open", &["Ada Counsel"]),
        ];
        sort_rows(&mut rows, "dri");
        let codes: Vec<&str> = rows.iter().map(|r| r.code.as_str()).collect();
        assert_eq!(codes, ["orphan-llc", "beta-llc", "acme-llc"]);
    }

    /// The first requested field is primary; later fields only break ties.
    #[cfg(feature = "server")]
    #[test]
    fn the_first_sort_field_is_primary() {
        let mut rows = vec![
            row("b-open", "open", &[]),
            row("a-closed", "closed", &[]),
            row("c-closed", "closed", &[]),
        ];
        sort_rows(&mut rows, "status,-code");
        let codes: Vec<&str> = rows.iter().map(|r| r.code.as_str()).collect();
        assert_eq!(codes, ["c-closed", "a-closed", "b-open"]);
    }
}
