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
/// The title property the reconciler writes and reads. It is the only
/// property a page is matched on, so the name is stated once.
const PROJECT_CODE_PROPERTY: &str = "Project code";
const NOTION_BASE_URL: &str = "https://api.notion.com/v1";
/// Notion caps `/search` at 100 results a page. The reconciler has to see
/// every page carrying a code to report a duplicate, so the search follows
/// the cursor instead of reading the first page and stopping.
const SEARCH_PAGE_SIZE: u32 = 100;
/// A Firm past this many search pages is a misconfigured parent database,
/// not a matter list. Bounded so a provider that never clears `has_more`
/// cannot spin a request forever.
const MAX_SEARCH_PAGES: usize = 20;

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
    /// Every page carrying this Project code, not just the first.
    ///
    /// Reconciliation distinguishes a missing page from a duplicated one, and
    /// a lookup that can only answer `Option` collapses those two into the
    /// same answer. The default delegates, so an adapter that genuinely has
    /// one page per code needs no extra work; the REST client overrides it.
    async fn list_private_pages(&self, project_code: &str) -> Result<Vec<NotionPage>, NotionError> {
        Ok(self
            .find_private_page(project_code)
            .await?
            .into_iter()
            .collect())
    }
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
#[derive(Debug, Clone, Default)]
pub struct FakeNotion {
    pages: Arc<Mutex<BTreeMap<String, NotionPage>>>,
    /// Extra pages carrying a code the fake did not create, so a test can
    /// drive the duplicate outcome the reconciler reports.
    duplicates: Arc<Mutex<BTreeMap<String, Vec<NotionPage>>>>,
    create_calls: Arc<Mutex<usize>>,
    update_calls: Arc<Mutex<usize>>,
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

    /// Seed a second page carrying `project_code`, as a Firm's workspace can
    /// hold when someone created one by hand beside the provisioned page.
    pub fn add_duplicate(&self, project_code: &str, page_id: &str) {
        self.duplicates
            .lock()
            .expect("Notion fake lock poisoned")
            .entry(project_code.to_string())
            .or_default()
            .push(NotionPage {
                id: page_id.to_string(),
                url: format!("https://notion.example/{page_id}"),
                project_code: project_code.to_string(),
            });
    }

    #[must_use]
    pub fn create_calls(&self) -> usize {
        *self.create_calls.lock().expect("Notion fake lock poisoned")
    }

