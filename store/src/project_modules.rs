//! `store::project_modules` — the per-matter capability ledger (#684).
//!
//! Every Project opens as a blank slate and gains practice-area
//! capability through modules lawyers enable per matter. One engagement can
//! run litigation *and* a cap table at once, which is why this is a
//! ledger and not a type column.
//!
//! **Presence of a row is the enabled state.** Disabling deletes it.
//! There is no `enabled` flag and no `disabled_at`, so "is this module
//! on" has exactly one answer and a disabled module has no row for a
//! query to find — which is what makes the client lens toggle-blind by
//! construction rather than by remembering to filter.

use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::projects;
use crate::relationship_logs::{self, NewRelationshipLog};
use crate::surreal::{record_id, record_uuid, SurrealDb};

/// A per-matter capability.
///
/// **Closed by design.** A new capability is a deliberate addition here,
/// with a migration to widen the database `CHECK`, rather than a
/// free-text value a call site can invent. `contract_review` and the
/// other candidates in #683 join this enum when they are built, not
/// before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Module {
    /// Cases and their per-stage records — many cases per matter.
    Litigation,
    /// The entity cap table surfaced in matter context. A lens over
    /// entity-scoped data, not project-scoped data this module owns.
    CapTable,
    /// The estate plan as an explicit capability, replacing the
    /// workflow-sniffing inference it supersedes (#685).
    Estate,
    /// Statutory and firm deadlines for the matter. Distinct from the
    /// litigation case docket, which belongs to [`Module::Litigation`].
    Deadlines,
}

impl Module {
    /// Every module, in declaration order.
    pub const ALL: &'static [Module] = &[
        Module::Litigation,
        Module::CapTable,
        Module::Estate,
        Module::Deadlines,
    ];

    /// The stored string.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Module::Litigation => "litigation",
            Module::CapTable => "cap_table",
            Module::Estate => "estate",
            Module::Deadlines => "deadlines",
        }
    }

    /// Parse a stored value, or `None` when it is outside the closed set.
    #[must_use]
    pub fn parse(value: &str) -> Option<Module> {
        Self::ALL.iter().copied().find(|m| m.as_str() == value)
    }
}

