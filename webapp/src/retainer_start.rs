//! The "start a retainer walk" form as a Dioxus component (#956 Phase 4) — the
//! lawyer on-ramp that opens a matter.
//!
//! The successor to the `views::pages::admin::retainers::start_walk`. It
//! lists the seeded `onboarding__*` templates as the picker's options, reads the
//! session CSRF token, and renders the shared [`crate::components::FormCard`] as
//! a native `POST` to `/lawyer/retainers/new` — the existing create handler,
//! unchanged.
//!
//! A refused start (a client email with no `@`, no template chosen, an unknown
//! template) used to re-render this form inline from the `POST`. It now
//! redirects back here with the reason as `?error=` and the submitted values
//! echoed, so nothing is retyped — the post/redirect/get convention the other
//! migrated create forms already use.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Choice, Field, FormCard};
use crate::people::ViewerRole;

/// The template picker's `?error=` flash and the echoed submitted values a
/// refused start redirects back with.
#[derive(Deserialize, Default)]
pub struct RetainerStartQuery {
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub client_email: Option<String>,
    #[serde(default)]
    pub retainer_template_code: Option<String>,
}

/// One onboarding-template option: the submitted `code` and its display label.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct TemplateChoice {
    pub code: String,
    pub label: String,
}

/// The rendered start form: the onboarding-template options, the echoed values
/// and refusal flash from a rejected submit, the session CSRF token, and the
/// viewer's tier.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct RetainerStartView {
    pub templates: Vec<TemplateChoice>,
    pub client_email: String,
    pub retainer_template_code: String,
    pub csrf_token: String,
    pub role: ViewerRole,
    #[serde(default)]
    pub error: Option<String>,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// Load the start form: refuse non-lawyer, read the injected CSRF token and the
/// echoed `?error=` / values, and list the seeded onboarding templates as
/// `(code, label)` options sorted by label.
///
/// Restricting the picker to the `onboarding__*` family is what makes "opening a
/// matter" always start it with a retainer-type notation.
#[server]
pub async fn get_retainer_start_form() -> Result<RetainerStartView, ServerFnError> {
    let role = crate::admin_listing::require_lawyer().await?;
    let csrf_token = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::csrf::CsrfToken>,
        _,
    >()
    .await
    .map(|axum::Extension(token)| token.0)
    .unwrap_or_default();
    let axum::extract::Query(query) = dioxus_fullstack_core::FullstackContext::extract::<
        axum::extract::Query<RetainerStartQuery>,
        _,
    >()
    .await?;

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let mut templates: Vec<TemplateChoice> = store::templates::list_current(&surreal)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .into_iter()
        .filter(|t| t.project_id.is_none() && t.code.starts_with("onboarding__"))
        .map(|t| TemplateChoice {
            label: format!("{} — {}", t.title, t.code),
            code: t.code,
        })
        .collect();
    templates.sort_by(|a, b| a.label.cmp(&b.label));

    Ok(RetainerStartView {
        firm_name: crate::app_chrome::firm_name_from_context().await,
        templates,
        client_email: query.client_email.unwrap_or_default(),
        retainer_template_code: query.retainer_template_code.unwrap_or_default(),
        csrf_token,
        role,
        error: query.error.filter(|message| !message.is_empty()),
    })
}

/// The lawyer "start a retainer walk" form. Server-side rendered as a native
/// `POST` form to `/lawyer/retainers/new` carrying the CSRF token, so it works
/// without JavaScript.
#[component]
pub fn LawyerRetainerStart() -> Element {
    let resource = use_server_future(get_retainer_start_form)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "retainer-new", p { "Failed to load the form." } }
            }
        }
        None => {
            return rsx! {
                main { id: "retainer-new", p { "Loading…" } }
            }
        }
    };

    start_body(&view)
}

