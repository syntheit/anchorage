//! Persistent configuration: the server URL (GSettings) and the API token
//! (Secret Service via `oo7`, with the URL stored alongside as a keyring
//! attribute so both live together where the secret store is available).
//!
//! The URL is *also* mirrored into GSettings as a non-sensitive fallback so the
//! app can show the last server even before the keyring is unlocked.

use gtk::gio;
use gtk::prelude::*;

use crate::APP_ID;

/// GSettings key holding the last-used server base URL (non-secret fallback).
const KEY_SERVER_URL: &str = "server-url";

/// Keyring attributes identifying our single token item.
const KEYRING_APP: &str = "anchorage";
const KEYRING_KIND: &str = "server-token";

/// A resolved server connection: base URL + API token.
#[derive(Clone, Debug)]
pub struct Credentials {
    pub url: String,
    pub token: String,
}

/// Errors surfaced while loading/storing credentials.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("secret service error: {0}")]
    Keyring(#[from] oo7::Error),
}

/// Read the last server URL from GSettings (may be empty on first run).
pub fn stored_url() -> String {
    settings().string(KEY_SERVER_URL).to_string()
}

fn settings() -> gio::Settings {
    // Safe to call repeatedly; GSettings caches the backend per schema.
    gio::Settings::new(APP_ID)
}

/// Persist the URL to GSettings (non-secret mirror). Best-effort.
fn store_url(url: &str) {
    if let Err(err) = settings().set_string(KEY_SERVER_URL, url) {
        tracing::warn!(%err, "failed to persist server url to gsettings");
    }
}

/// Load credentials from the keyring. Returns `Ok(None)` when nothing is stored
/// yet (first run) — callers should route to onboarding.
///
/// Async: runs on the tokio runtime (oo7 is async + needs a reactor).
pub async fn load() -> Result<Option<Credentials>, ConfigError> {
    let keyring = oo7::Keyring::new().await?;
    let attrs = [("app", KEYRING_APP), ("kind", KEYRING_KIND)];
    let items = keyring.search_items(&attrs).await?;

    let Some(item) = items.first() else {
        return Ok(None);
    };

    if item.is_locked().await? {
        item.unlock().await?;
    }

    let secret = item.secret().await?;
    let token = String::from_utf8_lossy(secret.as_bytes()).into_owned();

    // The URL is stored as an item attribute; fall back to GSettings if absent.
    let item_attrs = item.attributes().await?;
    let url = item_attrs
        .get("url")
        .cloned()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(stored_url);

    if url.is_empty() || token.is_empty() {
        return Ok(None);
    }

    Ok(Some(Credentials { url, token }))
}

/// Store credentials, replacing any existing item. Mirrors the URL to GSettings.
///
/// Async: runs on the tokio runtime.
pub async fn store(creds: &Credentials) -> Result<(), ConfigError> {
    let keyring = oo7::Keyring::new().await?;
    let attrs = [
        ("app", KEYRING_APP),
        ("kind", KEYRING_KIND),
        ("url", creds.url.as_str()),
    ];
    keyring
        .create_item("Anchorage server token", &attrs, creds.token.as_bytes(), true)
        .await?;
    store_url(&creds.url);
    Ok(())
}

/// Remove stored credentials (used by "disconnect from server").
///
/// Async: runs on the tokio runtime.
pub async fn clear() -> Result<(), ConfigError> {
    let keyring = oo7::Keyring::new().await?;
    let attrs = [("app", KEYRING_APP), ("kind", KEYRING_KIND)];
    keyring.delete(&attrs).await?;
    Ok(())
}
