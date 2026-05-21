pub mod commands;
pub mod core;

use crate::core::Core;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let core = build_core().expect("initialise GMM core");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(core)
        .invoke_handler(tauri::generate_handler![
            commands::list_mods,
            commands::adopt_folder,
            commands::import_zip,
            commands::set_mod_enabled,
            commands::get_game_install_path,
            commands::set_game_install_path,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Build the GMM Core against the user's app-data directory. Synchronous
/// wrapper around the async constructor so it fits into Tauri's startup.
fn build_core() -> Result<Core, Box<dyn std::error::Error>> {
    let data_dir = dirs::data_dir()
        .ok_or("could not resolve OS data directory")?
        .join("GMM");
    std::fs::create_dir_all(&data_dir)?;

    let library_root = data_dir.join("library");
    let db_path = data_dir.join("gmm.db");
    let db_url = format!("sqlite://{}?mode=rwc", db_path.display());

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let core = rt.block_on(Core::new(library_root, &db_url))?;
    // Leak the runtime so it stays alive — Core's sqlx pool needs it for
    // future async calls invoked from Tauri commands.
    Box::leak(Box::new(rt));
    Ok(core)
}
