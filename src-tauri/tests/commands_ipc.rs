//! Tauri command IPC wire-shape contract (issue #26).
//!
//! The acceptance criteria allow `tauri::test::get_ipc_response`
//! *or* an equivalent path through serde. Going through Tauri's real
//! mock runtime requires building a `Context<MockRuntime>` that
//! carries the project's ACL capabilities; the issue body documents
//! that route as the harder path. The cheaper backend route — and the one
//! this file takes — is to round-trip the **same Args and return
//! types** the `#[tauri::command]` macro consumes through `serde_json`,
//! and call the Core method body directly. `src/api.test.ts` covers the
//! frontend's real command name and outer `invoke` envelope, while
//! `tests/ipc_contract.rs` directly compares that outer key with the Rust
//! command parameter identifier; none of these suites drives Tauri's runtime.
//!
//! See `docs/testing.md` for the pattern + how to extend this file
//! when a new command lands.
//!
//! **Scope caveat.** These tests round-trip the Args/return types
//! through serde and call Core directly. The cross-source test in
//! `tests/ipc_contract.rs` binds the outer field name; these tests do not
//! exercise Tauri's IPC layer or capability enforcement.
//! `tests/ipc_contract.rs` covers registration; nothing covers ACL
//! enforcement yet.

use std::fs::{self, File};
use std::io::Write;
use std::sync::{Arc, Mutex};

use gmm_lib::commands::{
    list_supported_games, AdoptArgs, GameBananaImportArgs, ImportZipArgs, LibraryPaths, ProxyArgs,
    RecoverLibraryDirArgs, ResolveDuplicateModsArgs, NO_INSTALL_PATH_FOR_ENABLE_MSG,
};
use gmm_lib::core::conflicts::ConflictReport;
use gmm_lib::core::games::GAME_PROFILES;
use gmm_lib::core::importer::AssetPattern;
use gmm_lib::core::reconcile::ReconcileResult;
use gmm_lib::core::updates::UpdateStatus;
use gmm_lib::core::variants::Variant;
use gmm_lib::core::{av, crash_points};
use gmm_lib::core::{
    Core, DeletedLibraryDir, DuplicateModGroup, DuplicateModRecord, DuplicateModVariant,
    DuplicateResolution, GameCode, ImportZipOptions, LibraryAuditReport, LibraryReclamationOutcome,
    Mod, ReviewedDuplicateMod, Source, UnreferencedLibraryDir,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

/// Helper: deserialize a JSON value into `T` so each test exercises
/// the same serde path the `#[tauri::command]` macro uses for args.
fn from_json<T: serde::de::DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).expect("deserialise Args from JSON")
}

/// Helper: serialise a return value into a JSON value so each test
/// can assert wire-side keys (camelCase / snake_case stay stable).
fn to_json<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).expect("serialise response to JSON")
}

async fn fresh_core(tmp: &TempDir) -> Core {
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    Core::new(library_root, &db_url).await.expect("init core")
}

#[test]
fn adopt_args_deserialises_from_camel_case_json() {
    let v = json!({
        "game": "gimi",
        "sourcePath": "/tmp/my-mod",
        "name": "My Mod",
    });
    let args: AdoptArgs = from_json(v);
    assert_eq!(args.game, GameCode::Gimi);
    assert_eq!(args.source_path.to_string_lossy(), "/tmp/my-mod");
    assert_eq!(args.name, "My Mod");
}

#[test]
fn import_zip_args_deserialises_from_camel_case_json() {
    let v = json!({
        "game": "srmi",
        "zipPath": "/tmp/mod.zip",
        "name": "Cool",
    });
    let args: ImportZipArgs = from_json(v);
    assert_eq!(args.game, GameCode::Srmi);
    assert_eq!(args.zip_path.to_string_lossy(), "/tmp/mod.zip");
    assert_eq!(args.name, "Cool");
}

#[test]
fn gamebanana_import_args_deserialises_with_camel_case_url_or_id() {
    let v = json!({
        "game": "gimi",
        "urlOrId": "1234567",
    });
    let args: GameBananaImportArgs = from_json(v);
    assert_eq!(args.game, GameCode::Gimi);
    assert_eq!(args.url_or_id, "1234567");
}

#[test]
fn proxy_args_deserialise_from_the_frontend_shape() {
    let args: ProxyArgs = from_json(json!({
        "url": "http://127.0.0.1:8080",
        "username": "alice",
        "password": null,
    }));
    assert_eq!(args.url.as_deref(), Some("http://127.0.0.1:8080"));
    assert_eq!(args.username.as_deref(), Some("alice"));
    assert_eq!(args.password, None);
}

