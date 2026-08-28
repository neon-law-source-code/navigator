//! Lawyer letter detail as a Dioxus component (#641 Phase 3, admin cluster).
//!
//! The successor to the `views::pages::admin::letters::detail` /
//! `not_found` views — the first migrated **detail** page (a single record's
//! fields, not a listing). It follows the listing seam: a `#[server]` function
//! reads the `{id}` path parameter and the injected `SurrealDb`, refuses any
//! non-lawyer caller, resolves the letter's mailroom (name + address) through the
//! same nested join the handler did, and `use_server_future` renders the
//! record's fields — or a not-found state when no letter has that id — into the
//! SSR HTML, readable before hydration.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::people::ViewerRole;

/// A single letter's displayed fields, in a wasm-safe shape.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct LetterFields {
    pub direction: String,
    pub sender: String,
    pub recipient: String,
    pub summary: String,
    pub mailroom_name: String,
    pub mailroom_address: String,
}

/// The rendered letter-detail view: the letter id, its fields (`None` when no
/// letter has that id), and the viewer's tier for the nav chrome.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct LetterDetailView {
    pub id: String,
    pub fields: Option<LetterFields>,
    pub role: ViewerRole,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// Fetch one letter for the `{id}` in the request path: refuse non-lawyer, read
/// the injected `SurrealDb`, load the letter, and resolve its mailroom name and
/// address through the same nested join the detail handler used. Returns
/// `fields: None` when the id resolves to no row (the not-found state).
#[server]
pub async fn get_letter() -> Result<LetterDetailView, ServerFnError> {
    let role = crate::admin_listing::require_lawyer().await?;
    let axum::extract::Path(id) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Path<uuid::Uuid>, _>()
            .await?;

    // The whole `letter -> mailroom -> address` chain lives in SurrealDB
    // now, so the detail page is one engine's work end to end.
    let surreal = consume_context::<store::surreal::SurrealDb>();
    let letter = store::letters::find_by_id(&surreal, id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let fields = match letter {
        None => None,
        Some(letter) => {
            let mailroom = store::mailrooms::find_by_id(&surreal, letter.mailroom_id)
                .await
                .map_err(|e| ServerFnError::new(e.to_string()))?;
            let (mailroom_name, mailroom_address) = match mailroom {
                Some(m) => {
                    let address = store::addresses::find_by_id(&surreal, m.address_id)
                        .await
                        .map_err(|e| ServerFnError::new(e.to_string()))?
                        .map_or_else(
                            || format!("(unknown address #{})", m.address_id),
                            |a| format!("{}, {}, {}", a.line1, a.city, a.region),
                        );
                    (m.name, address)
                }
                None => (
                    format!("(unknown mailroom #{})", letter.mailroom_id),
                    String::new(),
                ),
            };
            Some(LetterFields {
                direction: letter.direction,
                sender: letter.sender,
                recipient: letter.recipient,
                summary: letter.summary,
                mailroom_name,
                mailroom_address,
            })
        }
    };

    Ok(LetterDetailView {
        firm_name: crate::app_chrome::firm_name_from_context().await,
        id: id.to_string(),
        fields,
        role,
    })
}

/// The lawyer letter-detail page. Server-side rendered with the record already in
/// the markup (via [`use_server_future`]); renders a definition list of the
/// letter's fields, or a not-found state, with a back link to the listing.
#[component]
pub fn LawyerLetterDetail() -> Element {
    let resource = use_server_future(get_letter)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "letter-detail", p { "Failed to load letter." } }
            }
        }
        None => {
            return rsx! {
                main { id: "letter-detail", p { "Loading…" } }
            }
        }
    };

    let role = view.role;

    rsx! {
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        nav { class: "lawyer-nav",
            a { class: "nav-link", href: "/app/projects", "Portal" }
            if role.is_lawyer_tier() {
                a { class: "nav-link", href: "/lawyer", "Lawyer" }
            }
            if role.is_admin_tier() {
                a { class: "nav-link", href: "/app/admin", "Admin" }
            }
            a { class: "nav-link", href: "/auth/logout", "Sign out" }
        }
        main { id: "letter-detail", class: "nav-theme",
            match view.fields {
                Some(fields) => rsx! {
                    document::Title { "{view.firm_name} | Lawyer | Letters | Letter #{view.id}" }
                    header { class: "page-header",
                        h1 { "Letter #{view.id}" }
                        p { a { href: "/app/admin/letters", "← Back to letters" } }
                    }
                    dl { class: "admin-detail",
                        dt { "Direction" }
                        dd { "{fields.direction}" }
                        dt { "Sender" }
                        dd { "{fields.sender}" }
                        dt { "Recipient" }
                        dd { "{fields.recipient}" }
                        dt { "Summary" }
                        dd { "{fields.summary}" }
                        dt { "Mailroom" }
                        dd { "{fields.mailroom_name}" }
                        dt { "Mailroom address" }
                        dd { "{fields.mailroom_address}" }
                    }
                },
                None => rsx! {
                    document::Title { "{view.firm_name} | Lawyer | Letters | Not found" }
                    h1 { "Letter not found" }
                    p { "No letter exists with id " code { "{view.id}" } "." }
                    p { a { href: "/app/admin/letters", "← Back to letters" } }
                },
            }
        }
    }
}
