//! A Mod's screenshot for the detail panel (#437).
//!
//! The bytes are fetched here, in the Core, through [`Core::http_client_builder`]
//! — the proxy-aware client from [`super::network`]. They are never fetched by
//! the WebView. A WebView image load does not go through `reqwest`, so it cannot
//! honour `network.proxy.url`: the image would fail for the users the proxy
//! exists for, and for a user who *has* configured one it would disclose their
//! real IP address, and which Mod they are looking at, to GameBanana on every
//! selection change. The frontend therefore gets a self-contained `data:` URI
//! and never sees a remote URL it could load. This needs no custom URI scheme,
//! no CSP change and no object-URL lifecycle in React.
//!
//! # Validation
//!
//! `screenshot_url` is stored verbatim from the GameBanana API, so it is
//! untrusted. [`validate_screenshot_url`] rejects anything that is not `https`
//! on `gamebanana.com` or a subdomain of it, and anything carrying userinfo.
//! The response must be `image/*` and at most [`MAX_IMAGE_BYTES`]; a larger body
//! is "unavailable", never a truncated image. Every redirect hop is validated
//! the same way ([`redirect_policy`]).
//!
//! A rejected URL is logged as a `tracing::warn!` that names only the reason,
//! never the URL: a URL can carry `user:password@` userinfo, and the proxy
//! password is the one secret in GMM (see [`super::network`]).
//!
//! # Caching
//!
//! In memory only: a small LRU keyed by the validated URL, capped at
//! [`CACHE_MAX_ENTRIES`] entries or [`CACHE_MAX_BYTES`] bytes, whichever binds
//! first. It survives panel reopens within a session and is gone on exit.
//! There is deliberately **no disk cache**, so there is no cache directory,
//! no eviction policy to maintain, no migration debt and no ADR. A disk cache
//! is a separate decision for later, once there is evidence that re-fetching
//! actually annoys anyone. Failures are never cached, so a retry after fixing
//! the proxy works without restarting.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use serde::Serialize;

use super::{network, Core};

/// Response body cap. Larger is `unavailable`, not truncated.
pub const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
/// LRU entry cap.
pub const CACHE_MAX_ENTRIES: usize = 16;
/// LRU size cap, measured on the stored data URI text.
pub const CACHE_MAX_BYTES: usize = 32 * 1024 * 1024;
/// Redirect hops followed, each re-validated.
const MAX_REDIRECTS: usize = 3;

/// What the panel shows, tagged like `gamePath.status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ModScreenshot {
    Image {
        data_uri: String,
    },
    /// About the **image**, never about the Mod. `reason` is one short sentence.
    Unavailable {
        reason: String,
    },
}

impl ModScreenshot {
    fn unavailable(reason: impl Into<String>) -> Self {
        Self::Unavailable {
            reason: reason.into(),
        }
    }
}

/// Accept only `https` on `gamebanana.com` or a subdomain, without userinfo.
/// The `Err` is a short reason that is safe to log and to show: it never
/// contains the URL.
pub fn validate_screenshot_url(raw: &str) -> Result<reqwest::Url, &'static str> {
    let url = reqwest::Url::parse(raw.trim()).map_err(|_| "the address is not a valid URL")?;
    allowed(&url)?;
    Ok(url)
}

fn allowed(url: &reqwest::Url) -> Result<(), &'static str> {
    if url.scheme() != "https" {
        return Err("the address is not https");
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("the address carries credentials");
    }
    // `Url` lowercases the host and, for a special scheme, rejects an IP
    // literal that is not well formed. Comparing whole labels is what stops
    // `gamebanana.com.example.test` and `notgamebanana.com`.
    let host = url
        .host_str()
        .map(|h| h.trim_end_matches('.'))
        .ok_or("the address has no host")?;
    if host == "gamebanana.com" || host.ends_with(".gamebanana.com") {
        Ok(())
    } else {
        Err("the host is not gamebanana.com")
    }
}

/// Redirect policy for the screenshot request: a hop to anything
/// [`validate_screenshot_url`] would refuse is not followed. The refused
/// redirect response is returned to the caller, which treats a non-2xx status
/// as "unavailable".
pub fn redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= MAX_REDIRECTS {
            return attempt.stop();
        }
        match allowed(attempt.url()) {
            Ok(()) => attempt.follow(),
            Err(_) => attempt.stop(),
        }
    })
}

/// A fetch that did not produce an image. `reason` is shown to the user.
#[derive(Debug, PartialEq, Eq)]
pub struct FetchFailure {
    pub reason: String,
}

fn failure(reason: impl Into<String>) -> FetchFailure {
    FetchFailure {
        reason: reason.into(),
    }
}

