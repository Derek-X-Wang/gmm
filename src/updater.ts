import { check, type Update } from "@tauri-apps/plugin-updater";
import { invoke } from "./commandError";

export type UpdateState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "up-to-date" }
  | { kind: "available"; version: string; notes?: string }
  | { kind: "declined"; version: string; remembered?: boolean }
  | { kind: "downloading"; downloaded: number; total: number | null }
  | { kind: "installed"; restartError?: string }
  | { kind: "error"; message: string };

export const declinedBuildKey = "updater.skippedBuild";
export interface SkipStorage {
  getItem(key: string): string | null | Promise<string | null>;
  setItem(key: string, value: string): void | Promise<void>;
}
const databaseStorage: SkipStorage = {
  getItem: () => invoke<string | null>("get_skipped_update"),
  setItem: (_key, build) => invoke<void>("set_skipped_update", { build }),
};
type OfferedUpdate = Pick<Update, "version" | "body" | "downloadAndInstall"> & Partial<Pick<Update, "rawJson">>;

function buildIdentity(update: OfferedUpdate): string | null {
  const raw = update.rawJson;
  if (!raw) return null;
  const platforms = raw.platforms;
  const artifacts = platforms && typeof platforms === "object" && !Array.isArray(platforms)
    ? Object.entries(platforms).sort(([a], [b]) => a.localeCompare(b))
    : [["dynamic", raw]];
  if (!artifacts.length) return null;
  const signed = artifacts.map(([platform, artifact]) => {
    if (!artifact || typeof artifact !== "object") return null;
    const { url, signature } = artifact as Record<string, unknown>;
    return typeof url === "string" && url.length > 0 && typeof signature === "string" && signature.length > 0
      ? [platform, url, signature] : null;
  });
  // Alpha tags share the application version. Each artifact signature identifies
  // its signed installer; release URLs also distinguish tags. Date/body/rid
  // do not. Missing metadata cannot prove sameness: offer again rather than hide.
  return signed.some((artifact) => !artifact) ? null : JSON.stringify([update.version, signed]);
}

/** Shared by startup and Settings so neither path can bypass consent. */
export function createUpdateController(
  checkForUpdate: () => Promise<OfferedUpdate | null> = check,
  storage: SkipStorage = databaseStorage,
) {
  let state: UpdateState = { kind: "idle" };
  let lastError: string | null = null;
  let savingSkip = false;
  let offered: OfferedUpdate | null = null;
  const listeners = new Set<() => void>();
  const publish = (next: UpdateState) => {
    state = next;
    listeners.forEach((listener) => listener());
  };
  const fail = (action: string, error: unknown) => {
    lastError = `${action}: ${error instanceof Error ? error.message : String(error)}`;
    publish({ kind: "error", message: lastError });
  };

  const checkForUpdates = async (background = false) => {
    if (savingSkip || state.kind === "checking" || state.kind === "downloading" || state.kind === "installed") return;
    offered = null;
    publish({ kind: "checking" });
    try {
      const update = await checkForUpdate();
      if (!update) {
        publish({ kind: "up-to-date" });
        return;
      }
      const identity = buildIdentity(update);
      const saved = background && identity !== null ? await storage.getItem(declinedBuildKey) : null;
      if (identity !== null && saved === JSON.stringify({ version: update.version, identity })) {
        publish({ kind: "declined", version: update.version, remembered: true });
        return;
      }
      offered = update;
      publish({ kind: "available", version: update.version, notes: update.body });
    } catch (error) {
      fail("Update check failed", error);
    }
  };

  // Only the explicit Install update button calls this action. Checking never
  // downloads; consume the offer before awaiting to prevent duplicate installs.
  const install = async () => {
    if (state.kind !== "available" || !offered) return;
    const update = offered;
    offered = null;
    let downloaded = 0;
    let total: number | null = null;
    publish({ kind: "downloading", downloaded, total });
    try {
      await update.downloadAndInstall((event) => {
        if (event.event === "Started") total = event.data.contentLength ?? null;
        if (event.event === "Progress") downloaded += event.data.chunkLength;
        // Finished means download finished; installation can still fail.
        publish({ kind: "downloading", downloaded, total });
      });
      publish({ kind: "installed" });
    } catch (error) {
      fail("Update installation failed", error);
    }
  };

  const decline = async () => {
    if (state.kind !== "available" || !offered) return;
    const version = offered.version;
    const identity = buildIdentity(offered);
    offered = null;
    savingSkip = true;
    try {
      if (identity !== null) await storage.setItem(declinedBuildKey, JSON.stringify({ version, identity }));
      publish(identity === null ? { kind: "declined", version, remembered: false } : { kind: "declined", version });
    } catch (error) {
      fail("Update skipped, but could not remember this choice", error);
    } finally {
      savingSkip = false;
    }
  };

  const reportRestartError = (error: unknown) => {
    lastError = `Restart failed: ${error instanceof Error ? error.message : String(error)}`;
    publish({ kind: "installed", restartError: lastError });
  };
  return {
    getSnapshot: () => state,
    getLastError: () => lastError,
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      return () => { listeners.delete(listener); };
    },
    checkForUpdates,
    install,
    decline,
    reportRestartError,
  };
}

export type UpdateController = ReturnType<typeof createUpdateController>;
export const updater = createUpdateController();
