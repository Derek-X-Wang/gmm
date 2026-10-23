import { useRef, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  applyAllModUpdates, stopModUpdateBatch, type BatchMod, type BatchModResult, type GameCode,
  type ModUpdateBatchReport, type ModUpdateProgress, type ModUpdateRow,
} from "./api";
import { SessionCheckNotice, useSessionCheck } from "./SessionCheckNotice";
import { commandFailureMessage } from "./commandError";

type BatchView = {
  mods: BatchMod[];
  results: BatchModResult[];
  reading: string;
  started?: boolean;
  batchId?: string;
  current?: BatchMod;
  scanning?: boolean;
  stopRequested?: boolean;
  stopError?: string;
  report?: ModUpdateBatchReport;
  error?: string;
};

export function ModUpdateBatchAction({ game, rows, verified, checking }: {
  game: GameCode;
  rows: ModUpdateRow[];
  verified: boolean;
  checking: boolean;
}) {
  const qc = useQueryClient();
  const requestedStops = useRef(new Set<string>());
  const sessionCheck = useSessionCheck();
  const [batches, setBatches] = useState<Partial<Record<GameCode, BatchView>>>({});
  const change = (selected: GameCode, update: (old: BatchView) => BatchView) => {
    setBatches((old) => ({ ...old, [selected]: update(old[selected] ?? {
      mods: [], results: [], reading: "Starting updates…",
    }) }));
  };
  const apply = useMutation({
    scope: { id: "conflicts" },
    mutationFn: ({ selected, batchId }: { selected: GameCode; batchId: string }) => applyAllModUpdates(selected, batchId, (event: ModUpdateProgress) => {
      change(selected, (previous) => {
        // Any progress proves the backend passed its preflight, even if the
        // initial started message was not received.
        const old = { ...previous, started: true };
        switch (event.phase) {
          case "started": return { ...old, mods: event.mods };
          case "applying": return { ...old, current: event.item, reading: `Updating ${event.item.name}…` };
          case "completed": return { ...old, results: [...old.results, event.result] };
          case "checkingConflicts": return { ...old, current: undefined, scanning: true, reading: "Checking Conflicts…" };
        }
      });
    }),
    onSuccess: (report, { selected }) => {
      change(selected, (old) => ({ ...old, current: undefined, stopError: undefined, results: report.results, report, reading: report.stopped ? "Batch stopped." : "Batch finished." }));
      if (report.conflicts && !report.conflictError) {
        qc.setQueryData(["conflicts", selected], report.conflicts);
      }
    },
    onError: (error, { selected }) => {
      change(selected, (old) => ({
        ...old,
        reading: old.started ? "Batch result unavailable." : "Batch refused before updates started.",
        error: commandFailureMessage(error),
        results: !old.started ? [] : old.mods.map((item) => old.results.find((result) => result.modId === item.modId) ?? {
          ...item, status: "unknown", reason: "No result received. Check the Library before retrying.",
        }),
      }));
    },
    onSettled: (_report, _error, { selected }) => {
      // A rejected command can still leave recovery state. Refresh on either outcome.
      for (const key of ["mods", "modUpdates", "conflicts"]) {
        void qc.invalidateQueries({ queryKey: [key, selected] });
      }
      for (const key of ["variants", "attentionStatus", "libraryAudit"]) {
        void qc.invalidateQueries({ queryKey: [key] });
      }
    },
  });
  const stop = async (selected: GameCode, batchId: string) => {
    if (requestedStops.current.has(batchId)) return;
    requestedStops.current.add(batchId);
    change(selected, (old) => ({ ...old, stopRequested: true, stopError: undefined }));
    try {
      await stopModUpdateBatch(batchId);
    } catch (error) {
      requestedStops.current.delete(batchId);
      change(selected, (old) => old.batchId !== batchId || old.report || old.error ? old : ({
        ...old, stopRequested: false,
        stopError: `Stop request unverified: ${commandFailureMessage(error)} The batch may continue. Try again.`,
      }));
    }
  };
  const ahead = rows.filter((row) => row.upstreamAhead);
  const batch = batches[game];
  const sessionBlocked = !sessionCheck.session.isSuccess || !!sessionCheck.session.data;
  const start = () => {
    const batchId = crypto.randomUUID();
    setBatches((old) => ({ ...old, [game]: {
      batchId, mods: ahead.map(({ modId, name }) => ({ modId, name })), results: [], reading: "Starting updates…",
    } }));
    apply.mutate({ selected: game, batchId });
  };
  return <div>
    <button onClick={start} disabled={apply.isPending || checking || !verified || !ahead.length || sessionBlocked}>
      {apply.isPending ? "Applying Mod updates…" : "Apply all Mod updates"}
    </button>
    {apply.isPending && batch && !batch.scanning && apply.variables?.selected === game &&
      <button onClick={() => void stop(game, apply.variables!.batchId)} disabled={batch.stopRequested}>
        Stop after this Mod
      </button>}
    <SessionCheckNotice check={sessionCheck} />
    {sessionCheck.session.isSuccess && sessionCheck.session.data &&
      <p className="muted small">End the Game Session before applying Mod updates.</p>}
    {!verified && <p className="muted small">Available Mod updates are unverified.</p>}
    <p className="muted small">Updates run one at a time. Stop after this Mod lets the current Mod finish its metadata fetch, download, and swap. Keep GMM open until it finishes.</p>
    {apply.isPending && apply.variables?.selected !== game &&
      <p role="status">Mod updates are running for {apply.variables?.selected}.</p>}
    {batch && <div>
      <p role="status">{batch.reading} {apply.isPending && apply.variables?.selected === game ? `${batch.results.length} of ${batch.mods.length} completed.` : null}</p>
      {apply.isPending && apply.variables?.selected === game && batch.stopRequested && batch.current &&
        <p>Stop requested. Waiting for {batch.current.name} to finish; later Mods will not start.</p>}
      {batch.stopError && <p role="alert">{batch.stopError}</p>}
      <ul>{batch.results.map((result) => <li key={result.modId}>
        {result.name}: {result.status === "updated" ? "Updated" :
          `${result.status === "failed" ? "Failed" : result.status === "skipped" ? "Skipped" : "Unknown"} — ${result.reason}`}
      </li>)}</ul>
      {batch.error && <p role="alert">{batch.error}{batch.started ? " Conflict check result unavailable." : ""}</p>}
      {batch.report?.conflictError && <p role="alert">Conflicts unverified: {batch.report.conflictError}</p>}
      {batch.report?.conflicts && <div>
        {!batch.report.conflictError && <p>{batch.report.conflicts.conflicts.length} Conflicts found after the batch.</p>}
        {batch.report.conflictError && batch.report.conflicts.conflicts.length > 0 &&
          <p>{batch.report.conflicts.conflicts.length} known Conflicts found; the full result is unverified.</p>}
        <ul>{batch.report.conflicts.conflicts.map((conflict) => <li key={conflict.hash}>
          Hash {conflict.hash}: {conflict.mod_ids.map((id) =>
            rows.find((row) => row.modId === id)?.name ?? id).join(", ")}
        </li>)}</ul>
      </div>}
    </div>}
  </div>;
}
