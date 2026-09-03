//! Create / edit form, as Dioxus components (issue #641, Phase 2).
//!
//! The successor to the `views::components::form`. A [`FormCard`] renders a
//! constrained card with a heading, a stack of [`Field`]s, and a submit button.
//! It is a **native** form — it submits over a classic navigation to `action`
//! with `method`, so it works before hydration. (The builder also has an
//! HTMX-submit mode; that interactive path converts to a Dioxus server function
//! when the admin cluster migrates in Phase 3, and is intentionally out of scope
//! here.) Every field carries a label, the accessible `required` state with a
//! visible `*` cue, optional help text wired through `aria-describedby`, and the
//! theme's form styling — no Bootstrap.

use dioxus::prelude::*;

use super::{PersonChoice, PersonPicker};

/// Escape a string for safe inclusion as `<textarea>` RCDATA content: `&`
/// becomes `&amp;` and `<` becomes `&lt;`, so the value can never introduce an
/// entity ambiguity or a `</textarea>` that closes the element early.
fn escape_rcdata(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;")
}

/// One option in a [`FieldKind::Select`].
#[derive(Clone, PartialEq, Eq)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

impl Choice {
    #[must_use]
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
        }
    }
}

/// The control a [`Field`] renders.
#[derive(Clone, PartialEq, Eq)]
pub enum FieldKind {
    /// A single-line `<input>` of the given `input_type` (`text`, `email`,
    /// `number`, `file`, …), with optional placeholder / numeric step / prefix
    /// add-on / datalist suggestions.
    Input {
        input_type: String,
        value: String,
        placeholder: Option<String>,
        prefix: Option<String>,
        step: Option<String>,
        disabled: bool,
        multiple: bool,
        suggestions: Option<Vec<String>>,
    },
    Textarea {
        value: String,
        rows: u8,
    },
    Select {
        options: Vec<Choice>,
        selected: Option<String>,
        disabled: bool,
    },
    /// A searchable selector whose submitted value is a Person foreign key.
    PersonPicker {
        blank_label: String,
        people: Vec<PersonChoice>,
        selected: Option<String>,
        search: Option<String>,
        disabled: bool,
    },
    Checkbox {
        value: String,
        checked: bool,
    },
    /// A radio group: one `<fieldset>`/`<legend>` over the mutually exclusive
    /// choices. `locked` marks the choices already spoken for — rendered greyed
    /// and `disabled`, so taking one is a deliberate act rather than a stray
    /// click. A locked choice does not submit; the door that owns the invariant
    /// still refuses it if it arrives.
    Radio {
        options: Vec<Choice>,
        selected: Option<String>,
        locked: Vec<String>,
    },
}

/// A labeled form control. Build with a constructor ([`Field::text`],
/// [`Field::select`], …) and the chaining setters, then hand the `Vec<Field>` to
/// a [`FormCard`].
#[derive(Clone, PartialEq, Eq)]
pub struct Field {
    label: String,
    name: String,
    kind: FieldKind,
    required: bool,
    help: Option<String>,
    /// A validation message for this control (see [`Field::error`]).
    error: Option<String>,
    /// The control's DOM id, when it must differ from `name` (see
    /// [`Field::id`]).
    control_id: Option<String>,
}

impl Field {
    fn new(label: impl Into<String>, name: impl Into<String>, kind: FieldKind) -> Self {
        Self {
            label: label.into(),
            name: name.into(),
            kind,
            required: false,
            help: None,
            error: None,
            control_id: None,
        }
    }

    /// A single-line input of `input_type`.
    #[must_use]
    pub fn input(
        label: impl Into<String>,
        name: impl Into<String>,
        value: impl Into<String>,
        input_type: impl Into<String>,
    ) -> Self {
        Self::new(
            label,
            name,
            FieldKind::Input {
                input_type: input_type.into(),
                value: value.into(),
                placeholder: None,
                prefix: None,
                step: None,
                disabled: false,
                multiple: false,
                suggestions: None,
            },
        )
    }

    /// A `text` input.
    #[must_use]
    pub fn text(
        label: impl Into<String>,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        Self::input(label, name, value, "text")
    }

    /// An `email` input.
    #[must_use]
    pub fn email(
        label: impl Into<String>,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        Self::input(label, name, value, "email")
    }

    /// A multi-line textarea of `rows` rows.
    #[must_use]
    pub fn textarea(
        label: impl Into<String>,
        name: impl Into<String>,
        value: impl Into<String>,
        rows: u8,
    ) -> Self {
        Self::new(
            label,
            name,
            FieldKind::Textarea {
                value: value.into(),
                rows,
            },
        )
    }

