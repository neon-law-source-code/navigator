//! Client self-serve intake as a Dioxus component (#956 Phase 4) — the
//! magic-link surface where a client answers (or confirms) the client-facing
//! questions on a notation.
//!
//! The successor to the `views::pages::portal::intake`. One question per
//! step, pre-filled with anything lawyer already entered on the client's behalf
//! and editable, with plain "here's what you're confirming" framing. The page
//! saves per step, so a drop-off resumes where the client left off.
//!
//! **Where the step comes from.** Resolving the current step means calling
//! `workflows::notation_session::client_intake_step`, and `webapp` does not
//! depend on `workflows`. The portal router's pre-layer resolves it — along with
//! the matter-visibility check, the flow label and the seeded country options —
//! and injects the wasm-safe [`InjectedIntake`] this page reads back, the same
//! seam [`crate::lawyer_project_detail::ProjectRepositoryPointer`] uses. The
//! pre-layer owns the 404, so an unauthorised or unknown notation never
//! reaches the render.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{
    question_fields, PeopleListInputs, PersonChoice, QuestionFieldContext, QuestionStage, StepMeta,
};

/// The current intake step, shaped by the portal pre-layer into plain fields
/// that cross the server→client boundary.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct IntakeStepData {
    pub project_id: String,
    pub project_code: String,
    pub notation_id: String,
    /// The bound template's title ("Retainer Agreement") — names what the client
    /// is filling in.
    pub flow_label: String,
    /// `Question.code` the form is asking — the `POST`'s question key.
    pub question_code: String,
    /// Human-readable prompt, rendered as the form label.
    pub question_prompt: String,
    /// `string`, `text`, `int`, `bool`, … — selects the input shape.
    pub answer_type: String,
    /// The attorney-authored guidance for this question
    /// (`Question.yaml`'s `help_text`), rendered as the `Hero` lede.
    /// `None` when the question declares none.
    #[serde(default)]
    pub help_text: Option<String>,
    /// Any current answer to pre-fill — including one lawyer entered on the
    /// client's behalf, which the client confirms or corrects.
    pub prior_value: String,
    /// Seeded option names for a `country` question; empty for every other
    /// `answer_type`.
    pub country_options: Vec<String>,
    /// `(value, label)` options for a `radio` question — the template's own
    /// declared choices. Empty for every other `answer_type`.
    #[serde(default)]
    pub choices: Vec<(String, String)>,
    /// The project-scoped people this notation's matter carries — a `person`
    /// question's real candidate list. Empty for every other `answer_type`,
    /// or when the matter has no participants yet.
    #[serde(default)]
    pub person_candidates: Vec<PersonChoice>,
    /// The full client-facing chain, for the `StepList` progress rail.
    #[serde(default)]
    pub steps: Vec<StepMeta>,
    /// `(current, total)` — client-facing progress.
    pub position: usize,
    pub total: usize,
}

/// Either a question to answer or the "your part is done" landing. The client
/// also lands on `Complete` once the document has gone out for signature, when
/// the answers are frozen.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub enum IntakeState {
    NeedsAnswer(Box<IntakeStepData>),
    Complete {
        project_code: String,
        flow_label: String,
        total: usize,
    },
}

impl Default for IntakeState {
    fn default() -> Self {
        Self::Complete {
            project_code: String::new(),
            flow_label: String::new(),
            total: 0,
        }
    }
}

/// The resolved intake state the portal pre-layer injects per request.
#[derive(Clone, Default)]
pub struct InjectedIntake(pub IntakeState);

/// Everything the page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ClientIntakeView {
    /// The resolved brand's tokens stylesheet href, so the page wears
    /// its own palette rather than the firm's on a non-default host.
    #[serde(default)]
    pub tokens_href: String,
    pub state: IntakeState,
    pub csrf_token: String,
    /// The `?error=` flash a rejected save redirects back with — a reference
    /// answer that matched nothing on the matter. `None` on a plain visit.
    #[serde(default)]
    pub error: Option<String>,
}

/// The intake page's `?error=` flash.
#[derive(Deserialize, Default)]
pub struct ClientIntakeQuery {
    #[serde(default)]
    pub error: Option<String>,
}

