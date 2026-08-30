//! Slice 15: Library path settings.
//!
//! Two flows:
//!   * Set the global Library root → every game's subtree relocates.
//!   * Set a per-game override → only that game relocates, the others
//!     stay put.
//!
//! Junctions are recreated on the new root for any mod that was
//! enabled before the move.

use std::fs;

use gmm_lib::core::{Core, Error, GameCode};
use tempfile::TempDir;

async fn persist_library_setting(tmp: &TempDir, key: &str, path: &std::path::Path) {
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let pool = sqlx::SqlitePool::connect(&db_url)
        .await
        .expect("open fixture database");
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(path.to_string_lossy().as_ref())
    .execute(&pool)
    .await
    .expect("persist pre-existing Library setting");
    pool.close().await;
}

async fn make_mod(
    core: &Core,
    game: GameCode,
    name: &str,
    fixture_root: &std::path::Path,
) -> gmm_lib::core::Mod {
    fs::create_dir_all(fixture_root).expect("fixture dir");
    fs::write(
        fixture_root.join("merged.ini"),
        b"[TextureOverride]\nhash=1\n" as &[u8],
    )
    .expect("ini");
    core.adopt_folder(game, fixture_root, name)
        .await
        .expect("adopt")
}

#[tokio::test]
async fn changing_global_root_relocates_every_mod_and_rebuilds_junctions() {
    let tmp = TempDir::new().expect("tmp");
    let library_default = tmp.path().join("default_library");
    let library_new = tmp.path().join("relocated_library");
    let game_install = tmp.path().join("Genshin");
    let game_mods = game_install.join("Mods");
    fs::create_dir_all(&game_mods).expect("mods dir");

    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_default.clone(), &db_url)
        .await
        .expect("init core");
    core.set_game_install_path(GameCode::Gimi, &game_install)
        .await
        .expect("install path");

    let mod_a = make_mod(
        &core,
        GameCode::Gimi,
        "Mod A",
        &tmp.path().join("fixture_a"),
    )
    .await;
    let mod_b = make_mod(
        &core,
        GameCode::Gimi,
        "Mod B",
        &tmp.path().join("fixture_b"),
    )
    .await;
    core.set_enabled(&mod_a.id, true, &game_mods)
        .await
        .expect("enable A");
    core.set_enabled(&mod_b.id, true, &game_mods)
        .await
        .expect("enable B");

    let report = core
        .set_library_root(Some(&library_new))
        .await
        .expect("set root");

    assert_eq!(report.relocated.len(), 2, "both mods relocated: {report:?}");
    assert!(
        library_new.join("gimi").exists(),
        "new gimi subtree present"
    );
    assert!(
        !library_default.join("gimi").join(&mod_a.id).exists(),
        "old per-mod path is gone",
    );

    // Junctions point into the new Library.
    let link_a = game_mods.join("Mod A");
    let link_b = game_mods.join("Mod B");
    assert!(link_a.exists() && link_a.join("merged.ini").exists());
    assert!(link_b.exists() && link_b.join("merged.ini").exists());

    let resolved = core.resolved_library_root().await.expect("resolved");
    assert_eq!(resolved, library_new);
}

/// Relocation withdraws every enabled deployment before moving Library bytes.
/// If that withdrawal fails, continuing would leave the game loading through a
/// deployment entry whose target is about to move.
///
/// Mutation oracle: discarding `junction::remove`'s result in `move_root`
/// makes relocation return `Ok` and fires the named refusal assertion below.
#[tokio::test]
async fn relocation_stops_when_a_deployment_entry_survives_withdrawal() {
    let tmp = TempDir::new().expect("tmp");
    let library_default = tmp.path().join("default_library");
    let library_new = tmp.path().join("relocated_library");
    let game_install = tmp.path().join("Genshin");
    let game_mods = game_install.join("Mods");
    fs::create_dir_all(&game_mods).expect("mods dir");

    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_default.clone(), &db_url)
        .await
        .expect("init core");
    core.set_game_install_path(GameCode::Gimi, &game_install)
        .await
        .expect("install path");
    let installed = make_mod(
        &core,
        GameCode::Gimi,
        "Withdrawal Refusal",
        &tmp.path().join("fixture"),
    )
    .await;
    core.set_enabled(&installed.id, true, &game_mods)
        .await
        .expect("enable Mod");

    let deployment = game_mods.join("Withdrawal Refusal");
    gmm_lib::core::junction::remove(&deployment).expect("remove real deployment Junction");
    fs::create_dir(&deployment).expect("plant non-link deployment entry");
    fs::write(deployment.join("still-loading.ini"), b"hash=2\n")
        .expect("make the deployment entry non-empty so removal must fail");

    let error = core
        .set_library_root(Some(&library_new))
        .await
        .expect_err("relocation must stop when a deployment entry survives withdrawal");
    assert!(
        matches!(error, Error::Io { ref path, .. } if path == &deployment),
        "relocation must identify the deployment entry whose withdrawal failed, got: {error}",
    );
    assert!(
        installed.library_path.join("merged.ini").is_file(),
        "failed withdrawal must stop relocation before Library bytes move",
    );
    assert!(
        deployment.join("still-loading.ini").is_file(),
        "the surviving deployment entry must remain visible and unreported as removed",
    );
}

