import { useQuery } from "@tanstack/react-query";

import { checkLoaderUpdate } from "./api";

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
 */
export function LoaderVersionNote() {
  const loader = useQuery({
    queryKey: ["loader", "version"],
    queryFn: () => checkLoaderUpdate(),
    retry: false,
  });

  const status = loader.data;
  if (!status) return null;

  const shipped = (
    <>
      GMM ships Loader <code>3dmloader.dll</code> <code>{status.shippedVersion}</code>
    </>
  );

  return (
    <p className="muted small" role="status">
      {status.checkError ? (
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
