use std::collections::{BTreeMap, BTreeSet};
use std::{cell::RefCell, rc::Rc};

use egui::{
    Ui,
    accesskit::{Live, Role},
};

use crate::model::*;

#[derive(Default)]
struct BatchView {
    batch_id: u64,
    mods: Vec<BatchMod>,
    results: Vec<BatchModResult>,
    reading: String,
    started: bool,
    current: Option<BatchMod>,
    scanning: bool,
    stop_requested: bool,
    stop_error: Option<String>,
    report: Option<ModUpdateBatchReport>,
    error: Option<String>,
}

pub struct ModUpdateBatchAction {
    pub game: String,
    pub rows: Vec<ModUpdateRow>,
    pub verified: bool,
    pub checking: bool,
    /// None means the session reading is unverified; Some(true) means active.
    pub session: Option<bool>,
    pub service: StubService,
    batches: BTreeMap<String, BatchView>,
    pending: Option<String>,
    requested_stops: BTreeSet<u64>,
    progress_queue: Rc<RefCell<Vec<(String, ModUpdateProgress)>>>,
}

impl ModUpdateBatchAction {
    pub fn new(game: &str, rows: Vec<ModUpdateRow>) -> Self {
        Self {
            game: game.into(),
            rows,
            verified: true,
            checking: false,
            session: Some(false),
            service: StubService::default(),
            batches: BTreeMap::new(),
            pending: None,
            requested_stops: BTreeSet::new(),
            progress_queue: Rc::default(),
        }
    }

    pub fn batch_id(&self, game: &str) -> u64 {
        self.batches[game].batch_id
    }

    fn start(&mut self) {
        let queue = self.progress_queue.clone();
        let game = self.game.clone();
        let batch_id = self.service.apply(&self.game, move |event| {
            queue.borrow_mut().push((game.clone(), event));
        });
        let mods = self
            .rows
            .iter()
            .filter(|r| r.upstream_ahead)
            .map(|r| r.item.clone())
            .collect();
        self.batches.insert(
            self.game.clone(),
            BatchView {
                batch_id,
                mods,
                reading: "Starting updates…".into(),
                ..Default::default()
            },
        );
        self.pending = Some(self.game.clone());
    }

    pub fn progress(&mut self, game: &str, event: ModUpdateProgress) {
        let batch = self.batches.get_mut(game).expect("batch started");
        batch.started = true;
        match event {
            ModUpdateProgress::Started(mods) => batch.mods = mods,
            ModUpdateProgress::Applying(item) => {
                batch.reading = format!("Updating {}…", item.name);
                batch.current = Some(item);
            }
            ModUpdateProgress::Completed(result) => batch.results.push(result),
            ModUpdateProgress::CheckingConflicts => {
                batch.current = None;
                batch.scanning = true;
                batch.reading = "Checking Conflicts…".into();
            }
        }
    }

    /// Duplicate deliveries share the same batch fence, even before another frame.
    pub fn request_stop(&mut self, game: &str, batch_id: u64) {
        if !self.requested_stops.insert(batch_id) {
            return;
        }
        let batch = self.batches.get_mut(game).expect("batch started");
        batch.stop_requested = true;
        batch.stop_error = None;
        self.service.stop(batch_id);
    }

    pub fn stop_failed(&mut self, game: &str, batch_id: u64, error: &str) {
        self.requested_stops.remove(&batch_id);
        let batch = self.batches.get_mut(game).expect("batch started");
        if batch.batch_id != batch_id || batch.report.is_some() || batch.error.is_some() {
            return;
        }
        batch.stop_requested = false;
        batch.stop_error = Some(format!(
            "Stop request unverified: {error} The batch may continue. Try again."
        ));
    }

    pub fn finish(&mut self, game: &str, report: ModUpdateBatchReport) {
        let batch = self.batches.get_mut(game).expect("batch started");
        batch.current = None;
        batch.stop_error = None;
        batch.results = report.results.clone();
        batch.reading = if report.stopped {
            "Batch stopped."
        } else {
            "Batch finished."
        }
        .into();
        if let Some(conflicts) = &report.conflicts
            && report.conflict_error.is_none()
        {
            self.service.calls.push(Command::SetConflicts {
                game: game.into(),
                report: conflicts.clone(),
            });
        }
        batch.report = Some(report);
        self.settled(game);
    }

    pub fn apply_failed(&mut self, game: &str, error: &str) {
        let batch = self.batches.get_mut(game).expect("batch started");
        batch.reading = if batch.started {
            "Batch result unavailable."
        } else {
            "Batch refused before updates started."
        }
        .into();
        batch.error = Some(error.into());
        batch.results = if !batch.started {
            vec![]
        } else {
            batch
                .mods
                .iter()
                .map(|item| {
                    batch
                        .results
                        .iter()
                        .find(|r| r.item.mod_id == item.mod_id)
                        .cloned()
                        .unwrap_or_else(|| BatchModResult {
                            item: item.clone(),
                            status: ModResult::Unknown(
                                "No result received. Check the Library before retrying.".into(),
                            ),
                        })
                })
                .collect()
        };
        self.settled(game);
    }