/// Errors from the ledger commands.
#[derive(Debug, thiserror::Error)]
pub enum ModuleError {
    /// A stored value is outside [`Module`], which means a row was written
    /// around the `CHECK`.
    #[error("`{0}` is not a recognized module")]
    UnknownModule(String),
    #[error("SurrealDB: {0}")]
    Surreal(#[from] surrealdb::Error),
    #[error("audit trail: {0}")]
    Audit(#[from] crate::relationship_logs::RelationshipLogError),
    #[error(transparent)]
    Project(#[from] projects::ProjectStoreError),
    #[error("writing a project module returned no usable row")]
    WriteReturnedNothing,
}

/// One enabled capability in the SurrealDB project-module ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectModule {
    pub id: Uuid,
    pub project_id: Uuid,
    pub module: String,
    pub enabled_at: String,
    pub enabled_by_person_id: Option<Uuid>,
    pub inserted_at: String,
    pub updated_at: String,
}

#[derive(SurrealValue)]
struct ProjectModuleRow {
    id: surrealdb::types::RecordId,
    project_id: surrealdb::types::RecordId,
    module: String,
    enabled_at: String,
    enabled_by_person_id: Option<surrealdb::types::RecordId>,
    inserted_at: String,
    updated_at: String,
}

impl ProjectModuleRow {
    fn into_project_module(self) -> Option<ProjectModule> {
        Some(ProjectModule {
            id: record_uuid(&self.id)?,
            project_id: record_uuid(&self.project_id)?,
            module: self.module,
            enabled_at: self.enabled_at,
            enabled_by_person_id: self.enabled_by_person_id.as_ref().and_then(record_uuid),
            inserted_at: self.inserted_at,
            updated_at: self.updated_at,
        })
    }
}

const SELECT: &str =
    "id, project_id, module, enabled_at, enabled_by_person_id, inserted_at, updated_at";

/// Enable `module` on `project_id`, recording who did it.
///
/// Idempotent: enabling an already-enabled module returns the existing row
/// and writes no second audit entry, so a double-submitted lawyer form does
/// not manufacture a toggle that did not happen.
///
/// The ledger state and its relationship audit are both written in
/// SurrealDB, on the one handle.
///
/// # Errors
/// Propagates any database error.
pub async fn enable(
    surreal: &SurrealDb,
    project_id: Uuid,
    module: Module,
    actor_person_id: Option<Uuid>,
) -> Result<ProjectModule, ModuleError> {
    if let Some(existing) = find(surreal, project_id, module).await? {
        return Ok(existing);
    }

    if projects::find_by_id(surreal, project_id).await?.is_none() {
        return Err(projects::ProjectStoreError::NoSuchProject(project_id).into());
    }

    let now = chrono::Utc::now().to_rfc3339();
    let written = surreal
        .query(format!(
            "CREATE $id SET project_id = $project_id, module = $module, enabled_at = $now, \
             enabled_by_person_id = $actor, inserted_at = $now, updated_at = $now RETURN {SELECT}"
        ))
        .bind(("id", record_id("project_module", Uuid::now_v7())))
        .bind(("project_id", record_id("project", project_id)))
        .bind(("module", module.as_str().to_string()))
        .bind(("actor", actor_person_id.map(|id| record_id("person", id))))
        .bind(("now", now))
        .await
        .and_then(surrealdb::IndexedResults::check);
    let row = match written {
        Ok(mut response) => {
            let row: Option<ProjectModuleRow> = response.take(0)?;
            row.and_then(ProjectModuleRow::into_project_module)
                .ok_or(ModuleError::WriteReturnedNothing)?
        }
        Err(error)
            if crate::surreal::retry::unique_violation(&error) == Some("project_module_pair") =>
        {
            find(surreal, project_id, module)
                .await?
                .ok_or(ModuleError::WriteReturnedNothing)?
        }
        Err(error) => return Err(error.into()),
    };

    log_toggle(
        surreal,
        project_id,
        module,
        actor_person_id,
        "module_enabled",
    )
    .await?;
    Ok(row)
}

/// Disable `module` on `project_id`, recording who did it.
///
/// Deletes the ledger row. **Module-owned data is untouched** — disabling
/// litigation hides the capability, it does not destroy the cases — so
/// re-enabling restores the matter to where it was.
///
/// Returns `true` when a row was removed and `false` when the module was
/// already off. An already-off module writes no audit entry, for the same
/// reason enabling twice does not.
///
/// # Errors
/// Propagates any database error.
pub async fn disable(
    surreal: &SurrealDb,
    project_id: Uuid,
    module: Module,
    actor_person_id: Option<Uuid>,
) -> Result<bool, ModuleError> {
    let Some(existing) = find(surreal, project_id, module).await? else {
        return Ok(false);
    };

    surreal
        .query("DELETE $id")
        .bind(("id", record_id("project_module", existing.id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    log_toggle(
        surreal,
        project_id,
        module,
        actor_person_id,
        "module_disabled",
    )
    .await?;
    Ok(true)
}

/// Every module enabled on `project_id`.
///
/// Returns only what is **on**. There is no "all modules with their
/// state" read here on purpose: a caller that received the full set and
/// filtered would be one refactor away from shipping the disabled list to
/// a client, which is precisely what the toggle-blindness rule forbids.
/// A lawyer management surface composes the off-modules by differencing
/// this against [`Module::ALL`] on the server.
///
/// # Errors
/// [`ModuleError::UnknownModule`] if a stored value is outside the closed
/// set, or a database error.
pub async fn list_for_project(
    surreal: &SurrealDb,
    project_id: Uuid,
) -> Result<Vec<Module>, ModuleError> {
    let mut response = surreal
        .query(format!(
            "SELECT {SELECT} FROM project_module WHERE project_id = $project_id ORDER BY module"
        ))
        .bind(("project_id", record_id("project", project_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<ProjectModuleRow> = response.take(0)?;
    rows.iter()
        .map(|r| {
            Module::parse(&r.module).ok_or_else(|| ModuleError::UnknownModule(r.module.clone()))
        })
        .collect()
}

/// True when `module` is enabled on `project_id`.
///
/// # Errors
/// Propagates any database error.
pub async fn is_enabled(
    surreal: &SurrealDb,
    project_id: Uuid,
    module: Module,
) -> Result<bool, ModuleError> {
    Ok(find(surreal, project_id, module).await?.is_some())
}

async fn find(
    surreal: &SurrealDb,
    project_id: Uuid,
    module: Module,
) -> Result<Option<ProjectModule>, ModuleError> {
    let mut response = surreal
        .query(format!(
            "SELECT {SELECT} FROM ONLY project_module WHERE project_id = $project_id AND module = $module LIMIT 1"
        ))
        .bind(("project_id", record_id("project", project_id)))
        .bind(("module", module.as_str().to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<ProjectModuleRow> = response.take(0)?;
    Ok(row.and_then(ProjectModuleRow::into_project_module))
}

/// Append the audit entry for a toggle.
///
/// The entry and the module row it describes are written to one engine,
/// so a toggle cannot land with its trail missing, nor a trail with its
/// toggle missing.
async fn log_toggle(
    surreal: &SurrealDb,
    project_id: Uuid,
    module: Module,
    actor_person_id: Option<Uuid>,
    action: &str,
) -> Result<(), relationship_logs::RelationshipLogError> {
    relationship_logs::record(
        surreal,
        &NewRelationshipLog {
            actor_person_id,
            subject_type: "project".to_string(),
            subject_id: project_id,
            action: action.to_string(),
            detail: module.as_str().to_string(),
        },
    )
    .await
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::Module;

    #[test]
    fn every_module_round_trips() {
        for module in Module::ALL {
            assert_eq!(Module::parse(module.as_str()), Some(*module));
        }
    }

    #[test]
    fn the_set_is_closed() {
        // Candidates named in #683 are deliberately absent until built.
        assert_eq!(Module::parse("contract_review"), None);
        assert_eq!(
            Module::parse("docket"),
            None,
            "docket belongs to litigation"
        );
        assert_eq!(Module::ALL.len(), 4);
    }
}
