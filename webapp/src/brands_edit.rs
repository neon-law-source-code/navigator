//! `/app/admin/brands/{key}/edit` — a brand's presentation: name, typeface, a free
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
        typefaces: Vec::new(),
        font_licences,
        csrf_token,
        error: query.error.clone(),
    };

    let brand = match store::brands::find_by_key_for_actor(
        &surreal,
        store_role(role),
        actor_person_id,
        &key,
    )
    .await
    {
        Ok(brand) => brand,
        Err(
            store::brands::BrandError::NotAuthorized
            | store::brands::BrandError::NoSuchBrand(_)
            | store::brands::BrandError::NoSuchFirm(_),
        ) => {
            dioxus_fullstack_core::FullstackContext::commit_http_status(
                axum::http::StatusCode::NOT_FOUND,
                None,
            );
            return Ok(base);
        }
        Err(error) => return Err(ServerFnError::new(error.to_string())),
    };

    // ENG-659: the typeface select draws only from this Firm's own uploaded
    // fonts — never the compiled `views::brand::TYPEFACES` catalog. Empty
    // when the Firm has uploaded no font yet; a historical row with no
    // `firm_id` at all (pre-ENG-659, not yet visited by the schema
    // backfill) offers none either, since there is no Firm to scope the
    // list to.
    let typefaces = match brand.firm_id {
        Some(firm_id) => store::brands::uploaded_font_families_for_firm(&surreal, firm_id)
            .await
            .map_err(|error| ServerFnError::new(error.to_string()))?
            .into_iter()
            .map(|family| FormChoice {
                value: family.clone(),
                label: family,
            })
            .collect(),
        None => Vec::new(),
    };

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
        typefaces,
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

