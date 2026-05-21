//! Slice 14: HTTP / SOCKS5 proxy settings.
//!
//! Roundtrip tests for the persisted proxy config, plus a safety
//! assertion that the diagnostics snapshot never includes the proxy
//! password.

use gmm_lib::core::diagnostics::SettingsSnapshot;
use gmm_lib::core::network::ProxyConfig;
use gmm_lib::core::Core;
use tempfile::TempDir;

async fn fresh_core(tmp: &TempDir) -> Core {
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    Core::new(library_root, &db_url).await.expect("init")
}

#[tokio::test]
async fn proxy_config_roundtrips_through_settings() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    // Default state: nothing set.
    let initial = core.proxy_config().await.expect("read");
    assert_eq!(initial, ProxyConfig::default());

    // Save full config.
    let cfg = ProxyConfig {
        url: Some("socks5://proxy.local:1080".into()),
        username: Some("derek".into()),
        password: Some("hunter2".into()),
    };
    core.set_proxy_config(&cfg).await.expect("save");

    let loaded = core.proxy_config().await.expect("reload");
    assert_eq!(loaded, cfg);

    // Public view hides the password but keeps the password_set flag.
    let public = core.proxy_config_public().await.expect("public");
    assert_eq!(public.url.as_deref(), Some("socks5://proxy.local:1080"));
    assert_eq!(public.username.as_deref(), Some("derek"));
    assert!(public.password_set);

    // Clear the password but keep URL+username.
    core.set_proxy_config(&ProxyConfig {
        url: cfg.url.clone(),
        username: cfg.username.clone(),
        password: None,
    })
    .await
    .expect("clear pw");
    let public = core.proxy_config_public().await.expect("public");
    assert!(
        !public.password_set,
        "password_set must follow the stored value"
    );
}

#[tokio::test]
async fn diagnostics_snapshot_never_includes_proxy_password() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    core.set_proxy_config(&ProxyConfig {
        url: Some("http://proxy.local:8080".into()),
        username: Some("user".into()),
        password: Some("hunter2".into()),
    })
    .await
    .expect("save");

    let snapshot = core.settings_snapshot().await.expect("snapshot");
    let json = serde_json::to_string(&snapshot.redacted()).expect("serialise");
    assert!(
        !json.contains("hunter2"),
        "password must never appear in the diagnostics bundle JSON: {json}",
    );
}

#[tokio::test]
async fn snapshot_redacts_userinfo_pasted_into_url() {
    // Defence-in-depth: the password lives in its own setting, but if a
    // user pastes `http://user:pass@host` into the URL field by mistake,
    // the snapshot serialiser still has to strip the userinfo.
    let snapshot = SettingsSnapshot {
        library_root: None,
        game_install_paths: Default::default(),
        proxy_url: Some("http://user:pass@proxy.local:8080".into()),
    };
    let redacted = snapshot.redacted();
    let url = redacted.proxy_url.unwrap();
    assert!(
        !url.contains("pass"),
        "userinfo must be stripped, got {url}",
    );
    assert!(url.contains("proxy.local:8080"), "host must remain: {url}");
    assert!(url.contains("REDACTED"));
}
