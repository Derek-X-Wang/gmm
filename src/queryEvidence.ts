import type { QueryObserverResult } from "@tanstack/react-query";

import { commandFailureMessage } from "./commandError";

/**
 * What GMM actually knows about a value it reads from the backend.
 *
 * `data` alone cannot say. React Query keeps the last value when a refresh
 * fails, and keeps it (with `status: "success"`) when a refresh is paused
 * because the app is offline. A surface that states a claim from `data` then
 * presents an old read as a current one. Read the evidence instead:
 *
 * - `fresh`: the last read succeeded and nothing is wrong with it.
 * - `stale`: a value was established, but the refresh failed or is paused.
 * - `pending`: no value yet, and the first read is in flight.
 * - `unavailable`: no value was ever established, and no read is in flight.
 *
 * A refresh that is merely in flight does not make a value stale. It is the
 * last value that was read, and a failure or a pause is what says otherwise.
 */
export type Evidence<T> =
  | { kind: "fresh"; value: T }
  | { kind: "stale"; value: T; reason: "refresh-failed"; error: unknown }
  | { kind: "stale"; value: T; reason: "refresh-paused" }
  | { kind: "pending" }
  | { kind: "unavailable"; reason: "read-failed"; error: unknown }
  | { kind: "unavailable"; reason: "read-paused" }
  | { kind: "unavailable"; reason: "not-read" };

type QueryReading<T> = Pick<
  QueryObserverResult<T, unknown>,
  "data" | "dataUpdatedAt" | "error" | "fetchStatus" | "status"
>;

/**
 * Read the evidence out of one query result. It takes the result, not a key,
 * so it serves a `useQuery` return value, one element of a `useQueries` array,
 * and a wrapper hook's result alike.
 *
 * `dataUpdatedAt > 0` is what separates "a value was established" from
 * "never read". `status` and `fetchStatus` say how the latest attempt went.
 * A failure outranks a pause, because it is the attempt that already ended.
 */
export function readEvidence<T>(query: QueryReading<T>): Evidence<T> {
  if (query.dataUpdatedAt > 0) {
    const value = query.data as T;
    if (query.status === "error") return { kind: "stale", value, reason: "refresh-failed", error: query.error };
    if (query.fetchStatus === "paused") return { kind: "stale", value, reason: "refresh-paused" };
    return { kind: "fresh", value };
  }
  if (query.status === "error") return { kind: "unavailable", reason: "read-failed", error: query.error };
  if (query.fetchStatus === "paused") return { kind: "unavailable", reason: "read-paused" };
  if (query.fetchStatus === "idle") return { kind: "unavailable", reason: "not-read" };
  return { kind: "pending" };
}

/** The value, but only when one was established. Stale values are values. */
export function evidenceValue<T>(evidence: Evidence<T>): T | undefined {
  return evidence.kind === "fresh" || evidence.kind === "stale" ? evidence.value : undefined;
}

/** Why a read cannot be trusted, in words for a user. Takes the two cases that carry a reason. */
export function evidenceDetail(evidence: Extract<Evidence<unknown>, { kind: "stale" | "unavailable" }>): string {
  if ("error" in evidence) return commandFailureMessage(evidence.error);
  return evidence.reason === "not-read" ? "The read has not run." : "The read is waiting for a connection.";
}