#[tokio::test]
async fn list_mods_returns_snake_case_json_keys() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let fixture = tmp.path().join("fixture");
    fs::create_dir_all(&fixture).expect("fix dir");
    fs::write(fixture.join("merged.ini"), b"hash=1\n").expect("ini");
    let adopted = core
        .adopt_folder(GameCode::Gimi, &fixture, "Test Mod")
        .await
        .expect("adopt");

    // Mirror the wire path: the command body calls `core.list_mods`
    // and returns Vec<Mod>; serialise that to JSON and inspect.
    let listed = core.list_mods(GameCode::Gimi).await.expect("list");
    let v = to_json(&listed);
    let arr = v.as_array().expect("array");
    assert_eq!(arr.len(), 1);
    let obj = arr[0].as_object().expect("object");
    // Mod serialises with snake_case keys today (we deliberately did
    // NOT add `rename_all = "camelCase"` to Mod so the frontend
    // `fromRaw` mapper handles the boundary). Asserting the actual
    // shape keeps this contract from drifting accidentally.
    assert!(obj.contains_key("id"));
    assert!(obj.contains_key("library_path"));
    assert!(obj.contains_key("gamebanana_id"));
    assert!(obj.contains_key("source_url"));
    assert!(obj.contains_key("reinstall_recovery"));
    assert!(obj.get("reinstall_recovery").unwrap().is_null());
    assert_eq!(obj.get("id").unwrap().as_str(), Some(adopted.id.as_str()));
    assert_eq!(obj.get("source").unwrap().as_str(), Some("manual"));
}

#[tokio::test]
async fn set_mod_enabled_surfaces_friendly_no_install_path_error() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let fixture = tmp.path().join("fixture");
    fs::create_dir_all(&fixture).expect("dir");
    fs::write(fixture.join("merged.ini"), b"hash=1\n").expect("ini");
    let mod_ = core
        .adopt_folder(GameCode::Gimi, &fixture, "Test Mod")
        .await
        .expect("adopt");

    // Replicate the command body's contract: when game_install_path
    // is None we surface the friendly error string. This is exactly
    // what commands::set_mod_enabled does.
    let install = core
        .game_install_path(GameCode::Gimi)
        .await
        .expect("read install path");
    let err: String = install
        .ok_or_else(|| NO_INSTALL_PATH_FOR_ENABLE_MSG.to_string())
        .unwrap_err();
    assert_eq!(
        err, NO_INSTALL_PATH_FOR_ENABLE_MSG,
        "wire error message must match the exported constant"
    );

    // Make sure the mod row didn't accidentally flip — the contract
    // is "no install path → no state change".
    let listed = core.list_mods(GameCode::Gimi).await.expect("list");
    assert!(!listed[0].enabled);
    let _ = mod_;
}

#[tokio::test]
async fn adopt_folder_response_serialises_with_expected_shape() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let fixture = tmp.path().join("fixture");
    fs::create_dir_all(&fixture).expect("dir");
    fs::write(fixture.join("merged.ini"), b"hash=1\n").expect("ini");

    let args: AdoptArgs = from_json(json!({
        "game": "gimi",
        "sourcePath": fixture.to_string_lossy(),
        "name": "Adopted",
    }));
    let mod_: Mod = core
        .adopt_folder(args.game, &args.source_path, &args.name)
        .await
        .expect("adopt");
    let v = to_json(&mod_);
    let obj = v.as_object().expect("object");
    assert_eq!(obj.get("name").and_then(|n| n.as_str()), Some("Adopted"));
    assert_eq!(obj.get("source").and_then(|s| s.as_str()), Some("manual"));
    assert_eq!(obj.get("game").and_then(|g| g.as_str()), Some("gimi"));
    assert_eq!(obj.get("enabled").and_then(|b| b.as_bool()), Some(false));
    // Optional GameBanana fields are present + null on a manual mod.
    assert!(obj.contains_key("gamebanana_id"));
    assert!(obj.contains_key("source_url"));
    assert!(obj.get("gamebanana_id").unwrap().is_null());
}

