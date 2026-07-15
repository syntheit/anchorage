//! API layer — a thin, UI-friendly wrapper over the `linkding-rs` async client.
//!
//! `linkding-rs` gives us the transport and types; this module adds:
//!  * a cloneable [`Client`] handle we can share across UI callbacks,
//!  * convenience constructors for the list/create/update bodies,
//!  * an [`ApiError`] with a `Display` string suitable for toasts, and
//!  * a `verify()` call used during onboarding to validate URL + token.
//!
//! Everything here is `async` and must run on the tokio runtime (see
//! [`crate::runtime`]); the returned futures are `Send`.

use std::sync::Arc;

use linkding_rs::{
    Bookmark, CreateBookmarkBody, LinkDingAsyncClient, LinkDingError, ListBookmarksArgs,
    ListTagsArgs, TagData, UpdateBookmarkBody,
};

/// Default page size for list requests.
pub const PAGE_LIMIT: i32 = 100;

/// An owned, cloneable view of a bookmark for the UI layer.
///
/// `linkding_rs::Bookmark` deliberately doesn't derive `Clone`, and coupling the
/// UI to the wire type is brittle. This carries exactly what the views need and
/// is trivially `Clone` (useful for storing in per-row state). Reuse this pattern
/// for the Memos sibling.
#[derive(Clone, Debug)]
pub struct BookmarkView {
    pub id: i32,
    pub url: String,
    pub title: String,
    pub description: String,
    pub notes: String,
    pub tag_names: Vec<String>,
    pub website_title: Option<String>,
    pub website_description: Option<String>,
    pub unread: bool,
    pub shared: bool,
    pub is_archived: bool,
    pub date_added: String,
}

impl From<Bookmark> for BookmarkView {
    fn from(b: Bookmark) -> Self {
        BookmarkView {
            id: b.id,
            url: b.url,
            title: b.title,
            description: b.description,
            notes: b.notes,
            tag_names: b.tag_names,
            website_title: b.website_title,
            website_description: b.website_description,
            unread: b.unread,
            shared: b.shared,
            is_archived: b.is_archived,
            date_added: b.date_added,
        }
    }
}

/// A page of bookmarks with the total count (for pagination).
#[derive(Clone, Debug)]
pub struct Page {
    pub total: i32,
    pub items: Vec<BookmarkView>,
}

/// The result of checking a URL: the existing bookmark (if any), scraped
/// title/description, and suggested tags.
#[derive(Clone, Debug, Default)]
pub struct CheckResult {
    pub existing: Option<BookmarkView>,
    pub scraped_title: Option<String>,
    pub scraped_description: Option<String>,
    pub suggested_tags: Vec<String>,
}

/// A shareable Linkding client handle. Cheap to clone (`Arc` inside).
#[derive(Clone)]
pub struct Client {
    inner: Arc<LinkDingAsyncClient>,
    base_url: Arc<str>,
}

/// Errors surfaced to the UI. We keep a human string because `linkding-rs`
/// collapses HTTP status codes into a single `reqwest::Error` variant.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    Message(String),
}

impl From<LinkDingError> for ApiError {
    fn from(err: LinkDingError) -> Self {
        // Try to extract a useful hint (auth vs. network) from the reqwest error.
        let msg = match &err {
            LinkDingError::SendHttpError(e) => {
                if let Some(status) = e.status() {
                    match status.as_u16() {
                        401 | 403 => "Authentication failed — check the API token".to_string(),
                        404 => "Not found — check the server URL".to_string(),
                        code => format!("Server returned HTTP {code}"),
                    }
                } else if e.is_connect() || e.is_timeout() {
                    "Could not reach the server — check the URL and your network".to_string()
                } else {
                    format!("Request failed: {e}")
                }
            }
            other => other.to_string(),
        };
        ApiError::Message(msg)
    }
}

impl Client {
    /// Build a client for `url` authenticated with `token`. Infallible — errors
    /// surface per request.
    pub fn new(url: &str, token: &str) -> Self {
        let trimmed = url.trim_end_matches('/');
        Client {
            inner: Arc::new(LinkDingAsyncClient::new(trimmed, token)),
            base_url: Arc::from(trimmed),
        }
    }

    /// The base server URL, for display and building web links.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Validate the credentials by making a minimal authenticated request.
    /// Used by onboarding/settings before persisting.
    pub async fn verify(&self) -> Result<(), ApiError> {
        let args = ListBookmarksArgs {
            limit: Some(1),
            ..Default::default()
        };
        self.inner.list_bookmarks(args).await?;
        Ok(())
    }

