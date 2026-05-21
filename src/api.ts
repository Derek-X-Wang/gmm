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

export interface UpdateStatus {
  available: boolean;
  installedVersion: string | null;
  latestVersion: string | null;
  pinned: boolean;
  upstreamAhead: boolean;
}

export async function checkImporterUpdate(game: GameCode): Promise<UpdateStatus> {
  return invoke<UpdateStatus>("check_importer_update", { game });
}

export async function checkLoaderUpdate(): Promise<UpdateStatus> {
  return invoke<UpdateStatus>("check_loader_update");
}

export async function setImporterPinned(
  game: GameCode,
  version: string | null,
): Promise<void> {
  await invoke("set_importer_pinned", { game, version });
}

export interface ModUpdateRow {
  modId: string;
  name: string;
  installedVersion: string | null;
  upstreamVersion: string | null;
  upstreamAhead: boolean;
  updateCheckEnabled: boolean;
}

export async function listModUpdates(game: GameCode): Promise<ModUpdateRow[]> {
  return invoke<ModUpdateRow[]>("list_mod_updates", { game });
}

export async function checkModUpdatesNow(game: GameCode): Promise<ModUpdateRow[]> {
  return invoke<ModUpdateRow[]>("check_mod_updates_now", { game });
}

export async function setModUpdateCheckEnabled(
  modId: string,
  enabled: boolean,
): Promise<void> {
  await invoke("set_mod_update_check_enabled", { modId, enabled });
}

export async function setModUpdatesGloballyEnabled(enabled: boolean): Promise<void> {
  await invoke("set_mod_updates_globally_enabled", { enabled });
}

export async function modUpdatesGloballyEnabled(): Promise<boolean> {
  return invoke<boolean>("mod_updates_globally_enabled");
}

export async function applyModUpdate(modId: string): Promise<void> {
  await invoke("apply_mod_update", { modId });
}

// ---- slice 4b (#12) — game session ----

export interface SessionInfo {
  game: GameCode;
  pid: number;
  startedAt: string; // RFC 3339
}

interface RawSessionInfo {
  game: GameCode;
  pid: number;
  started_at: string;
}

const fromRawSession = (s: RawSessionInfo): SessionInfo => ({
  game: s.game,
  pid: s.pid,
  startedAt: s.started_at,
});

export async function currentSession(): Promise<SessionInfo | null> {
  const raw = await invoke<RawSessionInfo | null>("current_session");
  return raw ? fromRawSession(raw) : null;
}

export async function cleanStaleSession(): Promise<SessionInfo | null> {
  const raw = await invoke<RawSessionInfo | null>("clean_stale_session");
  return raw ? fromRawSession(raw) : null;
}

export async function launchGame(game: GameCode): Promise<SessionInfo> {
  const raw = await invoke<RawSessionInfo>("launch_game", { game });
  return fromRawSession(raw);
}

export const SESSION_STARTED_EVENT = "session-started";
export const SESSION_ENDED_EVENT = "session-ended";

// ---- slice 16-b (#24) — onboarding wizard ----

/**
 * Persistent onboarding state. The App router uses this on every
 * cold start to choose between rendering the wizard vs. the main
 * app.
 */
export interface OnboardingStatus {
  complete: boolean;
  /** `true` iff the user pressed Skip setup. The "Finish setup"
   * banner in Settings stays alive until they Resume. */
  skipped: boolean;
}

export async function isOnboardingComplete(): Promise<OnboardingStatus> {
  return invoke<OnboardingStatus>("is_onboarding_complete");
}

export async function markOnboardingComplete(skipped: boolean): Promise<void> {
  await invoke("mark_onboarding_complete", { skipped });
}

export async function resetOnboarding(): Promise<void> {
  await invoke("reset_onboarding");
}

/** Per-game detection result returned by `detect_all_games`. The
 * wizard's Step 2 renders one row per supported game. */
export interface GameDetection {
  code: GameCode;
  displayName: string;
  detectedPath: string | null;
}

export async function detectAllGames(): Promise<GameDetection[]> {
  return invoke<GameDetection[]>("detect_all_games");
}

// ---- slice 6 (#16) — per-game registry ----

/**
 * Backend-supported game summary. The React tab strip uses this to
 * decide which games to render. New per-game ports (#17–#20) light
 * up additional entries as their Rust registry rows fill in.
 */
export interface GameSummary {
  code: GameCode;
  displayName: string;
}

export async function listSupportedGames(): Promise<GameSummary[]> {
  return invoke<GameSummary[]>("list_supported_games");
}

// ---- slice NEW-AV (#13) — antivirus / SmartScreen guidance ----

/**
 * Structured payload backing the in-app antivirus / SmartScreen
 * guidance. The same shape is reused by the first-run onboarding
 * wizard (#24) so both render from a single source of truth in
 * `docs/antivirus-and-smartscreen.md`.
 */
export interface AvGuidance {
  headline: string;
  body: string;
  exclusionSteps: string[];
  docPath: string;
  /**
   * Sentinel prefix used on launch error strings classified as
   * AV-pattern. The launch button strips this prefix and renders the
   * structured guidance instead of dumping the raw error to the user.
   */
  sentinel: string;
}

export async function avGuidance(): Promise<AvGuidance> {
  return invoke<AvGuidance>("av_guidance");
}

/**
 * Inspect a thrown error string from `launch_game`. If the backend
 * classifier matched a known AV / SmartScreen pattern, the message is
 * prefixed with the sentinel from `AvGuidance`; we return the
 * original (sentinel-stripped) message alongside an `isAvPattern`
 * flag. Non-AV errors round-trip with the flag set to false.
 */
export function partitionLaunchError(
  raw: unknown,
  sentinel: string,
): { isAvPattern: boolean; message: string } {
  const message = String(raw);
  if (message.startsWith(sentinel)) {
    return { isAvPattern: true, message: message.slice(sentinel.length) };
  }
  return { isAvPattern: false, message };
}
