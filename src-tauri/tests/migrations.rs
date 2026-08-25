//! Migration corpus: open a populated database at every schema version
//! GMM has ever had, and migrate it forward (issue #55).
//!
//! Every other test in the suite starts from `Core::new` against an
//! empty file, so the migrations only ever run against nothing. A
//! migration that breaks on real rows — a NOT NULL column with no
//! default, a UNIQUE index that existing data violates, a rename that
//! drops a column — passes the whole suite and bricks the install on
//! first launch. GMM self-updates, so it would reach users on its own.
//!
//! ## The corpus
//!
//! `tests/fixtures/migrations/NNN_<name>.db` holds a SQLite file with
//! migrations `1..=NNN` applied and representative rows in every table
//! that existed at that point: a Game with an install path, two Mods
//! (one enabled, one not) with their Junction directory names, a
//! Library root override, a Variant, GameBanana provenance, update
//! state.
//!
//! The files are checked in as binaries on purpose. They carry the
//! `_sqlx_migrations` rows — including sqlx's checksum of each
//! migration's SQL — exactly as they were written at generation time,
//! so editing an already-shipped migration file makes these tests fail
//! with a checksum mismatch. That is the same error a user's install
//! would hit, which is the point.
//!
//! ## Regenerating
//!
//! Adding a migration means adding a fixture. The generator is an
//! ignored test in this file so the seed data and the assertions can't
//! drift apart:
//!
//! ```bash
//! cargo xtask migration-fixture
//! git add src-tauri/tests/fixtures/migrations
//! ```
//!
//! Historical fixtures are immutable evidence, not build output. The
//! generator creates only the newest missing fixture and refuses to touch
//! every existing byte. An exceptional repair requires the explicit
//! `--regenerate-existing NNN --reason "..."` flags; the generator records
//! that reason in `REGENERATIONS.md` so a binary rewrite cannot hide in a
//! diff. `SHA256SUMS` makes an unrecorded byte change fail this test suite.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use gmm_lib::core::settings::keys;
use gmm_lib::core::{Core, GameCode, Source, REINSTALL_SWAP_COLUMNS};
use sha2::{Digest, Sha256};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Row, SqlitePool};
use tempfile::TempDir;

// ---- the seed -------------------------------------------------------
//
// Written by `regenerate_the_migration_corpus`, asserted by the tests
// below. Values already present in a committed fixture are themselves
// historical and must not be edited; extend the version-aware seed when a
// new schema needs representative data instead of rewriting older members.

const SEEDED_INSTALL_PATH: &str = r"C:\Games\Genshin Impact\Genshin Impact Game";
const SEEDED_LIBRARY_ROOT: &str = r"D:\gmm-library";
const SEEDED_VARIANT_NAME: &str = "Hu Tao — Snow";
const SEEDED_UPSTREAM_VERSION: &str = "2.1.0";

/// Two Mods, deliberately including an enabled one: `enabled` and
/// `junction_dir_name` together decide which Junctions the startup
/// reconcile pass rebuilds, so losing either in a migration silently
/// unlinks a user's whole Library.
struct SeedMod {
    id: &'static str,
    name: &'static str,
    junction_dir_name: &'static str,
    enabled: bool,
    /// Source drives update-check behaviour, so the corpus carries one
    /// of each rather than two of a kind.
    source: Source,
}

const SEED_MODS: &[SeedMod] = &[
    SeedMod {
        id: "01JCORPUS0000000000000001",
        name: "Hu Tao Skin",
        junction_dir_name: "Hu Tao Skin",
        enabled: true,
        source: Source::Gamebanana,
    },
    SeedMod {
        id: "01JCORPUS0000000000000002",
        name: "Raiden: Shogun?",
        // The sanitised form — reserved characters stripped (ADR 0003).
        junction_dir_name: "Raiden Shogun",
        enabled: false,
        source: Source::Local,
    },
];

// ---- fixture plumbing ----------------------------------------------

fn src_tauri_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_dir() -> PathBuf {
    src_tauri_dir().join("tests/fixtures/migrations")
}

fn fixture_checksum_path() -> PathBuf {
    fixture_dir().join("SHA256SUMS")
}

fn fixture_regeneration_log_path() -> PathBuf {
    fixture_dir().join("REGENERATIONS.md")
}

/// Every migration the app ships, in order.
fn all_migrations() -> Vec<sqlx::migrate::Migration> {
    sqlx::migrate!("./migrations").iter().cloned().collect()
}

fn db_url(path: &Path) -> String {
    format!("sqlite://{}?mode=rwc", path.display())
}