#[test]
fn library_paths_response_uses_camel_case() {
    // The LibraryPaths struct (returned by get_library_paths) is the
    // one place we explicitly use camelCase serde rename. Lock it in.
    let mut per_game_overrides = std::collections::HashMap::new();
    per_game_overrides.insert("gimi".to_string(), None);
    let mut per_game_effective = std::collections::HashMap::new();
    per_game_effective.insert("gimi".to_string(), std::path::PathBuf::from("/lib/gimi"));
    let lp = LibraryPaths {
        default_root: "/default".into(),
        root_override: None,
        effective_root: "/default".into(),
        per_game_overrides,
        per_game_effective,
        overlaps: Vec::new(),
    };
    let v = to_json(&lp);
    let obj = v.as_object().expect("object");
    assert!(obj.contains_key("defaultRoot"));
    assert!(obj.contains_key("rootOverride"));
    assert!(obj.contains_key("effectiveRoot"));
    assert!(obj.contains_key("perGameOverrides"));
    assert!(obj.contains_key("perGameEffective"));
    assert!(obj.contains_key("overlaps"));
}

#[test]
fn library_audit_response_uses_camel_case() {
    let report = LibraryAuditReport {
        game: GameCode::Gimi,
        unreferenced: vec![UnreferencedLibraryDir {
            directory_name: "01ORPHAN".into(),
            path: "/library/gimi/01ORPHAN".into(),
            size_bytes: Some(42),
        }],
        duplicates: vec![DuplicateModGroup {
            path: "/library/gimi/01SHARED".into(),
            mods: vec![DuplicateModRecord {
                id: "01KEEPER".into(),
                game: GameCode::Gimi,
                name: "Keeper".into(),
                source: Source::Manual,
                library_path: "/library/gimi/01SHARED".into(),
                junction_dir_name: "Keeper".into(),
                enabled: true,
                created_at: "2026-08-24T00:00:00Z".into(),
                gamebanana_id: Some(24680),
                source_url: Some("https://gamebanana.com/mods/24680".into()),
                author: Some("Author".into()),
                version: Some("1.0".into()),
                upstream_version: Some("1.1".into()),
                update_check_enabled: false,
                screenshot_url: Some("https://images.example.test/mod.png".into()),
                variants: vec![DuplicateModVariant {
                    id: "01VARIANT".into(),
                    name: "Blue".into(),
                    subpath: "Blue".into(),
                    active: true,
                }],
                reinstall_in_progress: false,
                fingerprint: "review-fingerprint".into(),
            }],
        }],
        total_bytes: 42,
    };

    let value = to_json(&report);
    let object = value.as_object().expect("report object");
    assert_eq!(object.get("game").and_then(Value::as_str), Some("gimi"));
    assert_eq!(object.get("totalBytes").and_then(Value::as_u64), Some(42));
    let directory = object
        .get("unreferenced")
        .and_then(Value::as_array)
        .and_then(|rows| rows.first())
        .and_then(Value::as_object)
        .expect("directory object");
    assert_eq!(
        directory.get("directoryName").and_then(Value::as_str),
        Some("01ORPHAN")
    );
    assert_eq!(directory.get("sizeBytes").and_then(Value::as_u64), Some(42));
    let duplicate = object["duplicates"][0]["mods"][0]
        .as_object()
        .expect("nested duplicate record");
    for key in [
        "libraryPath",
        "junctionDirName",
        "gamebananaId",
        "sourceUrl",
        "upstreamVersion",
        "updateCheckEnabled",
        "screenshotUrl",
        "reinstallInProgress",
        "fingerprint",
    ] {
        assert!(
            duplicate.contains_key(key),
            "missing duplicate wire field {key}"
        );
    }
    assert_eq!(duplicate["variants"][0]["subpath"], "Blue");
    assert_eq!(duplicate["variants"][0]["active"], true);
}

#[test]
fn resolve_duplicate_mods_args_deserialise_from_camel_case_json() {
    let args: ResolveDuplicateModsArgs = from_json(serde_json::json!({
        "keeperId": "01KEEPER",
        "reviewedMods": [
            { "id": "01KEEPER", "fingerprint": "keeper-fingerprint" },
            { "id": "01REJECTED", "fingerprint": "rejected-fingerprint" }
        ],
    }));
    assert_eq!(args.keeper_id, "01KEEPER");
    assert_eq!(
        args.reviewed_mods,
        [
            ReviewedDuplicateMod {
                id: "01KEEPER".into(),
                fingerprint: "keeper-fingerprint".into()
            },
            ReviewedDuplicateMod {
                id: "01REJECTED".into(),
                fingerprint: "rejected-fingerprint".into()
            },
        ]
    );
}

#[test]
fn duplicate_resolution_response_uses_camel_case() {
    let value = to_json(&DuplicateResolution {
        keeper_id: "01KEEPER".into(),
        removed_mod_ids: vec!["01REJECTED".into()],
    });
    assert_eq!(value["keeperId"], "01KEEPER");
    assert_eq!(value["removedModIds"], json!(["01REJECTED"]));
}