/// The loaded form: the nav chrome, the refusal flash, the intro, and the native
/// `POST` card. Split from the component so the tests render a fixed view
/// without standing up the server function.
fn start_body(view: &RetainerStartView) -> Element {
    let role = view.role;
    // A matter opens on an onboarding template; the dropdown is the canonical
    // picker so lawyers choose by title rather than typing a code. Default the
    // selection to the generic retainer.
    let selected = if view.retainer_template_code.is_empty() {
        "onboarding__letter".to_string()
    } else {
        view.retainer_template_code.clone()
    };
    let options: Vec<Choice> = view
        .templates
        .iter()
        .map(|t| Choice::new(t.code.clone(), t.label.clone()))
        .collect();
    let fields = vec![
        Field::email("Client email", "client_email", &view.client_email)
            .required()
            .placeholder("libra@example.com"),
        Field::select(
            "Onboarding template",
            "retainer_template_code",
            options,
            Some(selected),
        )
        .required()
        .help("Every matter opens on an onboarding template. Pick any one and edit it for this client."),
    ];

    rsx! {
        document::Title { "{view.firm_name} | Lawyer | Retainers | New" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        nav { class: "lawyer-nav",
            a { class: "nav-link", href: "/app/projects", "Portal" }
            if role.is_lawyer_tier() {
                a { class: "nav-link", href: "/lawyer", "Lawyer" }
            }
            if role.is_admin_tier() {
                a { class: "nav-link", href: "/app/admin", "Admin" }
            }
            a { class: "nav-link", href: "/auth/logout", "Sign out" }
        }
        main { id: "retainer-new", class: "nav-theme",
            if let Some(error) = view.error.as_ref() {
                p { class: "nav-form-error", role: "alert", "{error}" }
            }
            p { class: "nav-muted",
                "Creates a Notation and walks the questionnaire one question at a "
                "time. Once the questionnaire reaches "
                code { "END" }
                ", the retainer-intake workflow takes over (intake → render → "
                "signature)."
            }
            FormCard {
                title: "New retainer".to_string(),
                action: "/lawyer/retainers/new".to_string(),
                submit_label: "Start walk".to_string(),
                csrf_token: Some(view.csrf_token.clone()),
                fields,
            }
            p { a { href: "/lawyer", "← Cancel" } }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::form::assert_forms_accessible;

    fn view(error: Option<&str>) -> RetainerStartView {
        RetainerStartView {
            firm_name: "Neon Law".to_string(),
            templates: vec![TemplateChoice {
                code: "onboarding__letter".to_string(),
                label: "Onboarding Letter — onboarding__letter".to_string(),
            }],
            client_email: "libra@example.com".to_string(),
            retainer_template_code: "onboarding__letter".to_string(),
            csrf_token: "tok".to_string(),
            role: ViewerRole::Lawyer,
            error: error.map(str::to_string),
        }
    }

    /// Render the page body for a fixed view, bypassing the server function.
    fn render_start(view: &RetainerStartView) -> String {
        dioxus_ssr::render_element(start_body(view))
    }

    /// The picker offers every `onboarding__*` template and refuses no
    /// pairing. Lawyers choose the engagement agreement and edit it per
    /// client; nothing narrows the list to a matter's service.
    #[test]
    fn the_picker_offers_every_onboarding_template_and_refuses_none() {
        let codes = [
            "onboarding__letter",
            "onboarding__letter_transcript",
        ];
        let mut view = view(None);
        view.templates = codes
            .iter()
            .map(|code| TemplateChoice {
                code: (*code).to_string(),
                label: format!("Label — {code}"),
            })
            .collect();
        // Selecting one that shares no service with the others must still
        // render — the old product/retainer pairing rule is gone.
        view.retainer_template_code = "onboarding__letter_transcript".to_string();

        let html = render_start(&view);
        for code in codes {
            assert!(
                html.contains(&format!("value=\"{code}\"")),
                "`{code}` must be offered; the picker narrows by nothing\n{html}"
            );
        }
        assert!(
            !html.contains("does not match the selected service"),
            "no pairing is refused\n{html}"
        );
    }

    #[test]
    fn the_form_keeps_the_browser_e2e_field_names() {
        // `server/tests/browser_e2e.rs` drives this form by
        // `input[name='client_email']` and `select[name='retainer_template_code']`,
        // and `accessibility_e2e.rs` scopes axe to `form.admin-form`. Losing any
        // of the three times the nightly deploy gate out.
        let html = render_start(&view(None));
        assert!(html.contains("name=\"client_email\""), "{html}");
        assert!(html.contains("name=\"retainer_template_code\""), "{html}");
        assert!(html.contains("admin-form"), "{html}");
        assert!(html.contains("action=\"/lawyer/retainers/new\""), "{html}");
        assert_forms_accessible(&html, "retainer_start::LawyerRetainerStart");
    }

    #[test]
    fn a_refused_start_renders_the_flash_over_the_echoed_values() {
        // The `POST` refusals redirect back here rather than re-rendering, so
        // the reason and the submitted values have to survive the round trip.
        let html = render_start(&view(Some("client email needs an @")));
        assert!(html.contains("nav-form-error"), "{html}");
        assert!(html.contains(">client email needs an @<"), "{html}");
        assert!(html.contains("value=\"libra@example.com\""), "{html}");
        // The echoed template stays selected, not the default.
        let selected_at = html.find("selected").expect("a chosen option: {html}");
        let letter_at = html
            .find("onboarding__letter")
            .expect("the echoed option renders");
        assert!(letter_at < selected_at, "{html}");
    }
}
