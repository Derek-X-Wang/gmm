import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  applyAllModUpdates, type BatchMod, type BatchModResult, type GameCode,
  type ModUpdateBatchReport, type ModUpdateProgress, type ModUpdateRow,
} from "./api";
import { SessionCheckNotice, useSessionCheck } from "./SessionCheckNotice";
import { commandFailureMessage } from "./commandError";

type BatchView = {
  mods: BatchMod[];
  results: BatchModResult[];
  reading: string;
  started?: boolean;
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
  const sessionCheck = useSessionCheck();
  const [batches, setBatches] = useState<Partial<Record<GameCode, BatchView>>>({});
  const change = (selected: GameCode, update: (old: BatchView) => BatchView) => {
    setBatches((old) => ({ ...old, [selected]: update(old[selected] ?? {
      mods: [], results: [], reading: "Starting updates…",
    }) }));
  };
  const apply = useMutation({
    mutationFn: (selected: GameCode) => applyAllModUpdates(selected, (event: ModUpdateProgress) => {
      change(selected, (previous) => {
        // Any progress proves the backend passed its preflight, even if the
        // initial started message was not received.
        const old = { ...previous, started: true };
        switch (event.phase) {
          case "started": return { ...old, mods: event.mods };
          case "applying": return { ...old, reading: `Updating ${event.item.name}…` };
          case "completed": return { ...old, results: [...old.results, event.result] };
          case "checkingConflicts": return { ...old, reading: "Checking Conflicts…" };
        }
      });
    }),
    onSuccess: (report, selected) => {
      change(selected, (old) => ({ ...old, results: report.results, report, reading: "Batch finished." }));
      if (report.conflicts && !report.conflictError) {
        qc.setQueryData(["conflicts", selected], report.conflicts);
      }
    },
    onError: (error, selected) => {
      change(selected, (old) => ({
        ...old,
        reading: old.started ? "Batch result unavailable." : "Batch refused before updates started.",
        error: commandFailureMessage(error),
        results: !old.started ? [] : old.mods.map((item) => old.results.find((result) => result.modId === item.modId) ?? {
          ...item, status: "unknown", reason: "No result received. Check the Library before retrying.",
        }),
      }));
    },
    onSettled: (_report, _error, selected) => {
      // A rejected command can still leave recovery state. Refresh on either outcome.
      for (const key of ["mods", "modUpdates", "conflicts"]) {
        void qc.invalidateQueries({ queryKey: [key, selected] });
      }
      for (const key of ["variants", "attentionStatus", "libraryAudit"]) {
        void qc.invalidateQueries({ queryKey: [key] });
      }
    },
  });
  const ahead = rows.filter((row) => row.upstreamAhead);
  const batch = batches[game];
  const sessionBlocked = !sessionCheck.session.isSuccess || !!sessionCheck.session.data;
  const start = () => {
    setBatches((old) => ({ ...old, [game]: {
      mods: ahead.map(({ modId, name }) => ({ modId, name })), results: [], reading: "Starting updates…",
    } }));
    apply.mutate(game);
  };
  return <div>
    <button onClick={start} disabled={apply.isPending || checking || !verified || !ahead.length || sessionBlocked}>
      {apply.isPending ? "Applying Mod updates…" : "Apply all Mod updates"}
    </button>
    <SessionCheckNotice check={sessionCheck} />
    {sessionCheck.session.isSuccess && sessionCheck.session.data &&
      <p className="muted small">End the Game Session before applying Mod updates.</p>}
    {!verified && <p className="muted small">Available Mod updates are unverified.</p>}
    <p className="muted small">Updates run one at a time. Cancellation is unavailable; keep GMM open until the batch finishes.</p>
    {apply.isPending && apply.variables !== game &&
      <p role="status">Mod updates are running for {apply.variables}.</p>}
    {batch && <div>
      <p role="status">{batch.reading} {apply.isPending && apply.variables === game ? `${batch.results.length} of ${batch.mods.length} completed.` : null}</p>
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
