//! Add / edit bookmark sheet.
//!
//! Presented as an [`adw::Dialog`], which adapts automatically: a bottom sheet
//! on narrow (phone) windows and a centred dialog on wide (desktop) ones.
//!
//! Flow (add): paste a URL → **Validate** calls `/check` → prefills title,
//! description and suggested tags (or the existing bookmark's fields if the URL
//! is already saved) → **Save** POSTs (Linkding upserts by URL).
//!
//! Flow (edit): fields are prefilled from the bookmark; Save PATCHes.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib::clone;

use crate::api::{BookmarkDraft, BookmarkView, Client};
use crate::runtime;

/// Open the add-bookmark sheet. `on_saved` runs after a successful save so the
/// caller can refresh the list and toast. `clipboard_url`, if `Some`, prefills
/// and auto-validates the URL (the "share/paste" fast path).
pub fn open_add<F>(
    parent: &impl IsA<gtk::Widget>,
    client: Client,
    clipboard_url: Option<String>,
    on_saved: F,
) where
    F: Fn(String) + 'static,
{
    build(parent, client, None, clipboard_url, on_saved);
}

/// Open the sheet in edit mode for an existing bookmark.
pub fn open_edit<F>(
    parent: &impl IsA<gtk::Widget>,
    client: Client,
    bookmark: BookmarkView,
    on_saved: F,
) where
    F: Fn(String) + 'static,
{
    build(parent, client, Some(bookmark), None, on_saved);
}

