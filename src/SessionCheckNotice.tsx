import { useQuery } from "@tanstack/react-query";
import { currentSession } from "./api";
import { commandFailureMessage } from "./commandError";
import { readEvidence } from "./queryEvidence";

export function useSessionCheck() {
  const session = useQuery({ queryKey: ["session"], queryFn: currentSession });
  const evidence = readEvidence(session);
  // A paused read is the one case that stays silent here, as it always has.
  // The Launch gate still keys off `isSuccess`; closing that is a migration step.
  const reason = evidence.kind === "pending"
    || (evidence.kind === "unavailable" && evidence.reason !== "read-failed")
    ? "Game Session unverified — checking whether a Game is running."
    : evidence.kind === "unavailable"
      ? "Cannot verify Game Session — session check failed."
      : evidence.kind === "stale" && evidence.reason === "refresh-failed"
        ? "Cannot verify Game Session — refresh failed; the previous result is unverified."
        : null;
  const failure = evidence.kind !== "fresh" && "error" in evidence ? evidence.error : null;
  return { session, reason, failure };
}

export function SessionCheckNotice({ check }: { check: ReturnType<typeof useSessionCheck> }) {
  return check.reason ? (
    <p className="muted small" role="status">
      {check.reason}{check.failure !== null ? ` ${commandFailureMessage(check.failure)}` : ""}
    </p>
  ) : null;
}
