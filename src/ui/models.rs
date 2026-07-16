//! View-model helpers: presentation logic derived from the raw API types.
//!
//! Kept free of GTK so it can be unit-tested and reused (e.g. by the Memos
//! sibling) without a display.

use crate::api::BookmarkView;

/// The title to show for a bookmark, with Linkdy's fallback chain:
/// `title` → `website_title` → the URL itself.
pub fn display_title(b: &BookmarkView) -> String {
    first_non_empty([
        b.title.as_str(),
        b.website_title.as_deref().unwrap_or_default(),
        b.url.as_str(),
    ])
    .to_string()
}

/// The description snippet, falling back `description` → `website_description`.
/// Returns `None` when neither is present (so the row can hide the label).
pub fn display_description(b: &BookmarkView) -> Option<String> {
    let candidate = first_non_empty([
        b.description.as_str(),
        b.website_description.as_deref().unwrap_or_default(),
    ]);
    (!candidate.is_empty()).then(|| candidate.to_string())
}

/// The notes snippet to show, or `None` when the bookmark has no notes (so the
/// row can hide the label). Notes are Markdown on the server; we show the raw
/// text collapsed to a single spaced line — enough context for the list.
pub fn display_notes(b: &BookmarkView) -> Option<String> {
    let collapsed = b.notes.split_whitespace().collect::<Vec<_>>().join(" ");
    (!collapsed.is_empty()).then_some(collapsed)
}

/// The host portion of the URL (`https://news.ycombinator.com/x` → `news.ycombinator.com`).
/// Falls back to the raw URL when it can't be parsed.
pub fn host(url_str: &str) -> String {
    url::Url::parse(url_str)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| url_str.to_string())
}

/// Format an ISO-8601 timestamp (as returned by Linkding) into a short,
/// human date like `2026-07-14`. We deliberately avoid a heavy date crate:
/// Linkding returns `2026-07-14T09:46:23.006313Z`, so the date is the prefix
/// before `T`. Returns the input unchanged if it doesn't look like ISO-8601.
pub fn short_date(iso: &str) -> String {
    match iso.split_once('T') {
        Some((date, _)) if date.len() == 10 && date.as_bytes()[4] == b'-' => date.to_string(),
        _ => iso.to_string(),
    }
}

fn first_non_empty<const N: usize>(candidates: [&str; N]) -> &str {
    candidates
        .into_iter()
        .find(|c| !c.trim().is_empty())
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bookmark_with_notes(notes: &str) -> BookmarkView {
        BookmarkView {
            id: 1,
            url: "https://example.com".into(),
            title: String::new(),
            description: String::new(),
            notes: notes.into(),
            tag_names: Vec::new(),
            website_title: None,
            website_description: None,
            unread: false,
            shared: false,
            is_archived: false,
            date_added: String::new(),
        }
    }

    #[test]
    fn notes_snippet() {
        assert_eq!(
            display_notes(&bookmark_with_notes("first line\n\nsecond   line")),
            Some("first line second line".to_string())
        );
        assert_eq!(display_notes(&bookmark_with_notes("   \n  ")), None);
        assert_eq!(display_notes(&bookmark_with_notes("")), None);
    }

    #[test]
    fn host_extraction() {
        assert_eq!(host("https://news.ycombinator.com/item?id=1"), "news.ycombinator.com");
        assert_eq!(host("not a url"), "not a url");
    }

    #[test]
    fn date_prefix() {
        assert_eq!(short_date("2026-07-14T09:46:23.006313Z"), "2026-07-14");
        assert_eq!(short_date("garbage"), "garbage");
    }

    #[test]
    fn first_non_empty_picks() {
        assert_eq!(first_non_empty(["", "  ", "x"]), "x");
        assert_eq!(first_non_empty(["", ""]), "");
    }
}
