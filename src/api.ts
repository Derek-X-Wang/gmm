import {
  CommandFailure,
  commandFailureFrom,
  invoke,
  type SurfaceFailureKind,
} from "./commandError";
export type { SurfaceFailureKind } from "./commandError";

export type GameCode = "gimi" | "srmi" | "zzmi" | "wwmi" | "himi" | "efmi";

export type Source = "manual" | "local" | "gamebanana";

export interface ReinstallRecovery {
  reason: string;
  attemptedAt: string;
  attempts: number;
  libraryPath: string;
  stagedPath: string;
  quarantinePath: string;
  junctionWithdrawn: boolean;
  junctionWithdrawalError: string | null;
}

export interface EnabledTransitionRecovery {
  intendedEnabled: boolean;
  reason: string;
  attemptedAt: string;
  attempts: number;
  junctionPath: string;
}

export type ReinstallRecoveryOutcome =
  | { status: "recovered" }
  | { status: "alreadyRecovered" }
  | { status: "quarantined"; recovery: ReinstallRecovery };

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
  reinstallRecovery: ReinstallRecovery | null;
  enabledTransitionRecovery: EnabledTransitionRecovery | null;
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
  reinstall_recovery?: ReinstallRecovery | null;
  enabled_transition_recovery?: EnabledTransitionRecovery | null;
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
  reinstallRecovery: m.reinstall_recovery ?? null,
  enabledTransitionRecovery: m.enabled_transition_recovery ?? null,
});

export async function listMods(game: GameCode): Promise<Mod[]> {
  const raw = await invoke<RawMod[]>("list_mods", { game });
  return raw.map(fromRaw);
}

export async function retryReinstallRecovery(
  modId: string,
): Promise<ReinstallRecoveryOutcome> {
  return invoke<ReinstallRecoveryOutcome>("retry_reinstall_recovery", { modId });
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
  /** Mod IDs whose stranded junction was deleted because the Mod is disabled. */
  removed: string[];
  skipped: string[];
  /** Mods whose uncertain reinstall state was deliberately left untouched. */
  quarantined: string[];
}

export interface StartupReconcileFailure {
  game: GameCode;
  kind: SurfaceFailureKind;
  error: string;
}

export interface StartupReconcileStatus {
  finished: boolean;
  failures: StartupReconcileFailure[];
}

export async function getStartupReconcileStatus(): Promise<StartupReconcileStatus> {
  return invoke<StartupReconcileStatus>("get_startup_reconcile_status");
}

interface RawReconcile {
  recreated: string[];
  healthy: string[];
  conflicting: { mod_id: string; link: string; expected_target: string }[];
  removed: string[];
  skipped: string[];
  quarantined?: string[];
}

