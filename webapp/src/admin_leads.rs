//! Owner/Admin lead queue at `/app/admin/leads`.
//!
//! A lead is a public contact request, not a Person. This surface is the
//! reader for those rows: the list masks a phone to its last four digits, the
//! row page shows the recorded number, and status or conversion writes stay on
//! native `POST`s in `portal::admin`.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Column, DataTable, Field, FormCard, Heading, SortState};
use crate::people::ViewerRole;

/// The queue list.
pub const LEADS_PATH: &str = "/app/admin/leads";
/// One lead's row page.
pub const LEAD_PATH: &str = "/app/admin/leads/{id}";

/// Closed status words the status form offers.
const STATUSES: &[&str] = &["new", "contacted", "converted", "declined", "unsubscribed"];

/// One row on the queue list. The phone is already masked.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct LeadListRow {
    pub id: String,
    pub email: String,
    pub phone_masked: String,
    pub brand_key: String,
    pub source_path: String,
    pub consent_version: String,
    pub consented_at: String,
    pub sms_consented_at: String,
    pub status: String,
    pub submissions: String,
}

/// Everything the list renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct LeadsListView {
    pub role: ViewerRole,
    #[serde(default)]
    pub logo: Option<crate::components::AppLogo>,
    #[serde(default)]
    pub tokens_href: String,
    #[serde(default)]
    pub firm_name: String,
    #[serde(default)]
    pub rows: Vec<LeadListRow>,
}

/// One lead as the row page shows it, including the full phone.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct LeadDetail {
    pub id: String,
    pub email: String,
    pub phone: String,
    pub brand_key: String,
    pub source_path: String,
    pub consent_version: String,
    pub consented_at: String,
    pub sms_consented_at: String,
    pub status: String,
    pub submissions: String,
    pub unsubscribed_at: String,
    pub person_id: String,
    /// Set when a Person already holds this mailbox, so the page can offer a
    /// link instead of create.
    pub existing_person_id: Option<String>,
}

/// The row page, including flashes and the session CSRF token.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct LeadShowView {
    pub role: ViewerRole,
    #[serde(default)]
    pub logo: Option<crate::components::AppLogo>,
    #[serde(default)]
    pub tokens_href: String,
    #[serde(default)]
    pub firm_name: String,
    #[serde(default)]
    pub csrf_token: String,
    #[serde(default)]
    pub notice: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    pub lead: Option<LeadDetail>,
}

/// Flash query on the row page.
#[cfg(feature = "server")]
#[derive(Deserialize, Default)]
struct LeadShowQuery {
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    notice: Option<String>,
}

#[cfg(feature = "server")]
fn format_time(value: chrono::DateTime<chrono::Utc>) -> String {
    value.to_rfc3339()
}

#[cfg(feature = "server")]
fn format_optional_time(value: Option<chrono::DateTime<chrono::Utc>>) -> String {
    value.map_or_else(|| "—".to_string(), format_time)
}

/// Load the queue: Owner/Admin only, newest first, phones masked.
#[server]
pub async fn leads_list_view() -> Result<LeadsListView, ServerFnError> {
    let role = crate::admin_listing::require_admin().await?;
    let surreal = consume_context::<store::surreal::SurrealDb>();
    let rows = store::leads::list(&surreal)
        .await
        .map_err(|error| ServerFnError::new(error.to_string()))?
        .into_iter()
        .map(|lead| LeadListRow {
            id: lead.id.to_string(),
            email: lead.email,
            phone_masked: store::leads::mask_phone(lead.phone.as_deref()),
            brand_key: lead.brand_key,
            source_path: lead.source_path,
            consent_version: lead.consent_version,
            consented_at: format_time(lead.consented_at),
            sms_consented_at: format_optional_time(lead.sms_consented_at),
            status: lead.status,
            submissions: lead.submissions.to_string(),
        })
        .collect();
    Ok(LeadsListView {
        firm_name: crate::app_chrome::firm_name_from_context().await,
        role,
        logo: crate::app_chrome::app_logo_from_context().await,
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
        rows,
    })
}

