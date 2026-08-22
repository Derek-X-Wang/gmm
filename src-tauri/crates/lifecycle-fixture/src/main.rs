//! Test-only GMM stand-in that writes, and later re-checks, the user
//! state an MSI upgrade must not disturb. Not shipped: `tauri build`
//! bundles only the `gmm` binary, and nothing in the app depends on
//! this crate.
//!
//! `installer-smoke.ps1` covers install → launch → uninstall on a clean
//! machine. Upgrade is the path every *existing* user takes and was
//! untested (#57). The hard part of testing it is not running `msiexec`
//! twice — it is having something real to preserve. A canary text file
//! proves almost nothing: it is not written through GMM's own code, it
//! is not in the database, and it is not a Junction.
//!
//! So this seeds the four things that actually matter, through `Core`'s
//! own API, so what is asserted is the state GMM really produces:
//!
//! - a **Library** entry holding a Mod's files
//! - a **Mod** row, enabled
//! - a live **Junction** from the game's `Mods/` directory into that
//!   Library path — the thing that makes an enabled Mod load
//! - an **Importer Pin**, which lives in the settings table. ADR 0004
//!   makes the pin the escape hatch during ban-wave windows, so losing
//!   it across an upgrade is an account-safety regression, not a
//!   cosmetic one. It survives if and only if `gmm.db` survives, and
//!   that deserves its own assertion rather than riding on a generic
//!   "app data preserved" check.
//!
//! ```text
//! lifecycle-fixture [--data-dir D] [--game-dir G] seed
//! lifecycle-fixture [--data-dir D] [--game-dir G] verify
//! ```
//!
//! `--data-dir` defaults to the same `%APPDATA%\GMM` the installed app
//! resolves, via GMM's own `data_dir()`, so the fixture and the app
//! cannot disagree about where the user's state lives.
//!
//! Exit status: 0 when everything held, 2 when a check failed, 1 on a
//! usage error. `verify` reports **every** failure rather than stopping
//! at the first, because a CI log a Windows-less maintainer reads once
//! should say everything that broke.

use std::path::{Path, PathBuf};

use gmm_lib::core::{Core, GameCode};

/// The game the fixture uses. Any one would do; Genshin is the one with
/// the longest-lived Model Importer, so its profile is least likely to
/// be the thing that changes underneath this test.
const GAME: GameCode = GameCode::Gimi;

const MOD_NAME: &str = "Lifecycle Fixture Mod";
const MARKER_FILE: &str = "lifecycle-fixture.txt";
const MARKER_TEXT: &str = "this Mod's files must survive an MSI upgrade";

/// Deliberately not a plausible real version. If this string ever shows
/// up in a bug report it came from CI, not from a user.
const PIN_VERSION: &str = "9.9.9-fixture-pin";

fn main() {
    let args = match Args::parse() {
        Ok(args) => args,
        Err(msg) => {
            eprintln!("usage error: {msg}");
            eprintln!("usage: lifecycle-fixture [--data-dir D] [--game-dir G] <seed|verify>");
            std::process::exit(1);
        }
    };

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("could not build a tokio runtime: {e}");
            std::process::exit(1);
        }
    };

    let code = rt.block_on(async move {
        match args.op {
            Op::Seed => match seed(&args).await {
                Ok(report) => {
                    println!("{report}");
                    0
                }
                Err(e) => {
                    eprintln!("seed failed: {e}");
                    2
                }
            },
            Op::Verify => match verify(&args).await {
                Ok(failures) if failures.is_empty() => {
                    println!("all seeded state survived");
                    0
                }
                Ok(failures) => {
                    for f in &failures {
                        eprintln!("FAIL: {f}");
                    }
                    eprintln!(
                        "{} of the seeded invariants did not survive",
                        failures.len()
                    );
                    2
                }
                Err(e) => {
                    eprintln!("verify could not run: {e}");
                    2
                }
            },
        }
    });

    std::process::exit(code);
}

async fn open_core(args: &Args) -> Result<Core, Box<dyn std::error::Error>> {
    // Exactly what `build_core` in the app does, so the fixture and the
    // installed binary are looking at one database and one Library.
    let library_root = args.data_dir.join("library");
    let db_path = args.data_dir.join("gmm.db");
    let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
    Ok(Core::new(library_root, &db_url).await?)
}

fn mods_dir(args: &Args) -> PathBuf {
    args.game_dir.join("Mods")
}

async fn seed(args: &Args) -> Result<String, Box<dyn std::error::Error>> {
    let core = open_core(args).await?;

    // A staging tree standing in for an extracted download. `adopt_folder`
    // copies it into the Library and records the Mod, which is the same
    // path a user's "adopt this folder" takes.
    let staging = args.data_dir.join("lifecycle-fixture-staging");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    std::fs::write(staging.join(MARKER_FILE), MARKER_TEXT)?;
    std::fs::write(
        staging.join("mod.ini"),
        "; a stand-in for a real 3dmigoto mod definition\n[TextureOverrideFixture]\nhash = deadbeef\n",
    )?;

    std::fs::create_dir_all(mods_dir(args))?;
    core.set_game_install_path(GAME, &args.game_dir).await?;

    let adopted = core.adopt_folder(GAME, &staging, MOD_NAME).await?;
    core.set_enabled(&adopted.id, true, &mods_dir(args)).await?;
    core.set_importer_pinned(GAME, Some(PIN_VERSION)).await?;

    // The staging tree has served its purpose; leaving it behind would
    // make "the Library still has the files" ambiguous.
    let _ = std::fs::remove_dir_all(&staging);

    Ok(serde_json::json!({
        "modId": adopted.id,
        "libraryPath": adopted.library_path,
        "junctions": junction_links(args)?
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>(),
        "pin": PIN_VERSION,
    })
    .to_string())
}

