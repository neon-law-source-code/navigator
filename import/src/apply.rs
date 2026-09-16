//! Apply a validated [`Payload`] to the database — find-or-create the
//! organizations as `entities`, the people as `persons`, and the
//! `person_entity_roles` links between them. Every step is idempotent
//! and reported per row; a referenced `entity_type` or `jurisdiction`
//! that does not exist fails only that row, never the whole batch.
//!
//! Single-engine since wave four (ENG-120) moved `entities` and
//! `person_entity_roles`: every table this touches — `person` (ENG-19),
//! `jurisdiction`, `entity_type` (ENG-20), `entity`, and the
//! `entity_role` relation — lives in `SurrealDB` (#1093). The engine
//! still does not validate a link, which is why [`upsert_person`]
//! resolves the person first and a failed person skips its own link.

use std::collections::HashMap;

use anyhow::anyhow;
use serde::Serialize;
use uuid::Uuid;

use crate::contract::Payload;
use crate::validate::{canonical_url, validate, Diagnostic, Severity};
use store::entities::{self, NewEntity};
use store::persons::{self, ContactUpdate, NewPerson};
use store::surreal::SurrealDb;

/// What happened to one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// A new row was inserted.
    Created,
    /// An existing row was found and at least one field changed.
    Updated,
    /// An existing row was found and nothing changed.
    Unchanged,
    /// The row could not be applied (reason in `detail`).
    Failed,
}

/// The result of applying one organization or person.
#[derive(Debug, Clone, Serialize)]
pub struct RowOutcome {
    /// The payload `key` this outcome is for.
    pub key: String,
    pub status: Outcome,
    /// The database id, when the row was created/updated/unchanged.
    pub id: Option<Uuid>,
    /// A failure reason, or a note (e.g. a skipped link).
    pub detail: Option<String>,
}

/// The whole import result. Returned even when structural validation
/// rejects the payload (then `organizations`/`people` are empty and the
/// reason is in `diagnostics`).
#[derive(Debug, Clone, Serialize)]
pub struct ImportReport {
    pub diagnostics: Vec<Diagnostic>,
    pub organizations: Vec<RowOutcome>,
    pub people: Vec<RowOutcome>,
}

impl ImportReport {
    /// `true` if any structural error blocked the apply, or any row failed.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
            || self
                .organizations
                .iter()
                .chain(&self.people)
                .any(|r| r.status == Outcome::Failed)
    }

    /// Count rows (orgs + people) with the given outcome.
    #[must_use]
    pub fn count(&self, status: Outcome) -> usize {
        self.organizations
            .iter()
            .chain(&self.people)
            .filter(|r| r.status == status)
            .count()
    }

    /// One-line tally for logs and CLI output.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{} created, {} updated, {} unchanged, {} failed",
            self.count(Outcome::Created),
            self.count(Outcome::Updated),
            self.count(Outcome::Unchanged),
            self.count(Outcome::Failed),
        )
    }

    /// A human-readable, multi-line list of everything that went wrong or
    /// is worth flagging — every structural [`Diagnostic`] (errors AND
    /// warnings) followed by every per-row `detail` (a failure reason or a
    /// note like a skipped link) — or `None` when the import was wholly
    /// clean.
    ///
    /// This exists because the structured `diagnostics` / `RowOutcome.detail`
    /// fields are invisible on surfaces that render only text: the
    /// `bulk_import` MCP/A2A `content` Part (Gemini Enterprise shows
    /// that text and drops the structured payload) and the CLI. Folding the
    /// detail into one block is what lets a caller see *why* `0 created`
    /// instead of a silent, message-less non-result.
    #[must_use]
    pub fn problem_lines(&self) -> Option<String> {
        let mut lines = Vec::new();
        for d in &self.diagnostics {
            let label = match d.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            };
            lines.push(format!("• {} ({label}): {}", d.pointer, d.message));
        }
        for (kind, rows) in [
            ("organization", &self.organizations),
            ("person", &self.people),
        ] {
            for row in rows {
                match (&row.status, &row.detail) {
                    (Outcome::Failed, detail) => lines.push(format!(
                        "• {kind} `{}` failed: {}",
                        row.key,
                        detail.as_deref().unwrap_or("unknown reason")
                    )),
                    // A non-failed row can still carry a note (e.g. a
                    // person created but whose org link was skipped).
                    (_, Some(note)) => lines.push(format!("• {kind} `{}`: {note}", row.key)),
                    (_, None) => {}
                }
            }
        }
        (!lines.is_empty()).then(|| lines.join("\n"))
    }
}

