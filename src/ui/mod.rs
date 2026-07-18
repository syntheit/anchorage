//! UI layer. Widgets are built in pure Rust (no `.ui` templates) — the app is
//! small enough that builder-pattern construction stays legible and keeps
//! everything type-checked in one language.

pub mod add_sheet;
pub mod list;
pub mod models;
pub mod onboarding;
pub mod row;
pub mod tags;

use gtk::gdk;

/// A small stylesheet for pill-shaped tag/status badges used by list rows.
const APP_CSS: &str = "
.badge {
    padding: 1px 8px;
    border-radius: 9999px;
    font-size: 0.8em;
    background-color: alpha(@accent_bg_color, 0.18);
    color: @accent_color;
}
.badge.unread { background-color: alpha(@warning_color, 0.20); color: @warning_color; }
.badge.shared { background-color: alpha(@success_color, 0.20); color: @success_color; }
.bookmark-title { font-weight: bold; }
.bookmark-meta { font-size: 0.85em; }
.bookmark-notes { font-style: italic; font-size: 0.9em; }
/* The per-row overflow (⋮) menu button: subtle until hovered/opened. */
.row-menu { min-width: 44px; min-height: 44px; padding: 0; opacity: 0.55; }
.row-menu:hover, .row-menu:checked, .row-menu:active { opacity: 1; }
";

/// Install the app stylesheet. Call once at application startup.
pub fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(APP_CSS);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

/// Show a transient toast on the given overlay. Convenience wrapper so callers
/// don't repeat the builder boilerplate.
pub fn toast(overlay: &adw::ToastOverlay, message: &str) {
    overlay.add_toast(adw::Toast::builder().title(message).timeout(3).build());
}
