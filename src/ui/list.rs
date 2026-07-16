//! The bookmark list view: a searchable, refreshable, paginated list. Each row
//! carries a right-aligned overflow (⋮) menu with the full action set — open,
//! edit, mark read/unread, archive/unarchive, copy link, and delete (confirmed)
//! — MoeMemos-style. Exposes a [`BookmarkList`] handle the shell uses to trigger
//! refreshes and reads.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib::{self, clone};

use crate::api::{BookmarkView, Client, SortOrder, UnreadFilter};
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
    filter: Cell<UnreadFilter>,
    sort: Cell<SortOrder>,
    offset: Cell<i32>,
    total: Cell<i32>,
    loading: Cell<bool>,
    // Whether the server has favicons enabled; fetched once, rows read it to
    // decide whether to show a leading icon.
    favicons_enabled: Cell<bool>,
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

        // Read-status filter: All / Unread / Read as a linked toggle group,
        // with a compact sort selector on the trailing edge.
        let (filter_bar, filter_all, filter_unread, filter_read, sort_menu) = build_filter_bar();

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        content.append(&search_bar);
        content.append(&filter_bar);
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
            filter: Cell::new(UnreadFilter::All),
            sort: Cell::new(SortOrder::default()),
            offset: Cell::new(0),
            total: Cell::new(0),
            loading: Cell::new(false),
            favicons_enabled: Cell::new(false),
            debounce: RefCell::new(None),
        });

        let this = BookmarkList { content, search_bar, inner };

        // Wire search entry.
        this.wire_search(&search_entry);
        this.wire_filter(&filter_all, UnreadFilter::All);
        this.wire_filter(&filter_unread, UnreadFilter::Unread);
        this.wire_filter(&filter_read, UnreadFilter::Read);
        this.wire_sort(&sort_menu);
        this.wire_pagination(&scroller);
        this.wire_row_activation();
        this.load_favicon_capability();

        this
    }

    /// Fetch the server's favicon capability once. If enabled, re-render any rows
    /// already shown so their favicons appear.
    fn load_favicon_capability(&self) {
        let client = self.inner.client.clone();
        let this = self.clone();
        runtime::spawn(
            async move { client.favicons_enabled().await },
            move |result| {
                if matches!(result, Ok(true)) && !this.inner.favicons_enabled.replace(true) {
                    // Only re-render if we've already loaded rows without favicons.
                    if this.inner.listbox.first_child().is_some() {
                        this.refresh();
                    }
                }
            },
        );
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

    /// Wire a filter toggle: when it becomes active, apply `filter` and refresh.
    /// Only the active button fires a reload (the group is exclusive, so the
    /// previously-active button also emits `toggled` when it deactivates).
    fn wire_filter(&self, button: &gtk::ToggleButton, filter: UnreadFilter) {
        let this = self.clone();
        button.connect_toggled(move |button| {
            if button.is_active() && this.inner.filter.replace(filter) != filter {
                this.refresh();
            }
        });
    }

    /// Wire the sort selector: each option applies its `SortOrder` and refreshes
    /// (only on a real change). The choice persists for the session in
    /// `Inner::sort`, so it survives searches and filter changes.
    fn wire_sort(&self, menu: &gtk::MenuButton) {
        let popover = gtk::Popover::builder().has_arrow(true).build();
        let vbox = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .build();

        // Radio group: exactly one option checked, seeded from the current sort.
        let current = self.inner.sort.get();
        let mut group: Option<gtk::CheckButton> = None;
        for (order, label) in SortOrder::CHOICES {
            let check = gtk::CheckButton::builder()
                .label(label)
                .active(order == current)
                .build();
            if let Some(first) = &group {
                check.set_group(Some(first));
            } else {
                group = Some(check.clone());
            }
            let this = self.clone();
            let popover = popover.clone();
            check.connect_toggled(move |c| {
                // Only the newly-activated button triggers a reload; the
                // deactivated one also emits `toggled`.
                if c.is_active() && this.inner.sort.replace(order) != order {
                    popover.popdown();
                    this.refresh();
                }
            });
            vbox.append(&check);
        }

        popover.set_child(Some(&vbox));
        menu.set_popover(Some(&popover));
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
        let filter = self.inner.filter.get();
        let sort = self.inner.sort.get();
        let offset = self.inner.offset.get();
        let client = self.inner.client.clone();
        let scope = self.inner.scope;
        let query_opt = (!query.is_empty()).then_some(query);

        let this = self.clone();
        runtime::spawn(
            async move {
                match scope {
                    Scope::Active => client.list(query_opt, filter, sort, offset).await,
                    Scope::Archived => client.list_archived(query_opt, filter, sort, offset).await,
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
        let show_favicons = self.inner.favicons_enabled.get();
        let base_url = self.inner.client.base_url();
        for b in items {
            let favicon = show_favicons
                .then(|| super::models::resolve_favicon(base_url, &b))
                .flatten();
            let (row, menu) = row::build(&self.inner.client, &b, favicon);
            self.wire_row_menu(b.id, &menu);
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

    /// Attach the per-bookmark action popover to a row's overflow (⋮) menu
    /// button. The button owns the popover, so its lifecycle is managed for us —
    /// no manual parenting/unparenting needed (unlike [`show_actions`]).
    fn wire_row_menu(&self, id: i32, menu: &gtk::MenuButton) {
        let this = self.clone();
        // Build the popover lazily on first open so it always reflects the
        // bookmark's current state (read/unread, archived). The MenuButton owns
        // the popover it's given, so its lifecycle is handled for us.
        menu.set_create_popup_func(move |mb| {
            let Some(bookmark) = this.inner.bookmarks.borrow().get(&id).cloned() else {
                return;
            };
            let popover = gtk::Popover::builder().has_arrow(true).build();
            let content = this.build_actions_menu(id, &bookmark, &popover, mb);
            popover.set_child(Some(&content));
            mb.set_popover(Some(&popover));
        });
    }

    /// Show the action menu for a row via a manually-parented popover. Used by
    /// whole-row activation (tap the row body, not the ⋮ button). Kept separate
    /// from [`wire_row_menu`] because a manually-parented popover must be
    /// unparented on close or every activation leaks a widget.
    fn show_actions(&self, id: i32, anchor: &gtk::ListBoxRow) {
        let Some(bookmark) = self.inner.bookmarks.borrow().get(&id).cloned() else {
            return;
        };

        let popover = gtk::Popover::builder().has_arrow(true).build();
        popover.set_parent(anchor);
        // The popover is manually parented to the row, so it must be unparented
        // when it closes — otherwise every row-action click leaks a widget.
        popover.connect_closed(|p| p.unparent());

        let content = self.build_actions_menu(id, &bookmark, &popover, anchor);
        popover.set_child(Some(&content));
        popover.popup();
    }

    /// Build the shared action-menu content for `bookmark`: Open / Edit /
    /// Mark read-unread / Archive-Unarchive / Copy link (share) / Delete. Each
    /// button pops `popover` down and acts on this specific bookmark, then
    /// refreshes the list (re-applying the active filter). `anchor` supplies the
    /// root window for the URI launcher. Shared by [`show_actions`] and
    /// [`wire_row_menu`].
    fn build_actions_menu(
        &self,
        id: i32,
        bookmark: &BookmarkView,
        popover: &gtk::Popover,
        anchor: &impl IsA<gtk::Widget>,
    ) -> gtk::Box {
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
        let (read_label, read_icon) = super::models::read_action(bookmark.unread);
        let read_b = make(read_label, read_icon);
        let (archive_label, archive_icon) =
            super::models::archive_action(self.inner.scope == Scope::Archived);
        let archive_b = make(archive_label, archive_icon);
        let share_b = make("Copy link", "edit-copy-symbolic");
        let delete_b = make("Delete", "edit-delete-symbolic");
        delete_b.add_css_class("destructive-action");

        vbox.append(&open_b);
        vbox.append(&edit_b);
        vbox.append(&read_b);
        vbox.append(&archive_b);
        vbox.append(&share_b);
        vbox.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        vbox.append(&delete_b);

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

        // Mark as read / unread.
        read_b.connect_clicked(clone!(
            #[strong] popover,
            #[strong(rename_to = this)] self,
            #[strong(rename_to = was_unread)] bookmark.unread,
            move |_| {
                popover.popdown();
                this.toggle_unread(id, !was_unread);
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

        // Share — copy the bookmark URL to the clipboard.
        share_b.connect_clicked(clone!(
            #[strong] popover,
            #[strong(rename_to = url)] bookmark.url,
            #[strong(rename_to = this)] self,
            move |_| {
                popover.popdown();
                this.copy_url(&url);
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

        vbox
    }

    /// Copy `url` to the display clipboard and toast confirmation. This is the
    /// "Share" action; a desktop share portal could layer on later.
    fn copy_url(&self, url: &str) {
        self.content.clipboard().set_text(url);
        super::toast(&self.inner.toasts, "Link copied to clipboard");
    }

    /// Set the read status of a bookmark and refresh so the list reflects the
    /// change (and any active read/unread filter re-applies).
    fn toggle_unread(&self, id: i32, unread: bool) {
        let client = self.inner.client.clone();
        let this = self.clone();
        runtime::spawn(
            async move { client.set_unread(id, unread).await },
            move |result| match result {
                Ok(()) => {
                    let msg = if unread { "Marked as unread" } else { "Marked as read" };
                    super::toast(&this.inner.toasts, msg);
                    this.refresh();
                }
                Err(err) => super::toast(&this.inner.toasts, &err.to_string()),
            },
        );
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

/// Build the filter/sort bar: three linked, exclusive read-status toggles in the
/// centre, with a trailing sort selector (a menu button whose popover is wired
/// by [`BookmarkList::wire_sort`]). Returns the container plus the individual
/// controls for wiring. "All" starts active to match [`UnreadFilter::All`], the
/// list's default.
fn build_filter_bar() -> (
    gtk::Box,
    gtk::ToggleButton,
    gtk::ToggleButton,
    gtk::ToggleButton,
    gtk::MenuButton,
) {
    let all = gtk::ToggleButton::builder().label("All").active(true).build();
    let unread = gtk::ToggleButton::builder().label("Unread").build();
    let read = gtk::ToggleButton::builder().label("Read").build();
    // Group them so exactly one is active at a time.
    unread.set_group(Some(&all));
    read.set_group(Some(&all));

    let group = gtk::Box::builder().css_classes(["linked"]).build();
    group.append(&all);
    group.append(&unread);
    group.append(&read);

    let sort_menu = gtk::MenuButton::builder()
        .icon_name("view-sort-descending-symbolic")
        .tooltip_text("Sort")
        .css_classes(["flat"])
        .valign(gtk::Align::Center)
        .halign(gtk::Align::End)
        .hexpand(true)
        .build();

    let bar = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .margin_top(6)
        .margin_start(6)
        .margin_end(6)
        .build();
    // A leading spacer mirrors the trailing sort button so the toggle group
    // stays visually centred.
    bar.append(&gtk::Box::builder().hexpand(true).build());
    bar.append(&group);
    bar.append(&sort_menu);

    (bar, all, unread, read, sort_menu)
}
