//! A collapsible section, as a native `<details>` disclosure — the same
//! no-JS pattern [`crate::catalog_step`]'s jump-to-section menu uses. Reaches
//! for zero JavaScript on purpose: `<details>` opens before hydration and is
//! keyboard-accessible (Enter/Space on the `<summary>`) with no script at
//! all, so a page built from these degrades to plain, readable HTML even if
//! the client bundle never loads.
//!
//! Every call site hand-rolled its own `details`/`summary` before this
//! (`project_new.rs`, `person_show.rs`, `lawyer_project_detail.rs`,
//! `intake_review.rs`); this is the shared building block for a page — like
//! `/notations/{slug}` — that wants several stacked, independently
//! collapsible sections.

use dioxus::prelude::*;

/// One collapsible section. `open` seeds the initial (pre-hydration) state;
/// the browser owns whether it stays open after that, same as any native
/// `<details>`.
#[component]
pub fn Accordion(
    title: String,
    #[props(default = false)] open: bool,
    children: Element,
) -> Element {
    rsx! {
        details { class: "nav-accordion", open,
            summary { class: "nav-accordion__toggle", "{title}" }
            div { class: "nav-accordion__body", {children} }
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
    fn renders_a_native_details_disclosure_with_the_given_title() {
        fn app() -> Element {
            rsx! {
                Accordion { title: "Frontmatter".to_string(), p { "body copy" } }
            }
        }
        let out = ssr(app);
        assert!(out.contains("<details"), "native disclosure: {out}");
        assert!(out.contains("Frontmatter"), "title: {out}");
        assert!(out.contains("body copy"), "children render inside: {out}");
    }

    #[test]
    fn defaults_to_closed() {
        fn app() -> Element {
            rsx! {
                Accordion { title: "Body".to_string(), p { "content" } }
            }
        }
        let out = ssr(app);
        assert!(!out.contains(" open"), "closed by default: {out}");
    }

    #[test]
    fn open_prop_seeds_the_initial_disclosed_state() {
        fn app() -> Element {
            rsx! {
                Accordion { title: "Body".to_string(), open: true, p { "content" } }
            }
        }
        let out = ssr(app);
        assert!(out.contains(" open"), "seeded open: {out}");
    }
}