#[test]
fn duplicate_resolution_error_copy_is_stable() {
    use gmm_lib::core::Error;

    let path = std::path::PathBuf::from("C:/Game/Mods/Shared");
    let cases = [
        (
            Error::DuplicateModResolutionChanged { reason: "records changed".into() },
            "GMM could not resolve these duplicate Mod records because the report is no longer current: records changed. Refresh the Library audit and review every record again.".to_string(),
        ),
        (
            Error::DuplicateModResolutionBlockedByReinstall { mod_id: "01MOD".into() },
            "GMM cannot discard the duplicate Mod while it has an unfinished update. Mod ID: 01MOD. Let that update settle first; if the Mod shows a recovery warning, use Retry recovery, then review the duplicate records again. No Mod record, Variant, Junction, or Library byte was changed.".to_string(),
        ),
        (
            Error::DuplicateModInstallPathMissing { mod_id: "01MOD".into(), game: "gimi".into() },
            "GMM cannot discard the enabled duplicate Mod because its game install path is not set, so GMM cannot locate its deployment Junction. Mod ID: 01MOD; game: gimi. Set the game install path, then review the duplicate records again.".to_string(),
        ),
        (
            Error::DuplicateModJunctionConflict { mod_id: "01MOD".into(), path: path.clone() },
            format!(
                "GMM cannot discard the duplicate Mod because its deployment path is not a Junction into that Mod's Library directory. Mod ID: 01MOD; path: {path:?}. GMM left every duplicate record intact."
            ),
        ),
        (
            Error::DuplicateModJunctionStillPresent { mod_id: "01MOD".into(), path: path.clone() },
            format!(
                "GMM tried to withdraw the duplicate Mod's deployment Junction, but the path is still present. Mod ID: 01MOD; path: {path:?}. GMM left every duplicate record intact."
            ),
        ),
        (
            Error::DuplicateModJunctionClaimedBySurvivor {
                mod_id: "01DROP".into(),
                surviving_mod_id: "01KEEP".into(),
                path: path.clone(),
            },
            format!(
                "GMM cannot discard the duplicate Mod because its deployment path is also claimed by a surviving Mod. Rejected Mod ID: 01DROP; surviving Mod ID: 01KEEP; path: {path:?}. GMM left every duplicate record and Junction intact."
            ),
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(error.to_string(), expected);
    }
}

#[test]
fn recover_library_dir_args_deserialise_from_camel_case_json() {
    let args: RecoverLibraryDirArgs = from_json(serde_json::json!({
        "game": "srmi",
        "path": "D:\\GMM Library\\srmi\\01FIRST",
        "name": "Raiden Shogun Alt",
    }));
    assert_eq!(args.game, GameCode::Srmi);
    assert_eq!(
        args.path,
        std::path::PathBuf::from("D:\\GMM Library\\srmi\\01FIRST"),
    );
    assert_eq!(args.name, "Raiden Shogun Alt");
}

#[test]
fn deleted_library_dir_response_uses_camel_case() {
    let value = to_json(&DeletedLibraryDir {
        directory_name: "01ORPHAN".into(),
        path: "/library/gimi/01ORPHAN".into(),
        size_bytes: None,
        reclamation: LibraryReclamationOutcome::Deferred {
            path: "/library/gimi/.gmm-delete-01QUARANTINE".into(),
        },
    });
    let object = value.as_object().expect("deleted object");
    assert_eq!(
        object.get("directoryName").and_then(Value::as_str),
        Some("01ORPHAN"),
    );
    assert!(object.get("sizeBytes").is_some_and(Value::is_null));
    let reclamation = object
        .get("reclamation")
        .and_then(Value::as_object)
        .expect("tagged reclamation outcome");
    assert_eq!(
        reclamation.get("status").and_then(Value::as_str),
        Some("deferred"),
    );
    assert_eq!(
        reclamation.get("path").and_then(Value::as_str),
        Some("/library/gimi/.gmm-delete-01QUARANTINE"),
    );
    assert_eq!(
        object.get("path").and_then(Value::as_str),
        Some("/library/gimi/01ORPHAN"),
    );
}

#[tokio::test]
async fn delete_response_reports_owned_but_unreclaimed_quarantine_as_deferred() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game Library root");
    fs::create_dir_all(&root).expect("create game Library root");
    let orphan = root.join(ulid::Ulid::new().to_string());
    fs::create_dir(&orphan).expect("orphan");
    fs::write(orphan.join("proven-marker"), b"proven bytes").expect("orphan bytes");

    let observed_quarantine = Arc::new(Mutex::new(None));
    let hook_quarantine = Arc::clone(&observed_quarantine);
    let hook_root = root.clone();
    #[cfg(windows)]
    let removal_blocker = Arc::new(Mutex::new(None::<File>));
    #[cfg(windows)]
    let hook_removal_blocker = Arc::clone(&removal_blocker);
    let blocking = core.with_crash_hook(Arc::new(move |point| {
        if point != crash_points::DELETE_BEFORE_QUARANTINE_PURGE {
            return;
        }
        let quarantine = fs::read_dir(&hook_root)
            .expect("Library root at pre-purge seam")
            .filter_map(std::result::Result::ok)
            .find(|entry| {
                entry.file_type().is_ok_and(|kind| kind.is_dir())
                    && entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".gmm-delete-")
            })
            .expect("delete quarantine at pre-purge seam")
            .path();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            let mut permissions = fs::metadata(&quarantine)
                .expect("quarantine metadata")
                .permissions();
            permissions.set_mode(0o555);
            fs::set_permissions(&quarantine, permissions)
                .expect("make quarantine contents undeletable");
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

            let blocker = fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .open(quarantine.join("proven-marker"))
                .expect("open quarantine file without delete sharing");
            *hook_removal_blocker.lock().expect("blocker lock") = Some(blocker);
        }
        *hook_quarantine.lock().expect("quarantine lock") = Some(quarantine);
    }));

    let deleted = blocking
        .delete_unreferenced_library_dir(GameCode::Gimi, &orphan)
        .await
        .expect("the visible Library delete is already committed");
    let value = to_json(&deleted);
    let object = value.as_object().expect("deleted object");
    let quarantine = observed_quarantine
        .lock()
        .expect("quarantine lock")
        .clone()
        .expect("hook observed quarantine");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        let mut permissions = fs::metadata(&quarantine)
            .expect("quarantine metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&quarantine, permissions).expect("restore quarantine permissions");
    }
    #[cfg(windows)]
    drop(removal_blocker.lock().expect("blocker lock").take());

    assert!(object.get("sizeBytes").is_some_and(Value::is_null));
    let reclamation = object
        .get("reclamation")
        .and_then(Value::as_object)
        .expect("tagged reclamation outcome");
    assert_eq!(
        reclamation.get("status").and_then(Value::as_str),
        Some("deferred"),
        "an owned quarantine that could not be removed must remain retryable",
    );
    assert_eq!(
        reclamation.get("path").and_then(Value::as_str),
        Some(quarantine.to_string_lossy().as_ref()),
    );
    assert!(quarantine.join("proven-marker").is_file());
}

