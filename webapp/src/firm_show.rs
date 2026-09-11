//! The Firm detail view (ENG-494) — the membership surface the Owner listing
//! at `/app/owner` links into: one Firm's own fields, the brands it wears,
//! its Admin-DRI standing (ENG-499), and every person who holds a
//! `person_firm_role` on it.
//!
//! Read-only in this cut: it renders the same facts the write commands in
//! `store::firms` already enforce (`update`, `update_membership`,
//! `remove_membership`, `detach_brand`, `appoint_admin_dri`), so a Firm's
//! standing is visible here even before this page grows its own forms.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::people::ViewerRole;

/// The path segment base; `{id}` is appended by the route and by every link
/// into this page. Lives under `/app/admin`, not `/app/owner`: embedded Rego
/// carves out every `/app/owner/*` path as Owner-only (`owner_only_path` in
/// `portal/policy/navigator.rego`), but this page must admit an Admin
/// scoped to their own Firm too — the fine-grained check is
/// `store::firm_capability::FirmCapability::ViewDirectory`, not the route.
pub const FIRM_SHOW_PATH: &str = "/app/admin/firms";

/// One person's membership on this Firm.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct MemberRow {
    pub person_id: String,
    pub name: String,
    pub email: String,
    /// `admin` / `lawyer` / `clerk`.
    pub membership: String,
    /// The Admin-DRI marker — read-only here; only
    /// `store::firms::appoint_admin_dri` writes it (ENG-499).
    pub is_dri: bool,
}

/// Everything the Firm detail view renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct FirmShowView {
    #[serde(default)]
    pub tokens_href: String,
    #[serde(default)]
    pub firm_name: String,
    pub role: ViewerRole,
    #[serde(default)]
    pub logo: Option<crate::components::AppLogo>,
    /// The Firm id from the path. Present even on a not-found render, so the
    /// not-found copy can still name what was asked for.
    pub id: String,
    /// `None` when the id resolves to no Firm this caller may see — a
    /// missing Firm and a Firm outside the caller's own membership render
    /// identically, so neither discloses which one it was
    /// (`docs/access-model.md`).
    pub fields: Option<FirmFields>,
}

/// The Firm's own facts, present only when [`FirmShowView::fields`] is `Some`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct FirmFields {
    pub name: String,
    pub status: String,
    pub entity_name: String,
    pub brand_keys: Vec<String>,
    /// The current Admin DRI's name and email, when exactly one eligible
    /// designation exists.
    pub admin_dri: Option<(String, String)>,
    /// A human-readable Admin-DRI problem (missing / multiple / ineligible)
    /// from `store::firms::admin_dri_invariant_report`, scoped to this one
    /// Firm — `None` when the invariant holds.
    pub admin_dri_problem: Option<String>,
    pub members: Vec<MemberRow>,
    /// Whether this caller may reach `/app/admin/firms/{id}/edit` — the same
    /// `FirmCapability::ManageMembership` check `store::firms::update` itself
    /// authorizes against (ENG-585). `false` hides the Edit link rather than
    /// rendering it toward a refusal.
    #[serde(default)]
    pub can_edit: bool,
    /// The trailing-30-day invoiced/paid graphs (ENG-591), grouped by brand
    /// and by lawyer DRI. Nested here rather than a second top-level
    /// `Option` on [`FirmShowView`]: it is only ever computed once
    /// `ViewDirectory` has already gated `fields` to `Some`.
    #[serde(default)]
    pub invoices: crate::firm_invoice_graphs::FirmInvoiceGraphsView,
}

#[cfg(feature = "server")]
fn store_role(role: ViewerRole) -> store::persons::Role {
    match role {
        ViewerRole::Owner => store::persons::Role::Owner,
        ViewerRole::Admin => store::persons::Role::Admin,
        ViewerRole::Lawyer => store::persons::Role::Lawyer,
        ViewerRole::Clerk => store::persons::Role::Clerk,
        ViewerRole::Client => store::persons::Role::Client,
    }
}

#[cfg(feature = "server")]
fn admin_dri_problem_text(problem: &store::firms::AdminDriProblem) -> String {
    match problem {
        store::firms::AdminDriProblem::Missing => "No Admin DRI is designated.".to_string(),
        store::firms::AdminDriProblem::Multiple(ids) => {
            format!("{} people hold the Admin DRI designation.", ids.len())
        }
        store::firms::AdminDriProblem::Ineligible(_) => {
            "The designated Admin DRI no longer holds an eligible admin membership.".to_string()
        }
    }
}

