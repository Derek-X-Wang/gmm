//! NTFS reserves DOS device names (CON, PRN, AUX, NUL, COM1..9, LPT1..9)
//! and refuses to create files or directories with those names regardless
//! of case or extension. A Mod called "CON" must still produce a usable
//! junction directory name.

use std::fs;

use gmm_lib::core::{Core, GameCode};
use tempfile::TempDir;

async fn make_core_with(tmp: &TempDir) -> (Core, std::path::PathBuf) {
    let library_root = tmp.path().join("library");
    let game_mods = tmp.path().join("Genshin/Mods");
    fs::create_dir_all(&game_mods).expect("game mods dir");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_root, &db_url).await.expect("init core");
    (core, game_mods)
}

fn make_fixture(tmp: &TempDir, name: &str) -> std::path::PathBuf {
    let p = tmp.path().join("fixtures").join(name);
    fs::create_dir_all(&p).expect("fixture dir");
    fs::write(p.join("merged.ini"), "").expect("fixture ini");
    p
}

#[tokio::test]
async fn dos_reserved_names_are_prefixed_underscore() {
    let tmp = TempDir::new().expect("tmp");
    let (core, game_mods) = make_core_with(&tmp).await;

    // Try a representative sample: bare, lowercase, with extension.
    for (idx, raw) in ["CON", "prn", "AUX.skin", "NUL", "com1", "LPT9"]
        .into_iter()
        .enumerate()
    {
        let fx = make_fixture(&tmp, &format!("fx{idx}"));
        let m = core
            .adopt_folder(GameCode::Gimi, &fx, raw)
            .await
            .expect("adopt");
        core.set_enabled(&m.id, true, &game_mods)
            .await
            .expect("enable");
    }

    let names: Vec<String> = fs::read_dir(&game_mods)
        .expect("read mods")
        .filter_map(Result::ok)
        .map(|e| e.file_name().into_string().expect("utf8"))
        .collect();

    for n in &names {
        let upper = n.to_uppercase();
        let base = upper.split('.').next().unwrap_or(&upper);
        let reserved =
            matches!(base, "CON" | "PRN" | "AUX" | "NUL")
            || (base.starts_with("COM")
                && base.len() == 4
                && base.chars().nth(3).map(|c| c.is_ascii_digit() && c != '0').unwrap_or(false))
            || (base.starts_with("LPT")
                && base.len() == 4
                && base.chars().nth(3).map(|c| c.is_ascii_digit() && c != '0').unwrap_or(false));
        assert!(
            !reserved,
            "junction dir {n:?} is a reserved DOS device name — should have been prefixed",
        );
    }
}
