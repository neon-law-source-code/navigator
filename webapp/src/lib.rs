//! The Dioxus fullstack application for Neon Law Navigator.
//!
//! Phase 0 of the Dioxus adoption (issue #641) mounts this crate's [`App`]
//! into the existing `web` axum router on one low-risk page. `web` links this
//! library and server-side renders [`App`] through `dioxus-server`'s
//! `render_handler`; `navigator dev build-webapp` compiles the same crate to
//! `wasm32-unknown-unknown` (driving `dx`) to produce the same-origin client
//! bundle that hydrates that server-rendered markup.
//!
//! The component tree here stays deliberately small: Phase 0 proves the mount,
//! not the design system. Later phases rebuild the `views` components as Dioxus
//! components and migrate real page clusters onto this seam.

use dioxus::prelude::*;

pub mod admin_landing;
pub mod admin_listing;
pub mod admin_listings;
pub mod admin_people_new;
pub mod admin_unassigned_project_detail;
pub mod analytics;
pub mod app_chrome;
#[cfg(feature = "server")]
pub mod auth_pages;
pub mod blog_index;
pub mod blog_post;
pub mod brand_style;
pub mod clause_editor;
pub mod clerk;
pub mod cli_release;
pub mod client_intake;
pub mod components;
pub mod contact_page;
pub mod contract_review;
pub mod conversation;
pub mod csrf;
pub mod design;
pub mod docs_page;
// Server-only: the DocuSign consent callback is returned inline from
// `portal` rather than a Dioxus router, so it renders its component through
// the standalone SSR document seam.
#[cfg(feature = "server")]
pub mod docusign_consent;
pub mod entity_edit;
pub mod entity_list;
pub mod entity_new;
pub mod entity_types;
// Server-only: these are rendered inline from `portal`'s handlers, so they need
// the SSR renderer, `axum`'s response type, and the `views` brand seam — none of
// which the wasm client build links. Gated like `public_chrome`'s constructors,
// which they call.
pub mod catalog_certificate_sent;
pub mod catalog_display;
pub mod catalog_index;
pub mod catalog_material;
pub mod catalog_slide_body;
pub mod catalog_slides;
pub mod catalog_step;
#[cfg(feature = "server")]
pub mod error_pages;
pub mod expunge_document;
pub mod expunge_requests;
pub mod gov_forms;
pub mod harvard_outline;
pub mod home;
pub mod html_escape;
pub mod intake_review;
pub mod lawyer_dashboard;
pub mod lawyer_project_detail;
pub mod legal_page;
pub mod letter_detail;
pub mod litigation_page;
pub mod marketing_page;
pub mod matter_directory;
pub mod matter_surface;
pub mod notation_outline;
pub mod notation_preview;
pub mod people;
pub mod person_show;
pub mod playbooks;
pub mod portal_project_detail;
pub mod portal_project_list;
pub mod project_calendar;
pub mod project_document_detail;
pub mod project_edit;
pub mod project_list;
pub mod project_new;
pub mod project_notation;
pub mod project_participation;
pub mod project_resources;
pub mod public_chrome;
pub mod reask;
pub mod retainer_start;
pub mod review;
pub mod schedules;
pub mod source_repository;
pub mod team_home;
pub mod template_gallery;
pub mod transactional_page;
pub mod walker_step;

/// The root component `web` renders on the server and the wasm client
/// hydrates in the browser.
///
/// The markup is fully server-rendered — readable before any JavaScript runs —
/// while the counter is inert until the WebAssembly bundle hydrates it, which
/// is exactly the property Phase 0 exists to demonstrate.
#[allow(non_snake_case)]
pub fn App() -> Element {
    let mut clicks = use_signal(|| 0_u32);

    rsx! {
        main { id: "dioxus-demo",
            h1 { "Dioxus is mounted" }
            p {
                "This page is server-side rendered by the Navigator "
                code { "web" }
                " process and hydrated by a same-origin WebAssembly bundle — no \
                 CDN, no inline script. Every other route (the JSON API, MCP, \
                 A2A, git smart-HTTP, OIDC, and the marketing pages) is served \
                 by the same axum router, unchanged."
            }
            p {
                "The counter below is inert in the server-rendered HTML and \
                 becomes interactive only once the client bundle has hydrated \
                 this markup, which is how you can tell hydration ran:"
            }
            button {
                r#type: "button",
                onclick: move |_| clicks += 1,
                "Clicked {clicks} times"
            }
        }
    }
}