/// Load one lead: Owner/Admin only, with the recorded phone.
#[server]
pub async fn lead_show_view() -> Result<LeadShowView, ServerFnError> {
    let role = crate::admin_listing::require_admin().await?;
    let axum::extract::Path(id) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Path<uuid::Uuid>, _>()
            .await?;
    let csrf_token = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::csrf::CsrfToken>,
        _,
    >()
    .await
    .map(|axum::Extension(token)| token.0)
    .unwrap_or_default();
    let query =
        dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Query<LeadShowQuery>, _>(
        )
        .await
        .map(|axum::extract::Query(q)| q)
        .unwrap_or_default();

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let Some(lead) = store::leads::find(&surreal, id)
        .await
        .map_err(|error| ServerFnError::new(error.to_string()))?
    else {
        dioxus_fullstack_core::FullstackContext::commit_http_status(
            axum::http::StatusCode::NOT_FOUND,
            None,
        );
        return Ok(LeadShowView {
            firm_name: crate::app_chrome::firm_name_from_context().await,
            role,
            logo: crate::app_chrome::app_logo_from_context().await,
            tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
            csrf_token,
            notice: query.notice,
            error: query.error,
            lead: None,
        });
    };

    let existing_person_id = if lead.person_id.is_none() {
        store::persons::find_by_email_ci(&surreal, &lead.email)
            .await
            .map_err(|error| ServerFnError::new(error.to_string()))?
            .map(|person| person.id.to_string())
    } else {
        None
    };

    Ok(LeadShowView {
        firm_name: crate::app_chrome::firm_name_from_context().await,
        role,
        logo: crate::app_chrome::app_logo_from_context().await,
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
        csrf_token,
        notice: query.notice,
        error: query.error,
        lead: Some(LeadDetail {
            id: lead.id.to_string(),
            email: lead.email,
            phone: lead.phone.unwrap_or_else(|| "—".to_string()),
            brand_key: lead.brand_key,
            source_path: lead.source_path,
            consent_version: lead.consent_version,
            consented_at: format_time(lead.consented_at),
            sms_consented_at: format_optional_time(lead.sms_consented_at),
            status: lead.status,
            submissions: lead.submissions.to_string(),
            unsubscribed_at: format_optional_time(lead.unsubscribed_at),
            person_id: lead
                .person_id
                .map_or_else(|| "—".to_string(), |id| id.to_string()),
            existing_person_id,
        }),
    })
}

/// `/app/admin/leads`.
#[component]
pub fn AdminLeads() -> Element {
    let resource = use_server_future(leads_list_view)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "admin-leads", p { "Failed to load the lead queue." } }
            }
        }
        None => {
            return rsx! {
                main { id: "admin-leads", p { "Loading…" } }
            }
        }
    };
    leads_list_body(&view)
}

/// `/app/admin/leads/{id}`.
#[component]
pub fn AdminLeadShow() -> Element {
    let resource = use_server_future(lead_show_view)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "admin-lead", p { "Failed to load the lead." } }
            }
        }
        None => {
            return rsx! {
                main { id: "admin-lead", p { "Loading…" } }
            }
        }
    };
    lead_show_body(&view)
}

