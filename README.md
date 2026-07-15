# Anchorage

A native GTK4/libadwaita desktop (and mobile) client for [Linkding](https://linkding.link/), the self-hosted bookmark manager.

The primary target is a phone running GNOME Shell Mobile (aarch64, ~360-430 px wide), though it scales up to the desktop fine. UX is loosely inspired by [Linkdy](https://github.com/JGeek00/linkdy) and MoeMemos, but built entirely on the native GNOME stack. App id: `io.matv.Anchorage`.

## What it does

On first launch you enter your Linkding server URL and API token (Linkding → Settings → Integrations → REST API). The token is stored in your keyring via the Secret Service; the URL goes to GSettings. From there you get a bookmark list with title, host, date, tags and status badges, debounced search (supports `#tag`, `!unread`), and an add sheet that auto-fills title/description/tags by calling `/api/bookmarks/check/` when you paste a URL. You can edit, archive, unarchive, delete (with a confirmation dialog), and open bookmarks in the system browser. There's a separate archived view and tag suggestions when adding.

Layout adapts with an `AdwBreakpoint` at 550 px: bottom view-switcher on phones, header switcher on the desktop.

## Build and run

Everything is in the Nix flake, no host Rust toolchain or GTK dev packages needed.

```sh
# Enter the dev shell (Rust toolchain, gtk4, libadwaita, pkg-config, ...).
# The shellHook compiles the GSettings schema and sets GSETTINGS_SCHEMA_DIR.
nix develop

cargo run
```

Or build the packaged binary:

```sh
nix build            # produces ./result/bin/anchorage
./result/bin/anchorage
```

If you run `cargo` outside the Nix shell you need to compile the GSettings schema yourself first (`glib-compile-schemas data` and `export GSETTINGS_SCHEMA_DIR=$PWD/data`), otherwise `gio::Settings` will abort on startup.

To build for aarch64 (e.g. for the phone):

```sh
nix build .#packages.aarch64-linux.anchorage
```

## Internals (brief)

Plain gtk4-rs + libadwaita, no relm4. The app is small enough that builder-pattern Rust stays readable without `.ui` templates or `CompositeTemplate` boilerplate.

`linkding-rs`, `oo7` and `reqwest` all need a tokio reactor that the GLib executor doesn't provide. `runtime::spawn` runs async work on a shared tokio runtime and delivers results back to the GTK thread via `async-channel` + `glib::spawn_future_local`. All network and keyring calls go through this path; errors surface as toasts.

The UI never touches wire types directly. `linkding_rs::Bookmark` doesn't implement `Clone`, so `api::BookmarkView` is the owned, cloneable view model that widgets store and pass around.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