    /// A `<select>` of `options`, with `selected` pre-selected.
    #[must_use]
    pub fn select(
        label: impl Into<String>,
        name: impl Into<String>,
        options: Vec<Choice>,
        selected: Option<String>,
    ) -> Self {
        Self::new(
            label,
            name,
            FieldKind::Select {
                options,
                selected,
                disabled: false,
            },
        )
    }

    /// A searchable picker for a person foreign key. The form posts the
    /// selected person's id under `name`; the displayed name and email remain
    /// presentation context only.
    #[must_use]
    pub fn person_picker(
        label: impl Into<String>,
        name: impl Into<String>,
        blank_label: impl Into<String>,
        people: Vec<PersonChoice>,
        selected: Option<String>,
    ) -> Self {
        Self::new(
            label,
            name,
            FieldKind::PersonPicker {
                blank_label: blank_label.into(),
                people,
                selected,
                search: None,
                disabled: false,
            },
        )
    }

    /// A `number` input.
    #[must_use]
    pub fn number(
        label: impl Into<String>,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        Self::input(label, name, value, "number")
    }

    /// Preserve the non-authoritative filter text for a person picker after a
    /// GET round trip. The selected person id remains the only submitted
    /// foreign-key value.
    #[must_use]
    pub fn person_search(mut self, search: Option<String>) -> Self {
        if let FieldKind::PersonPicker { search: value, .. } = &mut self.kind {
            *value = search;
        }
        self
    }

    /// The country picker a `country` question renders: a required `value`
    /// select over the seeded jurisdiction names, led by an empty prompt so a
    /// blank submit is caught by the browser rather than silently storing the
    /// first country. The chosen **name** is what posts, so the stored answer
    /// matches a jurisdictions row.
    #[must_use]
    pub fn country_select(
        label: impl Into<String>,
        country_names: &[String],
        prior: Option<&str>,
    ) -> Self {
        let mut options = vec![Choice::new("", "Select a country…")];
        options.extend(country_names.iter().map(|name| Choice::new(name, name)));
        Self::select(
            label,
            "value",
            options,
            prior.filter(|v| !v.is_empty()).map(str::to_string),
        )
        .required()
    }

    /// A file input. Pair with [`FormCard::multipart`] so the form submits as
    /// `multipart/form-data`; add [`Field::multiple`] to accept several files.
    #[must_use]
    pub fn file(label: impl Into<String>, name: impl Into<String>) -> Self {
        Self::input(label, name, "", "file")
    }

    /// A radio group over `options`, with `selected` pre-chosen.
    ///
    /// Every choice needs a non-empty value: it becomes part of each input's DOM
    /// id, which is what `<label for>` targets.
    #[must_use]
    pub fn radio(
        label: impl Into<String>,
        name: impl Into<String>,
        options: Vec<Choice>,
        selected: Option<String>,
    ) -> Self {
        Self::new(
            label,
            name,
            FieldKind::Radio {
                options,
                selected,
                locked: Vec::new(),
            },
        )
    }

    /// Grey out and `disable` the named radio choices — the ones already spoken
    /// for, which cannot be taken without the deliberate confirming step the
    /// page renders beside them (no-op on other kinds).
    #[must_use]
    pub fn locked(mut self, values: Vec<String>) -> Self {
        if let FieldKind::Radio { locked, .. } = &mut self.kind {
            *locked = values;
        }
        self
    }

    /// A checkbox.
    #[must_use]
    pub fn checkbox(
        label: impl Into<String>,
        name: impl Into<String>,
        value: impl Into<String>,
        checked: bool,
    ) -> Self {
        Self::new(
            label,
            name,
            FieldKind::Checkbox {
                value: value.into(),
                checked,
            },
        )
    }

    /// Mark the field required — sets the native `required` attribute and shows
    /// the visible `*` cue.
    #[must_use]
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    /// Disable the control — sets the native `disabled` attribute (no-op on the
    /// textarea/checkbox kinds). A disabled input/select is shown but not
    /// submitted, so it reads as a locked, look-but-don't-touch field: the
    /// read-only legal-name parts on the person edit form, and a role select
    /// whose submitted value the command layer would drop.
    #[must_use]
    pub fn disabled(mut self) -> Self {
        match &mut self.kind {
            FieldKind::Input { disabled, .. }
            | FieldKind::Select { disabled, .. }
            | FieldKind::PersonPicker { disabled, .. } => {
                *disabled = true;
            }
            FieldKind::Textarea { .. } | FieldKind::Checkbox { .. } | FieldKind::Radio { .. } => {}
        }
        self
    }

