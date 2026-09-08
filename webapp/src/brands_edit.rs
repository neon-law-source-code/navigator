//! `/app/brands/{key}/edit` — closed typeface and palette selects.
//!
//! Owner edits a system-wide brand; a Firm's Admin DRI edits that Firm's
//! brands. The native form posts to this same path; `PATCH /app/api/brands/{key}`
//! is the JSON twin and refuses any typeface or palette outside the catalogs.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Choice, Field, FormCard, Heading};
use crate::entity_new::FormChoice;
use crate::people::ViewerRole;

/// Everything the edit page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct BrandsEditView {
    #[serde(default)]
    pub tokens_href: String,
    #[serde(default)]
    pub firm_name: String,
    pub role: ViewerRole,
    #[serde(default)]
    pub logo: Option<crate::components::AppLogo>,
    pub key: String,
    pub fields: Option<BrandPresentationFields>,
    pub typefaces: Vec<FormChoice>,
    pub palettes: Vec<FormChoice>,
    pub csrf_token: String,
    #[serde(default)]
    pub error: Option<String>,
}

/// Prefill for the two selects.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct BrandPresentationFields {
    pub name: String,
    pub typeface: String,
    pub palette: String,
}

#[derive(Deserialize, Default)]
pub struct BrandsEditQuery {
    #[serde(default)]
    pub error: Option<String>,
}

/// Load the edit form for `{key}`.
#[server]
pub async fn get_brands_edit() -> Result<BrandsEditView, ServerFnError> {
    let role = crate::admin_listing::require_admin().await?;
    let axum::extract::Path(key) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Path<String>, _>()
            .await?;
    let axum::extract::Query(query) = dioxus_fullstack_core::FullstackContext::extract::<
        axum::extract::Query<BrandsEditQuery>,
        _,
    >()
    .await?;
    let csrf_token = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::csrf::CsrfToken>,
        _,
    >()
    .await
    .map(|axum::Extension(token)| token.0)
    .unwrap_or_default();

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let actor_person_id = crate::admin_listing::injected_person_id().await;

    let typefaces = views::brand::TYPEFACES
        .iter()
        .map(|face| FormChoice {
            value: face.id.to_string(),
            label: face.label.to_string(),
        })
        .collect();
    let palettes = views::brand::PALETTE
        .iter()
        .map(|palette| FormChoice {
            value: palette.id.to_string(),
            label: palette.label.to_string(),
        })
        .collect();

    let base = BrandsEditView {
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
        firm_name: crate::app_chrome::firm_name_from_context().await,
        logo: crate::app_chrome::app_logo_from_context().await,
        role,
        key: key.clone(),
        fields: None,
        typefaces,
        palettes,
        csrf_token,
        error: query.error.as_deref().map(flash_message),
    };

    let Some(brand) = store::brands::find_by_key(&surreal, &key)
        .await
        .map_err(|error| ServerFnError::new(error.to_string()))?
    else {
        dioxus_fullstack_core::FullstackContext::commit_http_status(
            axum::http::StatusCode::NOT_FOUND,
            None,
        );
        return Ok(base);
    };

    match store::brands::update(
        &surreal,
        store_role(role),
        actor_person_id,
        brand.id,
        &store::brands::BrandEdit::default(),
    )
    .await
    {
        Ok(_) => {}
        Err(store::brands::BrandError::NotAuthorized) => {
            dioxus_fullstack_core::FullstackContext::commit_http_status(
                axum::http::StatusCode::NOT_FOUND,
                None,
            );
            return Ok(base);
        }
        Err(error) => return Err(ServerFnError::new(error.to_string())),
    }

    let compiled = views::brand::BrandKey::parse(&key);
    let (face, palette) = views::brand::resolve_presentation(
        brand.typeface.as_deref(),
        brand.primary_color.as_deref(),
        compiled,
    )
    .map_or_else(
        || ("gorp-serif".to_string(), "neon-teal".to_string()),
        |(face, palette)| (face.id.to_string(), palette.id.to_string()),
    );

    Ok(BrandsEditView {
        fields: Some(BrandPresentationFields {
            name: brand.name,
            typeface: face,
            palette,
        }),
        ..base
    })
}

#[cfg(feature = "server")]
fn flash_message(code: &str) -> String {
    match code {
        "unknown-choice" => {
            "Typeface and palette must be chosen from the closed lists.".to_string()
        }
        other => other.to_string(),
    }
}

