import { useState } from "react";

import type { EnabledTransitionRecovery } from "./api";

/** A transition that automatic startup recovery could not settle yet (#190). */
export function EnabledTransitionRecoveryWarning({
  modName,
  recovery,
  pending,
  onRetire,
  onRelease,
}: {
  modName: string;
  recovery: EnabledTransitionRecovery;
  pending: boolean;
  onRetire: () => void;
  /** Gives up the transition after the user confirms below. */
  onRelease?: () => void;
}) {
  const [confirmingRelease, setConfirmingRelease] = useState(false);
  if (recovery.released) {
    return (
      <section
        className="reinstall-recovery-warning"
        aria-label={`Unknown enabled state for ${modName}`}
      >
        <strong>Unknown — GMM does not know whether this Mod is enabled</strong>
        <p>
          An interrupted {recovery.intendedEnabled ? "enable" : "disable"} could not be finished, and
          you released it. GMM no longer blocks game launch or Library changes for it, but it cannot
          prove whether the Junction matches the stored enabled flag, so it does not show either.
        </p>
        <p>Enable or disable the Mod to settle it. GMM will do the work again and then record the result.</p>
        <details>
          <summary>Recorded path</summary>
          <dl>
            <dt>Deployment path</dt>
            <dd><code>{recovery.junctionPath}</code></dd>
          </dl>
        </details>
      </section>
    );
  }
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
        </>
      ) : null}
      {!recovery.ownerUncertain && onRelease ? (
        confirmingRelease ? (
          <div role="group" aria-label={`Confirm release for ${modName}`}>
            <p>
              <strong>If you release this, GMM gives up the following:</strong>
            </p>
            <ul>
              <li>GMM stops trying to finish the {recovery.intendedEnabled ? "enable" : "disable"}.</li>
              <li>
                GMM will not know whether {modName} is enabled. It will show Unknown, not Enabled or
                Disabled.
              </li>
              <li>GMM stops blocking game launch and Library changes, in every Game.</li>
              <li>
                Whatever is now at <code>{recovery.junctionPath}</code> stays as it is. GMM will not
                treat it as something it created.
              </li>
            </ul>
            <p>You can settle it later by enabling or disabling the Mod again.</p>
            <button
              type="button"
              onClick={() => {
                setConfirmingRelease(false);
                onRelease();
              }}
              disabled={pending}
            >
              {pending ? "Releasing…" : `I understand — release and mark ${modName} as unknown`}
            </button>
            <button type="button" onClick={() => setConfirmingRelease(false)} disabled={pending}>
              Keep waiting
            </button>
          </div>
        ) : (
          <button type="button" onClick={() => setConfirmingRelease(true)} disabled={pending}>
            Give up on this recovery…
          </button>
        )
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
