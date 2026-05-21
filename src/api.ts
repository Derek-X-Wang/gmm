import { invoke } from "@tauri-apps/api/core";

export type GameCode = "gimi" | "srmi" | "zzmi" | "wwmi" | "himi" | "efmi";

export type Source = "manual" | "local" | "gamebanana";

export interface Mod {
  id: string;
  game: GameCode;
  name: string;
  source: Source;
  libraryPath: string;
  enabled: boolean;
}

interface RawMod {
  id: string;
  game: GameCode;
  name: string;
  source: Source;
  library_path: string;
  enabled: boolean;
}

const fromRaw = (m: RawMod): Mod => ({
  id: m.id,
  game: m.game,
  name: m.name,
  source: m.source,
  libraryPath: m.library_path,
  enabled: m.enabled,
});

export async function listMods(game: GameCode): Promise<Mod[]> {
  const raw = await invoke<RawMod[]>("list_mods", { game });
  return raw.map(fromRaw);
}

export async function adoptFolder(
  game: GameCode,
  sourcePath: string,
  name: string,
): Promise<Mod> {
  const raw = await invoke<RawMod>("adopt_folder", {
    args: { game, sourcePath, name },
  });
  return fromRaw(raw);
}

export async function importZip(
  game: GameCode,
  zipPath: string,
  name: string,
): Promise<Mod> {
  const raw = await invoke<RawMod>("import_zip", {
    args: { game, zipPath, name },
  });
  return fromRaw(raw);
}

export async function setModEnabled(
  id: string,
  enabled: boolean,
  game: GameCode,
): Promise<void> {
  await invoke("set_mod_enabled", { id, enabled, game });
}

export async function getGameInstallPath(game: GameCode): Promise<string | null> {
  return (await invoke<string | null>("get_game_install_path", { game })) ?? null;
}

export async function setGameInstallPath(
  game: GameCode,
  path: string,
): Promise<void> {
  await invoke("set_game_install_path", { game, path });
}
