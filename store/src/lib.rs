#![allow(clippy::doc_markdown)]
//! Neon Law Navigator CRM data layer.
//!
//! Owns the SurrealDB schema and the canonical seed. Every workspace
//! crate that touches the store — `web`, `cli`, `mcp` — depends on this
//! crate; nothing here depends on axum, reqwest, or any HTTP machinery.
//!
//! One table is one top-level module: `store::persons` owns `person`,
//! `store::projects` owns `project`, and so on. The schema itself is a
//! statement of the present rather than a history — one idempotent
//! `DEFINE` file at `store/src/schema/navigator.surql`, applied whole on
//! every boot, plus a `schema_version` record. You change it by editing
//! that file, not by appending a step.
//!
//! # What is deliberately absent
//!
//! Billing and cap tables have no module here and never will. The Firm
//! bills through Xero and keeps cap tables in Carta, so Navigator models
//! neither: there is no `entity_billing_profiles`, `invoices`,
//! `invoice_line_items`, `share_issuances`, or `subscriptions` to look
//! for. [`xero_invoices`] is a *mirror* — it backs the matter page's
//! Xero button and is a link out to the system of record, not a ledger
//! of its own.

pub mod access;
pub mod addresses;
pub mod answers;
pub mod assets;
pub mod attestations;
pub mod authorities;
pub mod cases;
pub mod communications;
pub mod config;
pub mod conflicts;
pub mod contract_reviews;
pub mod credentials;
pub mod deployment;
pub mod disclosures;
pub mod document_comments;
pub mod documents;
pub mod email_conversations;
pub mod email_tokens;
pub mod entities;
pub mod entity_commands;
pub mod entity_roles;
pub mod entity_types;
pub mod expunge_records;
pub mod expunge_requests;
pub mod external_identities;
pub mod filings;
pub mod git_access_tokens;
pub mod git_repositories;
pub mod glossary;
pub mod jurisdictions;
pub mod letters;
pub mod mailrooms;
pub mod notarizations;
pub mod notation_clauses;
pub mod notation_events;
pub mod notations;
pub mod participation;
pub mod people_commands;
pub mod persons;
pub mod playbooks;
pub mod project_modules;
pub mod project_reconcile;
pub mod project_surfaces;
pub mod projects;
pub mod question_registry;
pub mod questions;
pub mod reask;
pub mod relationship_logs;
pub mod relationships;
pub mod review_documents;
/// Publishing a built sample-project bundle into the applications bucket.
pub mod sample_project;
/// The schema, applied as one idempotent `DEFINE` file (#1093).
pub mod schema;
pub mod seed;
pub mod sent_emails;
pub mod signatures;
pub mod source_pages;
pub mod statutory_deadlines;
/// The store connection, over the `NAVIGATOR_SURREAL_*` contract.
pub mod surreal;
pub mod template_source;
pub mod templates;
pub mod testimonials;
pub mod trust;
pub mod visitor_analytics;
pub mod xero_invoices;

#[cfg(feature = "test-support")]
pub mod test_support;
pub mod verifications;

pub use config::{
    sample_matters, sample_matters_from, DeploymentEnvironment, DeploymentEnvironmentError,
    SampleMattersError, NAVIGATOR_ENVIRONMENT, NAVIGATOR_SIMULATED_MATTERS,
};