fn fixture_name(version: usize, migration: &sqlx::migrate::Migration) -> String {
    // Spaces in the description would land in the filename, and a path
    // with spaces has to be escaped in a sqlite:// URL.
    let slug = migration.description.replace(' ', "_");
    format!("{version:03}_{slug}.db")
}

fn sha256(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    format!("{:x}", Sha256::digest(bytes))
}

fn read_fixture_checksums() -> BTreeMap<String, String> {
    let path = fixture_checksum_path();
    let contents =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (hash, name) = line
                .split_once("  ")
                .unwrap_or_else(|| panic!("{}: malformed checksum line {line:?}", path.display()));
            assert_eq!(hash.len(), 64, "{name}: SHA-256 must have 64 hex digits");
            (name.to_string(), hash.to_string())
        })
        .collect()
}

fn write_fixture_checksums(checksums: &BTreeMap<String, String>) {
    let contents = checksums
        .iter()
        .map(|(name, hash)| format!("{hash}  {name}\n"))
        .collect::<String>();
    std::fs::write(fixture_checksum_path(), contents).expect("write fixture checksums");
}

/// The checked-in corpus, ordered by schema version. Each entry is
/// `(version_number, path)` where the number is the 1-based migration
/// index the fixture stops at.
fn corpus() -> Vec<(usize, PathBuf)> {
    let dir = fixture_dir();
    let mut out: Vec<(usize, PathBuf)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e} — regenerate the corpus", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "db"))
        .map(|p| {
            let name = p
                .file_name()
                .expect("file name")
                .to_string_lossy()
                .to_string();
            let n: usize = name
                .split('_')
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(|| panic!("fixture {name} must start with its version number"));
            (n, p)
        })
        .collect();
    out.sort_by_key(|(n, _)| *n);
    assert_eq!(
        out.len(),
        all_migrations().len(),
        "the corpus must hold one fixture per migration — regenerate it \
         (see this file's docs) after adding a migration",
    );
    out
}

/// Copy a fixture somewhere writable. Tests must never migrate the
/// checked-in file in place; that would rewrite the corpus on every run.
fn stage(fixture: &Path, tmp: &TempDir) -> PathBuf {
    let dest = tmp.path().join("gmm.db");
    std::fs::copy(fixture, &dest).expect("stage fixture");
    dest
}

async fn open_core(db: &Path, tmp: &TempDir) -> Core {
    Core::new(tmp.path().join("library"), &db_url(db))
        .await
        .expect("Core::new must migrate the fixture forward")
}

async fn raw_pool(db: &Path) -> SqlitePool {
    let opts: SqliteConnectOptions = db_url(db).parse::<SqliteConnectOptions>().expect("db url");
    SqlitePool::connect_with(opts).await.expect("open sqlite")
}

// ---- the tests ------------------------------------------------------

/// The headline property: every schema version GMM has ever written
/// migrates forward, and the user's data is all still there afterwards.
#[tokio::test]
async fn every_schema_version_migrates_and_keeps_the_users_data() {
    for (version, fixture) in corpus() {
        let tmp = TempDir::new().expect("tmp");
        let db = stage(&fixture, &tmp);
        let core = open_core(&db, &tmp).await;
        let label = fixture.file_name().unwrap().to_string_lossy().to_string();

        // Every migration ran, and sqlx recorded each as successful.
        let pool = raw_pool(&db).await;
        let applied: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations WHERE success = 1")
                .fetch_one(&pool)
                .await
                .expect("count applied migrations");
        assert_eq!(
            applied as usize,
            all_migrations().len(),
            "{label}: every migration must be applied after startup",
        );
        pool.close().await;

        // The Game's install path survives.
        assert_eq!(
            core.game_install_path(GameCode::Gimi)
                .await
                .expect("install path")
                .map(|p| p.to_string_lossy().to_string()),
            Some(SEEDED_INSTALL_PATH.to_string()),
            "{label}: the Game install path must survive migration",
        );

        // Both Mods survive, with their enabled state and Junction
        // directory names intact.
        let mods = core.list_mods(GameCode::Gimi).await.expect("list mods");
        assert_eq!(
            mods.len(),
            SEED_MODS.len(),
            "{label}: Mod rows must survive"
        );
        for seed in SEED_MODS {
            let found = mods
                .iter()
                .find(|m| m.id == seed.id)
                .unwrap_or_else(|| panic!("{label}: Mod {} vanished", seed.id));
            assert_eq!(found.name, seed.name, "{label}: Mod name must survive");
            assert_eq!(
                found.source, seed.source,
                "{label}: Source must survive — it decides update-check behaviour",
            );

            assert_eq!(
                found.enabled, seed.enabled,
                "{label}: enabled state must survive — it decides which Junctions \
                 the startup reconcile rebuilds",
            );
        }

        // `junction_dir_name` has no place on the public `Mod` — it is
        // reconcile's business — but losing it in a migration would
        // orphan every Junction on disk, so check it at the source.
        let pool = raw_pool(&db).await;
        for seed in SEED_MODS {
            let name: String =
                sqlx::query_scalar("SELECT junction_dir_name FROM mods WHERE id = ?")
                    .bind(seed.id)
                    .fetch_one(&pool)
                    .await
                    .expect("read junction_dir_name");
            assert_eq!(
                name, seed.junction_dir_name,
                "{label}: the Junction directory name must survive migration",
            );
        }
        pool.close().await;

        // Settings arrived with migration 2.
        if version >= 2 {
            assert_eq!(
                core.library_root_override()
                    .await
                    .expect("library root override")
                    .map(|p| p.to_string_lossy().to_string()),
                Some(SEEDED_LIBRARY_ROOT.to_string()),
                "{label}: the Library root override must survive migration",
            );
        }

        // Variants arrived with migration 3.
        if version >= 3 {
            let variants = core
                .list_variants(SEED_MODS[0].id)
                .await
                .expect("list variants");
            assert_eq!(variants.len(), 1, "{label}: the Variant must survive");
            assert_eq!(variants[0].name, SEEDED_VARIANT_NAME);
            assert_eq!(
                core.active_variant_id(SEED_MODS[0].id)
                    .await
                    .expect("active variant"),
                Some(variants[0].id.clone()),
                "{label}: the active Variant selection must survive",
            );
        }

        // Per-Mod update tracking arrived with migration 5.
        if version >= 5 {
            let rows = core
                .list_mod_updates(GameCode::Gimi)
                .await
                .expect("list mod updates");
            let row = rows
                .iter()
                .find(|r| r.mod_id == SEED_MODS[0].id)
                .unwrap_or_else(|| panic!("{label}: update row for the seeded Mod vanished"));
            assert_eq!(
                row.upstream_version.as_deref(),
                Some(SEEDED_UPSTREAM_VERSION),
                "{label}: the last-seen upstream version must survive",
            );
        }
    }
}

