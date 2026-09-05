//! `Progress` — the shared progress bar for a long-running or multi-step
//! operation (ENG-502, follow-up to ENG-403).
//!
//! `walker_step.rs`, `client_intake.rs`, and `notation_demo.rs` each carried
//! their own bare `div { role: "progressbar", ... }` with no visible track or
//! fill. This is the one component all three now render, matching the public
//! Navigator UX gallery's shipped markup exactly
//! (`?component=skeleton-and-progress`).

use dioxus::prelude::*;

/// A determinate or indeterminate progress bar.
///
/// `value` is the current position (e.g. the questionnaire step reached);
/// `max` is the total. `value: None` renders the indeterminate variant — a
/// bar with motion but no reportable position (`nav-progress__track--indeterminate`,
/// no `aria-valuenow`), for an operation with no known length (e.g. "Running
/// the conflicts check").
#[component]
pub fn Progress(
    /// Names the operation for assistive technology — "Intake progress".
    label: String,
    value: Option<usize>,
    #[props(default = 100)] max: usize,
    /// Render the numeric value ("2 of 4") beside the bar.
    #[props(default)]
    show_value: bool,
) -> Element {
    let track_class = if value.is_some() {
        "nav-progress__track"
    } else {
        "nav-progress__track nav-progress__track--indeterminate"
    };
    let percent =
        value.map(|v| v.min(max).saturating_mul(100).checked_div(max).unwrap_or(0));
    let fill_style = percent.map(|p| format!("width: {p}%;"));
    let value_text = value.map(|v| format!("{v} of {max}"));

    rsx! {
        div { class: "nav-progress",
            div {
                class: "{track_class}",
                role: "progressbar",
                "aria-label": "{label}",
                "aria-valuenow": value.map(|v| v.to_string()),
                "aria-valuemin": value.map(|_| "0".to_string()),
                "aria-valuemax": value.map(|_| max.to_string()),
                div { class: "nav-progress__fill", style: fill_style }
            }
            if show_value {
                if let Some(value_text) = value_text {
                    span { class: "nav-progress__value", "{value_text}" }
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

    #[test]
    fn a_determinate_value_renders_the_track_fill_and_aria_attributes() {
        fn app() -> Element {
            rsx! {
                Progress { label: "Intake progress".to_string(), value: Some(2), max: 4 }
            }
        }
        let out = ssr(app);
        assert!(out.contains("nav-progress"), "{out}");
        assert!(out.contains(r#"class="nav-progress__track""#), "{out}");
        assert!(!out.contains("nav-progress__track--indeterminate"), "{out}");
        assert!(out.contains(r#"role="progressbar""#), "{out}");
        assert!(out.contains(r#"aria-label="Intake progress""#), "{out}");
        assert!(out.contains(r#"aria-valuenow="2""#), "{out}");
        assert!(out.contains(r#"aria-valuemin="0""#), "{out}");
        assert!(out.contains(r#"aria-valuemax="4""#), "{out}");
        assert!(out.contains("nav-progress__fill"), "{out}");
        assert!(out.contains("width: 50%;"), "{out}");
    }

    #[test]
    fn a_none_value_renders_the_indeterminate_variant_with_no_position() {
        fn app() -> Element {
            rsx! {
                Progress { label: "Running the conflicts check".to_string(), value: None }
            }
        }
        let out = ssr(app);
        assert!(out.contains("nav-progress__track--indeterminate"), "{out}");
        assert!(!out.contains("aria-valuenow"), "{out}");
        assert!(!out.contains("aria-valuemin"), "{out}");
        assert!(!out.contains("aria-valuemax"), "{out}");
    }

    #[test]
    fn show_value_renders_the_position_beside_the_bar() {
        fn app() -> Element {
            rsx! {
                Progress { label: "Intake progress".to_string(), value: Some(3), max: 10, show_value: true }
            }
        }
        let out = ssr(app);
        assert!(out.contains(r#"class="nav-progress__value""#), "{out}");
        assert!(out.contains("3 of 10"), "{out}");
    }

    #[test]
    fn show_value_renders_nothing_extra_when_indeterminate() {
        fn app() -> Element {
            rsx! {
                Progress { label: "Working".to_string(), value: None, show_value: true }
            }
        }
        let out = ssr(app);
        assert!(!out.contains("nav-progress__value"), "{out}");
    }
}
