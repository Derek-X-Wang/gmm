//! GameBanana URL ingest (slice 11).
//!
//! User pastes either a full mod URL (`https://gamebanana.com/mods/<id>`)
//! or a bare submission ID. GMM resolves the submission via the public
//! `apiv11` JSON endpoint, downloads the first `.zip` asset, and hands
//! it to the existing zip-import path. The resulting Mod row records
//! `source = 'gamebanana'` plus the metadata the UI surfaces (author,
//! version, screenshot, source URL).
//!
//! Network traffic goes through whatever `reqwest::Client` the caller
//! hands in — see [`Core::http_client`] for the proxy-aware variant.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::error::{Error, Result};

/// Where the GameBanana API lives. Production hard-codes
/// `https://gamebanana.com`; tests substitute a local mock server URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub api_base: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            api_base: "https://gamebanana.com".to_string(),
        }
    }
}

/// Metadata + first downloadable asset for one GameBanana submission.
/// Populated by [`fetch_submission`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GameBananaSubmission {
    pub id: u64,
    pub name: String,
    pub profile_url: String,
    pub author: Option<String>,
    pub version: Option<String>,
    pub screenshot_url: Option<String>,
    pub file_url: String,
    pub file_name: String,
}

/// Parse a user-supplied URL or bare ID. Returns `None` if neither
/// shape matches — the UI surfaces "Couldn't read that URL" so the
/// user can paste again.
///
/// Accepted forms:
///
/// * `1234567` — bare numeric submission ID.
/// * `https://gamebanana.com/mods/1234567` — full URL.
/// * `gamebanana.com/mods/1234567` — schemeless URL (browsers tolerate
///   this; we should too).
pub fn parse_url_or_id(input: &str) -> Option<u64> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(id) = trimmed.parse::<u64>() {
        return Some(id);
    }
    // Match `…/mods/<id>` anywhere in the string. We accept other
    // category names too (e.g. `/wips/<id>`) for forward-compatibility
    // with GameBanana's broader content tree.
    for prefix in ["/mods/", "/wips/", "/skins/", "/sounds/", "/textures/"] {
        if let Some(idx) = trimmed.find(prefix) {
            let rest = &trimmed[idx + prefix.len()..];
            let id_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(id) = id_str.parse::<u64>() {
                return Some(id);
            }
        }
    }
    None
}

/// Fetch a single submission's metadata. The API is GET
/// `<api_base>/apiv11/Mod/<id>?_csvProperties=...` and returns JSON.
///
/// We pick the first downloadable file's URL + name out of `_aFiles`.
/// The first preview-media image becomes the screenshot URL.
pub async fn fetch_submission(
    client: &reqwest::Client,
    endpoints: &Endpoints,
    id: u64,
) -> Result<GameBananaSubmission> {
    let url = format!(
        "{}/apiv11/Mod/{id}?\
         _csvProperties=_idRow,_sName,_sProfileUrl,_aPreviewMedia,_aFiles,_aSubmitter,_sVersion",
        endpoints.api_base
    );
    let res = client
        .get(&url)
        .send()
        .await
        .map_err(|e| Error::GameBanana(format!("GET {url}: {e}")))?
        .error_for_status()
        .map_err(|e| Error::GameBanana(format!("status {url}: {e}")))?;
    let json: serde_json::Value = res
        .json()
        .await
        .map_err(|e| Error::GameBanana(format!("parse {url}: {e}")))?;

    let name = json
        .get("_sName")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::GameBanana("missing _sName".to_string()))?
        .to_string();
    let profile_url = json
        .get("_sProfileUrl")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let author = json
        .get("_aSubmitter")
        .and_then(|s| s.get("_sName"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let version = json
        .get("_sVersion")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let screenshot_url = json
        .get("_aPreviewMedia")
        .and_then(|m| m.get("_aImages"))
        .and_then(|imgs| imgs.as_array())
        .and_then(|arr| arr.first())
        .and_then(|img| {
            let base = img.get("_sBaseUrl").and_then(|v| v.as_str())?;
            let file = img.get("_sFile").and_then(|v| v.as_str())?;
            Some(format!("{base}/{file}"))
        });

    let (file_url, file_name) = json
        .get("_aFiles")
        .and_then(|fs| fs.as_array())
        .and_then(|files| {
            files
                .iter()
                .find(|f| {
                    f.get("_sFile")
                        .and_then(|v| v.as_str())
                        .is_some_and(|s| s.to_ascii_lowercase().ends_with(".zip"))
                })
                .or_else(|| files.first())
        })
        .and_then(|file| {
            let url = file.get("_sDownloadUrl").and_then(|v| v.as_str())?;
            let name = file.get("_sFile").and_then(|v| v.as_str())?;
            Some((url.to_string(), name.to_string()))
        })
        .ok_or_else(|| Error::GameBanana("submission has no downloadable files".to_string()))?;

    Ok(GameBananaSubmission {
        id,
        name,
        profile_url,
        author,
        version,
        screenshot_url,
        file_url,
        file_name,
    })
}

/// Download the submission's first file to `dest`. Returns bytes
/// written. The caller is responsible for placing `dest` somewhere
/// safe (typically the GMM cache directory).
pub async fn download_to(client: &reqwest::Client, url: &str, dest: &Path) -> Result<u64> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let bytes = client
        .get(url)
        .send()
        .await
        .map_err(|e| Error::GameBanana(format!("GET {url}: {e}")))?
        .error_for_status()
        .map_err(|e| Error::GameBanana(format!("download {url}: {e}")))?
        .bytes()
        .await
        .map_err(|e| Error::GameBanana(format!("read {url}: {e}")))?;
    std::fs::write(dest, &bytes).map_err(|source| Error::Io {
        path: dest.to_path_buf(),
        source,
    })?;
    Ok(bytes.len() as u64)
}
