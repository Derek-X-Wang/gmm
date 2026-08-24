import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import {
  auditLibrary,
  deleteUnreferencedLibraryDir,
  recoverUnreferencedLibraryDir,
  revealUnreferencedLibraryDir,
  resolveDuplicateMods,
  type DeletedLibraryDir,
  type DuplicateModGroup,
  type DuplicateModRecord,
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
  kind: "recover" | "delete" | "resolve-duplicates";
  trigger: HTMLButtonElement;
} | null;

type ActionFeedback =
  | { kind: "recovered"; directoryName: string; name: string }
  | { kind: "deleted"; deleted: DeletedLibraryDir }
  | { kind: "duplicatesResolved"; keeperName: string; removedCount: number };

/**
 * The Settings report for Library directories with no Mod row, and the
 * three things a user can do about one (#72), plus multiple Mod rows that
 * resolve to the same directory (#185).
 *
 * Every action is per folder and explicitly chosen. There is deliberately
 * no "delete all": a bulk button is a bulk confirmation, and this is the
 * one surface in GMM that destroys Library bytes (ADR 0003 otherwise keeps
 * the Library untouched by everything else). The real safety here is that
 * Inspect and Recover sit beside Delete, so a user with something valuable
 * in a folder never has to reach for the destructive option.
 *
 * Duplicate rows are similarly preserve-first: every record is shown, no
 * keeper is preselected, and the confirmation removes database state only
 * after the user has reviewed and selected the record to retain. Shared
 * Library bytes are never removed by duplicate resolution.
 */
