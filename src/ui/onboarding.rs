//! First-run / connect page: collects the server URL + API token, validates
//! them against the server, persists on success, and invokes `on_connected`.
//!
//! Rendered as an `AdwNavigationPage` so it slots into the app's navigation
//! view (used both for first-run and for reconnecting from settings).

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib::clone;

use crate::api::Client;
use crate::config::{self, Credentials};
use crate::runtime;

/// Build the connect page. `on_connected` fires (on the GTK thread) with a live,
/// verified [`Client`] once the user connects successfully.
pub fn page<F>(prefill_url: &str, on_connected: F) -> adw::NavigationPage
where
    F: Fn(Client) + 'static,
{
    let on_connected = Rc::new(on_connected);

    let toasts = adw::ToastOverlay::new();

    let url_row = adw::EntryRow::builder()
        .title("Server URL")
        .text(prefill_url)
        .build();
    url_row.set_input_purpose(gtk::InputPurpose::Url);

    let token_row = adw::PasswordEntryRow::builder()
        .title("API token")
        .build();

    let server_group = adw::PreferencesGroup::builder()
        .title("Connect to Linkding")
        .description("Enter your server URL and the API token from Settings → Integrations → REST API.")
        .build();
    server_group.add(&url_row);
    server_group.add(&token_row);

    // Connect button + inline status.
    let connect_button = gtk::Button::builder()
        .label("Connect")
        .halign(gtk::Align::Center)
        .css_classes(["pill", "suggested-action"])
        .margin_top(12)
        .build();

    // Inline error banner: a failed connection surfaces here and stays visible
    // while the user corrects the URL/token. A successful connect hides it.
    let status = adw::Banner::builder().revealed(false).build();

    let action_group = adw::PreferencesGroup::new();
    let action_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    action_box.append(&connect_button);
    action_group.add(&action_box);

    // Guard against overlapping connect attempts.
    let in_flight = Rc::new(RefCell::new(false));

    connect_button.connect_clicked(clone!(
        #[strong] url_row,
        #[strong] token_row,
        #[strong] connect_button,
        #[strong] status,
        #[strong] toasts,
        #[strong] in_flight,
        #[strong] on_connected,
        move |_| {
            if *in_flight.borrow() {
                return;
            }

            let url = url_row.text().trim().to_string();
            let token = token_row.text().trim().to_string();

            if url.is_empty() || token.is_empty() {
                super::toast(&toasts, "Both the URL and token are required");
                return;
            }
            if url::Url::parse(&url).is_err() {
                super::toast(&toasts, "That doesn't look like a valid URL");
                return;
            }

            *in_flight.borrow_mut() = true;
            connect_button.set_sensitive(false);
            connect_button.set_label("Connecting…");
            // Clear any prior error while this attempt is in flight.
            status.set_revealed(false);

            let client = Client::new(&url, &token);
            let creds = Credentials { url, token };

            let verify_client = client.clone();
            runtime::spawn(
                async move {
                    // Verify, then persist. Persisting is also fallible (keyring).
                    verify_client.verify().await?;
                    config::store(&creds)
                        .await
                        .map_err(|e| crate::api::ApiError::Message(e.to_string()))?;
                    Ok::<(), crate::api::ApiError>(())
                },
                clone!(
                    #[strong] connect_button,
                    #[strong] status,
                    #[strong] in_flight,
                    #[strong] on_connected,
                    #[strong] client,
                    move |result| {
                        *in_flight.borrow_mut() = false;
                        connect_button.set_sensitive(true);
                        connect_button.set_label("Connect");
                        match result {
                            Ok(()) => {
                                status.set_revealed(false);
                                on_connected(client.clone());
                            }
                            Err(err) => {
                                status.set_title(&err.to_string());
                                status.set_revealed(true);
                            }
                        }
                    }
                ),
            );
        }
    ));

    let content = adw::PreferencesPage::new();
    content.add(&server_group);
    content.add(&action_group);

    let toolbar = adw::ToolbarView::builder().content(&content).build();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    // The error banner sits under the header, above the form.
    toolbar.add_top_bar(&status);
    toasts.set_child(Some(&toolbar));

    adw::NavigationPage::builder()
        .title("Connect")
        .tag("connect")
        .child(&toasts)
        .build()
}
