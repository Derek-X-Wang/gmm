import { useLayoutEffect, useRef, useState } from "react";
import type { GameCode } from "./api";

type Feedback = { message: string; failed: boolean };

/** Own feedback and focus outside the warning that a recovery refresh can remove. */
export function useRecoveryOutcomes() {
  const [feedback, setFeedback] = useState<Partial<Record<GameCode, Feedback>>>({});
  const regions = useRef(new Map<GameCode, HTMLDivElement>());
  const initiators = useRef(new Map<GameCode, Element | null>());
  const [completed, setCompleted] = useState<{ game: GameCode } | null>(null);

  useLayoutEffect(() => {
    if (!completed) return;
    const origin = initiators.current.get(completed.game);
    // Do not steal focus if the user moved elsewhere, including into Settings,
    // while the command was pending.
    if (origin && (document.activeElement === origin ||
      (!origin.isConnected && document.activeElement === document.body))) {
      regions.current.get(completed.game)?.focus();
    }
  }, [completed]);

  return {
    start(game: GameCode, message: string) {
      initiators.current.set(game, document.activeElement);
      setFeedback((previous) => ({ ...previous, [game]: { message, failed: false } }));
    },
    finish(game: GameCode, message: string, failed = false) {
      setFeedback((previous) => ({ ...previous, [game]: { message, failed } }));
      setCompleted({ game });
    },
    region(game: GameCode, label: string) {
      const result = feedback[game];
      return (
        <div
          key={game}
          ref={(element) => {
            if (element) regions.current.set(game, element);
            else regions.current.delete(game);
          }}
          role="region"
          aria-label={label}
          aria-live="polite"
          aria-atomic="true"
          tabIndex={-1}
        >
          {result ? <p className={result.failed ? "error" : "muted"} role={result.failed ? "alert" : undefined}>{result.message}</p> : null}
        </div>
      );
    },
  };
}