/// A Library root may never overlap the Model Importer backup tree
/// (`<data dir>/backups`): importer backups and their sidecar
/// recovery-remnant markers are written there outside the Library writer
/// fence, and Library content inside the backups tree could become an
/// importer rollback candidate.
///
/// Mutation oracle: deleting the `ensure_library_root_disjoint_from_backups`
/// call in `set_library_path_for_game` lets the override relocate Library
/// bytes and fires the named refusal assertions below.
#[tokio::test]
async fn per_game_library_override_may_not_overlap_the_importer_backup_tree() {
    let tmp = TempDir::new().expect("tmp");
    let library_default = tmp.path().join("default_library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_default.clone(), &db_url)
        .await
        .expect("init core");
    let backups_root = tmp.path().join("backups");
    let not_yet = backups_root.join("library");

    // The backup tree does not exist until the first importer install, so
    // the refusal cannot rely on canonicalising it.
    let error = core
        .set_library_path_for_game(GameCode::Gimi, Some(&not_yet))
        .await
        .expect_err("a per-game override inside a not-yet-created backup tree must be refused");
    assert!(
        matches!(error, Error::LibraryRootOverlapsBackups { .. }),
        "the refusal must cover a root inside a not-yet-created backup tree, got: {error}",
    );

    fs::create_dir_all(backups_root.join("gimi")).expect("backup tree");

    let error = core
        .set_library_path_for_game(GameCode::Gimi, Some(&backups_root))
        .await
        .expect_err("a per-game override at the backup tree root must be refused");
    assert!(
        matches!(error, Error::LibraryRootOverlapsBackups { ref path, .. } if path == &backups_root),
        "the refusal must name the overlapping root, got: {error}",
    );

    let inside = backups_root.join("gimi").join("library");
    let error = core
        .set_library_path_for_game(GameCode::Gimi, Some(&inside))
        .await
        .expect_err("a per-game override inside the backup tree must be refused");
    assert!(
        matches!(error, Error::LibraryRootOverlapsBackups { .. }),
        "the refusal must also cover a root inside the backup tree, got: {error}",
    );

    // The data directory contains the backup tree, so the unfenced marker
    // writes would land inside Library storage too.
    let error = core
        .set_library_path_for_game(GameCode::Gimi, Some(tmp.path()))
        .await
        .expect_err("a root containing the backup tree must be refused");
    assert!(
        matches!(error, Error::LibraryRootOverlapsBackups { .. }),
        "the refusal must also cover a root containing the backup tree, got: {error}",
    );

    assert_eq!(
        core.resolved_library_root_for(GameCode::Gimi)
            .await
            .expect("resolved root"),
        library_default.join("gimi"),
        "every refused override must leave the effective Library root untouched",
    );
}

/// The global Library root obeys the same importer-backup-tree
/// disjointness rule as the per-game override.
///
/// Mutation oracle: deleting the `ensure_library_root_disjoint_from_backups`
/// call in `set_library_root` lets the global root relocate Library bytes
/// into the backup tree and fires the named refusal assertion below.
#[tokio::test]
async fn global_library_root_may_not_overlap_the_importer_backup_tree() {
    let tmp = TempDir::new().expect("tmp");
    let library_default = tmp.path().join("default_library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_default.clone(), &db_url)
        .await
        .expect("init core");
    let backups_root = tmp.path().join("backups");
    fs::create_dir_all(backups_root.join("gimi")).expect("backup tree");

    let error = core
        .set_library_root(Some(&backups_root))
        .await
        .expect_err("a global root at the backup tree must be refused");
    assert!(
        matches!(error, Error::LibraryRootOverlapsBackups { ref path, .. } if path == &backups_root),
        "the refusal must name the overlapping root, got: {error}",
    );

    assert_eq!(
        core.resolved_library_root().await.expect("resolved root"),
        library_default,
        "a refused global root must leave the effective Library root untouched",
    );
}

/// Existing settings written by an older GMM must pass through the same
/// validating resolver as newly selected roots. Settings still reads the raw
/// override, and changing away from the unsafe root repairs only the setting.
///
/// Mutation oracle: removing validation from `resolved_library_root` or
/// `resolved_library_root_for` fires the named existing-overlap assertions.
#[tokio::test]
async fn existing_overlapping_library_configuration_is_refused_when_used_but_remains_fixable() {
    let tmp = TempDir::new().expect("tmp");
    let library_default = tmp.path().join("default_library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_default.clone(), &db_url)
        .await
        .expect("init core");
    let backups_root = tmp.path().join("backups");
    let unsafe_global = backups_root.join("legacy-library");
    fs::create_dir_all(&unsafe_global).expect("legacy overlapping root");
    fs::write(unsafe_global.join("must-stay.txt"), b"untouched").expect("unsafe-root sentinel");
    persist_library_setting(&tmp, "library.root", &unsafe_global).await;

    let global = core.resolved_library_root().await;
    assert!(
        matches!(
            global,
            Err(Error::LibraryRootOverlapsBackups { ref path, ref backups })
                if path == &unsafe_global && backups == &backups_root
        ),
        "an existing global overlap must be refused when GMM resolves it for use and name both paths, got: {global:?}",
    );
    assert_eq!(
        core.library_root_override()
            .await
            .expect("read raw global override"),
        Some(unsafe_global.clone()),
        "Settings must still be able to read the unsafe override so the user can repair it",
    );

    let safe_global = tmp.path().join("safe-library");
    core.set_library_root(Some(&safe_global))
        .await
        .expect("repair unsafe global setting");
    assert_eq!(
        core.resolved_library_root()
            .await
            .expect("resolved repaired global"),
        safe_global,
    );
    assert_eq!(
        fs::read(unsafe_global.join("must-stay.txt")).expect("read unsafe-root sentinel"),
        b"untouched",
        "repair must not read, move, or delete bytes from the overlapping root",
    );

    let unsafe_game = backups_root.join("legacy-gimi");
    fs::create_dir_all(&unsafe_game).expect("legacy per-game overlapping root");
    persist_library_setting(&tmp, "library.gimi", &unsafe_game).await;
    let per_game = core.resolved_library_root_for(GameCode::Gimi).await;
    assert!(
        matches!(
            per_game,
            Err(Error::LibraryRootOverlapsBackups { ref path, ref backups })
                if path == &unsafe_game && backups == &backups_root
        ),
        "an existing per-game overlap must be refused when GMM resolves it for use and name both paths, got: {per_game:?}",
    );
    assert_eq!(
        core.library_root_override_for_game(GameCode::Gimi)
            .await
            .expect("read raw per-game override"),
        Some(unsafe_game),
        "Settings must still be able to read the unsafe per-game override",
    );

    let safe_game = tmp.path().join("safe-gimi");
    core.set_library_path_for_game(GameCode::Gimi, Some(&safe_game))
        .await
        .expect("repair unsafe per-game setting");
    assert_eq!(
        core.resolved_library_root_for(GameCode::Gimi)
            .await
            .expect("resolved repaired per-game root"),
        safe_game,
    );
}

/// Canonicalisation can fail for an ordinary future descendant, for a missing
/// tail reached through a junction/symlink, or because the spelling contains
/// `..`. The fallback must retain case-insensitive overlap evidence for all
/// three shapes.
///
/// Mutation oracle: making the fallback case-sensitive fires the first named
/// refusal assertion; bypassing ancestor resolution or lexical normalisation
/// fires the corresponding later assertion.
#[tokio::test]
async fn overlap_fallback_handles_case_missing_descendants_links_and_parent_segments() {
    let tmp = TempDir::new().expect("tmp");
    let library_default = tmp.path().join("default_library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_default, &db_url)
        .await
        .expect("init core");
    let backups_root = tmp.path().join("backups");

    let differently_cased_missing = tmp.path().join("BACKUPS").join("Future-Library");
    let error = core
        .set_library_root(Some(&differently_cased_missing))
        .await;
    assert!(
        matches!(error, Err(Error::LibraryRootOverlapsBackups { .. })),
        "the fallback must refuse a differently-cased not-yet-created descendant of the backup tree, got: {error:?}",
    );

    fs::create_dir_all(&backups_root).expect("backup tree");
    let alias = tmp.path().join("backup-alias");
    gmm_lib::core::junction::create(&alias, &backups_root).expect("backup tree alias");
    let through_alias = alias.join("future-library");
    let error = core.set_library_root(Some(&through_alias)).await;
    assert!(
        matches!(error, Err(Error::LibraryRootOverlapsBackups { .. })),
        "the fallback must refuse a missing descendant reached through a junction or symlink ancestor, got: {error:?}",
    );

    let with_parent = backups_root
        .join("not-created")
        .join("..")
        .join("future-library");
    let error = core.set_library_root(Some(&with_parent)).await;
    assert!(
        matches!(error, Err(Error::LibraryRootOverlapsBackups { .. })),
        "the fallback must refuse an overlapping path whose spelling contains a parent segment, got: {error:?}",
    );
}

/// Diagnostics is a Settings reporting path, so it must preserve the raw
/// effective root and describe an overlap instead of asking the validating
/// filesystem-use resolver to reject the export.
///
/// Mutation oracle: restoring `resolved_library_root` in `settings_snapshot`
/// fires the named export-availability assertion below.
#[tokio::test]
async fn settings_snapshot_reports_legacy_library_overlap_without_refusing_export() {
    let tmp = TempDir::new().expect("tmp");
    let library_default = tmp.path().join("default_library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_default, &db_url)
        .await
        .expect("init core");
    let unsafe_root = tmp.path().join("backups").join("legacy-library");
    persist_library_setting(&tmp, "library.root", &unsafe_root).await;

    let snapshot = match core.settings_snapshot().await {
        Ok(snapshot) => snapshot,
        Err(error) => panic!(
            "diagnostics export must remain available for an overlapping Library configuration: {error}"
        ),
    };
    assert_eq!(
        snapshot.library_root,
        Some(unsafe_root),
        "diagnostics must report the raw effective Library root",
    );
    assert!(
        snapshot.library_root_overlaps_importer_backups,
        "diagnostics must explicitly record that the Library root overlaps importer backups",
    );
}

/// The case-insensitive missing-tail fallback is additive to the legacy
/// overlap evidence. In particular, a link entry lexically inside `backups`
/// remains refused even if its target resolves outside that tree.
///
/// Mutation oracle: removing the raw `Path::starts_with` disjunct records the
/// symlink-leaf case as loosened and fires the named differential assertion.
#[tokio::test]
async fn overlap_guard_is_never_looser_than_legacy_corpus() {
    let tmp = TempDir::new().expect("tmp");
    let library_default = tmp.path().join("default_library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_default, &db_url)
        .await
        .expect("init core");
    let backups_root = tmp.path().join("backups");
    fs::create_dir_all(backups_root.join("inside-existing")).expect("backup tree");

    let outside_target = tmp.path().join("outside-target");
    fs::create_dir(&outside_target).expect("outside link target");
    let symlink_leaf = backups_root.join("symlink-leaf");
    gmm_lib::core::junction::create(&symlink_leaf, &outside_target)
        .expect("symlink leaf inside backups");

    let backup_alias = tmp.path().join("backup-alias");
    gmm_lib::core::junction::create(&backup_alias, &backups_root).expect("backup tree alias");

    let cases = [
        ("inside, existing", backups_root.join("inside-existing")),
        (
            "inside, UPPER missing",
            tmp.path().join("BACKUPS").join("future-library"),
        ),
        ("symlink leaf inside backups", symlink_leaf),
        (
            "missing tail through alias",
            backup_alias.join("future-library"),
        ),
        (
            "parent segment inside backups",
            backups_root
                .join("not-created")
                .join("..")
                .join("future-library"),
        ),
        ("sibling prefix", tmp.path().join("backups2")),
        ("separate root", tmp.path().join("safe-library")),
        ("ancestor of backups", tmp.path().to_path_buf()),
    ];

    let legacy_path_within = |path: &std::path::Path, ancestor: &std::path::Path| {
        let canonical = |candidate: &std::path::Path| {
            fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf())
        };
        canonical(path).starts_with(canonical(ancestor))
    };
    let legacy_overlap = |path: &std::path::Path, ancestor: &std::path::Path| {
        legacy_path_within(path, ancestor) || path.starts_with(ancestor)
    };

    let mut tightened = Vec::new();
    let mut loosened = Vec::new();
    for (name, candidate) in cases {
        let base =
            legacy_overlap(&candidate, &backups_root) || legacy_overlap(&backups_root, &candidate);
        persist_library_setting(&tmp, "library.root", &candidate).await;
        let current = matches!(
            core.resolved_library_root().await,
            Err(Error::LibraryRootOverlapsBackups { .. })
        );
        match (base, current) {
            (false, true) => tightened.push(name),
            (true, false) => loosened.push(name),
            _ => {}
        }
    }

    assert!(
        loosened.is_empty(),
        "the overlap guard must never be looser than base; loosened cases: {loosened:?}",
    );
    assert!(
        tightened.contains(&"inside, UPPER missing"),
        "the differential must retain the intended case-insensitive tightening: {tightened:?}",
    );
    eprintln!("overlap differential: tightened={tightened:?}; loosened={loosened:?}");
}

#[tokio::test]
async fn per_game_override_relocates_only_that_game() {
    let tmp = TempDir::new().expect("tmp");
    let library_default = tmp.path().join("default_library");
    let gimi_override = tmp.path().join("gimi_drive");
    let gimi_install = tmp.path().join("Genshin");
    let gimi_mods = gimi_install.join("Mods");
    let srmi_install = tmp.path().join("StarRail");
    let srmi_mods = srmi_install.join("Mods");
    fs::create_dir_all(&gimi_mods).expect("gimi mods dir");
    fs::create_dir_all(&srmi_mods).expect("srmi mods dir");

    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_default.clone(), &db_url)
        .await
        .expect("init core");
    core.set_game_install_path(GameCode::Gimi, &gimi_install)
        .await
        .expect("gimi install");
    core.set_game_install_path(GameCode::Srmi, &srmi_install)
        .await
        .expect("srmi install");

    let gimi_mod = make_mod(&core, GameCode::Gimi, "Genshin Mod", &tmp.path().join("g")).await;
    let srmi_mod = make_mod(
        &core,
        GameCode::Srmi,
        "Star Rail Mod",
        &tmp.path().join("s"),
    )
    .await;
    core.set_enabled(&gimi_mod.id, true, &gimi_mods)
        .await
        .expect("enable gimi");
    core.set_enabled(&srmi_mod.id, true, &srmi_mods)
        .await
        .expect("enable srmi");

    let srmi_old_path = library_default.join("srmi");
    assert!(
        srmi_old_path.exists(),
        "precondition: srmi subtree exists at default"
    );

    let report = core
        .set_library_path_for_game(GameCode::Gimi, Some(&gimi_override))
        .await
        .expect("set gimi override");

    assert_eq!(
        report.relocated.len(),
        1,
        "only the gimi mod relocates: {report:?}"
    );

    // Genshin junction now resolves into the override directory.
    let gimi_link = gimi_mods.join("Genshin Mod");
    assert!(gimi_link.exists() && gimi_link.join("merged.ini").exists());

    // Star Rail subtree untouched.
    assert!(
        srmi_old_path.exists(),
        "srmi must not be moved by a gimi-only override"
    );
    let srmi_link = srmi_mods.join("Star Rail Mod");
    assert!(srmi_link.exists() && srmi_link.join("merged.ini").exists());

    // Resolvers see the new gimi path but unchanged global default.
    assert_eq!(
        core.resolved_library_root_for(GameCode::Gimi)
            .await
            .unwrap(),
        gimi_override,
    );
    assert_eq!(
        core.resolved_library_root_for(GameCode::Srmi)
            .await
            .unwrap(),
        library_default.join("srmi"),
    );
}
