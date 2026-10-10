use std::fs;
use std::path::Path;

use gmm_lib::core::GameCode;

/// Give backend fixtures the game layout required by the saved-path boundary.
/// Preserve executable bytes supplied by native launch tests.
pub fn seed_game_install(game: GameCode, path: &Path) -> &Path {
    let (exe, data) = match game {
        GameCode::Gimi => ("GenshinImpact.exe", path.join("GenshinImpact_Data")),
        GameCode::Srmi => ("StarRail.exe", path.join("StarRail_Data")),
        GameCode::Zzmi => ("ZenlessZoneZero.exe", path.join("ZenlessZoneZero_Data")),
        GameCode::Himi => ("BH3.exe", path.join("BH3_Data")),
        GameCode::Wwmi => (
            "Client-Win64-Shipping.exe",
            path.parent().unwrap().parent().unwrap().join("Content"),
        ),
        GameCode::Efmi => (
            "Endfield-Win64-Shipping.exe",
            path.parent().unwrap().parent().unwrap().join("Content"),
        ),
    };
    fs::create_dir_all(path).expect("fixture game directory");
    fs::create_dir_all(data).expect("fixture game data directory");
    let executable = path.join(exe);
    if !executable.exists() {
        fs::write(executable, b"fixture game").expect("fixture game executable");
    }
    path
}
