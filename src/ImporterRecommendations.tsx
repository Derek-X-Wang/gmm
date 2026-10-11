import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { importerRecommendationsEnabled, setImporterRecommendationsEnabled, type GameCode, type GameSummary } from "./api";
import { CommandErrorNotice } from "./CommandErrorNotice";

export function ImporterRecommendations({ games, onGamePage }: {
  games: GameSummary[];
  onGamePage: (game: GameCode) => void;
}) {
  const qc = useQueryClient();
  const preference = useQuery({
    queryKey: ["importerRecommendations"],
    queryFn: importerRecommendationsEnabled,
  });
  const toggleRecommendations = useMutation({
    mutationFn: (enabled: boolean) => setImporterRecommendationsEnabled(enabled),
    // Every Game's surface changes, including previously visited pages.
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["importerRecommendations"] });
      qc.invalidateQueries({ queryKey: ["importerOrigin"] });
      qc.invalidateQueries({ queryKey: ["importer"] });
      qc.invalidateQueries({ queryKey: ["gamePathStatus"] });
    },
  });

  return <section className="card" aria-label="Importer recommendations">
    <h2>Importer recommendations</h2>
    <p className="muted">GMM recommends Model Importer origins for all Games. Turning this off removes
      the recommendation layer: GMM stops fetching or consulting recommendations and falls back to
      your overrides or its built-in defaults. Your dismissed recommendations stay saved.</p>
    <p className="muted small">Review proposals, undo dismissals, and set overrides on each Game's page.</p>
    {preference.isPending ? <p role="status">Reading recommendations preference…</p> : null}
    {preference.isError ? <p role="status">Recommendations preference unavailable.</p> : null}
    {preference.data !== undefined && (preference.isFetching || preference.isError) ?
      <p className="muted small">The preference below is from the previous check.</p> : null}
    <CommandErrorNotice error={preference.error} />
    {preference.data !== undefined ? <label className="toggle">
      <input type="checkbox" checked={preference.data}
        disabled={toggleRecommendations.isPending || !preference.isSuccess || preference.isFetching}
        onChange={(event) => toggleRecommendations.mutate(event.currentTarget.checked)} />
      <span>Recommend importer origins (all games)</span>
    </label> : null}
    {toggleRecommendations.isPending ? <p role="status">Saving recommendations preference…</p> : null}
    {toggleRecommendations.isSuccess ? <p role="status">Recommendations preference saved.</p> : null}
    <CommandErrorNotice error={toggleRecommendations.error} />
    <div className="row">
      {games.map((game) => <button key={game.code} onClick={() => onGamePage(game.code)}>
        View {game.displayName} importer proposals, dismissals and overrides
      </button>)}
    </div>
  </section>;
}
