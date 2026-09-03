//! The focus set — Stage, Hero, `ChoiceGroup`, `StepList`, Stepper (ENG-455).
//!
//! Ported from Navigator UX's `src/components/Focus.tsx`, so a page can be
//! built in either stack and read identically. Everything else in this theme
//! sits in a wide column beside other things; these five are for the page
//! that shows one thing — a sign-in, one intake question, a decision between
//! two options. shadcn's login and onboarding blocks are the reference: a
//! card no wider than it needs to be, centered in the whole viewport, with
//! more space around it than inside it. Spacing reads the `--nav-space-*`
//! scale (`server/public/css/tokens.css`), a step or two up from the rest of
//! the theme's — density right for a portal page read forty times a day is
//! wrong for a question put to someone who has never seen the page before.
//!
//! Per the leaf rule, [`Stepper`]'s Back/Continue/Finish are plain `href`s (an
//! `aria-disabled` span when there is nowhere to go), never a client-side
//! callback: the flow works before hydration, and the current step lives in
//! the URL rather than in component state.

use dioxus::prelude::*;

use super::icon::IconName;
use super::Icon;

/* ------------------------------------------------------------------ Stage -- */

/// How wide the one thing on a [`Stage`] may be: a card (`Sm`, 24rem), a form
/// (`Md`, 36rem, the default), or a reading column (`Lg`, 48rem).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum StageWidth {
    Sm,
    #[default]
    Md,
    Lg,
}

/// The viewport, given to one thing.
///
/// Three rows — header, the thing, footer — with the thing centered in
/// whatever is left, so a short card floats and a long form starts near the
/// top and scrolls. `fill` takes the whole viewport (`100svh`) by default;
/// turn it off when the stage sits inside chrome that already gives it a
/// height.
#[component]
pub fn Stage(
    children: Element,
    #[props(default)] width: StageWidth,
    #[props(default)] header: Option<Element>,
    #[props(default)] footer: Option<Element>,
    #[props(default = true)] fill: bool,
    #[props(default)] class: Option<String>,
) -> Element {
    let width_class = match width {
        StageWidth::Sm => Some("nav-stage--sm"),
        StageWidth::Md => None,
        StageWidth::Lg => Some("nav-stage--lg"),
    };
    let classes = [
        "nav-stage",
        width_class.unwrap_or_default(),
        if fill { "nav-stage--fill" } else { "" },
        class.as_deref().unwrap_or_default(),
    ]
    .into_iter()
    .filter(|c| !c.is_empty())
    .collect::<Vec<_>>()
    .join(" ");
    rsx! {
        div { class: "{classes}",
            if let Some(header) = header {
                div { class: "nav-stage__header", {header} }
            }
            div { class: "nav-stage__body", {children} }
            if let Some(footer) = footer {
                div { class: "nav-stage__footer", {footer} }
            }
        }
    }
}

/* ------------------------------------------------------------------- Hero -- */

/// `H1` on a page with no other h1; `H2` inside a stepper or a section.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum HeroLevel {
    #[default]
    H1,
    H2,
}

/// Centered, because the [`Stage`] is; `Start` for a hero at the top of a
/// column.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum HeroAlign {
    #[default]
    Center,
    Start,
}

/// The big type block: eyebrow, title, lede, actions.
///
/// `PageHeader` and `CaseHead` are for a page with a lot on it and put the
/// title at the top left at a fixed size. This is for a page with one thing on
/// it and puts the title in the middle at up to `3.5rem` — the size the serif
/// was drawn to be read at.
#[component]
pub fn Hero(
    #[props(default)] eyebrow: Option<Element>,
    title: Element,
    #[props(default)] lede: Option<Element>,
    #[props(default)] actions: Option<Element>,
    #[props(default)] align: HeroAlign,
    #[props(default)] level: HeroLevel,
    #[props(default)] id: Option<String>,
) -> Element {
    let class = match align {
        HeroAlign::Center => "nav-hero",
        HeroAlign::Start => "nav-hero nav-hero--start",
    };
    rsx! {
        div { class: "{class}",
            if let Some(eyebrow) = eyebrow {
                p { class: "nav-hero__eyebrow", {eyebrow} }
            }
            if matches!(level, HeroLevel::H1) {
                h1 { class: "nav-hero__title", id: id.clone(), {title} }
            } else {
                h2 { class: "nav-hero__title", id: id.clone(), {title} }
            }
            if let Some(lede) = lede {
                p { class: "nav-hero__lede", {lede} }
            }
            if let Some(actions) = actions {
                div { class: "nav-hero__actions", {actions} }
            }
        }
    }
}

