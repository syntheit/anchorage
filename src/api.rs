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
    ListBookmarksResponse, ListTagsArgs, TagData, UpdateBookmarkBody,
};

/// Default page size for list requests.
pub const PAGE_LIMIT: i32 = 100;

/// Read-status filter for the bookmark list, mapping to Linkding's `unread`
/// query parameter. `linkding-rs` doesn't expose this param, so the list calls
/// build the request directly (see [`Client::list_endpoint`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UnreadFilter {
    #[default]
    All,
    Unread,
    Read,
}

impl UnreadFilter {
    /// The `unread` query value to send, or `None` to omit the parameter (All).
    fn param(self) -> Option<&'static str> {
        match self {
            UnreadFilter::All => None,
            UnreadFilter::Unread => Some("yes"),
            UnreadFilter::Read => Some("no"),
        }
    }
}

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
    pub favicon_url: Option<String>,
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
            favicon_url: b.favicon_url,
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
    // A direct reqwest client + token for the handful of endpoints/params that
    // `linkding-rs` doesn't parameterise (the `unread` list filter and the
    // `enable_favicons` profile flag).
    http: reqwest::Client,
    token: Arc<str>,
    base_url: Arc<str>,
}

/// Errors surfaced to the UI. We keep a human string because `linkding-rs`
/// collapses HTTP status codes into a single `reqwest::Error` variant.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    Message(String),
}

/// Turn a `reqwest::Error` into a user-facing hint (auth vs. network vs. other).
fn reqwest_message(e: &reqwest::Error) -> String {
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

impl From<LinkDingError> for ApiError {
    fn from(err: LinkDingError) -> Self {
        let msg = match &err {
            LinkDingError::SendHttpError(e) => reqwest_message(e),
            other => other.to_string(),
        };
        ApiError::Message(msg)
    }
}

impl From<reqwest::Error> for ApiError {
    fn from(err: reqwest::Error) -> Self {
        ApiError::Message(reqwest_message(&err))
    }
}

impl Client {
    /// Build a client for `url` authenticated with `token`. Infallible — errors
    /// surface per request.
    pub fn new(url: &str, token: &str) -> Self {
        let trimmed = url.trim_end_matches('/');
        Client {
            inner: Arc::new(LinkDingAsyncClient::new(trimmed, token)),
            http: reqwest::Client::new(),
            token: Arc::from(token),
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

    /// List active bookmarks with an optional search query, read-status filter
    /// and pagination.
    pub async fn list(
        &self,
        query: Option<String>,
        filter: UnreadFilter,
        offset: i32,
    ) -> Result<Page, ApiError> {
        self.list_endpoint("/api/bookmarks/", query, filter, offset)
            .await
    }

    /// List archived bookmarks with the same query/filter/pagination semantics.
    pub async fn list_archived(
        &self,
        query: Option<String>,
        filter: UnreadFilter,
        offset: i32,
    ) -> Result<Page, ApiError> {
        self.list_endpoint("/api/bookmarks/archived/", query, filter, offset)
            .await
    }

    /// Shared implementation for the active and archived list endpoints. Built
    /// with a direct request because `linkding-rs`'s `ListBookmarksArgs` has no
    /// `unread` slot; the response is deserialised into its public wire type.
    async fn list_endpoint(
        &self,
        path: &str,
        query: Option<String>,
        filter: UnreadFilter,
        offset: i32,
    ) -> Result<Page, ApiError> {
        let mut params: Vec<(&str, String)> = vec![
            ("limit", PAGE_LIMIT.to_string()),
            ("offset", offset.to_string()),
        ];
        if let Some(q) = query.filter(|q| !q.trim().is_empty()) {
            params.push(("q", q));
        }
        if let Some(unread) = filter.param() {
            params.push(("unread", unread.to_string()));
        }

        let resp: ListBookmarksResponse = self
            .http
            .get(format!("{}{path}", self.base_url))
            .header("Authorization", format!("Token {}", self.token))
            .query(&params)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
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

    /// Toggle just the read status of a bookmark via a minimal PATCH — leaves
    /// all other fields untouched (unlike [`update`], which sends the full form).
    pub async fn set_unread(&self, id: i32, unread: bool) -> Result<(), ApiError> {
        let body = UpdateBookmarkBody {
            unread: Some(unread),
            ..Default::default()
        };
        self.inner.update_bookmark(id, body).await?;
        Ok(())
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

    /// Fetch the raw bytes of a favicon (or any small image) at `url`. Used by
    /// list rows to render the site favicon. Includes the auth token in case the
    /// server gates static files behind it.
    pub async fn fetch_favicon(&self, url: &str) -> Result<Vec<u8>, ApiError> {
        let bytes = self
            .http
            .get(url)
            .header("Authorization", format!("Token {}", self.token))
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        Ok(bytes.to_vec())
    }

    /// Whether the server has favicons enabled (`enable_favicons` in the user
    /// profile). When off, `favicon_url` is absent from bookmark responses.
    /// `linkding-rs`'s `UserProfile` keeps its fields private, so we read just
    /// the flag we need directly.
    pub async fn favicons_enabled(&self) -> Result<bool, ApiError> {
        let profile: ProfileFlags = self
            .http
            .get(format!("{}/api/user/profile/", self.base_url))
            .header("Authorization", format!("Token {}", self.token))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(profile.enable_favicons)
    }
}

/// The single user-profile flag we care about; the rest of the response is
/// ignored by serde.
#[derive(serde::Deserialize)]
struct ProfileFlags {
    enable_favicons: bool,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unread_filter_param() {
        assert_eq!(UnreadFilter::All.param(), None);
        assert_eq!(UnreadFilter::Unread.param(), Some("yes"));
        assert_eq!(UnreadFilter::Read.param(), Some("no"));
        assert_eq!(UnreadFilter::default(), UnreadFilter::All);
    }
}
