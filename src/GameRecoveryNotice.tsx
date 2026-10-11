import { useQuery } from "@tanstack/react-query";
import { getImporterEvacuationRecovery, type GameCode } from "./api";
import { CommandErrorNotice } from "./CommandErrorNotice";

/** Only the summary belongs on main; recovery actions remain on this Game's Settings page. */
export function GameRecoveryNotice({ game, displayName, onSettings }: {
  game: GameCode;
  displayName: string;
  onSettings: () => void;
}) {
  const recovery = useQuery({
    queryKey: ["importer", "evacuation", game],
    queryFn: () => getImporterEvacuationRecovery(game),
  });
  if (recovery.isError) return <CommandErrorNotice error={recovery.error}
    heading={`Could not check ${displayName} Model Importer recovery`} />;
  if (!recovery.data) return null;
  return <section className="card" aria-label={`${displayName} Model Importer recovery`}>
    <strong>{displayName} Model Importer recovery needs attention.</strong>
    {recovery.isFetching ? <span className="muted small">Checking again; this is the previous recovery report.</span> : null}
    <p>{recovery.data.reason}</p>
    <button onClick={onSettings}>Review {displayName} recovery in Settings</button>
  </section>;
}
