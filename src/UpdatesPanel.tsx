import { useSyncExternalStore } from "react";
import { updater, type UpdateController, type UpdateState } from "./updater";

function UpdateStatus({ controller }: { controller: UpdateController }) {
  const state = useSyncExternalStore(controller.subscribe, controller.getSnapshot);

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
    {state.kind === "error" ? <p role="alert" className="error">{state.message}</p> : null}
  </>;
}

/** Settings and the status bar's reading observe the same offer; the consent
 * actions — Install update and Skip this build — exist only here (#357). */
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
      disabled={state.kind === "checking" || state.kind === "downloading"}>
      {state.kind === "checking" ? "Checking…" : "Check for updates"}
    </button>
    <UpdateStatus controller={controller} />
  </section>;
}

/**
 * What the status bar says about a GMM update, or `null` when it says nothing.
 *
 * The bar carries ambient facts about this application, so it states the ones
 * that stay true until someone acts on them and leaves the rest to the GMM
 * updates group in Settings (#357):
 *
 * - An offer and a download in flight outlive the surface that started them. The
 *   download percentage is not in the bar: Settings is where the install was
 *   started and where the progress is, and the bar is a reading, not a meter.
 * - A failed check is the one failure that can happen with no surface open at
 *   all, so the bar names it rather than letting a launch that could not check
 *   look like a launch that found nothing. A failed installation is named for
 *   the same reason — it leaves the build unapplied.
 * - `idle`, `checking` and `up-to-date` have nothing to act on, and a decline
 *   has been answered. A reading for any of them would be the bar nagging
 *   about an answered question; Settings keeps the declined outcome visible
 *   and a failed decline-memory explains itself there (#346, ADR 0004).
 */
function updateReading(state: UpdateState): string | null {
  switch (state.kind) {
    case "available": return `GMM update available: ${state.version}`;
    case "downloading": return "GMM update downloading…";
    case "error": return state.failure === "check" ? "GMM update check unavailable"
      : state.failure === "install" ? "GMM update installation failed"
      : null;
    default: return null;
  }
}

/** The bar's reading, for the status bar's own render gate. */
export function useUpdateReading(controller: UpdateController = updater): string | null {
  return updateReading(useSyncExternalStore(controller.subscribe, controller.getSnapshot));
}

/**
 * An available update as a reading in the status bar (#357).
 *
 * It replaces the fixed-position card that used to float in the bottom-right
 * corner: the card covered the bar's own Settings link, drew undimmed over the
 * modal Settings dialog, and stayed mouse-clickable while the screen behind
 * the dialog was inert yet was keyboard-unreachable, because the dialog owns
 * the focus cycle. A reading in the bar occupies space instead of floating, so
 * it covers nothing, and it is inert with the screen it belongs to.
 *
 * Its only control is a route to the GMM updates group in Settings. There is
 * no Install, no Skip and no dismissal here: the decision to install stays
 * behind an explicit click on that group's own button (#346, ADR 0004), and
 * declining, remembering a decline and re-offering are unchanged.
 */
export function StatusBarUpdateReading({ controller = updater, onSettings }: {
  controller?: UpdateController;
  onSettings: () => void;
}) {
  const reading = useUpdateReading(controller);
  if (reading === null) return null;
  return <div className="status-bar__update">
    <span>{reading}</span>
    {/* `data-settings-arrival` is what the focus return resolves after the
        dialog closes, the way the Game facts' own link does. */}
    <button data-settings-arrival="gmm-update" onClick={onSettings}>Review GMM updates in Settings</button>
  </div>;
}
