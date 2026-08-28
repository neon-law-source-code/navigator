//! Compact row-actions cell for an admin/list row — an optional view and edit
//! link beside an optional inline delete form, packed into one group with
//! inline-SVG glyphs.
//!
//! The Dioxus successor to the `views::components::row_actions` builder,
//! framework-free: the icons are inline SVG via the shared [`Icon`] component
//! (no Bootstrap Icons webfont), the chrome is theme buttons (no Bootstrap
//! `.btn`), and the delete is a **native** `POST` form — no HTMX. The
//! builder's inline `onsubmit="return confirm(…)"` prompt is dropped: the strict
//! `script-src` CSP the Dioxus pages carry blocks inline event handlers, so a
//! confirmation belongs in a dialog component, not an inline handler.
//!
//! `row_label` (an email / name / code) is stitched into the ARIA labels so a
//! screen reader hears "Delete libra@example.com", not a bare "Delete"; the icon
//! glyphs are decorative (`aria-hidden` via [`Icon`] with no label), so the
//! button's label is the sole accessible name.

use dioxus::prelude::*;

use crate::components::{Icon, IconName};

/// A row's action cell. Any of the three verbs may be omitted; a `None`
/// `delete_action` renders no form, a `None` `edit_href`/`view_href` no link.
#[component]
pub fn RowActions(
    /// A read-only/detail link (eye glyph).
    #[props(default)]
    view_href: Option<String>,
    /// An edit link (pencil glyph).
    #[props(default)]
    edit_href: Option<String>,
    /// A delete form action (trash glyph). The form is a native `POST`.
    #[props(default)]
    delete_action: Option<String>,
    /// The HTTP method the delete form submits with. Defaults to `post` (the
    /// production mutation); a preview may override it to `get` when it cannot
    /// perform a real mutation.
    #[props(default = "post".to_string())]
    delete_method: String,
    /// The session CSRF token, threaded into the delete form's hidden `_csrf`.
    /// Empty (tests bypassing middleware) omits the field.
    #[props(default)]
    csrf_token: String,
    /// Human-readable row identity (email / name / code) echoed into the ARIA
    /// labels to disambiguate the verbs row-to-row. Empty falls back to the bare
    /// verb.
    #[props(default)]
    row_label: String,
) -> Element {
    let with_label = |verb: &str| {
        if row_label.is_empty() {
            verb.to_string()
        } else {
            format!("{verb} {row_label}")
        }
    };
    let view_aria = if row_label.is_empty() {
        "View details".to_string()
    } else {
        format!("View details for {row_label}")
    };
    let edit_aria = with_label("Edit");
    let delete_aria = with_label("Delete");

    rsx! {
        div { class: "row-actions", role: "group", "aria-label": "Row actions",
            if let Some(href) = view_href {
                a {
                    class: "nav-btn nav-btn--secondary row-action",
                    href: "{href}",
                    "data-action": "view",
                    "aria-label": "{view_aria}",
                    title: "{view_aria}",
                    Icon { name: IconName::Eye }
                }
            }
            if let Some(href) = edit_href {
                a {
                    class: "nav-btn nav-btn--secondary row-action",
                    href: "{href}",
                    "data-action": "edit",
                    "aria-label": "{edit_aria}",
                    title: "{edit_aria}",
                    Icon { name: IconName::PencilSquare }
                }
            }
            if let Some(action) = delete_action {
                form { class: "row-action-form", method: "{delete_method}", action: "{action}",
                    if !csrf_token.is_empty() {
                        input { r#type: "hidden", name: "_csrf", value: "{csrf_token}" }
                    }
                    button {
                        class: "nav-btn nav-btn--danger row-action",
                        r#type: "submit",
                        "data-action": "delete",
                        "aria-label": "{delete_aria}",
                        title: "{delete_aria}",
                        Icon { name: IconName::Trash3Fill }
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

    #[test]
    fn renders_all_three_verbs_packed_in_one_group() {
        fn app() -> Element {
            rsx! {
                RowActions {
                    view_href: "/app/projects/42".to_string(),
                    edit_href: "/app/admin/people/42/edit".to_string(),
                    delete_action: "/app/admin/people/42/delete".to_string(),
                    csrf_token: "TOK".to_string(),
                }
            }
        }
        let html = ssr(app);
        assert!(html.contains(r#"class="row-actions""#), "{html}");
        assert!(html.contains(r#"role="group""#));
        assert!(html.contains(r#"aria-label="Row actions""#));
        assert!(
            html.contains(r#"href="/app/projects/42""#),
            "view link: {html}"
        );
        assert!(
            html.contains(r#"href="/app/admin/people/42/edit""#),
            "edit link: {html}"
        );
        assert!(html.contains(r#"data-action="view""#));
        assert!(html.contains(r#"data-action="edit""#));
        assert!(html.contains(r#"data-action="delete""#));
    }

    #[test]
    fn delete_is_a_native_post_form_with_csrf_and_no_htmx() {
        fn app() -> Element {
            rsx! {
                RowActions {
                    delete_action: "/app/admin/people/42/delete".to_string(),
                    csrf_token: "SESSION_TOKEN".to_string(),
                }
            }
        }
        let html = ssr(app);
        assert!(html.contains(r#"method="post""#), "native POST: {html}");
        assert!(html.contains(r#"action="/app/admin/people/42/delete""#));
        assert!(
            html.contains(r#"name="_csrf""#) && html.contains("SESSION_TOKEN"),
            "csrf: {html}"
        );
        // Framework-free: no HTMX attributes, no inline onsubmit handler.
        assert!(!html.contains("hx-"), "no HTMX: {html}");
        assert!(
            !html.contains("onsubmit"),
            "no inline confirm handler (CSP): {html}"
        );
    }

    #[test]
    fn delete_method_defaults_to_post_and_is_overridable_for_a_preview() {
        fn production() -> Element {
            rsx! { RowActions { delete_action: "/app/admin/people/42/delete".to_string() } }
        }
        // A preview can submit with GET when it must not issue a real mutation.
        fn preview() -> Element {
            rsx! {
                RowActions {
                    delete_action: "/preview".to_string(),
                    delete_method: "get".to_string(),
                }
            }
        }
        assert!(
            ssr(production).contains(r#"method="post""#),
            "production delete stays POST"
        );
        let html = ssr(preview);
        assert!(html.contains(r#"method="get""#), "{html}");
        assert!(html.contains(r#"action="/preview""#), "{html}");
    }

    #[test]
    fn csrf_field_omitted_when_token_empty() {
        fn app() -> Element {
            rsx! { RowActions { delete_action: "/x".to_string() } }
        }
        assert!(!ssr(app).contains(r#"name="_csrf""#));
    }

    #[test]
    fn row_label_disambiguates_aria_labels() {
        fn app() -> Element {
            rsx! {
                RowActions {
                    view_href: "/v".to_string(),
                    edit_href: "/e".to_string(),
                    delete_action: "/d".to_string(),
                    row_label: "libra@example.com".to_string(),
                }
            }
        }
        let html = ssr(app);
        assert!(
            html.contains(r#"aria-label="View details for libra@example.com""#),
            "{html}"
        );
        assert!(
            html.contains(r#"aria-label="Edit libra@example.com""#),
            "{html}"
        );
        assert!(
            html.contains(r#"aria-label="Delete libra@example.com""#),
            "{html}"
        );
    }

    #[test]
    fn empty_row_label_falls_back_to_bare_verb() {
        fn app() -> Element {
            rsx! { RowActions { edit_href: "/e".to_string(), delete_action: "/d".to_string() } }
        }
        let html = ssr(app);
        assert!(html.contains(r#"aria-label="Edit""#), "{html}");
        assert!(html.contains(r#"aria-label="Delete""#), "{html}");
    }

    #[test]
    fn view_only_row_renders_no_edit_link_or_delete_form() {
        fn app() -> Element {
            rsx! { RowActions { view_href: "/x".to_string() } }
        }
        let html = ssr(app);
        assert!(html.contains(r#"href="/x""#));
        assert!(!html.contains(r#"data-action="edit""#), "{html}");
        assert!(!html.contains("<form"), "{html}");
    }

    #[test]
    fn glyphs_are_decorative_so_the_button_label_is_the_accessible_name() {
        fn app() -> Element {
            rsx! {
                RowActions {
                    view_href: "/v".to_string(),
                    edit_href: "/e".to_string(),
                    delete_action: "/d".to_string(),
                }
            }
        }
        // Every glyph is a decorative inline SVG (aria-hidden), so a screen
        // reader announces the button's aria-label once, not the icon too.
        let html = ssr(app);
        assert!(html.matches(r#"aria-hidden="true""#).count() >= 3, "{html}");
    }
}
