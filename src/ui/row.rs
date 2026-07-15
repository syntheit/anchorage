//! A single bookmark list row: title, description snippet, host · date, and
//! tag / status badges. Built as a plain `gtk::ListBoxRow` (an `AdwActionRow`
//! is too constrained for the multi-line + badges layout).

use gtk::prelude::*;

use crate::api::BookmarkView;

use super::models;

/// Build a list row for `bookmark`. The returned row carries the bookmark id in
/// its `widget_name` (numeric string) so activation handlers can recover it.
pub fn build(bookmark: &BookmarkView) -> gtk::ListBoxRow {
    let vbox = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .margin_top(10)
        .margin_bottom(10)
        .margin_start(12)
        .margin_end(12)
        .build();

    // Row 1: title.
    let title = gtk::Label::builder()
        .label(models::display_title(bookmark))
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .single_line_mode(true)
        .build();
    title.add_css_class("bookmark-title");
    vbox.append(&title);

    // Row 2: description snippet (up to 3 lines), if present.
    if let Some(desc) = models::display_description(bookmark) {
        let label = gtk::Label::builder()
            .label(desc)
            .halign(gtk::Align::Start)
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(3)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        label.add_css_class("dim-label");
        vbox.append(&label);
    }

    // Row 3: host · date + badges.
    let meta = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .build();

    let host_date = gtk::Label::builder()
        .label(format!(
            "{} · {}",
            models::host(&bookmark.url),
            models::short_date(&bookmark.date_added)
        ))
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .hexpand(true)
        .build();
    host_date.add_css_class("dim-label");
    host_date.add_css_class("bookmark-meta");
    meta.append(&host_date);

    if bookmark.unread {
        meta.append(&badge("unread", Some("unread")));
    }
    if bookmark.shared {
        meta.append(&badge("shared", Some("shared")));
    }
    if bookmark.is_archived {
        meta.append(&badge("archived", None));
    }

    // Tag badges (cap to keep rows tidy; full list is on the detail/edit sheet).
    for tag in bookmark.tag_names.iter().take(4) {
        meta.append(&badge(&format!("#{tag}"), None));
    }

    vbox.append(&meta);

    let row = gtk::ListBoxRow::builder().child(&vbox).build();
    row.set_widget_name(&bookmark.id.to_string());
    row
}

/// Recover the bookmark id stored on a row by [`build`].
pub fn row_id(row: &gtk::ListBoxRow) -> Option<i32> {
    row.widget_name().parse().ok()
}

fn badge(text: &str, kind: Option<&str>) -> gtk::Label {
    let label = gtk::Label::builder().label(text).build();
    label.add_css_class("badge");
    if let Some(k) = kind {
        label.add_css_class(k);
    }
    label
}
