import type { EnabledTransitionRecovery } from "./api";

/** A transition that automatic startup recovery could not settle yet (#190). */
export function EnabledTransitionRecoveryWarning({
  modName,
  recovery,
  pending,
  error,
  onRetire,
}: {
  modName: string;
  recovery: EnabledTransitionRecovery;
  pending: boolean;
  error?: string;
  onRetire: () => void;
}) {
  return (
    <section
      className="reinstall-recovery-warning"
      aria-label={`Interrupted enable or disable recovery for ${modName}`}
    >
      <strong>Unavailable — deployment recovery is still pending</strong>
      <p>
        GMM recorded the requested {recovery.intendedEnabled ? "enable" : "disable"}
        {" "}before changing the Junction, but could not finish making the Junction and enabled
        flag agree.
        {recovery.ownerUncertain
          ? " GMM cannot retry while the original producer may still be running."
          : " GMM will retry automatically the next time it starts."}
      </p>
      <p>
        Until recovery succeeds, GMM blocks game launch and later Mod or Library changes
        so neither side can silently become authoritative.
      </p>
      {recovery.ownerUncertain ? (
        <>
          <p>
            GMM cannot prove whether the original producer is still running. Close any other GMM
            instance, then retire that producer only after confirming no GMM is changing this Mod.
          </p>
          <button type="button" onClick={onRetire} disabled={pending}>
            {pending
              ? "Retiring producer…"
              : "I confirmed no other GMM is changing this Mod — retire producer"}
          </button>
          {error ? (
            <span className="error">Could not retire the transition producer: {error}</span>
          ) : null}
        </>
      ) : null}
      <details>
        <summary>Recorded path and recovery status</summary>
        <dl>
          <dt>Deployment path</dt>
          <dd><code>{recovery.junctionPath}</code></dd>
          <dt>{recovery.ownerUncertain ? "Recovery status" : "Last recovery error"}</dt>
          <dd>{recovery.reason}</dd>
        </dl>
      </details>
    </section>
  );
}