/// Migrations 7 and 8 are recovery infrastructure, so a populated old fixture
/// cannot seed them before migration. Exercise their actual schema contract
/// after upgrading the schema-6 corpus member: the shape is inspectable, only
/// one active reinstall may own a Mod, and deleting that Mod retires the
/// witness.
#[tokio::test]
async fn reinstall_witness_migrations_enforce_the_recovery_contract() {
    let (_, fixture) = corpus()
        .into_iter()
        .find(|(version, _)| *version == 6)
        .expect("schema-6 fixture");
    let tmp = TempDir::new().expect("tmp");
    let db = stage(&fixture, &tmp);
    let core = open_core(&db, &tmp).await;
    drop(core);
    let pool = raw_pool(&db).await;

    let columns = sqlx::query("PRAGMA table_info(reinstall_swaps)")
        .fetch_all(&pool)
        .await
        .expect("inspect reinstall_swaps columns");
    let column_names: Vec<String> = columns
        .iter()
        .map(|row| row.try_get("name").expect("column name"))
        .collect();
    assert_eq!(
        column_names, REINSTALL_SWAP_COLUMNS,
        "the reinstall migrations must create every witness column in the expected order",
    );

    let foreign_keys = sqlx::query("PRAGMA foreign_key_list(reinstall_swaps)")
        .fetch_all(&pool)
        .await
        .expect("inspect reinstall_swaps foreign keys");
    let foreign_keys: Vec<(String, String, String, String)> = foreign_keys
        .iter()
        .map(|row| {
            (
                row.try_get("table").expect("foreign table"),
                row.try_get("from").expect("foreign from"),
                row.try_get("to").expect("foreign to"),
                row.try_get("on_delete").expect("foreign on_delete"),
            )
        })
        .collect();
    assert!(
        foreign_keys.contains(&(
            "mods".to_string(),
            "mod_id".to_string(),
            "id".to_string(),
            "CASCADE".to_string(),
        )),
        "migration 7 must declare mod_id -> mods(id) ON DELETE CASCADE: {foreign_keys:?}",
    );
    assert!(
        foreign_keys.contains(&(
            "games".to_string(),
            "game_code".to_string(),
            "code".to_string(),
            "NO ACTION".to_string(),
        )),
        "migration 7 must declare game_code -> games(code): {foreign_keys:?}",
    );

    let insert = |token: &'static str| {
        sqlx::query(
            "INSERT INTO reinstall_swaps (
                token, mod_id, game_code, library_path, staged_path,
                quarantine_path, old_identity, staged_identity, created_at
             ) VALUES (?, ?, 'gimi', ?, ?, ?, 'old-id', 'staged-id', ?)",
        )
        .bind(token)
        .bind(SEED_MODS[1].id)
        .bind(r"D:\gmm-library\gimi\01JCORPUS0000000000000002")
        .bind(format!(r"D:\gmm-library\gimi\.gmm-reinstall-{token}"))
        .bind(format!(r"D:\gmm-library\gimi\.gmm-delete-{token}"))
        .bind("2026-08-23T00:00:00Z")
    };
    insert("01JMIGRATIONWITNESS0000001")
        .execute(&pool)
        .await
        .expect("insert first active reinstall witness");
    let duplicate = insert("01JMIGRATIONWITNESS0000002").execute(&pool).await;
    assert!(
        duplicate.is_err(),
        "UNIQUE(mod_id) must reject a second active reinstall for one Mod",
    );

    sqlx::query("DELETE FROM mods WHERE id = ?")
        .bind(SEED_MODS[1].id)
        .execute(&pool)
        .await
        .expect("delete witnessed Mod");
    let remaining: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM reinstall_swaps WHERE mod_id = ?")
            .bind(SEED_MODS[1].id)
            .fetch_one(&pool)
            .await
            .expect("count cascaded reinstall witnesses");
    assert_eq!(
        remaining, 0,
        "ON DELETE CASCADE must retire the reinstall witness when its Mod is deleted",
    );

    let columns = sqlx::query("PRAGMA table_info(staged_library_operations)")
        .fetch_all(&pool)
        .await
        .expect("inspect staged_library_operations columns");
    let column_names: Vec<String> = columns
        .iter()
        .map(|row| row.try_get("name").expect("column name"))
        .collect();
    assert_eq!(
        column_names,
        [
            "id",
            "game_code",
            "operation",
            "staged_path",
            "staged_identity",
            "created_at",
            "recovery_error",
            "recovery_attempted_at",
            "recovery_attempts",
        ],
        "the staging migration must create the complete durable witness",
    );
    let foreign_keys = sqlx::query("PRAGMA foreign_key_list(staged_library_operations)")
        .fetch_all(&pool)
        .await
        .expect("inspect staged_library_operations foreign keys");
    let foreign_keys: Vec<(String, String, String)> = foreign_keys
        .iter()
        .map(|row| {
            (
                row.try_get("table").expect("foreign table"),
                row.try_get("from").expect("foreign from"),
                row.try_get("to").expect("foreign to"),
            )
        })
        .collect();
    let game_foreign_key = foreign_keys
        .iter()
        .any(|(table, from, to)| table == "games" && from == "game_code" && to == "code");
    assert!(
        game_foreign_key,
        "the staging witness must reference its Game: {foreign_keys:?}",
    );
    sqlx::query(
        "INSERT INTO staged_library_operations (
            id, game_code, operation, staged_path, staged_identity, created_at
         ) VALUES ('01JMIGRATIONSTAGE000000001', 'gimi', 'adopt',
                   'D:\\gmm-library\\gimi\\01JMIGRATIONSTAGE000000001',
                   'staged-directory-id', '2026-08-24T00:00:00Z')",
    )
    .execute(&pool)
    .await
    .expect("insert adopt staging witness without a Mod row");
    let invalid_operation = sqlx::query(
        "INSERT INTO staged_library_operations (
            id, game_code, operation, staged_path, staged_identity, created_at
         ) VALUES ('01JMIGRATIONSTAGE000000002', 'gimi', 'reinstall',
                   'D:\\gmm-library\\gimi\\01JMIGRATIONSTAGE000000002',
                   'other-staged-directory-id', '2026-08-24T00:00:00Z')",
    )
    .execute(&pool)
    .await;
    assert!(
        invalid_operation.is_err(),
        "the staging witness must reject operations outside adopt/import_zip",
    );
    pool.close().await;
}

