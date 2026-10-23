//! Inspector width is a CSS-pixel preference, independent of native geometry.
use gmm_lib::core::{settings, Core};
use tempfile::TempDir;

async fn open_core(tmp: &TempDir) -> Core {
    Core::new(
        tmp.path().join("library"),
        &format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display()),
    )
    .await
    .expect("open core")
}

#[tokio::test]
async fn width_survives_reopening_without_touching_geometry() {
    let tmp = TempDir::new().unwrap();
    let core = open_core(&tmp).await;
    assert_eq!(core.mod_panel_width().await.unwrap(), None);
    let pool = sqlx::SqlitePool::connect(&format!(
        "sqlite://{}/gmm.db?mode=rwc",
        tmp.path().display()
    ))
    .await
    .unwrap();
    settings::put(&pool, "window.mainGeometry", Some("geometry sentinel"))
        .await
        .unwrap();
    core.set_mod_panel_width(460).await.unwrap();
    assert_eq!(
        settings::get(&pool, "mods.panelWidthCssPx").await.unwrap(),
        Some("460".to_string())
    );
    drop(core);
    let reopened = open_core(&tmp).await;
    assert_eq!(reopened.mod_panel_width().await.unwrap(), Some(460));
    assert_eq!(
        reopened.remembered_window_state().await.unwrap(),
        Some("geometry sentinel".to_string())
    );
    pool.close().await;
}

#[tokio::test]
async fn implausible_or_corrupt_width_is_absent_not_zero() {
    let tmp = TempDir::new().unwrap();
    let core = open_core(&tmp).await;
    let pool = sqlx::SqlitePool::connect(&format!(
        "sqlite://{}/gmm.db?mode=rwc",
        tmp.path().display()
    ))
    .await
    .unwrap();
    for invalid in ["", "0", "239", "561", "-1", "320.5", "null", "garbage"] {
        settings::put(&pool, "mods.panelWidthCssPx", Some(invalid))
            .await
            .unwrap();
        assert_eq!(core.mod_panel_width().await.unwrap(), None, "{invalid:?}");
    }
    pool.close().await;
}

#[tokio::test]
async fn requested_width_is_bounded_in_css_pixels() {
    let tmp = TempDir::new().unwrap();
    let core = open_core(&tmp).await;
    for (requested, expected) in [
        (0, 240),
        (240, 240),
        (320, 320),
        (560, 560),
        (u32::MAX, 560),
    ] {
        core.set_mod_panel_width(requested).await.unwrap();
        assert_eq!(core.mod_panel_width().await.unwrap(), Some(expected));
    }
}
