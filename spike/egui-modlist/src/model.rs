//! Local UI contracts and recorded service effects; no GMM Core or Tauri dependency.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchMod {
    pub mod_id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModResult {
    Updated,
    Failed(String),
    Unknown(String),
    Skipped(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchModResult {
    pub item: BatchMod,
    pub status: ModResult,
}

#[derive(Clone, Debug)]
pub enum ModUpdateProgress {
    Started(Vec<BatchMod>),
    Applying(BatchMod),
    Completed(BatchModResult),
    CheckingConflicts,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConflictReport {
    pub conflicts: Vec<Conflict>,
    pub per_mod_count: std::collections::BTreeMap<String, usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub hash: String,
    pub mod_ids: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ModUpdateBatchReport {
    pub stopped: bool,
    pub results: Vec<BatchModResult>,
    pub conflicts: Option<ConflictReport>,
    pub conflict_error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ModUpdateRow {
    pub item: BatchMod,
    pub upstream_ahead: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Apply {
        game: String,
        batch_id: u64,
    },
    Stop {
        batch_id: u64,
    },
    Invalidate(Vec<String>),
    SetConflicts {
        game: String,
        report: ConflictReport,
    },
}

/// The host delivers completion/progress events explicitly, like deferred promises
/// in the original component tests. A counter replaces UUIDs only in this stub.
#[derive(Default)]
pub struct StubService {
    pub calls: Vec<Command>,
    pub on_progress: Option<Box<dyn FnMut(ModUpdateProgress)>>,
    next_batch_id: u64,
}

impl StubService {
    pub fn apply(
        &mut self,
        game: &str,
        on_progress: impl FnMut(ModUpdateProgress) + 'static,
    ) -> u64 {
        self.on_progress = Some(Box::new(on_progress));
        self.next_batch_id += 1;
        let batch_id = self.next_batch_id;
        self.calls.push(Command::Apply {
            game: game.into(),
            batch_id,
        });
        batch_id
    }

    pub fn stop(&mut self, batch_id: u64) {
        self.calls.push(Command::Stop { batch_id });
    }

    pub fn emit(&mut self, event: ModUpdateProgress) {
        self.on_progress
            .as_mut()
            .expect("apply registered a progress callback")(event);
    }
}