    /// List bookmarks with an optional search query and pagination.
    pub async fn list(&self, query: Option<String>, offset: i32) -> Result<Page, ApiError> {
        let args = ListBookmarksArgs {
            query: query.filter(|q| !q.trim().is_empty()),
            limit: Some(PAGE_LIMIT),
            offset: Some(offset),
            ..Default::default()
        };
        let resp = self.inner.list_bookmarks(args).await?;
        Ok(into_page(resp.count, resp.results))
    }

    /// List archived bookmarks with the same query/pagination semantics.
    pub async fn list_archived(&self, query: Option<String>, offset: i32) -> Result<Page, ApiError> {
        let args = ListBookmarksArgs {
            query: query.filter(|q| !q.trim().is_empty()),
            limit: Some(PAGE_LIMIT),
            offset: Some(offset),
            ..Default::default()
        };
        let resp = self.inner.list_archived_bookmarks(args).await?;
        Ok(into_page(resp.count, resp.results))
    }

    /// Check a URL: returns any existing bookmark, scraped metadata and
    /// suggested tags. Powers the add-sheet auto-fill.
    pub async fn check(&self, url: &str) -> Result<CheckResult, ApiError> {
        let resp = self.inner.check_url(url).await?;
        Ok(CheckResult {
            existing: resp.bookmark.map(BookmarkView::from),
            scraped_title: resp.metadata.title,
            scraped_description: resp.metadata.description,
            suggested_tags: resp.auto_tags,
        })
    }

    /// Create (or, per Linkding's upsert-by-URL behaviour, update) a bookmark.
    /// Leaving `title`/`description` empty triggers server-side scraping.
    pub async fn create(&self, draft: BookmarkDraft) -> Result<BookmarkView, ApiError> {
        let created = self.inner.create_bookmark(draft.into_create_body()).await?;
        Ok(BookmarkView::from(created))
    }

    /// Patch an existing bookmark. Only `Some(_)` fields are sent.
    pub async fn update(&self, id: i32, draft: BookmarkDraft) -> Result<BookmarkView, ApiError> {
        let updated = self.inner.update_bookmark(id, draft.into_update_body()).await?;
        Ok(BookmarkView::from(updated))
    }

    /// Delete a bookmark by id.
    pub async fn delete(&self, id: i32) -> Result<(), ApiError> {
        self.inner.delete_bookmark(id).await?;
        Ok(())
    }

    /// Archive a bookmark.
    pub async fn archive(&self, id: i32) -> Result<(), ApiError> {
        self.inner.archive_bookmark(id).await?;
        Ok(())
    }

    /// Unarchive a bookmark.
    pub async fn unarchive(&self, id: i32) -> Result<(), ApiError> {
        self.inner.unarchive_bookmark(id).await?;
        Ok(())
    }

    /// List all tags (first page only; Linkding installs rarely exceed 100 tags,
    /// but callers can page via `offset` if needed).
    pub async fn tags(&self, offset: i32) -> Result<Vec<TagData>, ApiError> {
        let args = ListTagsArgs {
            limit: Some(PAGE_LIMIT),
            offset: Some(offset),
        };
        Ok(self.inner.list_tags(args).await?.results)
    }
}

/// A user-editable bookmark draft shared by the create and edit flows.
///
/// `title`/`description` use `Option<String>`: `None` (or empty after trimming)
/// means "let the server scrape it".
#[derive(Clone, Debug, Default)]
pub struct BookmarkDraft {
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub notes: Option<String>,
    pub tag_names: Vec<String>,
    pub unread: bool,
    pub shared: bool,
}

impl BookmarkDraft {
    fn into_create_body(self) -> CreateBookmarkBody {
        CreateBookmarkBody {
            url: self.url,
            title: self.title,
            description: self.description,
            notes: self.notes,
            tag_names: Some(self.tag_names),
            unread: Some(self.unread),
            shared: Some(self.shared),
            ..Default::default()
        }
    }

    fn into_update_body(self) -> UpdateBookmarkBody {
        UpdateBookmarkBody {
            title: self.title,
            description: self.description,
            notes: self.notes,
            tag_names: Some(self.tag_names),
            unread: Some(self.unread),
            shared: Some(self.shared),
            ..Default::default()
        }
    }
}

fn into_page(total: i32, items: Vec<Bookmark>) -> Page {
    Page {
        total,
        items: items.into_iter().map(BookmarkView::from).collect(),
    }
}
