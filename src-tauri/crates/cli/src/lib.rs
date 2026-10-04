//! JSON CLI for the bounded discovery-and-apply workflow.
//! `run` accepts arguments without the executable name. Exit codes follow the
//! probe: 0 success, 1 usage error, 2 operation failure (including refusal).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gmm_lib::command_error::{CommandError, CommandResult};
use gmm_lib::core::{
    gamebanana, games::GAME_PROFILES, instance_lock, Core, GameCode, ImportZipOptions, Mod,
};
use gmm_lib::runtime::{
    launch::{launch_headless, LaunchOptions},
    SessionRuntime,
};
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Serialize)]
pub struct Outcome {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<CommandError>,
    #[serde(skip)]
    pub exit_code: i32,
}

impl Outcome {
    fn success(result: Value) -> Self {
        Self {
            ok: true,
            result: Some(result),
            error: None,
            exit_code: 0,
        }
    }
    fn failure(error: CommandError, exit_code: i32) -> Self {
        Self {
            ok: false,
            result: None,
            error: Some(error),
            exit_code,
        }
    }
}

struct Args {
    command: String,
    values: HashMap<String, String>,
    data_dir: PathBuf,
    dry_run: bool,
    confirm: bool,
    allow_attention: bool,
}

impl Args {
    fn parse(argv: Vec<String>) -> Result<Self, String> {
        let mut values = HashMap::new();
        let mut command = None;
        let mut dry_run = false;
        let mut confirm = false;
        let mut allow_attention = false;
        let mut args = argv.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--dry-run" | "--confirm" | "--allow-attention" => {
                    let flag = match arg.as_str() {
                        "--dry-run" => &mut dry_run,
                        "--confirm" => &mut confirm,
                        _ => &mut allow_attention,
                    };
                    if *flag {
                        return Err(format!("duplicate flag {arg}"));
                    }
                    *flag = true;
                }
                _ if arg.starts_with("--") => {
                    let value = args
                        .next()
                        .filter(|value| !value.starts_with("--"))
                        .ok_or_else(|| format!("{arg} requires a value"))?;
                    if values.insert(arg.clone(), value).is_some() {
                        return Err(format!("duplicate option {arg}"));
                    }
                }
                _ => {
                    if command.replace(arg).is_some() {
                        return Err("unexpected positional argument".into());
                    }
                }
            }
        }
        let command = command.ok_or("a command is required; see docs/cli.md")?;
        let required: &[&str] = match command.as_str() {
            "games" | "mods" | "status" => &[],
            "importer" | "conflicts" | "launch" => &["--game"],
            "variants" | "enable" | "disable" => &["--mod-id"],
            "set-variant" => &["--mod-id", "--variant-id"],
            "import" => &["--game", "--source"],
            "import-zip" => &["--game", "--archive", "--name"],
            "adopt" => &["--game", "--from", "--name"],
            _ => return Err(format!("unknown command {command}")),
        };
        for key in required {
            if values.get(*key).is_none_or(|value| value.trim().is_empty()) {
                return Err(format!("{command} requires {key}"));
            }
        }
        for key in values.keys() {
            if key != "--data-dir"
                && !required.contains(&key.as_str())
                && !(command == "mods" && key == "--game")
            {
                return Err(format!("{command} does not accept {key}"));
            }
        }
        if let Some(game) = values.get("--game") {
            game.parse::<GameCode>().map_err(|e| e.to_string())?;
        }
        if command == "import" && gamebanana::parse_url_or_id(&values["--source"]).is_none() {
            return Err("--source must be a GameBanana Mod URL or submission ID".into());
        }
        let changing = matches!(
            command.as_str(),
            "import" | "import-zip" | "adopt" | "enable" | "disable" | "set-variant" | "launch"
        );
        if !changing && (dry_run || confirm || allow_attention) {
            return Err(
                "--dry-run, --confirm and --allow-attention apply only to state-changing commands"
                    .into(),
            );
        }
        let data_dir = match values.remove("--data-dir") {
            Some(path) if !path.is_empty() => PathBuf::from(path),
            Some(_) => return Err("--data-dir must not be empty".into()),
            None => dirs::data_dir()
                .ok_or("could not resolve OS data directory")?
                .join("GMM"),
        };
        Ok(Self {
            command,
            values,
            data_dir,
            dry_run,
            confirm,
            allow_attention,
        })
    }

    fn changing(&self) -> bool {
        matches!(
            self.command.as_str(),
            "import" | "import-zip" | "adopt" | "enable" | "disable" | "set-variant" | "launch"
        )
    }
    fn writes_game(&self) -> bool {
        // Variant selection can retarget an enabled Mod's Junction.
        matches!(self.command.as_str(), "enable" | "disable" | "set-variant")
    }
    fn value(&self, key: &str) -> &str {
        &self.values[key]
    }
    fn game(&self) -> GameCode {
        self.value("--game").parse().expect("validated game")
    }
}

