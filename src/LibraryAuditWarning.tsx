import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import {
  auditLibrary,
  deleteUnreferencedLibraryDir,
  recoverUnreferencedLibraryDir,
  revealUnreferencedLibraryDir,
  type DeletedLibraryDir,
  type GameCode,
  type UnreferencedLibraryDir,
} from "./api";

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

/** Which folder, if any, has an open recover form or delete confirmation. */
type OpenAction = { path: string; kind: "recover" | "delete" } | null;

/**
 * The Settings report for Library directories with no Mod row, and the
 * three things a user can do about one (#72).
 *
 * Every action is per folder and explicitly chosen. There is deliberately
 * no "delete all": a bulk button is a bulk confirmation, and this is the
 * one surface in GMM that destroys Library bytes (ADR 0003 otherwise keeps
 * the Library untouched by everything else). The real safety here is that
 * Inspect and Recover sit beside Delete, so a user with something valuable
 * in a folder never has to reach for the destructive option.
 */
export function LibraryAuditWarning({ game }: { game: GameCode }) {
  const qc = useQueryClient();
  const [open, setOpen] = useState<OpenAction>(null);
  const [reclamation, setReclamation] = useState<DeletedLibraryDir | null>(null);
  useEffect(() => {
    setOpen(null);
    setReclamation(null);
  }, [game]);

  const audit = useQuery({
    queryKey: ["libraryAudit", game],
    queryFn: () => auditLibrary(game),
    retry: false,
  });

  const refresh = () => {
    setOpen(null);
    void qc.invalidateQueries({ queryKey: ["libraryAudit", game] });
    void qc.invalidateQueries({ queryKey: ["mods", game] });
  };

  const reveal = useMutation({
    mutationFn: (path: string) => revealUnreferencedLibraryDir(game, path),
  });
  const recover = useMutation({
    mutationFn: (args: { path: string; name: string }) =>
      recoverUnreferencedLibraryDir(game, args.path, args.name),
    onSuccess: () => {
      setReclamation(null);
      refresh();
    },
  });
  const remove = useMutation({
    mutationFn: (path: string) => deleteUnreferencedLibraryDir(game, path),
    onSuccess: (deleted) => {
      setReclamation(
        deleted.reclamationDeferred || deleted.reclamationFailed ? deleted : null,
      );
      refresh();
    },
  });

  const report = audit.data;
  if (!report) return null;

  const failure = reveal.error ?? recover.error ?? remove.error;
  const busy = reveal.isPending || recover.isPending || remove.isPending;
  const count = report.unreferenced.length;
  if (count === 0) {
    return reclamation ? (
      <section className="library-audit-warning" aria-label="Unreferenced Library folders">
        <ReclamationNotice reclamation={reclamation} />
      </section>
    ) : null;
  }

  // A region, not the `role="status"` live region #70 used: that was right
  // for a passive report, but a live region re-announces its whole contents
  // on every change, and this one now contains a text field and a
  // confirmation the user is interacting with.
  return (
    <section className="library-audit-warning" aria-label="Unreferenced Library folders">
      <ReclamationNotice reclamation={reclamation} />
      {failure ? (
        <p className="error" role="alert">
          {String(failure)}
        </p>
      ) : null}
      <strong>
        {count} unreferenced Library {count === 1 ? "folder" : "folders"} using{" "}
        {formatBytes(report.totalBytes)}
      </strong>
      <p className="muted small">
        These are most likely imports GMM was interrupted partway through. Look inside one
        before you decide: recovering adds it to your Library as a Mod without moving or
        copying anything, and deleting is permanent.
      </p>
      <ul>
        {report.unreferenced.map((directory) => (
          <li key={directory.path}>
            <code>{directory.path}</code>
            {directory.sizeBytes === null ? null : (
              <span className="muted small"> · {formatBytes(directory.sizeBytes)}</span>
            )}
            <div className="row">
              <button
                type="button"
                disabled={busy}
                onClick={() => reveal.mutate(directory.path)}
              >
                Inspect
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={() => setOpen({ path: directory.path, kind: "recover" })}
              >
                Recover…
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={() => setOpen({ path: directory.path, kind: "delete" })}
              >
                Delete…
              </button>
            </div>
            {open?.path === directory.path && open.kind === "recover" ? (
              <RecoverForm
                pending={recover.isPending}
                onCancel={() => setOpen(null)}
                onRecover={(name) => recover.mutate({ path: directory.path, name })}
              />
            ) : null}
            {open?.path === directory.path && open.kind === "delete" ? (
              <DeleteConfirmation
                directory={directory}
                pending={remove.isPending}
                onCancel={() => setOpen(null)}
                onDelete={() => remove.mutate(directory.path)}
              />
            ) : null}
          </li>
        ))}
      </ul>
    </section>
  );
}

function ReclamationNotice({ reclamation }: { reclamation: DeletedLibraryDir | null }) {
  if (reclamation?.reclamationDeferred && reclamation.reclamationPath) {
    return (
      <p className="muted small">
        The folder left the Library, but its disk space has not been reclaimed. Its bytes
        remain at {reclamation.reclamationPath}. GMM will retry at startup.
      </p>
    );
  }
  if (reclamation?.reclamationFailed && reclamation.reclamationPath) {
    return (
      <p className="error">
        The folder left the Library, but its disk space was not reclaimed. The quarantine at{" "}
        {reclamation.reclamationPath} changed, so GMM will not retry; the moved bytes need
        manual cleanup.
      </p>
    );
  }
  return null;
}

/**
 * GMM asks for the name rather than guessing one. Nothing on disk records
 * what the interrupted import was called, so any name GMM produced would be
 * invented and shown as a recovered fact.
 */
function RecoverForm({
  pending,
  onCancel,
  onRecover,
}: {
  pending: boolean;
  onCancel: () => void;
  onRecover: (name: string) => void;
}) {
  const [name, setName] = useState("");
  const trimmed = name.trim();
  return (
    <form
      className="row"
      onSubmit={(e) => {
        e.preventDefault();
        if (trimmed) onRecover(trimmed);
      }}
    >
      <label>
        Name
        <input
          autoFocus
          value={name}
          placeholder="What is this mod called?"
          onChange={(e) => setName(e.target.value)}
        />
      </label>
      <button type="submit" disabled={pending || trimmed === ""}>
        Recover
      </button>
      <button type="button" onClick={onCancel} disabled={pending}>
        Cancel
      </button>
    </form>
  );
}

/**
 * A plain confirmation naming the one folder and its size — no
 * type-to-confirm. Ceremony trains people to type through prompts; it earns
 * its place against bulk or ambiguous destruction, and this is neither.
 */
function DeleteConfirmation({
  directory,
  pending,
  onCancel,
  onDelete,
}: {
  directory: UnreferencedLibraryDir;
  pending: boolean;
  onCancel: () => void;
  onDelete: () => void;
}) {
  return (
    <div className="row" role="alertdialog" aria-modal="false" aria-label="Confirm delete">
      <p>
        Permanently delete <code>{directory.directoryName}</code>
        {directory.sizeBytes === null ? null : (
          <> and the {formatBytes(directory.sizeBytes)} inside it</>
        )}
        ? This cannot be undone.
      </p>
      <button type="button" onClick={onDelete} disabled={pending}>
        Delete
      </button>
      {/* The safe option takes focus, so Enter cancels rather than deletes. */}
      <button type="button" autoFocus onClick={onCancel} disabled={pending}>
        Cancel
      </button>
    </div>
  );
}