/// Duplicate Mod rows are user state, not input for a migration-time survivor
/// policy. Stage the last schema that predates issue #185 with all four states
/// the reverted attempt would have destroyed, then run every current migration.
#[tokio::test]
async fn migrations_preserve_duplicate_rows_metadata_variants_and_reinstall_witnesses() {
    let fixture = fixture_dir().join("009_reinstall_recovery_junction_state.db");
    assert!(
        fixture.is_file(),
        "schema-9 fixture predating duplicate preservation"
    );
    let tmp = TempDir::new().expect("tmp");
    let db = stage(&fixture, &tmp);
    let pool = raw_pool(&db).await;
    let keeper_id = SEED_MODS[0].id;
    let duplicate_id = "01JDUPLICATEMIGRATION000001";
    let duplicate_variant_id = "01JDUPLICATEVARIANT0000001";
    let shared_path: String = sqlx::query_scalar("SELECT library_path FROM mods WHERE id = ?")
        .bind(keeper_id)
        .fetch_one(&pool)
        .await
        .expect("read shared path");

    sqlx::query(
        "INSERT INTO mods (
            id, game_code, name, source, library_path, junction_dir_name,
            enabled, created_at, gamebanana_id, source_url, author, version,
            upstream_version, update_check_enabled, screenshot_url
         ) VALUES (?, 'gimi', 'Duplicate With Metadata', 'gamebanana', ?,
                   'Duplicate With Metadata', 1, '2026-08-24T00:00:00Z',
                   98765, 'https://gamebanana.com/mods/98765', 'Duplicate Author', '4.2.0',
                   '4.3.0', 0, 'https://images.example.test/duplicate.png')",
    )
    .bind(duplicate_id)
    .bind(&shared_path)
    .execute(&pool)
    .await
    .expect("seed duplicate Mod row");
    sqlx::query("INSERT INTO mod_variants (id, mod_id, name, subpath) VALUES (?, ?, ?, ?)")
        .bind(duplicate_variant_id)
        .bind(duplicate_id)
        .bind("Selected Duplicate Variant")
        .bind("selected")
        .execute(&pool)
        .await
        .expect("seed duplicate Variant");
    sqlx::query("UPDATE mods SET active_variant_id = ? WHERE id = ?")
        .bind(duplicate_variant_id)
        .bind(duplicate_id)
        .execute(&pool)
        .await
        .expect("seed active Variant selection");
    sqlx::query(
        "INSERT INTO reinstall_swaps (
            token, mod_id, game_code, library_path, staged_path,
            quarantine_path, old_identity, staged_identity, created_at
         ) VALUES ('01JDUPLICATEWITNESS000001', ?, 'gimi', ?, ?, ?, ?, ?, ?)",
    )
    .bind(duplicate_id)
    .bind(&shared_path)
    .bind(r"D:\gmm-library\gimi\.gmm-reinstall-01JDUPLICATEWITNESS000001")
    .bind(r"D:\gmm-library\gimi\.gmm-delete-01JDUPLICATEWITNESS000001")
    .bind("0000000000000001:0000000000000002")
    .bind("0000000000000001:0000000000000003")
    .bind("2026-08-24T00:00:01Z")
    .execute(&pool)
    .await
    .expect("seed duplicate reinstall witness");

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run current migrations over duplicate state");

    let rows = sqlx::query(
        "SELECT id, name, source, enabled, gamebanana_id, author, version,
                upstream_version, update_check_enabled, screenshot_url, active_variant_id
         FROM mods WHERE library_path = ? ORDER BY id",
    )
    .bind(&shared_path)
    .fetch_all(&pool)
    .await
    .expect("read preserved duplicate rows");
    assert_eq!(
        rows.len(),
        2,
        "no migration may choose a duplicate survivor"
    );
    let duplicate = rows
        .iter()
        .find(|row| row.try_get::<String, _>("id").expect("id") == duplicate_id)
        .expect("duplicate row survives");
    assert_eq!(
        duplicate.try_get::<String, _>("name").expect("name"),
        "Duplicate With Metadata"
    );
    assert_eq!(
        duplicate.try_get::<String, _>("source").expect("source"),
        "gamebanana"
    );
    assert_eq!(duplicate.try_get::<i64, _>("enabled").expect("enabled"), 1);
    assert_eq!(
        duplicate
            .try_get::<i64, _>("gamebanana_id")
            .expect("GameBanana ID"),
        98765
    );
    assert_eq!(
        duplicate.try_get::<String, _>("author").expect("author"),
        "Duplicate Author"
    );
    assert_eq!(
        duplicate.try_get::<String, _>("version").expect("version"),
        "4.2.0"
    );
    assert_eq!(
        duplicate
            .try_get::<String, _>("upstream_version")
            .expect("upstream version"),
        "4.3.0"
    );
    assert_eq!(
        duplicate
            .try_get::<i64, _>("update_check_enabled")
            .expect("update check preference"),
        0
    );
    assert_eq!(
        duplicate
            .try_get::<String, _>("screenshot_url")
            .expect("screenshot URL"),
        "https://images.example.test/duplicate.png"
    );
    assert_eq!(
        duplicate
            .try_get::<String, _>("active_variant_id")
            .expect("active Variant"),
        duplicate_variant_id,
    );
    let variants: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mod_variants WHERE mod_id = ?")
        .bind(duplicate_id)
        .fetch_one(&pool)
        .await
        .expect("count preserved Variants");
    let witnesses: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM reinstall_swaps WHERE mod_id = ?")
            .bind(duplicate_id)
            .fetch_one(&pool)
            .await
            .expect("count preserved witness");
    assert_eq!(
        variants, 1,
        "the duplicate's Variant set survives migration"
    );
    assert_eq!(
        witnesses, 1,
        "the duplicate's reinstall witness survives migration"
    );
    pool.close().await;
}

