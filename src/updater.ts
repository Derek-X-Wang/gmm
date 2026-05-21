import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

/**
 * Result of an update check, shaped for the Settings UI.
 *  - "checking"  : currently asking GitHub
 *  - "up-to-date": we asked + got nothing back
 *  - "available" : a newer release exists; details in `update`
 *  - "downloading" / "installed" : transitions during the install flow
 *  - "error"     : check or download failed (message preserved for UI)
 */
export type UpdateState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "up-to-date" }
  | { kind: "available"; version: string; notes?: string }
  | { kind: "downloading"; downloaded: number; total: number | null }
  | { kind: "installed" }
  | { kind: "error"; message: string };

/**
 * Background check fired on app start. Best-effort — failures only log.
 * If an update is available we download + install immediately, then
 * prompt for relaunch.
 */
export async function checkAndInstallSilently(): Promise<void> {
  try {
    const update = await check();
    if (!update) return;
    await applyUpdate(update);
    await relaunch();
  } catch (e) {
    // Networking failures, signature mismatches, etc. Log; don't toast.
    // tauri-plugin-updater includes its own user-visible signature
    // failure modal; we don't try to second-guess it here.
    // eslint-disable-next-line no-console
    console.warn("background update check failed", e);
  }
}

async function applyUpdate(update: Update): Promise<void> {
  let downloaded = 0;
  let total: number | null = null;
  await update.downloadAndInstall((event) => {
    switch (event.event) {
      case "Started":
        total = event.data.contentLength ?? null;
        break;
      case "Progress":
        downloaded += event.data.chunkLength;
        break;
      case "Finished":
        break;
    }
  });
  // Note: relaunch handled by caller.
  void downloaded;
  void total;
}

/**
 * Settings entry point — returns an explicit state machine the UI can
 * render. Caller is responsible for invoking `relaunch()` after a
 * successful "installed" state.
 */
export async function checkInteractively(
  onState: (state: UpdateState) => void,
): Promise<void> {
  onState({ kind: "checking" });
  try {
    const update = await check();
    if (!update) {
      onState({ kind: "up-to-date" });
      return;
    }
    onState({
      kind: "available",
      version: update.version,
      notes: update.body ?? undefined,
    });
    let downloaded = 0;
    let total: number | null = null;
    await update.downloadAndInstall((event) => {
      switch (event.event) {
        case "Started":
          total = event.data.contentLength ?? null;
          onState({ kind: "downloading", downloaded, total });
          break;
        case "Progress":
          downloaded += event.data.chunkLength;
          onState({ kind: "downloading", downloaded, total });
          break;
        case "Finished":
          onState({ kind: "installed" });
          break;
      }
    });
  } catch (e) {
    onState({ kind: "error", message: e instanceof Error ? e.message : String(e) });
  }
}
