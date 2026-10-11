import { useState } from "react";
import type { GameCode as ApiGameCode, GameSummary } from "./api";

/**
 * Per-game tab strip. Rendered between the session banner and the
 * per-game cards so the user can switch contexts without leaving the
 * single-page shell. Saved install paths determine tabs; supported games
 * without paths remain reachable through the separate Add game control.
 */
export function GameTabs({
  games,
  remaining,
  pending,
  pathsUnavailable,
  active,
  onChange,
  onAddGame,
}: {
  games: GameSummary[];
  remaining: GameSummary[];
  pending: boolean;
  pathsUnavailable: boolean;
  active: ApiGameCode;
  onChange: (next: ApiGameCode) => void;
  onAddGame: (next: ApiGameCode) => void;
}) {
  const [adding, setAdding] = useState(false);
  return (
    <div className="game-navigation">
      {pending ? <p className="muted small">Checking game install paths…</p> : null}
      {!pending && !pathsUnavailable && games.length === 0 ? (
        <p className="muted">No game install paths set. Add a game to get started.</p>
      ) : null}
      <div className="row">
        {games.length > 0 ? (
          <nav className="game-tabs" role="tablist" aria-label="Game">
            {games.map((g) => {
              const isActive = g.code === active;
              return (
                <button
                  key={g.code}
                  role="tab"
                  aria-selected={isActive}
                  className={`game-tabs__tab${isActive ? " game-tabs__tab--active" : ""}`}
                  onClick={() => onChange(g.code)}
                >
                  {g.displayName}
                </button>
              );
            })}
          </nav>
        ) : null}
        {!pending && remaining.length > 0 ? (
          <button
            type="button"
            aria-expanded={adding}
            aria-controls="add-game-picker"
            data-settings-arrival="add"
            onClick={() => setAdding((open) => !open)}
          >
            Add game
          </button>
        ) : null}
      </div>
      {adding && remaining.length > 0 ? (
        <section id="add-game-picker" className="card" aria-label="Add a game">
          <p className="muted small">Choose a supported game to set its install path.</p>
          <div className="row game-navigation__choices">
            {remaining.map((game) => (
              <button key={game.code} type="button" onClick={() => {
                onAddGame(game.code);
                setAdding(false);
              }}>
                Set up {game.displayName}
              </button>
            ))}
          </div>
        </section>
      ) : null}
    </div>
  );
}
