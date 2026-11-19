//! The weekly Mod update check (ADR 0004, #461).
//!
//! GMM checks each Game's GameBanana Mods about once a week without being
//! asked, so the badge in the Mod row appears on its own. A scheduled
//! check does exactly what "Check now" does and nothing more: it refreshes
//! `upstream_version`. It never applies an update; that stays an explicit
//! click (ADR 0004).
//!
//! Cadence is per Mod (#471). Each Mod records when GameBanana last
//! answered about it, `mods.upstream_reached_at`; a Mod is *due* when that
//! is missing, unreadable, in the future, or a full [`CADENCE`] old. An
//! answer that the Mod is gone counts, so a deleted submission settles. No
//! answer (offline, a dead proxy, a server failure) records nothing, so
//! that Mod alone stays due and the next pass, hours later, retries it.
//!
//! The pass still walks Games, because a check covers one Game: a Game is
//! due when any Mod it would poll is due, and its check then polls only
//! the Mods that are due.
//!
//! The decision logic takes `now` as an argument so tests can drive a week
//! of cadence without waiting; only [`spawn`] reads the real clock.

use chrono::{DateTime, Duration, Utc};

use super::error::Result;
use super::gamebanana::Endpoints;
use super::games::{GameCode, GAME_PROFILES};
use super::mods::Source;
use super::Core;

/// How old a Mod's last answer from upstream must be before it is due again.
pub const CADENCE: Duration = Duration::days(7);

/// How often a running GMM looks for a due Mod. Looking is one database
/// read per Game; only a due Mod costs network. This is also the retry
/// spacing for a Mod upstream did not answer about.
const RECHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);

/// Whether a Mod that upstream last answered about at `reached_at` is due
/// at `now`.
///
/// A missing, unparseable or future time counts as due. The last case
/// keeps one wrong system clock from silencing the check until real time
/// catches up with the bad value.
pub(crate) fn mod_due(reached_at: Option<&str>, now: DateTime<Utc>) -> bool {
    let Some(last) = reached_at
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.with_timezone(&Utc))
    else {
        return true;
    };
    last > now || now - last >= CADENCE
}

impl Core {
    /// Whether any of `game`'s Mods is due for its scheduled check at
    /// `now`. Only Mods a check would poll count: a GameBanana Mod with a
    /// submission ID and its own check on.
    pub async fn mod_update_check_due(&self, game: GameCode, now: DateTime<Utc>) -> Result<bool> {
        let recorded: Vec<Option<String>> = sqlx::query_scalar(
            "SELECT upstream_reached_at FROM mods
             WHERE game_code = ? AND source = ?
               AND update_check_enabled != 0 AND gamebanana_id IS NOT NULL",
        )
        .bind(game.as_str())
        .bind(Source::Gamebanana.as_str())
        .fetch_all(&self.pool)
        .await?;
        Ok(recorded.iter().any(|r| mod_due(r.as_deref(), now)))
    }

    /// Check every due Game's Mods once. Returns the Games a check was
    /// attempted for, in the order tried.
    ///
    /// With the global toggle off this makes no network call at all. A
    /// Game with no Mod whose per-Mod check is on is not due, and a Mod
    /// imported later is due at once because it has never been answered
    /// about.
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
            // One Game's failure must not starve the others.
            match self.poll_mod_updates(game, endpoints, now, true).await {
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