/// Startup runs the migrator every time, not just on upgrade. Opening
/// an already-current database must change nothing.
#[tokio::test]
async fn reopening_an_already_migrated_database_changes_nothing() {
    let (_, oldest) = corpus().into_iter().next().expect("a corpus fixture");
    let tmp = TempDir::new().expect("tmp");
    let db = stage(&oldest, &tmp);

    let core = open_core(&db, &tmp).await;
    let before = core.list_mods(GameCode::Gimi).await.expect("mods before");
    drop(core);
    let after_first = std::fs::read(&db).expect("read migrated db");

    // Second startup against the same file.
    let core = open_core(&db, &tmp).await;
    let after = core.list_mods(GameCode::Gimi).await.expect("mods after");
    assert_eq!(
        before.len(),
        after.len(),
        "a second startup must not duplicate or drop rows",
    );
    drop(core);

    let pool = raw_pool(&db).await;
    let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await
        .expect("count migrations");
    pool.close().await;
    assert_eq!(
        applied as usize,
        all_migrations().len(),
        "a second startup must not re-record migrations",
    );

    let after_second = std::fs::read(&db).expect("read reopened db");
    assert_eq!(
        after_first.len(),
        after_second.len(),
        "a no-op startup must not rewrite the database",
    );
}

/// A migration interrupted partway — GMM killed, machine powered off —
/// must leave a database the next startup can finish migrating.
///
/// sqlx runs each migration inside a transaction, so an interruption is
/// exactly an uncommitted transaction: this stages one by applying a
/// later migration's DDL and dropping the connection without a commit,
/// then starts up normally. If a future migration is ever marked
/// `no_tx`, this test is what notices that a crash can now strand a
/// half-applied schema.
#[tokio::test]
async fn an_interrupted_migration_leaves_a_database_startup_can_finish() {
    let (_, oldest) = corpus().into_iter().next().expect("a corpus fixture");
    let tmp = TempDir::new().expect("tmp");
    let db = stage(&oldest, &tmp);

    {
        let pool = raw_pool(&db).await;
        let mut tx = pool.begin().await.expect("begin");
        // The second migration's work, abandoned midway.
        sqlx::query("CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT)")
            .execute(&mut *tx)
            .await
            .expect("partial DDL");
        // No commit, no rollback — drop is the crash.
        drop(tx);
        pool.close().await;
    }

    let core = open_core(&db, &tmp).await;
    let mods = core.list_mods(GameCode::Gimi).await.expect("list mods");
    assert_eq!(
        mods.len(),
        SEED_MODS.len(),
        "an interrupted migration must not cost the user their Mods",
    );
    // The abandoned migration's table is present and usable, not left
    // half-built. A settings round-trip proves it; onboarding state is
    // the cheapest one, since unlike the Library root it writes nothing
    // to disk.
    core.mark_onboarding_complete(true)
        .await
        .expect("settings table must work after the interrupted migration");
    let status = core.onboarding_status().await.expect("onboarding status");
    assert!(
        status.complete && status.skipped,
        "the abandoned migration must have been re-applied cleanly on startup",
    );
}

