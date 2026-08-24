import type { ReinstallRecovery } from "./api";

export type ReinstallRecoveryFeedback =
  | { kind: "recovered"; modName: string }
  | { kind: "alreadyRecovered"; modName: string }
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
          : feedback?.kind === "alreadyRecovered"
            ? `Recovery had already completed for ${feedback.modName}. The Mod is usable again.`
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
      <strong>
        {recovery.junctionWithdrawn
          ? "Unavailable — interrupted reinstall needs recovery"
          : "Unavailable — this Mod may still be loading"}
      </strong>
      <p>
        The recovery witness says the old tree should be live, but GMM could not
        restore or verify it. GMM left every directory it could not prove untouched.
        {recovery.junctionWithdrawn ? (
          <> GMM withdrew the recorded deployment entry and will not recreate it until
          recovery succeeds.</>
        ) : (
          <> GMM could not withdraw the recorded deployment entry, so this Mod may still
          be loading in the game. Withdrawal failed because: {recovery.junctionWithdrawalError
            ?? "the previous attempt ended before withdrawal was confirmed"}.</>
        )}{" "}
        Your enabled or disabled choice is unchanged, and ordinary cleanup will not
        discard either recorded byte identity.
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
