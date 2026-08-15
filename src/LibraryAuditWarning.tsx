import { useQuery } from "@tanstack/react-query";

import { auditLibrary, type GameCode } from "./api";

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = -1;
  do {
    value /= 1024;
    unit += 1;
  } while (value >= 1024 && unit < units.length - 1);
  const digits = value >= 10 ? 0 : 1;
  return `${value.toFixed(digits)} ${units[unit]}`;
}

/**
 * Passive Settings warning for Library directories with no Mod row.
 * This issue is intentionally read-only; actions belong to #72.
 */
export function LibraryAuditWarning({ game }: { game: GameCode }) {
  const audit = useQuery({
    queryKey: ["libraryAudit", game],
    queryFn: () => auditLibrary(game),
    retry: false,
  });
  const report = audit.data;

  if (!report || report.unreferenced.length === 0) return null;

  const count = report.unreferenced.length;
  return (
    <div className="library-audit-warning" role="status">
      <strong>
        {count} unreferenced Library {count === 1 ? "folder" : "folders"} using{" "}
        {formatBytes(report.totalBytes)}
      </strong>
      <p className="muted small">
        These may be interrupted imports. GMM only reports them here; their files are untouched.
      </p>
      <ul>
        {report.unreferenced.map((directory) => (
          <li key={directory.path}>
            <code>{directory.path}</code>
            {directory.sizeBytes === null ? null : (
              <span className="muted small"> · {formatBytes(directory.sizeBytes)}</span>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}
