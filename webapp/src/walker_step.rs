//! The lawyer questionnaire walker step as a Dioxus component (#956 Phase 4) —
//! one question at a time on a notation, whatever template it is bound to.
//!
//! The successor to the `views::pages::admin::retainers::question_step`.
//! The supply-side mirror of [`crate::client_intake`]: same notation, same
//! per-answer-type controls, lawyer chrome. Both authorships interleave, so the
//! two walkers share [`crate::components::question_fields`] and cannot drift.
//!
//! **Where the step comes from.** Resolving it means calling
//! `workflows::notation_session::current_step` against the questionnaire
//! runtime, and `webapp` does not depend on `workflows`. The portal route's
//! pre-layer resolves it — the question, the prior answer, the template-derived
//! flow label, the progress and the seeded country options — and injects the
//! wasm-safe [`InjectedWalkerStep`] this page reads back. That pre-layer also
//! owns the `?format=json` CLI surface on the same path, the redirect once the
//! questionnaire is complete, and the `404`.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{question_fields, FormCard, PeopleListInputs};
use crate::people::ViewerRole;

/// The current walker step, shaped by the portal pre-layer into plain fields
/// that cross the server→client boundary.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct WalkerStepData {
    pub notation_id: String,
    /// The bound template's title ("Retainer Agreement", "Closing Letter"). The
    /// walker is generic over any notation, so the chrome names the actual
    /// template rather than assuming the retainer.
    pub flow_label: String,
    /// `Question.code` the form is asking — the row's payload key when the
    /// `POST` writes the answer.
    pub question_code: String,
    /// Human-readable prompt, rendered as the form label.
    pub question_prompt: String,
    /// `string`, `text`, `int`, `bool`, … — selects the input shape.
    pub answer_type: String,
    /// Prior answer to pre-fill, so navigating back re-displays it without
    /// mutating durable state.
    pub prior_answer: String,
    /// Seeded option names for a `country` question; empty for every other
    /// `answer_type`.
    pub country_options: Vec<String>,
    /// `(current, total)` — the lawyer-visible progress indicator.
    pub position: usize,
    pub total: usize,
}

/// The resolved step the portal pre-layer injects per request.
#[derive(Clone, Default)]
pub struct InjectedWalkerStep(pub WalkerStepData);

/// Everything the page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct WalkerStepView {
    /// The resolved brand's tokens stylesheet href, so the page wears
    /// its own palette rather than the firm's on a non-default host.
    #[serde(default)]
    pub tokens_href: String,
    pub step: WalkerStepData,
    pub csrf_token: String,
    pub role: ViewerRole,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// Read the injected step, the viewer's tier and the session CSRF token. The
/// runtime and database work already happened in the portal pre-layer, so this
/// loader only assembles them.
#[server]
pub async fn get_walker_step() -> Result<WalkerStepView, ServerFnError> {
    let role = crate::admin_listing::require_lawyer().await?;
    let InjectedWalkerStep(step) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<InjectedWalkerStep>, _>(
        )
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

    Ok(WalkerStepView {
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
        firm_name: crate::app_chrome::firm_name_from_context().await,
        step,
        csrf_token,
        role,
    })
}

/// The lawyer walker step.
#[component]
pub fn WalkerStep() -> Element {
    let resource = use_server_future(get_walker_step)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "walker-step", p { "Failed to load this step." } }
            }
        }
        None => {
            return rsx! {
                main { id: "walker-step", p { "Loading…" } }
            }
        }
    };
    step_body(&view)
}

