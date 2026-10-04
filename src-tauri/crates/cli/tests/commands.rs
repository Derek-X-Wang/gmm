use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use gmm_cli::{run, Outcome};
use gmm_lib::core::{Core, GameCode};
use serde_json::Value;
use sqlx::SqlitePool;
use tempfile::TempDir;

struct Fixture {
    tmp: TempDir,
    data: PathBuf,
    url: String,
}

impl Fixture {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let data = tmp.path().join("data");
        let url = format!("sqlite://{}", data.join("gmm.db").display());
        Self { tmp, data, url }
    }
    async fn core(&self) -> Core {
        std::fs::create_dir_all(&self.data).unwrap();
        Core::new(self.data.join("library"), &self.url)
            .await
            .unwrap()
    }
    async fn invoke(&self, args: &[&str]) -> Outcome {
        let mut argv = vec![
            "--data-dir".into(),
            self.data.to_string_lossy().into_owned(),
        ];
        argv.extend(args.iter().map(|arg| (*arg).to_string()));
        run(argv).await
    }
    fn source(&self) -> PathBuf {
        let source = self.tmp.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(
            source.join("mod.ini"),
            "[TextureOverrideExample]\nhash=12345678\n",
        )
        .unwrap();
        source
    }
    async fn deployed_mod(&self) -> gmm_lib::core::Mod {
        let core = self.core().await;
        let game = self.tmp.path().join("Game");
        std::fs::create_dir_all(&game).unwrap();
        core.set_game_install_path(GameCode::Gimi, &game)
            .await
            .unwrap();
        let item = core
            .adopt_folder(GameCode::Gimi, &self.source(), "Example")
            .await
            .unwrap();
        core.close().await;
        item
    }
}

fn result(outcome: Outcome) -> Value {
    assert_eq!(
        outcome.exit_code, 0,
        "operation should succeed: {:?}",
        outcome.error
    );
    assert!(outcome.ok);
    let encoded = serde_json::to_string(&outcome).unwrap();
    assert!(!encoded.contains('\n'));
    outcome.result.unwrap()
}

// Snapshot actual bytes, entry types and link targets, including every database
// file. Exclude only the required, empty instance.lock bookkeeping file.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, (String, Vec<u8>)> {
    fn walk(base: &Path, dir: &Path, entries: &mut BTreeMap<PathBuf, (String, Vec<u8>)>) {
        if !std::fs::exists(dir).unwrap() {
            return;
        }
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().unwrap() == "instance.lock" {
                continue;
            }
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            let relative = path.strip_prefix(base).unwrap().to_path_buf();
            if metadata.file_type().is_symlink() {
                entries.insert(
                    relative,
                    (
                        "link".into(),
                        std::fs::read_link(&path)
                            .unwrap()
                            .to_string_lossy()
                            .as_bytes()
                            .to_vec(),
                    ),
                );
            } else if metadata.is_dir() {
                entries.insert(relative, ("directory".into(), Vec::new()));
                walk(base, &path, entries);
            } else {
                entries.insert(relative, ("file".into(), std::fs::read(&path).unwrap()));
            }
        }
    }
    let mut entries = BTreeMap::new();
    walk(root, root, &mut entries);
    entries
}

#[tokio::test]
async fn usage_errors_are_distinct_from_operation_failures() {
    let env = Fixture::new();
    for args in [
        &[][..],
        &["unknown"],
        &["enable"],
        &["mods", "--game", "wrong"],
        &["status", "--unused", "x"],
        &["adopt", "--game", "gimi"],
        &["status", "--dry-run"],
        &["status", "--data-dir", "x", "--data-dir", "y"],
    ] {
        let outcome = env.invoke(args).await;
        assert_eq!(outcome.exit_code, 1, "usage error for {args:?}");
        assert_eq!(
            serde_json::to_value(outcome.error.unwrap()).unwrap()["kind"],
            "other"
        );
    }
    assert!(
        !std::fs::exists(&env.data).unwrap(),
        "usage errors must not open state"
    );
    let outcome = env.invoke(&["variants", "--mod-id", "missing"]).await;
    assert_eq!(outcome.exit_code, 2);
    assert!(outcome.error.unwrap().message.contains("does not exist"));
}

#[tokio::test]
async fn fresh_discovery_reports_supported_games_and_safe_status() {
    let env = Fixture::new();
    let games = result(env.invoke(&["games"]).await);
    assert_eq!(games["games"].as_array().unwrap().len(), 6);
    assert_eq!(games["games"][0]["code"], "gimi");
    assert!(games["games"][0].get("detectedPath").is_some());
    assert_eq!(
        result(env.invoke(&["mods"]).await)["mods"],
        serde_json::json!([])
    );
    assert_eq!(result(env.invoke(&["status"]).await)["safeToProceed"], true);
    assert_eq!(
        result(env.invoke(&["importer", "--game", "gimi"]).await)["installedVersion"],
        Value::Null
    );
    assert!(!std::fs::exists(env.data.join("gmm.db")).unwrap());
    assert!(!std::fs::exists(env.data.join("library")).unwrap());
}

