//! The lawyer-on-behalf re-ask surface as a Dioxus component (#956 Phase 4) —
//! re-collect the answers a `lawyer_review` flagged, then resubmit for review.
//!
//! The successor to the `views::pages::admin::retainers::reask`, and the
//! last render in that module.
//!
//! **Only the flagged answers are re-collected.** Every other answer stays as it
//! was, so a matter sent back over one wrong name does not lose the rest of the
//! intake. The same shared engine backs the client's self-serve path from their
//! portal, which is why the note explains what to fix rather than assuming lawyer
//! will be the one fixing it.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Field, FormCard};
use crate::people::ViewerRole;

/// One answer flagged for re-collection.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct FlaggedQuestion {
    /// The question's state code — the input posts as `a:{code}`.
    pub code: String,
    pub label: String,
}

/// The re-ask screen, resolved by the portal pre-layer.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ReaskData {
    pub notation_id: String,
    pub flagged: Vec<FlaggedQuestion>,
    /// The reviewer's note explaining what to fix, if they left one.
    pub note: Option<String>,
}

/// The resolved re-ask screen the portal pre-layer injects per request.
#[derive(Clone, Default)]
pub struct InjectedReask(pub ReaskData);

/// Everything the page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ReaskView {
    /// The resolved brand's tokens stylesheet href, so the page wears
    /// its own palette rather than the firm's on a non-default host.
    #[serde(default)]
    pub tokens_href: String,
    pub data: ReaskData,
    pub csrf_token: String,
    pub role: ViewerRole,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// Read the injected re-ask data, the viewer's tier and the session CSRF token.
/// The change-request lookup and label resolution already happened in the portal
/// pre-layer, so this loader only assembles them.
#[server]
pub async fn get_reask() -> Result<ReaskView, ServerFnError> {
    let role = crate::admin_listing::require_lawyer().await?;
    let InjectedReask(data) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<InjectedReask>, _>()
            .await
            .map(|axum::Extension(injected)| injected)
            .unwrap_or_default();
    let csrf_token = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::csrf::CsrfToken>,
        _,
    >()
    .await
    .map(|axum::Extension(token)| token.0)
    .unwrap_or_default();

    Ok(ReaskView {
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
        firm_name: crate::app_chrome::firm_name_from_context().await,
        data,
        csrf_token,
        role,
    })
}

/// The lawyer re-ask screen.
#[component]
pub fn Reask() -> Element {
    let resource = use_server_future(get_reask)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "reask", p { "Failed to load the flagged answers." } }
            }
        }
        None => {
            return rsx! {
                main { id: "reask", p { "Loading…" } }
            }
        }
    };
    reask_body(&view)
}

/// The loaded screen. Split from the component so the tests render a fixed view
/// without standing up the server function.
fn reask_body(view: &ReaskView) -> Element {
    let data = &view.data;
    let action = format!("/app/lawyer/notations/{}/reask", data.notation_id);
    let review = format!("/app/lawyer/notations/{}/review", data.notation_id);
    let role = view.role;
    let is_empty = data.flagged.is_empty();
    // One required text input per flagged question, posting under `a:{code}` —
    // the key `store::reask` gates the write against the flagged set by.
    let fields: Vec<Field> = data
        .flagged
        .iter()
        .map(|q| {
            Field::text(q.label.clone(), format!("a:{}", q.code), "")
                .required()
                .id(q.code.clone())
        })
        .collect();

    rsx! {
        document::Title { "{view.firm_name} | Lawyer | Notations | Re-collect flagged answers" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
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
        main { id: "reask", class: "nav-theme",
            header { class: "page-header",
                h1 { "Re-collect flagged answers" }
                p { class: "nav-muted",
                    "This matter was sent back for changes. Correct the flagged answers "
                    "below — on the client's behalf, or have the client do it from their "
                    "portal — then resubmit for review. Only the flagged answers are "
                    "re-collected; every other answer stays as it was."
                }
            }
            if let Some(note) = data.note.as_ref() {
                dl { class: "detail-dl",
                    dt { "Reviewer note" }
                    dd { "{note}" }
                }
            }
            if is_empty {
                p { class: "nav-muted", "No answers are flagged for re-collection." }
            } else {
                FormCard {
                    title: "Corrected answers".to_string(),
                    action,
                    submit_label: "Save and resubmit for review".to_string(),
                    csrf_token: Some(view.csrf_token.clone()),
                    fields,
                }
            }
            p {
                a { href: "{review}", "Back to review" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::form::assert_forms_accessible;

    const NOTATION: &str = "00000000-0000-0000-0000-00000000002a";

    fn view(flagged: &[(&str, &str)], note: Option<&str>) -> ReaskView {
        ReaskView {
            tokens_href: String::new(),
            firm_name: "Neon Law".to_string(),
            data: ReaskData {
                notation_id: NOTATION.to_string(),
                flagged: flagged
                    .iter()
                    .map(|(code, label)| FlaggedQuestion {
                        code: (*code).to_string(),
                        label: (*label).to_string(),
                    })
                    .collect(),
                note: note.map(str::to_string),
            },
            csrf_token: "TOK".to_string(),
            role: ViewerRole::Lawyer,
        }
    }

    fn render(view: &ReaskView) -> String {
        dioxus_ssr::render_element(reask_body(view))
    }

    #[test]
    fn each_flagged_answer_gets_its_own_input_under_the_engines_key() {
        let html = render(&view(
            &[
                ("person__client", "Client name"),
                ("project__engagement", "Project name"),
            ],
            None,
        ));
        assert!(
            html.contains(&format!(
                "action=\"/app/lawyer/notations/{NOTATION}/reask\""
            )),
            "{html}"
        );
        // `a:{code}` is what `store::reask` gates the write against.
        assert!(html.contains("name=\"a:person__client\""), "{html}");
        assert!(html.contains("name=\"a:project__engagement\""), "{html}");
        assert!(html.contains("Client name"), "{html}");
        assert!(html.contains("name=\"_csrf\" value=\"TOK\""), "{html}");
        assert_forms_accessible(&html, "reask::Reask");
    }

    #[test]
    fn the_reviewers_note_is_shown_when_they_left_one() {
        let html = render(&view(
            &[("person__client", "Client name")],
            Some("The client's surname is misspelled."),
        ));
        assert!(html.contains("Reviewer note"), "{html}");
        assert!(html.contains("surname is misspelled"), "{html}");
    }

    #[test]
    fn no_note_renders_no_empty_note_block() {
        let html = render(&view(&[("person__client", "Client name")], None));
        assert!(!html.contains("Reviewer note"), "{html}");
    }

    #[test]
    fn nothing_flagged_offers_no_form_to_submit() {
        // A matter with an empty flag set has nothing to re-collect; offering a
        // submit would post an empty answer set back into the engine.
        let html = render(&view(&[], None));
        assert!(
            html.contains("No answers are flagged for re-collection."),
            "{html}"
        );
        assert!(!html.contains("<form"), "{html}");
        // The way back is still there.
        assert!(
            html.contains(&format!("href=\"/app/lawyer/notations/{NOTATION}/review\"")),
            "{html}"
        );
    }
}
