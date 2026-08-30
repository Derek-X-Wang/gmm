import type { LibraryRootOverlap } from "./api";

/** A persisted Library root that GMM refuses to use until Settings repairs it. */
export function LibraryRootOverlapWarning({
  overlaps,
}: {
  overlaps: LibraryRootOverlap[];
}) {
  if (overlaps.length === 0) return null;

  return (
    <section
      className="reinstall-recovery-warning"
      aria-label="Unsafe Library path configuration"
    >
      <strong>Library path needs attention</strong>
      <p>
        GMM will not use these Library roots because they overlap its Model Importer
        backup tree. Choose a different root that is outside the backup tree and does
        not contain it.
      </p>
      <p>
        When you change an unsafe root, GMM updates the setting without reading or
        moving anything from the overlapping path. Any files already there stay
        untouched so you can inspect and recover them manually.
      </p>
      <ul>
        {overlaps.map((overlap) => (
          <li key={`${overlap.game ?? "global"}:${overlap.path}`}>
            <strong>{overlap.game ? `${overlap.game.toUpperCase()} override` : "Global root"}</strong>
            <dl>
              <dt>Library path</dt>
              <dd><code>{overlap.path}</code></dd>
              <dt>Model Importer backup tree</dt>
              <dd><code>{overlap.backups}</code></dd>
            </dl>
          </li>
        ))}
      </ul>
    </section>
  );
}