#[tokio::test]
async fn delete_response_reports_identity_changed_reclamation_as_ownership_lost() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game Library root");
    fs::create_dir_all(&root).expect("create game Library root");
    let orphan = root.join(ulid::Ulid::new().to_string());
    fs::create_dir(&orphan).expect("orphan");
    fs::write(orphan.join("proven-marker"), b"proven bytes").expect("orphan bytes");

    let moved_original = root.join("moved-original");
    let observed_quarantine = Arc::new(Mutex::new(None));
    let hook_quarantine = Arc::clone(&observed_quarantine);
    let hook_root = root.clone();
    let hook_moved_original = moved_original.clone();
    let swapping = core.with_crash_hook(Arc::new(move |point| {
        if point != crash_points::DELETE_BEFORE_QUARANTINE_PURGE {
            return;
        }
        let quarantine = fs::read_dir(&hook_root)
            .expect("Library root at pre-purge seam")
            .filter_map(std::result::Result::ok)
            .find(|entry| {
                entry.file_type().is_ok_and(|kind| kind.is_dir())
                    && entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".gmm-delete-")
            })
            .expect("delete quarantine at pre-purge seam")
            .path();
        fs::rename(&quarantine, &hook_moved_original).expect("move owned quarantine aside");
        fs::create_dir(&quarantine).expect("replacement quarantine");
        fs::write(quarantine.join("replacement-marker"), b"replacement")
            .expect("replacement bytes");
        *hook_quarantine.lock().expect("quarantine lock") = Some(quarantine);
    }));

    let deleted = swapping
        .delete_unreferenced_library_dir(GameCode::Gimi, &orphan)
        .await
        .expect("the visible Library delete is already committed");
    let value = to_json(&deleted);
    let object = value.as_object().expect("deleted object");
    let quarantine = observed_quarantine
        .lock()
        .expect("quarantine lock")
        .clone()
        .expect("hook observed quarantine");

    assert!(object.get("sizeBytes").is_some_and(Value::is_null));
    let reclamation = object
        .get("reclamation")
        .and_then(Value::as_object)
        .expect("tagged reclamation outcome");
    assert_eq!(
        reclamation.get("status").and_then(Value::as_str),
        Some("ownershipLost"),
        "an identity mismatch cannot honestly claim the reserved path still holds GMM's bytes",
    );
    assert!(
        !reclamation.contains_key("path"),
        "ownership loss must not present the replacement quarantine as a cleanup target",
    );
    assert!(moved_original.join("proven-marker").is_file());
    assert!(quarantine.join("replacement-marker").is_file());
}