/// Parse, acquire the instance lock, perform one bounded operation, and map
/// typed failures. All invocations acquire the lock before opening the database;
/// the lock is held through pool close, and through the entire launched session.
pub async fn run(argv: Vec<String>) -> Outcome {
    let args = match Args::parse(argv) {
        Ok(args) => args,
        Err(message) => return Outcome::failure(CommandError::other(message), 1),
    };
    let _lock = match instance_lock::acquire(&args.data_dir) {
        Ok(lock) => lock,
        Err(error) => return Outcome::failure(error.into(), 2),
    };
    if args.writes_game() && !args.dry_run && !args.confirm {
        return Outcome::failure(CommandError::other("This command writes inside the Game directory. Review --dry-run, then pass --confirm."), 2);
    }
    let library = args.data_dir.join("library");
    let db_url = format!("sqlite://{}", args.data_dir.join("gmm.db").display());
    let core = if args.changing() && !args.dry_run {
        Core::new_without_recovery(library, &db_url).await
    } else {
        Core::for_inspection(library, &db_url).await
    };
    let core = match core {
        Ok(core) => core,
        Err(error) => return Outcome::failure(error.into(), 2),
    };
    let result = execute(&core, &args).await;
    core.close().await;
    match result {
        Ok(result) => Outcome::success(result),
        Err(error) => Outcome::failure(error, 2),
    }
}

async fn find_mod(core: &Core, id: &str) -> CommandResult<Mod> {
    for profile in GAME_PROFILES {
        if let Some(item) = core
            .list_mods(profile.code)
            .await?
            .into_iter()
            .find(|item| item.id == id)
        {
            return Ok(item);
        }
    }
    Err(CommandError::other(format!("Mod {id} does not exist")))
}

async fn mods_dir(core: &Core, game: GameCode) -> CommandResult<PathBuf> {
    core.game_install_path(game)
        .await?
        .map(|path| path.join("Mods"))
        .ok_or_else(|| {
            CommandError::other(
                "Set the game install path in GMM before changing deployment or launching.",
            )
        })
}

