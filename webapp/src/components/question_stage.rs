//! `QuestionStage` — the shared chrome for one questionnaire question,
//! rendered on Navigator's focus set (ENG-503).
//!
//! Both real walkers — the lawyer's `webapp::walker_step` and the client's
//! `webapp::client_intake` — ask one question per request over a plain
//! `<form method=post>`, so neither can adopt [`Stepper`]'s href-driven,
//! all-panels-rendered design: there is exactly one panel to show, and
//! advancing it is a server round trip, not a client-side reveal. This
//! composes the same pieces `Stepper` does — [`Stage`], [`StepList`], the
//! `nav-stepper__*` class family — around a single real `<form>` instead.
//!
//! Because there is no revisit route on either walker today, [`StepList`]'s
//! completed markers render as plain spans, never links (`step_hrefs` is
//! never supplied).

use dioxus::prelude::*;

use super::focus::{Hero, HeroAlign, HeroLevel, Stage, StageWidth, StepList, StepMeta};
use super::form::Field;
use super::progress::Progress;

/// One question, staged: eyebrow (the flow's name), the prompt as the
/// title, help text as the lede, the full chain as a `StepList`, and the
/// question's own controls inside one native form.
#[component]
pub fn QuestionStage(
    /// Names the flow — "Retainer Agreement", "Closing Letter".
    eyebrow: String,
    prompt: String,
    #[props(default)] help_text: Option<String>,
    /// The full questionnaire chain, in order.
    steps: Vec<StepMeta>,
    /// 1-based, matching `total`.
    position: usize,
    total: usize,
    action: String,
    csrf_token: String,
    #[props(default = "Continue".to_string())] submit_label: String,
    fields: Vec<Field>,
    /// A composite widget's extra inputs (e.g. `people_list`'s rows),
    /// appended after `fields` inside the same form.
    #[props(default)]
    extra_fields: Option<Element>,
    /// Muted introductory prose before the controls.
    #[props(default)]
    intro: Option<Element>,
    /// The stage's secondary links and hand-off forms (save-and-exit,
    /// send-intake, custom clauses) — rendered in the `Stage` footer.
    #[props(default)]
    footer: Option<Element>,
) -> Element {
    let current = position.saturating_sub(1);
    let steps_label = "Intake progress".to_string();

    rsx! {
        Stage {
            width: StageWidth::Md,
            header: rsx! {
                Hero {
                    eyebrow: rsx! { "{eyebrow}" },
                    title: rsx! { "{prompt}" },
                    lede: help_text.clone().map(|text| rsx! { "{text}" }),
                    align: HeroAlign::Start,
                    level: HeroLevel::H2,
                }
            },
            footer,
            div { class: "nav-stepper",
                StepList { steps, current, label: steps_label.clone() }
                p { class: "nav-stepper__count", "Step {position} of {total}" }
                Progress { label: steps_label, value: Some(position), max: total }
                div { class: "nav-stepper__body",
                    form {
                        // `admin-form` carries no styling — it is the stable
                        // hook the browser accessibility e2e scopes axe to.
                        class: "nav-form admin-form",
                        action: "{action}",
                        method: "post",
                        "aria-label": "{prompt}",
                        input { r#type: "hidden", name: "_csrf", value: "{csrf_token}" }
                        if let Some(intro) = intro {
                            {intro}
                        }
                        for field in fields.iter() {
                            {field.render()}
                        }
                        if let Some(extra) = extra_fields {
                            {extra}
                        }
                        button { class: "nav-btn nav-btn--primary nav-btn--lg", r#type: "submit", "{submit_label}" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssr(app: fn() -> Element) -> String {
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    fn steps() -> Vec<StepMeta> {
        vec![
            StepMeta::new("entity", "Entity"),
            StepMeta::new("address__principal_office", "Principal office"),
            StepMeta::new("person__client", "Person client"),
        ]
    }

    #[test]
    fn renders_the_stage_stepper_and_form_with_the_csrf_token() {
        fn app() -> Element {
            rsx! {
                QuestionStage {
                    eyebrow: "Retainer Agreement".to_string(),
                    prompt: "What is the entity's name?".to_string(),
                    steps: steps(),
                    position: 1,
                    total: 3,
                    action: "/app/lawyer/notations/abc/step".to_string(),
                    csrf_token: "TOK".to_string(),
                    fields: vec![Field::text("Name", "value", "")],
                }
            }
        }
        let out = ssr(app);
        assert!(out.contains("nav-stage"), "{out}");
        assert!(out.contains("nav-stepper"), "{out}");
        assert!(out.contains(r#"class="nav-form admin-form""#), "{out}");
        assert!(out.contains(r#"name="_csrf" value="TOK""#), "{out}");
        assert!(out.contains("What is the entity"), "{out}");
        assert!(out.contains("name?"), "{out}");
        assert!(out.contains("Step 1 of 3"), "{out}");
        assert_eq!(out.matches("nav-steps__item").count(), 3, "{out}");
        assert!(out.contains(r#"aria-current="step""#), "{out}");
    }

    #[test]
    fn help_text_renders_as_the_hero_lede_and_is_absent_when_none() {
        fn with_help() -> Element {
            rsx! {
                QuestionStage {
                    eyebrow: "Retainer Agreement".to_string(),
                    prompt: "What is the entity's name?".to_string(),
                    help_text: Some("Use the legal name on file.".to_string()),
                    steps: steps(),
                    position: 1,
                    total: 3,
                    action: "/x".to_string(),
                    csrf_token: "TOK".to_string(),
                    fields: vec![Field::text("Name", "value", "")],
                }
            }
        }
        fn without_help() -> Element {
            rsx! {
                QuestionStage {
                    eyebrow: "Retainer Agreement".to_string(),
                    prompt: "What is the entity's name?".to_string(),
                    steps: steps(),
                    position: 1,
                    total: 3,
                    action: "/x".to_string(),
                    csrf_token: "TOK".to_string(),
                    fields: vec![Field::text("Name", "value", "")],
                }
            }
        }
        let with = ssr(with_help);
        assert!(with.contains("nav-hero__lede"), "{with}");
        assert!(with.contains("Use the legal name on file."), "{with}");
        let without = ssr(without_help);
        assert!(!without.contains("nav-hero__lede"), "{without}");
    }

    #[test]
    fn footer_and_extra_fields_render_in_their_own_slots() {
        fn app() -> Element {
            rsx! {
                QuestionStage {
                    eyebrow: "Retainer Agreement".to_string(),
                    prompt: "What is the entity's name?".to_string(),
                    steps: steps(),
                    position: 1,
                    total: 3,
                    action: "/x".to_string(),
                    csrf_token: "TOK".to_string(),
                    fields: vec![Field::text("Name", "value", "")],
                    extra_fields: rsx! { p { "extra row" } },
                    footer: rsx! { a { href: "/app/lawyer", "Save and exit" } },
                }
            }
        }
        let out = ssr(app);
        assert!(out.contains("extra row"), "{out}");
        assert!(out.contains("Save and exit"), "{out}");
        assert!(out.contains("nav-stage__footer"), "{out}");
    }
}