/// GET `url` with `client`, enforcing the status, content-type and size rules,
/// and return a `data:` URI. `url` must already have passed
/// [`validate_screenshot_url`]; the client is injected so a test can point it at
/// a loopback server exactly as the other GameBanana tests do.
pub async fn fetch_data_uri(
    client: &reqwest::Client,
    url: &reqwest::Url,
    proxy_configured: bool,
) -> Result<String, FetchFailure> {
    // `reqwest::Error` text embeds the request URL; it is validated and has no
    // userinfo, so it is safe to show.
    let mut response = client
        .get(url.clone())
        .send()
        .await
        .map_err(|e| failure(network::classify_error(&e, proxy_configured)))?;
    if !response.status().is_success() {
        return Err(failure(format!(
            "GameBanana answered with status {}.",
            response.status().as_u16()
        )));
    }
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(image_mime)
        .ok_or_else(|| failure("The address did not return an image."))?;
    if response
        .content_length()
        .is_some_and(|len| len > MAX_IMAGE_BYTES as u64)
    {
        return Err(failure("The image is larger than GMM will load."));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| failure(network::classify_error(&e, proxy_configured)))?
    {
        if body.len() + chunk.len() > MAX_IMAGE_BYTES {
            return Err(failure("The image is larger than GMM will load."));
        }
        body.extend_from_slice(&chunk);
    }
    if body.is_empty() {
        return Err(failure("The address returned an empty image."));
    }
    Ok(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&body)
    ))
}

/// The bare `image/<subtype>` from a Content-Type header, or `None`. The
/// subtype is restricted to token characters so it cannot break out of the
/// data URI it is spliced into.
fn image_mime(content_type: &str) -> Option<String> {
    let essence = content_type.split(';').next()?.trim().to_ascii_lowercase();
    let subtype = essence.strip_prefix("image/")?;
    let ok = !subtype.is_empty()
        && subtype
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'));
    ok.then_some(essence)
}

/// Bounded least-recently-used store of data URIs, keyed by validated URL.
#[derive(Debug, Default)]
pub struct ScreenshotCache {
    /// Front = least recently used.
    entries: VecDeque<(String, Arc<str>)>,
    bytes: usize,
}

impl ScreenshotCache {
    pub fn get(&mut self, key: &str) -> Option<Arc<str>> {
        let index = self.entries.iter().position(|(k, _)| k == key)?;
        let entry = self.entries.remove(index)?;
        let value = Arc::clone(&entry.1);
        self.entries.push_back(entry);
        Some(value)
    }

    pub fn insert(&mut self, key: String, value: Arc<str>) {
        if let Some(index) = self.entries.iter().position(|(k, _)| *k == key) {
            if let Some((_, old)) = self.entries.remove(index) {
                self.bytes -= old.len();
            }
        }
        // An entry that alone exceeds the byte cap would evict everything and
        // still not fit. The 8 MiB fetch cap keeps this unreachable in
        // production; refuse it rather than empty the cache.
        if value.len() > CACHE_MAX_BYTES {
            return;
        }
        self.bytes += value.len();
        self.entries.push_back((key, value));
        while self.entries.len() > CACHE_MAX_ENTRIES || self.bytes > CACHE_MAX_BYTES {
            if let Some((_, evicted)) = self.entries.pop_front() {
                self.bytes -= evicted.len();
            }
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

pub(super) type SharedScreenshotCache = Arc<Mutex<ScreenshotCache>>;

impl Core {
    /// The screenshot for `mod_id`, fetched through the user's proxy.
    ///
    /// Every failure to produce an image is `Unavailable`, a statement about
    /// the image. A Mod with no stored address is also `Unavailable`: the
    /// frontend never asks in that case, so reaching here means the record
    /// changed under it.
    pub async fn mod_screenshot(&self, mod_id: &str) -> super::Result<ModScreenshot> {
        let stored: Option<Option<String>> =
            sqlx::query_scalar("SELECT screenshot_url FROM mods WHERE id = ?")
                .bind(mod_id)
                .fetch_optional(&self.pool)
                .await?;
        let Some(raw) = stored.flatten().filter(|s| !s.trim().is_empty()) else {
            return Ok(ModScreenshot::unavailable(
                "GMM has no screenshot address on record for this Mod.",
            ));
        };
        let url = match validate_screenshot_url(&raw) {
            Ok(url) => url,
            Err(why) => {
                tracing::warn!(mod_id, why, "mod screenshot address rejected");
                return Ok(ModScreenshot::unavailable(
                    "GMM only loads screenshots from gamebanana.com over https.",
                ));
            }
        };
        let key = url.as_str().to_owned();
        if let Some(hit) = self.screenshots.lock().ok().and_then(|mut c| c.get(&key)) {
            return Ok(ModScreenshot::Image {
                data_uri: hit.to_string(),
            });
        }

        // The same proxy-aware builder every outbound path uses; only the
        // redirect policy is added. A builder error can embed the proxy URL,
        // which may carry userinfo, so it is neither logged nor shown.
        let client = self
            .http_client_builder()
            .await
            .ok()
            .and_then(|b| b.redirect(redirect_policy()).build().ok());
        let Some(client) = client else {
            tracing::warn!(mod_id, "mod screenshot client could not be built");
            return Ok(ModScreenshot::unavailable(
                "GMM could not use the network settings to load the image.",
            ));
        };
        let proxy_configured = self.proxy_config().await?.is_configured();
        match fetch_data_uri(&client, &url, proxy_configured).await {
            Ok(data_uri) => {
                if let Ok(mut cache) = self.screenshots.lock() {
                    cache.insert(key, Arc::from(data_uri.as_str()));
                }
                Ok(ModScreenshot::Image { data_uri })
            }
            Err(FetchFailure { reason }) => {
                tracing::warn!(mod_id, "mod screenshot fetch failed");
                Ok(ModScreenshot::Unavailable { reason })
            }
        }
    }
}
