//! The weekly Mod update check (ADR 0004, #461).
//!
//! GMM checks each Game's GameBanana Mods about once a week without being
//! asked, so the badge in the Mod row appears on its own. A scheduled
//! check does exactly what "Check now" does and nothing more: it refreshes
//! `upstream_version`. It never applies an update; that stays an explicit
//! click (ADR 0004).
//!
//! Cadence is per Game, because a check covers one Game. A Game is *due*
//! when its recorded last-check time (see
//! [`crate::core::mod_updates::keys::last_check_at`]) is missing, unreadable,
//! in the future, or a full [`CADENCE`] old. That time is only recorded
//! when a check reached upstream, so a check that reached nothing leaves
//! the Game due and the next pass tries again.
//!
//! The decision logic takes `now` as an argument so tests can drive a week
//! of cadence without waiting; only [`spawn`] reads the real clock.

use chrono::{DateTime, Duration, Utc};

use super::error::Result;
use super::gamebanana::Endpoints;
use super::games::{GameCode, GAME_PROFILES};
use super::mod_updates::keys;
use super::settings::get as get_setting;
use super::Core;

/// How old a Game's last reached check must be before it is due again.
pub const CADENCE: Duration = Duration::days(7);

/// How often a running GMM looks for a due Game. Looking is two database
/// reads per Game; only a due Game costs network. This is also the retry
/// spacing after a check that reached nothing.
const RECHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);

impl Core {
    /// Whether `game`'s Mods are due for their scheduled check at `now`.
    ///
    /// A missing, unparseable or future timestamp counts as due. The last
    /// case keeps one wrong system clock from silencing the check until
    /// real time catches up with the bad value.
    pub async fn mod_update_check_due(&self, game: GameCode, now: DateTime<Utc>) -> Result<bool> {
        let recorded = get_setting(&self.pool, &keys::last_check_at(game)).await?;
        let Some(last) = recorded
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&Utc))
        else {
            return Ok(true);
        };
        Ok(last > now || now - last >= CADENCE)
    }

    /// Check every due Game's Mods once. Returns the Games a check was
    /// attempted for, in the order tried.
    ///
    /// With the global toggle off this makes no network call at all. A
    /// Game with no Mod whose per-Mod check is on is not attempted, so it
    /// stays due and picks up a Mod imported later without waiting a week.
    pub async fn run_due_mod_update_checks(
        &self,
        endpoints: &Endpoints,
        now: DateTime<Utc>,
    ) -> Result<Vec<GameCode>> {
        if !self.mod_updates_globally_enabled().await? {
            return Ok(Vec::new());
        }
        let mut attempted = Vec::new();
        for profile in GAME_PROFILES {
            let game = profile.code;
            if !self.mod_update_check_due(game, now).await? {
                continue;
            }
            if !self
                .list_mod_updates(game)
                .await?
                .iter()
                .any(|row| row.update_check_enabled)
            {
                continue;
            }
            // One Game's failure must not starve the others.
            match self.check_mod_updates_at(game, endpoints, now).await {
                Ok(report) => {
                    // Nobody is watching a scheduled check, so the log is
                    // where a partial failure is recorded. The per-Mod
                    // detail is already logged by the check itself.
                    tracing::info!(
                        target: "gmm::mod_updates",
                        game = game.as_str(),
                        unchecked = report.unchecked.len(),
                        "scheduled mod update check finished",
                    );
                }
                Err(e) => tracing::warn!(
                    target: "gmm::mod_updates",
                    game = game.as_str(),
                    error = %e,
                    "scheduled mod update check could not run",
                ),
            }
            attempted.push(game);
        }
        Ok(attempted)
    }
}

/// Start the background check on its own thread and return at once.
///
/// Nothing waits on it and nothing it does can stop GMM from starting: a
/// thread or runtime that cannot be created is logged and dropped, as is
/// a pass that errors. The loop checks on launch and then every
/// [`RECHECK_EVERY`], so a GMM left open for weeks still checks weekly.
pub fn spawn(core: Core) {
    let started = std::thread::Builder::new()
        .name("mod-update-schedule".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::warn!(
                        target: "gmm::mod_updates",
                        error = %e,
                        "scheduled mod update check could not start a runtime",
                    );
                    return;
                }
            };
            rt.block_on(async move {
                let endpoints = Endpoints::default();
                loop {
                    if let Err(e) = core.run_due_mod_update_checks(&endpoints, Utc::now()).await {
                        tracing::warn!(
                            target: "gmm::mod_updates",
                            error = %e,
                            "scheduled mod update pass failed",
                        );
                    }
                    tokio::time::sleep(RECHECK_EVERY).await;
                }
            });
        });
    if let Err(e) = started {
        tracing::warn!(
            target: "gmm::mod_updates",
            error = %e,
            "scheduled mod update check could not start a thread",
        );
    }
}
