//! Application shell: the top-level window, adaptive view switcher, and the
//! first-run → connected transition.
//!
//! Layout:
//!  * A `AdwNavigationView` hosts either the **connect** page (first run) or the
//!    **main** page (once we have credentials).
//!  * The main page is an `AdwViewStack` with *Bookmarks*, *Archived* and
//!    *Settings* views. On wide windows the switcher lives in the header; on
//!    narrow (phone) windows an `AdwBreakpoint` moves it to a bottom bar.

use adw::prelude::*;
use gtk::glib;
use gtk::glib::clone;

use crate::api::Client;
use crate::config;
use crate::runtime;
use crate::ui::list::{BookmarkList, Scope};
use crate::ui::onboarding;

/// Build the main window and kick off credential loading.
pub fn build_ui(app: &adw::Application) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Anchorage")
        .default_width(420)
        .default_height(720)
        .width_request(300)
        .height_request(400)
        .build();

    let nav = adw::NavigationView::new();
    window.set_content(Some(&nav));

    // Show a loading page while we consult the keyring.
    let loading = loading_page();
    nav.push(&loading);

    window.present();

    // Load credentials off the main thread (keyring is async).
    let nav_weak = nav.downgrade();
    let app = app.clone();
    runtime::spawn(config::load(), move |result| {
        let Some(nav) = nav_weak.upgrade() else { return };
        match result {
            Ok(Some(creds)) => {
                let client = Client::new(&creds.url, &creds.token);
                show_main(&app, &nav, client);
            }
            Ok(None) => show_connect(&app, &nav, config::stored_url()),
            Err(err) => {
                tracing::warn!(%err, "keyring unavailable; routing to connect");
                show_connect(&app, &nav, config::stored_url());
            }
        }
    });
}

fn loading_page() -> adw::NavigationPage {
    let spinner = gtk::Spinner::builder()
        .width_request(32)
        .height_request(32)
        .build();
    spinner.start();
    let status = adw::StatusPage::builder()
        .title("Anchorage")
        .child(&spinner)
        .build();
    let toolbar = adw::ToolbarView::builder().content(&status).build();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    adw::NavigationPage::builder()
        .title("Anchorage")
        .tag("loading")
        .child(&toolbar)
        .build()
}

/// Replace the navigation stack with the connect page.
fn show_connect(app: &adw::Application, nav: &adw::NavigationView, prefill: String) {
    let app = app.clone();
    let nav_weak = nav.downgrade();
    let page = onboarding::page(&prefill, move |client| {
        if let Some(nav) = nav_weak.upgrade() {
            show_main(&app, &nav, client);
        }
    });
    nav.replace(&[page]);
}

/// Replace the navigation stack with the main connected UI.
fn show_main(app: &adw::Application, nav: &adw::NavigationView, client: Client) {
    let page = main_page(app, nav, client);
    nav.replace(&[page]);
}

/// Build the connected main page (view stack + switcher + adaptive breakpoint).
fn main_page(
    app: &adw::Application,
    nav: &adw::NavigationView,
    client: Client,
) -> adw::NavigationPage {
    let active = BookmarkList::new(client.clone(), Scope::Active);
    let archived = BookmarkList::new(client.clone(), Scope::Archived);

    let stack = adw::ViewStack::new();
    let active_page = stack.add_titled_with_icon(
        active.widget(),
        Some("active"),
        "Bookmarks",
        "user-bookmarks-symbolic",
    );
    active_page.set_icon_name(Some("user-bookmarks-symbolic"));

    let archived_page = stack.add_titled_with_icon(
        archived.widget(),
        Some("archived"),
        "Archived",
        "folder-symbolic",
    );
    let _ = archived_page;

    let settings = settings_view(app, nav, &client);
    stack.add_titled_with_icon(&settings, Some("settings"), "Settings", "emblem-system-symbolic");

    // Load the first page of the active list eagerly; archived loads on switch.
    active.refresh();
    let archived_loaded = std::rc::Rc::new(std::cell::Cell::new(false));
    stack.connect_visible_child_name_notify(clone!(
        #[strong] archived,
        #[strong] archived_loaded,
        move |stack| {
            if stack.visible_child_name().as_deref() == Some("archived")
                && !archived_loaded.replace(true)
            {
                archived.refresh();
            }
        }
    ));

    // Wide: switcher in the header. Narrow: switcher bar at the bottom.
    let switcher_title = adw::ViewSwitcher::builder()
        .stack(&stack)
        .policy(adw::ViewSwitcherPolicy::Wide)
        .build();

    let header = adw::HeaderBar::builder()
        .title_widget(&switcher_title)
        .build();

    let switcher_bar = adw::ViewSwitcherBar::builder().stack(&stack).build();

    let toolbar = adw::ToolbarView::builder().content(&stack).build();
    toolbar.add_top_bar(&header);
    toolbar.add_bottom_bar(&switcher_bar);

    let page = adw::NavigationPage::builder()
        .title("Anchorage")
        .tag("main")
        .child(&toolbar)
        .build();

    // Breakpoint: below 550px show the bottom switcher bar and hide the header one.
    // We can't attach a breakpoint to a NavigationPage, so the window owns it —
    // set it up via the toolbar/switcher visibility here using a size handler.
    // libadwaita's ViewSwitcherBar has a `reveal` property we bind to width.
    if let Some(window) = app.active_window().and_downcast::<adw::ApplicationWindow>() {
        install_breakpoint(&window, &switcher_bar, &switcher_title);
    }

    page
}