/// Read the injected step, the session CSRF token and any `?error=` flash. All
/// the database and workflow work already happened in the portal pre-layer, so
/// this loader only assembles them.
#[server]
pub async fn get_client_intake() -> Result<ClientIntakeView, ServerFnError> {
    let InjectedIntake(state) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<InjectedIntake>, _>()
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
    let axum::extract::Query(query) = dioxus_fullstack_core::FullstackContext::extract::<
        axum::extract::Query<ClientIntakeQuery>,
        _,
    >()
    .await?;

    Ok(ClientIntakeView {
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
        state,
        csrf_token,
        error: query.error.filter(|message| !message.is_empty()),
    })
}

/// The client intake page — one question, or the completion landing.
#[component]
pub fn ClientIntake() -> Element {
    let resource = use_server_future(get_client_intake)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "intake", p { "Failed to load your intake." } }
            }
        }
        None => {
            return rsx! {
                main { id: "intake", p { "Loading…" } }
            }
        }
    };
    intake_body(&view)
}

/// The loaded page. Split from the component so the tests render a fixed view
/// without standing up the server function.
fn intake_body(view: &ClientIntakeView) -> Element {
    match &view.state {
        IntakeState::NeedsAnswer(step) => step_body(step, view),
        IntakeState::Complete {
            project_code,
            flow_label,
            total,
        } => complete_body(project_code, flow_label, *total, &view.tokens_href),
    }
}

