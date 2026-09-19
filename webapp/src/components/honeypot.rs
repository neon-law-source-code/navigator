//! An intentionally empty field for rejecting automated form submissions.

use dioxus::prelude::*;

/// A bot trap that stays out of the keyboard and accessibility tree.
#[component]
pub fn Honeypot(name: String) -> Element {
    rsx! {
        div { class: "nav-honeypot nav-visually-hidden", "aria-hidden": "true",
            label {
                "Leave this field blank"
                input {
                    name: "{name}",
                    tabindex: "-1",
                    autocomplete: "off",
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hides_the_honeypot_from_people_and_keeps_it_out_of_tab_order() {
        fn app() -> Element {
            rsx! { Honeypot { name: "website".to_string() } }
        }

        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        let html = dioxus_ssr::render(&dom);
        assert!(html.contains("aria-hidden=\"true\""), "{html}");
        assert!(html.contains("tabindex=\"-1\""), "{html}");
        assert!(html.contains("autocomplete=\"off\""), "{html}");
        assert!(!html.contains("aria-describedby"), "{html}");
    }
}
