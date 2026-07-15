//! Anchorage — a native GTK4/libadwaita client for Linkding.
//!
//! Entry point: sets up logging, loads GResources (CSS), and hands off to the
//! [`app`] module which builds the [`adw::Application`].

mod api;
mod app;
mod config;
mod runtime;
mod ui;

use gtk::prelude::*;

/// Reverse-DNS application id. Also used as the GSettings schema id and the
/// D-Bus name; keep it in sync with `data/*.gschema.xml` and the `.desktop` file.
pub const APP_ID: &str = "io.matv.Anchorage";

fn main() -> gtk::glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "anchorage=info,warn".into()),
        )
        .init();

    // Initialise libadwaita (also initialises GTK).
    adw::init().expect("failed to initialise libadwaita");

    let application = adw::Application::builder()
        .application_id(APP_ID)
        .build();

    application.connect_startup(|_| {
        ui::load_css();
    });

    application.connect_activate(app::build_ui);

    // We manage our own args (none of interest); pass an empty slice so GTK
    // doesn't try to parse cargo/test flags.
    application.run_with_args::<&str>(&[])
}
