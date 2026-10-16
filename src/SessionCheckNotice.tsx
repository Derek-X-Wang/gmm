import { useQuery } from "@tanstack/react-query";
import { currentSession } from "./api";
import { commandFailureMessage } from "./commandError";

export function useSessionCheck() {
  const session = useQuery({ queryKey: ["session"], queryFn: currentSession });
  const reason = session.isPending
    ? "Game Session unverified — checking whether a Game is running."
    : session.isError
      ? session.dataUpdatedAt > 0
        ? "Cannot verify Game Session — refresh failed; the previous result is unverified."
        : "Cannot verify Game Session — session check failed."
      : null;
  return { session, reason };
}

export function SessionCheckNotice({ check }: { check: ReturnType<typeof useSessionCheck> }) {
  return check.reason ? (
    <p className="muted small" role="status">
      {check.reason}{check.session.isError ? ` ${commandFailureMessage(check.session.error)}` : ""}
    </p>
  ) : null;
}