/// The loaded step. Split from the component so the tests render a fixed view
/// without standing up the server function.
fn step_body(view: &WalkerStepView) -> Element {
    let step = &view.step;
    let action = format!("/app/lawyer/notations/{}/step", step.notation_id);
    let send_intake = format!("/app/lawyer/notations/{}/send-intake", step.notation_id);
    let clauses = format!("/app/lawyer/notations/{}/clauses", step.notation_id);
    let title = format!(
        "{} — step {} of {}",
        step.flow_label, step.position, step.total
    );
    let page_title = format!(
        "{} | Lawyer | Notations | {}",
        view.firm_name, step.flow_label
    );
    let role = view.role;
    let csrf = view.csrf_token.clone();
    let prompt = step.question_prompt.as_str();

    let is_people_list = step.answer_type == "people_list";
    let fields = question_fields(
        &step.answer_type,
        prompt,
        &step.prior_answer,
        &step.country_options,
    );
    let extra = is_people_list.then(|| {
        rsx! {
            PeopleListInputs { prior_json: step.prior_answer.clone(), rows: 3 }
        }
    });
    let code = step.question_code.clone();
    let intro = rsx! {
        "Question "
        code { "{code}" }
        "."
        if is_people_list {
            p { "{prompt}" }
        }
    };

    rsx! {
        document::Title { "{page_title}" }
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
        main { id: "walker-step", class: "nav-theme",
            div {
                role: "progressbar",
                "aria-label": "Intake progress",
                "aria-valuenow": "{step.position}",
                "aria-valuemin": "0",
                "aria-valuemax": "{step.total}",
            }
            FormCard {
                title,
                action,
                submit_label: "Continue".to_string(),
                csrf_token: Some(csrf.clone()),
                intro: Some(intro),
                extra_fields: extra,
                fields,
            }
            p {
                a { href: "/app/lawyer", "Save and exit" }
            }
            // Hand off to the client: they answer the client-facing questions
            // themselves, pre-filled with anything entered here, and both
            // authorships interleave on this notation.
            form {
                method: "post",
                action: "{send_intake}",
                "aria-label": "Send the client their intake link",
                input { r#type: "hidden", name: "_csrf", value: "{csrf}" }
                button { class: "nav-btn nav-btn--secondary", r#type: "submit",
                    "Send the client their intake link"
                }
            }
            // Add per-matter custom prose before sending — any clause routes the
            // document back through attorney review.
            p {
                a { href: "{clauses}", "Add custom clauses to this matter →" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::form::assert_forms_accessible;

    const NOTATION: &str = "00000000-0000-0000-0000-00000000002a";

    fn view(answer_type: &str, prior: &str, country_options: &[&str]) -> WalkerStepView {
        WalkerStepView {
            tokens_href: String::new(),
            firm_name: "Neon Law".to_string(),
            step: WalkerStepData {
                notation_id: NOTATION.to_string(),
                flow_label: "Closing letter".to_string(),
                question_code: "client_email".to_string(),
                question_prompt: "What is the client's email address?".to_string(),
                answer_type: answer_type.to_string(),
                prior_answer: prior.to_string(),
                country_options: country_options.iter().map(|s| (*s).to_string()).collect(),
                position: 2,
                total: 4,
            },
            csrf_token: "TOK".to_string(),
            role: ViewerRole::Lawyer,
        }
    }

    fn render(view: &WalkerStepView) -> String {
        dioxus_ssr::render_element(step_body(view))
    }

    #[test]
    fn the_step_names_its_question_progress_and_post_target() {
        let html = render(&view("string", "", &[]));
        assert!(
            html.contains(&format!("action=\"/app/lawyer/notations/{NOTATION}/step\"")),
            "{html}"
        );
        assert!(html.contains("client_email"), "{html}");
        // The chrome is template-driven, not hard-coded to the retainer.
        assert!(html.contains("Closing letter — step 2 of 4"), "{html}");
        assert!(html.contains("What is the client"), "{html}");
        assert!(html.contains("email address?"), "{html}");
        assert!(html.contains(">Continue</button>"), "{html}");
        assert!(html.contains("type=\"text\""), "{html}");
        assert_forms_accessible(&html, "walker_step");
    }

    #[test]
    fn the_step_exposes_accessible_progress_semantics() {
        let html = render(&view("string", "", &[]));
        assert!(html.contains(r#"role="progressbar""#), "{html}");
        assert!(html.contains(r#"aria-label="Intake progress""#), "{html}");
        assert!(html.contains(r#"aria-valuenow="2""#), "{html}");
        assert!(html.contains(r#"aria-valuemin="0""#), "{html}");
        assert!(html.contains(r#"aria-valuemax="4""#), "{html}");
    }

    #[test]
    fn a_prior_answer_pre_fills_so_navigating_back_re_displays_it() {
        let html = render(&view("string", "Libra", &[]));
        assert!(html.contains("value=\"Libra\""), "{html}");
    }

    #[test]
    fn a_text_answer_type_renders_a_textarea() {
        let html = render(&view("text", "", &[]));
        assert!(html.contains("<textarea"), "{html}");
    }

    #[test]
    fn a_country_answer_type_renders_the_seeded_picker() {
        let html = render(&view("country", "Mexico", &["Canada", "Mexico"]));
        assert!(html.contains("<select"), "{html}");
        assert!(html.contains("Select a country…"), "{html}");
        let mexico = html.find("value=\"Mexico\"").expect("the option renders");
        assert!(
            html[mexico..].starts_with("value=\"Mexico\" selected"),
            "{html}"
        );
    }

    #[test]
    fn a_people_list_question_renders_its_rows_inside_the_form() {
        // The rows are a composite widget the POST handler assembles into one
        // answer, so they must post with the form.
        let html = render(&view("people_list", "", &[]));
        assert!(html.contains("name=\"p0_name\""), "{html}");
        let form_open = html.find("<form").expect("the form renders");
        let form_close = html.find("</form>").expect("the form closes");
        let row = html.find("name=\"p0_name\"").unwrap();
        assert!(form_open < row && row < form_close, "{html}");
    }

    #[test]
    fn the_step_keeps_the_hand_off_and_clause_doors() {
        // Both are how a lawyer walk stops being a solo activity: the client can
        // take over their own questions, and per-matter prose can be added
        // before anything goes out.
        let html = render(&view("string", "", &[]));
        assert!(
            html.contains(&format!(
                "action=\"/app/lawyer/notations/{NOTATION}/send-intake\""
            )),
            "{html}"
        );
        assert!(
            html.contains(&format!(
                "href=\"/app/lawyer/notations/{NOTATION}/clauses\""
            )),
            "{html}"
        );
        // The hand-off is a write, so it carries the session CSRF token.
        assert_eq!(
            html.matches("name=\"_csrf\" value=\"TOK\"").count(),
            2,
            "{html}"
        );
    }
}
