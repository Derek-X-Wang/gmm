//! Capture current Mod state into database-only, per-Game snapshots.
use serde::{Deserialize, Serialize};
use sqlx::Row;
use ulid::Ulid;

use super::{Core, Error, GameCode, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Preset {
    pub id: String,
    pub game: GameCode,
    pub name: String,
    pub created_at: String,
    pub mod_count: i64,
}

impl Core {
    pub async fn save_preset(&self, game: GameCode, name: &str) -> Result<Preset> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::InvalidPresetName);
        }
        let mut tx = self.pool.begin().await?;
        let id = Ulid::new().to_string();
        let created_at = chrono::Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO presets (id, game_code, name, created_at) VALUES (?, ?, ?, ?)")
            .bind(&id)
            .bind(game.as_str())
            .bind(name)
            .bind(&created_at)
            .execute(&mut *tx)
            .await
            .map_err(|error| {
                if error
                    .as_database_error()
                    .is_some_and(|error| error.is_unique_violation())
                {
                    Error::PresetNameTaken {
                        game: game.as_str().to_owned(),
                        name: name.to_owned(),
                    }
                } else {
                    Error::Database(error)
                }
            })?;
        let captured = sqlx::query(
            "INSERT INTO preset_entries (preset_id, mod_id, active_variant_id)
             SELECT ?, id, active_variant_id FROM mods WHERE game_code = ? AND enabled = 1",
        )
        .bind(&id)
        .bind(game.as_str())
        .execute(&mut *tx)
        .await?;
        let mod_count = captured.rows_affected() as i64;
        tx.commit().await?;
        Ok(Preset {
            id,
            game,
            name: name.to_owned(),
            created_at,
            mod_count,
        })
    }

    pub async fn list_presets(&self, game: GameCode) -> Result<Vec<Preset>> {
        let rows = sqlx::query(
            "SELECT p.id, p.name, p.created_at, COUNT(e.mod_id) AS mod_count
             FROM presets p LEFT JOIN preset_entries e ON e.preset_id = p.id
             WHERE p.game_code = ?
             GROUP BY p.id ORDER BY p.name, p.id",
        )
        .bind(game.as_str())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(Preset {
                    id: row.try_get("id")?,
                    game,
                    name: row.try_get("name")?,
                    created_at: row.try_get("created_at")?,
                    mod_count: row.try_get("mod_count")?,
                })
            })
            .collect()
    }
}