/// Route entry for `/app/admin/brands/{key}/edit`.
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
    let has_uploaded_fonts = !type_opts.is_empty();

    let mut form_fields = vec![Field::text("Name", "name", fields.name.clone()).required()];
    // ENG-659: the typeface select's options are this Firm's own uploaded
    // font family names — never the compiled catalog — so choosing one sets
    // both the brand's `typeface` (always "uploaded" once any family is
    // chosen) and `font_family` fields together. There is no separate
    // "Font family" text input any more: a Firm with no uploaded font yet
    // has nothing to choose, so the control is omitted entirely rather than
    // rendered empty and required.
    if has_uploaded_fonts {
        form_fields.push(
            Field::select(
                "Typeface",
                "typeface",
                type_opts,
                Some(fields.font_family.clone()),
            )
            .required()
            .help("This Firm's uploaded font family names. Upload a .woff2 below to add one."),
        );
    }
    form_fields.push(
        Field::text(
            "Primary colour",
            "primary_color",
            fields.primary_color.clone(),
        )
        .required()
        .placeholder("#007c91")
        .help(
            "A #rrggbb hex. Its on-primary text (white or black, whichever contrasts more) must \
             clear WCAG AA 4.5:1, and it must clear 3:1 against the light page surface, or the \
             save is refused naming the ratio.",
        ),
    );
    rsx! {
        if let Some(error) = &view.error {
            p { class: "nav-form-error", role: "alert", "{error}" }
        }
        if !has_uploaded_fonts {
            p { class: "muted", id: "no-uploaded-fonts",
                "No fonts uploaded yet for this Firm — upload one below, then it appears here."
            }
        }
        FormCard {
            title: format!("Edit {}", fields.name),
            action: format!("/app/admin/brands/{}/edit", view.key),
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
                action: format!("/app/admin/brands/{}/logo", view.key),
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
                action: format!("/app/admin/brands/{}/font", view.key),
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
            p { class: "page-subtitle", id: "brand-website",
                "{crate::brand_website::BrandWebsite::from_key(&view.key).host_line()}"
            }
            match &view.fields {
                Some(fields) => rsx! {
                    {presentation_form(view, fields)}
                    {logo_form(view, fields)}
                    {font_form(view, fields)}
                    p { a { href: "/app/admin/brands", "← Brands" } }
                },
                None => rsx! {
                    h1 { "Brand not found" }
                    p { "No brand exists with key " code { "{view.key}" } "." }
                    p { a { href: "/app/admin/brands", "← Brands" } }
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
            // ENG-659: a Firm's own uploaded font family names, never the
            // compiled catalog.
            typefaces: vec![FormChoice {
                value: "Custom Sans".to_string(),
                label: "Custom Sans".to_string(),
            }],
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
            typeface: "uploaded".to_string(),
            primary_color: "#007c91".to_string(),
            font_family: "Custom Sans".to_string(),
            logo_url: None,
            font: None,
        }
    }

    #[test]
    fn the_presentation_form_is_a_hex_input_and_posts_to_the_key() {
        let html = dioxus_ssr::render_element(brands_edit_body(&view(Some(fields()))));
        assert_forms_accessible(&html, "brand presentation");
        assert!(
            html.contains(r#"action="/app/admin/brands/neon/edit""#),
            "{html}"
        );
        assert!(html.contains(r#"name="typeface""#), "{html}");
        assert!(html.contains(r#"name="primary_color""#), "{html}");
        assert!(html.contains(r##"value="#007c91""##), "{html}");
        assert!(!html.contains(r#"name="palette""#), "{html}");
        assert!(!html.contains(r#"name="font_family""#), "{html}");
        assert!(html.contains("www.neonlaw.com"), "{html}");
    }

    /// ENG-659: the typeface select's only options are this Firm's own
    /// uploaded font family names — the compiled catalog never appears,
    /// whether or not this Firm has uploaded a font yet.
    #[test]
    fn the_typeface_select_never_lists_the_compiled_catalog() {
        let html = dioxus_ssr::render_element(brands_edit_body(&view(Some(fields()))));
        for compiled in ["gorp-serif", "tinos", "system-serif", "system-sans"] {
            assert!(!html.contains(compiled), "{compiled} leaked into: {html}");
        }
        assert!(html.contains("Custom Sans"), "{html}");
    }

    /// ENG-659: a Firm with no uploaded font yet gets no typeface select at
    /// all — an empty required `<select>` would be unusable — and sees an
    /// explanation instead.
    #[test]
    fn a_firm_with_no_uploaded_fonts_sees_no_typeface_select() {
        let mut view = view(Some(fields()));
        view.typefaces = Vec::new();
        let html = dioxus_ssr::render_element(brands_edit_body(&view));
        assert!(!html.contains(r#"name="typeface""#), "{html}");
        assert!(html.contains("No fonts uploaded yet"), "{html}");
    }

    #[test]
    fn the_logo_and_font_uploads_are_native_multipart_forms() {
        let html = dioxus_ssr::render_element(brands_edit_body(&view(Some(fields()))));
        assert!(
            html.contains(r#"action="/app/admin/brands/neon/logo""#),
            "{html}"
        );
        assert!(
            html.contains(r#"action="/app/admin/brands/neon/font""#),
            "{html}"
        );
        assert!(html.contains(r#"enctype="multipart/form-data""#), "{html}");
        assert!(html.contains(r#"name="licence""#), "{html}");
    }

    #[test]
    fn an_uploaded_logo_and_font_render_their_status() {
        let mut fields = fields();
        fields.logo_url = Some("/app/admin/brands/neon/logo".to_string());
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
            "#f5f5a0 is 1.1:1 against the light page surface; it must be at least 3.0:1."
                .to_string(),
        );
        let html = dioxus_ssr::render_element(brands_edit_body(&view));
        assert!(html.contains("must be at least 3.0:1"), "{html}");
    }

    #[test]
    fn a_missing_brand_renders_not_found() {
        let html = dioxus_ssr::render_element(brands_edit_body(&view(None)));
        assert!(html.contains("Brand not found"), "{html}");
    }
}
