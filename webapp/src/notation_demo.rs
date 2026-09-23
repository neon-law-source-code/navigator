//! `/notations/{slug}`'s "Try answering this" section (ENG-452) — a
//! client-side-only walk through the template's real declared questionnaire
//! order, rendered with Navigator's real [`Field`] controls.
//!
//! **This never persists anything.** There is no `#[server]` function and no
//! `<form action>` anywhere in this module — advancing or going back is a
//! plain [`use_signal`] index into a fixed list resolved once at render time.
//! That is a structural guarantee, not a runtime toggle: there is no wire to
//! a mutation endpoint for a stray edit to reconnect. Contrast the two real,
//! authenticated walkers that share this same [`Field`] library —
//! `portal::retainer_walk` and [`crate::client_intake`] — both of which do
//! post to a notation. This module is not a third walker and must not become
//! one.
//!
//! Every real Notation is bound to a Project (see `docs/notation.md#notation`
//! in the workspace root); [`SAMPLE_PROJECT_LABEL`] names an obviously
//! synthetic one so the demo never implies an unscoped Notation can exist,
//! and no `projects` or `notation` row is created to back it.
//!
//! A record/reference question (`person`, `entity`, `signature`, …) answers
//! by creating or linking a real database row, which this demo cannot
//! meaningfully fake — [`DemoQuestion`]'s `interactive` field is `false` for
//! those, and the page shows the explanation `views::questionnaire_preview`
//! built for it instead of a control that only pretends to work.

use std::collections::BTreeMap;

use dioxus::prelude::*;

use crate::components::{Progress, StepList, StepMeta};

/// A clearly-synthetic sample matter, named in the demo's chrome so the walk
/// never reads as scoped to nothing — every real Notation has a Project.
pub const SAMPLE_PROJECT_LABEL: &str = "Sample matter: Acme LLC";

/// One step of the demo — the plain-data mirror of
/// `views::questionnaire_preview::PreviewQuestion` that crosses the `neon` →
/// `webapp` boundary, the same pattern
/// [`crate::notation_preview::PreviewDoc`] uses for the rest of this page's
/// content.
#[derive(Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct DemoQuestion {
    pub code: String,
    pub answer_type: String,
    pub prompt: String,
    /// `(value, label)` options — populated only when [`Self::interactive`]
    /// is `true` and the answer type is `custom_single_choice`.
    pub choices: Vec<(String, String)>,
    /// Whether this step has a real control to demo, or is a record/reference
    /// state rendered as a short explanation instead.
    pub interactive: bool,
}

/// The client-side-only questionnaire stepper. Renders nothing for a
/// notation with no declared questionnaire (a template whose frontmatter
/// carries none, or one `views::questionnaire_preview::parse` couldn't read).
#[component]
pub fn QuestionnaireDemo(
    questions: Vec<DemoQuestion>,
    #[props(default)] template_html: String,
) -> Element {
    let Some(total) = std::num::NonZeroUsize::new(questions.len()) else {
        return rsx! {};
    };
    let total = total.get();
    let mut step = use_signal(|| 0_usize);
    let answers = use_signal(BTreeMap::<String, String>::new);
    let index = *step.read();
    let question = &questions[index];
    let position = index + 1;
    let steps: Vec<StepMeta> = questions
        .iter()
        .map(|q| StepMeta::new(q.code.clone(), q.prompt.clone()))
        .collect();

    rsx! {
        section { class: "notation-demo", "aria-label": "Try answering this",
            h2 { "Try answering this" }
            p { class: "nav-muted",
                "{SAMPLE_PROJECT_LABEL} — nothing you type below is saved anywhere."
            }
            div { class: "nav-stepper",
                StepList {
                    steps,
                    current: index,
                    label: "Demo question progress".to_string(),
                }
                p { class: "nav-stepper__count", "Step {position} of {total}" }
                Progress {
                    label: "Demo question progress".to_string(),
                    value: Some(position),
                    max: total,
                }
                div {
                    class: "nav-stepper__body notation-demo__step",
                    key: "{question.code}",
                    {demo_field(question, answers)}
                }
            }
            div { class: "notation-demo__actions",
                if index > 0 {
                    button {
                        class: "nav-btn nav-btn--secondary",
                        r#type: "button",
                        onclick: move |_| step.set(index - 1),
                        "Back"
                    }
                }
                if position < total {
                    button {
                        class: "nav-btn nav-btn--primary",
                        r#type: "button",
                        onclick: move |_| step.set(index + 1),
                        "Continue"
                    }
                } else {
                    span { class: "nav-muted", "That is every question this notation asks." }
                }
            }
            if !template_html.is_empty() {
                section { class: "notation-live-template", "aria-label": "Live document preview",
                    h3 { "Live document" }
                    div {
                        dangerous_inner_html: "{render_live_template(&template_html, &questions, &answers.read())}"
                    }
                }
            }
        }
    }
}

