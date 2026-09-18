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
const STATUSES: &[&str] = &["new", "contacted", "declined", "unsubscribed"];

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
    #[serde(default)]
    pub person_id: Option<String>,
    #[serde(default)]
    pub person_name: Option<String>,
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
    pub sms_consent_version: String,
    pub sms_policy_version: String,
    pub status: String,
    pub submissions: String,
    pub unsubscribed_at: String,
    pub person_id: String,
    /// Display name from the Person directory when this lead is linked.
    pub person_name: Option<String>,
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
    let leads = store::leads::list(&surreal)
        .await
        .map_err(|error| ServerFnError::new(error.to_string()))?;
    let people = store::persons::find_by_ids(
        &surreal,
        &leads
            .iter()
            .filter_map(|lead| lead.person_id)
            .collect::<Vec<_>>(),
    )
    .await
    .map_err(|error| ServerFnError::new(error.to_string()))?;
    let people: std::collections::HashMap<_, _> = people
        .into_iter()
        .map(|person| (person.id, person))
        .collect();
    let rows = leads
        .into_iter()
        .map(|lead| {
            let person = lead.person_id.and_then(|id| people.get(&id));
            LeadListRow {
                id: lead.id.to_string(),
                email: person
                    .map(|person| person.email.clone())
                    .unwrap_or(lead.email),
                phone_masked: store::leads::mask_phone(
                    person
                        .and_then(|person| person.phone.as_deref())
                        .or(lead.phone.as_deref()),
                ),
                brand_key: lead.brand_key,
                source_path: lead.source_path,
                consent_version: lead.consent_version,
                consented_at: format_time(lead.consented_at),
                sms_consented_at: format_optional_time(lead.sms_consented_at),
                status: lead.status,
                submissions: lead.submissions.to_string(),
                person_id: person.map(|person| person.id.to_string()),
                person_name: person.map(|person| person.name.clone()),
            }
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

    let linked_person = if let Some(person_id) = lead.person_id {
        store::persons::find_by_id(&surreal, person_id)
            .await
            .map_err(|error| ServerFnError::new(error.to_string()))?
    } else {
        None
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
            email: linked_person
                .as_ref()
                .map_or_else(|| lead.email.clone(), |person| person.email.clone()),
            phone: linked_person
                .as_ref()
                .and_then(|person| person.phone.clone())
                .or(lead.phone)
                .unwrap_or_else(|| "—".to_string()),
            brand_key: lead.brand_key,
            source_path: lead.source_path,
            consent_version: lead.consent_version,
            consented_at: format_time(lead.consented_at),
            sms_consented_at: format_optional_time(lead.sms_consented_at),
            sms_consent_version: lead.sms_consent_version.unwrap_or_else(|| "—".to_string()),
            sms_policy_version: lead.sms_policy_version.unwrap_or_else(|| "—".to_string()),
            status: lead.status,
            submissions: lead.submissions.to_string(),
            unsubscribed_at: format_optional_time(lead.unsubscribed_at),
            person_id: lead
                .person_id
                .map_or_else(|| "—".to_string(), |id| id.to_string()),
            person_name: linked_person.map(|person| person.name),
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
                    "Public contact requests. Phone numbers on this list show only the last four digits. The firm talks to a lead under professional ethics, not as a sales queue."
                }
            }
            if is_empty {
                p { class: "nav-muted", role: "status", "No leads yet." }
            } else {
                DataTable {
                    columns: vec![
                        Column::fixed("email", "Email"),
                        Column::fixed("person", "Person"),
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
                            td {
                                if let (Some(person_id), Some(person_name)) =
                                    (row.person_id.as_ref(), row.person_name.as_ref())
                                {
                                    a { href: "/app/admin/people/{person_id}", "{person_name}" }
                                } else {
                                    "—"
                                }
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
                p { class: "nav-muted",
                    "The firm talks to a lead under professional ethics, not as a sales queue. A converted lead is a Person."
                }
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
                dt { "SMS consent version" }
                dd { "{lead.sms_consent_version}" }
                dt { "SMS policy version" }
                dd { "{lead.sms_policy_version}" }
                dt { "Status" }
                dd { "{lead.status}" }
                dt { "Submissions" }
                dd { "{lead.submissions}" }
                dt { "Unsubscribed" }
                dd { "{lead.unsubscribed_at}" }
                dt { "Person" }
                dd {
                    if lead.person_id == "—" {
                        "—"
                    } else if let Some(name) = lead.person_name.as_ref() {
                        a { href: "/app/admin/people/{lead.person_id}", "{name}" }
                    } else {
                        a { href: "/app/admin/people/{lead.person_id}", "{lead.person_id}" }
                    }
                }
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
            person_id: None,
            person_name: None,
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
                sms_consent_version: "—".to_string(),
                sms_policy_version: "—".to_string(),
                status: "new".to_string(),
                submissions: "1".to_string(),
                unsubscribed_at: "—".to_string(),
                person_id: "—".to_string(),
                person_name: None,
                existing_person_id: None,
            }),
        }));
        assert!(html.contains("+1 (555) 010-9876"), "{html}");
        assert!(html.contains("professional ethics"), "{html}");
        assert!(html.contains("Create Person"), "{html}");
        assert!(!html.contains(r#"value="converted""#), "{html}");
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
                sms_consent_version: "—".to_string(),
                sms_policy_version: "—".to_string(),
                status: "new".to_string(),
                submissions: "1".to_string(),
                unsubscribed_at: "—".to_string(),
                person_id: "—".to_string(),
                person_name: None,
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

    #[test]
    fn a_converted_lead_links_the_person_directory() {
        let html = dioxus_ssr::render_element(lead_show_body(&LeadShowView {
            tokens_href: String::new(),
            firm_name: "Neon Law".to_string(),
            role: ViewerRole::Admin,
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
                sms_consent_version: "—".to_string(),
                sms_policy_version: "—".to_string(),
                status: "converted".to_string(),
                submissions: "1".to_string(),
                unsubscribed_at: "—".to_string(),
                person_id: "22222222-2222-2222-2222-222222222222".to_string(),
                person_name: Some("Visitor Example".to_string()),
                existing_person_id: None,
            }),
        }));
        assert!(
            html.contains(r#"href="/app/admin/people/22222222-2222-2222-2222-222222222222""#),
            "{html}"
        );
        assert!(html.contains("Visitor Example"), "{html}");
        assert!(!html.contains("Create Person"), "{html}");
    }

    #[test]
    fn the_list_names_professional_ethics() {
        let html = dioxus_ssr::render_element(leads_list_body(&list_view(vec![sample_row()])));
        assert!(html.contains("professional ethics"), "{html}");
    }
}
