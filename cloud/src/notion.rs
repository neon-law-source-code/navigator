//! Firm-private Notion resource adapter.
//!
//! The adapter knows how to create, find, and update a private page. It does
//! not expose the page to a client-facing payload; callers decide explicitly
//! which internal resource coordinates may cross that boundary.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use thiserror::Error;

const NOTION_VERSION: &str = "2022-06-28";

#[derive(Debug, Error)]
pub enum NotionError {
    #[error("Notion credential is unavailable")]
    MissingCredential,
    #[error("Notion request failed")]
    Transport,
    #[error("Notion returned HTTP {0}")]
    HttpStatus(u16),
    #[error("Notion rejected the request")]
    Api,
    #[error("Notion returned an incomplete page")]
    IncompleteResponse,
}

/// The internal page coordinate returned by Notion. It is intentionally not
/// serializable as part of a client project response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotionPage {
    pub id: String,
    pub url: String,
    pub project_code: String,
}

#[async_trait]
pub trait NotionService: Send + Sync {
    async fn find_private_page(
        &self,
        project_code: &str,
    ) -> Result<Option<NotionPage>, NotionError>;
    async fn create_private_page(&self, project_code: &str) -> Result<NotionPage, NotionError>;
    async fn update_private_page(
        &self,
        page_id: &str,
        project_code: &str,
    ) -> Result<NotionPage, NotionError>;
}

/// Find-then-create is the idempotency boundary. A failed lookup never falls
/// through to create, so a transient Notion outage cannot make a duplicate.
pub async fn ensure_private_page<S: NotionService + ?Sized>(
    service: &S,
    project_code: &str,
) -> Result<(NotionPage, bool), NotionError> {
    if let Some(page) = service.find_private_page(project_code).await? {
        return Ok((page, false));
    }
    Ok((service.create_private_page(project_code).await?, true))
}

/// Deterministic fake used by provider and durable-workflow tests.
#[derive(Clone, Default)]
pub struct FakeNotion {
    pages: Arc<Mutex<BTreeMap<String, NotionPage>>>,
    create_calls: Arc<Mutex<usize>>,
    unavailable: Arc<Mutex<bool>>,
}

impl FakeNotion {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_unavailable(&self, unavailable: bool) {
        *self.unavailable.lock().expect("Notion fake lock poisoned") = unavailable;
    }

    #[must_use]
    pub fn create_calls(&self) -> usize {
        *self.create_calls.lock().expect("Notion fake lock poisoned")
    }

    #[must_use]
    pub fn page(&self, project_code: &str) -> Option<NotionPage> {
        self.pages
            .lock()
            .expect("Notion fake lock poisoned")
            .get(project_code)
            .cloned()
    }

    fn check_available(&self) -> Result<(), NotionError> {
        if *self.unavailable.lock().expect("Notion fake lock poisoned") {
            Err(NotionError::Transport)
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl NotionService for FakeNotion {
    async fn find_private_page(
        &self,
        project_code: &str,
    ) -> Result<Option<NotionPage>, NotionError> {
        self.check_available()?;
        Ok(self
            .pages
            .lock()
            .expect("Notion fake lock poisoned")
            .get(project_code)
            .cloned())
    }

    async fn create_private_page(&self, project_code: &str) -> Result<NotionPage, NotionError> {
        self.check_available()?;
        let mut pages = self.pages.lock().expect("Notion fake lock poisoned");
        if let Some(page) = pages.get(project_code) {
            return Ok(page.clone());
        }
        *self.create_calls.lock().expect("Notion fake lock poisoned") += 1;
        let page = NotionPage {
            id: format!("page-{project_code}"),
            url: format!("https://notion.example/{project_code}"),
            project_code: project_code.to_string(),
        };
        pages.insert(project_code.to_string(), page.clone());
        Ok(page)
    }

    async fn update_private_page(
        &self,
        page_id: &str,
        project_code: &str,
    ) -> Result<NotionPage, NotionError> {
        self.check_available()?;
        let page = NotionPage {
            id: page_id.to_string(),
            url: format!("https://notion.example/{project_code}"),
            project_code: project_code.to_string(),
        };
        self.pages
            .lock()
            .expect("Notion fake lock poisoned")
            .insert(project_code.to_string(), page.clone());
        Ok(page)
    }
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<PageResponse>,
}

#[derive(Debug, Deserialize)]
struct PageResponse {
    id: String,
    url: String,
    #[serde(default)]
    properties: serde_json::Value,
}

/// Notion REST adapter. The credential is injected by the Firm-secret
/// resolver; there is no environment fallback or ambient provider login.
pub struct NotionClient {
    http: reqwest::Client,
    base_url: String,
    token: String,
    parent_database_id: String,
}

impl std::fmt::Debug for NotionClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NotionClient")
            .field("base_url", &self.base_url)
            .field("parent_database_id", &self.parent_database_id)
            .finish_non_exhaustive()
    }
}

impl NotionClient {
    #[must_use]
    pub fn with_base_url(
        token: impl Into<String>,
        parent_database_id: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token: token.into(),
            parent_database_id: parent_database_id.into(),
        }
    }

