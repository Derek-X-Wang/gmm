import type { ReactNode } from "react";

export function GameHeader({ displayName, action }: { displayName: string; action: ReactNode }) {
  return <section className="game-header row row--between" aria-label={`${displayName} header`}>
    <h1>{displayName}</h1>
    {action}
  </section>;
}
