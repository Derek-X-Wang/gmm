import { useIsMutating, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { getLibraryPaths, setLibraryPathForGame, setLibraryRoot, type GameCode, type GameSummary, type MoveReport } from "./api";
import { CommandErrorNotice } from "./CommandErrorNotice";
import { LibraryRootOverlapWarning } from "./LibraryRootOverlapWarning";
import { PathField } from "./PathField";

const moveKey = ["libraryMove"];

export function GlobalLibraryRootEditor({ games, onGamePage }: {
  games: GameSummary[];
  onGamePage: (game: GameCode) => void;
}) {
  const qc = useQueryClient();
  const paths = useQuery({ queryKey: ["libraryPaths"], queryFn: getLibraryPaths });
  const busy = useIsMutating({ mutationKey: moveKey }) > 0;
  const refresh = () => {
    qc.invalidateQueries({ queryKey: ["libraryPaths"] });
    qc.invalidateQueries({ queryKey: ["mods"] });
    qc.invalidateQueries({ queryKey: ["libraryAudit"] });
    qc.invalidateQueries({ queryKey: ["conflicts"] });
  };
  const setRoot = useMutation({
    mutationKey: moveKey,
    mutationFn: (next: string | null) => setLibraryRoot(next),
    onSettled: refresh,
  });
  const p = paths.data;
  return <section className="card" aria-label="Global Library root settings">
    <h2>Global Library root</h2>
    <LibraryMoveExplanation />
    <p className="muted">Global changes affect only Games that inherit this root. Each Game override affects that Game only.</p>
    {p ? <p className="muted small">Games that inherit this root: {games.filter((game) => !p.perGameOverrides[game.code]).map((game) => game.displayName).join(", ") || "None"}.</p> : null}
    <div className="row">
      {games.map((game) => <a key={game.code} href={`#settings-${game.code}`} onClick={(event) => { event.preventDefault(); onGamePage(game.code); }}>
        Library override for {game.displayName}
      </a>)}
    </div>
    {paths.isError ? <CommandErrorNotice error={paths.error} heading="Could not resolve Library paths." detail="Library settings are unavailable until GMM can read their current paths." />
      : !p ? <p className="muted">Resolving paths…</p>
      : <>
        <LibraryRootOverlapWarning overlaps={(p.overlaps ?? []).filter((overlap) => overlap.game === null)} modOverlaps={(p.modOverlaps ?? []).filter((overlap) => !p.perGameOverrides[overlap.game])} />
        <p className="muted small">Default root: <code>{p.defaultRoot}</code></p>
        <p className="muted small">Global override: {p.rootOverride ?? "Not set"}</p>
        <div className="row">
          <PathField label="Global Library root" value={p.effectiveRoot} disabled={busy} pickerLabel="Change global root…" onApply={(path) => setRoot.mutate(path)} />
        </div>
        <button onClick={() => setRoot.mutate(null)} disabled={busy || !p.rootOverride}>Reset to default</button>
      </>}
    {setRoot.isPending ? <p role="status">{setRoot.variables === null ? "Resetting…" : "Moving…"}</p> : null}
    <CommandErrorNotice error={setRoot.error} />
    <LibraryMoveResult report={setRoot.data} />
  </section>;
}

export function GameLibraryOverrideEditor({ game, displayName, onGlobalPage }: {
  game: GameCode;
  displayName: string;
  onGlobalPage: () => void;
}) {
  const qc = useQueryClient();
  const paths = useQuery({ queryKey: ["libraryPaths"], queryFn: getLibraryPaths });
  const busy = useIsMutating({ mutationKey: moveKey }) > 0;
  const refresh = () => {
    qc.invalidateQueries({ queryKey: ["libraryPaths"] });
    qc.invalidateQueries({ queryKey: ["mods"] });
    qc.invalidateQueries({ queryKey: ["libraryAudit"] });
    qc.invalidateQueries({ queryKey: ["conflicts"] });
  };
  const setPerGame = useMutation({
    mutationKey: moveKey,
    mutationFn: (path: string | null) => setLibraryPathForGame(game, path),
    onSettled: refresh,
  });
  const p = paths.data;
  return <section className="card" aria-label={`Library for ${displayName}`}>
    <h2>Library for {displayName}</h2>
    <LibraryMoveExplanation />
    <p className="muted">This override affects {displayName} only.</p>
    <a href="#settings-gmm" onClick={(event) => { event.preventDefault(); onGlobalPage(); }}>View global Library root</a>
    {paths.isError ? <CommandErrorNotice error={paths.error} heading="Could not resolve Library paths." detail="Library settings are unavailable until GMM can read their current paths." />
      : !p ? <p className="muted">Resolving paths…</p>
      : <>
        <LibraryRootOverlapWarning overlaps={(p.overlaps ?? []).filter((overlap) => overlap.game === game || overlap.game === null && !p.perGameOverrides[game])} modOverlaps={(p.modOverlaps ?? []).filter((overlap) => overlap.game === game)} />
        <p className="muted small">{p.perGameOverrides[game] ? `Override for ${displayName}` : "Inherited from the global Library root"}</p>
        <div className="row">
          <PathField label={`${displayName} Library path`} value={p.perGameEffective[game] ?? ""} disabled={busy} pickerLabel="Change…" onApply={(path) => setPerGame.mutate(path)} />
        </div>
        <button onClick={() => setPerGame.mutate(null)} disabled={busy || !p.perGameOverrides[game]}>Reset override</button>
      </>}
    {setPerGame.isPending ? <p role="status">{setPerGame.variables === null ? "Resetting…" : "Moving…"}</p> : null}
    <CommandErrorNotice error={setPerGame.error} />
    <LibraryMoveResult report={setPerGame.data} />
  </section>;
}

function LibraryMoveExplanation() {
  return <p className="muted">
    Changing a safe path disables affected Mods, moves the bytes, then re-enables
    them against the new location. Repairing an overlap changes only the setting
    and leaves the old tree untouched. Non-NTFS targets are refused before any move.
  </p>;
}

function LibraryMoveResult({ report }: { report?: MoveReport }) {
  if (!report) return null;
  return <>
    {report.overlap_repair ? <div className="reinstall-action-notice error" role="alert">
      <strong>Library setting changed, but existing Mod folders were not moved.</strong>{" "}
      GMM did not read the overlapping path. The warning remains while any Mod is still
      recorded there; inspect and recover those folders manually.
    </div> : !report.failed_junction_restores.length ? <p role="status">Library move completed.</p> : null}
    <JunctionRestoreFailures failures={report.failed_junction_restores} />
  </>;
}

/** The bytes moved successfully; these individual deployment failures remain. */
function JunctionRestoreFailures({ failures }: { failures: MoveReport["failed_junction_restores"] }) {
  const invalidSelections = failures.filter((failure) => failure.kind === "invalidActiveVariant");
  const retryableRestores = failures.filter((failure) => failure.kind !== "invalidActiveVariant");
  return <>
    {invalidSelections.length > 0 ? <div className="error" role="alert">
      <strong>The Library moved, but some Mods have an invalid Variant selection.</strong>
      <p>GMM left those Mods unlinked. Rebuilding junctions cannot repair a saved Variant selection.</p>
      <ul>{invalidSelections.map((failure) => <li key={`${failure.game}:${failure.mod_id}`} className="small">
        {failure.game.toUpperCase()}: {failure.error}
      </li>)}</ul>
    </div> : null}
    {retryableRestores.length > 0 ? <div className="error">
      <p>The Library moved, but {retryableRestores.length} enabled Mod{retryableRestores.length === 1 ? "" : "s"} could not be linked back into the
        game folder. Their files are safe. Use “Rebuild junctions” on the affected Game's
        Settings page; if it keeps failing, the reason is shown here.</p>
      <ul>{retryableRestores.map((failure) => <li key={`${failure.game}:${failure.mod_id}`} className="small">
        <code>{failure.game}</code> {failure.mod_id}: {failure.error}
      </li>)}</ul>
    </div> : null}
  </>;
}
