//! The admin clause editor as a Dioxus component (#956 Phase 4) — add, edit,
//! reorder, and remove the custom paragraphs spliced into a single notation's
//! assembled document before it is sent. Per-matter prose without forking the
//! shared template.
//!
//! The successor to the `views::pages::admin::clauses`. Every control is a
//! native `POST` form to an unchanged handler that redirects back here (PRG), so
//! the page works without JavaScript.
//!
//! `GET …/clauses?format=json` is a separate, thin JSON surface the
//! `navigator retainer clause list` CLI consumes. It stays on the portal side:
//! the Dioxus route's pre-layer answers that query before the render runs.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Field, FormCard, Heading};
use crate::people::ViewerRole;

/// One clause row for display.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ClauseRow {
    pub id: String,
    pub body: String,
}

/// The clause editor for one notation.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ClauseEditorView {
    pub notation_id: String,
    /// The bound template's title, for the page chrome.
    pub flow_label: String,
    pub clauses: Vec<ClauseRow>,
    pub csrf_token: String,
    pub role: ViewerRole,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// Load the clause editor: refuse non-lawyer, resolve the notation's template
/// title, and list its clauses in position order. A notation (or template) that
/// is gone commits a real `404` rather than rendering an empty editor.
#[server]
pub async fn get_clause_editor() -> Result<ClauseEditorView, ServerFnError> {
    let role = crate::admin_listing::require_lawyer().await?;
    let axum::extract::Path(notation_id) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Path<uuid::Uuid>, _>()
            .await?;
    let csrf_token = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::csrf::CsrfToken>,
        _,
    >()
    .await
    .map(|axum::Extension(token)| token.0)
    .unwrap_or_default();

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let Some(flow_label) = notation_flow_label(&surreal, notation_id).await else {
        dioxus_fullstack_core::FullstackContext::commit_http_status(
            axum::http::StatusCode::NOT_FOUND,
            None,
        );
        return Err(ServerFnError::new("notation not found"));
    };
    let clauses = store::notation_clauses::for_notation(&surreal, notation_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .into_iter()
        .map(|c| ClauseRow {
            id: c.id.to_string(),
            body: c.body_markdown,
        })
        .collect();

    Ok(ClauseEditorView {
        firm_name: crate::app_chrome::firm_name_from_context().await,
        notation_id: notation_id.to_string(),
        flow_label,
        clauses,
        csrf_token,
        role,
    })
}

/// The bound template's title, for the page chrome. `None` when the notation or
/// its template is gone — the editor's `404`.
#[cfg(feature = "server")]
async fn notation_flow_label(
    surreal: &store::surreal::SurrealDb,
    notation_id: uuid::Uuid,
) -> Option<String> {
    let n = store::notations::find_by_id(surreal, notation_id)
        .await
        .ok()
        .flatten()?;
    let t = store::templates::find_by_id(surreal, n.template_id)
        .await
        .ok()
        .flatten()?;
    Some(t.title)
}

/// The clause editor. Server-side rendered; every control is a native `POST`.
#[component]
pub fn ClauseEditor() -> Element {
    let resource = use_server_future(get_clause_editor)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "clauses", p { "Failed to load the clause editor." } }
            }
        }
        None => {
            return rsx! {
                main { id: "clauses", p { "Loading…" } }
            }
        }
    };
    editor_body(&view)
}