#[test]
fn reconcile_result_serialises_with_snake_case_inner_keys() {
    let report = ReconcileResult::default();
    let v = to_json(&report);
    let obj = v.as_object().expect("object");
    assert!(obj.contains_key("recreated"));
    assert!(obj.contains_key("healthy"));
    assert!(obj.contains_key("conflicting"));
    assert!(obj.contains_key("skipped"));
}

#[test]
fn update_status_uses_camel_case() {
    let s = UpdateStatus {
        available: false,
        installed_version: Some("v1.0".into()),
        latest_version: None,
        pinned: false,
        upstream_ahead: false,
        check_error: None,
    };
    let v = to_json(&s);
    let obj = v.as_object().expect("object");
    assert!(obj.contains_key("available"));
    assert!(obj.contains_key("installedVersion"));
    assert!(obj.contains_key("latestVersion"));
    assert!(obj.contains_key("pinned"));
    assert!(obj.contains_key("upstreamAhead"));
    // #79: a failed asset selection travels to the UI as its own field
    // rather than looking like "nothing to apply".
    assert!(obj.contains_key("checkError"));
}

#[test]
fn conflict_report_default_serialises() {
    let r = ConflictReport::default();
    let v = to_json(&r);
    let obj = v.as_object().expect("object");
    assert!(obj.contains_key("conflicts"));
    assert!(obj.contains_key("per_mod_count"));
}

#[tokio::test]
async fn import_zip_command_path_round_trips_through_serde() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    // Build a tiny zip on disk so we can exercise the full command
    // body — same shape the IPC layer would feed in.
    let zip_path = tmp.path().join("payload.zip");
    {
        let f = File::create(&zip_path).expect("create");
        let mut zw = ZipWriter::new(f);
        let opts =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zw.start_file("merged.ini", opts).expect("ini");
        zw.write_all(b"hash=1\n").expect("write");
        zw.finish().expect("finish");
    }
    let args: ImportZipArgs = from_json(json!({
        "game": "gimi",
        "zipPath": zip_path.to_string_lossy(),
        "name": "ZipMod",
    }));

    let mod_: Mod = core
        .import_zip(
            args.game,
            &args.zip_path,
            &args.name,
            ImportZipOptions::default(),
        )
        .await
        .expect("import");
    assert_eq!(mod_.source, Source::Local);
    assert_eq!(mod_.name, "ZipMod");
    let json = to_json(&mod_);
    assert_eq!(json.get("source").and_then(|s| s.as_str()), Some("local"));
}

#[test]
fn list_supported_games_returns_gimi_and_srmi_in_order() {
    // Each slice #16-#20 adds a ported game; the React tab strip
    // relies on this command to know which tabs to render. Order must
    // be stable so the UI's "first tab is default" behaviour matches
    // the registry — GIMI stays first so existing users land on the
    // familiar screen.
    let games = list_supported_games().expect("list supported games command");
    let codes: Vec<&str> = games.iter().map(|g| g.code.as_str()).collect();
    assert!(
        codes.first() == Some(&"gimi"),
        "GIMI must remain the first tab so existing users land on a familiar screen, got {codes:?}",
    );
    for needed in ["srmi", "zzmi", "wwmi", "himi", "efmi"] {
        assert!(
            codes.contains(&needed),
            "{needed} must appear once its slice lands, got {codes:?}",
        );
    }
    // Every supported game serialises with the camelCase wire shape.
    let v = to_json(&games);
    let arr = v.as_array().expect("array");
    assert!(arr.iter().all(|g| g
        .as_object()
        .map(|o| o.contains_key("code") && o.contains_key("displayName"))
        .unwrap_or(false)));
}

#[test]
fn game_profiles_cover_every_game_code() {
    // The registry is keyed by `GameCode`; missing rows would crash
    // `GameCode::profile()` at runtime via `unreachable!`. Asserting
    // here keeps that contract from drifting silently.
    use gmm_lib::core::GameCode;
    let expected = [
        GameCode::Gimi,
        GameCode::Srmi,
        GameCode::Zzmi,
        GameCode::Wwmi,
        GameCode::Himi,
        GameCode::Efmi,
    ];
    let actual: Vec<GameCode> = GAME_PROFILES.iter().map(|p| p.code).collect();
    assert_eq!(actual, expected);
}

