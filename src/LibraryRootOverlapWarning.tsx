import type { LibraryModPathOverlap, LibraryRootOverlap } from "./api";

/** Configured roots and recorded Mods that overlap importer backup storage. */
export function LibraryRootOverlapWarning({
  overlaps,
  modOverlaps,
}: {
  overlaps: LibraryRootOverlap[];
  modOverlaps: LibraryModPathOverlap[];
}) {
  if (overlaps.length === 0 && modOverlaps.length === 0) return null;

  return (
    <section
      className="reinstall-recovery-warning"
      aria-label="Unsafe Library paths"
    >
      <strong>Library path needs attention</strong>
      {overlaps.length > 0 ? (
        <>
          <p>
            GMM blocks Library-wide operations that resolve an overlapping configured
            root, including audit, import, and relocation. Enable, disable, and Junction
            reconciliation still use each Mod&apos;s recorded path, so those operations
            remain available during repair.
          </p>
          <p>
            When you change an unsafe root, GMM updates the setting without reading or
            moving anything from the overlapping path. Choose a different root outside
            the backup tree that does not contain it. Existing Mod records and folders
            stay untouched for manual recovery, and this warning remains until no Mod
            record points there.
          </p>
        </>
      ) : (
        <p>
          No in-app action clears these stranded Mod records. Move the named folders
          into the current Library root by hand and re-adopt them so GMM can use them
          again; the old records and this warning will remain.
        </p>
      )}
      {overlaps.length > 0 ? (
        <>
          <strong>Configured roots</strong>
          <ul>
            {overlaps.map((overlap) => (
              <li key={`${overlap.game ?? "global"}:${overlap.path}`}>
                <strong>
                  {overlap.game
                    ? `${overlap.game.toUpperCase()} configured root`
                    : "Global configured root"}
                </strong>
                <dl>
                  <dt>Library path</dt>
                  <dd><code>{overlap.path}</code></dd>
                  <dt>Model Importer backup tree</dt>
                  <dd><code>{overlap.backups}</code></dd>
                </dl>
              </li>
            ))}
          </ul>
        </>
      ) : null}
      {modOverlaps.length > 0 ? (
        <>
          <strong>Mods still recorded inside the backup tree</strong>
          <ul>
            {modOverlaps.map((overlap) => (
              <li key={overlap.modId}>
                <strong>{overlap.modName}</strong>{" "}
                <code>{overlap.game}</code> · <code>{overlap.modId}</code>
                <dl>
                  <dt>Recorded Mod path</dt>
                  <dd><code>{overlap.path}</code></dd>
                  <dt>Model Importer backup tree</dt>
                  <dd><code>{overlap.backups}</code></dd>
                </dl>
              </li>
            ))}
          </ul>
        </>
      ) : null}
    </section>
  );
}
