//! A browser-local checklist. It does not accept or value a claim.

use super::CyberInjuryContent;
use dioxus::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Answers {
    incident: usize,
    deadline_check: bool,
    medical_care: bool,
}

fn answers(incident: &str, timing: &str, care: &str) -> Option<Answers> {
    let incident = match incident {
        "vehicle" => 0,
        "fall" => 1,
        "work" => 2,
        "other" => 3,
        _ => return None,
    };
    let deadline_check = match timing {
        "recent" | "months" => false,
        "older" | "unsure" => true,
        _ => return None,
    };
    let medical_care = match care {
        "yes" => true,
        "no" => false,
        _ => return None,
    };
    Some(Answers {
        incident,
        deadline_check,
        medical_care,
    })
}

#[component]
pub(super) fn Assessment(copy: CyberInjuryContent, consultation_href: String) -> Element {
    let mut incident = use_signal(String::new);
    let mut timing = use_signal(String::new);
    let mut care = use_signal(String::new);
    let mut result = use_signal(|| None::<Answers>);
    let mut invalid = use_signal(|| false);
    rsx! {
        form { class: "cyber-assessment-card",
            onsubmit: move |event| { event.prevent_default(); let next = answers(&incident(), &timing(), &care()); invalid.set(next.is_none()); result.set(next); },
            div { class: "cyber-form-title", h3 { "YOUR FREE CASE CHECK" } span { "ABOUT 30 SECONDS" } }
            label { r#for: "cyber-incident", "What happened?" }
            select { id: "cyber-incident", required: true, value: "{incident}", oninput: move |event| { incident.set(event.value()); result.set(None); },
                option { value: "", disabled: true, selected: incident().is_empty(), "Select an incident" }
                option { value: "vehicle", "Car, truck, bicycle, or pedestrian crash" }
                option { value: "fall", "Slip, trip, or fall" }
                option { value: "work", "Workplace or construction injury" }
                option { value: "other", "Another type of injury" }
            }
            label { r#for: "cyber-timing", "When did it happen?" }
            select { id: "cyber-timing", required: true, value: "{timing}", oninput: move |event| { timing.set(event.value()); result.set(None); },
                option { value: "", disabled: true, selected: timing().is_empty(), "Select a time frame" }
                option { value: "recent", "Within the last 30 days" }
                option { value: "months", "1–12 months ago" }
                option { value: "older", "More than a year ago" }
                option { value: "unsure", "I’m not sure" }
            }
            fieldset {
                legend { "Have you received medical care?" }
                for (value, label) in [("yes", "Yes"), ("no", "Not yet")] {
                    label { class: "cyber-radio",
                        input { r#type: "radio", name: "cyber-care", value, required: true, checked: care() == value, onchange: move |_| { care.set(value.to_string()); result.set(None); } }
                        "{label}"
                    }
                }
            }
            button { class: "cyber-button", r#type: "submit", "See my preliminary assessment" span { "aria-hidden": "true", "↗" } }
            if invalid() { p { role: "alert", "Please answer all three questions." } }
            p { class: "cyber-fine", "{copy.assessment_note}" }
            div { "aria-live": "polite", "aria-atomic": "true",
                if let Some(answer) = result() {
                    section { class: "cyber-result", "aria-labelledby": "cyber-result-title",
                        span { class: "cyber-eyebrow", "YOUR NEXT STEP" }
                        h3 { id: "cyber-result-title", if answer.deadline_check { "MAKE A DEADLINE CHECK YOUR NEXT MOVE." } else { "YOUR STORY DESERVES A CLOSER LOOK." } }
                        p { if answer.deadline_check { "{copy.deadline_result}" } else { "{copy.recent_result}" } }
                        ul {
                            li { "{copy.evidence_tips[answer.incident]}" }
                            li { if answer.medical_care { "{copy.medical_tips[0]}" } else { "{copy.medical_tips[1]}" } }
                            li { "{copy.deadline_tip}" }
                        }
                        p { class: "cyber-fine", "{copy.result_note}" }
                        a { class: "cyber-button", href: consultation_href, "Free consultation" span { "aria-hidden": "true", "↗" } }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_or_unknown_answers_never_produce_an_assessment() {
        for (incident, timing, care) in [
            ("", "recent", "yes"),
            ("vehicle", "", "yes"),
            ("vehicle", "recent", ""),
            ("invalid", "older", "no"),
        ] {
            assert!(answers(incident, timing, care).is_none());
        }
    }

    #[test]
    fn older_and_unknown_dates_request_a_deadline_check() {
        for timing in ["older", "unsure"] {
            let answer = answers("work", timing, "no").unwrap();
            assert!(answer.deadline_check);
            assert!(!answer.medical_care);
            assert_eq!(answer.incident, 2);
        }
        assert!(!answers("vehicle", "recent", "yes").unwrap().deadline_check);
    }
}