    async fn checked(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, NotionError> {
        let response = request
            .bearer_auth(&self.token)
            .header("Notion-Version", NOTION_VERSION)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .send()
            .await
            .map_err(|_| NotionError::Transport)?;
        if response.status().is_success() {
            Ok(response)
        } else {
            Err(NotionError::HttpStatus(response.status().as_u16()))
        }
    }
}

#[async_trait]
impl NotionService for NotionClient {
    async fn find_private_page(
        &self,
        project_code: &str,
    ) -> Result<Option<NotionPage>, NotionError> {
        let response = self
            .checked(
                self.http
                    .post(format!("{}/search", self.base_url))
                    .json(&json!({
                        "query": project_code,
                        "page_size": 100,
                    })),
            )
            .await?;
        let body = response
            .json::<SearchResponse>()
            .await
            .map_err(|_| NotionError::Transport)?;
        Ok(body.results.into_iter().find_map(|page| {
            let matches_code = page.properties.to_string().contains(project_code);
            matches_code.then_some(NotionPage {
                id: page.id,
                url: page.url,
                project_code: project_code.to_string(),
            })
        }))
    }

    async fn create_private_page(&self, project_code: &str) -> Result<NotionPage, NotionError> {
        let response = self
            .checked(self.http.post(format!("{}/pages", self.base_url)).json(&json!({
                "parent": { "database_id": self.parent_database_id },
                "properties": { "Project code": { "title": [{ "text": { "content": project_code } }] } }
            })))
            .await?;
        let page = response
            .json::<PageResponse>()
            .await
            .map_err(|_| NotionError::Transport)?;
        Ok(NotionPage {
            id: page.id,
            url: page.url,
            project_code: project_code.to_string(),
        })
    }

    async fn update_private_page(
        &self,
        page_id: &str,
        project_code: &str,
    ) -> Result<NotionPage, NotionError> {
        let response = self
            .checked(self.http.patch(format!("{}/pages/{page_id}", self.base_url)).json(&json!({
                "properties": { "Project code": { "title": [{ "text": { "content": project_code } }] } }
            })))
            .await?;
        let page = response
            .json::<PageResponse>()
            .await
            .map_err(|_| NotionError::Transport)?;
        Ok(NotionPage {
            id: page.id,
            url: page.url,
            project_code: project_code.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ensure_private_page, FakeNotion, NotionError, NotionService};

    #[tokio::test]
    async fn ensure_adopts_without_duplicate_creation() {
        let notion = FakeNotion::new();
        let (first, created) = ensure_private_page(&notion, "sample-project")
            .await
            .unwrap();
        let (second, created_again) = ensure_private_page(&notion, "sample-project")
            .await
            .unwrap();
        assert!(created);
        assert!(!created_again);
        assert_eq!(first, second);
        assert_eq!(notion.create_calls(), 1);
    }

    #[tokio::test]
    async fn unavailable_lookup_does_not_create_a_duplicate() {
        let notion = FakeNotion::new();
        notion.set_unavailable(true);
        assert!(matches!(
            ensure_private_page(&notion, "sample-project").await,
            Err(NotionError::Transport)
        ));
        assert_eq!(notion.create_calls(), 0);
    }

    #[tokio::test]
    async fn updates_preserve_the_internal_page_coordinate() {
        let notion = FakeNotion::new();
        let page = notion
            .update_private_page("page-1", "sample-project")
            .await
            .unwrap();
        assert_eq!(page.id, "page-1");
        assert_eq!(notion.page("sample-project"), Some(page));
    }
}
