import type { ImporterEvacuationRecovery } from "./api";

/** A Model Importer evacuation that automatic startup rollback could not settle (#227). */
export function ImporterEvacuationRecoveryWarning({
  displayName,
  recovery,
  pending,
  error,
  onRetry,
  onRetire,
}: {
  displayName: string;
  recovery: ImporterEvacuationRecovery;
  pending: boolean;
  error?: string;
  onRetry: () => void;
  onRetire: () => void;
}) {
  return (
    <section
      className="reinstall-recovery-warning"
      aria-label={`Interrupted Model Importer recovery for ${displayName}`}
    >
      <strong>Unavailable — Model Importer recovery is still pending</strong>
      <p>
        GMM recorded the backup location before moving any Model Importer files, but could not
        finish restoring every evacuated entry. The game directory may still contain only part of
        its previous Model Importer.
        {recovery.ownerUncertain
          ? " GMM cannot retry while the original producer may still be running."
          : " Fix the problem described below, then retry the recorded rollback here."}
      </p>
      <p>
        Until recovery succeeds, GMM blocks launching {displayName} and blocks another importer
        install or rollback so neither location can silently become authoritative.
      </p>
      {recovery.ownerUncertain ? (
        <>
          <p>
            Close any other GMM instance, then retire the producer only after confirming no GMM is
            changing this Model Importer.
          </p>
          <button type="button" onClick={onRetire} disabled={pending}>
            {pending
              ? "Retiring producer…"
              : "I confirmed no other GMM is changing this importer — retire producer"}
          </button>
          {error ? <span className="error">Could not retire the producer: {error}</span> : null}
        </>
      ) : (
        <>
          <button type="button" onClick={onRetry} disabled={pending}>
            {pending ? "Retrying recovery…" : "Retry Model Importer recovery"}
          </button>
          {error ? <span className="error">Could not recover the Model Importer: {error}</span> : null}
        </>
      )}
      <details>
        <summary>Recorded paths and recovery status</summary>
        <dl>
          <dt>Game directory</dt>
          <dd><code>{recovery.gamePath}</code></dd>
          <dt>Backup directory</dt>
          <dd><code>{recovery.backupPath}</code></dd>
          <dt>{recovery.ownerUncertain ? "Recovery status" : "Last recovery error"}</dt>
          <dd>{recovery.reason}</dd>
        </dl>
      </details>
    </section>
  );
}
