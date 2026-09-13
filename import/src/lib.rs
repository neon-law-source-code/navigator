//! Bulk contact import — the shared engine behind every surface that
//! turns a list of organizations and the people who work at them into
//! `entities`, `persons`, and the `person_entity_roles` links between
//! them.
//!
//! Mirrors the `rules` crate's shape: one library, many callers. The
//! `cli` (`import-contacts`), a Navigator MCP tool (`bulk_import`),
//! and a future `web` upload route all parse a [`Payload`], run
//! [`validate`] for structural diagnostics, then [`apply`] it against
//! the database. Nothing here is surface-specific.
//!
//! The contract is documented for humans and LLMs in
//! [`docs/bulk-contact-import.md`](../../docs/bulk-contact-import.md).
//!
//! ## Idempotency
//!
//! Every write is find-or-create. People dedupe on the unique
//! `persons.email`; organizations on `(name, entity_type, jurisdiction)`;
//! links on `(person, entity, role)`. Re-running the same payload is a
//! no-op that reports `unchanged`, so an import is always safe to retry.
//! The JSON is authoritative: a re-run overwrites `name`/`title`/`phone`
//! from the file (but never a person's `role` — promotions are sticky).

mod apply;
mod contract;
mod validate;

pub use apply::{apply, ImportReport, Outcome, RowOutcome};
pub use contract::{OrgRecord, Payload, PersonRecord, DEFAULT_ENTITY_ROLE, SUPPORTED_VERSION};
pub use validate::{canonical_url, validate, Diagnostic, Severity};

/// Parse a [`Payload`] from JSON or YAML bytes. JSON is the canonical
/// wire format (it's what an LLM emits); YAML is accepted because the
/// workspace's seed files are YAML and operators may hand-author one.
///
/// The format is detected by content: a leading `{` (after whitespace)
/// is parsed as JSON, anything else as YAML. YAML is a JSON superset,
/// so the YAML parser also accepts JSON — the sniff just keeps error
/// messages pointing at the format the author actually wrote.
pub fn parse(bytes: &str) -> anyhow::Result<Payload> {
    if bytes.trim_start().starts_with('{') {
        Ok(serde_json::from_str(bytes)?)
    } else {
        Ok(serde_yaml::from_str(bytes)?)
    }
}
