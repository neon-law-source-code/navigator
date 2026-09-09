//! The self-service `/app/profile` page — every authenticated tier, Client
//! included, updates their own avatar here through the native `POST
//! /app/profile/avatar` multipart form. No tier gate at all: unlike the
//! admin-only `/app/admin/people/{id}/avatar`, the target is always the
//! caller's own row, resolved server-side from the signed session, never a
//! person id supplied by the page.
//!
//! Email renders read-only: the account's mailbox is also its sign-in
//! identity, so a self-service edit here would drift from the OIDC identity
//! the session was minted against. The disabled field carries a note to
//! contact the firm instead.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Field, FormCard, Heading};
use crate::people::ViewerRole;

/// The profile page path.
pub const PROFILE_PATH: &str = "/app/profile";
/// The native multipart upload the page's avatar card posts to.
pub const PROFILE_AVATAR_PATH: &str = "/app/profile/avatar";

/// The rendered self-service profile page.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ProfileView {
    /// The resolved brand's tokens stylesheet href, so the page wears its own
    /// palette rather than the firm's on a non-default host.
    #[serde(default)]
    pub tokens_href: String,
    /// The deploy's firm name, for the document title.
    #[serde(default)]
    pub firm_name: String,
    pub role: ViewerRole,
    /// `None` when the session carries no linked person — unreachable outside
    /// a test harness edge case, since every route this page mounts on
    /// requires an authenticated session with a linked `persons` row.
    pub fields: Option<ProfileFields>,
    /// The deploy's brand mark for the navbar. `None` when the mounted brand
    /// configures none.
    #[serde(default)]
    pub logo: Option<crate::components::AppLogo>,
}

/// The caller's own prefilled fields.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ProfileFields {
    pub name: String,
    pub email: String,
    pub csrf_token: String,
}

