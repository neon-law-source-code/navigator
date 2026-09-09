//! Pure reconciliation decisions for the environment-selected Notion DB.
//!
//! This module never guesses through a stale URL. Missing, duplicate,
//! moved, deleted, and unshared pages become explicit operator outcomes, and
//! manual fields are carried as a preserve instruction rather than replaced.

use std::collections::BTreeMap;

use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotionPageSnapshot {
    pub id: String,
    pub url: String,
    pub project_code: String,
    pub person_ids: Vec<Uuid>,
    pub manual_fields: BTreeMap<String, String>,
    pub accessible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotionProjectInput {
    pub project_code: String,
    pub canonical_url: String,
    pub person_ids: Vec<Uuid>,
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
        person_ids: Vec<Uuid>,
        preserve_manual_fields: bool,
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
    if page.url == input.canonical_url && page.person_ids == input.person_ids {
        NotionRepairDecision::Unchanged {
            page_id: page.id.clone(),
        }
    } else {
        NotionRepairDecision::Repair {
            page_id: page.id.clone(),
            canonical_url: input.canonical_url.clone(),
            person_ids: input.person_ids.clone(),
            preserve_manual_fields: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        reconcile, NotionDatabaseConfig, NotionPageSnapshot, NotionProjectInput,
        NotionReconcileError, NotionRepairDecision,
    };
    use std::collections::BTreeMap;
    use uuid::Uuid;

    fn input() -> NotionProjectInput {
        NotionProjectInput {
            project_code: "sample-project".to_string(),
            canonical_url: "https://notion.example/sample-project".to_string(),
            person_ids: vec![Uuid::from_u128(7)],
        }
    }

    fn page(code: &str, url: &str) -> NotionPageSnapshot {
        NotionPageSnapshot {
            id: "page-1".to_string(),
            url: url.to_string(),
            project_code: code.to_string(),
            person_ids: vec![Uuid::from_u128(7)],
            manual_fields: BTreeMap::from([("status".to_string(), "manual".to_string())]),
            accessible: true,
        }
    }

    #[test]
    fn reconcile_repairs_moved_page_and_preserves_manual_fields() {
        assert_eq!(
            reconcile(
                &input(),
                &[page("sample-project", "https://notion.example/moved")]
            ),
            NotionRepairDecision::Repair {
                page_id: "page-1".to_string(),
                canonical_url: "https://notion.example/sample-project".to_string(),
                person_ids: vec![Uuid::from_u128(7)],
                preserve_manual_fields: true,
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