/// Prop-driven list body.
pub fn leads_list_body(view: &LeadsListView) -> Element {
    let view = view.clone();
    let is_empty = view.rows.is_empty();
    rsx! {
        document::Title { "{view.firm_name} | Admin | Leads" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        crate::components::AppNavbar {
            destinations: crate::app_chrome::app_destinations(view.role),
            logo: view.logo.clone(),
        }
        main { id: "admin-leads", class: "nav-theme",
            header { class: "page-header",
                h1 { "Leads" }
                p { class: "nav-muted",
                    "Public contact requests. Phone numbers on this list show only the last four digits."
                }
            }
            if is_empty {
                p { class: "nav-muted", role: "status", "No leads yet." }
            } else {
                DataTable {
                    columns: vec![
                        Column::fixed("email", "Email"),
                        Column::fixed("phone", "Phone"),
                        Column::fixed("brand", "Brand"),
                        Column::fixed("source", "Source"),
                        Column::fixed("consent", "Consent"),
                        Column::fixed("consented", "Consented"),
                        Column::fixed("sms", "SMS"),
                        Column::fixed("status", "Status"),
                        Column::fixed("submissions", "Submissions"),
                    ],
                    sort: SortState::default(),
                    base_path: LEADS_PATH.to_string(),
                    for row in view.rows.iter() {
                        tr {
                            td {
                                a { href: "/app/admin/leads/{row.id}", "{row.email}" }
                            }
                            td { "{row.phone_masked}" }
                            td { "{row.brand_key}" }
                            td { "{row.source_path}" }
                            td { "{row.consent_version}" }
                            td { "{row.consented_at}" }
                            td { "{row.sms_consented_at}" }
                            td { "{row.status}" }
                            td { "{row.submissions}" }
                        }
                    }
                }
            }
        }
    }
}

/// Prop-driven row body.
pub fn lead_show_body(view: &LeadShowView) -> Element {
    let view = view.clone();
    let Some(lead) = view.lead.clone() else {
        return rsx! {
            document::Title { "{view.firm_name} | Admin | Lead" }
            document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
            document::Stylesheet { href: "{view.tokens_href}" }
            crate::components::AppNavbar {
                destinations: crate::app_chrome::app_destinations(view.role),
                logo: view.logo.clone(),
            }
            main { id: "admin-lead", class: "nav-theme",
                p { role: "status", "That lead was not found." }
                p { a { href: LEADS_PATH, "Back to leads" } }
            }
        };
    };
    let status_action = format!("/app/admin/leads/{}/status", lead.id);
    let csrf = view.csrf_token.clone();
    let status_fields = vec![Field::select(
        "Status",
        "status",
        STATUSES
            .iter()
            .map(|word| crate::components::Choice::new(*word, *word))
            .collect(),
        Some(lead.status.clone()),
    )];
    let conversion = conversion_controls(&lead, &csrf);
    rsx! {
        document::Title { "{view.firm_name} | Admin | Lead" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        crate::components::AppNavbar {
            destinations: crate::app_chrome::app_destinations(view.role),
            logo: view.logo.clone(),
        }
        main { id: "admin-lead", class: "nav-theme",
            p { a { href: LEADS_PATH, "Back to leads" } }
            header { class: "page-header",
                h1 { "Lead" }
            }
            if let Some(notice) = view.notice.as_ref() {
                p { class: "nav-notice", role: "status", "{notice}" }
            }
            if let Some(error) = view.error.as_ref() {
                p { class: "nav-error", role: "alert", "{error}" }
            }
            dl { class: "lead-facts",
                dt { "Email" }
                dd { "{lead.email}" }
                dt { "Phone" }
                dd { "{lead.phone}" }
                dt { "Brand" }
                dd { "{lead.brand_key}" }
                dt { "Source" }
                dd { "{lead.source_path}" }
                dt { "Consent version" }
                dd { "{lead.consent_version}" }
                dt { "Consented" }
                dd { "{lead.consented_at}" }
                dt { "SMS consented" }
                dd { "{lead.sms_consented_at}" }
                dt { "Status" }
                dd { "{lead.status}" }
                dt { "Submissions" }
                dd { "{lead.submissions}" }
                dt { "Unsubscribed" }
                dd { "{lead.unsubscribed_at}" }
                dt { "Person" }
                dd { "{lead.person_id}" }
            }
            FormCard {
                title: "Change status".to_string(),
                action: status_action,
                submit_label: "Save status".to_string(),
                heading: Heading::H2,
                csrf_token: Some(csrf),
                fields: status_fields,
            }
            {conversion}
        }
    }
}

fn conversion_controls(lead: &LeadDetail, csrf: &str) -> Element {
    if lead.person_id != "—" {
        return rsx! {};
    }
    if let Some(existing) = lead.existing_person_id.as_ref() {
        let link_action = format!("/app/admin/leads/{}/link", lead.id);
        return rsx! {
            section { class: "lead-convert",
                p {
                    "A Person already holds this mailbox ("
                    a { href: "/app/admin/people/{existing}", "{existing}" }
                    "). Link this lead instead of creating another."
                }
                form { method: "post", action: "{link_action}", "aria-label": "Link existing Person",
                    input { r#type: "hidden", name: "_csrf", value: "{csrf}" }
                    button { class: "nav-btn nav-btn--primary", r#type: "submit", "Link existing Person" }
                }
            }
        };
    }
    let convert_action = format!("/app/admin/leads/{}/convert", lead.id);
    rsx! {
        form { method: "post", action: "{convert_action}", "aria-label": "Create Person",
            input { r#type: "hidden", name: "_csrf", value: "{csrf}" }
            button { class: "nav-btn nav-btn--primary", r#type: "submit", "Create Person" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        lead_show_body, leads_list_body, LeadDetail, LeadListRow, LeadShowView, LeadsListView,
    };
    use crate::people::ViewerRole;

    fn list_view(rows: Vec<LeadListRow>) -> LeadsListView {
        LeadsListView {
            tokens_href: String::new(),
            firm_name: "Neon Law".to_string(),
            role: ViewerRole::Admin,
            logo: None,
            rows,
        }
    }

    fn sample_row() -> LeadListRow {
        LeadListRow {
            id: "11111111-1111-1111-1111-111111111111".to_string(),
            email: "visitor@example.com".to_string(),
            phone_masked: "…9876".to_string(),
            brand_key: "neon".to_string(),
            source_path: "/contact".to_string(),
            consent_version: "By sending this, you agree.".to_string(),
            consented_at: "2026-01-01T00:00:00Z".to_string(),
            sms_consented_at: "—".to_string(),
            status: "new".to_string(),
            submissions: "1".to_string(),
        }
    }

    #[test]
    fn the_list_masks_the_phone_and_never_prints_the_recorded_number() {
        let html = dioxus_ssr::render_element(leads_list_body(&list_view(vec![sample_row()])));
        assert!(html.contains("visitor@example.com"), "{html}");
        assert!(html.contains("…9876"), "{html}");
        assert!(!html.contains("555"), "{html}");
        assert!(!html.contains("010-9876"), "{html}");
        assert!(
            html.contains(r#"href="/app/admin/leads/11111111-1111-1111-1111-111111111111""#),
            "{html}"
        );
    }

    #[test]
    fn the_row_page_shows_the_full_phone_and_create_person() {
        let html = dioxus_ssr::render_element(lead_show_body(&LeadShowView {
            tokens_href: String::new(),
            firm_name: "Neon Law".to_string(),
            role: ViewerRole::Owner,
            logo: None,
            csrf_token: "TOK".to_string(),
            notice: None,
            error: None,
            lead: Some(LeadDetail {
                id: "11111111-1111-1111-1111-111111111111".to_string(),
                email: "visitor@example.com".to_string(),
                phone: "+1 (555) 010-9876".to_string(),
                brand_key: "neon".to_string(),
                source_path: "/contact".to_string(),
                consent_version: "By sending this, you agree.".to_string(),
                consented_at: "2026-01-01T00:00:00Z".to_string(),
                sms_consented_at: "—".to_string(),
                status: "new".to_string(),
                submissions: "1".to_string(),
                unsubscribed_at: "—".to_string(),
                person_id: "—".to_string(),
                existing_person_id: None,
            }),
        }));
        assert!(html.contains("+1 (555) 010-9876"), "{html}");
        assert!(html.contains("Create Person"), "{html}");
        assert!(
            html.contains(
                r#"action="/app/admin/leads/11111111-1111-1111-1111-111111111111/convert""#
            ),
            "{html}"
        );
        assert!(html.contains(r#"name="_csrf" value="TOK""#), "{html}");
    }

    #[test]
    fn a_duplicate_mailbox_offers_link_instead_of_create() {
        let html = dioxus_ssr::render_element(lead_show_body(&LeadShowView {
            tokens_href: String::new(),
            firm_name: "Neon Law".to_string(),
            role: ViewerRole::Admin,
            logo: None,
            csrf_token: "TOK".to_string(),
            notice: None,
            error: Some("A Person already holds this mailbox.".to_string()),
            lead: Some(LeadDetail {
                id: "11111111-1111-1111-1111-111111111111".to_string(),
                email: "visitor@example.com".to_string(),
                phone: "—".to_string(),
                brand_key: "neon".to_string(),
                source_path: "/contact".to_string(),
                consent_version: "By sending this, you agree.".to_string(),
                consented_at: "2026-01-01T00:00:00Z".to_string(),
                sms_consented_at: "—".to_string(),
                status: "new".to_string(),
                submissions: "1".to_string(),
                unsubscribed_at: "—".to_string(),
                person_id: "—".to_string(),
                existing_person_id: Some("22222222-2222-2222-2222-222222222222".to_string()),
            }),
        }));
        assert!(html.contains("Link existing Person"), "{html}");
        assert!(!html.contains("Create Person"), "{html}");
        assert!(
            html.contains(r#"action="/app/admin/leads/11111111-1111-1111-1111-111111111111/link""#),
            "{html}"
        );
    }
}
