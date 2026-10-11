import { useQuery } from "@tanstack/react-query";
import { getGamePathStatus, type GameCode, type GamePathStatus } from "./api";
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
      // The detailed importer name, health, verified version and pin belong to #297.
      return { line: "Game readable · Model Importer details in Settings" };
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
  const error = pathReadError ?? status.error;
  const pending = pathReadPending || status.isFetching;
  const summary = error ? { line: pathReadError != null
    ? "Cannot tell — install path unreadable · Model Importer unverified"
    : "Cannot tell — path check unavailable · Model Importer unverified", detail: commandFailureMessage(error) }
    : pending ? { line: "Checking Game install path · Model Importer unverified" }
    : status.data ? pathSummary(status.data) : { line: "Cannot tell — path check unavailable · Model Importer unverified" };
  const path = savedPath ?? status.data?.savedPath;
  return <section className="game-status-strip" aria-label={`${displayName} status`} aria-busy={pending}>
    <div className="game-status-strip__summary" role="status">
      <span>{summary.line}</span>
      {path ? <span className="muted small">{pending || error ? "Last saved path: " : "Saved path: "}<code>{path}</code></span> : null}
      {summary.detail ? <span className="muted small">{summary.detail}</span> : null}
    </div>
    <button data-settings-arrival="game" onClick={onSettings}>Settings for {displayName}</button>
  </section>;
}
