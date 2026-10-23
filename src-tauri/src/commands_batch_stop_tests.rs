use super::{stop_mod_update_batch, RegisteredBatchStop, BATCH_STOPS};

#[test]
fn explicit_stop_is_idempotent_scoped_and_retired_with_command() {
    let batch = RegisteredBatchStop::new(ulid::Ulid::new().to_string());
    let other = RegisteredBatchStop::new(ulid::Ulid::new().to_string());
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

#[tokio::test]
async fn dropped_progress_receiver_never_requests_stop() {
    use tauri::Manager;
    let tmp = tempfile::TempDir::new().unwrap();
    let core = crate::core::Core::new(tmp.path().join("library"), "sqlite::memory:")
        .await
        .unwrap();
    let app = tauri::test::mock_app();
    app.manage(core);
    let batch = RegisteredBatchStop::new(ulid::Ulid::new().to_string());
    let (tx, rx) = std::sync::mpsc::channel();
    drop(rx);
    let channel = tauri::ipc::Channel::new(move |event| {
        tx.send(event)
            .map_err(|error| std::io::Error::other(error.to_string()).into())
    });
    super::apply_all_mod_updates(
        app.state(),
        crate::core::GameCode::Gimi,
        batch.id.clone(),
        channel,
    )
    .await
    .unwrap();
    assert!(
        !batch.stop.is_requested(),
        "a dropped progress channel must not request a stop"
    );
}
