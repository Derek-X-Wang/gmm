import { useState } from "react";
import { checkInteractively, type UpdateState } from "./updater";
import { relaunch } from "@tauri-apps/plugin-process";

/**
 * Settings → Updates. Wraps the interactive update check from
 * updater.ts in a small state-machine UI. The background-check
 * fired in main.tsx handles silent auto-update; this panel is for
 * the user-pressed "Check for updates" path.
 */
export function UpdatesPanel() {
  const [state, setState] = useState<UpdateState>({ kind: "idle" });

  const onCheck = async () => {
    await checkInteractively(setState);
  };

  const onRestart = async () => {
    await relaunch();
  };

  return (
    <section className="card">
      <h2>GMM updates</h2>
      <p className="muted small">
        GMM checks for a signed update on every launch. Signatures are verified
        against the public key embedded in this build — only releases signed by
        the project maintainer install. Click below to check on demand.
      </p>
      <div className="row">
        <button onClick={onCheck} disabled={state.kind === "checking" || state.kind === "downloading"}>
          {state.kind === "checking" ? "Checking…" : "Check for updates"}
        </button>
        {state.kind === "installed" ? (
          <button onClick={onRestart}>Restart to apply</button>
        ) : null}
      </div>
      {state.kind === "up-to-date" ? (
        <p className="muted">You're on the latest release.</p>
      ) : null}
      {state.kind === "available" ? (
        <p>
          Version <strong>{state.version}</strong> is available — downloading now.
        </p>
      ) : null}
      {state.kind === "downloading" ? (
        <p className="muted">
          Downloading{state.total ? ` (${Math.round((100 * state.downloaded) / state.total)}%)` : "…"}
        </p>
      ) : null}
      {state.kind === "installed" ? (
        <p>Update downloaded. Click <strong>Restart to apply</strong> to load it.</p>
      ) : null}
      {state.kind === "error" ? <p className="error">{state.message}</p> : null}
    </section>
  );
}
