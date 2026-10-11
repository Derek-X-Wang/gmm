import { useQuery } from "@tanstack/react-query";
import { useId, useRef, useState, type ReactNode } from "react";
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

  const [expanded, setExpanded] = useState(false);
  const regionId = useId();
  const indicator = useRef<HTMLButtonElement>(null);
  const previous = report.isFetching || report.isError;
  const loud = report.isSuccess && !report.isFetching && report.data.safeToProceed === false;
  const findings = report.data ? findingCount(report.data) : 0;
  const checkingLabel = report.data ? "Refreshing attention…" : "Checking attention…";
  const label = report.isFetching ? checkingLabel
    : report.isError || !report.data ? "Attention unavailable"
    : loud ? "Needs attention" : "No attention needed";

  function collapse() {
    setExpanded(false);
    indicator.current?.focus();
  }

  return (
    <section className="attention-indicator" aria-label="Attention across all Games" aria-busy={report.isFetching}>
      <div className="attention-indicator__controls">
        <button ref={indicator} className={`attention-indicator__toggle${loud ? " attention-indicator__toggle--loud" : ""}`}
          aria-expanded={expanded} aria-controls={regionId}
          onClick={() => expanded ? collapse() : setExpanded(true)}>
          {loud ? <span role="img" aria-label="Warning">⚠</span> : null}
          <span>{label}</span>
          <span className="attention-indicator__scope">All Games{!previous && report.data ? ` · ${findings} ${findings === 1 ? "finding" : "findings"}` : ""}</span>
        </button>
        <button className="attention-indicator__refresh" onClick={() => void report.refetch()} disabled={report.isFetching}>
          Refresh attention
        </button>
      </div>
      {report.isFetching ? <span role="status" className="muted small">{checkingLabel}</span> : null}
      {report.isError ? (
        <CommandErrorNotice error={report.error} heading="Could not check attention"
          detail="Attention status is unavailable. Refresh to try again." />
      ) : null}
      <div id={regionId} role="region" aria-label="All-Games attention report" hidden={!expanded}
        className="attention-indicator__report" onKeyDown={(event) => {
          if (event.key === "Escape") { event.stopPropagation(); collapse(); }
        }}>
        {expanded ? <>
          <h2>Attention across all Games</h2>
          <p className="muted small">All Games. Refresh after making changes to check again.</p>
          {report.data ? <>
            <p className="muted small">
              {previous ? "Previous snapshot. Checked at " : "Checked at "}<time dateTime={new Date(report.dataUpdatedAt).toISOString()}>
                {new Date(report.dataUpdatedAt).toLocaleString()}
              </time>. This is a snapshot; each action checks its own availability.
            </p>
            {report.data.safeToProceed ? (
              <p role="status">{previous ? "The previous report found nothing needing attention." : "Nothing needs attention in this report."}</p>
            ) : null}
            <OutstandingAttention report={report.data} />
          </> : null}
          <button onClick={collapse}>Collapse attention report</button>
        </> : null}
      </div>
    </section>
  );
}

function findingCount(report: AttentionReport): number {
  return report.reinstalls.length + report.enabledTransitions.length + report.importerEvacuations.length
    + report.stagedLibraryOperations.length + report.sessionLaunches.length + (report.activeSession ? 1 : 0)
    + report.libraryAudits.reduce((count, audit) => count + audit.unreferenced.length + audit.duplicates.length, 0)
    + report.libraryRootOverlaps.length + report.modPathOverlaps.length;
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