export function LibraryAuditWarning({ game }: { game: GameCode }) {
  const qc = useQueryClient();
  const [open, setOpen] = useState<OpenAction>(null);
  const [feedback, setFeedback] = useState<ActionFeedback | null>(null);
  const [keepers, setKeepers] = useState<Record<string, string>>({});
  const reportRef = useRef<HTMLElement>(null);
  useEffect(() => {
    setOpen(null);
    setFeedback(null);
    setKeepers({});
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
  const publishFeedback = (nextFeedback: ActionFeedback) => {
    // Keep focus out of the disappearing action controls, but let the
    // already-mounted live region — not focus — announce the outcome.
    reportRef.current?.focus();
    setFeedback(nextFeedback);
    refresh();
  };

  const reveal = useMutation({
    mutationFn: (path: string) => revealUnreferencedLibraryDir(game, path),
  });
  const recover = useMutation({
    mutationFn: (args: { path: string; directoryName: string; name: string }) =>
      recoverUnreferencedLibraryDir(game, args.path, args.name),
    onSuccess: (recovered, args) => {
      publishFeedback({
        kind: "recovered",
        directoryName: args.directoryName,
        name: recovered.name,
      });
    },
  });
  const remove = useMutation({
    mutationFn: (path: string) => deleteUnreferencedLibraryDir(game, path),
    onSuccess: (deleted) => {
      publishFeedback({ kind: "deleted", deleted });
    },
  });
  const resolveDuplicates = useMutation({
    mutationFn: (args: {
      keeperId: string;
      keeperName: string;
      reviewedMods: Array<{ id: string; fingerprint: string }>;
    }) => resolveDuplicateMods(args.keeperId, args.reviewedMods),
    onSuccess: (resolution, args) => {
      publishFeedback({
        kind: "duplicatesResolved",
        keeperName: args.keeperName,
        removedCount: resolution.removedModIds.length,
      });
    },
  });

  const beginAction = (
    path: string,
    kind: "recover" | "delete" | "resolve-duplicates",
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

  const failure = reveal.error ?? recover.error ?? remove.error ?? resolveDuplicates.error;
  const busy =
    reveal.isPending || recover.isPending || remove.isPending || resolveDuplicates.isPending;
  const count = report.unreferenced.length;
  const duplicates = report.duplicates ?? [];
  if (count === 0 && duplicates.length === 0) {
    return feedback ? (
      <section
        ref={reportRef}
        className="library-audit-warning"
        aria-label="Unreferenced Library folders and duplicate Mod records"
        tabIndex={-1}
      >
        <ActionNotice feedback={feedback} />
      </section>
    ) : null;
  }

  // A region, not the `role="status"` live region #70 used: that was right
  // for a passive report, but a live region re-announces its whole contents
  // on every change, and this one now contains a text field and a
  // confirmation the user is interacting with.
  return (
    <section
      ref={reportRef}
      className="library-audit-warning"
      aria-label="Unreferenced Library folders and duplicate Mod records"
      tabIndex={-1}
    >
      <ActionNotice feedback={feedback} />
      <p className="error" role="alert" aria-label="Library action failed">
        {failure ? String(failure) : null}
      </p>
      {count > 0 ? (
        <>
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
        </>
      ) : null}
      {duplicates.length > 0 ? (
        <div className="duplicate-mod-report">
          <strong>
            {duplicates.length} Library {duplicates.length === 1 ? "directory has" : "directories have"}{" "}
            duplicate Mod records
          </strong>
          <p className="muted small">
            GMM found multiple records for the same on-disk directory. Review every field, then
            choose the one record that actually represents those bytes. GMM never chooses for you.
          </p>
          <ul>
            {duplicates.map((group) => {
              const keeperId = keepers[group.path];
              const keeper = group.mods.find((mod) => mod.id === keeperId);
              const blocked = group.mods.some((mod) => mod.reinstallInProgress);
              return (
                <li key={group.path}>
                  <DuplicateGroup
                    group={group}
                    keeperId={keeperId}
                    blocked={blocked}
                    busy={busy}
                    onKeeperChange={(id) =>
                      setKeepers((current) => ({ ...current, [group.path]: id }))
                    }
                    onBegin={(trigger) =>
                      beginAction(group.path, "resolve-duplicates", trigger)
                    }
                  />
                  {open?.path === group.path && open.kind === "resolve-duplicates" && keeper ? (
                    <DuplicateConfirmation
                      keeper={keeper}
                      rejectedCount={group.mods.length - 1}
                      pending={resolveDuplicates.isPending}
                      onCancel={cancelAction}
                      onResolve={() =>
                        resolveDuplicates.mutate({
                          keeperId: keeper.id,
                          keeperName: keeper.name,
                          reviewedMods: group.mods.map(({ id, fingerprint }) => ({
                            id,
                            fingerprint,
                          })),
                        })
                      }
                    />
                  ) : null}
                </li>
              );
            })}
          </ul>
        </div>
      ) : null}
    </section>
  );
}

function ActionNotice({ feedback }: { feedback: ActionFeedback | null }) {
  let status: ReactNode = null;
  let alert: ReactNode = null;

  if (feedback?.kind === "recovered") {
    status = (
      <>
        Recovered <code>{feedback.directoryName}</code> as {feedback.name}.
      </>
    );
  } else if (feedback?.kind === "deleted") {
    const { deleted } = feedback;
    const outcome = deleted.reclamation;
    if (outcome.status === "reclaimed") {
      status = (
        <>
          Deleted <code>{deleted.directoryName}</code>
          {deleted.sizeBytes === null ? (
            <>. Its disk space was reclaimed, but the freed size is unknown.</>
          ) : (
            <> and freed {formatBytes(deleted.sizeBytes)}.</>
          )}
        </>
      );
    } else if (outcome.status === "deferred") {
      status = (
        <>
          GMM removed {deleted.path} from the Library, but could not reclaim its disk
          space now. Its bytes remain at {outcome.path}. GMM will retry during a later
          startup while it can still verify that directory at its reserved name.
        </>
      );
    } else if (outcome.status === "ownershipLost") {
      alert = (
        <>
          GMM removed {deleted.path} from the Library, but could not confirm whether its
          disk space was reclaimed. GMM does not know whether any of that folder&apos;s bytes
          remain or, if they do, where they are. If GMM can again verify the original
          directory at its reserved name, a later startup will retry reclamation.
        </>
      );
    }
  } else if (feedback?.kind === "duplicatesResolved") {
    status = (
      <>
        Kept {feedback.keeperName} as the only GMM record for this Library directory and
        removed {feedback.removedCount} rejected duplicate{" "}
        {feedback.removedCount === 1 ? "record" : "records"}. The shared Library bytes were
        left in place.
      </>
    );
  }

  // Both regions must exist before their text changes. Mounting a populated
  // live region is not announced reliably by every screen reader.
  return (
    <>
      <div className="action-notice muted small" role="status">
        {status}
      </div>
      <div className="action-notice error" role="alert" aria-label="Library cleanup warning">
        {alert}
      </div>
    </>
  );
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

function DuplicateGroup({
  group,
  keeperId,
  blocked,
  busy,
  onKeeperChange,
  onBegin,
}: {
  group: DuplicateModGroup;
  keeperId: string | undefined;
  blocked: boolean;
  busy: boolean;
  onKeeperChange: (id: string) => void;
  onBegin: (trigger: HTMLButtonElement) => void;
}) {
  return (
    <div>
      <p>
        Shared directory: <code>{group.path}</code>
      </p>
      <fieldset>
        <legend>Select the one Mod record to keep</legend>
        {group.mods.map((mod) => (
          <label key={mod.id} className="duplicate-mod-record">
            <span>
              <input
                type="radio"
                name={`duplicate-keeper-${group.path}`}
                value={mod.id}
                checked={keeperId === mod.id}
                onChange={() => onKeeperChange(mod.id)}
              />{" "}
              <strong>{mod.name}</strong> · {mod.game.toUpperCase()} ·{" "}
              {mod.enabled ? "Enabled" : "Disabled"}
            </span>
            <DuplicateModDetails mod={mod} />
          </label>
        ))}
      </fieldset>
      {blocked ? (
        <p className="error small">
          At least one record has an unfinished update. Let it settle first; if its Mod card
          shows a recovery warning, use Retry recovery there, then refresh this audit. GMM will
          not remove any record while its recovery witness exists.
        </p>
      ) : null}
      <button
        type="button"
        disabled={busy || blocked || keeperId === undefined}
        onClick={(event) => onBegin(event.currentTarget)}
      >
        Resolve…
      </button>
    </div>
  );
}

function DuplicateModDetails({ mod }: { mod: DuplicateModRecord }) {
  return (
    <dl className="duplicate-mod-details small">
      <div>
        <dt>Record ID</dt>
        <dd>
          <code>{mod.id}</code>
        </dd>
      </div>
      <div>
        <dt>Stored path</dt>
        <dd>
          <code>{mod.libraryPath}</code>
        </dd>
      </div>
      <div>
        <dt>Provenance</dt>
        <dd>{mod.source}</dd>
      </div>
      <div>
        <dt>GameBanana</dt>
        <dd>
          {mod.gamebananaId === null ? "None" : `submission ${mod.gamebananaId}`}
          {mod.sourceUrl ? (
            <>
              {" "}· <code>{mod.sourceUrl}</code>
            </>
          ) : null}
        </dd>
      </div>
      <div>
        <dt>Author / version</dt>
        <dd>
          {mod.author ?? "Unknown"} / {mod.version ?? "Unknown"}
        </dd>
      </div>
      <div>
        <dt>Last seen upstream</dt>
        <dd>{mod.upstreamVersion ?? "Unknown"}</dd>
      </div>
      <div>
        <dt>Update checks</dt>
        <dd>{mod.updateCheckEnabled ? "Enabled" : "Disabled"}</dd>
      </div>
      <div>
        <dt>Screenshot metadata</dt>
        <dd>{mod.screenshotUrl ? <code>{mod.screenshotUrl}</code> : "None"}</dd>
      </div>
      <div>
        <dt>Variants</dt>
        <dd>
          {mod.variants.length === 0 ? (
            "None"
          ) : (
            <ul>
              {mod.variants.map((variant) => (
                <li key={variant.id}>
                  {variant.name} (<code>{variant.subpath}</code>)
                  {variant.active ? " — selected" : ""}
                </li>
              ))}
            </ul>
          )}
        </dd>
      </div>
      <div>
        <dt>Junction name</dt>
        <dd>
          <code>{mod.junctionDirName}</code>
        </dd>
      </div>
      <div>
        <dt>Created</dt>
        <dd>{mod.createdAt}</dd>
      </div>
      <div>
        <dt>Unfinished update</dt>
        <dd>{mod.reinstallInProgress ? "Yes" : "No"}</dd>
      </div>
    </dl>
  );
}

function DuplicateConfirmation({
  keeper,
  rejectedCount,
  pending,
  onCancel,
  onResolve,
}: {
  keeper: DuplicateModRecord;
  rejectedCount: number;
  pending: boolean;
  onCancel: () => void;
  onResolve: () => void;
}) {
  const descriptionId = useId();
  return (
    <div
      className="row"
      role="group"
      aria-label="Confirm duplicate Mod resolution"
      aria-describedby={descriptionId}
    >
      <p id={descriptionId}>
        Keep <strong>{keeper.name}</strong> ({keeper.id}) and permanently discard the other{" "}
        {rejectedCount} GMM {rejectedCount === 1 ? "record" : "records"}, including their
        Variant selections and metadata? The shared Library directory and every byte inside it
        will remain. GMM requests removal of each rejected record&apos;s Junction before deleting
        its database record; it does not claim that request proves no externally changed link can
        remain. This cannot be undone.
      </p>
      <button type="button" onClick={onResolve} disabled={pending}>
        Keep this record
      </button>
      <button type="button" autoFocus onClick={onCancel} disabled={pending}>
        Cancel
      </button>
    </div>
  );
}