    fn settled(&mut self, game: &str) {
        self.pending = None;
        for key in ["mods", "modUpdates", "conflicts"] {
            self.service
                .calls
                .push(Command::Invalidate(vec![key.into(), game.into()]));
        }
        for key in ["variants", "attentionStatus", "libraryAudit"] {
            self.service
                .calls
                .push(Command::Invalidate(vec![key.into()]));
        }
    }

    pub fn show(&mut self, ui: &mut Ui) {
        let events = std::mem::take(&mut *self.progress_queue.borrow_mut());
        for (game, event) in events {
            self.progress(&game, event);
        }
        let enabled = self.pending.is_none()
            && !self.checking
            && self.verified
            && self.rows.iter().any(|r| r.upstream_ahead)
            && self.session == Some(false);
        let apply_name = if self.pending.is_some() {
            "Applying Mod updates…"
        } else {
            "Apply all Mod updates"
        };
        if ui
            .add_enabled(enabled, egui::Button::new(apply_name))
            .clicked()
        {
            self.start();
        }
        let pending_here = self.pending.as_deref() == Some(&self.game);
        if pending_here
            && let Some(batch) = self.batches.get(&self.game)
            && !batch.scanning
        {
            let id = batch.batch_id;
            if ui
                .add_enabled(
                    !batch.stop_requested,
                    egui::Button::new("Stop after this Mod"),
                )
                .clicked()
            {
                self.request_stop(&self.game.clone(), id);
            }
        }
        if self.session == Some(true) {
            text(
                ui,
                Role::Paragraph,
                "End the Game Session before applying Mod updates.",
            );
        }
        if !self.verified {
            text(ui, Role::Paragraph, "Available Mod updates are unverified.");
        }
        text(
            ui,
            Role::Paragraph,
            "Updates run one at a time. Stop after this Mod lets the current Mod finish its metadata fetch, download, and swap. Keep GMM open until it finishes.",
        );
        if let Some(game) = &self.pending
            && game != &self.game
        {
            text(
                ui,
                Role::Status,
                &format!("Mod updates are running for {game}."),
            );
        }
        let Some(batch) = self.batches.get(&self.game) else {
            return;
        };
        let status = if pending_here {
            format!(
                "{} {} of {} completed.",
                batch.reading,
                batch.results.len(),
                batch.mods.len()
            )
        } else {
            batch.reading.clone()
        };
        text(ui, Role::Status, &status);
        if pending_here
            && batch.stop_requested
            && let Some(item) = &batch.current
        {
            text(
                ui,
                Role::Paragraph,
                &format!(
                    "Stop requested. Waiting for {} to finish; later Mods will not start.",
                    item.name
                ),
            );
        }
        if let Some(error) = &batch.stop_error {
            text(ui, Role::Alert, error);
        }
        ui.scope(|ui| {
            ui.ctx()
                .accesskit_node_builder(ui.scope_id(), |node| node.set_role(Role::List));
            for result in &batch.results {
                let outcome = match &result.status {
                    ModResult::Updated => "Updated".into(),
                    ModResult::Failed(reason) => format!("Failed — {reason}"),
                    ModResult::Skipped(reason) => format!("Skipped — {reason}"),
                    ModResult::Unknown(reason) => format!("Unknown — {reason}"),
                };
                text(
                    ui,
                    Role::ListItem,
                    &format!("{}: {outcome}", result.item.name),
                );
            }
        });
        if let Some(error) = &batch.error {
            text(
                ui,
                Role::Alert,
                &format!(
                    "{error}{}",
                    if batch.started {
                        " Conflict check result unavailable."
                    } else {
                        ""
                    }
                ),
            );
        }
        if let Some(report) = &batch.report {
            if let Some(error) = &report.conflict_error {
                text(ui, Role::Alert, &format!("Conflicts unverified: {error}"));
            }
            if let Some(conflicts) = &report.conflicts {
                let count = conflicts.conflicts.len();
                if report.conflict_error.is_none() {
                    text(
                        ui,
                        Role::Paragraph,
                        &format!("{count} Conflicts found after the batch."),
                    );
                } else if count > 0 {
                    text(
                        ui,
                        Role::Paragraph,
                        &format!("{count} known Conflicts found; the full result is unverified."),
                    );
                }
                for conflict in &conflicts.conflicts {
                    let names = conflict
                        .mod_ids
                        .iter()
                        .map(|id| {
                            self.rows
                                .iter()
                                .find(|r| &r.item.mod_id == id)
                                .map_or(id.as_str(), |r| &r.item.name)
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    text(
                        ui,
                        Role::ListItem,
                        &format!("Hash {}: {names}", conflict.hash),
                    );
                }
            }
        }
    }
}

/// Real rendered labels receive semantics every frame, not a separate test tree.
/// egui has no status convenience widget; AccessKit supplies the role/live flags.
pub fn text(ui: &mut Ui, role: Role, content: &str) {
    let response = ui.label(content);
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_role(role);
        node.set_label(content);
        if role == Role::Status || role == Role::Alert {
            node.set_live(if role == Role::Alert {
                Live::Assertive
            } else {
                Live::Polite
            });
            node.set_live_atomic();
        }
    });
}
