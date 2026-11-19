import { useQuery, useQueryClient } from "@tanstack/react-query";
import { getImporterEvacuationRecovery, type GameCode } from "./api";
import { CommandErrorNotice } from "./CommandErrorNotice";
import { evidenceDetail, evidenceValue, readEvidence } from "./queryEvidence";

/**
 * Only the summary belongs on main; recovery actions remain on this Game's Settings page.
 *
 * On the main screen the absence of this card is itself a claim — that GMM read
 * this Game and found no Model Importer recovery outstanding. So the whole
 * reading goes through the evidence rule. `getImporterEvacuationRecovery`
 * returns `null` to mean nothing is outstanding, which is an answer; no value
 * at all means GMM never got one, and only the first may stay silent.
 */
export function GameRecoveryNotice({ game, displayName, onSettings }: {
  game: GameCode;
  displayName: string;
  onSettings: () => void;
}) {
  const qc = useQueryClient();
  const recovery = useQuery({
    queryKey: ["importer", "evacuation", game],
    queryFn: () => getImporterEvacuationRecovery(game),
  });
  // A refresh in flight is a fact about the request, not a claim about the
  // value, which is why the evidence rule has no case for it: the cached fetch
  // status answers it directly. Reading the cache rather than subscribing to it
  // keeps this to the renders the observer below already causes, since reading
  // `fetchStatus` through `readEvidence` tracks it. That indirect coupling is
  // pinned by the evidence test "keeps the previous-report hint while a refresh
  // of the report is in flight"; without it, a change to what `readEvidence`
  // touches would stop the hint appearing in silence.
  const refreshing = qc.getQueryState(["importer", "evacuation", game])?.fetchStatus === "fetching";

  const evidence = readEvidence(recovery);
  // A failed read, and a failed refresh of an established report, are both
  // named and claim nothing else: the report a failure replaces has stopped
  // being a statement about this Game.
  if ("error" in evidence) return <CommandErrorNotice error={evidence.error}
    heading={`Could not check ${displayName} Model Importer recovery`} />;
  // A first read still in flight has not failed; the card appears when it
  // lands, as it always did.
  if (evidence.kind === "pending") return null;
  const report = evidenceValue(evidence);
  if (!report) {
    // A read that landed and said nothing is outstanding renders nothing, as
    // before. Every other case has to say so out loud: a paused or never-run
    // read used to leave this screen looking like that healthy one.
    if (evidence.kind === "fresh") return null;
    return (
      <p className="muted small" role="status">
        {evidence.kind === "stale"
          ? `GMM could not re-check whether ${displayName} Model Importer recovery needs attention. The previous read found nothing outstanding.`
          : `GMM could not read whether ${displayName} Model Importer recovery needs attention. This is not a Game with nothing outstanding.`}
        {" "}
        {evidenceDetail(evidence)}
      </p>
    );
  }
  return <section className="card" aria-label={`${displayName} Model Importer recovery`}>
    <strong>{displayName} Model Importer recovery needs attention.</strong>
    {/*
      A paused refresh keeps `status: "success"` and sets only `fetchStatus`, so
      the hint this card used to key off `isFetching` disappeared exactly when
      it was needed and the old report read as the current one. The evidence
      rule sees the pause; the cached fetch status covers the in-flight case.
    */}
    {evidence.kind === "stale" ? (
      <span className="muted small">
        {`GMM could not re-check ${displayName} Model Importer recovery; this is the previous recovery report. `}
        {evidenceDetail(evidence)}
      </span>
    ) : refreshing ? (
      <span className="muted small">Checking again; this is the previous recovery report.</span>
    ) : null}
    <p>{report.reason}</p>
    <button onClick={onSettings}>Review {displayName} recovery in Settings</button>
  </section>;
}