    /// Accept several files at once — sets the native `multiple` attribute on a
    /// file (or other) input. No-op on the non-input kinds.
    #[must_use]
    pub fn multiple(mut self) -> Self {
        if let FieldKind::Input { multiple, .. } = &mut self.kind {
            *multiple = true;
        }
        self
    }

    /// Attach hint text below the control, wired through `aria-describedby`.
    #[must_use]
    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// Attach a validation message to *this* control — Rails' `field_with_errors`
    /// convention, where the reason sits beside the input that caused it rather
    /// than only in a banner at the top of the form.
    ///
    /// Sets `aria-invalid="true"` and appends the message's id to
    /// `aria-describedby`, so a screen reader announces the control as invalid
    /// and reads the reason as part of the control's description. The wrapper
    /// also gains `nav-field--invalid` for the visual treatment.
    ///
    /// Server-rendered: the handler redirects back with the message and the
    /// submitted values (post/redirect/get), so this needs no JavaScript and
    /// works before hydration.
    #[must_use]
    pub fn error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }

    /// A placeholder for an input field (no-op on other kinds).
    #[must_use]
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        if let FieldKind::Input { placeholder: p, .. } = &mut self.kind {
            *p = Some(placeholder.into());
        }
        self
    }

    /// A leading add-on rendered inside the control's group — the `$` on a
    /// currency amount (no-op on other kinds).
    #[must_use]
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        if let FieldKind::Input { prefix: p, .. } = &mut self.kind {
            *p = Some(prefix.into());
        }
        self
    }

    /// The numeric `step` increment — `0.01` on a dollars-and-cents amount
    /// (no-op on other kinds).
    #[must_use]
    pub fn step(mut self, step: impl Into<String>) -> Self {
        if let FieldKind::Input { step: s, .. } = &mut self.kind {
            *s = Some(step.into());
        }
        self
    }

    /// Override the control's DOM id, which defaults to `name`.
    ///
    /// Needed when a page renders the same field name in more than one form —
    /// the clause editor's per-clause `body` textareas. Without it every one of
    /// them takes `id="body"`, and duplicate ids break `<label for>` targeting
    /// as well as the WCAG duplicate-id rule.
    #[must_use]
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.control_id = Some(id.into());
        self
    }

    /// The control's DOM id — the override when set, else `name`.
    fn control_id(&self) -> String {
        self.control_id.clone().unwrap_or_else(|| self.name.clone())
    }

    /// Datalist suggestions for an input field (no-op on other kinds).
    #[must_use]
    pub fn suggestions(mut self, suggestions: Vec<String>) -> Self {
        if let FieldKind::Input { suggestions: s, .. } = &mut self.kind {
            *s = Some(suggestions);
        }
        self
    }

    /// Render this field to an `Element`.
    /// The id `aria-describedby` should point at, given which of the hint and
    /// the validation message are present.
    ///
    /// It takes a space-separated id list, so a field carrying both announces
    /// both — the reason first, because it is why the user is back on this
    /// form at all.
    fn described_by(&self) -> Option<String> {
        let help_id = format!("{}-help", self.control_id());
        let error_id = format!("{}-error", self.control_id());
        match (self.error.is_some(), self.help.is_some()) {
            (true, true) => Some(format!("{error_id} {help_id}")),
            (true, false) => Some(error_id),
            (false, true) => Some(help_id),
            (false, false) => None,
        }
    }

    /// `Some("true")` when the control is invalid, shaped for the optional
    /// `aria-invalid` attribute (absent entirely on a valid control).
    fn invalid(&self) -> Option<&'static str> {
        self.error.as_ref().map(|_| "true")
    }

    /// The wrapper class, carrying the invalid state for the visual treatment.
    fn field_class(&self) -> &'static str {
        if self.error.is_some() {
            "nav-field nav-field--invalid"
        } else {
            "nav-field"
        }
    }

    /// `pub(crate)`: [`crate::notation_demo`] renders a `Field` directly,
    /// outside a [`FormCard`] — its stepper has no `<form>` at all, so it
    /// cannot go through [`FormCard::render`]'s field loop.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn render(&self) -> Element {
        let control_id = self.control_id();
        let help_id = format!("{control_id}-help");
        let error_id = format!("{control_id}-error");
        let described_by = self.described_by();
        let invalid = self.invalid();
        let field_class = self.field_class();
        let check_class = if self.error.is_some() {
            "nav-field nav-field--check nav-field--invalid"
        } else {
            "nav-field nav-field--check"
        };
        let help = self.help.clone();
        let error = self.error.clone();
        let picker_help = help.clone();
        let picker_error = error.clone();
        let name = self.name.clone();
        let label = self.label.clone();
        let required = self.required;

        let help_block = rsx! {
            if let Some(e) = error {
                // `role="alert"` so the message is announced when the page
                // loads after the redirect, not only when focus reaches the
                // control.
                div { class: "nav-field__error", id: "{error_id}", role: "alert", "{e}" }
            }
            if let Some(h) = help {
                div { class: "nav-field__help", id: "{help_id}", "{h}" }
            }
        };

        match &self.kind {
            FieldKind::Checkbox { value, checked } => rsx! {
                div { class: "{check_class}",
                    input {
                        class: "nav-checkbox",
                        r#type: "checkbox",
                        id: "{control_id}",
                        name: "{name}",
                        value: "{value}",
                        checked: *checked,
                        required,
                        "aria-describedby": described_by,
                        "aria-invalid": invalid,
                    }
                    label { class: "nav-label", r#for: "{control_id}",
                        "{label}"
                        if required {
                            span { class: "nav-required", "aria-hidden": "true", " *" }
                        }
                    }
                    {help_block}
                }
            },
            FieldKind::Radio {
                options,
                selected,
                locked,
            } => rsx! {
                fieldset { class: "{field_class} nav-field--radio",
                    legend { class: "nav-label",
                        "{label}"
                        if required {
                            span { class: "nav-required", "aria-hidden": "true", " *" }
                        }
                    }
                    for choice in options.iter() {
                        div {
                            class: if locked.contains(&choice.value) { "nav-radio nav-radio--locked" } else { "nav-radio" },
                            input {
                                class: "nav-radio__input",
                                r#type: "radio",
                                id: "{control_id}-{choice.value}",
                                name: "{name}",
                                value: "{choice.value}",
                                checked: selected.as_deref() == Some(choice.value.as_str()),
                                disabled: locked.contains(&choice.value),
                                "aria-describedby": described_by.clone(),
                                "aria-invalid": invalid,
                            }
                            label { class: "nav-label nav-label--inline", r#for: "{control_id}-{choice.value}", "{choice.label}" }
                        }
                    }
                    {help_block}
                }
            },
            FieldKind::Input { .. } => self.render_input(described_by.as_deref(), help_block),
            FieldKind::Textarea { value, rows } => rsx! {
                div { class: "{field_class}",
                    label { class: "nav-label", r#for: "{control_id}",
                        "{label}"
                        if required {
                            span { class: "nav-required", "aria-hidden": "true", " *" }
                        }
                    }
                    // A `<textarea>`'s value must be its inner content, and a
                    // `<textarea>` is RCDATA — so neither a `value` attribute
                    // (browsers ignore it) nor a Dioxus child text node (whose
                    // hydration-comment markers would render as literal `<!--…-->`
                    // text in the box) works. Set the escaped value as inner HTML.
                    textarea {
                        class: "nav-input",
                        id: "{control_id}",
                        name: "{name}",
                        rows: "{rows}",
                        required,
                        "aria-describedby": described_by,
                        "aria-invalid": invalid,
                        dangerous_inner_html: escape_rcdata(value),
                    }
                    {help_block}
                }
            },
            FieldKind::Select {
                options,
                selected,
                disabled,
            } => rsx! {
                div { class: "{field_class}",
                    label { class: "nav-label", r#for: "{control_id}",
                        "{label}"
                        if required {
                            span { class: "nav-required", "aria-hidden": "true", " *" }
                        }
                    }
                    select {
                        class: "nav-select",
                        id: "{control_id}",
                        name: "{name}",
                        required,
                        disabled: *disabled,
                        "aria-describedby": described_by,
                        "aria-invalid": invalid,
                        for o in options.iter() {
                            option {
                                value: "{o.value}",
                                selected: Some(o.value.clone()) == *selected,
                                "{o.label}"
                            }
                        }
                    }
                    {help_block}
                }
            },
            FieldKind::PersonPicker {
                blank_label,
                people,
                selected,
                search,
                disabled,
            } => rsx! {
                PersonPicker {
                    label: label.clone(),
                    name: name.clone(),
                    blank_label: blank_label.clone(),
                    people: people.clone(),
                    selected: selected.clone(),
                    search: search.clone(),
                    help: picker_help,
                    error: picker_error,
                    required,
                    disabled: *disabled,
                    control_id: Some(control_id.clone()),
                }
            },
        }
    }

    /// Render the [`FieldKind::Input`] variant (the richest one: prefix add-on,
    /// placeholder, numeric step, datalist suggestions), split out to keep
    /// [`Field::render`] under the line limit. `self.kind` is always `Input`
    /// here — the sole caller dispatches on it.
    fn render_input(&self, described_by: Option<&str>, help_block: Element) -> Element {
        let name = self.name.clone();
        let label = self.label.clone();
        let required = self.required;
        let invalid = self.invalid();
        let field_class = self.field_class();
        let FieldKind::Input {
            input_type,
            value,
            placeholder,
            prefix,
            step,
            disabled,
            multiple,
            suggestions,
        } = &self.kind
        else {
            return rsx! {};
        };
        let control_id = self.control_id();
        let list_id = format!("{control_id}-suggestions");
        let list_attr = suggestions.as_ref().map(|_| list_id.clone());
        let options = suggestions.clone().unwrap_or_default();
        rsx! {
            div { class: "{field_class}",
                label { class: "nav-label", r#for: "{control_id}",
                    "{label}"
                    if required {
                        span { class: "nav-required", "aria-hidden": "true", " *" }
                    }
                }
                if let Some(prefix) = prefix {
                    div { class: "nav-input-group",
                        span { class: "nav-input-group__addon", "{prefix}" }
                        input {
                            class: "nav-input",
                            r#type: "{input_type}",
                            id: "{control_id}",
                            name: "{name}",
                            value: "{value}",
                            placeholder: placeholder.clone(),
                            step: step.clone(),
                            required,
                            disabled: *disabled,
                            multiple: *multiple,
                            list: list_attr.clone(),
                            "aria-describedby": described_by,
                        "aria-invalid": invalid,
                        }
                    }
                } else {
                    input {
                        class: "nav-input",
                        r#type: "{input_type}",
                        id: "{control_id}",
                        name: "{name}",
                        value: "{value}",
                        placeholder: placeholder.clone(),
                        step: step.clone(),
                        required,
                        disabled: *disabled,
                        multiple: *multiple,
                        list: list_attr.clone(),
                        "aria-describedby": described_by,
                        "aria-invalid": invalid,
                    }
                }
                if !options.is_empty() {
                    datalist { id: "{list_id}",
                        for opt in options.iter() {
                            option { value: "{opt}" }
                        }
                    }
                }
                {help_block}
            }
        }
    }
}

