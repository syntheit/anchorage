//! The tags screen: a paginated list of all the user's tags plus a create-tag
//! action. Tapping a tag asks the shell to filter the bookmark list by
//! `#tagname` (see `app.rs`). Mirrors [`crate::ui::list::BookmarkList`]'s shape:
//! a self-contained widget carrying its own state behind an `Rc<Inner>`.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib::{self, clone};

use crate::api::Client;
use crate::runtime;

/// A self-contained tags list widget + its state.
#[derive(Clone)]
pub struct TagsView {
    content: gtk::Box,
    inner: Rc<Inner>,
}

struct Inner {
    client: Client,
    listbox: gtk::ListBox,
    stack: gtk::Stack,
    spinner: gtk::Spinner,
    toasts: adw::ToastOverlay,
    /// Called with the tag name when a tag row is activated.
    on_select: Box<dyn Fn(&str)>,
    offset: Cell<i32>,
    total: Cell<i32>,
    loaded: Cell<i32>,
    loading: Cell<bool>,
}

impl TagsView {
    /// Build the tags screen. `on_select` runs with the chosen tag's name when a
    /// row is tapped — the shell uses it to switch to the bookmark list filtered
    /// by that tag.
    pub fn new<F>(client: Client, on_select: F) -> Self
    where
        F: Fn(&str) + 'static,
    {
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
            .icon_name("tag-symbolic")
            .title("No tags")
            .description("Create one above, or tags will appear as you add them to bookmarks.")
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

        // Create-tag row: an entry with a trailing "Add" button.
        let (create_bar, name_entry, add_button) = build_create_bar();

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        content.append(&create_bar);
        content.append(&toasts);

        let inner = Rc::new(Inner {
            client,
            listbox: listbox.clone(),
            stack,
            spinner,
            toasts,
            on_select: Box::new(on_select),
            offset: Cell::new(0),
            total: Cell::new(0),
            loaded: Cell::new(0),
            loading: Cell::new(false),
        });

        let this = TagsView { content, inner };

        this.wire_pagination(&scroller);
        this.wire_row_activation();
        this.wire_create(&name_entry, &add_button);

        this
    }

    /// The content widget to embed as a view-stack child.
    pub fn widget(&self) -> &gtk::Box {
        &self.content
    }

    /// Reload the tag list from the first page.
    pub fn refresh(&self) {
        self.inner.offset.set(0);
        self.inner.loaded.set(0);
        self.inner.total.set(0);
        while let Some(child) = self.inner.listbox.first_child() {
            self.inner.listbox.remove(&child);
        }
        self.load(true);
    }

    // --- internal ------------------------------------------------------------

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
            // The tag name is stored in the row's widget_name (see `add_tag_row`).
            let name = listrow.widget_name().to_string();
            if !name.is_empty() {
                (this.inner.on_select)(&name);
            }
        });
    }

    /// Wire the create-tag entry + button. Both Enter and the button submit.
    fn wire_create(&self, entry: &adw::EntryRow, button: &gtk::Button) {
        let submit = Rc::new(clone!(
            #[strong(rename_to = this)] self,
            #[weak] entry,
            move || {
                let name = entry.text().trim().trim_start_matches('#').to_string();
                if name.is_empty() {
                    return;
                }
                entry.set_sensitive(false);
                this.create_tag(name, &entry);
            }
        ));
        button.connect_clicked(clone!(
            #[strong] submit,
            move |_| submit()
        ));
        entry.connect_entry_activated(move |_| submit());
    }

    fn create_tag(&self, name: String, entry: &adw::EntryRow) {
        let client = self.inner.client.clone();
        let this = self.clone();
        let entry = entry.clone();
        runtime::spawn(
            async move { client.create_tag(&name).await },
            move |result| {
                entry.set_sensitive(true);
                match result {
                    Ok(tag) => {
                        entry.set_text("");
                        super::toast(&this.inner.toasts, &format!("Created #{}", tag.name));
                        this.refresh();
                    }
                    Err(err) => super::toast(&this.inner.toasts, &err.to_string()),
                }
            },
        );
    }

    fn load_more(&self) {
        if self.inner.loading.get() {
            return;
        }
        let loaded = self.inner.loaded.get();
        if loaded >= self.inner.total.get() && self.inner.total.get() > 0 {
            return; // all pages fetched
        }
        self.inner.offset.set(loaded);
        self.load(false);
    }

    /// Fetch a page of tags. `reset` shows the full-view spinner (refresh);
    /// otherwise it's a background load-more.
    fn load(&self, reset: bool) {
        if self.inner.loading.get() {
            return;
        }
        self.inner.loading.set(true);

        if reset {
            self.inner.stack.set_visible_child_name("loading");
            self.inner.spinner.start();
        }

        let offset = self.inner.offset.get();
        let client = self.inner.client.clone();
        let this = self.clone();
        runtime::spawn(
            async move { client.tags_page(offset).await },
            move |result| {
                this.inner.loading.set(false);
                this.inner.spinner.stop();
                match result {
                    Ok(page) => {
                        this.inner.total.set(page.total);
                        this.inner
                            .loaded
                            .set(this.inner.loaded.get() + page.items.len() as i32);
                        for tag in page.items {
                            this.add_tag_row(&tag.name);
                        }
                        this.update_visibility();
                    }
                    Err(err) => {
                        super::toast(&this.inner.toasts, &err.to_string());
                        this.update_visibility();
                    }
                }
            },
        );
    }

    fn add_tag_row(&self, name: &str) {
        let row = adw::ActionRow::builder()
            .title(format!("#{name}"))
            .activatable(true)
            .build();
        row.add_prefix(&gtk::Image::from_icon_name("tag-symbolic"));
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        // Stash the bare tag name so activation can recover it.
        row.set_widget_name(name);
        self.inner.listbox.append(&row);
    }

    fn update_visibility(&self) {
        let has_rows = self.inner.listbox.first_child().is_some();
        self.inner
            .stack
            .set_visible_child_name(if has_rows { "list" } else { "empty" });
    }
}

/// The create-tag bar: an `AdwEntryRow` (inside a boxed-list preferences group)
/// with a trailing "Add" button as its suffix.
fn build_create_bar() -> (gtk::Widget, adw::EntryRow, gtk::Button) {
    let entry = adw::EntryRow::builder().title("New tag name").build();
    let button = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("Create tag")
        .valign(gtk::Align::Center)
        .css_classes(["flat", "suggested-action"])
        .build();
    entry.add_suffix(&button);

    let group = adw::PreferencesGroup::builder()
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();
    group.add(&entry);

    (group.upcast(), entry, button)
}
