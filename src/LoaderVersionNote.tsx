import { useQuery } from "@tanstack/react-query";

import { checkLoaderUpdate } from "./api";
import { evidenceDetail, readEvidence } from "./queryEvidence";

/**
 * Informational Loader version report.
 *
 * The Loader (`3dmloader.dll`) is embedded in GMM via FFI rather than
 * installed (ADR 0001), so there is nothing here for a user to apply.
 * This note exists to state the relationship honestly — what GMM
 * ships, what upstream has published, and when we could not find out.
 *
 * Before #78 this surface claimed a "loader update" was available and
 * told users to re-run the importer install to get it, which pulls a
 * Model Importer package and never touches the Loader. It also
 * rendered a failed check exactly like a healthy one.
 *
 * "Up to date with upstream" is only said about a check GMM just made. When
 * the command fails or its refresh is paused, the last result is kept but is
 * no longer a statement about upstream, so the note says it could not confirm.
 */
export function LoaderVersionNote() {
  const loader = useQuery({
    queryKey: ["loader", "version"],
    queryFn: () => checkLoaderUpdate(),
    retry: false,
  });

  const evidence = readEvidence(loader);
  if (evidence.kind === "pending") return null;
  if (evidence.kind === "unavailable") {
    return (
      <p className="muted small" role="status">
        GMM could not check the Loader version. {evidenceDetail(evidence)}
      </p>
    );
  }
  const status = evidence.value;
  // The command reports nothing GMM can state: say nothing rather than guess.
  if (!status) return null;

  const shipped = (
    <>
      GMM ships Loader <code>3dmloader.dll</code> <code>{status.shippedVersion}</code>
    </>
  );

  return (
    <p className="muted small" role="status">
      {evidence.kind === "stale" ? (
        <>
          {shipped}. GMM could not confirm whether upstream has a newer Loader, because the
          last check could not be refreshed.
        </>
      ) : status.checkError ? (
        <>
          {shipped}. Couldn't check upstream: {status.checkError}
        </>
      ) : status.upstreamAhead ? (
        <>
          {shipped}; upstream's latest is <code>{status.latestVersion}</code>. The Loader is
          built into GMM, so a newer one arrives with a GMM update — there is nothing to
          install here.
        </>
      ) : (
        <>{shipped}, which is up to date with upstream.</>
      )}
    </p>
  );
}