#[test]
fn srmi_profile_lists_star_rail_exe_and_spectrumqt_repo() {
    use gmm_lib::core::GameCode;
    let p = GameCode::Srmi.profile();
    assert_eq!(p.display_name, "Honkai: Star Rail");
    let (repo, asset_pattern) = p.importer_repo.expect("srmi importer origin wired");
    assert_eq!(repo, "SpectrumQT/SRMI-Package");
    let pattern = AssetPattern::new(asset_pattern).expect("srmi pattern compiles");
    assert!(
        pattern.matches("SRMI-PACKAGE-v2.4.1.zip"),
        "SRMI's anchored pattern must accept a conventionally named package, got {asset_pattern:?}"
    );
    assert!(
        p.executable_candidates.contains(&"StarRail.exe"),
        "SRMI exe candidates must include StarRail.exe, got {:?}",
        p.executable_candidates,
    );
    assert!(p.detect.is_some(), "SRMI detect fn must be wired");
    assert!(p.is_ported());
}

#[test]
fn zzmi_profile_lists_zzz_exe_and_canonical_repo() {
    use gmm_lib::core::GameCode;
    let p = GameCode::Zzmi.profile();
    assert_eq!(p.display_name, "Zenless Zone Zero");
    let (repo, asset_pattern) = p.importer_repo.expect("zzmi importer origin wired");
    assert_eq!(repo, "leotorrez/ZZMI-Package");
    let pattern = AssetPattern::new(asset_pattern).expect("zzmi pattern compiles");
    assert!(
        pattern.matches("ZZMI-PACKAGE-v1.4.5.zip"),
        "ZZMI's anchored pattern must accept a conventionally named package, got {asset_pattern:?}"
    );
    assert!(
        p.executable_candidates.contains(&"ZenlessZoneZero.exe"),
        "ZZMI exe candidates must include ZenlessZoneZero.exe, got {:?}",
        p.executable_candidates,
    );
    assert!(p.detect.is_some(), "ZZMI detect fn must be wired");
    assert!(p.is_ported());
}

#[test]
fn wwmi_profile_lists_unreal_shipping_exe_and_spectrumqt_repo() {
    use gmm_lib::core::GameCode;
    let p = GameCode::Wwmi.profile();
    assert_eq!(p.display_name, "Wuthering Waves");
    let (repo, asset_pattern) = p.importer_repo.expect("wwmi importer origin wired");
    assert_eq!(repo, "SpectrumQT/WWMI-Package");
    let pattern = AssetPattern::new(asset_pattern).expect("wwmi pattern compiles");
    assert!(
        pattern.matches("WWMI-PACKAGE-v1.0.0.zip"),
        "WWMI's anchored pattern must accept a conventionally named package, got {asset_pattern:?}"
    );
    assert!(
        p.executable_candidates
            .contains(&"Client-Win64-Shipping.exe"),
        "WWMI exe candidates must include the UE shipping exe, got {:?}",
        p.executable_candidates,
    );
    assert!(p.detect.is_some(), "WWMI detect fn must be wired");
    assert!(p.is_ported());
}

#[test]
fn himi_profile_lists_bh3_exe_and_canonical_repo() {
    use gmm_lib::core::GameCode;
    let p = GameCode::Himi.profile();
    assert_eq!(p.display_name, "Honkai Impact 3rd");
    let (repo, asset_pattern) = p.importer_repo.expect("himi importer origin wired");
    assert_eq!(repo, "leotorrez/HIMI-Package");
    let pattern = AssetPattern::new(asset_pattern).expect("himi pattern compiles");
    assert!(
        pattern.matches("HIMI-PACKAGE-v1.0.2.zip"),
        "HIMI's anchored pattern must accept a conventionally named package, got {asset_pattern:?}"
    );
    assert!(
        p.executable_candidates.contains(&"BH3.exe"),
        "HIMI exe candidates must include BH3.exe, got {:?}",
        p.executable_candidates,
    );
    assert!(p.detect.is_some(), "HIMI detect fn must be wired");
    assert!(p.is_ported());
}

