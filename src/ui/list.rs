//! The bookmark list view: a searchable, refreshable, paginated list with
//! per-row actions (open, edit, archive, delete + undo). Exposes a
//! [`BookmarkList`] handle the shell uses to trigger refreshes and reads.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib::{self, clone};

use crate::api::{BookmarkView, Client};
use crate::runtime;

use super::{add_sheet, row};

/// Which list this view is showing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Active,
    Archived,
}

/// A self-contained bookmark list widget + its state.
#[derive(Clone)]
pub struct BookmarkList {
    /// The content widget (search bar + toast overlay/stack) — embed this directly.
    content: gtk::Box,
    search_bar: gtk::SearchBar,
    inner: Rc<Inner>,
}

struct Inner {
    client: Client,
    scope: Scope,
    listbox: gtk::ListBox,
    stack: gtk::Stack,
    spinner: gtk::Spinner,
    toasts: adw::ToastOverlay,
    // id -> full bookmark, so actions can read the current model without refetch.
    bookmarks: RefCell<HashMap<i32, BookmarkView>>,
    query: RefCell<String>,
    offset: Cell<i32>,
    total: Cell<i32>,
    loading: Cell<bool>,
    debounce: RefCell<Option<glib::SourceId>>,
}

impl BookmarkList {
    /// Build a list view for `scope`.
    pub fn new(client: Client, scope: Scope) -> Self {
        let listbox = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .build();

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&listbox)
            .build();

        let empty = adw::StatusPage::builder()
            .icon_name("user-bookmarks-symbolic")
            .title("No bookmarks")
            .description(match scope {
                Scope::Active => "Add one with the + button, or adjust your search.",
                Scope::Archived => "Nothing archived yet.",
            })
            .build();

