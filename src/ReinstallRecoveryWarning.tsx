import type { ReinstallRecovery } from "./api";

export type ReinstallRecoveryFeedback =
  | { kind: "recovered"; modName: string }
  | { kind: "stillQuarantined"; modName: string; reason: string }
  | null;

/**
 * Retry feedback lives outside the Mod row because a successful retry removes
 * the callout. Both live regions are mounted before their text changes, and
 * callers move focus to the containing Mods section rather than either region.
 */
export function ReinstallRecoveryNotices({
  feedback,
}: {
  feedback: ReinstallRecoveryFeedback;
}) {
  return (
    <>
      <div className="reinstall-action-notice muted small" role="status">
        {feedback?.kind === "recovered"
          ? `Recovered the interrupted reinstall for ${feedback.modName}. The Mod is usable again.`
          : null}
      </div>
      <div className="reinstall-action-notice error" role="alert">
        {feedback?.kind === "stillQuarantined"
          ? `Recovery still needs intervention for ${feedback.modName}: ${feedback.reason}`
          : null}
      </div>
    </>
  );
}

/** One Mod's durable, user-actionable reinstall quarantine (#179). */
export function ReinstallRecoveryWarning({
  modName,
  recovery,
  pending,
  disabled = false,
  onRetry,
}: {
  modName: string;
  recovery: ReinstallRecovery;
  pending: boolean;
  disabled?: boolean;
  onRetry: () => void;
}) {
  return (
    <section
      className="reinstall-recovery-warning"
      aria-label={`Interrupted reinstall recovery for ${modName}`}
    >
      <strong>Unavailable — interrupted reinstall needs recovery</strong>
      <p>
        GMM could not safely decide which reinstall tree should be live, so it
        left every directory it could not prove untouched and quarantined only
        this Mod. Its recovery witness still owns both recorded byte identities;
        ordinary cleanup will not discard either one.
      </p>
      <p>
        <strong>Retry may work:</strong> if a file was briefly locked or the
        Library device was unavailable, close the software using it or reconnect
        the device, then choose Retry recovery.
      </p>
      <p>
        <strong>This needs you:</strong> if you moved or deleted one of the paths
        below, restore it; if access is denied, fix that path&apos;s permissions.
        Then choose Retry recovery. GMM cannot reliably distinguish those causes
        from a temporary filesystem failure, so it will not guess.
      </p>
      <details>
        <summary>Recorded paths and recovery error</summary>
        <p className="muted small">
          A recorded path may currently be missing. These are evidence for
          recovery, not instructions to delete anything.
        </p>
        <dl>
          <dt>Mod path</dt>
          <dd><code>{recovery.libraryPath}</code></dd>
          <dt>Interrupted update path</dt>
          <dd><code>{recovery.stagedPath}</code></dd>
          <dt>Rollback path</dt>
          <dd><code>{recovery.quarantinePath}</code></dd>
          <dt>Last recovery error</dt>
          <dd>{recovery.reason}</dd>
        </dl>
      </details>
      <button type="button" onClick={onRetry} disabled={pending || disabled}>
        {pending ? "Retrying recovery…" : "Retry recovery"}
      </button>
    </section>
  );
}