/// Install an adaptive breakpoint on the window: narrow → reveal bottom switcher
/// and hide the header switcher (fall back to a plain title).
fn install_breakpoint(
    window: &adw::ApplicationWindow,
    bottom: &adw::ViewSwitcherBar,
    header_switcher: &adw::ViewSwitcher,
) {
    // Default (wide): bottom hidden, header switcher shown.
    bottom.set_reveal(false);
    header_switcher.set_visible(true);

    let condition = adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        550.0,
        adw::LengthUnit::Px,
    );
    let breakpoint = adw::Breakpoint::new(condition);

    let reveal = true.to_value();
    breakpoint.add_setter(bottom, "reveal", Some(&reveal));
    let hide = false.to_value();
    breakpoint.add_setter(header_switcher, "visible", Some(&hide));

    window.add_breakpoint(breakpoint);
}

/// The Settings view: server info + disconnect + about.
fn settings_view(
    app: &adw::Application,
    nav: &adw::NavigationView,
    client: &Client,
) -> adw::ToolbarView {
    let page = adw::PreferencesPage::new();

    let server_group = adw::PreferencesGroup::builder().title("Server").build();
    let server_row = adw::ActionRow::builder()
        .title("Connected to")
        .subtitle(client.base_url())
        .build();
    server_group.add(&server_row);

    let disconnect = gtk::Button::builder()
        .label("Disconnect from server")
        .css_classes(["destructive-action", "pill"])
        .halign(gtk::Align::Center)
        .margin_top(12)
        .build();
    let disconnect_group = adw::PreferencesGroup::new();
    disconnect_group.add(&disconnect);

    let about_group = adw::PreferencesGroup::builder().title("About").build();
    let about_row = adw::ActionRow::builder()
        .title("About Anchorage")
        .activatable(true)
        .build();
    about_row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    about_group.add(&about_row);

    page.add(&server_group);
    page.add(&disconnect_group);
    page.add(&about_group);

    let toolbar = adw::ToolbarView::builder().content(&page).build();
    toolbar.add_top_bar(&adw::HeaderBar::new());

    // Disconnect: confirm, clear keyring, return to connect page.
    disconnect.connect_clicked(clone!(
        #[strong] app,
        #[weak] nav,
        #[strong] toolbar,
        move |_| {
            let dialog = adw::AlertDialog::builder()
                .heading("Disconnect?")
                .body("The stored server URL and token will be removed.")
                .build();
            dialog.add_response("cancel", "Cancel");
            dialog.add_response("disconnect", "Disconnect");
            dialog.set_response_appearance("disconnect", adw::ResponseAppearance::Destructive);
            dialog.set_close_response("cancel");

            let app = app.clone();
            let nav = nav.clone();
            dialog.connect_response(None, move |_, response| {
                if response != "disconnect" {
                    return;
                }
                let app = app.clone();
                let nav = nav.clone();
                runtime::spawn(config::clear(), move |_res| {
                    show_connect(&app, &nav, String::new());
                });
            });
            dialog.present(Some(&toolbar));
        }
    ));

    // About dialog.
    about_row.connect_activated(clone!(
        #[strong] toolbar,
        move |_| {
            let about = adw::AboutDialog::builder()
                .application_name("Anchorage")
                .application_icon(crate::APP_ID)
                .version(env!("CARGO_PKG_VERSION"))
                .developer_name("Anchorage contributors")
                .license_type(gtk::License::Gpl30)
                .comments("A native GTK4/libadwaita client for Linkding.")
                .website("https://github.com/matv/anchorage")
                .build();
            about.present(Some(&toolbar));
        }
    ));

    toolbar
}
