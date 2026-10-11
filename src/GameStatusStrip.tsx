import { useQuery } from "@tanstack/react-query";
import { checkImporterUpdate, getGamePathStatus, type GameCode, type GamePathStatus } from "./api";
import { commandFailureMessage } from "./commandError";

function pathSummary(value: GamePathStatus): { line: string; detail?: string } {
  const path = value.gamePath;
  switch (path.status) {
    case "unreadable": return { line: "Cannot tell — install path unreadable · Model Importer unverified", detail: `${path.path}: ${path.error}` };
    case "savedPathNotFound": return { line: "Saved installation cannot be found · Model Importer unverified", detail: path.paths.join(", ") };
    case "invalid": return { line: "Cannot verify Game layout · Model Importer unverified", detail: path.error };
    case "noSavedPath": return { line: "No saved install path · Model Importer unverified" };
    case "readable":
      if (value.modelImporter.status === "requiredEntriesNotFound") {
        return { line: "Game readable · Model Importer not installed", detail: value.modelImporter.paths.join(", ") };
      }
      if (value.modelImporter.status === "unverified") {
        return { line: "Game readable · Model Importer unverified", detail: value.modelImporter.error };
      }
      return { line: "Game readable · Model Importer healthy" };
  }
}

export function GameStatusStrip({ game, displayName, savedPath, pathReadError, pathReadPending, onSettings }: {
  game: GameCode;
  displayName: string;
  savedPath: string | null;
  pathReadError: unknown;
  pathReadPending: boolean;
  onSettings: () => void;
}) {
  const status = useQuery({ queryKey: ["gamePathStatus", game], queryFn: () => getGamePathStatus(game) });
  const update = useQuery({
    queryKey: ["importer", "update", game],
    queryFn: () => checkImporterUpdate(game),
    // Each upstream attempt can take ten seconds; retries would delay the unavailable state.
    retry: false,
    // Avoid a network refetch on ordinary tab/Settings returns; invalidation still refreshes.
    staleTime: 5 * 60_000,
  });
  const error = pathReadError ?? status.error;
  const pending = pathReadPending || status.isFetching;
  const summary = error ? { line: pathReadError != null
    ? "Cannot tell — install path unreadable · Model Importer unverified"
    : "Cannot tell — path check unavailable · Model Importer unverified", detail: commandFailureMessage(error) }
    : pending ? { line: "Checking Game install path · Model Importer unverified" }
    : status.data ? pathSummary(status.data) : { line: "Cannot tell — path check unavailable · Model Importer unverified" };
  const path = savedPath ?? status.data?.savedPath;
  const installed = !error && !pending && status.data?.gamePath.status === "readable"
    && status.data.modelImporter.status === "installed";
  const absent = !error && !pending && status.data?.gamePath.status === "readable"
    && status.data.modelImporter.status === "requiredEntriesNotFound";
  const metadataVerified = !update.isError && update.data != null;
  const previousMetadata = metadataVerified && update.isFetching;
  const metadataPending = update.isPending || update.isFetching;
  const version = installed && metadataVerified ? update.data?.installedVersion : null;
  return <section className="game-status-strip" aria-label={`${displayName} status`} aria-busy={pending}>
    <div className="game-status-strip__summary" role="status">
      <div className="row">
      <strong>Model Importer ({game.toUpperCase()})</strong>
      <span>{summary.line}</span>
      {absent ? <span className="muted small">Unhealthy</span> : null}
      {version ? <span className="muted small">{previousMetadata ? "Previous: Installed version: " : "Installed version: "}<code>{version}</code></span> : null}
      {installed && metadataVerified && update.data?.pinned ? <span className="muted small" aria-label="Importer Pin applies">{previousMetadata ? "Previous: Pinned" : "Pinned"}</span> : null}
      {installed && !version ? <span className="muted small">{update.isError
        ? `Installed version check unavailable: ${commandFailureMessage(update.error)}`
        : metadataVerified ? `${previousMetadata ? "Previous: " : ""}Installed version unknown (not recorded).`
        : metadataPending ? "Checking installed version and Importer Pin…"
        : "Installed version check unavailable."}</span> : null}
      </div>
      {path ? <span className="muted small">{pending || error ? "Last saved path: " : "Saved path: "}<code>{path}</code></span> : null}
      {summary.detail ? <span className="muted small">{summary.detail}</span> : null}
    </div>
    <button data-settings-arrival="game" onClick={onSettings}>Settings for {displayName}</button>
  </section>;
}