/// The loaded editor. Split from the component so the tests render a fixed view
/// without standing up the server function.
fn editor_body(view: &ClauseEditorView) -> Element {
    let base = format!("/app/lawyer/notations/{}/clauses", view.notation_id);
    let back = format!("/app/lawyer/notations/{}/step", view.notation_id);
    let page_title = format!(
        "{} | Lawyer | Notations | {} | Custom clauses",
        view.firm_name, view.flow_label
    );
    let role = view.role;
    let csrf = view.csrf_token.clone();
    let is_empty = view.clauses.is_empty();

    rsx! {
        document::Title { "{page_title}" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
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
        main { id: "clauses", class: "nav-theme",
            p {
                a { href: "{back}", "← Back to the intake walk" }
            }
            header { class: "page-header",
                h1 { "Custom clauses" }
                p { class: "nav-muted",
                    "Paragraphs added here are spliced into this matter's "
                    "{view.flow_label} at its custom-clauses marker, in order. Any "
                    "clause sends the document back through attorney review before "
                    "it can go out for signature."
                }
            }
            if is_empty {
                p { class: "nav-muted", "No custom clauses yet." }
            }
            for (index , clause) in view.clauses.iter().enumerate() {
                ClauseCard {
                    key: "{clause.id}",
                    base: base.clone(),
                    csrf: csrf.clone(),
                    clause: clause.clone(),
                    ordinal: index + 1,
                }
            }
            FormCard {
                title: "Add a clause".to_string(),
                action: base.clone(),
                submit_label: "Add clause".to_string(),
                heading: Heading::H2,
                csrf_token: Some(csrf.clone()),
                fields: vec![
                    Field::textarea("Clause text", "body", "", 3)
                        .required()
                        .id("clause-new")
                        .placeholder("A custom paragraph for this matter only…"),
                ],
            }
        }
    }
}

/// One existing clause: its editable body, the two reorder buttons, and delete.
/// Each is its own native `POST` to an unchanged handler that redirects back.
#[component]
fn ClauseCard(base: String, csrf: String, clause: ClauseRow, ordinal: usize) -> Element {
    let edit_action = format!("{base}/{}/edit", clause.id);
    let move_action = format!("{base}/{}/move", clause.id);
    let delete_action = format!("{base}/{}/delete", clause.id);
    let label = format!("Clause {ordinal}");
    rsx! {
        div { class: "clause-card",
            // Through the shared card, not a hand-rolled `<textarea>`: a
            // textarea is RCDATA, so Dioxus SSR's hydration comments would land
            // inside it as *literal text* and be saved back into the clause.
            // `Field::textarea` writes the body as escaped inner HTML instead.
            FormCard {
                title: label.clone(),
                action: edit_action,
                submit_label: "Save".to_string(),
                heading: Heading::H2,
                csrf_token: Some(csrf.clone()),
                fields: vec![
                    Field::textarea("Clause text", "body", clause.body.clone(), 3)
                        .required()
                        // Every clause form posts `body`, so the DOM id has to
                        // come from the clause instead — duplicate ids break
                        // `<label for>` targeting across the page.
                        .id(format!("clause-{}", clause.id)),
                ],
            }
            div { class: "row-actions", role: "group", "aria-label": "{label} actions",
                    MoveForm {
                        action: move_action.clone(),
                        csrf: csrf.clone(),
                        direction: "up".to_string(),
                        label: "Move up".to_string(),
                    }
                    MoveForm {
                        action: move_action,
                        csrf: csrf.clone(),
                        direction: "down".to_string(),
                        label: "Move down".to_string(),
                    }
                form { method: "post", action: "{delete_action}", "aria-label": "Delete {label}",
                    input { r#type: "hidden", name: "_csrf", value: "{csrf}" }
                    button { class: "nav-btn nav-btn--danger", r#type: "submit", "Delete" }
                }
            }
        }
    }
}

/// One reorder button — a `POST` carrying the direction.
#[component]
fn MoveForm(action: String, csrf: String, direction: String, label: String) -> Element {
    rsx! {
        form { method: "post", action: "{action}", "aria-label": "{label}",
            input { r#type: "hidden", name: "_csrf", value: "{csrf}" }
            input { r#type: "hidden", name: "direction", value: "{direction}" }
            button { class: "nav-btn nav-btn--secondary", r#type: "submit", "{label}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::form::assert_forms_accessible;

    const NOTATION: &str = "00000000-0000-0000-0000-00000000002a";

    fn view(clauses: &[(&str, &str)]) -> ClauseEditorView {
        ClauseEditorView {
            firm_name: "Neon Law".to_string(),
            notation_id: NOTATION.to_string(),
            flow_label: "Retainer Agreement".to_string(),
            clauses: clauses
                .iter()
                .map(|(id, body)| ClauseRow {
                    id: (*id).to_string(),
                    body: (*body).to_string(),
                })
                .collect(),
            csrf_token: "TOK".to_string(),
            role: ViewerRole::Lawyer,
        }
    }

    fn render(view: &ClauseEditorView) -> String {
        dioxus_ssr::render_element(editor_body(view))
    }

    #[test]
    fn an_empty_editor_says_so_and_still_offers_the_add_form() {
        let html = render(&view(&[]));
        assert!(html.contains("No custom clauses yet."), "{html}");
        assert!(
            html.contains(&format!("action=\"/app/lawyer/notations/{NOTATION}/clauses\"")),
            "{html}"
        );
        assert_forms_accessible(&html, "clause_editor (empty)");
    }

    #[test]
    fn each_clause_carries_its_edit_reorder_and_delete_posts() {
        let html = render(&view(&[(
            "11111111-1111-1111-1111-111111111111",
            "First para.",
        )]));
        let base =
            format!("/app/lawyer/notations/{NOTATION}/clauses/11111111-1111-1111-1111-111111111111");
        for suffix in ["/edit", "/move", "/delete"] {
            assert!(
                html.contains(&format!("action=\"{base}{suffix}\"")),
                "missing {suffix}: {html}"
            );
        }
        // Both directions post, so a clause can move either way.
        assert!(html.contains("value=\"up\""), "{html}");
        assert!(html.contains("value=\"down\""), "{html}");
        // Every write form carries the session CSRF token.
        assert_eq!(
            html.matches("name=\"_csrf\" value=\"TOK\"").count(),
            5,
            "edit + 2 moves + delete + add: {html}"
        );
    }

    #[test]
    fn a_clause_body_renders_as_the_textareas_content() {
        // A textarea's value is its content, not a `value` attribute — and it
        // is RCDATA, so a Dioxus child text node would put the SSR hydration
        // comments *in the box* as literal text and save them back into the
        // clause. `Field::textarea` writes it as escaped inner HTML.
        let html = render(&view(&[(
            "11111111-1111-1111-1111-111111111111",
            "The firm may withdraw.",
        )]));
        assert!(
            html.contains(">The firm may withdraw.</textarea>"),
            "{html}"
        );
    }

    #[test]
    fn clause_bodies_are_escaped_not_injected() {
        let html = render(&view(&[(
            "11111111-1111-1111-1111-111111111111",
            "</textarea><script>alert(1)</script>",
        )]));
        // A textarea is RCDATA: the browser decodes the character references
        // back to text, so nothing here closes the element or runs. Dioxus
        // escapes angle brackets as `&#60;`/`&#62;`, not `&lt;`/`&gt;`.
        assert!(!html.contains("<script>"), "{html}");
        assert!(!html.contains("</textarea><script"), "{html}");
        assert!(html.contains("&lt;script>alert(1)"), "{html}");
    }

    #[test]
    fn the_editor_numbers_clauses_in_position_order() {
        let html = render(&view(&[
            ("11111111-1111-1111-1111-111111111111", "First."),
            ("22222222-2222-2222-2222-222222222222", "Second."),
        ]));
        let first = html.find("Clause 1").expect("first is numbered");
        let second = html.find("Clause 2").expect("second is numbered");
        assert!(first < second, "{html}");
        assert_forms_accessible(&html, "clause_editor (two clauses)");
    }
}