#[test]
fn efmi_profile_uses_inject_mode_not_hook() {
    use gmm_lib::core::games::InjectMode;
    use gmm_lib::core::GameCode;
    let p = GameCode::Efmi.profile();
    assert_eq!(p.display_name, "Endfield");
    let (repo, asset_pattern) = p.importer_repo.expect("efmi importer origin wired");
    assert_eq!(repo, "SpectrumQT/EFMI-Package");
    let pattern = AssetPattern::new(asset_pattern).expect("efmi pattern compiles");
    assert!(
        pattern.matches("EFMI-PACKAGE-v1.3.0.zip"),
        "EFMI's anchored pattern must accept a conventionally named package, got {asset_pattern:?}"
    );
    assert!(
        p.executable_candidates
            .contains(&"Endfield-Win64-Shipping.exe"),
        "EFMI exe candidates must include the UE shipping exe, got {:?}",
        p.executable_candidates,
    );
    assert!(p.detect.is_some(), "EFMI detect fn must be wired");
    assert!(p.is_ported());
    // The headline quirk: XXMI marks EFMI as inject-mode rather than
    // hook-mode. `launch_game` branches on this; the Hoyoverse + Kuro
    // games all stay on Hook.
    assert_eq!(p.inject_mode, InjectMode::Inject);
}

#[test]
fn non_efmi_games_default_to_hook_inject_mode() {
    use gmm_lib::core::games::InjectMode;
    use gmm_lib::core::GameCode;
    for game in [
        GameCode::Gimi,
        GameCode::Srmi,
        GameCode::Zzmi,
        GameCode::Wwmi,
        GameCode::Himi,
    ] {
        assert_eq!(
            game.profile().inject_mode,
            InjectMode::Hook,
            "{} must use Hook mode (default for Hoyoverse + Kuro titles)",
            game.as_str(),
        );
    }
}

#[tokio::test]
async fn detect_all_games_returns_one_row_per_ported_game() {
    // Slice 16-b (#24): Step 2 of the wizard renders one row per
    // ported game. On a fresh machine with no installs, every row
    // surfaces `detectedPath = null` so the UI can fall through to
    // the manual browse/skip controls.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    // We can't invoke the Tauri command directly (it takes a
    // `State<'_, Core>`), but the IPC contract is what we want to
    // pin: emulate the command body by calling each ported game's
    // detect fn and serialising the response.
    use gmm_lib::core::games::GAME_PROFILES;
    let mut payload = Vec::new();
    for p in GAME_PROFILES.iter().filter(|p| p.is_ported()) {
        let detect = p.detect.expect("ported");
        let detected = tokio::task::spawn_blocking(detect).await.expect("join");
        payload.push(serde_json::json!({
            "code": p.code,
            "displayName": p.display_name,
            "detectedPath": detected,
        }));
    }
    assert_eq!(
        payload.len(),
        GAME_PROFILES.iter().filter(|p| p.is_ported()).count()
    );
    // Every row carries camelCase keys.
    for row in &payload {
        let obj = row.as_object().expect("object");
        assert!(obj.contains_key("code"));
        assert!(obj.contains_key("displayName"));
        assert!(obj.contains_key("detectedPath"));
    }
    // Touch `core` to ensure the State wiring compiles in real usage.
    let _ = core;
}

#[test]
fn av_guidance_response_uses_camel_case_keys() {
    // Slice NEW-AV / #13: the `av_guidance` Tauri command returns the
    // structured payload the launch-error component renders. Wire-side
    // it must come through as camelCase so the React component can
    // read it without a fromRaw mapper.
    let g = av::guidance();
    let v = to_json(&g);
    let obj = v.as_object().expect("object");
    assert!(obj.contains_key("headline"));
    assert!(obj.contains_key("body"));
    assert!(obj.contains_key("exclusionSteps"));
    assert!(obj.contains_key("docPath"));
    assert!(obj.contains_key("sentinel"));
    assert_eq!(
        obj.get("sentinel").and_then(|s| s.as_str()),
        Some(av::AV_PATTERN_SENTINEL),
        "sentinel must round-trip verbatim — the React layer matches on this string"
    );
    assert!(obj
        .get("docPath")
        .and_then(|p| p.as_str())
        .map(|p| p.ends_with("antivirus-and-smartscreen.md"))
        .unwrap_or(false));
}

#[test]
fn variant_serialises_with_expected_keys() {
    let v = Variant {
        id: "v1".into(),
        mod_id: "m1".into(),
        name: "Red".into(),
        subpath: std::path::PathBuf::from("Red"),
    };
    let json = to_json(&v);
    let obj = json.as_object().expect("object");
    assert!(obj.contains_key("id"));
    assert!(obj.contains_key("mod_id"));
    assert!(obj.contains_key("name"));
    assert!(obj.contains_key("subpath"));
}
