//! `/app/brands/{key}/edit` — a brand's presentation: name, typeface, a free
//! hex primary colour behind a WCAG AA contrast gate, an uploaded logo, and
//! an uploaded font.
//!
//! Owner edits a system-wide brand; a Firm's Admin DRI edits that Firm's own
//! brands. The presentation form (name/typeface/colour/font-family) posts to
//! this same path natively; `PATCH /app/api/brands/{key}` is its JSON twin.
//! The logo and font uploads are two further native multipart forms on this
//! page, posting to their own paths — uploads are never JSON.

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
    pub font_licences: Vec<FormChoice>,
    pub csrf_token: String,
    #[serde(default)]
    pub error: Option<String>,
}

/// Prefill for the presentation form and the read-only upload status.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct BrandPresentationFields {
    pub name: String,
    pub typeface: String,
    pub primary_color: String,
    pub font_family: String,
    /// The uploaded logo's serving URL, when one exists.
    pub logo_url: Option<String>,
    /// The uploaded font's serving URL and its attested licence, when one
    /// exists.
    pub font: Option<(String, String)>,
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
        .chain(std::iter::once(FormChoice {
            value: "uploaded".to_string(),
            label: "Uploaded font".to_string(),
        }))
        .collect();
    let font_licences = store::brands::FONT_LICENCES
        .iter()
        .map(|licence| FormChoice {
            value: (*licence).to_string(),
            label: (*licence).to_string(),
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
        font_licences,
        csrf_token,
        error: query.error.clone(),
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

    let logo_url = brand
        .logo_object_key
        .as_deref()
        .map(|object_key| format!("/assets/{object_key}"));
    let font = brand
        .font_object_key
        .as_deref()
        .zip(brand.font_licence.as_deref())
        .map(|(object_key, licence)| (format!("/assets/{object_key}"), licence.to_string()));

    Ok(BrandsEditView {
        fields: Some(BrandPresentationFields {
            name: brand.name,
            typeface: brand.typeface.unwrap_or_default(),
            primary_color: brand.primary_color.unwrap_or_default(),
            font_family: brand.font_family.unwrap_or_default(),
            logo_url,
            font,
        }),
        ..base
    })
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

fn presentation_form(view: &BrandsEditView, fields: &BrandPresentationFields) -> Element {
    let type_opts: Vec<Choice> = view
        .typefaces
        .iter()
        .map(|choice| Choice::new(choice.value.clone(), choice.label.clone()))
        .collect();
    let form_fields = vec![
        Field::text("Name", "name", fields.name.clone()).required(),
        Field::select(
            "Typeface",
            "typeface",
            type_opts,
            Some(fields.typeface.clone()),
        )
        .required(),
        Field::text(
            "Primary colour",
            "primary_color",
            fields.primary_color.clone(),
        )
        .required()
        .placeholder("#007c91")
        .help(
            "A #rrggbb hex. Its best on-primary contrast (white or black) must clear WCAG AA \
                 4.5:1, or the save is refused with the ratio.",
        ),
        Field::text("Font family", "font_family", fields.font_family.clone()).help(
            "The CSS font-family name for an uploaded font. Only used when Typeface is \
             \"Uploaded font\".",
        ),
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
    }
}

fn logo_form(view: &BrandsEditView, fields: &BrandPresentationFields) -> Element {
    rsx! {
        section { id: "brand-logo",
            h2 { "Logo" }
            if let Some(url) = &fields.logo_url {
                p { img { class: "brand-logo-preview", src: "{url}", alt: "Current logo" } }
            } else {
                p { class: "muted", "No logo uploaded — the compiled default renders instead." }
            }
            FormCard {
                title: "Upload logo".to_string(),
                action: format!("/app/brands/{}/logo", view.key),
                submit_label: "Upload".to_string(),
                heading: Heading::H2,
                multipart: true,
                csrf_token: Some(view.csrf_token.clone()),
                fields: vec![
                    Field::file("Logo file", "file").required().help(
                        "PNG or SVG, at most 512 KB. An SVG containing a script, an event \
                         handler, a foreignObject, or an external reference is refused.",
                    ),
                ],
            }
        }
    }
}

fn font_form(view: &BrandsEditView, fields: &BrandPresentationFields) -> Element {
    let licence_opts: Vec<Choice> = view
        .font_licences
        .iter()
        .map(|choice| Choice::new(choice.value.clone(), choice.label.clone()))
        .collect();
    rsx! {
        section { id: "brand-font",
            h2 { "Font" }
            if let Some((url, licence)) = &fields.font {
                p {
                    "Uploaded font: "
                    a { href: "{url}", "download" }
                    " (" {licence.clone()} ")"
                }
            } else {
                p { class: "muted", "No font uploaded." }
            }
            FormCard {
                title: "Upload font".to_string(),
                action: format!("/app/brands/{}/font", view.key),
                submit_label: "Upload".to_string(),
                heading: Heading::H2,
                multipart: true,
                csrf_token: Some(view.csrf_token.clone()),
                fields: vec![
                    Field::text("Font family", "family", String::new())
                        .required()
                        .help("The CSS font-family name this upload will render under."),
                    Field::select("Licence", "licence", licence_opts, None).required(),
                    Field::file("Font file", "file")
                        .required()
                        .help(".woff2 only, at most 2 MB."),
                ],
            }
        }
    }
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
                Some(fields) => rsx! {
                    {presentation_form(view, fields)}
                    {logo_form(view, fields)}
                    {font_form(view, fields)}
                    p { a { href: "/app/brands", "← Brands" } }
                },
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
                    value: "uploaded".to_string(),
                    label: "Uploaded font".to_string(),
                },
            ],
            font_licences: vec![FormChoice {
                value: "OFL-1.1".to_string(),
                label: "OFL-1.1".to_string(),
            }],
            csrf_token: "TOK".to_string(),
            error: None,
        }
    }

    fn fields() -> BrandPresentationFields {
        BrandPresentationFields {
            name: "Neon Law".to_string(),
            typeface: "gorp-serif".to_string(),
            primary_color: "#007c91".to_string(),
            font_family: String::new(),
            logo_url: None,
            font: None,
        }
    }

    #[test]
    fn the_presentation_form_is_a_hex_input_and_posts_to_the_key() {
        let html = dioxus_ssr::render_element(brands_edit_body(&view(Some(fields()))));
        assert_forms_accessible(&html, "brand presentation");
        assert!(html.contains(r#"action="/app/brands/neon/edit""#), "{html}");
        assert!(html.contains(r#"name="typeface""#), "{html}");
        assert!(html.contains(r#"name="primary_color""#), "{html}");
        assert!(html.contains(r##"value="#007c91""##), "{html}");
        assert!(!html.contains(r#"name="palette""#), "{html}");
    }

    #[test]
    fn the_logo_and_font_uploads_are_native_multipart_forms() {
        let html = dioxus_ssr::render_element(brands_edit_body(&view(Some(fields()))));
        assert!(html.contains(r#"action="/app/brands/neon/logo""#), "{html}");
        assert!(html.contains(r#"action="/app/brands/neon/font""#), "{html}");
        assert!(html.contains(r#"enctype="multipart/form-data""#), "{html}");
        assert!(html.contains(r#"name="licence""#), "{html}");
    }

    #[test]
    fn an_uploaded_logo_and_font_render_their_status() {
        let mut fields = fields();
        fields.logo_url = Some("/app/brands/neon/logo".to_string());
        fields.font = Some((
            "/assets/fonts/brands/neon/abc.woff2".to_string(),
            "OFL-1.1".to_string(),
        ));
        let html = dioxus_ssr::render_element(brands_edit_body(&view(Some(fields))));
        assert!(html.contains("Current logo"), "{html}");
        assert!(html.contains("Uploaded font:"), "{html}");
        assert!(html.contains("OFL-1.1"), "{html}");
    }

    #[test]
    fn a_refusal_names_the_rule() {
        let mut view = view(Some(fields()));
        view.error = Some(
            "#f5f5a0's best on-primary contrast is 1.2:1; it must be at least 4.5:1.".to_string(),
        );
        let html = dioxus_ssr::render_element(brands_edit_body(&view));
        assert!(html.contains("must be at least 4.5:1"), "{html}");
    }

    #[test]
    fn a_missing_brand_renders_not_found() {
        let html = dioxus_ssr::render_element(brands_edit_body(&view(None)));
        assert!(html.contains("Brand not found"), "{html}");
    }
}