    #[must_use]
    pub fn update_calls(&self) -> usize {
        *self.update_calls.lock().expect("Notion fake lock poisoned")
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

    async fn list_private_pages(&self, project_code: &str) -> Result<Vec<NotionPage>, NotionError> {
        self.check_available()?;
        let mut pages: Vec<NotionPage> = self
            .pages
            .lock()
            .expect("Notion fake lock poisoned")
            .get(project_code)
            .cloned()
            .into_iter()
            .collect();
        pages.extend(
            self.duplicates
                .lock()
                .expect("Notion fake lock poisoned")
                .get(project_code)
                .cloned()
                .unwrap_or_default(),
        );
        Ok(pages)
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
        *self
            .update_calls
            .lock()
            .expect("Notion fake lock poisoned") += 1;
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
    #[serde(default)]
    has_more: bool,
    #[serde(default)]
    next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PageResponse {
    id: String,
    url: String,
    #[serde(default)]
    properties: serde_json::Value,
}

/// Read the whole `Project code` title out of one search result.
///
/// The match has to be the assembled title compared for equality, not a
/// substring of the serialized properties: a code is a prefix of other codes
/// (`sample-project` of `sample-project-two`), and any property text can
/// contain it, so a substring test adopts another matter's page and the next
/// update writes this Project's code over it. Notion splits a title into rich
/// text runs, so the runs are joined before the comparison.
fn project_code_title(properties: &serde_json::Value) -> Option<String> {
    let runs = properties
        .get(PROJECT_CODE_PROPERTY)?
        .get("title")?
        .as_array()?;
    let mut title = String::new();
    for run in runs {
        let text = run
            .get("plain_text")
            .or_else(|| run.pointer("/text/content"))?
            .as_str()?;
        title.push_str(text);
    }
    Some(title)
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
    /// The production client. The credential is the Firm's resolved provider
    /// token, so there is no environment fallback to read here.
    #[must_use]
    pub fn new(token: impl Into<String>, parent_database_id: impl Into<String>) -> Self {
        Self::with_base_url(token, parent_database_id, NOTION_BASE_URL)
    }

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
        Ok(self
            .list_private_pages(project_code)
            .await?
            .into_iter()
            .next())
    }

    async fn list_private_pages(&self, project_code: &str) -> Result<Vec<NotionPage>, NotionError> {
        let mut matches = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_SEARCH_PAGES {
            let mut body = json!({
                "query": project_code,
                "page_size": SEARCH_PAGE_SIZE,
                "filter": { "property": "object", "value": "page" },
            });
            if let Some(cursor) = &cursor {
                body["start_cursor"] = json!(cursor);
            }
            let response = self
                .checked(
                    self.http
                        .post(format!("{}/search", self.base_url))
                        .json(&body),
                )
                .await?;
            let page = response
                .json::<SearchResponse>()
                .await
                .map_err(|_| NotionError::Transport)?;
            matches.extend(page.results.into_iter().filter_map(|result| {
                (project_code_title(&result.properties).as_deref() == Some(project_code)).then_some(
                    NotionPage {
                        id: result.id,
                        url: result.url,
                        project_code: project_code.to_string(),
                    },
                )
            }));
            match (page.has_more, page.next_cursor) {
                (true, Some(next)) => cursor = Some(next),
                _ => return Ok(matches),
            }
        }
        Ok(matches)
    }

    async fn create_private_page(&self, project_code: &str) -> Result<NotionPage, NotionError> {
        let response = self
            .checked(self.http.post(format!("{}/pages", self.base_url)).json(&json!({
                "parent": { "database_id": self.parent_database_id },
                "properties": { PROJECT_CODE_PROPERTY: { "title": [{ "text": { "content": project_code } }] } }
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
                "properties": { PROJECT_CODE_PROPERTY: { "title": [{ "text": { "content": project_code } }] } }
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
    use super::{
        ensure_private_page, project_code_title, FakeNotion, NotionError, NotionService,
        PROJECT_CODE_PROPERTY,
    };
    use serde_json::json;

    fn properties(title: &str) -> serde_json::Value {
        json!({
            PROJECT_CODE_PROPERTY: {
                "type": "title",
                "title": [{ "type": "text", "plain_text": title }]
            },
            "Notes": { "rich_text": [{ "plain_text": "sample-project overview" }] }
        })
    }

    #[test]
    fn a_page_matches_only_its_whole_project_code_title() {
        assert_eq!(
            project_code_title(&properties("sample-project")).as_deref(),
            Some("sample-project")
        );
        // Both of these contain `sample-project` somewhere in their
        // properties, and neither is that Project's page.
        assert_eq!(
            project_code_title(&properties("sample-project-two")).as_deref(),
            Some("sample-project-two")
        );
        assert_eq!(
            project_code_title(&json!({ "Notes": { "rich_text": [] } })),
            None
        );
    }

    /// The reconciler reports a duplicate only if the search saw both pages,
    /// and Notion caps a page of results. The second copy lives on the second
    /// page here, so a client that read one page and stopped would report the
    /// Firm's workspace as clean.
    #[tokio::test]
    async fn the_search_follows_the_cursor_so_a_duplicate_is_visible() {
        use super::{NotionClient, PROJECT_CODE_PROPERTY};
        use wiremock::matchers::{body_string_contains, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        fn result(id: &str, title: &str) -> serde_json::Value {
            json!({
                "id": id,
                "url": format!("https://notion.example/{id}"),
                "properties": { PROJECT_CODE_PROPERTY: { "title": [{ "plain_text": title }] } }
            })
        }

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .and(body_string_contains("start_cursor"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "results": [result("page-2", "sample-project")],
                "has_more": false,
                "next_cursor": null
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "results": [
                    result("page-1", "sample-project"),
                    result("page-9", "sample-project-two")
                ],
                "has_more": true,
                "next_cursor": "cursor-2"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = NotionClient::with_base_url("test-token", "db-1", server.uri());
        let pages = client.list_private_pages("sample-project").await.unwrap();
        assert_eq!(
            pages
                .iter()
                .map(|page| page.id.as_str())
                .collect::<Vec<_>>(),
            ["page-1", "page-2"],
            "both copies are seen, and the prefix match is not"
        );
    }

    #[test]
    fn a_split_title_is_joined_before_the_comparison() {
        let split = json!({
            PROJECT_CODE_PROPERTY: {
                "title": [
                    { "plain_text": "sample-" },
                    { "plain_text": "project" }
                ]
            }
        });
        assert_eq!(
            project_code_title(&split).as_deref(),
            Some("sample-project")
        );
    }

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