// ---- the generator --------------------------------------------------

const REGENERATE_VERSION_ENV: &str = "GMM_REGENERATE_MIGRATION_FIXTURE";
const REGENERATION_REASON_ENV: &str = "GMM_MIGRATION_FIXTURE_REASON";

enum GenerationPlan {
    Create {
        version: usize,
        path: PathBuf,
    },
    Regenerate {
        version: usize,
        path: PathBuf,
        reason: String,
    },
}

fn plan_fixture_generation(
    migrations: &[sqlx::migrate::Migration],
    regenerate_version: Option<usize>,
    regeneration_reason: Option<String>,
) -> Result<GenerationPlan, String> {
    let dir = fixture_dir();
    let expected: Vec<PathBuf> = migrations
        .iter()
        .enumerate()
        .map(|(idx, migration)| dir.join(fixture_name(idx + 1, migration)))
        .collect();

    if let Some(version) = regenerate_version {
        let reason = regeneration_reason
            .filter(|reason| !reason.trim().is_empty() && !reason.contains(['\r', '\n']))
            .ok_or_else(|| {
                "regenerating history requires --reason so the exceptional rewrite is recorded"
                    .to_string()
            })?;
        let path = expected.get(version.saturating_sub(1)).ok_or_else(|| {
            format!(
                "cannot regenerate schema {version}: current migrations end at schema {}",
                migrations.len()
            )
        })?;
        if !path.is_file() {
            return Err(format!(
                "cannot regenerate {}: it does not exist; default generation creates only the newest missing fixture",
                path.display()
            ));
        }
        return Ok(GenerationPlan::Regenerate {
            version,
            path: path.clone(),
            reason,
        });
    }

    if regeneration_reason.is_some() {
        return Err("--reason is valid only with --regenerate-existing".to_string());
    }

    let missing: Vec<(usize, PathBuf)> = expected
        .iter()
        .enumerate()
        .filter(|(_, path)| !path.is_file())
        .map(|(idx, path)| (idx + 1, path.clone()))
        .collect();
    if missing.is_empty() {
        let latest = expected
            .last()
            .ok_or_else(|| "no migrations exist".to_string())?;
        return Err(format!(
            "fixture {} already exists and is immutable; default generation only creates the newest missing fixture",
            latest.file_name().expect("fixture filename").to_string_lossy()
        ));
    }
    let newest_version = migrations.len();
    if missing.len() != 1 || missing[0].0 != newest_version {
        let names = missing
            .iter()
            .map(|(_, path)| {
                path.file_name()
                    .expect("fixture filename")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "refusing to reconstruct historical gaps ({names}); only the newest missing fixture may be created"
        ));
    }
    Ok(GenerationPlan::Create {
        version: newest_version,
        path: missing[0].1.clone(),
    })
}

