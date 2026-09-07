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

#[cfg(test)]
pub(crate) mod test_tracing {
    use tracing::span::{Attributes, Id, Record};
    use tracing::subscriber::Interest;
    use tracing::{Event, Metadata, Subscriber};

    /// A globally-installed subscriber that claims interest in every callsite
    /// but records nothing itself.
    ///
    /// `tracing` computes and caches each callsite's interest from the
    /// *globally registered* dispatchers only — a thread-local `set_default`
    /// (how the capture tests install their subscriber) is invisible to that
    /// cache. With no global dispatcher, a callsite first seen — or rebuilt —
    /// while the global default is `NoSubscriber` caches as `Interest::never()`,
    /// and the event a capturing test is asserting on is dropped before its
    /// per-event `enabled` check ever runs against the thread-local subscriber.
    /// Returning `Interest::sometimes()` from a globally-registered dispatcher
    /// keeps every callsite deferring to the current dispatcher, so the
    /// thread-local capture is consulted per event; `enabled` returns `false`
    /// so this global itself records nothing on threads without a capturing
    /// default. Mirrors `portal::test_tracing`.
    struct AlwaysInterested;

    impl Subscriber for AlwaysInterested {
        fn register_callsite(&self, _: &'static Metadata<'static>) -> Interest {
            Interest::sometimes()
        }
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            false
        }
        fn new_span(&self, _: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }
        fn record(&self, _: &Id, _: &Record<'_>) {}
        fn record_follows_from(&self, _: &Id, _: &Id) {}
        fn event(&self, _: &Event<'_>) {}
        fn enter(&self, _: &Id) {}
        fn exit(&self, _: &Id) {}
    }

    static INSTALL: std::sync::Once = std::sync::Once::new();

    /// Installs [`AlwaysInterested`] as the process-global default the first
    /// time any capture test runs, so callsite interest can never cache as
    /// `never`. Idempotent, and a no-op if some other global default is
    /// already set.
    pub(crate) fn ensure_callsite_interest() {
        INSTALL.call_once(|| {
            let _ = tracing::subscriber::set_global_default(AlwaysInterested);
        });
    }
}

pub mod access;
pub mod addresses;
pub mod answers;
pub mod assets;
pub mod attestations;
pub mod authorities;
pub mod brands;
pub mod cases;
pub mod communications;
pub mod config;
pub mod conflicts;
pub mod contract_reviews;
pub mod credentials;
pub mod delegations;
pub mod deployment;
pub mod disclosures;
pub mod document_comments;
pub mod document_pointers;
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
pub mod firm_capability;
pub mod firms;
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