/// Validate, then apply. If structural validation finds any error,
/// nothing is written and the diagnostics are returned. Otherwise every
/// organization and person is find-or-created and a per-row report comes
/// back. Each row's outcome is also emitted as an OTel/tracing event so
/// the import history lands in telemetry.
pub async fn apply(surreal: &SurrealDb, payload: &Payload) -> anyhow::Result<ImportReport> {
    let diagnostics = validate(payload);
    if diagnostics.iter().any(|d| d.severity == Severity::Error) {
        tracing::warn!(
            target: "import",
            errors = diagnostics.iter().filter(|d| d.severity == Severity::Error).count(),
            "bulk_import rejected: validation errors",
        );
        return Ok(ImportReport {
            diagnostics,
            organizations: Vec::new(),
            people: Vec::new(),
        });
    }

    let mut organizations = Vec::with_capacity(payload.organizations.len());
    let mut entity_by_key: HashMap<&str, Uuid> = HashMap::new();
    for org in &payload.organizations {
        let outcome = match upsert_entity(surreal, org).await {
            Ok((id, status)) => {
                entity_by_key.insert(org.key.as_str(), id);
                RowOutcome {
                    key: org.key.clone(),
                    status,
                    id: Some(id),
                    detail: None,
                }
            }
            Err(e) => RowOutcome {
                key: org.key.clone(),
                status: Outcome::Failed,
                id: None,
                detail: Some(e.to_string()),
            },
        };
        tracing::info!(
            target: "import",
            kind = "organization",
            key = %outcome.key,
            status = ?outcome.status,
            id = ?outcome.id,
            "bulk_import row",
        );
        organizations.push(outcome);
    }

    let mut people = Vec::with_capacity(payload.people.len());
    for record in &payload.people {
        let outcome = match upsert_person(surreal, record).await {
            Ok((person_id, status)) => {
                let detail = match entity_by_key.get(record.organization.as_str()) {
                    Some(&entity_id) => {
                        link_person_entity(surreal, person_id, entity_id, &record.entity_role)
                            .await
                            .err()
                            .map(|e| format!("link failed: {e}"))
                    }
                    None => Some(format!(
                        "organization `{}` was not created; link skipped",
                        record.organization
                    )),
                };
                RowOutcome {
                    key: record.key.clone(),
                    status,
                    id: Some(person_id),
                    detail,
                }
            }
            Err(e) => RowOutcome {
                key: record.key.clone(),
                status: Outcome::Failed,
                id: None,
                detail: Some(e.to_string()),
            },
        };
        tracing::info!(
            target: "import",
            kind = "person",
            key = %outcome.key,
            status = ?outcome.status,
            id = ?outcome.id,
            "bulk_import row",
        );
        people.push(outcome);
    }

    let report = ImportReport {
        diagnostics,
        organizations,
        people,
    };
    tracing::info!(
        target: "import",
        source = payload.source.as_deref().unwrap_or("(none)"),
        summary = %report.summary(),
        "bulk_import complete",
    );
    Ok(report)
}

/// Find-or-create one `entities` row, keyed on
/// `(name, entity_type_id, jurisdiction_id)`. Resolves the entity-type
/// name and jurisdiction code to their ids; an unknown one is an error
/// for this row.
async fn upsert_entity(
    surreal: &SurrealDb,
    org: &crate::contract::OrgRecord,
) -> anyhow::Result<(Uuid, Outcome)> {
    let entity_type = store::entity_types::find_by_name(surreal, org.entity_type.trim())
        .await?
        .ok_or_else(|| anyhow!("unknown entity_type `{}`", org.entity_type.trim()))?;

    let code = org.jurisdiction.trim().to_ascii_uppercase();
    let jurisdiction = store::jurisdictions::find_by_code(surreal, &code)
        .await?
        .ok_or_else(|| anyhow!("unknown jurisdiction code `{code}`"))?;

    let url = match &org.url {
        Some(raw) => Some(canonical_url(raw).map_err(|e| anyhow!(e))?),
        None => None,
    };
    let phone = clean(org.phone.as_deref());
    let name = org.name.trim();

    let existing =
        entities::find_by_identity(surreal, name, entity_type.id, jurisdiction.id).await?;

    if let Some(row) = existing {
        // Payload wins when it carries a value; an absent field never
        // erases what's already stored.
        let next_phone = phone.or_else(|| row.phone.clone());
        let next_url = url.or_else(|| row.url.clone());
        if row.phone == next_phone && row.url == next_url {
            return Ok((row.id, Outcome::Unchanged));
        }
        let updated = entities::update(
            surreal,
            row.id,
            &NewEntity {
                name: row.name.clone(),
                entity_type_id: row.entity_type_id,
                jurisdiction_id: row.jurisdiction_id,
                phone: next_phone,
                url: next_url,
                // The bulk-contact payload carries no Xero id; an admin
                // enters that by hand, and an import must not blank it out.
                xero_id: row.xero_id.clone(),
                // Carried, not recomputed: whether this row is the firm
                // anchor was decided when it was created, and an import
                // must not be able to promote or demote it.
                firm_anchor_key: row.firm_anchor_key.clone(),
            },
        )
        .await?
        .ok_or_else(|| anyhow!("entity {} vanished mid-import", row.id))?;
        Ok((updated.id, Outcome::Updated))
    } else {
        let inserted = entities::create(
            surreal,
            &NewEntity {
                name: name.to_string(),
                entity_type_id: entity_type.id,
                jurisdiction_id: jurisdiction.id,
                phone,
                url,
                xero_id: None,
                // An import must not be able to fork the firm's own row.
                // The shipped default stands in for the configured
                // anchor because this path has no configuration; a
                // white-label operator's firm is still protected by the
                // `entity_firm_anchor` index if it already carries a key.
                firm_anchor_key: store::entity_commands::firm_anchor_key(
                    store::seed::FIRM_ENTITY_NAME,
                    name,
                ),
            },
        )
        .await?;
        Ok((inserted.id, Outcome::Created))
    }
}