/// Load the Firm detail page for the `{id}` in the request path.
///
/// Admission mirrors `store::firm_capability::FirmCapability::ViewDirectory`:
/// Owner sees every Firm; an Admin sees only a Firm they hold a
/// `person_firm_role` on. A Firm outside that reach renders the same
/// `fields: None` a nonexistent id would, so neither discloses the other.
#[server]
#[cfg_attr(feature = "server", allow(clippy::too_many_lines))]
pub async fn get_firm_show() -> Result<FirmShowView, ServerFnError> {
    let role = crate::admin_listing::require_admin().await?;
    let axum::extract::Path(id) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Path<uuid::Uuid>, _>()
            .await?;
    let surreal = consume_context::<store::surreal::SurrealDb>();
    let actor_person_id = crate::admin_listing::injected_person_id().await;

    let decision = store::firm_capability::resolve(
        &surreal,
        store_role(role),
        actor_person_id,
        id,
        store::firm_capability::FirmCapability::ViewDirectory,
    )
    .await
    .map_err(|error| ServerFnError::new(error.to_string()))?;

    let base = FirmShowView {
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
        firm_name: crate::app_chrome::firm_name_from_context().await,
        logo: crate::app_chrome::app_logo_from_context().await,
        role,
        id: id.to_string(),
        fields: None,
    };

    if !decision.is_allowed() {
        dioxus_fullstack_core::FullstackContext::commit_http_status(
            axum::http::StatusCode::NOT_FOUND,
            None,
        );
        return Ok(base);
    }

    let Some(firm) = store::firms::find_by_id(&surreal, id)
        .await
        .map_err(|error| ServerFnError::new(error.to_string()))?
    else {
        dioxus_fullstack_core::FullstackContext::commit_http_status(
            axum::http::StatusCode::NOT_FOUND,
            None,
        );
        return Ok(base);
    };

    let entity_name = match firm.entity_id {
        Some(entity_id) => store::entities::find_by_id(&surreal, entity_id)
            .await
            .map_err(|error| ServerFnError::new(error.to_string()))?
            .map_or_else(|| "Unlinked entity".to_string(), |entity| entity.name),
        None => "Unlinked entity".to_string(),
    };
    let brand_keys = store::firms::brand_keys_for_firm(&surreal, firm.id)
        .await
        .map_err(|error| ServerFnError::new(error.to_string()))?;

    let mut members = Vec::new();
    let mut admin_dri = None;
    // `visible_person_ids`/`memberships_for_person` answer "which Firms a
    // person belongs to"; this page asks the inverse — "which people belong
    // to this Firm" — so it reads `person_firm_role` directly rather than
    // bending an existing helper to a question it does not ask.
    for row in store::firms::memberships_for_firm(&surreal, firm.id)
        .await
        .map_err(|error| ServerFnError::new(error.to_string()))?
    {
        let Some(person) = store::persons::find_by_id(&surreal, row.person_id)
            .await
            .map_err(|error| ServerFnError::new(error.to_string()))?
        else {
            continue;
        };
        if row.is_dri {
            admin_dri = Some((person.name.clone(), person.email.clone()));
        }
        members.push(MemberRow {
            person_id: row.person_id.to_string(),
            name: person.name,
            email: person.email,
            membership: row.membership.as_str().to_string(),
            is_dri: row.is_dri,
        });
    }

    let admin_dri_problem = store::firms::admin_dri_invariant_report(&surreal)
        .await
        .map_err(|error| ServerFnError::new(error.to_string()))?
        .into_iter()
        .find(|status| status.firm_id == firm.id)
        .and_then(|status| status.problem)
        .as_ref()
        .map(admin_dri_problem_text);

    // The Edit link is offered only to a caller who could actually reach the
    // edit page — the same `ManageMembership` capability `store::firms::update`
    // itself authorizes against, resolved separately from `ViewDirectory`
    // above (Owner holds both; a Firm's non-Admin membership holds only the
    // first).
    let can_edit = matches!(
        store::firm_capability::resolve(
            &surreal,
            store_role(role),
            actor_person_id,
            id,
            store::firm_capability::FirmCapability::ManageMembership,
        )
        .await
        .map_err(|error| ServerFnError::new(error.to_string()))?,
        store::firm_capability::FirmCapabilityDecision::Allowed
    );

    let invoices =
        store::xero_invoices::firm_thirty_day_rollup(&surreal, firm.id, chrono::Utc::now())
            .await
            .map_err(|error| ServerFnError::new(error.to_string()))?
            .into();

    Ok(FirmShowView {
        fields: Some(FirmFields {
            name: firm.name,
            status: firm.status,
            entity_name,
            brand_keys,
            admin_dri,
            admin_dri_problem,
            members,
            can_edit,
            invoices,
        }),
        ..base
    })
}

