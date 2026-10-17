import { useQuery } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { getGamePathStatus, modelImporterDetails, type GameCode, type GamePathStatus } from "./api";
import { commandFailureMessage } from "./commandError";

function pathSummary(value: GamePathStatus): { line: string; detail?: ReactNode } {
  const path = value.gamePath;
  switch (path.status) {
    case "unreadable": return { line: "Cannot tell — install path unreadable · Model Importer unverified", detail: `${path.path}: ${path.error}` };
    case "savedPathAtWrongLevel": return { line: "Saved Game path is at the wrong level · Model Importer unverified", detail: `Choose the Game folder ending in ${path.expectedSuffix}.` };
    case "savedPathNotFound": return { line: "Saved installation cannot be found · Model Importer unverified", detail: path.paths.join(", ") };
    case "requiredEntriesNotFound": return { line: "Required Game files or directories not found · Model Importer unverified", detail: <><span role="img" aria-label="Warning">⚠</span> Missing Game entries</> };
    case "invalid": return { line: "Cannot verify Game layout · Model Importer unverified", detail: path.error };
    case "noSavedPath": return { line: "No saved install path · Model Importer unverified" };
    case "readable":
      switch (value.modelImporter.status) {
        case "installed": return { line: "Game readable · Model Importer healthy" };
        case "damaged": return { line: "Game readable · Model Importer damaged", detail: modelImporterDetails(value.modelImporter) };
        case "noImporterEntriesFound": return { line: "Cannot tell — no GMM install recorded; no Model Importer entries found", detail: modelImporterDetails(value.modelImporter) };
        case "unrecordedImporterPresent": return { line: "Cannot tell — Model Importer files found; no GMM install recorded", detail: modelImporterDetails(value.modelImporter) };
        case "unverified": return { line: "Game readable · Model Importer unverified", detail: modelImporterDetails(value.modelImporter) };
      }
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
  const installed = !error && !pending && status.data?.gamePath.status === "readable"
    && status.data.modelImporter.status === "installed";
  const importer = status.data?.modelImporter;
  const record = importer && "record" in importer ? importer.record : null;
  const version = installed ? record?.installedVersion : null;
  return <section className="game-status-strip" aria-label={`${displayName} status`} aria-busy={pending}>
    <div className="game-status-strip__summary" role="status">
      <div className="row">
      <strong>Game and Model Importer ({game.toUpperCase()})</strong>
      <span>Game: {summary.line}</span>
      {version ? <span className="muted small">Installed version: <code>{version}</code></span> : null}
      {installed && status.data?.pinnedVersion != null ? <span className="muted small" aria-label="Importer Pin applies">Pinned</span> : null}
      {installed && !version ? <span className="muted small">Installed version unknown (not recorded).</span> : null}
      </div>
      {path ? <span className="muted small">{pending || error ? "Last saved Game path: " : "Saved Game path: "}<code>{path}</code></span> : null}
      {summary.detail ? <span className="muted small">{summary.detail}</span> : null}
    </div>
    <button data-settings-arrival="game" onClick={onSettings}>Settings for {displayName}</button>
  </section>;
}