async fn write_fixture(path: &Path, version: usize, all: &[sqlx::migrate::Migration]) {
    let partial = sqlx::migrate::Migrator {
        migrations: std::borrow::Cow::Owned(all[..version].to_vec()),
        ignore_missing: false,
        locking: true,
        no_tx: false,
    };
    let pool = raw_pool(path).await;
    partial.run(&pool).await.expect("apply partial migrations");
    seed(&pool, version).await;
    sqlx::query("VACUUM").execute(&pool).await.expect("vacuum");
    pool.close().await;
}

/// Creates only the newest missing fixture. Existing fixtures are historical
/// evidence and are immutable unless the xtask's explicit regeneration flags
/// carry a reason that is recorded beside the byte checksums.
#[tokio::test]
#[ignore = "creates the newest missing fixture; run deliberately through cargo xtask"]
async fn regenerate_the_migration_corpus() {
    let dir = fixture_dir();
    std::fs::create_dir_all(&dir).expect("create fixture dir");
    let all = all_migrations();
    let regenerate_version = std::env::var(REGENERATE_VERSION_ENV)
        .ok()
        .map(|raw| raw.parse::<usize>().expect("numeric regeneration version"));
    let regeneration_reason = std::env::var(REGENERATION_REASON_ENV).ok();
    let plan = plan_fixture_generation(&all, regenerate_version, regeneration_reason)
        .unwrap_or_else(|message| panic!("{message}"));

    let (version, path, reason) = match plan {
        GenerationPlan::Create { version, path } => (version, path, None),
        GenerationPlan::Regenerate {
            version,
            path,
            reason,
        } => (version, path, Some(reason)),
    };
    let staging_dir = TempDir::new_in(&dir).expect("create fixture staging directory");
    let staged_path = staging_dir.path().join("fixture.db");
    write_fixture(&staged_path, version, &all).await;

    if reason.is_some() {
        std::fs::remove_file(&path).unwrap_or_else(|e| {
            panic!("remove explicitly selected fixture {}: {e}", path.display())
        });
    }
    std::fs::rename(&staged_path, &path)
        .unwrap_or_else(|e| panic!("install generated fixture {}: {e}", path.display()));

    let name = path
        .file_name()
        .expect("fixture filename")
        .to_string_lossy()
        .into_owned();
    let hash = sha256(&path);
    let mut checksums = read_fixture_checksums();
    checksums.insert(name.clone(), hash.clone());
    write_fixture_checksums(&checksums);
    if let Some(reason) = reason {
        let mut log = OpenOptions::new()
            .append(true)
            .open(fixture_regeneration_log_path())
            .expect("open fixture regeneration log");
        writeln!(log, "- `{name}` (`{hash}`): {reason}")
            .expect("record fixture regeneration reason");
    }
    eprintln!("wrote {}", path.display());
}

#[test]
fn regenerating_existing_fixture_without_opt_in_is_rejected() {
    let error = plan_fixture_generation(&all_migrations(), None, None)
        .err()
        .expect("a complete corpus must not be regenerated by default");
    let latest = corpus()
        .last()
        .expect("latest fixture")
        .1
        .file_name()
        .expect("fixture filename")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        error,
        format!(
            "fixture {latest} already exists and is immutable; default generation only creates the newest missing fixture"
        ),
        "the generator must fail loudly before it can overwrite historical bytes",
    );
}