#[cfg(feature = "server")]
fn store_role(role: ViewerRole) -> store::persons::Role {
    match role {
        ViewerRole::Owner => store::persons::Role::Owner,
        ViewerRole::Admin => store::persons::Role::Admin,
        ViewerRole::Lawyer => store::persons::Role::Lawyer,
        ViewerRole::Clerk => store::persons::Role::Clerk,
        ViewerRole::Client => store::persons::Role::Client,
    }
}

/// Route entry for `/app/brands/{key}/edit`.
#[component]
pub fn BrandsEdit() -> Element {
    let resource = use_server_future(get_brands_edit)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "brands-edit", p { "Failed to load the brand." } }
            }
        }
        None => {
            return rsx! {
                main { id: "brands-edit", p { "Loading…" } }
            }
        }
    };
    brands_edit_body(&view)
}

/// Split from the component so tests render a fixed view.
pub fn brands_edit_body(view: &BrandsEditView) -> Element {
    let role = view.role;
    let firm_name = view.firm_name.clone();
    rsx! {
        document::Title { "{firm_name} | Brands | Edit" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        crate::components::AppNavbar {
            destinations: crate::app_chrome::app_destinations(role),
            logo: view.logo.clone(),
        }
        main { id: "brands-edit", class: "nav-theme",
            match &view.fields {
                Some(fields) => {
                    let type_opts: Vec<Choice> = view
                        .typefaces
                        .iter()
                        .map(|choice| Choice::new(choice.value.clone(), choice.label.clone()))
                        .collect();
                    let palette_opts: Vec<Choice> = view
                        .palettes
                        .iter()
                        .map(|choice| Choice::new(choice.value.clone(), choice.label.clone()))
                        .collect();
                    let form_fields = vec![
                        Field::select(
                            "Typeface",
                            "typeface",
                            type_opts,
                            Some(fields.typeface.clone()),
                        )
                        .required(),
                        Field::select(
                            "Palette",
                            "palette",
                            palette_opts,
                            Some(fields.palette.clone()),
                        )
                        .required(),
                    ];
                    rsx! {
                        if let Some(error) = &view.error {
                            p { class: "nav-form-error", role: "alert", "{error}" }
                        }
                        FormCard {
                            title: format!("Edit {}", fields.name),
                            action: format!("/app/brands/{}/edit", view.key),
                            submit_label: "Save presentation".to_string(),
                            heading: Heading::H1,
                            csrf_token: Some(view.csrf_token.clone()),
                            fields: form_fields,
                        }
                        p { a { href: "/app/brands", "← Brands" } }
                    }
                }
                None => rsx! {
                    h1 { "Brand not found" }
                    p { "No brand exists with key " code { "{view.key}" } "." }
                    p { a { href: "/app/brands", "← Brands" } }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::form::assert_forms_accessible;

    fn view(fields: Option<BrandPresentationFields>) -> BrandsEditView {
        BrandsEditView {
            tokens_href: String::new(),
            firm_name: "Neon Law".to_string(),
            role: ViewerRole::Owner,
            logo: None,
            key: "neon".to_string(),
            fields,
            typefaces: vec![
                FormChoice {
                    value: "gorp-serif".to_string(),
                    label: "GORP Serif".to_string(),
                },
                FormChoice {
                    value: "tinos".to_string(),
                    label: "Tinos".to_string(),
                },
            ],
            palettes: vec![FormChoice {
                value: "neon-teal".to_string(),
                label: "Neon teal".to_string(),
            }],
            csrf_token: "TOK".to_string(),
            error: None,
        }
    }

    #[test]
    fn the_edit_form_is_two_selects_and_posts_to_the_key() {
        let html =
            dioxus_ssr::render_element(brands_edit_body(&view(Some(BrandPresentationFields {
                name: "Neon Law".to_string(),
                typeface: "gorp-serif".to_string(),
                palette: "neon-teal".to_string(),
            }))));
        assert_forms_accessible(&html, "brand presentation");
        assert!(html.contains(r#"action="/app/brands/neon/edit""#), "{html}");
        assert!(html.contains(r#"name="typeface""#), "{html}");
        assert!(html.contains(r#"name="palette""#), "{html}");
        assert!(!html.contains("textarea"), "{html}");
        assert!(!html.contains(r#"type="text""#), "{html}");
    }
}
