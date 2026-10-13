import { check, type Update } from "@tauri-apps/plugin-updater";

export type UpdateState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "up-to-date" }
  | { kind: "available"; version: string; notes?: string }
  | { kind: "declined"; version: string; remembered?: boolean }
  | { kind: "downloading"; downloaded: number; total: number | null }
  | { kind: "installed"; restartError?: string }
  | { kind: "error"; message: string };

// Webview storage survives app launches. A version-specific choice must not
// suppress a later release, or prevent a deliberate check in Settings.
export const declinedVersionKey = "gmm.updater.declinedVersion";
type OfferedUpdate = Pick<Update, "version" | "body" | "downloadAndInstall">;

/** Shared by startup and Settings so neither path can bypass consent. */
export function createUpdateController(
  checkForUpdate: () => Promise<OfferedUpdate | null> = check,
  storage: Pick<Storage, "getItem" | "setItem"> = window.localStorage,
) {
  let state: UpdateState = { kind: "idle" };
  let offered: OfferedUpdate | null = null;
  const listeners = new Set<() => void>();
  const publish = (next: UpdateState) => {
    state = next;
    listeners.forEach((listener) => listener());
  };
  const fail = (action: string, error: unknown) => {
    publish({
      kind: "error",
      message: `${action}: ${error instanceof Error ? error.message : String(error)}`,
    });
  };

  const checkForUpdates = async (background = false) => {
    if (state.kind === "checking" || state.kind === "downloading" || state.kind === "installed") return;
    offered = null;
    publish({ kind: "checking" });
    try {
      const update = await checkForUpdate();
      if (!update) {
        publish({ kind: "up-to-date" });
        return;
      }
      if (background && storage.getItem(declinedVersionKey) === update.version) {
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

  const decline = () => {
    if (state.kind !== "available" || !offered) return;
    const version = offered.version;
    offered = null;
    try {
      storage.setItem(declinedVersionKey, version);
      publish({ kind: "declined", version });
    } catch (error) {
      fail("Update skipped, but could not remember this choice", error);
    }
  };

  const reportRestartError = (error: unknown) => {
    publish({ kind: "installed", restartError: `Restart failed: ${error instanceof Error ? error.message : String(error)}` });
  };
  return {
    getSnapshot: () => state,
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
