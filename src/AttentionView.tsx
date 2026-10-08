import { useQuery } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { attentionStatus, type AttentionReport } from "./api";
import { CommandErrorNotice } from "./CommandErrorNotice";

/** One all-Game snapshot on mount, then explicit refreshes only. */
export function AttentionView() {
  const report = useQuery({
    queryKey: ["attentionStatus"],
    queryFn: attentionStatus,
    staleTime: Infinity,
    retry: false,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
    refetchOnMount: false,
  });

  return (
    <section className="card" aria-label="Attention across all Games" aria-busy={report.isFetching}>
      <div className="row row--between">
        <h2>Needs attention</h2>
        <button onClick={() => void report.refetch()} disabled={report.isFetching}>
          Refresh attention
        </button>
      </div>
      <p className="muted small">All Games. Refresh after making changes to check again.</p>
      {report.isFetching ? <p role="status">Checking attention…</p> : null}
      {report.isError ? (
        <CommandErrorNotice error={report.error} heading="Could not check attention"
          detail="Attention status is unavailable. Refresh to try again." />
      ) : !report.isFetching && report.data?.safeToProceed ? (
        <p role="status">Nothing needs attention in this report.</p>
      ) : !report.isFetching && report.data ? (
        <OutstandingAttention report={report.data} />
      ) : null}
      {!report.isFetching && !report.isError && report.data ? (
        <p className="muted small">
          Checked at <time dateTime={new Date(report.dataUpdatedAt).toISOString()}>
            {new Date(report.dataUpdatedAt).toLocaleTimeString()}
          </time>. This is a snapshot; each action checks its own availability.
        </p>
      ) : null}
    </section>
  );
}

function AttentionGroup({ title, guidance, items }: {
  title: string;
  guidance: string;
  items: ReactNode[];
}) {
  if (items.length === 0) return null;
  return (
    <div>
      <h3>{title} ({items.length})</h3>
      <p className="muted small">{guidance}</p>
      <ul>{items.map((item, index) => <li key={index}>{item}</li>)}</ul>
    </div>
  );
}

function OutstandingAttention({ report }: { report: AttentionReport }) {
  return (
    <div>
      <AttentionGroup title="Mod reinstalls" guidance="Review the affected Mod's reinstall recovery warning."
        items={report.reinstalls.map((item) => <>
          <strong>{item.game.toUpperCase()}</strong> · Mod <code>{item.modId}</code>
          <p>Library: <code>{item.libraryPath}</code></p>
          <p>{item.recovery?.reason ?? "Reinstall still owns this Library path. Let the operation finish."}</p>
        </>)} />
      <AttentionGroup title="Enable or disable transitions" guidance="Review the affected Mod's transition recovery warning."
        items={report.enabledTransitions.map((item) => <>
          <strong>{item.game.toUpperCase()}</strong> · Mod <code>{item.modId}</code> · {item.intendedEnabled ? "Enabling" : "Disabling"}
          <p>Junction: <code>{item.junctionPath}</code></p>
          <p>{item.recovery?.reason ?? "A producer still owns this transition. Let the operation finish."}</p>
        </>)} />
      <AttentionGroup title="Model Importer recovery" guidance="Review the affected Game's Model Importer recovery warning."
        items={report.importerEvacuations.map((item) => <>
          <strong>{item.game.toUpperCase()}</strong>
          <p>Game: <code>{item.gamePath}</code> · Backup: <code>{item.backupPath}</code></p>
          <p>{item.recovery?.reason ?? "Model Importer replacement still owns these paths. Let the operation finish."}</p>
        </>)} />
      <AttentionGroup title="Staged Library operations"
        guidance="Let an active import or adoption finish. For an interrupted operation, restart GMM to retry recovery, then inspect the affected Game's Library audit."
        items={report.stagedLibraryOperations.map((item) => <>
          <strong>{item.game.toUpperCase()}</strong> · {item.operation} · <code>{item.id}</code>
          <p>Staged Library path: <code>{item.stagedPath}</code></p>
          <p>{item.recoveryError ?? "A producer still owns this staged Library path."}</p>
        </>)} />
      <AttentionGroup title="Interrupted Game launches" guidance="Review the interrupted launch warning in the Game Session area."
        items={report.sessionLaunches.map((item) => <>
          <strong>{item.game.toUpperCase()}</strong> · <code>{item.id}</code>
          <p>Started: {item.startedAt} · Child PID: {item.childPid ?? "unavailable"}</p>
        </>)} />
      <AttentionGroup title="Active Game Session" guidance="Review the Game Session banner. Close the Game before changing its Mods."
        items={report.activeSession ? [<>
          <strong>{report.activeSession.game.toUpperCase()}</strong> · PID {report.activeSession.pid}
          <p>Started: {report.activeSession.startedAt}</p>
        </>] : []} />
      <AttentionGroup title="Unreferenced Library folders" guidance="Inspect, recover, or explicitly delete these folders in the affected Game's Library audit."
        items={report.libraryAudits.flatMap((audit) => audit.unreferenced.map((item) => <>
          <strong>{audit.game.toUpperCase()}</strong> · <code>{item.path}</code>
        </>))} />
      <AttentionGroup title="Duplicate Mod records" guidance="Review duplicate ownership in the affected Game's Library audit."
        items={report.libraryAudits.flatMap((audit) => audit.duplicates.map((item) => <>
          <strong>{audit.game.toUpperCase()}</strong> · <code>{item.path}</code>
          <p>Mods: {item.mods.map((mod) => `${mod.name} (${mod.id})`).join(", ")}</p>
        </>))} />
      <AttentionGroup title="Library root overlaps" guidance="Review configured roots in Library paths."
        items={report.libraryRootOverlaps.map((item) => <>
          <strong>{item.game?.toUpperCase() ?? "Global Library"}</strong>
          <p>Library: <code>{item.path}</code> · Model Importer backups: <code>{item.backups}</code></p>
        </>)} />
      <AttentionGroup title="Mod path overlaps" guidance="Review stranded Mod records in Library paths."
        items={report.modPathOverlaps.map((item) => <>
          <strong>{item.game.toUpperCase()}</strong> · {item.modName} · <code>{item.modId}</code>
          <p>Mod: <code>{item.path}</code> · Model Importer backups: <code>{item.backups}</code></p>
        </>)} />
    </div>
  );
}
