//! The application's native menu — File and Help (#351, first slice of #326).
//!
//! The menu is declared as data in [`menu_spec`], and [`build_menu`] turns
//! that declaration into real items. The split exists because a host test
//! cannot construct a real menu: `muda::Menu::new` panics off the platform's
//! main thread on macOS, needs an initialised GTK on Linux, and
//! `tauri::test::mock_app` never runs an event loop, so the `run_main_thread!`
//! hop inside every constructor has nothing to run on. The declaration is
//! therefore what the tests assert, and `build_menu` carries no label, id or
//! ordering of its own.
//!
//! Clicking is the only reliable way to reach these items on Windows.
//! WebView2 consumes key presses before the host window sees them, so menu
//! accelerators and Alt mnemonics are unreliable under Tauri 2
//! (tauri-apps/wry#451, tauri-apps/tauri#4085). The labels carry `&`
//! mnemonics for Windows to underline, no accelerator is declared, and no
//! keyboard workaround is attempted here. The limitation is recorded in
//! `docs/design/main-screen.md`.
//!
//! Only Settings, Check for updates, Run setup again and Diagnostics reach
//! React, and they reach it as a forwarded event on [`MENU_ACTION_EVENT`] —
//! the existing frontend handlers own the behaviour. Exit and About GMM are
//! predefined items the OS handles itself.

use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Runtime};

/// Channel a clicked menu item is forwarded to React on. The payload is the
/// item's id. Mirrored by `MENU_ACTION_EVENT` in `src/api.ts`.
pub const MENU_ACTION_EVENT: &str = "menu://action";

/// Open the Settings surface on its GMM page.
pub const SETTINGS_ACTION: &str = "settings";
/// Open the Settings surface on its GMM page, where `UpdatesPanel` lives.
pub const CHECK_UPDATES_ACTION: &str = "check-updates";
/// Re-run the onboarding wizard.
pub const RUN_SETUP_ACTION: &str = "run-setup";
/// Open the Settings surface on its GMM page, where `Diagnostics` lives.
pub const DIAGNOSTICS_ACTION: &str = "diagnostics";

/// One entry inside a menu, as declared by [`menu_spec`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// A GMM action: clicking it forwards `id` to React.
    Action {
        id: &'static str,
        label: &'static str,
    },
    /// A horizontal rule.
    Separator,
    /// `PredefinedMenuItem::quit` — the OS closes the application.
    Quit { label: &'static str },
    /// `PredefinedMenuItem::about` — the OS shows [`about_metadata`].
    About { label: &'static str },
}

/// One top-level menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuSpec {
    pub label: &'static str,
    pub entries: Vec<Entry>,
}

/// The decided menu layout. Changing it is a maintainer decision, not an
/// implementation detail: `tests/menu.rs` pins this list exactly.
pub fn menu_spec() -> Vec<MenuSpec> {
    vec![
        MenuSpec {
            label: "&File",
            entries: vec![
                Entry::Action {
                    id: SETTINGS_ACTION,
                    label: "Settings…",
                },
                Entry::Separator,
                Entry::Quit { label: "Exit" },
            ],
        },
        MenuSpec {
            label: "&Help",
            entries: vec![
                Entry::Action {
                    id: CHECK_UPDATES_ACTION,
                    label: "Check for updates…",
                },
                Entry::Action {
                    id: RUN_SETUP_ACTION,
                    label: "Run setup again",
                },
                Entry::Action {
                    id: DIAGNOSTICS_ACTION,
                    label: "Diagnostics…",
                },
                Entry::Separator,
                Entry::About { label: "About GMM" },
            ],
        },
    ]
}

/// What the OS About dialog reports.
///
/// The version comes from the binary's compiled Tauri package metadata —
/// the same single source `get_app_version` reads (#350). A literal here
/// would be a second version that drifts from the shipped build. The
/// remaining fields come from the crate manifest at compile time for the
/// same reason. `icon` and `credits` are left unset: Windows ignores both.
pub fn about_metadata<R: Runtime>(app: &AppHandle<R>) -> AboutMetadata<'static> {
    let package = app.package_info();
    AboutMetadata {
        name: Some(package.name.clone()),
        version: Some(package.version.to_string()),
        authors: Some(
            package
                .authors
                .split(':')
                .map(|author| author.trim().to_string())
                .collect(),
        ),
        license: Some(env!("CARGO_PKG_LICENSE").to_string()),
        website: Some(env!("CARGO_PKG_REPOSITORY").to_string()),
        ..Default::default()
    }
}

/// The React action a clicked item becomes, or `None` when the item is one
/// the OS handles itself (Exit, About) or an id from outside this menu.
///
/// Deliberately an explicit match rather than a lookup derived from
/// [`menu_spec`]: a new `Entry::Action` with no arm here would otherwise
/// build a menu item that silently does nothing. `tests/menu.rs` walks the
/// spec and fails until every action has a route.
pub fn menu_action(id: &str) -> Option<&'static str> {
    match id {
        SETTINGS_ACTION => Some(SETTINGS_ACTION),
        CHECK_UPDATES_ACTION => Some(CHECK_UPDATES_ACTION),
        RUN_SETUP_ACTION => Some(RUN_SETUP_ACTION),
        DIAGNOSTICS_ACTION => Some(DIAGNOSTICS_ACTION),
        _ => None,
    }
}

/// Forward a clicked menu item to React. Wired to
/// `tauri::Builder::on_menu_event`.
pub fn forward_menu_event<R: Runtime>(app: &AppHandle<R>, id: &str) {
    let Some(action) = menu_action(id) else {
        return;
    };
    if let Err(error) = app.emit(MENU_ACTION_EVENT, action) {
        tracing::warn!(
            target: "gmm::menu",
            error = %error,
            action,
            "could not forward a menu action to the frontend",
        );
    }
}

/// Build the menu described by [`menu_spec`].
///
/// No accelerator is passed: see the module comment. `enabled` is `true` for
/// every item — **Run setup again** is gated in React, which owns the reset
/// mutation's pending state.
pub fn build_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;
    for spec in menu_spec() {
        let submenu = Submenu::new(app, spec.label, true)?;
        for entry in spec.entries {
            match entry {
                Entry::Action { id, label } => {
                    submenu.append(&MenuItem::with_id(app, id, label, true, None::<&str>)?)?
                }
                Entry::Separator => submenu.append(&PredefinedMenuItem::separator(app)?)?,
                Entry::Quit { label } => {
                    submenu.append(&PredefinedMenuItem::quit(app, Some(label))?)?
                }
                Entry::About { label } => submenu.append(&PredefinedMenuItem::about(
                    app,
                    Some(label),
                    Some(about_metadata(app)),
                )?)?,
            }
        }
        menu.append(&submenu)?;
    }
    Ok(menu)
}