/// The route entry for `{FIRM_SHOW_PATH}/{id}`.
#[component]
pub fn FirmShow() -> Element {
    let resource = use_server_future(get_firm_show)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "firm-show", p { "Failed to load the firm." } }
            }
        }
        None => {
            return rsx! {
                main { id: "firm-show", p { "Loading…" } }
            }
        }
    };
    firm_show_body(&view)
}

/// The loaded page. Split from the component so tests render a fixed view.
#[allow(clippy::too_many_lines)]
pub fn firm_show_body(view: &FirmShowView) -> Element {
    let role = view.role;
    let firm_name = view.firm_name.clone();
    let title = format!("{firm_name} | Owner | Firms");
    // Owner has a Firms inventory to return to; an Admin — who can reach
    // this page for their own Firm but not the Owner-only `/app/owner`
    // listing — goes back to the admin console home instead.
    let back_href = if role.is_owner() {
        "/app/owner"
    } else {
        "/app/admin"
    };

    let Some(fields) = view.fields.as_ref() else {
        return rsx! {
            document::Title { "{title} | Not found" }
            document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
            document::Stylesheet { href: "{view.tokens_href}" }
            crate::components::AppNavbar {
                destinations: crate::app_chrome::app_destinations(role),
                logo: view.logo.clone(),
            }
            main { id: "firm-show",
                h1 { "Firm not found" }
                p { a { href: "{back_href}", "← Back" } }
            }
        };
    };

    let brands = if fields.brand_keys.is_empty() {
        "No house brands attached.".to_string()
    } else {
        fields.brand_keys.join(", ")
    };

    rsx! {
        document::Title { "{title}" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        crate::components::AppNavbar {
            destinations: crate::app_chrome::app_destinations(role),
            logo: view.logo.clone(),
        }
        main { id: "firm-show", class: "nav-theme",
            header { class: "page-header",
                h1 { "{fields.name}" }
                p { class: "page-subtitle",
                    "Entity: {fields.entity_name}. Status: {fields.status}."
                }
                if fields.can_edit {
                    p {
                        a {
                            class: "nav-btn nav-btn--secondary",
                            href: "{FIRM_SHOW_PATH}/{view.id}/edit",
                            "Edit",
                        }
                    }
                }
            }
            section { id: "firm-brands",
                h2 { "Brands" }
                p { "{brands}" }
            }
            section { id: "firm-admin-dri",
                h2 { "Admin DRI" }
                match (&fields.admin_dri, &fields.admin_dri_problem) {
                    (Some((name, email)), None) => rsx! {
                        p { "{name} ({email})" }
                    },
                    (_, Some(problem)) => rsx! {
                        p { class: "nav-form-error", role: "alert", "{problem}" }
                    },
                    (None, None) => rsx! {
                        p { "No Admin DRI is designated." }
                    },
                }
            }
            section { id: "firm-members",
                h2 { "People" }
                if fields.members.is_empty() {
                    p { "No one holds a membership on this Firm yet." }
                } else {
                    div { class: "nav-table-wrap",
                        table { class: "nav-table",
                            thead {
                                tr {
                                    th { "Name" }
                                    th { "Email" }
                                    th { "Membership" }
                                    th { "Admin DRI" }
                                }
                            }
                            tbody {
                                for member in fields.members.iter().cloned() {
                                    tr { class: "firm-member-row", key: "{member.person_id}",
                                        td { "{member.name}" }
                                        td { "{member.email}" }
                                        td { "{member.membership}" }
                                        td { if member.is_dri { "Yes" } else { "" } }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            crate::firm_invoice_graphs::FirmInvoiceGraphs { view: fields.invoices.clone() }
            p { a { href: "{back_href}", "← Back" } }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{firm_show_body, FirmFields, FirmShowView, MemberRow};
    use crate::firm_invoice_graphs::{
        CurrencyInvoiceGraphsView, FirmInvoiceGraphsView, InvoiceBarView,
    };
    use crate::people::ViewerRole;

    fn render(fields: Option<FirmFields>) -> String {
        dioxus_ssr::render_element(firm_show_body(&FirmShowView {
            tokens_href: String::new(),
            firm_name: "Neon Law".to_string(),
            role: ViewerRole::Owner,
            logo: None,
            id: "firm-1".to_string(),
            fields,
        }))
    }

    #[test]
    fn renders_members_with_a_read_only_dri_marker() {
        let html = render(Some(FirmFields {
            name: "Shook Law PLLC".to_string(),
            status: "active".to_string(),
            entity_name: "Shook Law PLLC".to_string(),
            brand_keys: vec!["neon".to_string()],
            admin_dri: Some(("Nick Shook".to_string(), "nick@neonlaw.com".to_string())),
            admin_dri_problem: None,
            members: vec![
                MemberRow {
                    person_id: "p-1".to_string(),
                    name: "Nick Shook".to_string(),
                    email: "nick@neonlaw.com".to_string(),
                    membership: "admin".to_string(),
                    is_dri: true,
                },
                MemberRow {
                    person_id: "p-2".to_string(),
                    name: "Pat Lawyer".to_string(),
                    email: "pat@example.com".to_string(),
                    membership: "lawyer".to_string(),
                    is_dri: false,
                },
            ],
            can_edit: true,
            ..Default::default()
        }));
        assert!(html.contains("Nick Shook (nick@neonlaw.com)"), "{html}");
        assert!(html.contains("Pat Lawyer"), "{html}");
        assert!(html.contains(r#"id="firm-show""#), "{html}");
        assert!(
            html.contains(r#"href="/app/admin/firms/firm-1/edit""#),
            "{html}"
        );
    }

    /// A caller without `ManageMembership` sees the Firm's facts but no Edit
    /// link — the same "offer what the capability admits" rule the create
    /// form's Owner-only door mirrors from the other side.
    #[test]
    fn hides_the_edit_link_when_the_caller_cannot_edit() {
        let html = render(Some(FirmFields {
            name: "Read Only Practice".to_string(),
            status: "active".to_string(),
            entity_name: "Read Only Entity".to_string(),
            brand_keys: Vec::new(),
            admin_dri: None,
            admin_dri_problem: None,
            members: Vec::new(),
            can_edit: false,
            ..Default::default()
        }));
        assert!(!html.contains("/edit\""), "{html}");
    }

    #[test]
    fn surfaces_an_admin_dri_problem_instead_of_a_name() {
        let html = render(Some(FirmFields {
            name: "Orphaned Practice".to_string(),
            status: "active".to_string(),
            entity_name: "Orphaned Entity".to_string(),
            brand_keys: Vec::new(),
            admin_dri: None,
            admin_dri_problem: Some("No Admin DRI is designated.".to_string()),
            members: Vec::new(),
            can_edit: false,
            ..Default::default()
        }));
        assert!(html.contains("No Admin DRI is designated."), "{html}");
        assert!(html.contains("No house brands attached."), "{html}");
    }

    /// ENG-591: the invoice graphs render below the members table, driven by
    /// whatever `FirmFields::invoices` the loader resolved.
    #[test]
    fn renders_the_invoice_graphs_below_the_members_table() {
        let html = render(Some(FirmFields {
            name: "Graphed Practice".to_string(),
            status: "active".to_string(),
            entity_name: "Graphed Entity".to_string(),
            brand_keys: Vec::new(),
            admin_dri: None,
            admin_dri_problem: None,
            members: Vec::new(),
            can_edit: false,
            invoices: FirmInvoiceGraphsView {
                currencies: vec![CurrencyInvoiceGraphsView {
                    currency: "USD".to_string(),
                    by_brand: vec![InvoiceBarView {
                        label: "Neon Law".to_string(),
                        invoiced_cents: 10_000,
                        paid_cents: 5_000,
                    }],
                    by_lawyer_dri: vec![],
                }],
            },
        }));
        assert!(html.contains(r#"id="firm-invoice-graphs""#), "{html}");
        assert!(html.contains("Neon Law invoiced: $100"), "{html}");
    }

    /// A Firm with no invoices in the window renders the graphs section's
    /// own empty state rather than an omitted section.
    #[test]
    fn renders_the_invoice_graphs_empty_state_with_no_invoices() {
        let html = render(Some(FirmFields {
            name: "Quiet Practice".to_string(),
            status: "active".to_string(),
            entity_name: "Quiet Entity".to_string(),
            brand_keys: Vec::new(),
            admin_dri: None,
            admin_dri_problem: None,
            members: Vec::new(),
            can_edit: false,
            ..Default::default()
        }));
        assert!(
            html.contains("No invoices in the trailing 30 days."),
            "{html}"
        );
    }

    #[test]
    fn a_firm_outside_the_caller_s_reach_renders_not_found() {
        let html = render(None);
        assert!(html.contains("Firm not found"), "{html}");
        assert!(!html.contains("firm-member-row"), "{html}");
    }
}
