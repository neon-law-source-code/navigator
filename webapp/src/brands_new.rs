//! `/app/admin/brands/new` — create a brand row (ENG-586, ENG-659).
//!
//! Every brand is Firm-scoped: only a Firm's own Admin DRI may create one,
//! pinned to that Firm automatically rather than offered as a picker, since
//! an Admin may only ever create for the one Firm they are the DRI of.
//! Owner holds no Firm membership at all, so Owner sees the same disabled
//! explanation an Admin who is DRI of no Firm does — Owner no longer
//! creates a system-wide row through this CRUD; `store::brands::create`
//! refuses `firm_id: None` outright, for every actor.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Choice, Field, FormCard, Heading};
use crate::people::ViewerRole;

/// The `?error=` flash and the echoed fields a refused create carries back.
#[derive(Deserialize, Serialize, Clone, Default, PartialEq, Eq)]
pub struct BrandNewQuery {
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub typeface: Option<String>,
    #[serde(default)]
    pub primary_color: Option<String>,
}

/// Everything the create page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct BrandNewView {
    /// `false` for a non-owner-non-admin caller — the page renders not-found
    /// under a committed `403`.
    pub found: bool,
    pub role: ViewerRole,
    /// `Some(name)` when an Admin viewer is the DRI of exactly one Firm —
    /// the Firm this create pins to, named for display only (the id is
    /// resolved again server-side on submit, never trusted from the form).
    pub firm_name: Option<String>,
    /// `true` when an Admin viewer is DRI of no Firm, so the form renders
    /// disabled with an explanation instead of a create button that would
    /// only ever be refused.
    pub admin_has_no_firm: bool,
    pub typefaces: Vec<crate::entity_new::FormChoice>,
    pub csrf_token: String,
    pub query: BrandNewQuery,
    #[serde(default)]
    pub logo: Option<crate::components::AppLogo>,
    #[serde(default)]
    pub tokens_href: String,
    #[serde(default)]
    pub site_name: String,
}

/// Load the create-brand form.
#[server]
pub async fn get_brand_new_form() -> Result<BrandNewView, ServerFnError> {
    let role = crate::admin_listing::require_admin().await?;
    let csrf_token = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::csrf::CsrfToken>,
        _,
    >()
    .await
    .map(|axum::Extension(token)| token.0)
    .unwrap_or_default();
    let query =
        dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Query<BrandNewQuery>, _>(
        )
        .await
        .map(|axum::extract::Query(q)| q)
        .unwrap_or_default();

    let typefaces = views::brand::TYPEFACES
        .iter()
        .map(|face| crate::entity_new::FormChoice {
            value: face.id.to_string(),
            label: face.label.to_string(),
        })
        .collect();

    let base = BrandNewView {
        found: true,
        role,
        firm_name: None,
        admin_has_no_firm: false,
        typefaces,
        csrf_token,
        query,
        logo: crate::app_chrome::app_logo_from_context().await,
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
        site_name: crate::app_chrome::firm_name_from_context().await,
    };

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let Some(person_id) = crate::admin_listing::injected_person_id().await else {
        return Ok(BrandNewView {
            admin_has_no_firm: true,
            ..base
        });
    };
    let memberships = store::firms::memberships_for_person(&surreal, person_id)
        .await
        .map_err(|error| ServerFnError::new(error.to_string()))?;
    let dri_firm_id = memberships
        .iter()
        .find(|m| m.is_dri && m.membership == store::firms::FirmMembership::Admin)
        .map(|m| m.firm_id);
    let Some(firm_id) = dri_firm_id else {
        return Ok(BrandNewView {
            admin_has_no_firm: true,
            ..base
        });
    };
    let firm_name = store::firms::find_by_id(&surreal, firm_id)
        .await
        .map_err(|error| ServerFnError::new(error.to_string()))?
        .map(|firm| firm.name);

    Ok(BrandNewView { firm_name, ..base })
}

fn echoed(value: Option<&String>) -> String {
    value.cloned().unwrap_or_default()
}