/* ------------------------------------------------------------ ChoiceGroup -- */

/// One card in a [`ChoiceGroup`].
#[derive(Clone, PartialEq)]
pub struct ChoiceGroupOption {
    pub value: String,
    pub label: String,
    /// The second line — who this is for, what it costs, what happens next.
    pub description: Option<String>,
    /// A leading glyph. Decorative; the label carries the meaning.
    pub icon: Option<IconName>,
    pub disabled: bool,
}

impl ChoiceGroupOption {
    #[must_use]
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description: None,
            icon: None,
            disabled: false,
        }
    }

    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    #[must_use]
    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    #[must_use]
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// A question with its answers as cards.
///
/// `Field::radio`/`Field::select` are the compact form controls: a circle and
/// a word per line, for a field among fields. This is the same `<fieldset>`
/// and the same native inputs — it posts without JavaScript and autofills —
/// with each option grown to a card the reader can hit with a thumb and given
/// room for a second line. `selected` is what the server re-renders as
/// `is-checked`; there is no client-side toggle to wire up.
#[component]
pub fn ChoiceGroup(
    legend: Element,
    #[props(default)] legend_hidden: bool,
    name: String,
    options: Vec<ChoiceGroupOption>,
    #[props(default)] selected: Vec<String>,
    /// Several may be chosen: checkboxes rather than radios.
    #[props(default)]
    multiple: bool,
    /// Cards per row. `1` by default — a stack is read top to bottom, the
    /// order the reader weighs the options in. Collapses to one on a narrow
    /// viewport regardless.
    #[props(default = 1)]
    columns: u8,
    #[props(default)] help: Option<String>,
    #[props(default)] error: Option<String>,
    #[props(default)] required: bool,
) -> Element {
    let help_id = format!("{name}-help");
    let error_id = format!("{name}-error");
    let described_by = match (error.is_some(), help.is_some()) {
        (true, true) => Some(format!("{error_id} {help_id}")),
        (true, false) => Some(error_id.clone()),
        (false, true) => Some(help_id.clone()),
        (false, false) => None,
    };
    // Computed before the `rsx!` tree rather than inline: `error` is later
    // moved into the error paragraph below, and `rsx!` desugars each `if let`
    // block into its own closure, so an inline `error.is_some()` at the
    // fieldset's attributes can be ordered by the macro after that move.
    let has_error = error.is_some();
    let group_class = [
        "nav-choice-group".to_string(),
        if columns > 1 {
            format!("nav-choice-group--cols-{columns}")
        } else {
            String::new()
        },
        if has_error {
            "nav-field--invalid".to_string()
        } else {
            String::new()
        },
    ]
    .into_iter()
    .filter(|c| !c.is_empty())
    .collect::<Vec<_>>()
    .join(" ");
    let legend_class = if legend_hidden {
        "nav-choice-group__legend nav-visually-hidden"
    } else {
        "nav-choice-group__legend"
    };

    rsx! {
        // No `aria-required` here: WAI-ARIA doesn't allow it on the fieldset's
        // implicit `group` role (axe's `aria-allowed-attr`, WCAG A). Each radio
        // already carries the native `required` below, which is how the
        // browser and assistive tech learn "one of these" for a single-select
        // group.
        fieldset {
            class: "{group_class}",
            "aria-describedby": described_by,
            "aria-invalid": has_error.then_some("true"),
            legend { class: "{legend_class}",
                {legend}
                if required {
                    span { class: "nav-required", "aria-hidden": "true", " *" }
                }
            }
            div { class: "nav-choice-group__options",
                for option in options {
                    {
                        let input_id = format!("{name}-{}", option.value);
                        let checked = selected.contains(&option.value);
                        let card_class = [
                            "nav-choice".to_string(),
                            if checked {
                                "is-checked".to_string()
                            } else {
                                String::new()
                            },
                            if option.disabled {
                                "is-disabled".to_string()
                            } else {
                                String::new()
                            },
                        ]
                        .into_iter()
                        .filter(|c| !c.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ");
                        rsx! {
                            label { key: "{option.value}", class: "{card_class}", r#for: "{input_id}",
                                input {
                                    class: "nav-choice__input",
                                    r#type: if multiple { "checkbox" } else { "radio" },
                                    id: "{input_id}",
                                    name: "{name}",
                                    value: "{option.value}",
                                    checked,
                                    disabled: option.disabled,
                                    // `required` on every radio in a group is how HTML
                                    // says "one of these"; on checkboxes it would mean
                                    // "all of these".
                                    required: required && !multiple,
                                }
                                if let Some(icon) = option.icon {
                                    span { class: "nav-choice__icon", "aria-hidden": "true", Icon { name: icon } }
                                }
                                span { class: "nav-choice__text",
                                    span { class: "nav-choice__label", "{option.label}" }
                                    if let Some(description) = option.description {
                                        span { class: "nav-choice__description", "{description}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if let Some(help) = help {
                p { class: "nav-field__help", id: "{help_id}", "{help}" }
            }
            if let Some(error) = error {
                p { class: "nav-field__error", id: "{error_id}", role: "alert", "{error}" }
            }
        }
    }
}

/* --------------------------------------------------------------- StepList -- */

/// One entry in a [`StepList`] or [`Stepper`] progress rail.
#[derive(Clone, PartialEq)]
pub struct StepMeta {
    /// Stable identifier, also the [`Stepper`] panel's `key`.
    pub id: String,
    pub title: String,
}

impl StepMeta {
    #[must_use]
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
        }
    }
}

fn step_state(index: usize, current: usize) -> &'static str {
    match index.cmp(&current) {
        std::cmp::Ordering::Less => "done",
        std::cmp::Ordering::Equal => "current",
        std::cmp::Ordering::Greater => "upcoming",
    }
}

/// Where the reader is, out of how many.
///
/// An `<ol>` with `aria-current="step"`, the one ARIA value that means exactly
/// this. A completed step is a plain anchor when `step_hrefs` gives it
/// somewhere to go — a real link rather than a client `onClick`, per the leaf
/// rule — and a `<span>` otherwise, so a keyboard user tabbing through lands
/// only on what does something.
#[component]
pub fn StepList(
    steps: Vec<StepMeta>,
    /// Zero-based.
    current: usize,
    /// Names the list — "Intake progress".
    label: String,
    /// The href a completed step's marker should revisit, keyed by index
    /// (`step_hrefs[i]`). A missing entry, or `None`, renders a plain
    /// `<span>`. Plain data rather than a callback: a function pointer would
    /// make the derived `Props` comparison compare code addresses, which is
    /// meaningless and rustc warns on.
    #[props(default)]
    step_hrefs: Vec<Option<String>>,
) -> Element {
    rsx! {
        ol { class: "nav-steps", "aria-label": "{label}",
            for (index , step) in steps.iter().enumerate() {
                {
                    let state = step_state(index, current);
                    // Only a completed step may be revisited; a current or
                    // upcoming index never gets a link even if `step_hrefs`
                    // holds one for it — the stepper's Continue is the only
                    // way forward, because that is where the current step
                    // gets checked.
                    let href = (state == "done")
                        .then(|| step_hrefs.get(index).cloned().flatten())
                        .flatten();
                    let marker_text = (state != "done").then(|| (index + 1).to_string());
                    rsx! {
                        li {
                            key: "{step.id}",
                            class: "nav-steps__item is-{state}",
                            "aria-current": (state == "current").then_some("step"),
                            if let Some(href) = href {
                                a { class: "nav-steps__step", href: "{href}",
                                    span { class: "nav-steps__marker", "aria-hidden": "true",
                                        if let Some(marker_text) = marker_text.clone() { "{marker_text}" }
                                    }
                                    span { class: "nav-steps__label", "{step.title}" }
                                    if state == "done" {
                                        span { class: "nav-visually-hidden", ", completed" }
                                    }
                                }
                            } else {
                                span { class: "nav-steps__step",
                                    span { class: "nav-steps__marker", "aria-hidden": "true",
                                        if let Some(marker_text) = marker_text { "{marker_text}" }
                                    }
                                    span { class: "nav-steps__label", "{step.title}" }
                                    if state == "done" {
                                        span { class: "nav-visually-hidden", ", completed" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/* ---------------------------------------------------------------- Stepper -- */

/// One step at a time.
///
/// Every panel renders and every panel but the current one carries `hidden`,
/// so the whole flow is one `<form>` and posts as one. This is a server-only
/// component: the current step is a prop, driven by the caller from the URL
/// (`?step=2`), and Back/Continue/Finish are real `href`s rather than
/// client-side state — the shape a page with no hydration bundle needs.
#[component]
pub fn Stepper(
    steps: Vec<StepMeta>,
    /// The panels, one `nav-stepper__panel` [`StepperPanel`] per entry in
    /// `steps`, in the same order. Every panel renders; [`Stepper`] does not
    /// unmount the others, so the reader's answers on a step already visited
    /// survive a Back.
    children: Element,
    /// Zero-based.
    current: usize,
    /// Names the progress list — "Intake progress".
    label: String,
    /// Drop the progress list. The "Step n of N" line above each title stays.
    #[props(default)]
    hide_steps: bool,
    /// Forwarded to [`StepList`]'s `step_hrefs`.
    #[props(default)]
    step_hrefs: Vec<Option<String>>,
    /// Where Back points. Absent on the first step.
    #[props(default)]
    back_href: Option<String>,
    /// Where Continue points, when this is not the last step.
    #[props(default)]
    continue_href: Option<String>,
    /// Where Finish points, on the last step.
    #[props(default)]
    finish_href: Option<String>,
    #[props(default = "Back".to_string())] back_label: String,
    #[props(default = "Continue".to_string())] continue_label: String,
    #[props(default = "Finish".to_string())] finish_label: String,
) -> Element {
    let last = steps.len().saturating_sub(1);
    let is_last = current >= last;
    let primary_href = if is_last { finish_href } else { continue_href };
    let primary_label = if is_last {
        finish_label
    } else {
        continue_label
    };

    rsx! {
        div { class: "nav-stepper",
            if !hide_steps {
                StepList { steps: steps.clone(), current, label, step_hrefs }
            }
            {children}
            div { class: "nav-stepper__actions",
                if let Some(back_href) = back_href {
                    a { class: "nav-btn nav-btn--secondary nav-btn--lg", href: "{back_href}", "{back_label}" }
                }
                if let Some(primary_href) = primary_href {
                    a { class: "nav-btn nav-btn--primary nav-btn--lg", href: "{primary_href}", "{primary_label}" }
                } else {
                    span { class: "nav-btn nav-btn--primary nav-btn--lg", "aria-disabled": "true", "{primary_label}" }
                }
            }
        }
    }
}

/// One panel of a [`Stepper`]. `hidden` when `index != current`, so an
/// inactive step's markup stays in the document — and its form values survive
/// a Back — but paints nothing and reaches no assistive technology.
#[component]
pub fn StepperPanel(
    /// This panel's position among the stepper's steps (zero-based).
    index: usize,
    current: usize,
    title: Element,
    #[props(default)] description: Option<Element>,
    children: Element,
) -> Element {
    let total_hint = index + 1;
    rsx! {
        section {
            class: "nav-stepper__panel",
            hidden: index != current,
            "aria-labelledby": "nav-stepper-panel-{index}-title",
            p { class: "nav-stepper__count", "Step {total_hint}" }
            h2 { class: "nav-stepper__title", id: "nav-stepper-panel-{index}-title", tabindex: "-1", {title} }
            if let Some(description) = description {
                p { class: "nav-stepper__description", {description} }
            }
            div { class: "nav-stepper__body", {children} }
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

    #[test]
    fn stage_renders_header_body_and_footer_in_grid_rows() {
        fn app() -> Element {
            rsx! {
                Stage {
                    header: rsx! { "brand" },
                    footer: rsx! { "fine print" },
                    "the one thing"
                }
            }
        }
        let out = ssr(app);
        assert!(out.contains("nav-stage nav-stage--fill"), "{out}");
        assert!(out.contains("nav-stage__header"), "{out}");
        assert!(out.contains("nav-stage__body"), "{out}");
        assert!(out.contains("nav-stage__footer"), "{out}");
        assert!(out.contains("the one thing"), "{out}");
    }

    #[test]
    fn stage_width_modifiers_are_sm_and_lg_only() {
        fn sm() -> Element {
            rsx! {
                Stage { width: StageWidth::Sm, "x" }
            }
        }
        fn md() -> Element {
            rsx! {
                Stage { width: StageWidth::Md, "x" }
            }
        }
        fn lg() -> Element {
            rsx! {
                Stage { width: StageWidth::Lg, "x" }
            }
        }
        assert!(ssr(sm).contains("nav-stage--sm"));
        assert!(!ssr(md).contains("nav-stage--sm") && !ssr(md).contains("nav-stage--lg"));
        assert!(ssr(lg).contains("nav-stage--lg"));
    }

    #[test]
    fn hero_renders_one_h1_by_default_and_an_h2_at_level_two() {
        fn h1() -> Element {
            rsx! {
                Hero { title: rsx! { "Welcome" } }
            }
        }
        fn h2() -> Element {
            rsx! {
                Hero { title: rsx! { "Welcome" }, level: HeroLevel::H2 }
            }
        }
        let out = ssr(h1);
        assert_eq!(out.matches("<h1").count(), 1, "{out}");
        assert!(ssr(h2).contains("<h2"), "{}", ssr(h2));
    }

    #[test]
    fn choice_group_marks_the_selected_option_checked() {
        fn app() -> Element {
            rsx! {
                ChoiceGroup {
                    legend: rsx! { "Practice area" },
                    name: "area".to_string(),
                    options: vec![
                        ChoiceGroupOption::new("estate", "Estate planning"),
                        ChoiceGroupOption::new("litigation", "Litigation"),
                    ],
                    selected: vec!["litigation".to_string()],
                }
            }
        }
        let out = ssr(app);
        assert!(out.contains(r#"value="litigation" checked"#), "{out}");
        assert!(!out.contains(r#"value="estate" checked"#), "{out}");
        assert!(out.contains("is-checked"), "{out}");
        assert_eq!(out.matches(r#"type="radio""#).count(), 2, "{out}");
    }

    #[test]
    fn choice_group_multiple_renders_checkboxes_without_required() {
        fn app() -> Element {
            rsx! {
                ChoiceGroup {
                    legend: rsx! { "Add-ons" },
                    name: "addons".to_string(),
                    multiple: true,
                    required: true,
                    options: vec![ChoiceGroupOption::new("rush", "Rush filing")],
                }
            }
        }
        let out = ssr(app);
        // The legend still carries the required cue — `required` describes the
        // group's answer, not each checkbox — but the input itself carries no
        // `required` attribute: exact-matched here so a stray `required`
        // anywhere in the tag would fail the match rather than slip past a
        // substring check that "nav-required"/"aria-required" also satisfy.
        assert!(
            out.contains(
                r#"<input class="nav-choice__input" type="checkbox" id="addons-rush" name="addons" value="rush"/>"#
            ),
            "the checkbox carries no required attribute: {out}"
        );
    }

    #[test]
    fn step_list_marks_the_current_step_and_hides_future_numbers_correctly() {
        fn app() -> Element {
            rsx! {
                StepList {
                    steps: vec![
                        StepMeta::new("intake", "Intake"),
                        StepMeta::new("review", "Review"),
                        StepMeta::new("sign", "Sign"),
                    ],
                    current: 1,
                    label: "Intake progress".to_string(),
                }
            }
        }
        let out = ssr(app);
        assert!(out.contains("is-done"), "{out}");
        assert!(out.contains(r#"aria-current="step""#), "{out}");
        assert!(out.contains("is-upcoming"), "{out}");
        assert!(out.contains(", completed"), "{out}");
    }

    #[test]
    fn step_list_revisitable_step_renders_an_anchor_not_a_button() {
        fn app() -> Element {
            rsx! {
                StepList {
                    steps: vec![StepMeta::new("intake", "Intake"), StepMeta::new("review", "Review")],
                    current: 1,
                    label: "Intake progress".to_string(),
                    step_hrefs: vec![Some("/design?step=0".to_string())],
                }
            }
        }
        let out = ssr(app);
        assert!(out.contains(r#"href="/design?step=0""#), "{out}");
        assert!(!out.contains("<button"), "{out}");
    }

    #[test]
    fn step_list_never_links_a_current_or_upcoming_step_even_if_step_hrefs_would() {
        fn app() -> Element {
            rsx! {
                StepList {
                    steps: vec![StepMeta::new("intake", "Intake"), StepMeta::new("review", "Review")],
                    current: 0,
                    label: "Intake progress".to_string(),
                    // A misbehaving caller supplying an href for every index
                    // must not make the current or an upcoming step clickable.
                    step_hrefs: vec![
                        Some("/design?step=0".to_string()),
                        Some("/design?step=1".to_string()),
                    ],
                }
            }
        }
        let out = ssr(app);
        assert!(!out.contains("<a"), "no step should be a link here: {out}");
    }

    #[test]
    fn stepper_renders_only_the_current_panel_visibly() {
        fn app() -> Element {
            rsx! {
                Stepper {
                    steps: vec![StepMeta::new("one", "One"), StepMeta::new("two", "Two")],
                    current: 0,
                    label: "Demo".to_string(),
                    continue_href: Some("/design?step=1".to_string()),
                    StepperPanel { index: 0, current: 0, title: rsx! { "One" }, "first body" }
                    StepperPanel { index: 1, current: 0, title: rsx! { "Two" }, "second body" }
                }
            }
        }
        let out = ssr(app);
        assert!(out.contains("first body"), "{out}");
        assert!(out.contains("second body"), "{out}");
        // Scoped to the panel's own `hidden` attribute — a bare `"hidden"`
        // substring also matches each step marker's `aria-hidden="true"`.
        assert_eq!(
            out.matches(r#"nav-stepper__panel" hidden"#).count(),
            1,
            "exactly one hidden panel: {out}"
        );
        assert!(
            !out.contains("nav-btn--secondary"),
            "no Back on the first step: {out}"
        );
        assert!(out.contains(r#"href="/design?step=1""#), "{out}");
    }

    #[test]
    fn stepper_last_step_renders_finish_and_disables_it_with_no_href() {
        fn app() -> Element {
            rsx! {
                Stepper {
                    steps: vec![StepMeta::new("one", "One")],
                    current: 0,
                    label: "Demo".to_string(),
                    finish_href: None,
                    StepperPanel { index: 0, current: 0, title: rsx! { "One" }, "body" }
                }
            }
        }
        let out = ssr(app);
        assert!(out.contains("Finish"), "{out}");
        assert!(out.contains(r#"aria-disabled="true""#), "{out}");
    }
}