const fromRawReconcile = (r: RawReconcile): ReconcileResult => ({
  recreated: r.recreated,
  healthy: r.healthy,
  conflicting: r.conflicting.map((c) => ({
    modId: c.mod_id,
    link: c.link,
    expectedTarget: c.expected_target,
  })),
  removed: r.removed,
  skipped: r.skipped,
  quarantined: r.quarantined ?? [],
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
  failed_junction_restores: Array<{
    mod_id: string;
    game: GameCode;
    kind: SurfaceFailureKind;
    error: string;
  }>;
}

export async function getLibraryPaths(): Promise<LibraryPaths> {
  return invoke<LibraryPaths>("get_library_paths");
}

export interface UnreferencedLibraryDir {
  directoryName: string;
  path: string;
  sizeBytes: number | null;
}

export interface DuplicateModVariant {
  id: string;
  name: string;
  subpath: string;
  active: boolean;
}

export interface DuplicateModRecord {
  id: string;
  game: GameCode;
  name: string;
  source: Source;
  libraryPath: string;
  junctionDirName: string;
  enabled: boolean;
  createdAt: string;
  gamebananaId: number | null;
  sourceUrl: string | null;
  author: string | null;
  version: string | null;
  upstreamVersion: string | null;
  updateCheckEnabled: boolean;
  screenshotUrl: string | null;
  variants: DuplicateModVariant[];
  reinstallInProgress: boolean;
  fingerprint: string;
}

export interface DuplicateModGroup {
  path: string;
  mods: DuplicateModRecord[];
}

export interface DuplicateResolution {
  keeperId: string;
  removedModIds: string[];
}

export interface LibraryAuditReport {
  game: GameCode;
  unreferenced: UnreferencedLibraryDir[];
  duplicates: DuplicateModGroup[];
  totalBytes: number;
}

export async function auditLibrary(game: GameCode): Promise<LibraryAuditReport> {
  return invoke<LibraryAuditReport>("audit_library", { game });
}

export async function resolveDuplicateMods(
  keeperId: string,
  reviewedMods: Array<{ id: string; fingerprint: string }>,
): Promise<DuplicateResolution> {
  return invoke<DuplicateResolution>("resolve_duplicate_mods", {
    args: { keeperId, reviewedMods },
  });
}

export interface DeletedLibraryDir {
  directoryName: string;
  path: string;
  sizeBytes: number | null;
  reclamation:
    | { status: "reclaimed" }
    | { status: "deferred"; path: string }
    | { status: "ownershipLost" };
}

/** Open the file manager on an unreferenced Library folder. */
export async function revealUnreferencedLibraryDir(
  game: GameCode,
  path: string,
): Promise<void> {
  return invoke<void>("reveal_unreferenced_library_dir", { game, path });
}

/**
 * Adopt an unreferenced Library folder as a Mod. The name is the user's —
 * nothing on disk records what the interrupted import was called. Copies
 * nothing: the bytes are already in the Library.
 */
export async function recoverUnreferencedLibraryDir(
  game: GameCode,
  path: string,
  name: string,
): Promise<Mod> {
  const raw = await invoke<RawMod>("recover_unreferenced_library_dir", {
    args: { game, path, name },
  });
  return fromRaw(raw);
}

/** Permanently delete one confirmed unreferenced Library folder. */
export async function deleteUnreferencedLibraryDir(
  game: GameCode,
  path: string,
): Promise<DeletedLibraryDir> {
  return invoke<DeletedLibraryDir>("delete_unreferenced_library_dir", { game, path });
}

/**
 * A relocation that could not restore every Junction still succeeded: the
 * rows, the settings and the bytes agree. Normalise the failure list at the
 * boundary so callers never have to guard an absent field — a payload
 * without it means "nothing failed", not "unknown".
 */
const fromRawMoveReport = (r: Partial<MoveReport>): MoveReport => ({
  relocated: r.relocated ?? [],
  moved_directories: r.moved_directories ?? [],
  failed_junction_restores: r.failed_junction_restores ?? [],
});

export async function setLibraryRoot(path: string | null): Promise<MoveReport> {
  return fromRawMoveReport(
    await invoke<Partial<MoveReport>>("set_library_root", { path }),
  );
}

export async function setLibraryPathForGame(
  game: GameCode,
  path: string | null,
): Promise<MoveReport> {
  return fromRawMoveReport(
    await invoke<Partial<MoveReport>>("set_library_path_for_game", { game, path }),
  );
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

/**
 * Importer update check. `checkError` being set means "we don't know" —
 * the origin was unreachable, or its release did not yield exactly one
 * asset matching the origin's anchored pattern (#79). That is a
 * different statement from `available: false` ("we checked, nothing to
 * apply"), and collapsing the two is the defect #78 fixed for the Loader
 * and #79 fixed here.
 */
export interface UpdateStatus {
  available: boolean;
  installedVersion: string | null;
  latestVersion: string | null;
  pinned: boolean;
  upstreamAhead: boolean;
  checkError: string | null;
}

export async function checkImporterUpdate(game: GameCode): Promise<UpdateStatus> {
  return invoke<UpdateStatus>("check_importer_update", { game });
}

/**
 * What GMM ships versus what upstream published. Purely
 * informational — the Loader is embedded via FFI (ADR 0001), so there
 * is no `available` flag and no Apply button. `checkError` being set
 * means "we don't know", which is not the same as `upstreamAhead:
 * false` ("we checked, we're current"). See #78.
 */
export interface LoaderVersionStatus {
  shippedVersion: string;
  latestVersion: string | null;
  upstreamAhead: boolean;
  checkError: string | null;
}

export async function checkLoaderUpdate(): Promise<LoaderVersionStatus> {
  return invoke<LoaderVersionStatus>("check_loader_update");
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

export interface InterruptedSessionLaunch {
  id: string;
  game: GameCode;
  childPid: number | null;
  startedAt: string;
}

interface RawInterruptedSessionLaunch {
  id: string;
  game: GameCode;
  child_pid: number | null;
  started_at: string;
}

export async function interruptedSessionLaunches(): Promise<InterruptedSessionLaunch[]> {
  const rows = await invoke<RawInterruptedSessionLaunch[]>("interrupted_session_launches");
  return rows.map((row) => ({
    id: row.id,
    game: row.game,
    childPid: row.child_pid,
    startedAt: row.started_at,
  }));
}

export async function retireInterruptedSessionLaunch(id: string): Promise<void> {
  return invoke<void>("retire_interrupted_session_launch", { id });
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
 * Inspect a structured failure from `launch_game`. If the backend
 * classifier matched a known AV / SmartScreen pattern, the message is
 * prefixed with the sentinel from `AvGuidance`; we return a new failure
 * carrying the original kind and sentinel-stripped message alongside an
 * `isAvPattern` flag. Non-AV errors round-trip unchanged.
 */
export function partitionLaunchError(
  raw: unknown,
  sentinel: string,
): { isAvPattern: boolean; failure: CommandFailure } {
  const failure = commandFailureFrom(raw);
  if (failure.message.startsWith(sentinel)) {
    return {
      isAvPattern: true,
      failure: new CommandFailure({
        kind: failure.kind,
        message: failure.message.slice(sentinel.length),
      }),
    };
  }
  return { isAvPattern: false, failure };
}

// ---- Importer Origin surface (ADR 0005 / #109) ----

/**
 * A GitHub Importer Origin. `asset_pattern` is snake_case because this
 * shape is also what GMM persists in its settings table — renaming the
 * field would make every already-recorded origin unreadable.
 */
export interface ImporterOriginRef {
  kind: "gitHubRelease";
  owner: string;
  repo: string;
  asset_pattern: string;
}

/** `owner/repo`, the form GitHub and the UI both use. */
export function originSlug(origin: ImporterOriginRef): string {
  return `${origin.owner}/${origin.repo}`;
}

/** Which precedence layer supplied the origin in effect. */
export type OriginLayer =
  | "userOverride"
  | "recommendedManifest"
  | "compiledInDefault";

/**
 * Which origin is in effect, or that none is. Deliberately not
 * `ImporterOriginRef | null`: "no origin is in effect" is a real state
 * with a reason to show, not a missing value.
 */
export type OriginResolution =
  | { state: "inEffect"; origin: ImporterOriginRef; layer: OriginLayer }
  | { state: "noneInEffect"; reason: string | null };

/**
 * The origin an install came from. `unknown` means no install GMM
 * performed — a first-class state, never "not installed" and never
 * backfilled. `unreadable` makes the opposite claim: GMM did perform the
 * install and can no longer say from where.
 */
export type InstalledOrigin =
  | ({ state: "known" } & Omit<ImporterOriginRef, "kind"> & { kind: "gitHubRelease" })
  | { state: "unknown" }
  | { state: "unreadable"; raw: string; error: string };

export type OverrideState =
  | { state: "notSet" }
  | ({ state: "set" } & ImporterOriginRef)
  | { state: "unreadable"; raw: string; error: string };

/** Which origin an ordinary Install / Update would act on. */
export type InstallTarget =
  | ({ state: "installed" } & ImporterOriginRef)
  | { state: "resolved"; origin: ImporterOriginRef; layer: OriginLayer }
  | { state: "noneInEffect"; reason: string | null }
  | { state: "installedUnreadable"; raw: string; error: string };

export interface OriginProposal {
  origin: ImporterOriginRef;
  /** The manifest entry's explanation, when it wrote one down. */
  reason: string | null;
  replaces: InstalledOrigin;
}

export interface ImporterOriginStatus {
  game: GameCode;
  displayName: string;
  resolved: OriginResolution;
  installTarget: InstallTarget;
  installed: InstalledOrigin;
  userOverride: OverrideState;
  compiledDefault: ImporterOriginRef | null;
  proposal: OriginProposal | null;
  dismissed: ImporterOriginRef[];
  /** GMM holds dismissal state it cannot read. Shown, never swallowed. */
  dismissalsError: string | null;
  recommendationsEnabled: boolean;
  recommendationsUnusableReason: string | null;
}

export async function importerOriginStatus(
  game: GameCode,
): Promise<ImporterOriginStatus> {
  return invoke<ImporterOriginStatus>("importer_origin_status", { game });
}

/** What the override editor collects. Validated in Rust, not here. */
export interface ImporterOriginInput {
  owner: string;
  repo: string;
  assetPattern: string;
}

export async function setImporterOriginOverride(
  game: GameCode,
  origin: ImporterOriginInput | null,
): Promise<void> {
  await invoke("set_importer_origin_override", { game, origin });
}

/**
 * Accept the proposed Importer Origin: install from it, backing up and
 * leaving the move rollbackable. There is deliberately no way to record
 * an origin without installing.
 */
export async function acceptImporterOriginProposal(
  game: GameCode,
): Promise<InstallReport> {
  return invoke<InstallReport>("accept_importer_origin_proposal", { game });
}

export async function dismissImporterOrigin(
  game: GameCode,
  origin: ImporterOriginInput,
): Promise<void> {
  await invoke("dismiss_importer_origin", { game, origin });
}

export async function restoreImporterOrigin(
  game: GameCode,
  origin: ImporterOriginInput,
): Promise<void> {
  await invoke("restore_importer_origin", { game, origin });
}

export async function importerRecommendationsEnabled(): Promise<boolean> {
  return invoke<boolean>("importer_recommendations_enabled");
}

export async function setImporterRecommendationsEnabled(
  enabled: boolean,
): Promise<void> {
  await invoke("set_importer_recommendations_enabled", { enabled });
}

/** An `ImporterOriginRef` as the commands want it. */
export function toOriginInput(origin: ImporterOriginRef): ImporterOriginInput {
  return {
    owner: origin.owner,
    repo: origin.repo,
    assetPattern: origin.asset_pattern,
  };
}