        let spinner = gtk::Spinner::builder()
            .width_request(32)
            .height_request(32)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        let loading_page = adw::Bin::builder().child(&spinner).build();

        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        stack.add_named(&scroller, Some("list"));
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&loading_page, Some("loading"));

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&stack));

        // Search bar — lives below the (outer) header bar.
        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text("Search bookmarks\u{2026}  (try #tag)")
            .hexpand(true)
            .build();
        let search_bar = gtk::SearchBar::builder()
            .child(&search_entry)
            .key_capture_widget(&stack)
            .build();
        search_bar.connect_entry(&search_entry);

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        content.append(&search_bar);
        content.append(&toasts);

        let inner = Rc::new(Inner {
            client,
            scope,
            listbox: listbox.clone(),
            stack,
            spinner,
            toasts,
            bookmarks: RefCell::new(HashMap::new()),
            query: RefCell::new(String::new()),
            offset: Cell::new(0),
            total: Cell::new(0),
            loading: Cell::new(false),
            debounce: RefCell::new(None),
        });

        let this = BookmarkList { content, search_bar, inner };

        // Wire search entry.
        this.wire_search(&search_entry);
        this.wire_pagination(&scroller);
        this.wire_row_activation();

        this
    }

    /// The content widget to embed as a view-stack child.
    pub fn widget(&self) -> &gtk::Box {
        &self.content
    }

    /// The search bar for this list; the outer header binds its toggle to this.
    pub fn search_bar(&self) -> &gtk::SearchBar {
        &self.search_bar
    }

    /// Reload from the first page using the current query.
    pub fn refresh(&self) {
        self.inner.offset.set(0);
        self.inner.bookmarks.borrow_mut().clear();
        // Clear rows.
        while let Some(child) = self.inner.listbox.first_child() {
            self.inner.listbox.remove(&child);
        }
        self.load(true);
    }

    /// Open the add-bookmark sheet, prefilling from the clipboard if it holds a URL.
    pub fn open_add(&self) {
        let this = self.clone();
        let client = self.inner.client.clone();
        let parent = self.content.clone();

        // Try to read a URL from the clipboard for the fast paste path.
        let display = self.content.display();
        let clipboard = display.clipboard();
        clipboard.read_text_async(
            gtk::gio::Cancellable::NONE,
            clone!(
                #[strong] this,
                #[strong] client,
                #[strong] parent,
                move |res| {
                    let url = res
                        .ok()
                        .flatten()
                        .map(|g| g.to_string())
                        .filter(|s| s.starts_with("http://") || s.starts_with("https://"));
                    add_sheet::open_add(
                        &parent,
                        client.clone(),
                        url,
                        clone!(
                            #[strong] this,
                            move |msg| {
                                super::toast(&this.inner.toasts, &msg);
                                this.refresh();
                            }
                        ),
                    );
                }
            ),
        );
    }

    // --- internal ------------------------------------------------------------

    fn wire_search(&self, entry: &gtk::SearchEntry) {
        let this = self.clone();
        entry.connect_search_changed(move |entry| {
            let text = entry.text().to_string();
            this.schedule_search(text);
        });
    }

    /// Debounce search input by 300ms, then refresh with the new query.
    fn schedule_search(&self, text: String) {
        // Cancel any pending timer.
        if let Some(id) = self.inner.debounce.borrow_mut().take() {
            id.remove();
        }
        let this = self.clone();
        let id = glib::timeout_add_local_once(Duration::from_millis(300), move || {
            *this.inner.debounce.borrow_mut() = None;
            *this.inner.query.borrow_mut() = text.clone();
            this.refresh();
        });
        *self.inner.debounce.borrow_mut() = Some(id);
    }

    fn wire_pagination(&self, scroller: &gtk::ScrolledWindow) {
        let this = self.clone();
        let vadj = scroller.vadjustment();
        vadj.connect_value_changed(move |adj| {
            let remaining = adj.upper() - (adj.value() + adj.page_size());
            if remaining < 200.0 {
                this.load_more();
            }
        });
    }

    fn wire_row_activation(&self) {
        let this = self.clone();
        self.inner.listbox.connect_row_activated(move |_, listrow| {
            if let Some(id) = row::row_id(listrow) {
                this.show_actions(id, listrow);
            }
        });
    }

    fn load_more(&self) {
        if self.inner.loading.get() {
            return;
        }
        let loaded = self.inner.bookmarks.borrow().len() as i32;
        if loaded >= self.inner.total.get() && self.inner.total.get() > 0 {
            return; // all pages fetched
        }
        self.inner.offset.set(loaded);
        self.load(false);
    }

    /// Fetch a page. `reset` shows the full-view spinner (used for refresh);
    /// otherwise it's a background "load more".
    fn load(&self, reset: bool) {
        if self.inner.loading.get() {
            return;
        }
        self.inner.loading.set(true);

        if reset {
            self.inner.stack.set_visible_child_name("loading");
            self.inner.spinner.start();
        }

        let query = self.inner.query.borrow().clone();
        let offset = self.inner.offset.get();
        let client = self.inner.client.clone();
        let scope = self.inner.scope;
        let query_opt = (!query.is_empty()).then_some(query);

        let this = self.clone();
        runtime::spawn(
            async move {
                match scope {
                    Scope::Active => client.list(query_opt, offset).await,
                    Scope::Archived => client.list_archived(query_opt, offset).await,
                }
            },
            move |result| {
                this.inner.loading.set(false);
                this.inner.spinner.stop();
                match result {
                    Ok(page) => {
                        this.inner.total.set(page.total);
                        this.append_page(page.items);
                    }
                    Err(err) => {
                        super::toast(&this.inner.toasts, &err.to_string());
                        // Show whatever we have; fall back to empty state.
                        this.update_visibility();
                    }
                }
            },
        );
    }

    fn append_page(&self, items: Vec<BookmarkView>) {
        for b in items {
            let row = row::build(&b);
            self.inner.listbox.append(&row);
            self.inner.bookmarks.borrow_mut().insert(b.id, b);
        }
        self.update_visibility();
    }

    fn update_visibility(&self) {
        let has_rows = self.inner.listbox.first_child().is_some();
        self.inner
            .stack
            .set_visible_child_name(if has_rows { "list" } else { "empty" });
    }

    /// Show the action menu (open / edit / archive / delete) for a row.
    fn show_actions(&self, id: i32, anchor: &gtk::ListBoxRow) {
        let Some(bookmark) = self.inner.bookmarks.borrow().get(&id).cloned() else {
            return;
        };

        let popover = gtk::Popover::builder().has_arrow(true).build();
        popover.set_parent(anchor);
        // The popover is manually parented to the row, so it must be unparented
        // when it closes — otherwise every row-action click leaks a widget.
        popover.connect_closed(|p| p.unparent());

        let vbox = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .build();

        let make = |label: &str, icon: &str| {
            let b = gtk::Button::builder()
                .css_classes(["flat"])
                .child(&{
                    let bx = gtk::Box::builder().spacing(8).build();
                    bx.append(&gtk::Image::from_icon_name(icon));
                    bx.append(&gtk::Label::new(Some(label)));
                    bx
                })
                .build();
            b.set_halign(gtk::Align::Fill);
            b
        };

        let open_b = make("Open in browser", "web-browser-symbolic");
        let edit_b = make("Edit", "document-edit-symbolic");
        // Distinct icons so archive/unarchive don't read as "delete": a
        // put-away box for Archive, a restore glyph for Unarchive.
        let (archive_label, archive_icon) = if self.inner.scope == Scope::Archived {
            ("Unarchive", "view-restore-symbolic")
        } else {
            ("Archive", "folder-download-symbolic")
        };
        let archive_b = make(archive_label, archive_icon);
        let delete_b = make("Delete", "edit-delete-symbolic");
        delete_b.add_css_class("destructive-action");

        vbox.append(&open_b);
        vbox.append(&edit_b);
        vbox.append(&archive_b);
        vbox.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        vbox.append(&delete_b);
        popover.set_child(Some(&vbox));

        // Open in browser.
        open_b.connect_clicked(clone!(
            #[strong] popover,
            #[strong(rename_to = url)] bookmark.url,
            #[strong] anchor,
            move |_| {
                popover.popdown();
                let launcher = gtk::UriLauncher::new(&url);
                let window = anchor.root().and_downcast::<gtk::Window>();
                launcher.launch(
                    window.as_ref(),
                    gtk::gio::Cancellable::NONE,
                    |res| {
                        if let Err(err) = res {
                            tracing::warn!(%err, "failed to open uri");
                        }
                    },
                );
            }
        ));

        // Edit.
        edit_b.connect_clicked(clone!(
            #[strong] popover,
            #[strong] bookmark,
            #[strong(rename_to = this)] self,
            move |_| {
                popover.popdown();
                let this2 = this.clone();
                add_sheet::open_edit(
                    this.widget(),
                    this.inner.client.clone(),
                    bookmark.clone(),
                    move |msg| {
                        super::toast(&this2.inner.toasts, &msg);
                        this2.refresh();
                    },
                );
            }
        ));

        // Archive / unarchive.
        archive_b.connect_clicked(clone!(
            #[strong] popover,
            #[strong(rename_to = this)] self,
            move |_| {
                popover.popdown();
                this.toggle_archive(id);
            }
        ));

        // Delete (with confirmation).
        delete_b.connect_clicked(clone!(
            #[strong] popover,
            #[strong(rename_to = this)] self,
            move |_| {
                popover.popdown();
                this.confirm_delete(id);
            }
        ));

        popover.popup();
    }

    fn toggle_archive(&self, id: i32) {
        let client = self.inner.client.clone();
        let scope = self.inner.scope;
        let this = self.clone();
        runtime::spawn(
            async move {
                match scope {
                    Scope::Active => client.archive(id).await,
                    Scope::Archived => client.unarchive(id).await,
                }
            },
            move |result| match result {
                Ok(()) => {
                    let msg = if scope == Scope::Archived {
                        "Unarchived"
                    } else {
                        "Archived"
                    };
                    super::toast(&this.inner.toasts, msg);
                    this.refresh();
                }
                Err(err) => super::toast(&this.inner.toasts, &err.to_string()),
            },
        );
    }

    fn confirm_delete(&self, id: i32) {
        let title = self
            .inner
            .bookmarks
            .borrow()
            .get(&id)
            .map(super::models::display_title)
            .unwrap_or_default();

        let dialog = adw::AlertDialog::builder()
            .heading("Delete bookmark?")
            .body(format!("\u{201c}{title}\u{201d} will be permanently deleted."))
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("delete", "Delete");
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let this = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response == "delete" {
                this.do_delete(id);
            }
        });
        dialog.present(Some(&self.content));
    }

    fn do_delete(&self, id: i32) {
        let client = self.inner.client.clone();
        let this = self.clone();
        runtime::spawn(
            async move { client.delete(id).await },
            move |result| match result {
                Ok(()) => {
                    super::toast(&this.inner.toasts, "Bookmark deleted");
                    this.refresh();
                }
                Err(err) => super::toast(&this.inner.toasts, &err.to_string()),
            },
        );
    }
}
