//! A destructive-action confirmation card, as a Dioxus component.
//!
//! The Dioxus successor to the `views::components::confirm_delete`. The
//! builder was an inline `<form>` whose "are you sure?" was a browser `confirm()`
//! fired from an `onsubmit` handler — inline JS the Dioxus pages' strict
//! `script-src` CSP forbids. So the confirmation moves out of an inline prompt
//! and onto a dedicated **server-rendered page**: a caller (a
//! `…/:id/delete/confirm` GET handler) renders this card, and only its Confirm
//! button — a native `POST` — performs the delete. No JavaScript, readable
//! pre-hydration, and the destructive `POST` never fires from a stray click.

use dioxus::prelude::*;

/// A confirmation card: the prompt, a native-`POST` Confirm button (danger
/// styling — the destructive action is unmistakable), and a Cancel link back to
/// where the user came from.
#[component]
pub fn ConfirmDelete(
    /// The card heading, e.g. "Delete person". Omitted when empty.
    #[props(default)]
    title: String,
    /// The confirmation prompt, e.g. "Delete libra@example.com? This cannot be
    /// undone."
    message: String,
    /// The action the Confirm button `POST`s to — the real delete endpoint.
    confirm_action: String,
    /// The HTTP method the Confirm form submits with. Defaults to `post` (the
    /// production mutation); a preview may override it to `get` when it cannot
    /// perform a real mutation.
    #[props(default = "post".to_string())]
    confirm_method: String,
    /// The Confirm button label. Defaults to "Delete".
    #[props(default = "Delete".to_string())]
    confirm_label: String,
    /// Where Cancel navigates back to (the list or detail the user came from).
    cancel_href: String,
    /// The session CSRF token, threaded into the Confirm form's hidden `_csrf`.
    /// Empty (tests bypassing middleware) omits the field.
    #[props(default)]
    csrf_token: String,
) -> Element {
    rsx! {
        div { class: "nav-card confirm-delete", role: "alertdialog", "aria-label": "Confirm deletion",
            div { class: "nav-card__body",
                if !title.is_empty() {
                    h2 { class: "confirm-delete__title", "{title}" }
                }
                p { class: "confirm-delete__message", "{message}" }
                div { class: "confirm-delete__actions",
                    form { class: "confirm-delete__form", method: "{confirm_method}", action: "{confirm_action}",
                        if !csrf_token.is_empty() {
                            input { r#type: "hidden", name: "_csrf", value: "{csrf_token}" }
                        }
                        button { class: "nav-btn nav-btn--danger", r#type: "submit", "{confirm_label}" }
                    }
                    a { class: "nav-btn nav-btn--secondary", href: "{cancel_href}", "Cancel" }
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
    fn confirm_is_a_native_post_to_the_delete_action_with_csrf() {
        fn app() -> Element {
            rsx! {
                ConfirmDelete {
                    title: "Delete person".to_string(),
                    message: "Delete libra@example.com? This cannot be undone.".to_string(),
                    confirm_action: "/app/admin/people/42/delete".to_string(),
                    cancel_href: "/app/admin/people".to_string(),
                    csrf_token: "SESSION_TOKEN".to_string(),
                }
            }
        }
        let html = ssr(app);
        assert!(html.contains("Delete libra@example.com?"), "{html}");
        assert!(html.contains(r#"method="post""#), "native POST: {html}");
        assert!(
            html.contains(r#"action="/app/admin/people/42/delete""#),
            "{html}"
        );
        assert!(
            html.contains(r#"name="_csrf""#) && html.contains("SESSION_TOKEN"),
            "csrf threaded: {html}"
        );
        // No inline JS — the confirmation is the page itself, not an onsubmit.
        assert!(
            !html.contains("onsubmit"),
            "no inline confirm handler: {html}"
        );
        assert!(!html.contains("hx-"), "no HTMX: {html}");
    }

    #[test]
    fn cancel_links_back_and_defaults_the_confirm_label() {
        fn app() -> Element {
            rsx! {
                ConfirmDelete {
                    message: "Are you sure?".to_string(),
                    confirm_action: "/x/delete".to_string(),
                    cancel_href: "/x".to_string(),
                }
            }
        }
        let html = ssr(app);
        assert!(
            html.contains(r#"href="/x""#) && html.contains(">Cancel</a>"),
            "cancel link: {html}"
        );
        // The default danger label.
        assert!(html.contains(">Delete</button>"), "default label: {html}");
        // No CSRF field when no token (a test/no-session render).
        assert!(!html.contains(r#"name="_csrf""#), "{html}");
    }

    #[test]
    fn confirm_method_defaults_to_post_and_is_overridable_for_a_preview() {
        fn production() -> Element {
            rsx! {
                ConfirmDelete {
                    message: "Are you sure?".to_string(),
                    confirm_action: "/app/admin/people/42/delete".to_string(),
                    cancel_href: "/app/admin/people".to_string(),
                }
            }
        }
        // A preview can submit with GET when it must not issue a real mutation.
        fn preview() -> Element {
            rsx! {
                ConfirmDelete {
                    message: "Are you sure?".to_string(),
                    confirm_action: "/preview".to_string(),
                    confirm_method: "get".to_string(),
                    cancel_href: "/preview".to_string(),
                }
            }
        }
        assert!(
            ssr(production).contains(r#"method="post""#),
            "production confirm stays POST"
        );
        let html = ssr(preview);
        assert!(html.contains(r#"method="get""#), "{html}");
        assert!(html.contains(r#"action="/preview""#), "{html}");
    }

    #[test]
    fn confirm_label_is_overridable() {
        fn app() -> Element {
            rsx! {
                ConfirmDelete {
                    message: "Remove Libra from this matter?".to_string(),
                    confirm_action: "/app/projects/1/people/2/delete".to_string(),
                    confirm_label: "Remove".to_string(),
                    cancel_href: "/app/projects/1".to_string(),
                }
            }
        }
        let html = ssr(app);
        assert!(html.contains(">Remove</button>"), "{html}");
        assert!(!html.contains(">Delete</button>"), "{html}");
    }
}