/// Find-or-create one `persons` row, keyed on the unique `email`. On a
/// re-import the payload is authoritative for `name`/`title`/`phone`,
/// but `role` is never touched — a person promoted to lawyer/admin stays
/// promoted. New rows take the database default `role` (`client`).
async fn upsert_person(
    surreal: &SurrealDb,
    record: &crate::contract::PersonRecord,
) -> anyhow::Result<(Uuid, Outcome)> {
    let email = record.email.trim();
    let name = record.name.trim();
    let title = clean(record.title.as_deref());
    let phone = clean(record.phone.as_deref());

    // Case-insensitive, matching `persons_email_lower_key`. A byte-exact
    // lookup would miss a stored `Attorney@Example.com` for an incoming
    // `attorney@example.com` and then fail the insert on that index, so a
    // re-import with different casing must resolve to the same row.
    let existing = persons::find_by_email_ci(surreal, email).await?;

    if let Some(row) = existing {
        // An absent value in the payload keeps what is already recorded:
        // a re-import that omits a phone must not erase one.
        let update = ContactUpdate {
            name: name.to_string(),
            title: title.or_else(|| row.title.clone()),
            phone: phone.or_else(|| row.phone.clone()),
        };
        if row.name == update.name && row.title == update.title && row.phone == update.phone {
            return Ok((row.id, Outcome::Unchanged));
        }
        persons::update_contact(surreal, row.id, &update).await?;
        Ok((row.id, Outcome::Updated))
    } else {
        let inserted = persons::create(
            surreal,
            &NewPerson {
                title,
                phone,
                ..NewPerson::new(name, email)
            },
        )
        .await?;
        Ok((inserted.id, Outcome::Created))
    }
}

/// Find-or-create the `entity_role` tie. Returns `Ok(())` whether the
/// tie already existed or was created.
///
/// The find-then-insert this used to hand-roll had no constraint behind
/// it; `store::entity_roles::grant` is the same contract with the
/// UNIQUE `entity_role_tie` index closing the race (ENG-120).
async fn link_person_entity(
    surreal: &SurrealDb,
    person_id: Uuid,
    entity_id: Uuid,
    role: &str,
) -> anyhow::Result<()> {
    store::entity_roles::grant(surreal, person_id, entity_id, role).await?;
    Ok(())
}

/// Trim an optional string and treat empty as absent.
fn clean(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::{ImportReport, Outcome, RowOutcome};
    use crate::validate::{Diagnostic, Severity};
    use uuid::Uuid;

    #[test]
    fn problem_lines_is_none_for_a_clean_report() {
        let report = ImportReport {
            diagnostics: Vec::new(),
            organizations: vec![RowOutcome {
                key: "njp".into(),
                status: Outcome::Created,
                id: Some(Uuid::nil()),
                detail: None,
            }],
            people: Vec::new(),
        };
        assert!(report.problem_lines().is_none());
    }

    #[test]
    fn problem_lines_surfaces_diagnostics_failures_and_notes() {
        // The exact shape Gemini Enterprise would otherwise drop: a
        // structural error, a warning, a failed row, and a created row
        // that still carries a note. All four must appear in the text.
        let report = ImportReport {
            diagnostics: vec![
                Diagnostic {
                    severity: Severity::Error,
                    pointer: "people[0].email".into(),
                    message: "`bob@` is not a valid email address".into(),
                },
                Diagnostic {
                    severity: Severity::Warning,
                    pointer: "organizations[0].url".into(),
                    message: "url canonicalized to `https://njp.org`".into(),
                },
            ],
            organizations: vec![RowOutcome {
                key: "njp".into(),
                status: Outcome::Failed,
                id: None,
                detail: Some("unknown jurisdiction code `XX`".into()),
            }],
            people: vec![RowOutcome {
                key: "abigail".into(),
                status: Outcome::Created,
                id: Some(Uuid::nil()),
                detail: Some("organization `njp` was not created; link skipped".into()),
            }],
        };
        let text = report.problem_lines().expect("problems present");
        assert!(
            text.contains("people[0].email (error): `bob@` is not a valid email address"),
            "missing email error: {text}"
        );
        assert!(
            text.contains("organizations[0].url (warning): url canonicalized"),
            "missing url warning: {text}"
        );
        assert!(
            text.contains("organization `njp` failed: unknown jurisdiction code `XX`"),
            "missing row failure: {text}"
        );
        assert!(
            text.contains("person `abigail`: organization `njp` was not created; link skipped"),
            "missing row note: {text}"
        );
    }
}
