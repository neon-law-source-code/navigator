//! Pure reconciliation decisions for the environment-selected Notion DB.
//!
//! This module never guesses through a stale URL. Missing, duplicate,
//! moved, renamed, and archived pages become explicit operator outcomes.
//!
//! The only Navigator-declared fact this reconciler compares is the page's
//! canonical URL. It deliberately does not carry a "shared people" or
//! "manual fields" axis: Notion page sharing is out of scope for the
//! provider-sync surface this feeds (ENG-807 excludes invitations and
//! content mirroring), so there is no honest source to compare against yet,
//! and a field that can never disagree is worse than no field — it is a
//! silent claim of coverage the reconciler does not have. A repair only ever
//! patches the title property (see `cloud::notion::NotionClient::
//! update_private_page`), so every other property on the page — anything a
//! human added by hand — is preserved structurally, not by an instruction
//! carried in this decision.

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotionPageSnapshot {
    pub id: String,
    pub url: String,
    pub project_code: String,
    /// Whether this specific recorded page still resolves and is not
    /// archived — real once produced from [`crate::NotionService::get_page`],
    /// see the caller in `portal::integrations_api`.
    pub accessible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotionProjectInput {
    pub project_code: String,
    pub canonical_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotionRepairDecision {
    Missing,
    Duplicate {
        page_ids: Vec<String>,
    },
    Conflict {
        page_id: String,
        observed_code: String,
    },
    Unavailable {
        page_id: String,
    },
    Unchanged {
        page_id: String,
    },
    Repair {
        page_id: String,
        canonical_url: String,
    },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum NotionReconcileError {
    #[error("Notion database coordinate is missing")]
    MissingDatabase,
    #[error("Notion database coordinate is not a valid opaque id")]
    InvalidDatabase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotionDatabaseConfig {
    pub database_id: String,
}

impl NotionDatabaseConfig {
    pub fn from_lookup<F>(get: F) -> Result<Self, NotionReconcileError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let database_id = get("NAVIGATOR_NOTION_DATABASE_ID")
            .ok_or(NotionReconcileError::MissingDatabase)?
            .trim()
            .to_string();
        if database_id.is_empty()
            || !database_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(NotionReconcileError::InvalidDatabase);
        }
        Ok(Self { database_id })
    }
}

#[must_use]
pub fn reconcile(input: &NotionProjectInput, pages: &[NotionPageSnapshot]) -> NotionRepairDecision {
    if pages.is_empty() {
        return NotionRepairDecision::Missing;
    }
    if pages.len() > 1 {
        return NotionRepairDecision::Duplicate {
            page_ids: pages.iter().map(|page| page.id.clone()).collect(),
        };
    }
    let page = &pages[0];
    if page.project_code != input.project_code {
        return NotionRepairDecision::Conflict {
            page_id: page.id.clone(),
            observed_code: page.project_code.clone(),
        };
    }
    if !page.accessible {
        return NotionRepairDecision::Unavailable {
            page_id: page.id.clone(),
        };
    }
    if page.url == input.canonical_url {
        NotionRepairDecision::Unchanged {
            page_id: page.id.clone(),
        }
    } else {
        NotionRepairDecision::Repair {
            page_id: page.id.clone(),
            canonical_url: input.canonical_url.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        reconcile, NotionDatabaseConfig, NotionPageSnapshot, NotionProjectInput,
        NotionReconcileError, NotionRepairDecision,
    };

    fn input() -> NotionProjectInput {
        NotionProjectInput {
            project_code: "sample-project".to_string(),
            canonical_url: "https://notion.example/sample-project".to_string(),
        }
    }

    fn page(code: &str, url: &str) -> NotionPageSnapshot {
        NotionPageSnapshot {
            id: "page-1".to_string(),
            url: url.to_string(),
            project_code: code.to_string(),
            accessible: true,
        }
    }

    #[test]
    fn reconcile_repairs_a_moved_page() {
        assert_eq!(
            reconcile(
                &input(),
                &[page("sample-project", "https://notion.example/moved")]
            ),
            NotionRepairDecision::Repair {
                page_id: "page-1".to_string(),
                canonical_url: "https://notion.example/sample-project".to_string(),
            }
        );
    }

    #[test]
    fn reconcile_reports_duplicates_and_unshared_pages() {
        let mut second = page("sample-project", "https://notion.example/second");
        second.id = "page-2".to_string();
        assert!(matches!(
            reconcile(&input(), &[page("sample-project", "a"), second]),
            NotionRepairDecision::Duplicate { .. }
        ));
        let mut unavailable = page("sample-project", "a");
        unavailable.accessible = false;
        assert_eq!(
            reconcile(&input(), &[unavailable]),
            NotionRepairDecision::Unavailable {
                page_id: "page-1".to_string()
            }
        );
    }

    /// A caller building the snapshot straight from a title search can never
    /// observe this — the title *is* the query. It is reachable only when
    /// the caller looks up the specific recorded id directly and finds its
    /// current title has drifted away from the code Navigator still records.
    #[test]
    fn reconcile_reports_a_renamed_page_as_conflict() {
        assert_eq!(
            reconcile(&input(), &[page("someone-renamed-this", "a")]),
            NotionRepairDecision::Conflict {
                page_id: "page-1".to_string(),
                observed_code: "someone-renamed-this".to_string(),
            }
        );
    }

    #[test]
    fn database_selection_is_explicit_and_environment_free_of_tokens() {
        let config = NotionDatabaseConfig::from_lookup(|key| {
            (key == "NAVIGATOR_NOTION_DATABASE_ID").then(|| "staging-db".to_string())
        })
        .unwrap();
        assert_eq!(config.database_id, "staging-db");
        assert_eq!(
            NotionDatabaseConfig::from_lookup(|_| None),
            Err(NotionReconcileError::MissingDatabase)
        );
    }
}