/// Re-check everything `seed` established. Returns the list of things
/// that did not survive; empty means the upgrade was clean.
async fn verify(args: &Args) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let core = open_core(args).await?;
    let mut failures = Vec::new();

    let mods = core.list_mods(GAME).await?;
    let Some(seeded) = mods.iter().find(|m| m.name == MOD_NAME) else {
        failures.push(format!(
            "the seeded Mod {MOD_NAME:?} is gone from the database — \
             found {:?}",
            mods.iter().map(|m| &m.name).collect::<Vec<_>>()
        ));
        // Everything below is about that Mod, so there is nothing
        // further to say.
        return Ok(failures);
    };

    if !seeded.enabled {
        failures.push(
            "the seeded Mod is no longer enabled — an upgrade must not \
             silently disable a user's Mods"
                .to_string(),
        );
    }

    // The Library copy: the source of truth per ADR 0003.
    let library_marker = Path::new(&seeded.library_path).join(MARKER_FILE);
    match std::fs::read_to_string(&library_marker) {
        Ok(text) if text == MARKER_TEXT => {}
        Ok(_) => failures.push(format!(
            "the Library copy at {} was rewritten",
            library_marker.display()
        )),
        Err(e) => failures.push(format!(
            "the Library copy at {} is unreadable: {e}",
            library_marker.display()
        )),
    }

    // The Junction: the thing that makes an enabled Mod actually load.
    //
    // Checked by reading *through* it rather than by `exists()`. A
    // directory that is still there but no longer points at the Library
    // passes an existence check and loads nothing — which is precisely
    // the failure an upgrade could introduce.
    //
    // The link is found by enumerating the game's `Mods/` directory
    // rather than by recomputing its name. That is deliberate: the name
    // is derived and de-duplicated inside `Core`, so recomputing it here
    // would be a second implementation of a rule that can drift. It also
    // makes the count assertable, which is worth having — one enabled
    // Mod must mean exactly one link.
    match junction_links(args) {
        Ok(links) if links.len() == 1 => {
            let through = links[0].join(MARKER_FILE);
            match std::fs::read_to_string(&through) {
                Ok(text) if text == MARKER_TEXT => {}
                Ok(_) => failures.push(format!(
                    "reading through the Junction at {} gave different bytes \
                     than the Library holds",
                    links[0].display()
                )),
                Err(e) => failures.push(format!(
                    "the Junction at {} no longer resolves to the Library: {e}",
                    links[0].display()
                )),
            }
        }
        Ok(links) => failures.push(format!(
            "expected exactly one Junction in {}, found {}: {:?}",
            mods_dir(args).display(),
            links.len(),
            links
        )),
        Err(e) => failures.push(format!("could not read {}: {e}", mods_dir(args).display())),
    }

    // The Importer Pin. ADR 0004 makes this the escape hatch during a
    // ban-wave window, so losing it is an account-safety regression.
    match core.importer_pinned(GAME).await? {
        Some(v) if v == PIN_VERSION => {}
        other => failures.push(format!(
            "the Importer Pin did not survive: expected {PIN_VERSION:?}, \
             found {other:?}. Per ADR 0004 the pin is the only escape \
             hatch when a new Model Importer breaks mods or trips \
             detection, so losing it silently is an account-safety \
             regression"
        )),
    }

    match core.game_install_path(GAME).await? {
        Some(p) if p == args.game_dir => {}
        other => failures.push(format!(
            "the recorded game install path changed: expected {}, found {other:?}",
            args.game_dir.display()
        )),
    }

    Ok(failures)
}

/// Everything sitting in the game's `Mods/` directory.
///
/// For GMM these are Junctions, but the fixture deliberately does not
/// check the reparse point: what matters to a user is whether the files
/// are reachable through the path, and that is asserted by reading
/// through it.
fn junction_links(args: &Args) -> std::io::Result<Vec<PathBuf>> {
    let mut links: Vec<PathBuf> = std::fs::read_dir(mods_dir(args))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    links.sort();
    Ok(links)
}

enum Op {
    Seed,
    Verify,
}

struct Args {
    data_dir: PathBuf,
    game_dir: PathBuf,
    op: Op,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut data_dir: Option<PathBuf> = None;
        let mut game_dir: Option<PathBuf> = None;
        let mut op: Option<Op> = None;

        let mut it = std::env::args().skip(1);
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--data-dir" => {
                    data_dir = Some(PathBuf::from(it.next().ok_or("--data-dir needs a value")?))
                }
                "--game-dir" => {
                    game_dir = Some(PathBuf::from(it.next().ok_or("--game-dir needs a value")?))
                }
                "seed" => op = Some(Op::Seed),
                "verify" => op = Some(Op::Verify),
                other => return Err(format!("unrecognised argument {other:?}")),
            }
        }

        Ok(Self {
            // Defaulting through GMM's own resolver is what stops the
            // fixture and the installed app disagreeing about where the
            // user's state lives.
            data_dir: match data_dir {
                Some(d) => d,
                None => gmm_lib::data_dir().map_err(|e| e.to_string())?,
            },
            game_dir: game_dir.ok_or("--game-dir is required")?,
            op: op.ok_or("expected one of: seed, verify")?,
        })
    }
}
