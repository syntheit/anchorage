# Anchorage

A native **GTK4 / libadwaita** client for [Linkding](https://linkding.link/),
the self-hosted bookmark manager.

Anchorage is **mobile-first**: the primary target is a phone running GNOME Shell
Mobile (aarch64, ~360–430 px wide), and it adapts cleanly up to the desktop. It
takes its UX cues from [Linkdy](https://github.com/JGeek00/linkdy) and MoeMemos
but is built entirely on the native GNOME stack.

App id: `io.matv.Anchorage`

> **Why "Anchorage"?** Bookmarks are *anchors* you drop on the web so you can
> return to them, and an anchorage is a sheltered place where you moor. The name
> is evocative, on-theme, and unclaimed on Flathub / crates.io.

## Features

- **Onboarding / settings** — enter your server URL + API token; validated
  against the server before it's saved.
- **Bookmark list** — title, host · date, tags and status badges; refresh;
  search bound to Linkding's `q` (debounced 300 ms; supports `#tag`, `!unread`).
- **Add bookmark** — paste a URL → **Validate** calls `/api/bookmarks/check/` →
  prefills title, description and suggested tags → save (Linkding upserts by URL).
  The clipboard is auto-checked when the URL looks like a link.
- **Edit / archive / unarchive / delete** — with a destructive-style delete
  confirmation. Open in the system browser.
- **Archived view** and existing-tag suggestions in the add sheet.
- **Adaptive** — single-column with a bottom view-switcher on phones; header
  switcher on the desktop, driven by an `AdwBreakpoint` at 550 px.
- **Secure credentials** — the API token lives in the Secret Service (via
  [`oo7`](https://crates.io/crates/oo7)); the server URL is mirrored to GSettings.

## Build & run

Everything is provided by the Nix flake — you don't need a host Rust toolchain
or GTK dev packages.

```sh
# Enter the dev shell (Rust toolchain, gtk4, libadwaita, pkg-config, …).
# The shellHook compiles the GSettings schema and sets GSETTINGS_SCHEMA_DIR.
nix develop

# Build and run.
cargo run
```

Or build the packaged binary directly:

```sh
nix build            # produces ./result/bin/anchorage
./result/bin/anchorage
```

On first launch, enter your Linkding **server URL** (e.g. `https://links.example.com`)
and an **API token** (Linkding → Settings → Integrations → REST API). The token is
stored in your keyring; you can disconnect from **Settings → Disconnect**.

> **Dev note on GSettings.** `gio::Settings` aborts the process if its schema
> isn't compiled. `nix develop` handles this (it runs `glib-compile-schemas data`
> and exports `GSETTINGS_SCHEMA_DIR=$PWD/data`). If you run `cargo` outside the
> shell, do that yourself first.

### Cross-compiling for the phone (aarch64)

The flake pins a `fenix` stable toolchain and works on `aarch64-linux` too.
Build natively on the phone, or use a remote aarch64 builder:

```sh
nix build .#packages.aarch64-linux.anchorage
```

## Architecture

Plain **gtk4-rs + libadwaita** (not relm4). The app is small enough that
builder-pattern construction in Rust stays legible and keeps everything
type-checked in one language — no `.ui`/Blueprint templates or `CompositeTemplate`
boilerplate. relm4's Elm-style message plumbing would add ceremony without a
clear payoff at this size, and staying close to the raw bindings makes this a
better *template* for the sibling Memos client.

```
src/
  main.rs        Entry point: logging, CSS, adw::Application wiring, APP_ID.
  runtime.rs     Async bridge: a shared multi-thread tokio runtime; runs Send
                 futures on a worker and marshals results to the GTK thread via
                 async-channel + glib::spawn_future_local.
  config.rs      Credentials: token in the Secret Service (oo7), URL mirrored to
                 GSettings. load / store / clear.
  api.rs         API layer over `linkding-rs`. Owns the UI-facing view models
                 (BookmarkView, Page, CheckResult, BookmarkDraft) and ApiError
                 (which recovers auth/network hints from reqwest).
  app.rs         Shell: window, first-run → connected transition, AdwViewStack
                 (Bookmarks / Archived / Settings), adaptive AdwBreakpoint,
                 settings + AboutDialog.
  ui/
    mod.rs       CSS (badge pills) + toast helper.
    models.rs    Pure presentation helpers (title/description fallbacks, host,
                 short date) — GTK-free and unit-tested.
    row.rs       A bookmark list row (title / snippet / meta + badges).
    list.rs      BookmarkList: search (debounced), refresh, pagination, per-row
                 action popover (open/edit/archive/delete + confirmations).
    add_sheet.rs Adaptive add/edit AdwDialog with the /check auto-fill flow and
                 tag suggestions.
    onboarding.rs  Connect page: URL + token, verify, persist.
data/
  io.matv.Anchorage.desktop        Desktop entry.
  io.matv.Anchorage.metainfo.xml   AppStream metadata.
  io.matv.Anchorage.gschema.xml    GSettings schema (server-url fallback).
```

### Async model

`linkding-rs`, `oo7` and `reqwest` need a tokio reactor, which the GLib executor
doesn't provide. `runtime::spawn` runs the `Send` future on the tokio runtime and
delivers the result to a `glib::spawn_future_local` closure on the main thread,
where `!Send` widget updates are legal. Every UI path uses this — no blocking
calls, and errors from the network or user input are surfaced as toasts rather
than panicking. The only `expect()`s are two unrecoverable startup invariants
(initialising libadwaita and building the tokio runtime); if either fails the
process cannot run at all, so it exits with a clear message instead of limping on.

## API layer

Uses the [`linkding-rs`](https://github.com/zbrox/linkding-rs) crate (`0.4`,
async + rustls). Anchorage wraps it so the UI never touches wire types directly:
`linkding_rs::Bookmark` doesn't derive `Clone`, so `api::BookmarkView` is the
owned, cloneable view model the widgets store and pass around.

## License

GPL-3.0-or-later, per the GNOME norm. See [LICENSE](LICENSE).
