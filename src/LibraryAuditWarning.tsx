import { useEffect, useId, useRef, useState, type RefObject } from "react";
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
type OpenAction = {
  path: string;
  kind: "recover" | "delete";
  trigger: HTMLButtonElement;
} | null;

type ActionFeedback =
  | { kind: "recovered"; directoryName: string; name: string }
  | { kind: "deleted"; deleted: DeletedLibraryDir };

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
  const [feedback, setFeedback] = useState<ActionFeedback | null>(null);
  const feedbackRef = useRef<HTMLParagraphElement>(null);
  useEffect(() => {
    setOpen(null);
    setFeedback(null);
  }, [game]);
  useEffect(() => {
    if (feedback) feedbackRef.current?.focus();
  }, [feedback]);

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
    mutationFn: (args: { path: string; directoryName: string; name: string }) =>
      recoverUnreferencedLibraryDir(game, args.path, args.name),
    onSuccess: (recovered, args) => {
      setFeedback({
        kind: "recovered",
        directoryName: args.directoryName,
        name: recovered.name,
      });
      refresh();
    },
  });
  const remove = useMutation({
    mutationFn: (path: string) => deleteUnreferencedLibraryDir(game, path),
    onSuccess: (deleted) => {
      setFeedback({ kind: "deleted", deleted });
      refresh();
    },
  });

  const beginAction = (
    path: string,
    kind: "recover" | "delete",
    trigger: HTMLButtonElement,
  ) => {
    setFeedback(null);
    setOpen({ path, kind, trigger });
  };
  const cancelAction = () => {
    const trigger = open?.trigger;
    setOpen(null);
    trigger?.focus();
  };

  const report = audit.data;
  if (!report) return null;

  const failure = reveal.error ?? recover.error ?? remove.error;
  const busy = reveal.isPending || recover.isPending || remove.isPending;
  const count = report.unreferenced.length;
  if (count === 0) {
    return feedback ? (
      <section className="library-audit-warning" aria-label="Unreferenced Library folders">
        <ActionNotice feedback={feedback} focusRef={feedbackRef} />
      </section>
    ) : null;
  }

  // A region, not the `role="status"` live region #70 used: that was right
  // for a passive report, but a live region re-announces its whole contents
  // on every change, and this one now contains a text field and a
  // confirmation the user is interacting with.
  return (
    <section className="library-audit-warning" aria-label="Unreferenced Library folders">
      <ActionNotice feedback={feedback} focusRef={feedbackRef} />
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
                onClick={(event) =>
                  beginAction(directory.path, "recover", event.currentTarget)
                }
              >
                Recover…
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={(event) =>
                  beginAction(directory.path, "delete", event.currentTarget)
                }
              >
                Delete…
              </button>
            </div>
            {open?.path === directory.path && open.kind === "recover" ? (
              <RecoverForm
                pending={recover.isPending}
                onCancel={cancelAction}
                onRecover={(name) =>
                  recover.mutate({
                    path: directory.path,
                    directoryName: directory.directoryName,
                    name,
                  })
                }
              />
            ) : null}
            {open?.path === directory.path && open.kind === "delete" ? (
              <DeleteConfirmation
                directory={directory}
                pending={remove.isPending}
                onCancel={cancelAction}
                onDelete={() => remove.mutate(directory.path)}
              />
            ) : null}
          </li>
        ))}
      </ul>
    </section>
  );
}

function ActionNotice({
  feedback,
  focusRef,
}: {
  feedback: ActionFeedback | null;
  focusRef: RefObject<HTMLParagraphElement | null>;
}) {
  if (!feedback) return null;
  if (feedback.kind === "recovered") {
    return (
      <p ref={focusRef} className="muted small" role="status" tabIndex={-1}>
        Recovered <code>{feedback.directoryName}</code> as {feedback.name}.
      </p>
    );
  }

  const { deleted } = feedback;
  const outcome = deleted.reclamation;
  if (outcome.status === "reclaimed") {
    return (
      <p ref={focusRef} className="muted small" role="status" tabIndex={-1}>
        Deleted <code>{deleted.directoryName}</code>
        {deleted.sizeBytes === null ? (
          <>. Its disk space was reclaimed, but the freed size is unknown.</>
        ) : (
          <> and freed {formatBytes(deleted.sizeBytes)}.</>
        )}
      </p>
    );
  }
  if (outcome.status === "deferred") {
    return (
      <p ref={focusRef} className="muted small" role="status" tabIndex={-1}>
        GMM removed {deleted.path} from the Library, but could not reclaim its disk
        space now. Its bytes remain at {outcome.path}. GMM will retry during a later
        startup while it can still verify that directory at its reserved name.
      </p>
    );
  }
  if (outcome.status === "ownershipLost") {
    return (
      <p ref={focusRef} className="error" role="alert" tabIndex={-1}>
        GMM removed {deleted.path} from the Library, but could not confirm whether its
        disk space was reclaimed. GMM does not know whether any of that folder&apos;s bytes
        remain or, if they do, where they are. If GMM can again verify the original
        directory at its reserved name, a later startup will retry reclamation.
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
  const descriptionId = useId();
  return (
    <div
      className="row"
      role="group"
      aria-label="Confirm delete"
      aria-describedby={descriptionId}
    >
      <p id={descriptionId}>
        Permanently delete <code>{directory.directoryName}</code>?{" "}
        {directory.sizeBytes === null ? (
          <>Its size is unknown. </>
        ) : (
          <>This will delete the {formatBytes(directory.sizeBytes)} inside it. </>
        )}
        This cannot be undone.
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
