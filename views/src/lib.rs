//! Shared presentation data and server-side rendering utilities.
//!
//! Browser pages live in `webapp`'s Dioxus component tree. This crate keeps
//! presentation-neutral data and helpers shared by the portal, CLI, workers,
//! and the Dioxus server render path.

pub mod assets;
pub mod auth_state;
pub mod brand;
pub mod brand_bundle;
pub mod components;
pub mod harvard_outline;
pub mod locales;
pub mod lsp;
pub mod markdown;
pub mod notation;
pub mod notations;
pub mod questionnaire_preview;
pub mod slug;

pub use auth_state::AuthState;
pub use brand::SiteBrand;