#[cfg(feature = "server")]
async fn load_profile(role: ViewerRole) -> Result<ProfileView, ServerFnError> {
    let csrf_token = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::csrf::CsrfToken>,
        _,
    >()
    .await
    .map(|axum::Extension(token)| token.0)
    .unwrap_or_default();

    let crate::portal_project_list::PersonId(person_id) =
        dioxus_fullstack_core::FullstackContext::extract::<
            axum::Extension<crate::portal_project_list::PersonId>,
            _,
        >()
        .await
        .map(|axum::Extension(id)| id)
        .unwrap_or_default();
    let person_id = person_id.and_then(|raw| raw.parse::<uuid::Uuid>().ok());

    let tokens_href = crate::app_chrome::app_tokens_href_from_context().await;
    let firm_name = crate::app_chrome::firm_name_from_context().await;
    let logo = crate::app_chrome::app_logo_from_context().await;

    let Some(person_id) = person_id else {
        return Ok(ProfileView {
            tokens_href,
            firm_name,
            role,
            fields: None,
            logo,
        });
    };

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let person = store::persons::find_by_id(&surreal, person_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let Some(person) = person else {
        return Ok(ProfileView {
            tokens_href,
            firm_name,
            role,
            fields: None,
            logo,
        });
    };

    Ok(ProfileView {
        tokens_href,
        firm_name,
        role,
        fields: Some(ProfileFields {
            name: person.name,
            email: person.email,
            csrf_token,
        }),
        logo,
    })
}

/// Load the profile page: every authenticated tier reaches this — there is no
/// role check here, unlike every other `get_*` loader in this crate.
#[server]
pub async fn get_profile() -> Result<ProfileView, ServerFnError> {
    let role = dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<ViewerRole>, _>()
        .await
        .map(|axum::Extension(role)| role)
        .unwrap_or_default();
    load_profile(role).await
}

/// The `/app/profile` page.
#[component]
pub fn Profile() -> Element {
    let resource = use_server_future(get_profile)?;
    render_profile(&resource)
}

/// The avatar preview and self-service upload form, a sibling `FormCard` to
/// the read-only contact card below it — the two need different `enctype`s,
/// and `FormCard` only supports one per `<form>`.
fn avatar_upload_card(csrf_token: &str) -> Element {
    rsx! {
        section { id: "profile-avatar", class: "person-avatar",
            h2 { "Avatar" }
            img {
                class: "person-avatar__preview",
                src: "/app/me/avatar",
                alt: "Your profile photo",
                width: "96",
                height: "96",
            }
            FormCard {
                title: "Upload avatar".to_string(),
                action: PROFILE_AVATAR_PATH.to_string(),
                submit_label: "Upload".to_string(),
                heading: Heading::H2,
                multipart: true,
                csrf_token: Some(csrf_token.to_string()),
                fields: vec![
                    Field::file("Avatar", "file")
                        .required()
                        .help("PNG, JPEG, or WebP, up to 5 MB. Replaces any existing avatar."),
                ],
            }
        }
    }
}

/// The read-only contact card: name and email, for context. Email is
/// disabled — greyed out — with a note to contact the firm instead of
/// offering a Save that would drift from the sign-in identity.
fn contact_card(fields: &ProfileFields) -> Element {
    rsx! {
        section { id: "profile-details",
            h2 { "Details" }
            FormCard {
                title: "Contact".to_string(),
                action: PROFILE_PATH.to_string(),
                submit_label: String::new(),
                heading: Heading::H2,
                read_only: true,
                fields: vec![
                    Field::text("Name", "name", fields.name.clone()).disabled(),
                    Field::input("Email", "email", fields.email.clone(), "email")
                        .disabled()
                        .help("Contact us to update your email address."),
                ],
            }
        }
    }
}

/// Render the resolved profile page.
fn render_profile(resource: &Resource<Result<ProfileView, ServerFnError>>) -> Element {
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "profile", p { "Failed to load your profile." } }
            }
        }
        None => {
            return rsx! {
                main { id: "profile", p { "Loading…" } }
            }
        }
    };

    let page_title = format!("{} | Profile", view.firm_name);

    rsx! {
        document::Title { "{page_title}" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        crate::components::AppNavbar {
            destinations: crate::app_chrome::app_destinations(view.role),
            logo: view.logo.clone(),
        }
        main { id: "profile", class: "nav-theme",
            header { class: "page-header",
                h1 { "Your profile" }
            }
            match &view.fields {
                Some(fields) => rsx! {
                    {avatar_upload_card(&fields.csrf_token)}
                    {contact_card(fields)}
                },
                None => rsx! {
                    p { "Your session has no linked profile to show." }
                },
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

    fn sample_fields() -> ProfileFields {
        ProfileFields {
            name: "Libra Scales".to_string(),
            email: "libra@example.com".to_string(),
            csrf_token: "csrf-token".to_string(),
        }
    }

    fn avatar_app() -> Element {
        avatar_upload_card("csrf-token")
    }

    fn contact_app() -> Element {
        contact_card(&sample_fields())
    }

    #[test]
    fn the_avatar_card_is_a_sibling_multipart_form_posting_to_the_self_service_route() {
        let out = ssr(avatar_app);
        assert!(
            out.contains(r#"enctype="multipart/form-data""#),
            "the avatar form must be multipart: {out}"
        );
        assert!(
            out.contains(PROFILE_AVATAR_PATH),
            "posts to the self-service avatar route: {out}"
        );
        assert!(
            out.contains(r#"src="/app/me/avatar""#),
            "the preview reads the caller's own avatar route: {out}"
        );
        let csrf_pos = out.find(r#"name="_csrf""#);
        let file_pos = out.find(r#"type="file""#);
        assert!(
            csrf_pos.is_some() && file_pos.is_some() && csrf_pos < file_pos,
            "CSRF must be the first field, before the file input: {out}"
        );
    }

    /// The contact card renders every field disabled and offers no Save — an
    /// email edit here would drift from the OIDC identity the session was
    /// minted against, so the page never invites one.
    #[test]
    fn the_contact_card_is_read_only_with_no_submit_button() {
        let out = ssr(contact_app);
        assert!(
            out.contains(r#"id="name""#) && out.contains("disabled"),
            "{out}"
        );
        assert!(!out.contains(r#"type="submit""#), "{out}");
    }

    #[test]
    fn the_email_field_carries_a_contact_us_note() {
        let out = ssr(contact_app);
        assert!(
            out.contains("Contact us to update your email address."),
            "{out}"
        );
    }

    #[test]
    fn the_name_and_email_fields_are_prefilled_from_the_caller() {
        let out = ssr(contact_app);
        assert!(out.contains(r#"value="Libra Scales""#), "{out}");
        assert!(out.contains(r#"value="libra@example.com""#), "{out}");
    }
}