async fn execute(core: &Core, args: &Args) -> CommandResult<Value> {
    if args.changing()
        && !args.dry_run
        && !args.allow_attention
        && !core.attention_status().await?.safe_to_proceed
    {
        return Err(CommandError::other("GMM needs attention. Inspect status and resolve it in GMM, or explicitly pass --allow-attention after reviewing it. Core safety guards still apply."));
    }
    if args.dry_run {
        return dry_run(core, args).await;
    }
    match args.command.as_str() {
        "games" => {
            let mut games = Vec::new();
            for profile in GAME_PROFILES {
                games.push(
                    json!({"code": profile.code, "displayName": profile.display_name,
                    "detectedPath": profile.detect.and_then(|detect| detect()),
                    "installPath": core.game_install_path(profile.code).await?}),
                );
            }
            Ok(json!({"games": games}))
        }
        "mods" => {
            let mut mods = Vec::new();
            for profile in GAME_PROFILES {
                if args
                    .values
                    .get("--game")
                    .is_some_and(|game| game != profile.code.as_str())
                {
                    continue;
                }
                for item in core.list_mods(profile.code).await? {
                    mods.push(
                        json!({"mod": item, "variants": core.list_variants(&item.id).await?,
                        "activeVariantId": core.active_variant_id(&item.id).await?}),
                    );
                }
            }
            Ok(json!({"mods": mods}))
        }
        "importer" => Ok(json!({"game": args.game(),
            "installedVersion": core.installed_importer_version(args.game()).await?,
            "installedOrigin": core.installed_importer_origin(args.game()).await?,
            "pinnedVersion": core.importer_pinned(args.game()).await?})),
        "status" => {
            Ok(serde_json::to_value(core.attention_status().await?).expect("serialize status"))
        }
        "conflicts" => Ok(
            serde_json::to_value(core.detect_conflicts(args.game()).await?)
                .expect("serialize conflicts"),
        ),
        "variants" => {
            let item = find_mod(core, args.value("--mod-id")).await?;
            Ok(
                json!({"modId": item.id, "variants": core.list_variants(&item.id).await?, "activeVariantId": core.active_variant_id(&item.id).await?}),
            )
        }
        "adopt" => Ok(
            json!({"mod": core.adopt_folder(args.game(), Path::new(args.value("--from")), args.value("--name")).await?}),
        ),
        "import-zip" => Ok(
            json!({"mod": core.import_zip(args.game(), Path::new(args.value("--archive")), args.value("--name"), ImportZipOptions::default()).await?}),
        ),
        "import" => {
            Ok(json!({"mod": core.import_gamebanana(args.game(), args.value("--source")).await?}))
        }
        "enable" | "disable" | "set-variant" => {
            let item = find_mod(core, args.value("--mod-id")).await?;
            let dir = mods_dir(core, item.game).await?;
            if args.command == "set-variant" {
                core.set_active_variant(&item.id, args.value("--variant-id"), &dir)
                    .await?;
            } else {
                std::fs::create_dir_all(&dir).map_err(|error| {
                    CommandError::other(format!("create {}: {error}", dir.display()))
                })?;
                core.set_enabled(&item.id, args.command == "enable", &dir)
                    .await?;
            }
            Ok(json!({"mod": find_mod(core, &item.id).await?}))
        }
        "launch" => {
            let runtime = SessionRuntime::new();
            let outcome =
                launch_headless(core, &runtime, args.game(), &LaunchOptions::default()).await?;
            // The process owns the Loader and instance lock until the Game exits.
            outcome.watcher.await.map_err(|error| {
                CommandError::other(format!("Game Session watcher failed: {error}"))
            })?;
            Ok(json!({"session": outcome.info, "ended": true}))
        }
        _ => unreachable!("validated command"),
    }
}

async fn dry_run(core: &Core, args: &Args) -> CommandResult<Value> {
    let effects = match args.command.as_str() {
        "adopt" | "import-zip" | "import" => {
            let root = core.resolved_library_root_for(args.game()).await?;
            match args.command.as_str() {
                "adopt" => {
                    let from = Path::new(args.value("--from"));
                    let metadata =
                        std::fs::metadata(from).map_err(|source| gmm_lib::core::Error::Io {
                            path: from.to_path_buf(),
                            source,
                        })?;
                    if !metadata.is_dir() {
                        return Err(CommandError::other("--from must name an existing folder"));
                    }
                    json!({"action": "copyFolderToLibrary", "from": from, "libraryRoot": root, "name": args.value("--name")})
                }
                "import-zip" => {
                    let archive = Path::new(args.value("--archive"));
                    let metadata =
                        std::fs::metadata(archive).map_err(|source| gmm_lib::core::Error::Io {
                            path: archive.to_path_buf(),
                            source,
                        })?;
                    if !metadata.is_file() {
                        return Err(CommandError::other("--archive must name an existing ZIP"));
                    }
                    json!({"action": "extractArchiveToLibrary", "archive": archive, "libraryRoot": root, "name": args.value("--name")})
                }
                _ => {
                    json!({"action": "downloadAndImportToLibrary", "submissionId": gamebanana::parse_url_or_id(args.value("--source")), "libraryRoot": root})
                }
            }
        }
        "enable" | "disable" | "set-variant" => {
            let item = find_mod(core, args.value("--mod-id")).await?;
            let dir = mods_dir(core, item.game).await?;
            let (junction_path, target) = core
                .mod_deployment_paths(
                    &item.id,
                    &dir,
                    args.values.get("--variant-id").map(String::as_str),
                )
                .await?;
            json!({"action": args.command, "modId": item.id, "game": item.game,
                "junctionPath": junction_path, "libraryTarget": target,
                "variantId": args.values.get("--variant-id"), "wasEnabled": item.enabled})
        }
        "launch" => {
            json!({"action": "launchGameSession", "game": args.game(), "installPath": mods_dir(core, args.game()).await?.parent(), "holdsLockUntilGameExits": true})
        }
        _ => unreachable!("dry run is limited to mutations"),
    };
    Ok(
        json!({"dryRun": true, "command": args.command, "requiresConfirmation": args.writes_game(),
        "effects": effects, "attention": core.attention_status().await?}),
    )
}
