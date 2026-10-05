fn main() {
    println!("cargo:rerun-if-env-changed=TAURI_CONFIG");
    println!("cargo:rerun-if-changed=../../tauri.conf.json");

    // Version metadata only: no GUI runtime or Common-Controls manifest.
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_app_version();
    }
}

#[cfg(windows)]
fn embed_app_version() {
    let base: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string("../../tauri.conf.json").expect("read app config"),
    )
    .expect("parse app config");
    let override_config: serde_json::Value =
        serde_json::from_str(&std::env::var("TAURI_CONFIG").unwrap_or_else(|_| "{}".into()))
            .expect("parse Tauri build override");
    let version = override_config
        .get("version")
        .or_else(|| base.get("version"))
        .and_then(serde_json::Value::as_str)
        .expect("app version must be a string");
    let components: Vec<u16> = version
        .split(['-', '+'])
        .next()
        .expect("version core")
        .split('.')
        .map(|part| part.parse().expect("numeric app version component"))
        .collect();
    assert_eq!(
        components.len(),
        3,
        "app version must have three components"
    );
    let numeric_version = (u64::from(components[0]) << 48)
        | (u64::from(components[1]) << 32)
        | (u64::from(components[2]) << 16);
    tauri_winres::WindowsResource::new()
        .set("ProductName", "GMM CLI")
        .set("OriginalFilename", "gmm-cli.exe")
        .set("FileVersion", version)
        .set("ProductVersion", env!("CARGO_PKG_VERSION"))
        .set_version_info(tauri_winres::VersionInfo::FILEVERSION, numeric_version)
        .set_version_info(tauri_winres::VersionInfo::PRODUCTVERSION, numeric_version)
        .compile()
        .expect("embed app version in CLI");
}