fn build<F>(
    parent: &impl IsA<gtk::Widget>,
    client: Client,
    editing: Option<BookmarkView>,
    clipboard_url: Option<String>,
    on_saved: F,
) where
    F: Fn(String) + 'static,
{
    let is_edit = editing.is_some();
    let on_saved = Rc::new(on_saved);
    let edit_id = editing.as_ref().map(|b| b.id);

    let toasts = adw::ToastOverlay::new();

    // Inline error banner: network/save/validate failures surface here (above the
    // URL group) rather than as a transient toast, so the message stays visible
    // while the user fixes the input. Successes remain toasts.
    let banner = adw::Banner::builder().revealed(false).build();

    // --- URL group -----------------------------------------------------------
    let url_row = adw::EntryRow::builder().title("URL").build();
    url_row.set_input_purpose(gtk::InputPurpose::Url);
    if is_edit {
        url_row.set_editable(false); // changing the URL of an existing bookmark errors server-side
    }

    let validate_button = gtk::Button::builder()
        .icon_name("network-receive-symbolic")
        .tooltip_text("Validate & autofill")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    url_row.add_suffix(&validate_button);

    let url_group = adw::PreferencesGroup::builder().title("Bookmark URL").build();
    url_group.add(&url_row);

    // --- Details group -------------------------------------------------------
    let title_row = adw::EntryRow::builder().title("Title (blank = scrape)").build();
    let desc_row = adw::EntryRow::builder()
        .title("Description (blank = scrape)")
        .build();
    let tags_row = adw::EntryRow::builder()
        .title("Tags (space or comma separated)")
        .build();

    let notes_row = adw::EntryRow::builder().title("Notes (optional)").build();

    let details_group = adw::PreferencesGroup::builder()
        .title("Details")
        .build();
    details_group.add(&title_row);
    details_group.add(&desc_row);
    details_group.add(&tags_row);
    details_group.add(&notes_row);

    // Suggested tags appear here after a /check.
    let suggestions = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .row_spacing(4)
        .column_spacing(4)
        .max_children_per_line(12)
        .margin_top(4)
        .build();
    let suggestions_group = adw::PreferencesGroup::builder()
        .title("Suggested tags")
        .build();
    suggestions_group.add(&suggestions);
    suggestions_group.set_visible(false);

    // --- Options group -------------------------------------------------------
    let unread_row = adw::SwitchRow::builder().title("Mark as unread").build();
    let shared_row = adw::SwitchRow::builder().title("Share").build();
    let options_group = adw::PreferencesGroup::builder().title("Options").build();
    options_group.add(&unread_row);
    options_group.add(&shared_row);

    // Prefill for edit.
    if let Some(b) = &editing {
        url_row.set_text(&b.url);
        title_row.set_text(&b.title);
        desc_row.set_text(&b.description);
        tags_row.set_text(&b.tag_names.join(" "));
        notes_row.set_text(&b.notes);
        unread_row.set_active(b.unread);
        shared_row.set_active(b.shared);
    }
    if let Some(u) = &clipboard_url {
        url_row.set_text(u);
    }

    // --- Page + toolbar ------------------------------------------------------
    let page = adw::PreferencesPage::new();
    page.add(&url_group);
    page.add(&suggestions_group);
    page.add(&details_group);
    page.add(&options_group);

    let cancel_button = gtk::Button::with_label("Cancel");
    let save_button = gtk::Button::builder()
        .label("Save")
        .css_classes(["suggested-action"])
        .build();

    let header = adw::HeaderBar::builder().show_end_title_buttons(false).build();
    header.pack_start(&cancel_button);
    header.pack_end(&save_button);
    header.set_title_widget(Some(&adw::WindowTitle::new(
        if is_edit { "Edit bookmark" } else { "Add bookmark" },
        "",
    )));

    let toolbar = adw::ToolbarView::builder().content(&page).build();
    toolbar.add_top_bar(&header);
    // The banner sits directly under the header, above the page's groups.
    toolbar.add_top_bar(&banner);
    toasts.set_child(Some(&toolbar));

    let dialog = adw::Dialog::builder()
        .title(if is_edit { "Edit bookmark" } else { "Add bookmark" })
        .content_width(480)
        .child(&toasts)
        .build();

    // --- Validate (/check) wiring -------------------------------------------
    let in_flight = Rc::new(RefCell::new(false));

    let run_check = Rc::new(clone!(
        #[strong] client,
        #[strong] url_row,
        #[strong] title_row,
        #[strong] desc_row,
        #[strong] tags_row,
        #[strong] suggestions,
        #[strong] suggestions_group,
        #[strong] toasts,
        #[strong] banner,
        #[strong] in_flight,
        move || {
            let url = url_row.text().trim().to_string();
            if url.is_empty() {
                return;
            }
            if *in_flight.borrow() {
                return;
            }
            *in_flight.borrow_mut() = true;

            let client = client.clone();
            runtime::spawn(
                async move { client.check(&url).await },
                clone!(
                    #[strong] title_row,
                    #[strong] desc_row,
                    #[strong] tags_row,
                    #[strong] suggestions,
                    #[strong] suggestions_group,
                    #[strong] toasts,
                    #[strong] banner,
                    #[strong] in_flight,
                    move |result| {
                        *in_flight.borrow_mut() = false;
                        match result {
                            Ok(check) => {
                                banner.set_revealed(false);
                                if let Some(existing) = check.existing {
                                    // Already saved: prefill from it.
                                    if title_row.text().is_empty() {
                                        title_row.set_text(&existing.title);
                                    }
                                    if desc_row.text().is_empty() {
                                        desc_row.set_text(&existing.description);
                                    }
                                    if tags_row.text().is_empty() {
                                        tags_row.set_text(&existing.tag_names.join(" "));
                                    }
                                    super::toast(&toasts, "This URL is already bookmarked");
                                } else {
                                    if title_row.text().is_empty() {
                                        if let Some(t) = check.scraped_title {
                                            title_row.set_text(&t);
                                        }
                                    }
                                    if desc_row.text().is_empty() {
                                        if let Some(d) = check.scraped_description {
                                            desc_row.set_text(&d);
                                        }
                                    }
                                }

                                // Populate suggested-tag chips.
                                while let Some(child) = suggestions.first_child() {
                                    suggestions.remove(&child);
                                }
                                if check.suggested_tags.is_empty() {
                                    suggestions_group.set_visible(false);
                                } else {
                                    suggestions_group.set_visible(true);
                                    for tag in check.suggested_tags {
                                        let chip = gtk::Button::builder()
                                            .label(format!("#{tag}"))
                                            .css_classes(["pill"])
                                            .build();
                                        chip.connect_clicked(clone!(
                                            #[strong] tags_row,
                                            #[strong] tag,
                                            move |chip| {
                                                append_tag(&tags_row, &tag);
                                                chip.set_sensitive(false);
                                            }
                                        ));
                                        suggestions.insert(&chip, -1);
                                    }
                                }
                            }
                            Err(err) => {
                                banner.set_title(&err.to_string());
                                banner.set_revealed(true);
                            }
                        }
                    }
                ),
            );
        }
    ));

    validate_button.connect_clicked(clone!(
        #[strong] run_check,
        move |_| run_check()
    ));
    // Pressing Enter in the URL row also validates.
    url_row.connect_entry_activated(clone!(
        #[strong] run_check,
        move |_| run_check()
    ));

    cancel_button.connect_clicked(clone!(
        #[strong] dialog,
        move |_| {
            dialog.close();
        }
    ));

    // --- Save wiring ---------------------------------------------------------
    let save_in_flight = Rc::new(RefCell::new(false));
    save_button.connect_clicked(clone!(
        #[strong] client,
        #[strong] dialog,
        #[strong] url_row,
        #[strong] title_row,
        #[strong] desc_row,
        #[strong] tags_row,
        #[strong] notes_row,
        #[strong] unread_row,
        #[strong] shared_row,
        #[strong] save_button,
        #[strong] banner,
        #[strong] save_in_flight,
        #[strong] on_saved,
        move |_| {
            if *save_in_flight.borrow() {
                return;
            }
            let url = url_row.text().trim().to_string();
            if url.is_empty() || url::Url::parse(&url).is_err() {
                banner.set_title("Enter a valid URL first");
                banner.set_revealed(true);
                return;
            }
            banner.set_revealed(false);

            let draft = BookmarkDraft {
                url: url.clone(),
                title: non_empty(&title_row.text()),
                description: non_empty(&desc_row.text()),
                notes: non_empty(&notes_row.text()),
                tag_names: parse_tags(&tags_row.text()),
                unread: unread_row.is_active(),
                shared: shared_row.is_active(),
            };

            *save_in_flight.borrow_mut() = true;
            save_button.set_sensitive(false);
            save_button.set_label("Saving…");

            let client = client.clone();
            runtime::spawn(
                async move {
                    match edit_id {
                        Some(id) => client.update(id, draft).await.map(|_| ()),
                        None => client.create(draft).await.map(|_| ()),
                    }
                },
                clone!(
                    #[strong] dialog,
                    #[strong] save_button,
                    #[strong] banner,
                    #[strong] save_in_flight,
                    #[strong] on_saved,
                    move |result| {
                        *save_in_flight.borrow_mut() = false;
                        save_button.set_sensitive(true);
                        save_button.set_label("Save");
                        match result {
                            Ok(()) => {
                                dialog.close();
                                on_saved(if is_edit {
                                    "Bookmark updated".into()
                                } else {
                                    "Bookmark saved".into()
                                });
                            }
                            Err(err) => {
                                banner.set_title(&err.to_string());
                                banner.set_revealed(true);
                            }
                        }
                    }
                ),
            );
        }
    ));

    dialog.present(Some(parent));

    // Seed the suggestion chips with the server's existing tags so the user can
    // click to add them even before validating a URL. Uses the tags endpoint.
    runtime::spawn(
        {
            let client = client.clone();
            async move { client.tags(0).await }
        },
        clone!(
            #[strong] suggestions,
            #[strong] suggestions_group,
            #[strong] tags_row,
            move |result| {
                let Ok(tags) = result else { return };
                if tags.is_empty() || suggestions.first_child().is_some() {
                    // Don't clobber /check suggestions if they already arrived.
                    return;
                }
                suggestions_group.set_title("Existing tags");
                suggestions_group.set_visible(true);
                for tag in tags.into_iter().take(24) {
                    let name = tag.name;
                    let chip = gtk::Button::builder()
                        .label(format!("#{name}"))
                        .css_classes(["pill"])
                        .build();
                    chip.connect_clicked(clone!(
                        #[strong] tags_row,
                        #[strong] name,
                        move |chip| {
                            append_tag(&tags_row, &name);
                            chip.set_sensitive(false);
                        }
                    ));
                    suggestions.insert(&chip, -1);
                }
            }
        ),
    );

    // Auto-validate the shared/pasted URL after presenting.
    if clipboard_url.is_some() && !is_edit {
        run_check();
    }
}

/// Split a tags entry on whitespace/commas into a de-duplicated list.
fn parse_tags(input: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tag in input.split([' ', ',', '\t', '\n']) {
        let tag = tag.trim().trim_start_matches('#');
        if !tag.is_empty() && !out.iter().any(|t| t == tag) {
            out.push(tag.to_string());
        }
    }
    out
}

/// Append a tag to the entry if not already present.
fn append_tag(row: &adw::EntryRow, tag: &str) {
    let current = row.text();
    let existing = parse_tags(&current);
    if existing.iter().any(|t| t == tag) {
        return;
    }
    let sep = if current.trim().is_empty() { "" } else { " " };
    row.set_text(&format!("{current}{sep}{tag}"));
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

#[cfg(test)]
mod tests {
    use super::parse_tags;

    #[test]
    fn parses_and_dedups() {
        assert_eq!(parse_tags("rust  #gtk, rust\tnix"), vec!["rust", "gtk", "nix"]);
        assert_eq!(parse_tags("   "), Vec::<String>::new());
    }
}
