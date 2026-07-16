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

/// Resolve a bookmark's `favicon_url` to an absolute URL suitable for fetching.
///
/// Linkding usually returns an absolute URL, but depending on server config it
/// can be relative (e.g. `/static/favicons/x.png`); resolve those against
/// `base_url`. Returns `None` when the bookmark has no favicon. Linkding serves
/// the image itself — we never fall back to a third-party favicon service.
pub fn resolve_favicon(base_url: &str, b: &BookmarkView) -> Option<String> {
    let raw = b.favicon_url.as_deref()?.trim();
    if raw.is_empty() {
        return None;
    }
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return Some(raw.to_string());
    }
    // Relative path: join onto the server base.
    url::Url::parse(base_url)
        .ok()
        .and_then(|base| base.join(raw).ok())
        .map(|u| u.to_string())
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

/// Label + icon for the read-status toggle in the row action menu. The action
/// reflects what the click *does*, not the current state: an unread bookmark
/// offers "Mark as read", and vice-versa.
pub fn read_action(unread: bool) -> (&'static str, &'static str) {
    if unread {
        ("Mark as read", "object-select-symbolic")
    } else {
        ("Mark as unread", "mail-mark-important-symbolic")
    }
}

/// Label + icon for the archive toggle in the row action menu. In the archived
/// list the action is "Unarchive"; elsewhere it's "Archive". Distinct icons keep
/// it from reading as "delete".
pub fn archive_action(is_archived_scope: bool) -> (&'static str, &'static str) {
    if is_archived_scope {
        ("Unarchive", "view-restore-symbolic")
    } else {
        ("Archive", "folder-download-symbolic")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_bookmark() -> BookmarkView {
        BookmarkView {
            id: 1,
            url: "https://example.com".into(),
            title: String::new(),
            description: String::new(),
            notes: String::new(),
            tag_names: Vec::new(),
            website_title: None,
            website_description: None,
            favicon_url: None,
            unread: false,
            shared: false,
            is_archived: false,
            date_added: String::new(),
        }
    }

    #[test]
    fn notes_snippet() {
        let mut b = sample_bookmark();
        b.notes = "first line\n\nsecond   line".into();
        assert_eq!(
            display_notes(&b),
            Some("first line second line".to_string())
        );
        b.notes = "   \n  ".into();
        assert_eq!(display_notes(&b), None);
        b.notes = String::new();
        assert_eq!(display_notes(&b), None);
    }

    #[test]
    fn favicon_resolution() {
        let mut b = sample_bookmark();
        // No favicon.
        assert_eq!(resolve_favicon("https://ld.example.com", &b), None);
        b.favicon_url = Some("   ".into());
        assert_eq!(resolve_favicon("https://ld.example.com", &b), None);
        // Absolute URL is passed through.
        b.favicon_url = Some("https://ld.example.com/static/x.png".into());
        assert_eq!(
            resolve_favicon("https://ld.example.com", &b),
            Some("https://ld.example.com/static/x.png".to_string())
        );
        // Relative path is joined onto the base.
        b.favicon_url = Some("/static/favicons/y.png".into());
        assert_eq!(
            resolve_favicon("https://ld.example.com", &b),
            Some("https://ld.example.com/static/favicons/y.png".to_string())
        );
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

    #[test]
    fn read_action_reflects_click_not_state() {
        // Unread bookmark → the action reads it (Mark as read).
        assert_eq!(read_action(true).0, "Mark as read");
        // Read bookmark → the action unreads it.
        assert_eq!(read_action(false).0, "Mark as unread");
        // Icons differ so the two states are visually distinct.
        assert_ne!(read_action(true).1, read_action(false).1);
    }

    #[test]
    fn archive_action_depends_on_scope() {
        assert_eq!(archive_action(true).0, "Unarchive");
        assert_eq!(archive_action(false).0, "Archive");
        assert_ne!(archive_action(true).1, archive_action(false).1);
    }
}