#[tokio::test]
async fn adopt_listing_importer_and_variants_return_real_state() {
    let env = Fixture::new();
    let source = env.source();
    // A source with two Variant folders, and no root INI.
    std::fs::remove_file(source.join("mod.ini")).unwrap();
    for name in ["Blue", "Red"] {
        std::fs::create_dir(source.join(name)).unwrap();
        std::fs::write(
            source.join(name).join("mod.ini"),
            "[TextureOverride]\nhash=123\n",
        )
        .unwrap();
    }
    let adopted = result(
        env.invoke(&[
            "adopt",
            "--game",
            "gimi",
            "--from",
            source.to_str().unwrap(),
            "--name",
            "Variants",
        ])
        .await,
    );
    let id = adopted["mod"]["id"].as_str().unwrap();
    let mods = result(env.invoke(&["mods", "--game", "gimi"]).await);
    assert_eq!(mods["mods"][0]["mod"]["enabled"], false);
    assert_eq!(mods["mods"][0]["variants"].as_array().unwrap().len(), 2);
    assert!(mods["mods"][0]["activeVariantId"].is_string());
    assert_eq!(
        result(env.invoke(&["mods", "--game", "srmi"]).await)["mods"],
        serde_json::json!([])
    );
    let variants = result(env.invoke(&["variants", "--mod-id", id]).await);
    assert_eq!(variants["variants"].as_array().unwrap().len(), 2);
    let core = env.core().await;
    let game = env.tmp.path().join("Game");
    std::fs::create_dir(&game).unwrap();
    core.set_game_install_path(GameCode::Gimi, &game)
        .await
        .unwrap();
    core.set_importer_installed(GameCode::Gimi, "1.2.3")
        .await
        .unwrap();
    core.set_importer_pinned(GameCode::Gimi, Some("1.2.3"))
        .await
        .unwrap();
    core.close().await;
    let variant_id = variants["variants"][1]["id"].as_str().unwrap();
    let before = snapshot(env.tmp.path());
    let plan = result(
        env.invoke(&[
            "set-variant",
            "--mod-id",
            id,
            "--variant-id",
            variant_id,
            "--dry-run",
        ])
        .await,
    );
    assert_eq!(before, snapshot(env.tmp.path()));
    assert_eq!(
        plan["effects"]["libraryTarget"],
        Path::new(adopted["mod"]["library_path"].as_str().unwrap())
            .join("Red")
            .to_string_lossy()
            .as_ref()
    );
    result(env.invoke(&["enable", "--mod-id", id, "--confirm"]).await);
    result(
        env.invoke(&[
            "set-variant",
            "--mod-id",
            id,
            "--variant-id",
            variant_id,
            "--confirm",
        ])
        .await,
    );
    assert_eq!(
        result(env.invoke(&["variants", "--mod-id", id]).await)["activeVariantId"],
        variant_id
    );
    let importer = result(env.invoke(&["importer", "--game", "gimi"]).await);
    assert_eq!(importer["installedVersion"], "1.2.3");
    assert_eq!(importer["pinnedVersion"], "1.2.3");
}

