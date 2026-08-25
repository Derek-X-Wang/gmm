import type { InterruptedSessionLaunch } from "./api";

export function InterruptedSessionLaunchWarning({
  launch,
  pending,
  error,
  onRetire,
}: {
  launch: InterruptedSessionLaunch;
  pending: boolean;
  error?: string;
  onRetire: () => void;
}) {
  return (
    <section
      className="card session-banner session-banner--stale"
      aria-label={`Interrupted ${launch.game.toUpperCase()} launch`}
    >
      <strong>Library locked — interrupted {launch.game.toUpperCase()} launch</strong>
      <span className="muted">
        {launch.childPid === null
          ? "GMM stopped before it could record the game process. "
          : `GMM recorded process PID ${launch.childPid}, but that PID may now belong to a different process. `}
        GMM cannot determine whether a game from this launch is still running, so it kept the
        launch reservation and left the Library untouched.
      </span>
      <span className="muted small">
        Close the game if it is open. Retire this reservation only after you confirm the game is
        closed; Mod changes will then be available again. Launch started {launch.startedAt}.
      </span>
      <button type="button" onClick={onRetire} disabled={pending}>
        {pending ? "Retiring reservation…" : "I confirmed the game is closed — retire reservation"}
      </button>
      {error ? <span className="error">Could not retire the launch reservation: {error}</span> : null}
    </section>
  );
}
