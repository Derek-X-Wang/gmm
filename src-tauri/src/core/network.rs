//! Network configuration (slice 14): HTTP/SOCKS5 proxy plus the
//! shared `reqwest::Client` builder.
//!
//! Why this lives in the Core: every outbound HTTP path (GitHub
//! release downloads, the GameBanana API, the future updater) must
//! route through the user's proxy if one is set. Centralising the
//! client builder here means a new HTTP call site can't forget to
//! honour the setting.
//!
//! Sensitive material: the proxy **password** is the only field in
//! GMM today that we treat as a secret. It is:
//!
//! * stored in its own settings key (`network.proxy.password`) so
//!   the URL serialisation can never accidentally leak it,
//! * never returned by [`Core::proxy_config_public`] (the variant the
//!   UI reads),
//! * never written into a diagnostics bundle — the `SettingsSnapshot`
//!   only serialises the host portion of the URL, with any
//!   `user:password@` userinfo always replaced by `REDACTED@`
//!   (defence-in-depth in case a user pastes credentials into the URL
//!   field by mistake).

use serde::{Deserialize, Serialize};

use super::error::{Error, Result};
use super::settings::{get as get_setting, put as put_setting};

/// Settings keys used by the network panel. Centralised so a typo
/// can't introduce a silent bug.
pub mod keys {
    pub const PROXY_URL: &str = "network.proxy.url";
    pub const PROXY_USERNAME: &str = "network.proxy.username";
    pub const PROXY_PASSWORD: &str = "network.proxy.password";
}

/// Full proxy configuration as stored in the settings table. The
/// password field is included; this struct never leaves the Core via
/// a Tauri command (the UI variant is [`ProxyConfigPublic`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProxyConfig {
    pub url: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl ProxyConfig {
    /// Returns the public-safe view of this config (no password).
    pub fn public(&self) -> ProxyConfigPublic {
        ProxyConfigPublic {
            url: self.url.clone(),
            username: self.username.clone(),
            password_set: self.password.is_some(),
        }
    }

    /// True when a non-empty proxy URL is configured.
    pub fn is_configured(&self) -> bool {
        self.url.as_deref().is_some_and(|s| !s.is_empty())
    }
}

/// Public view of the proxy config — exactly what the UI needs to
/// render. The password value never crosses this boundary; the
/// `password_set` flag lets the UI show "Set / Not set" without
/// leaking the secret.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProxyConfigPublic {
    pub url: Option<String>,
    pub username: Option<String>,
    pub password_set: bool,
}

/// Load the persisted proxy config. Returns the empty default if no
/// fields have ever been set.
pub async fn load(pool: &sqlx::SqlitePool) -> Result<ProxyConfig> {
    Ok(ProxyConfig {
        url: get_setting(pool, keys::PROXY_URL).await?,
        username: get_setting(pool, keys::PROXY_USERNAME).await?,
        password: get_setting(pool, keys::PROXY_PASSWORD).await?,
    })
}

/// Persist a proxy config. A `None` field clears the corresponding
/// setting (the row stays with a NULL value).
pub async fn save(pool: &sqlx::SqlitePool, cfg: &ProxyConfig) -> Result<()> {
    put_setting(pool, keys::PROXY_URL, cfg.url.as_deref()).await?;
    put_setting(pool, keys::PROXY_USERNAME, cfg.username.as_deref()).await?;
    put_setting(pool, keys::PROXY_PASSWORD, cfg.password.as_deref()).await?;
    Ok(())
}

/// Build a `reqwest::ClientBuilder` honouring `cfg`. Returns the
/// builder so callers can keep adding their own bits (timeouts,
/// user-agent overrides). The default builder we hand back includes
/// the GMM user-agent string.
pub fn client_builder(cfg: &ProxyConfig) -> Result<reqwest::ClientBuilder> {
    let mut builder =
        reqwest::Client::builder().user_agent("gmm/0.1 (+https://github.com/Derek-X-Wang/gmm)");
    if let Some(url) = cfg.url.as_ref().filter(|u| !u.is_empty()) {
        let mut proxy = reqwest::Proxy::all(url)
            .map_err(|e| Error::Network(format!("proxy URL {url}: {e}")))?;
        if let (Some(u), Some(p)) = (cfg.username.as_ref(), cfg.password.as_ref()) {
            if !u.is_empty() {
                proxy = proxy.basic_auth(u, p);
            }
        } else if let Some(u) = cfg.username.as_ref() {
            if !u.is_empty() {
                proxy = proxy.basic_auth(u, "");
            }
        }
        builder = builder.proxy(proxy);
    }
    Ok(builder)
}

/// Friendly classification of a reqwest error so the UI can show
/// "Proxy unreachable" instead of a generic network failure.
pub fn classify_error(err: &reqwest::Error, proxy_configured: bool) -> String {
    if proxy_configured && (err.is_connect() || err.is_timeout() || err.is_request()) {
        format!(
            "Proxy unreachable: {err}. Verify the URL, credentials, and that the proxy is running."
        )
    } else {
        err.to_string()
    }
}
