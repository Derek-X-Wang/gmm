use super::{stop_mod_update_batch, RegisteredBatchStop, BATCH_STOPS};

#[test]
fn explicit_stop_is_idempotent_scoped_and_retired_with_command() {
    let batch = RegisteredBatchStop::new();
    let other = RegisteredBatchStop::new();
    let id = batch.id.clone();
    stop_mod_update_batch(id.clone()).unwrap();
    stop_mod_update_batch(id.clone()).unwrap();
    assert!(batch.stop.is_requested());
    assert!(
        !other.stop.is_requested(),
        "a stop must target only its own batch"
    );
    drop(batch);
    assert!(!BATCH_STOPS.lock().unwrap().contains_key(&id));
    stop_mod_update_batch(id).unwrap();
}

#[test]
fn dropped_progress_receiver_never_requests_stop() {
    use crate::core::mod_update_batch::ModUpdateProgress;
    let batch = RegisteredBatchStop::new();
    let (tx, rx) = std::sync::mpsc::channel();
    drop(rx);
    super::report_batch_progress(
        &batch,
        ModUpdateProgress::Started { mods: vec![] },
        |event| tx.send(event),
    );
    super::report_batch_progress(&batch, ModUpdateProgress::CheckingConflicts, |event| {
        tx.send(event)
    });
    assert!(
        !batch.stop.is_requested(),
        "a dropped progress channel must not request a stop"
    );
}

#[test]
fn readiness_event_has_the_frontend_stop_token_shape() {
    use crate::core::mod_update_batch::ModUpdateProgress;
    let batch = RegisteredBatchStop::new();
    let mut events = Vec::new();
    super::report_batch_progress(
        &batch,
        ModUpdateProgress::Started { mods: vec![] },
        |event| {
            events.push(event);
            Ok::<_, ()>(())
        },
    );
    assert_eq!(
        serde_json::to_value(&events[1]).unwrap(),
        serde_json::json!({"phase":"stopReady", "batchId":batch.id})
    );
}