fn new_body(view: &BrandNewView) -> Element {
    let q = &view.query;
    let type_opts: Vec<Choice> = view
        .typefaces
        .iter()
        .map(|choice| Choice::new(choice.value.clone(), choice.label.clone()))
        .collect();

    if view.admin_has_no_firm {
        return rsx! {
            h1 { "Create brand" }
            p { class: "nav-form-error", role: "alert",
                "You are not the Admin DRI of any Firm, so you may not create a brand. Only a \
                 Firm's own Admin DRI creates its brand — ask that Firm's Owner to appoint you \
                 first."
            }
            p { a { href: "/app/admin/brands", "← Brands" } }
        };
    }

    // Every brand is Firm-scoped (ENG-659) — `admin_has_no_firm` above is
    // the only refusal, so reaching here always means a real DRI Firm name.
    let scope_note = view.firm_name.as_ref().map_or_else(String::new, |name| {
        format!("This brand will be scoped to {name}.")
    });

    let fields = vec![
        Field::text("Name", "name", echoed(q.name.as_ref())).required(),
        Field::text("Key", "key", echoed(q.key.as_ref()))
            .required()
            .placeholder("acme-brand")
            .help("Lowercase letters, digits, and single hyphens. Chosen once; not editable here."),
        Field::select(
            "Typeface",
            "typeface",
            type_opts,
            q.typeface.clone().filter(|v| !v.is_empty()),
        )
        .required(),
        Field::text(
            "Primary colour",
            "primary_color",
            echoed(q.primary_color.as_ref()),
        )
        .required()
        .placeholder("#007c91")
        .help(
            "A #rrggbb hex. Its on-primary text (white or black, whichever contrasts more) must \
             clear WCAG AA 4.5:1, and it must clear 3:1 against the light page surface.",
        ),
    ];

    rsx! {
        header { class: "page-header",
            h1 { "Create brand" }
            p { class: "page-subtitle", "{scope_note}" }
            p { a { href: "/app/admin/brands", "← Back to brands" } }
        }
        if let Some(error) = q.error.as_ref() {
            p { class: "nav-form-error", role: "alert", "{error}" }
        }
        FormCard {
            title: "Create brand".to_string(),
            action: "/app/admin/brands/new".to_string(),
            submit_label: "Create".to_string(),
            heading: Heading::H2,
            csrf_token: Some(view.csrf_token.clone()),
            fields,
        }
    }
}

/// `/app/admin/brands/new`.
#[component]
pub fn BrandNew() -> Element {
    let resource = use_server_future(get_brand_new_form)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "brand-new", p { "Failed to load the form." } }
            }
        }
        None => {
            return rsx! {
                main { id: "brand-new", p { "Loading…" } }
            }
        }
    };

    rsx! {
        document::Title { "{view.site_name} | Brands | Create" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        crate::components::AppNavbar {
            destinations: crate::app_chrome::app_destinations(view.role),
            logo: view.logo.clone(),
        }
        main { id: "brand-new", class: "nav-theme",
            {new_body(&view)}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{new_body, BrandNewQuery, BrandNewView};
    use crate::entity_new::FormChoice;
    use crate::people::ViewerRole;

    fn view(query: BrandNewQuery) -> BrandNewView {
        BrandNewView {
            found: true,
            role: ViewerRole::Admin,
            firm_name: Some("Acme Practice".to_string()),
            admin_has_no_firm: false,
            typefaces: vec![FormChoice {
                value: "gorp-serif".to_string(),
                label: "GORP Serif".to_string(),
            }],
            csrf_token: "TOK".to_string(),
            query,
            logo: None,
            tokens_href: String::new(),
            site_name: "Neon Law".to_string(),
        }
    }

    fn render(view: &BrandNewView) -> String {
        dioxus_ssr::render_element(new_body(view))
    }

    #[test]
    fn an_admin_dri_sees_their_firm_pinned_and_the_form_posts_to_the_collection() {
        let html = render(&view(BrandNewQuery::default()));
        assert!(html.contains("scoped to Acme Practice"), "{html}");
        assert!(html.contains(r#"action="/app/admin/brands/new""#), "{html}");
        assert!(html.contains(r#"name="name""#), "{html}");
        assert!(html.contains(r#"name="key""#), "{html}");
        assert!(html.contains(r#"name="primary_color""#), "{html}");
        assert!(!html.contains(r#"name="firm_id""#), "{html}");
    }

    /// ENG-659: every brand is Firm-scoped now, and Owner holds no Firm
    /// membership at all, so Owner sees the same disabled explanation a
    /// DRI-less Admin does — there is no more system-wide creation path.
    #[test]
    fn an_owner_sees_the_same_disabled_explanation_as_a_dri_less_admin() {
        let mut view = view(BrandNewQuery::default());
        view.role = ViewerRole::Owner;
        view.firm_name = None;
        view.admin_has_no_firm = true;
        let html = render(&view);
        assert!(html.contains("not the Admin DRI of any Firm"), "{html}");
        assert!(!html.contains(r#"name="name""#), "{html}");
    }

    #[test]
    fn an_admin_with_no_dri_firm_sees_a_disabled_explanation_not_a_form() {
        let mut view = view(BrandNewQuery::default());
        view.firm_name = None;
        view.admin_has_no_firm = true;
        let html = render(&view);
        assert!(html.contains("not the Admin DRI of any Firm"), "{html}");
        assert!(!html.contains(r#"name="name""#), "{html}");
    }

    #[test]
    fn a_refusal_names_the_rule_and_echoes_the_typed_fields() {
        let html = render(&view(BrandNewQuery {
            error: Some("That brand key is already taken.".to_string()),
            name: Some("Acme Brand".to_string()),
            key: Some("acme-brand".to_string()),
            ..BrandNewQuery::default()
        }));
        assert!(html.contains("That brand key is already taken."), "{html}");
        assert!(html.contains(r#"value="Acme Brand""#), "{html}");
        assert!(html.contains(r#"value="acme-brand""#), "{html}");
    }
}