/// The single-field question form. `people_list` is a composite widget (several
/// inputs the `POST` handler assembles into one JSON answer); everything else is
/// one `value` control.
fn step_body(step: &IntakeStepData, view: &ClientIntakeView) -> Element {
    let prior = step.prior_value.as_str();
    let prompt = step.question_prompt.as_str();
    let action = format!(
        "/app/projects/{}/intake/{}",
        step.project_code, step.notation_id
    );
    let cancel = format!("/app/projects/{}", step.project_code);
    let page_title = format!("Your {} — Neon Law Navigator", step.flow_label);

    let is_people_list = step.answer_type == "people_list";
    let fields = question_fields(
        &step.answer_type,
        prompt,
        prior,
        &QuestionFieldContext {
            country_options: step.country_options.clone(),
            choices: step.choices.clone(),
            person_candidates: step.person_candidates.clone(),
        },
    );
    let extra = is_people_list.then(|| {
        rsx! {
            PeopleListInputs { prior_json: step.prior_value.clone(), rows: 3 }
        }
    });
    let intro = rsx! {
        if is_people_list {
            p { "{prompt}" }
        }
        "Your legal team started this for you. Confirm what's here or fix "
        "anything that's wrong, then continue — your answers save as you go, "
        "so you can finish later if you need to."
    };
    let footer = rsx! {
        p { a { href: "{cancel}", "Finish later" } }
    };

    rsx! {
        document::Title { "{page_title}" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        main { id: "intake", class: "nav-theme",
            if let Some(error) = view.error.as_ref() {
                p { class: "nav-form-error", role: "alert", "{error}" }
            }
            QuestionStage {
                eyebrow: step.flow_label.clone(),
                prompt: prompt.to_string(),
                help_text: step.help_text.clone(),
                steps: step.steps.clone(),
                position: step.position,
                total: step.total,
                action,
                csrf_token: view.csrf_token.clone(),
                submit_label: "Save and continue".to_string(),
                intro: Some(intro),
                extra_fields: extra,
                footer: Some(footer),
                fields,
            }
        }
    }
}

/// The "you're done with your part" landing, once the client has answered every
/// client-facing question — or once the document has gone out for signature and
/// the answers are frozen.
fn complete_body(project_code: &str, flow_label: &str, total: usize, tokens_href: &str) -> Element {
    let page_title = format!("Your {flow_label} — Neon Law Navigator");
    let back = format!("/app/projects/{project_code}");
    let _ = total;
    rsx! {
        document::Title { "{page_title}" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{tokens_href}" }
        main { id: "intake", class: "nav-theme",
            div { class: "nav-card",
                div { class: "nav-card__body",
                    h1 { "Thank you — your part is done" }
                    p { class: "nav-muted",
                        "You've answered everything we needed from you for your "
                        "{flow_label}. Your legal team will finish the rest, review "
                        "it, and send you the final document to sign. Nothing goes "
                        "out until an attorney has reviewed it."
                    }
                    a { class: "nav-btn nav-btn--primary", href: "{back}", "Back to your matter" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::form::assert_forms_accessible;

    fn step(answer_type: &str, prior: &str, country_options: &[&str]) -> ClientIntakeView {
        step_with_choices(answer_type, prior, country_options, &[])
    }

    fn step_with_choices(
        answer_type: &str,
        prior: &str,
        country_options: &[&str],
        choices: &[(&str, &str)],
    ) -> ClientIntakeView {
        ClientIntakeView {
            tokens_href: String::new(),
            state: IntakeState::NeedsAnswer(Box::new(IntakeStepData {
                project_id: "00000000-0000-0000-0000-000000000001".to_string(),
                project_code: "sample-litigation".to_string(),
                notation_id: "00000000-0000-0000-0000-000000000002".to_string(),
                flow_label: "Application for Naturalization".to_string(),
                question_code: "country__of_birth".to_string(),
                question_prompt: "In what country were you born?".to_string(),
                answer_type: answer_type.to_string(),
                help_text: None,
                prior_value: prior.to_string(),
                country_options: country_options.iter().map(|s| (*s).to_string()).collect(),
                choices: choices
                    .iter()
                    .map(|(v, l)| ((*v).to_string(), (*l).to_string()))
                    .collect(),
                person_candidates: Vec::new(),
                steps: vec![
                    StepMeta::new("s1", "First"),
                    StepMeta::new("s2", "Second"),
                    StepMeta::new("country__of_birth", "Of birth"),
                ],
                position: 3,
                total: 10,
            })),
            csrf_token: "TOK".to_string(),
            error: None,
        }
    }

    fn step_with_person_candidates(prior: &str, people: Vec<PersonChoice>) -> ClientIntakeView {
        let mut view = step("person", prior, &[]);
        if let IntakeState::NeedsAnswer(data) = &mut view.state {
            data.person_candidates = people;
        }
        view
    }

    fn render(view: &ClientIntakeView) -> String {
        dioxus_ssr::render_element(intake_body(view))
    }

    #[test]
    fn a_step_posts_back_to_the_intake_handler_with_the_csrf_token() {
        let html = render(&step("string", "", &[]));
        assert!(
            html.contains(
                "action=\"/app/projects/sample-litigation\
                 /intake/00000000-0000-0000-0000-000000000002\""
            ),
            "{html}"
        );
        assert!(html.contains("name=\"_csrf\""), "{html}");
        assert!(html.contains("value=\"TOK\""), "{html}");
        assert!(html.contains("name=\"value\""), "{html}");
        // The client sees where they are without having to guess.
        assert!(html.contains("Step 3 of 10"), "{html}");
        assert_forms_accessible(&html, "client_intake::step");
    }

    #[test]
    fn the_step_renders_the_full_chain_with_the_current_step_marked() {
        let html = render(&step("string", "", &[]));
        assert!(html.contains("nav-stage"), "{html}");
        assert_eq!(html.matches("nav-steps__item").count(), 3, "{html}");
        assert!(html.contains(r#"aria-current="step""#), "{html}");
    }

    #[test]
    fn help_text_renders_as_the_hero_lede_when_the_question_declares_one() {
        let mut view = step("string", "", &[]);
        if let IntakeState::NeedsAnswer(data) = &mut view.state {
            data.help_text = Some("Use the address on the engagement letter.".to_string());
        }
        let html = render(&view);
        assert!(html.contains("nav-hero__lede"), "{html}");
    }

    #[test]
    fn no_help_text_omits_the_hero_lede() {
        let html = render(&step("string", "", &[]));
        assert!(!html.contains("nav-hero__lede"), "{html}");
    }

    #[test]
    fn a_step_exposes_accessible_progress_semantics() {
        let html = render(&step("string", "", &[]));
        assert!(html.contains("nav-progress"), "{html}");
        assert!(html.contains(r#"role="progressbar""#), "{html}");
        assert!(html.contains(r#"aria-label="Intake progress""#), "{html}");
        assert!(html.contains(r#"aria-valuenow="3""#), "{html}");
        assert!(html.contains(r#"aria-valuemin="0""#), "{html}");
        assert!(html.contains(r#"aria-valuemax="10""#), "{html}");
    }

    #[test]
    fn country_question_renders_a_select_of_seeded_names() {
        let html = render(&step("country", "", &["Canada", "Mexico"]));
        assert!(html.contains("<select"), "{html}");
        assert!(html.contains("Select a country…"), "{html}");
        assert!(html.contains("value=\"Canada\""), "{html}");
        assert!(html.contains("value=\"Mexico\""), "{html}");
    }

    #[test]
    fn country_question_preselects_the_prior_answer() {
        let html = render(&step("country", "Mexico", &["Canada", "Mexico"]));
        // Dioxus emits the boolean attribute on the chosen option only.
        let mexico = html.find("value=\"Mexico\"").expect("the option renders");
        assert!(
            html[mexico..].starts_with("value=\"Mexico\" selected"),
            "{html}"
        );
    }

    #[test]
    fn eng_454_a_radio_question_renders_the_templates_own_choices() {
        let html = render(&step_with_choices(
            "radio",
            "",
            &[],
            &[("nevada", "Nevada"), ("california", "California")],
        ));
        assert!(html.contains(r#"type="radio""#), "{html}");
        assert!(
            html.contains("Nevada") && html.contains("California"),
            "{html}"
        );
        // ENG-504: cards, not a compact radio list.
        assert!(html.contains("nav-choice-group"), "{html}");
    }

    #[test]
    fn eng_504_a_bool_question_renders_a_two_card_yes_no_choice() {
        let html = render(&step("bool", "false", &[]));
        assert!(html.contains("nav-choice-group"), "{html}");
        assert!(html.contains(">Yes<") && html.contains(">No<"), "{html}");
    }

    #[test]
    fn eng_454_a_person_question_with_candidates_renders_a_picker() {
        let people = vec![PersonChoice::new(
            "00000000-0000-0000-0000-000000000010",
            "Ada Lovelace",
            "ada@example.com",
        )];
        let html = render(&step_with_person_candidates("", people));
        assert!(html.contains("Ada Lovelace"), "{html}");
        assert!(!html.contains(r#"type="text""#), "{html}");
    }

    #[test]
    fn eng_454_a_person_question_with_no_candidates_keeps_the_free_text_path() {
        let html = render(&step_with_person_candidates("", Vec::new()));
        assert!(html.contains(r#"type="text""#), "{html}");
    }

    #[test]
    fn custom_phone_question_renders_a_tel_input() {
        let html = render(&step("custom_phone", "", &[]));
        assert!(html.contains("type=\"tel\""), "{html}");
    }

    #[test]
    fn custom_usd_question_carries_the_dollar_prefix_and_cent_step() {
        let html = render(&step("custom_usd", "", &[]));
        assert!(html.contains("step=\"0.01\""), "{html}");
        assert!(html.contains(">$<"), "{html}");
    }

    #[test]
    fn a_people_list_question_renders_its_row_groups_inside_the_form() {
        // The rows are a composite widget the handler assembles into one answer,
        // so they must post with the form — i.e. sit inside the `<form>`, after
        // the (empty) field list.
        let html = render(&step("people_list", "", &[]));
        assert!(html.contains("name=\"p0_name\""), "{html}");
        let form_open = html.find("<form").expect("the form renders");
        let form_close = html.find("</form>").expect("the form closes");
        let row = html.find("name=\"p0_name\"").unwrap();
        assert!(form_open < row && row < form_close, "{html}");
    }

    #[test]
    fn a_rejected_save_renders_its_reason_above_the_question() {
        let mut view = step("string", "", &[]);
        view.error = Some("No one on this matter matches that.".to_string());
        let html = render(&view);
        assert!(html.contains("nav-form-error"), "{html}");
        assert!(
            html.contains(">No one on this matter matches that.<"),
            "{html}"
        );
    }

    #[test]
    fn the_completion_landing_links_back_to_the_matter_and_offers_no_write() {
        let html = render(&ClientIntakeView {
            tokens_href: String::new(),
            state: IntakeState::Complete {
                project_code: "sample-litigation".to_string(),
                flow_label: "Retainer Agreement".to_string(),
                total: 4,
            },
            csrf_token: "TOK".to_string(),
            error: None,
        });
        assert!(html.contains("Thank you — your part is done"), "{html}");
        assert!(
            html.contains("href=\"/app/projects/sample-litigation\""),
            "{html}"
        );
        // Nothing on the finished page invites another answer.
        assert!(!html.contains("<form"), "{html}");
    }
}
