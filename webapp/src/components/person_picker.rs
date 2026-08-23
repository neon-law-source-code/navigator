//! A searchable picker for a foreign-key reference to a [`Person`](PersonChoice).
//!
//! The control keeps the submitted value as the person's UUID. Its separate
//! search input narrows the native `<select>` by name or email on a regular
//! GET submission, while the selected value remains a native form field.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// One person a foreign-key picker can name. Components receive this render
/// shape rather than a store row, preserving the components module's leaf
/// boundary.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersonChoice {
    pub id: String,
    pub name: String,
    pub email: String,
    /// Optional context supplied by the caller, such as a system tier.
    pub detail: Option<String>,
}

impl PersonChoice {
    #[must_use]
    pub fn new(id: impl Into<String>, name: impl Into<String>, email: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            email: email.into(),
            detail: None,
        }
    }

    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    fn label(&self) -> String {
        match self.detail.as_deref() {
            Some(detail) => format!("{} <{}> — {detail}", self.name, self.email),
            None => format!("{} <{}>", self.name, self.email),
        }
    }
}

fn matches(person: &PersonChoice, needle: &str) -> bool {
    let needle = needle.trim().to_lowercase();
    needle.is_empty()
        || person.name.to_lowercase().contains(&needle)
        || person.email.to_lowercase().contains(&needle)
}

fn alphabetized(mut people: Vec<PersonChoice>) -> Vec<PersonChoice> {
    people.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.email.to_lowercase().cmp(&right.email.to_lowercase()))
    });
    people
}

/// A server-filterable native person selector.
///
/// `name` is the foreign-key field the surrounding form posts. The separate
/// `{name}_search` query field never becomes the stored person reference.
#[component]
pub fn PersonPicker(
    label: String,
    name: String,
    blank_label: String,
    people: Vec<PersonChoice>,
    #[props(default)] selected: Option<String>,
    #[props(default)] search: Option<String>,
    #[props(default)] help: Option<String>,
    #[props(default)] error: Option<String>,
    #[props(default)] required: bool,
    #[props(default)] disabled: bool,
    #[props(default)] control_id: Option<String>,
) -> Element {
    let control_id = control_id.unwrap_or_else(|| name.clone());
    let search_id = format!("{control_id}-search");
    let search_name = format!("{name}_search");
    let help_id = format!("{control_id}-help");
    let error_id = format!("{control_id}-error");
    let described_by = match (error.is_some(), help.is_some()) {
        (true, true) => Some(format!("{error_id} {help_id}")),
        (true, false) => Some(error_id.clone()),
        (false, true) => Some(help_id.clone()),
        (false, false) => None,
    };
    let search = search.unwrap_or_default();
    let filtered: Vec<PersonChoice> = alphabetized(people)
        .into_iter()
        .filter(|person| {
            matches(person, &search) || selected.as_deref() == Some(person.id.as_str())
        })
        .collect();
    let match_label = if filtered.len() == 1 {
        "person"
    } else {
        "people"
    };
    let field_class = if error.is_some() {
        "nav-field nav-field--invalid person-picker"
    } else {
        "nav-field person-picker"
    };
    let invalid = error.is_some().then_some("true");

    rsx! {
        div { class: "{field_class}",
            label { class: "nav-label", r#for: "{control_id}",
                "{label}"
                if required {
                    span { class: "nav-required", "aria-hidden": "true", " *" }
                }
            }
            label { class: "person-picker__search-label", r#for: "{search_id}", "Find a person" }
            input {
                class: "nav-input",
                r#type: "search",
                id: "{search_id}",
                name: "{search_name}",
                placeholder: "Type a name or email",
                value: "{search}",
                disabled,
            }
            button {
                class: "nav-btn nav-btn--secondary",
                r#type: "submit",
                formmethod: "get",
                formnovalidate: true,
                disabled,
                "Filter people"
            }
            p { class: "person-picker__matches", "aria-live": "polite",
                "{filtered.len()} {match_label} match"
            }
            select {
                class: "nav-select",
                id: "{control_id}",
                name: "{name}",
                required,
                disabled,
                "aria-describedby": described_by,
                "aria-invalid": invalid,
                option { value: "", selected: selected.is_none(), "{blank_label}" }
                if filtered.is_empty() {
                    option { value: "", disabled: true, "No people match that search." }
                } else {
                    for person in filtered {
                        option {
                            value: "{person.id}",
                            selected: selected.as_deref() == Some(person.id.as_str()),
                            "{person.label()}"
                        }
                    }
                }
            }
            if let Some(error) = error {
                div { class: "nav-field__error", id: "{error_id}", role: "alert", "{error}" }
            }
            if let Some(help) = help {
                div { class: "nav-field__help", id: "{help_id}", "{help}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{alphabetized, matches, PersonChoice, PersonPicker};
    use dioxus::prelude::*;

    fn people() -> Vec<PersonChoice> {
        vec![
            PersonChoice::new("2", "Zoe Client", "zoe@example.com"),
            PersonChoice::new("1", "Ada Client", "ada@example.com").with_detail("client"),
        ]
    }

    #[test]
    fn matches_name_or_email_case_insensitively() {
        let mut people = people();
        let ada = people.remove(1);
        assert!(matches(&ada, "ADA"));
        assert!(matches(&ada, "EXAMPLE.COM"));
        assert!(!matches(&ada, "zoe"));
    }

    #[test]
    fn alphabetizes_by_name_then_email() {
        let names: Vec<String> = alphabetized(people()).into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["Ada Client", "Zoe Client"]);
    }

    #[test]
    fn renders_a_named_search_and_uuid_select_with_email_labels() {
        fn app() -> Element {
            rsx! {
                PersonPicker {
                    label: "Client".to_string(),
                    name: "client_dri_person_id".to_string(),
                    blank_label: "Pick a client".to_string(),
                    people: people(),
                    required: true,
                }
            }
        }
        let html = dioxus_ssr::render_element(app());
        assert!(
            html.contains(r#"id="client_dri_person_id-search""#),
            "{html}"
        );
        assert!(html.contains(r#"name="client_dri_person_id""#), "{html}");
        assert!(
            html.contains(r#"name="client_dri_person_id_search""#),
            "{html}"
        );
        assert!(html.contains("Filter people"), "{html}");
        assert!(html.contains("formnovalidate"), "{html}");
        assert!(
            html.contains("Ada Client &#60;ada@example.com&#62; — client"),
            "{html}"
        );
        assert!(
            html.contains("Zoe Client &#60;zoe@example.com&#62;"),
            "{html}"
        );
    }

    #[test]
    fn search_round_trip_narrows_the_rendered_options() {
        fn app() -> Element {
            rsx! {
                PersonPicker {
                    label: "Client".to_string(),
                    name: "client_dri_person_id".to_string(),
                    blank_label: "Pick a client".to_string(),
                    people: people(),
                    search: Some("zoe".to_string()),
                }
            }
        }
        let html = dioxus_ssr::render_element(app());
        assert!(html.contains("Zoe Client"), "{html}");
        assert!(!html.contains("Ada Client"), "{html}");
        assert!(html.contains("1 person match"), "{html}");
    }
}
