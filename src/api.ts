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
  gamebananaId: number | null;
  sourceUrl: string | null;
  author: string | null;
  version: string | null;
  screenshotUrl: string | null;
}

interface RawMod {
  id: string;
  game: GameCode;
  name: string;
  source: Source;
  library_path: string;
  enabled: boolean;
  gamebanana_id?: number | null;
  source_url?: string | null;
  author?: string | null;
  version?: string | null;
  screenshot_url?: string | null;
}

const fromRaw = (m: RawMod): Mod => ({
  id: m.id,
  game: m.game,
  name: m.name,
  source: m.source,
  libraryPath: m.library_path,
  enabled: m.enabled,
  gamebananaId: m.gamebanana_id ?? null,
  sourceUrl: m.source_url ?? null,
  author: m.author ?? null,
  version: m.version ?? null,
  screenshotUrl: m.screenshot_url ?? null,
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

export async function detectGameInstallPath(
  game: GameCode,
): Promise<string | null> {
  return (
    (await invoke<string | null>("detect_game_install_path", { game })) ?? null
  );
}

export interface ConflictingJunction {
  modId: string;
  link: string;
  expectedTarget: string;
}

export interface ReconcileResult {
  recreated: string[];
  healthy: string[];
  conflicting: ConflictingJunction[];
  skipped: string[];
}

interface RawReconcile {
  recreated: string[];
  healthy: string[];
  conflicting: { mod_id: string; link: string; expected_target: string }[];
  skipped: string[];
}

const fromRawReconcile = (r: RawReconcile): ReconcileResult => ({
  recreated: r.recreated,
  healthy: r.healthy,
  conflicting: r.conflicting.map((c) => ({
    modId: c.mod_id,
    link: c.link,
    expectedTarget: c.expected_target,
  })),
  skipped: r.skipped,
});

export async function reconcileJunctions(game: GameCode): Promise<ReconcileResult> {
  return fromRawReconcile(
    await invoke<RawReconcile>("reconcile_junctions", { game }),
  );
}

export async function rebuildJunctions(game: GameCode): Promise<ReconcileResult> {
  return fromRawReconcile(
    await invoke<RawReconcile>("rebuild_junctions", { game }),
  );
}

export interface LibraryPaths {
  defaultRoot: string;
  rootOverride: string | null;
  effectiveRoot: string;
  perGameOverrides: Record<string, string | null>;
  perGameEffective: Record<string, string>;
}

export interface MoveReport {
  relocated: string[];
  moved_directories: string[];
}

export async function getLibraryPaths(): Promise<LibraryPaths> {
  return invoke<LibraryPaths>("get_library_paths");
}

export async function setLibraryRoot(path: string | null): Promise<MoveReport> {
  return invoke<MoveReport>("set_library_root", { path });
}

export async function setLibraryPathForGame(
  game: GameCode,
  path: string | null,
): Promise<MoveReport> {
  return invoke<MoveReport>("set_library_path_for_game", { game, path });
}

export interface LatestRelease {
  tag_name: string;
  asset_url: string;
  asset_name: string;
  sha256_digest: string | null;
}

export interface InstallReport {
  backup_dir: string | null;
  sha256: string;
  rewrote_files: string[];
}

export async function fetchLatestImporterRelease(
  game: GameCode,
): Promise<LatestRelease | null> {
  return (
    (await invoke<LatestRelease | null>("fetch_latest_importer_release", { game })) ?? null
  );
}

export async function installImporter(game: GameCode): Promise<InstallReport> {
  return invoke<InstallReport>("install_importer", { game });
}

export async function rollbackImporter(game: GameCode): Promise<string | null> {
  return (
    (await invoke<string | null>("rollback_importer", { game })) ?? null
  );
}

export interface ProxyConfigPublic {
  url: string | null;
  username: string | null;
  passwordSet: boolean;
}

export async function getProxyConfig(): Promise<ProxyConfigPublic> {
  return invoke<ProxyConfigPublic>("get_proxy_config");
}

export async function setProxyConfig(args: {
  url: string | null;
  username: string | null;
  password: string | null;
}): Promise<ProxyConfigPublic> {
  return invoke<ProxyConfigPublic>("set_proxy_config", { args });
}

export async function testProxyConnection(): Promise<void> {
  await invoke("test_proxy_connection");
}

export interface Variant {
  id: string;
  mod_id: string;
  name: string;
  subpath: string;
}

export interface ModVariants {
  variants: Variant[];
  activeVariantId: string | null;
}

export async function listVariants(modId: string): Promise<ModVariants> {
  return invoke<ModVariants>("list_variants", { modId });
}

export async function setActiveVariant(
  modId: string,
  variantId: string,
  game: GameCode,
): Promise<void> {
  await invoke("set_active_variant", { modId, variantId, game });
}

export interface Conflict {
  hash: string;
  mod_ids: string[];
  sections: string[];
}

export interface ConflictReport {
  conflicts: Conflict[];
  per_mod_count: Record<string, number>;
}

export async function detectConflicts(game: GameCode): Promise<ConflictReport> {
  return invoke<ConflictReport>("detect_conflicts", { game });
}

export async function importGamebanana(
  game: GameCode,
  urlOrId: string,
): Promise<Mod> {
  const raw = await invoke<RawMod>("import_gamebanana", {
    args: { game, urlOrId },
  });
  return fromRaw(raw);
}
