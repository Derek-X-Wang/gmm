//! Mod inspector preference. CSS pixels deliberately stay outside SavedWindowState.
use super::{settings, Core, Result};

const MIN_WIDTH: u32 = 240;
const MAX_WIDTH: u32 = 560;

impl Core {
    pub async fn mod_panel_width(&self) -> Result<Option<u32>> {
        Ok(settings::get(&self.pool, settings::keys::mod_panel_width())
            .await?
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|width| (MIN_WIDTH..=MAX_WIDTH).contains(width)))
    }

    pub async fn set_mod_panel_width(&self, width: u32) -> Result<()> {
        let value = width.clamp(MIN_WIDTH, MAX_WIDTH).to_string();
        settings::put(&self.pool, settings::keys::mod_panel_width(), Some(&value)).await
    }
}
