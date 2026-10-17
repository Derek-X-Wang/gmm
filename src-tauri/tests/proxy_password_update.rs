//! Production proxy command body: wire intents, atomic writes, and secret boundaries.
use gmm_lib::commands::ProxyArgs;
use gmm_lib::core::network::ProxyConfig;
use gmm_lib::core::Core;
use serde_json::{json, Value};
use tempfile::TempDir;

const SECRET: &str = "PLACEHOLDER_PROXY_PASSWORD";

async fn seeded_core(tmp: &TempDir) -> Core {
    let db = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(tmp.path().join("library"), &db).await.unwrap();
    core.set_proxy_config(&ProxyConfig {
        url: Some("http://previous.invalid:8080".into()),
        username: Some("PLACEHOLDER_USERNAME".into()),
        password: Some(SECRET.into()),
    })
    .await
    .unwrap();
    core
}

fn args(password: Value, clear_password: bool) -> ProxyArgs {
    serde_json::from_value(json!({
        "url": "http://replacement.invalid:8080",
        "username": "PLACEHOLDER_USERNAME",
        "password": password,
        "clearPassword": clear_password,
    }))
    .unwrap()
}

#[tokio::test]
async fn blank_password_url_edit_preserves_saved_password() {
    for blank in [Value::Null, json!("")] {
        let tmp = TempDir::new().unwrap();
        let core = seeded_core(&tmp).await;
        let public = args(blank, false).save(&core).await.unwrap();
        assert!(
            public.password_set,
            "blank password must keep the saved password"
        );
        assert_eq!(
            public.url.as_deref(),
            Some("http://replacement.invalid:8080")
        );
        assert_eq!(
            core.proxy_config().await.unwrap().password.as_deref(),
            Some(SECRET)
        );
    }
}

#[tokio::test]
async fn explicit_password_removal_clears_saved_password() {
    for blank in [Value::Null, json!("")] {
        let tmp = TempDir::new().unwrap();
        let core = seeded_core(&tmp).await;
        let public = args(blank, true).save(&core).await.unwrap();
        assert!(
            !public.password_set,
            "explicit removal must clear the saved password"
        );
        assert_eq!(core.proxy_config().await.unwrap().password, None);
    }
}

#[tokio::test]
async fn nonempty_password_sets_a_new_saved_value() {
    let tmp = TempDir::new().unwrap();
    let core = seeded_core(&tmp).await;
    let public = args(json!("PLACEHOLDER_REPLACEMENT_PASSWORD"), false)
        .save(&core)
        .await
        .unwrap();
    assert!(public.password_set);
    assert_eq!(
        core.proxy_config().await.unwrap().password.as_deref(),
        Some("PLACEHOLDER_REPLACEMENT_PASSWORD")
    );
}

#[tokio::test]
async fn conflicting_password_intents_reject_without_writing_or_disclosing() {
    let tmp = TempDir::new().unwrap();
    let core = seeded_core(&tmp).await;
    let previous = core.proxy_config().await.unwrap();
    let error = args(json!("PLACEHOLDER_REPLACEMENT_PASSWORD"), true)
        .save(&core)
        .await
        .unwrap_err();
    assert_eq!(
        error.message,
        "Cannot set and remove a proxy password in the same save."
    );
    assert_eq!(core.proxy_config().await.unwrap(), previous);
}

#[tokio::test]
async fn proxy_public_response_never_exposes_password() {
    let tmp = TempDir::new().unwrap();
    let core = seeded_core(&tmp).await;
    let saved = args(Value::Null, false).save(&core).await.unwrap();
    for public in [saved, core.proxy_config_public().await.unwrap()] {
        let value = serde_json::to_value(public).unwrap();
        assert_eq!(
            value,
            json!({
                "url": "http://replacement.invalid:8080",
                "username": "PLACEHOLDER_USERNAME",
                "passwordSet": true,
            }),
            "public response must contain only URL, username, and passwordSet"
        );
        assert!(!value.to_string().contains(SECRET));
    }
    let snapshot = core.settings_snapshot().await.unwrap();
    assert!(!serde_json::to_string(&snapshot.redacted())
        .unwrap()
        .contains(SECRET));
}

#[tokio::test]
async fn failed_intent_save_preserves_url_and_credentials() {
    for failing_key in ["network.proxy.username", "network.proxy.password"] {
        for (password, clear_password) in [
            (Value::Null, false),
            (Value::Null, true),
            (json!("PLACEHOLDER_REPLACEMENT_PASSWORD"), false),
        ] {
            if failing_key == "network.proxy.password" && password.is_null() && !clear_password {
                continue; // Keep intentionally does not write this key.
            }
            let tmp = TempDir::new().unwrap();
            let core = seeded_core(&tmp).await;
            let previous = core.proxy_config().await.unwrap();
            let db = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
            let pool = sqlx::SqlitePool::connect(&db).await.unwrap();
            sqlx::query(&format!("CREATE TRIGGER reject_proxy_update BEFORE INSERT ON settings WHEN NEW.key = '{failing_key}' BEGIN SELECT RAISE(ABORT, 'injected proxy update failure'); END"))
                .execute(&pool).await.unwrap();
            pool.close().await;
            let error = args(password, clear_password)
                .save(&core)
                .await
                .unwrap_err();
            assert!(error.message.contains("injected proxy update failure"));
            assert!(!error.message.contains(SECRET));
            assert_eq!(core.proxy_config().await.unwrap(), previous);
        }
    }
}

#[tokio::test]
async fn omitted_clear_flag_keeps_saved_password_for_existing_callers() {
    let tmp = TempDir::new().unwrap();
    let core = seeded_core(&tmp).await;
    let args: ProxyArgs = serde_json::from_value(json!({
        "url": "http://replacement.invalid:8080",
        "username": "PLACEHOLDER_USERNAME",
        "password": null,
    }))
    .unwrap();
    assert!(args.save(&core).await.unwrap().password_set);
    assert_eq!(
        core.proxy_config().await.unwrap().password.as_deref(),
        Some(SECRET)
    );
}

#[tokio::test]
async fn keep_leaves_the_stored_password_key_untouched() {
    let tmp = TempDir::new().unwrap();
    let core = seeded_core(&tmp).await;
    let db = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let pool = sqlx::SqlitePool::connect(&db).await.unwrap();
    sqlx::query("CREATE TRIGGER reject_password_write BEFORE INSERT ON settings WHEN NEW.key = 'network.proxy.password' BEGIN SELECT RAISE(ABORT, 'password key must remain untouched'); END")
        .execute(&pool).await.unwrap();
    pool.close().await;
    assert!(
        args(Value::Null, false)
            .save(&core)
            .await
            .unwrap()
            .password_set
    );
    assert_eq!(
        core.proxy_config().await.unwrap().password.as_deref(),
        Some(SECRET)
    );
}
