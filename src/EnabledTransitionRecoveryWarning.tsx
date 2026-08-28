import type { EnabledTransitionRecovery } from "./api";

/** A transition that automatic startup recovery could not settle yet (#190). */
export function EnabledTransitionRecoveryWarning({
  modName,
  recovery,
}: {
  modName: string;
  recovery: EnabledTransitionRecovery;
}) {
  return (
    <section
      className="reinstall-recovery-warning"
      aria-label={`Interrupted enable or disable recovery for ${modName}`}
    >
      <strong>Unavailable — deployment recovery is still pending</strong>
      <p>
        GMM recorded the requested {recovery.intendedEnabled ? "enable" : "disable"}
        {" "}before changing the Junction, but could not finish making the Junction and
        enabled flag agree. GMM will retry automatically the next time it starts.
      </p>
      <p>
        Until recovery succeeds, GMM blocks game launch and later Mod or Library changes
        so neither side can silently become authoritative.
      </p>
      <details>
        <summary>Recorded path and recovery error</summary>
        <dl>
          <dt>Deployment path</dt>
          <dd><code>{recovery.junctionPath}</code></dd>
          <dt>Last recovery error</dt>
          <dd>{recovery.reason}</dd>
        </dl>
      </details>
    </section>
  );
}