fn answer_label(question: &DemoQuestion, value: &str) -> String {
    if question.answer_type == "custom_yes_no" {
        return match value {
            "true" => "Yes".to_string(),
            "false" => "No".to_string(),
            _ => value.to_string(),
        };
    }
    question
        .choices
        .iter()
        .find(|(candidate, _)| candidate == value)
        .map_or_else(|| value.to_string(), |(_, label)| label.clone())
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn replace_unanswered(mut html: String, questions: &[DemoQuestion]) -> String {
    for question in questions {
        let token = format!("{{{{{}}}}}", question.code);
        let blank = format!(
            "<span class=\"notation-live-template__blank\" data-placeholder=\"{}\">{}</span>",
            escape_html(&question.code),
            escape_html(&question.prompt)
        );
        html = html.replace(&token, &blank);
    }
    while let Some(start) = html.find("{{") {
        let Some(end) = html[start + 2..].find("}}") else {
            break;
        };
        html.replace_range(
            start..start + end + 4,
            "<span class=\"notation-live-template__blank\"></span>",
        );
    }
    html
}

/// Render the same template on every answer change. Answer text is escaped
/// before it reaches the template's already-rendered HTML.
pub(crate) fn render_live_template(
    template_html: &str,
    questions: &[DemoQuestion],
    answers: &BTreeMap<String, String>,
) -> String {
    let context = answers
        .iter()
        .map(|(code, answer)| (code.clone(), answer.clone()))
        .collect();
    let display = questions
        .iter()
        .filter_map(|question| {
            answers.get(&question.code).map(|answer| {
                (
                    question.code.clone(),
                    escape_html(&answer_label(question, answer)),
                )
            })
        })
        .collect();
    replace_unanswered(
        forms::notation::fill_with_display(template_html, &context, &display),
        questions,
    )
}

/// Render one interactive demo step, or the explanation for a record state.
fn demo_field(question: &DemoQuestion, mut answers: Signal<BTreeMap<String, String>>) -> Element {
    if !question.interactive {
        return rsx! {
            p { class: "notation-demo__explanation", "{question.prompt}" }
        };
    }
    let code = question.code.clone();
    let current = answers.read().get(&code).cloned().unwrap_or_default();
    let input_type = match question.answer_type.as_str() {
        "custom_datetime" => "date",
        "custom_usd" => "number",
        "custom_phone" => "tel",
        _ => "text",
    };
    if matches!(
        question.answer_type.as_str(),
        "custom_yes_no" | "custom_single_choice"
    ) {
        let choices = if question.answer_type == "custom_yes_no" {
            vec![
                ("true".to_string(), "Yes".to_string()),
                ("false".to_string(), "No".to_string()),
            ]
        } else {
            question.choices.clone()
        };
        return rsx! {
            fieldset { class: "nav-field nav-choice-group",
                legend { "{question.prompt}" }
                for (value, label) in choices {
                    label { class: "nav-choice-card",
                        input {
                            class: "nav-control",
                            r#type: "radio",
                            name: "{question.code}",
                            value: "{value}",
                            checked: current == value,
                            onchange: {
                                let code = code.clone();
                                let value = value.clone();
                                move |_| { answers.write().insert(code.clone(), value.clone()); }
                            },
                        }
                        span { "{label}" }
                    }
                }
            }
        };
    }
    if question.answer_type == "custom_text" {
        return rsx! {
            label { class: "nav-field",
                span { "{question.prompt}" }
                textarea {
                    class: "nav-control",
                    rows: 4,
                    value: "{current}",
                    oninput: move |event| { answers.write().insert(code.clone(), event.value()); },
                }
            }
        };
    }
    rsx! {
        label { class: "nav-field",
            span { "{question.prompt}" }
            input {
                class: "nav-control",
                r#type: input_type,
                value: "{current}",
                step: if question.answer_type == "custom_usd" { "0.01" } else { "" },
                oninput: move |event| { answers.write().insert(code.clone(), event.value()); },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interactive(answer_type: &str, prompt: &str, choices: Vec<(&str, &str)>) -> DemoQuestion {
        DemoQuestion {
            code: format!("{answer_type}__demo"),
            answer_type: answer_type.to_string(),
            prompt: prompt.to_string(),
            choices: choices
                .into_iter()
                .map(|(v, l)| (v.to_string(), l.to_string()))
                .collect(),
            interactive: true,
        }
    }

    fn explanation(prompt: &str) -> DemoQuestion {
        DemoQuestion {
            code: "person__client".to_string(),
            answer_type: "person".to_string(),
            prompt: prompt.to_string(),
            choices: Vec::new(),
            interactive: false,
        }
    }

    fn render(questions: Vec<DemoQuestion>) -> String {
        let mut dom = VirtualDom::new_with_props(
            QuestionnaireDemo,
            QuestionnaireDemoProps {
                questions,
                template_html: String::new(),
            },
        );
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    #[test]
    fn a_notation_with_no_questionnaire_renders_nothing() {
        let out = render(Vec::new());
        assert!(!out.contains("Try answering this"), "{out}");
    }

    #[test]
    fn names_the_synthetic_sample_matter_and_that_nothing_is_saved() {
        let out = render(vec![interactive(
            "custom_text",
            "What's the scope?",
            vec![],
        )]);
        assert!(out.contains(SAMPLE_PROJECT_LABEL), "{out}");
        assert!(out.contains("nothing you type below is saved"), "{out}");
    }

    #[test]
    fn custom_text_renders_a_textarea() {
        let out = render(vec![interactive(
            "custom_text",
            "Describe the scope.",
            vec![],
        )]);
        assert!(out.contains("<textarea"), "{out}");
        assert!(out.contains("Describe the scope."), "{out}");
    }

    #[test]
    fn custom_single_choice_renders_the_templates_own_radio_options() {
        let out = render(vec![interactive(
            "custom_single_choice",
            "Which state's law governs?",
            vec![("nevada", "Nevada"), ("california", "California")],
        )]);
        assert!(out.contains(r#"type="radio""#), "{out}");
        assert!(
            out.contains("Nevada") && out.contains("California"),
            "{out}"
        );
        // ENG-506: cards, not a compact radio list — matching the real
        // walkers' own ENG-504 treatment.
        assert!(out.contains("nav-choice-group"), "{out}");
    }

    #[test]
    fn custom_yes_no_renders_a_two_card_yes_no_choice() {
        let out = render(vec![interactive(
            "custom_yes_no",
            "Do you have counsel?",
            vec![],
        )]);
        assert!(out.contains("nav-choice-group"), "{out}");
        assert!(out.contains(">Yes<") && out.contains(">No<"), "{out}");
    }

    #[test]
    fn the_step_list_names_every_question_with_the_current_one_marked() {
        let out = render(vec![
            interactive("custom_text", "First?", vec![]),
            interactive("custom_text", "Second?", vec![]),
        ]);
        assert!(out.contains("nav-stepper"), "{out}");
        assert_eq!(out.matches("nav-steps__item").count(), 2, "{out}");
        assert!(out.contains(r#"aria-current="step""#), "{out}");
        assert!(out.contains("Step 1 of 2"), "{out}");
    }

    #[test]
    fn a_record_state_renders_its_explanation_and_no_input_control() {
        let out = render(vec![explanation("This step records the client.")]);
        assert!(out.contains("This step records the client."), "{out}");
        assert!(
            !out.contains("<input") && !out.contains("<textarea") && !out.contains("<select"),
            "no fake control for a record state: {out}"
        );
    }

    #[test]
    fn the_progress_indicator_names_the_first_of_several_steps() {
        let out = render(vec![
            interactive("custom_text", "First?", vec![]),
            interactive("custom_text", "Second?", vec![]),
        ]);
        assert!(out.contains(r#"aria-valuenow="1""#), "{out}");
        assert!(out.contains(r#"aria-valuemax="2""#), "{out}");
    }

    #[test]
    fn a_single_step_notation_has_no_back_button_and_names_the_end() {
        let out = render(vec![interactive("custom_text", "Only question?", vec![])]);
        assert!(!out.contains(">Back<"), "{out}");
        assert!(
            out.contains("That is every question this notation asks."),
            "{out}"
        );
    }

    #[test]
    fn no_form_element_or_action_attribute_exists_anywhere_in_the_demo() {
        let out = render(vec![
            interactive("custom_text", "Free text?", vec![]),
            explanation("This step records the client."),
        ]);
        assert!(!out.contains("<form"), "no <form> in the demo: {out}");
        assert!(!out.contains("action="), "no action= in the demo: {out}");
    }

    #[test]
    fn live_template_hides_tokens_escapes_answers_and_evaluates_clauses() {
        let questions = vec![interactive("custom_text", "Client name", vec![])];
        let code = questions[0].code.clone();
        let token = format!("{{{{{code}}}}}");
        let template = ["Hello ", &token, ". {{#if ", &code, "}}Named.{{/if}}"].concat();
        let empty = render_live_template(&template, &questions, &BTreeMap::new());
        assert!(!empty.contains("{{"), "{empty}");
        assert!(!empty.contains("Named."), "{empty}");
        assert!(empty.contains("Client name"), "{empty}");
        let filled = render_live_template(
            &template,
            &questions,
            &BTreeMap::from([(code, "<Acme>".to_string())]),
        );
        assert!(filled.contains("&lt;Acme&gt;"), "{filled}");
        assert!(filled.contains("Named."), "{filled}");
    }
}