#[test]
fn committed_fixtures_match_their_immutable_checksums() {
    let checksums = read_fixture_checksums();
    let fixtures = corpus();
    assert_eq!(
        checksums.len(),
        fixtures.len(),
        "SHA256SUMS must contain exactly one entry per historical fixture",
    );
    for (_, fixture) in fixtures {
        let name = fixture
            .file_name()
            .expect("fixture filename")
            .to_string_lossy();
        let expected = checksums
            .get(name.as_ref())
            .unwrap_or_else(|| panic!("{name}: missing immutable checksum"));
        assert_eq!(
            sha256(&fixture),
            *expected,
            "{name}: committed historical fixture bytes changed; restore the fixture or use cargo xtask migration-fixture --regenerate-existing NNN --reason \"...\" so the exceptional rewrite is explicit and recorded",
        );
    }
}

/// Insert the representative rows every table available at `version`
/// can hold.
async fn seed(pool: &SqlitePool, version: usize) {
    sqlx::query("UPDATE games SET install_path = ? WHERE code = 'gimi'")
        .bind(SEEDED_INSTALL_PATH)
        .execute(pool)
        .await
        .expect("seed install path");

    for m in SEED_MODS {
        sqlx::query(
            "INSERT INTO mods (id, game_code, name, source, library_path, junction_dir_name,
                               enabled, created_at)
             VALUES (?, 'gimi', ?, ?, ?, ?, ?, '2026-05-21T00:00:00Z')",
        )
        .bind(m.id)
        .bind(m.name)
        .bind(m.source.as_str())
        .bind(format!(r"{SEEDED_LIBRARY_ROOT}\gimi\{}", m.id))
        .bind(m.junction_dir_name)
        .bind(i64::from(m.enabled))
        .execute(pool)
        .await
        .expect("seed mod");
    }

    if version >= 2 {
        for (key, value) in [
            (keys::library_root(), SEEDED_LIBRARY_ROOT),
            (keys::onboarding_complete(), "true"),
        ] {
            sqlx::query("INSERT INTO settings (key, value) VALUES (?, ?)")
                .bind(key)
                .bind(value)
                .execute(pool)
                .await
                .expect("seed setting");
        }
    }

    if version >= 3 {
        let variant_id = "01JCORPUSVARIANT000000001";
        sqlx::query("INSERT INTO mod_variants (id, mod_id, name, subpath) VALUES (?, ?, ?, ?)")
            .bind(variant_id)
            .bind(SEED_MODS[0].id)
            .bind(SEEDED_VARIANT_NAME)
            .bind("Snow")
            .execute(pool)
            .await
            .expect("seed variant");
        sqlx::query("UPDATE mods SET active_variant_id = ? WHERE id = ?")
            .bind(variant_id)
            .bind(SEED_MODS[0].id)
            .execute(pool)
            .await
            .expect("seed active variant");
    }

    if version >= 4 {
        sqlx::query(
            "UPDATE mods SET gamebanana_id = 12345, source_url = ?, author = ?, version = ?
             WHERE id = ?",
        )
        .bind("https://gamebanana.com/mods/12345")
        .bind("SomeAuthor")
        .bind("2.0.0")
        .bind(SEED_MODS[0].id)
        .execute(pool)
        .await
        .expect("seed gamebanana metadata");
    }

    if version >= 5 {
        sqlx::query("UPDATE mods SET upstream_version = ? WHERE id = ?")
            .bind(SEEDED_UPSTREAM_VERSION)
            .bind(SEED_MODS[0].id)
            .execute(pool)
            .await
            .expect("seed upstream version");
    }
}

/// Sanity check on the fixtures themselves: an old fixture must really
/// be old. Without this, a corpus accidentally regenerated at the
/// current schema would make every test above pass while proving
/// nothing about migrating.
#[tokio::test]
async fn the_corpus_really_holds_old_schema_versions() {
    for (version, fixture) in corpus() {
        let tmp = TempDir::new().expect("tmp");
        let db = stage(&fixture, &tmp);
        let pool = raw_pool(&db).await;
        let rows = sqlx::query("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&pool)
            .await
            .expect("read _sqlx_migrations");
        pool.close().await;

        let label = fixture.file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(
            rows.len(),
            version,
            "{label}: fixture must carry exactly {version} applied migration(s), \
             not the current schema — regenerate the corpus",
        );
        let recorded: Vec<i64> = rows
            .iter()
            .map(|r| r.try_get::<i64, _>("version").expect("version column"))
            .collect();
        let expected: Vec<i64> = all_migrations()
            .iter()
            .take(version)
            .map(|m| m.version)
            .collect();
        assert_eq!(recorded, expected, "{label}: unexpected migration set");
    }
}