#[tokio::test]
async fn archive_import_enable_conflicts_and_disable_succeed() {
    use std::io::Write;
    let env = Fixture::new();
    let item = env.deployed_mod().await;
    let archive = env.tmp.path().join("mod.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
    zip.start_file("mod.ini", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"[TextureOverrideSecond]\nhash=12345678\n")
        .unwrap();
    zip.finish().unwrap();
    let imported = result(
        env.invoke(&[
            "import-zip",
            "--game",
            "gimi",
            "--archive",
            archive.to_str().unwrap(),
            "--name",
            "Second",
        ])
        .await,
    );
    let second = imported["mod"]["id"].as_str().unwrap();
    assert_eq!(imported["mod"]["source"], "local");
    for id in [item.id.as_str(), second] {
        assert_eq!(
            result(env.invoke(&["enable", "--mod-id", id, "--confirm"]).await)["mod"]["enabled"],
            true
        );
    }
    let conflicts = result(env.invoke(&["conflicts", "--game", "gimi"]).await);
    assert!(conflicts.to_string().contains("12345678"));
    assert!(conflicts.to_string().contains(&item.id));
    assert!(conflicts.to_string().contains(second));
    assert_eq!(
        result(
            env.invoke(&["disable", "--mod-id", &item.id, "--confirm"])
                .await
        )["mod"]["enabled"],
        false
    );
}

#[tokio::test]
async fn fresh_dry_run_does_not_create_database_or_library() {
    let env = Fixture::new();
    // Taking the required lock may create the data directory itself.
    std::fs::create_dir(&env.data).unwrap();
    let source = env.source();
    let before = snapshot(env.tmp.path());
    let plan = result(
        env.invoke(&[
            "adopt",
            "--game",
            "gimi",
            "--from",
            source.to_str().unwrap(),
            "--name",
            "Example",
            "--dry-run",
        ])
        .await,
    );
    assert!(
        before == snapshot(env.tmp.path()),
        "a fresh dry run must not create a database or Library"
    );
    assert_eq!(plan["dryRun"], true);
    assert_eq!(plan["effects"]["action"], "copyFolderToLibrary");
}

#[tokio::test]
async fn dry_run_enable_does_not_change_state() {
    let env = Fixture::new();
    let item = env.deployed_mod().await;
    let before = snapshot(env.tmp.path());
    let outcome = env
        .invoke(&["enable", "--mod-id", &item.id, "--dry-run", "--confirm"])
        .await;
    assert!(
        before == snapshot(env.tmp.path()),
        "dry-run enable must leave database, Library and Game directory unchanged"
    );
    let plan = result(outcome);
    assert_eq!(plan["requiresConfirmation"], true);
    assert_eq!(
        plan["effects"]["junctionPath"],
        env.tmp
            .path()
            .join("Game/Mods/Example")
            .to_string_lossy()
            .as_ref()
    );
}

#[tokio::test]
async fn game_directory_write_requires_confirmation() {
    let env = Fixture::new();
    let item = env.deployed_mod().await;
    let before = snapshot(env.tmp.path());
    let outcome = env.invoke(&["enable", "--mod-id", &item.id]).await;
    assert_eq!(
        outcome.exit_code, 2,
        "Game-directory writes must be refused without --confirm"
    );
    assert!(outcome.error.unwrap().message.contains("--confirm"));
    assert_eq!(before, snapshot(env.tmp.path()));
    for command in ["disable", "set-variant"] {
        let mut args = vec![command, "--mod-id", &item.id];
        if command == "set-variant" {
            args.extend(["--variant-id", "missing"]);
        }
        assert_eq!(env.invoke(&args).await.exit_code, 2);
    }
}

#[tokio::test]
async fn status_with_pending_witnesses_does_not_mutate_database_or_library() {
    let env = Fixture::new();
    let core = env.core().await;
    let pool = SqlitePool::connect(&env.url).await.unwrap();
    // A start time of one cannot match this test process. Both recorded
    // identities have been reused, so Core::new would retire this witness.
    sqlx::query("INSERT INTO session_launch_claims (token, game_code, owner_pid, owner_started_at, child_pid, child_started_at, started_at) VALUES ('pending', 'gimi', ?, 1, ?, 1, '2026-08-24T00:00:00Z')")
        .bind(i64::from(std::process::id())).bind(i64::from(std::process::id()))
        .execute(&pool).await.unwrap();
    // Interrupted Library bytes remain visible to the audit.
    std::fs::create_dir_all(env.data.join("library/gimi/interrupted")).unwrap();
    std::fs::write(
        env.data.join("library/gimi/interrupted/mod.ini"),
        "preserve me",
    )
    .unwrap();
    pool.close().await;
    core.close().await;
    let before = snapshot(&env.data);
    let outcome = env.invoke(&["status"]).await;
    assert!(
        before == snapshot(&env.data),
        "status must leave the database and Library unchanged with pending witnesses"
    );
    let status = result(outcome);
    assert_eq!(status["safeToProceed"], false);
    assert_eq!(status["sessionLaunches"][0]["id"], "pending");
    assert_eq!(
        status["libraryAudits"][0]["unreferenced"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn unsafe_status_refuses_writes_unless_explicitly_overridden() {
    let env = Fixture::new();
    let core = env.core().await;
    std::fs::create_dir_all(env.data.join("library/gimi/orphan")).unwrap();
    core.close().await;
    let source = env.source();
    let args = [
        "adopt",
        "--game",
        "gimi",
        "--from",
        source.to_str().unwrap(),
        "--name",
        "Example",
    ];
    let before = snapshot(&env.data);
    let refused = env.invoke(&args).await;
    assert_eq!(
        refused.exit_code, 2,
        "unsafe status must refuse a write without --allow-attention"
    );
    assert!(refused.error.unwrap().message.contains("needs attention"));
    assert_eq!(before, snapshot(&env.data));
    let mut allowed = args.to_vec();
    allowed.push("--allow-attention");
    assert_eq!(result(env.invoke(&allowed).await)["mod"]["name"], "Example");
}

#[tokio::test]
async fn all_mutations_offer_dry_runs_without_changing_state() {
    let env = Fixture::new();
    let item = env.deployed_mod().await;
    let source = env.source();
    let archive = env.tmp.path().join("archive.zip");
    std::fs::write(&archive, b"archive placeholder; preview never extracts").unwrap();
    let before = snapshot(env.tmp.path());
    for args in [
        vec!["import", "--game", "gimi", "--source", "123", "--dry-run"],
        vec![
            "import-zip",
            "--game",
            "gimi",
            "--archive",
            archive.to_str().unwrap(),
            "--name",
            "Archive",
            "--dry-run",
        ],
        vec![
            "adopt",
            "--game",
            "gimi",
            "--from",
            source.to_str().unwrap(),
            "--name",
            "Folder",
            "--dry-run",
        ],
        vec!["enable", "--mod-id", &item.id, "--dry-run"],
        vec!["disable", "--mod-id", &item.id, "--dry-run"],
        vec!["launch", "--game", "gimi", "--dry-run"],
    ] {
        assert_eq!(result(env.invoke(&args).await)["dryRun"], true);
        assert_eq!(
            before,
            snapshot(env.tmp.path()),
            "dry run for {args:?} must preserve state"
        );
    }
}

#[tokio::test]
async fn launch_failure_is_classified_and_retires_its_claim() {
    let env = Fixture::new();
    env.deployed_mod().await;
    let outcome = env.invoke(&["launch", "--game", "gimi"]).await;
    assert_eq!(outcome.exit_code, 2);
    let error = outcome.error.unwrap();
    assert_eq!(serde_json::to_value(&error).unwrap()["kind"], "other");
    assert!(error.message.contains("not found"));
    assert_eq!(
        result(env.invoke(&["status"]).await)["sessionLaunches"],
        serde_json::json!([])
    );
}

#[tokio::test]
async fn failed_status_subreport_never_becomes_safe() {
    let env = Fixture::new();
    let core = env.core().await;
    let pool = SqlitePool::connect(&env.url).await.unwrap();
    sqlx::query("DROP TABLE session_launch_claims")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    core.close().await;
    let outcome = env.invoke(&["status"]).await;
    assert_eq!(outcome.exit_code, 2);
    assert!(outcome.result.is_none());
    assert!(outcome
        .error
        .unwrap()
        .message
        .contains("session_launch_claims"));
}

#[tokio::test]
async fn inspection_refuses_stale_schema_without_migrating() {
    let env = Fixture::new();
    let core = env.core().await;
    let pool = SqlitePool::connect(&env.url).await.unwrap();
    sqlx::query(
        "DELETE FROM _sqlx_migrations WHERE version = (SELECT MAX(version) FROM _sqlx_migrations)",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
    core.close().await;
    let before = snapshot(&env.data);
    let outcome = env.invoke(&["status"]).await;
    assert_eq!(
        outcome.exit_code, 2,
        "inspection must refuse a stale schema without migrating"
    );
    assert!(outcome.error.unwrap().message.contains("schema"));
    assert_eq!(before, snapshot(&env.data));
}

#[tokio::test]
async fn attention_override_does_not_bypass_core_session_guard() {
    let env = Fixture::new();
    let core = env.core().await;
    core.start_session(&gmm_lib::core::SessionInfo {
        game: GameCode::Gimi,
        pid: std::process::id(),
        started_at: "2026-08-24T00:00:00Z".parse().unwrap(),
    })
    .await
    .unwrap();
    core.close().await;
    let before = snapshot(&env.data);
    let source = env.source();
    let outcome = env
        .invoke(&[
            "adopt",
            "--game",
            "gimi",
            "--from",
            source.to_str().unwrap(),
            "--name",
            "Example",
            "--allow-attention",
        ])
        .await;
    assert_eq!(outcome.exit_code, 2);
    assert!(outcome.error.unwrap().message.contains("running"));
    assert_eq!(before, snapshot(&env.data));
}

#[tokio::test]
async fn active_variant_failure_keeps_its_shared_classification() {
    let env = Fixture::new();
    let item = env.deployed_mod().await;
    let pool = SqlitePool::connect(&env.url).await.unwrap();
    let mut connection = pool.acquire().await.unwrap();
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("UPDATE mods SET active_variant_id = 'missing' WHERE id = ?")
        .bind(&item.id)
        .execute(&mut *connection)
        .await
        .unwrap();
    drop(connection);
    pool.close().await;
    let outcome = env
        .invoke(&["enable", "--mod-id", &item.id, "--confirm"])
        .await;
    assert_eq!(outcome.exit_code, 2);
    assert_eq!(
        serde_json::to_value(outcome.error.unwrap()).unwrap()["kind"],
        "invalidActiveVariant"
    );
}