/// Build the control(s) a questionnaire question renders for its `answer_type`.
///
/// Shared by both walkers over the same notation — the lawyer walk and the
/// client self-serve intake — so a client confirming what lawyer entered meets
/// the same control for the same answer type.
///
/// `people_list` returns no [`Field`] at all: it is a composite widget whose
/// rows come through the card's `extra_fields` slot (see
/// [`crate::components::PeopleListInputs`]), which the `POST` handler assembles
/// into one answer.
#[must_use]
pub fn question_fields(
    answer_type: &str,
    prompt: &str,
    prior: &str,
    country_options: &[String],
) -> Vec<Field> {
    match answer_type {
        "people_list" => Vec::new(),
        "text" => vec![Field::textarea(prompt, "value", prior, 4).required()],
        "int" => vec![Field::number(prompt, "value", prior).required()],
        "datetime" => vec![Field::input(prompt, "value", prior, "datetime-local").required()],
        "custom_usd" => vec![Field::input(prompt, "value", prior, "number")
            .prefix("$")
            .step("0.01")
            .placeholder("0.00")
            .help("Enter dollars and cents, e.g. 1250.00.")
            .required()],
        "custom_phone" => vec![Field::input(prompt, "value", prior, "tel")
            .placeholder("(702) 555-0100")
            .help("Include the country code if the number is outside the U.S.")
            .required()],
        "country" => vec![Field::country_select(
            prompt,
            country_options,
            Some(prior).filter(|v| !v.is_empty()),
        )],
        "bool" | "yes_no" => vec![Field::checkbox(prompt, "value", "true", prior == "true")],
        _ => vec![Field::text(prompt, "value", prior).required()],
    }
}

