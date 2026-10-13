import { useState, useSyncExternalStore } from "react";
import { updater, type UpdateController } from "./updater";
import { relaunch } from "@tauri-apps/plugin-process";

function UpdateStatus({ controller }: { controller: UpdateController }) {
  const state = useSyncExternalStore(controller.subscribe, controller.getSnapshot);
  const onRestart = async () => {
    try {
      await relaunch();
    } catch (error) {
      controller.reportRestartError(error);
    }
  };

  return <>
    {state.kind === "up-to-date" ? <p className="muted">You're on the latest release.</p> : null}
    {state.kind === "available" ? <>
      <p>Version <strong>{state.version}</strong> is available. Install this update?</p>
      {state.notes ? <p className="small">{state.notes}</p> : null}
      <div className="row">
        <button onClick={() => void controller.install()}>Install update</button>
        <button onClick={() => void controller.decline()}>Skip this build</button>
      </div>
    </> : null}
    {state.kind === "declined" ? <>
      <p>Version {state.version} was skipped. Check for updates to review it again.</p>
      {state.remembered === false ? <p>This build could not be identified. It will be offered again on launch.</p> : null}
    </> : null}
    {state.kind === "downloading" ? <p className="muted">
      Downloading{state.total ? ` (${Math.round((100 * state.downloaded) / state.total)}%)` : "…"}
    </p> : null}
    {state.kind === "installed" ? <>
      <p>Update installed. Click <strong>Restart to apply</strong> to load it.</p>
      <button onClick={() => void onRestart()}>Restart to apply</button>
      {state.restartError ? <p role="alert" className="error">{state.restartError}</p> : null}
    </> : null}
    {state.kind === "error" ? <p role="alert" className="error">{state.message}</p> : null}
  </>;
}

/** Settings and the startup notice observe the same offer and consent actions. */
export function UpdatesPanel({ controller = updater }: { controller?: UpdateController }) {
  const state = useSyncExternalStore(controller.subscribe, controller.getSnapshot);
  return <section className="card">
    <h2>GMM updates</h2>
    <p className="muted small">
      GMM checks for a signed update on every launch. Signatures are verified
      against the public key embedded in this build — only releases signed by
      the project maintainer install. You choose whether to install each update.
    </p>
    <button onClick={() => void controller.checkForUpdates()}
      disabled={state.kind === "checking" || state.kind === "downloading" || state.kind === "installed"}>
      {state.kind === "checking" ? "Checking…" : "Check for updates"}
    </button>
    <UpdateStatus controller={controller} />
  </section>;
}

/** Visible on every destination, including first-run setup. Never auto-installs. */
export function StartupUpdateNotice({ controller = updater }: { controller?: UpdateController }) {
  const state = useSyncExternalStore(controller.subscribe, controller.getSnapshot);
  const [dismissed, setDismissed] = useState<typeof state | null>(null);
  if (state === dismissed || (state.kind === "declined" && state.remembered)) return null;
  if (state.kind !== "available" && state.kind !== "downloading" && state.kind !== "installed" && state.kind !== "error" && state.kind !== "declined") return null;
  return <aside className="card" aria-label="GMM update"
    style={{ position: "fixed", bottom: 16, right: 16, maxWidth: "min(420px, calc(100vw - 32px))", maxHeight: "70vh", overflowY: "auto", zIndex: 1000 }}>
    <h2>GMM update</h2>
    <UpdateStatus controller={controller} />
    {state.kind === "declined" || state.kind === "error" ? <button onClick={() => setDismissed(state)}>Close update notice</button> : null}
    {state.kind === "error" ? <button onClick={() => void controller.checkForUpdates(true)}>Retry update check</button> : null}
  </aside>;
}