/// The heading level at the top of the card. A standalone form page owns the
/// page `h1`; a form embedded under an existing `h1` uses `h2`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Heading {
    H1,
    H2,
}

/// A complete create / edit form rendered as a constrained card. Native submit
/// to `action` with `method` (works pre-hydration).
#[component]
pub fn FormCard(
    title: String,
    action: String,
    submit_label: String,
    #[props(default = "post".to_string())] method: String,
    #[props(default = Heading::H1)] heading: Heading,
    #[props(default)] multipart: bool,
    /// The session CSRF token. When present it renders a hidden `_csrf` field so
    /// a `POST` form clears the CSRF guard on the mutation handler; `None` on the
    /// public forms (e.g. the design gallery) that post nowhere protected.
    #[props(default)]
    csrf_token: Option<String>,
    /// Render the form read-only: keep the (disabled) fields for context but omit
    /// the submit button, so nothing on the page invites a write. The immutable
    /// super-admin person record uses this — the command layer rejects every edit
    /// to it, so the form must not offer a Save.
    #[props(default)]
    read_only: bool,
    /// Muted introductory prose between the title and the fields — the "here's
    /// what you're confirming" framing a client-facing form leads with.
    #[props(default)]
    intro: Option<Element>,
    /// Controls appended inside the `<form>` after the [`Field`]s, for a
    /// composite widget one `Field` cannot express — the `people_list` row
    /// groups, whose several inputs the handler assembles into one answer.
    /// They post with the rest of the form.
    #[props(default)]
    extra_fields: Option<Element>,
    fields: Vec<Field>,
) -> Element {
    let enctype = multipart.then(|| "multipart/form-data".to_string());
    rsx! {
        div { class: "nav-card nav-form-card",
            div { class: "nav-card__body",
                if heading == Heading::H1 {
                    h1 { class: "nav-form-card__title", "{title}" }
                } else {
                    h2 { class: "nav-form-card__title", "{title}" }
                }
                if let Some(intro) = intro {
                    div { class: "nav-muted nav-form-card__intro", {intro} }
                }
                form {
                    // `admin-form` carries no styling — it is the stable hook the
                    // browser accessibility e2e (`web/tests/accessibility_e2e.rs`)
                    // scopes axe to. The form it succeeds kept it for the same
                    // reason; dropping it in the Phase 3 migration timed the gate out.
                    class: "nav-form admin-form",
                    action: "{action}",
                    method: "{method}",
                    // The card title is the form's accessible name (WCAG: a
                    // `<form>` is only exposed as a landmark when it is named),
                    // matching the form a11y invariant.
                    "aria-label": "{title}",
                    enctype,
                    if let Some(token) = csrf_token.as_ref() {
                        input { r#type: "hidden", name: "_csrf", value: "{token}" }
                    }
                    for field in fields.iter() {
                        {field.render()}
                    }
                    if let Some(extra) = extra_fields {
                        {extra}
                    }
                    if !read_only {
                        button { class: "nav-btn nav-btn--primary", r#type: "submit", "{submit_label}" }
                    }
                }
            }
        }
    }
}

/// Layer-1 accessibility invariants for a rendered form page — the Dioxus
/// successor to the `views/tests/accessibility.rs` gate the migrated pages
/// were held to. Test-only, and shared across the page modules so each migrated
/// form is checked against exactly the same rules:
///
/// 1. no positive `tabindex` (DOM order drives focus);
/// 2. every `<label for>` points at an id that exists;
/// 3. every `aria-describedby` points at an id that exists; and
/// 4. every `<form>` carries an accessible name (`aria-label`).
///
/// `label` names the page in the failure message.
#[cfg(test)]
pub(crate) fn assert_forms_accessible(html: &str, label: &str) {
    /// Collect the values of every `attr="…"` occurrence in `html`.
    fn values<'a>(html: &'a str, attr: &str) -> Vec<&'a str> {
        let needle = format!(" {attr}=\"");
        html.match_indices(&needle)
            .filter_map(|(at, _)| {
                let rest = &html[at + needle.len()..];
                rest.find('"').map(|end| &rest[..end])
            })
            .collect()
    }

    let ids: std::collections::HashSet<&str> = values(html, "id").into_iter().collect();

    for raw in values(html, "tabindex") {
        let value: i32 = raw.parse().unwrap_or(0);
        assert!(
            value <= 0,
            "{label}: positive tabindex ({value}) breaks DOM focus order",
        );
    }
    for target in values(html, "for") {
        assert!(
            ids.contains(target),
            "{label}: <label for=\"{target}\"> points at no element",
        );
    }
    for described in values(html, "aria-describedby") {
        for target in described.split_whitespace() {
            assert!(
                ids.contains(target),
                "{label}: aria-describedby=\"{target}\" points at no element",
            );
        }
    }
    let forms = html.matches("<form").count();
    let named = values(html, "aria-label").len();
    assert!(
        named >= forms,
        "{label}: {forms} form(s) but only {named} aria-label(s) — a form is only \
         exposed as a landmark when it is named",
    );
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
    fn renders_a_native_form_with_labeled_fields_and_submit() {
        fn app() -> Element {
            rsx! {
                FormCard {
                    title: "Contact us".to_string(),
                    action: "/contact".to_string(),
                    submit_label: "Send".to_string(),
                    fields: vec![
                        Field::email("Email", "email", "").required(),
                        Field::textarea("Message", "message", "", 4),
                    ],
                }
            }
        }
        let html = ssr(app);
        assert!(
            html.contains(r#"<form class="nav-form admin-form" action="/contact" method="post""#),
            "{html}"
        );
        assert!(
            html.contains(r#"<label class="nav-label" for="email""#),
            "{html}"
        );
        assert!(html.contains(r#"type="email""#), "{html}");
        assert!(html.contains("required"), "{html}");
        // Required cue is a visible, aria-hidden star.
        assert!(
            html.contains(r#"<span class="nav-required" aria-hidden="true"> *</span>"#),
            "{html}"
        );
        assert!(html.contains("<textarea"), "{html}");
        assert!(html.contains(r#"type="submit""#), "{html}");
        assert!(html.contains("Send"), "{html}");
        // No Bootstrap classes.
        assert!(!html.contains("form-control"), "{html}");
    }

    #[test]
    fn keeps_admin_form_class_for_the_e2e_selector() {
        // `web/tests/accessibility_e2e.rs` waits for `form.admin-form` before
        // running axe. The Phase 3 migration to this Dioxus FormCard must keep
        // the class or the deploy's browser gate times out (WaitTimeout).
        fn app() -> Element {
            rsx! {
                FormCard {
                    title: "Edit".to_string(),
                    action: "/x".to_string(),
                    submit_label: "Save".to_string(),
                    fields: vec![Field::text("Name", "name", "")],
                }
            }
        }
        let html = ssr(app);
        assert!(
            html.contains("admin-form"),
            "browser e2e locates the form by .admin-form, got: {html}"
        );
    }

    #[test]
    fn select_marks_the_selected_option() {
        fn app() -> Element {
            rsx! {
                FormCard {
                    title: "Pick".to_string(),
                    action: "/pick".to_string(),
                    submit_label: "Go".to_string(),
                    fields: vec![Field::select(
                        "Role",
                        "role",
                        vec![Choice::new("lawyer", "Lawyer"), Choice::new("admin", "Admin")],
                        Some("admin".to_string()),
                    )],
                }
            }
        }
        let html = ssr(app);
        assert!(html.contains(r#"<select class="nav-select""#), "{html}");
        assert!(html.contains(r#"<option value="admin" selected"#), "{html}");
    }

    /// Rails' `field_with_errors`, as this component expresses it: the message
    /// sits with the control, the control is marked invalid, and both the
    /// message and any hint are reachable from `aria-describedby`.
    #[test]
    fn a_field_error_marks_the_control_invalid_and_describes_it() {
        fn app() -> Element {
            rsx! {
                FormCard {
                    title: "Add person".to_string(),
                    action: "/app/admin/people".to_string(),
                    submit_label: "Create".to_string(),
                    fields: vec![
                        Field::email("Email", "email", "not-an-email")
                            .required()
                            .help("We reply here.")
                            .error("Enter a valid email address."),
                        Field::text("Name", "name", "Ada"),
                    ],
                }
            }
        }
        let html = ssr(app);

        // The message renders beside its control, announced on load rather than
        // only when focus arrives. SSR interleaves hydration comments between an
        // element's attributes and its text, so match `>Text<`, never
        // `class="x">Text`.
        assert!(html.contains(r#"id="email-error""#), "{html}");
        assert!(html.contains(r#"role="alert""#), "{html}");
        assert!(html.contains(">Enter a valid email address.<"), "{html}");

        // The control is marked invalid and describes itself with the reason
        // FIRST, then the hint.
        assert!(html.contains(r#"aria-invalid="true""#), "{html}");
        assert!(
            html.contains(r#"aria-describedby="email-error email-help""#),
            "reason precedes hint in the description: {html}"
        );

        // The wrapper carries the invalid state for the visual treatment.
        assert!(html.contains("nav-field nav-field--invalid"), "{html}");

        // The value the user submitted survives the redirect, so the draft is
        // not retyped.
        assert!(html.contains(r#"value="not-an-email""#), "{html}");

        // A field with no error is untouched: no stray invalid state anywhere.
        assert!(!html.contains(r#"id="name-error""#), "{html}");

        // The whole-form accessibility invariants still hold, including that
        // every `aria-describedby` points at an element that exists.
        assert_forms_accessible(&html, "field error");
    }

    #[test]
    fn a_field_error_without_help_describes_only_the_error() {
        fn app() -> Element {
            rsx! {
                FormCard {
                    title: "Add person".to_string(),
                    action: "/app/admin/people".to_string(),
                    submit_label: "Create".to_string(),
                    fields: vec![Field::text("Name", "name", "").error("Name is required.")],
                }
            }
        }
        let html = ssr(app);
        assert!(html.contains(r#"aria-describedby="name-error""#), "{html}");
        assert!(!html.contains("name-help"), "{html}");
        assert_forms_accessible(&html, "field error, no help");
    }

    /// A clean form must gain nothing from this feature — no invalid state, no
    /// empty message node, no dangling description.
    #[test]
    fn a_form_without_errors_is_unchanged() {
        fn app() -> Element {
            rsx! {
                FormCard {
                    title: "Add person".to_string(),
                    action: "/app/admin/people".to_string(),
                    submit_label: "Create".to_string(),
                    fields: vec![Field::text("Name", "name", "Ada").help("Legal name.")],
                }
            }
        }
        let html = ssr(app);
        assert!(!html.contains("nav-field--invalid"), "{html}");
        assert!(!html.contains("aria-invalid"), "{html}");
        assert!(!html.contains("nav-field__error"), "{html}");
        assert!(html.contains(r#"aria-describedby="name-help""#), "{html}");
    }

    #[test]
    fn read_only_card_disables_fields_and_omits_the_submit() {
        // The immutable super-admin person record renders its fields for context
        // but offers no Save, and a `.disabled()` field carries the native
        // `disabled` attribute so nothing on the page invites a rejected write.
        fn app() -> Element {
            rsx! {
                FormCard {
                    title: "Edit person".to_string(),
                    action: "/app/admin/people/1".to_string(),
                    submit_label: "Save".to_string(),
                    read_only: true,
                    fields: vec![
                        Field::text("Name", "name", "Nick").disabled(),
                        Field::select(
                            "Role",
                            "role",
                            vec![Choice::new("admin", "Admin")],
                            Some("admin".to_string()),
                        )
                        .disabled(),
                    ],
                }
            }
        }
        let html = ssr(app);
        // The disabled attribute is present on both controls.
        assert!(
            html.contains(r#"id="name""#) && html.contains("disabled"),
            "{html}"
        );
        assert!(html.contains(r#"<select class="nav-select""#), "{html}");
        // No submit button on a read-only card.
        assert!(!html.contains(r#"type="submit""#), "{html}");
    }

    #[test]
    fn input_help_is_wired_through_aria_describedby() {
        fn app() -> Element {
            rsx! {
                FormCard {
                    title: "Kind".to_string(),
                    action: "/k".to_string(),
                    submit_label: "Save".to_string(),
                    fields: vec![Field::text("Kind", "kind", "").help("Optional.")],
                }
            }
        }
        let html = ssr(app);
        assert!(html.contains(r#"aria-describedby="kind-help""#), "{html}");
        assert!(
            html.contains(r#"<div class="nav-field__help" id="kind-help">Optional."#),
            "{html}"
        );
    }

    #[test]
    fn checkbox_renders_its_checked_state() {
        fn app() -> Element {
            rsx! {
                FormCard {
                    title: "Prefs".to_string(),
                    action: "/p".to_string(),
                    submit_label: "Save".to_string(),
                    fields: vec![Field::checkbox("Subscribe", "subscribe", "yes", true)],
                }
            }
        }
        let html = ssr(app);
        assert!(html.contains(r#"type="checkbox""#), "{html}");
        assert!(html.contains("checked"), "{html}");
    }

    #[test]
    fn checkbox_marks_required() {
        fn app() -> Element {
            rsx! {
                FormCard {
                    title: "Terms".to_string(),
                    action: "/t".to_string(),
                    submit_label: "Save".to_string(),
                    fields: vec![
                        Field::checkbox("I agree to the terms", "agree", "yes", false).required(),
                    ],
                }
            }
        }
        let html = ssr(app);
        assert!(html.contains(r#"type="checkbox""#), "{html}");
        // The native required state is on the control...
        assert!(html.contains("required"), "{html}");
        // ...and the visible, aria-hidden star cue is on the label.
        assert!(
            html.contains(r#"<span class="nav-required" aria-hidden="true"> *</span>"#),
            "{html}"
        );
    }

    #[test]
    fn input_prefix_and_suggestions_render() {
        fn app() -> Element {
            rsx! {
                FormCard {
                    title: "Fee".to_string(),
                    action: "/f".to_string(),
                    submit_label: "Save".to_string(),
                    fields: vec![
                        Field::input("Amount", "amount", "", "number")
                            .placeholder("0.00")
                            .suggestions(vec!["100".to_string(), "250".to_string()]),
                    ],
                }
            }
        }
        let html = ssr(app);
        assert!(html.contains(r#"placeholder="0.00""#), "{html}");
        assert!(html.contains("<datalist"), "{html}");
        assert!(html.contains(r#"list="amount-suggestions""#), "{html}");
    }
}
