import { onlineManager, useQueries, useQuery } from "@tanstack/react-query";
import { act, waitFor } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";

import { readEvidence } from "./queryEvidence";
import { makeQueryClient, renderWithQuery } from "./test/harness";

afterEach(() => onlineManager.setOnline(true));

const reading = (over: Record<string, unknown>) => ({
  data: undefined, dataUpdatedAt: 0, error: null, fetchStatus: "idle" as const, status: "pending" as const, ...over,
});

it("calls a settled successful read fresh", () => {
  expect(readEvidence(reading({ status: "success", data: 7, dataUpdatedAt: 1 })))
    .toEqual({ kind: "fresh", value: 7 });
});

it("calls a value stale when its paused refresh left status success and isFetching false", () => {
  // This is what going offline produces: only `fetchStatus` changes, so
  // `isSuccess` stays true and `isFetching` stays false. Every presence-keyed
  // or status-keyed guard reads this as fresh.
  const paused = reading({ status: "success", data: 7, dataUpdatedAt: 1, fetchStatus: "paused" });
  expect(paused.status).toBe("success");
  expect(readEvidence(paused)).toEqual({ kind: "stale", value: 7, reason: "refresh-paused" });
});

it("calls a value stale when its refresh failed, and keeps the error", () => {
  const error = new Error("boom");
  expect(readEvidence(reading({ status: "error", data: 7, dataUpdatedAt: 1, error })))
    .toEqual({ kind: "stale", value: 7, reason: "refresh-failed", error });
});

it("separates a never-read failure from a refresh failure by whether a value was established", () => {
  const error = new Error("boom");
  expect(readEvidence(reading({ status: "error", error })))
    .toEqual({ kind: "unavailable", reason: "read-failed", error });
  expect(readEvidence(reading({ status: "error", error, data: 7, dataUpdatedAt: 1 })).kind).toBe("stale");
});

it("keeps a refresh that is only in flight fresh", () => {
  expect(readEvidence(reading({ status: "success", data: 7, dataUpdatedAt: 1, fetchStatus: "fetching" })).kind)
    .toBe("fresh");
});

it("calls the first read in flight pending", () => {
  expect(readEvidence(reading({ fetchStatus: "fetching" }))).toEqual({ kind: "pending" });
});

it("does not call a first read pending when it is paused or was never started", () => {
  expect(readEvidence(reading({ fetchStatus: "paused" })))
    .toEqual({ kind: "unavailable", reason: "read-paused" });
  expect(readEvidence(reading({ fetchStatus: "idle" })))
    .toEqual({ kind: "unavailable", reason: "not-read" });
});

it("lets a failure outrank a pause, since the failed attempt already ended", () => {
  const error = new Error("boom");
  expect(readEvidence(reading({ status: "error", error, data: 7, dataUpdatedAt: 1, fetchStatus: "paused" })).kind)
    .toBe("stale");
  expect(readEvidence(reading({ status: "error", error, data: 7, dataUpdatedAt: 1, fetchStatus: "paused" })))
    .toMatchObject({ reason: "refresh-failed" });
});

it("reads a real useQuery result: pending, fresh, then stale once offline pauses the refresh", async () => {
  const client = makeQueryClient();
  let seen: ReturnType<typeof readEvidence<number>> = { kind: "pending" };
  function Probe() {
    seen = readEvidence(useQuery({ queryKey: ["one"], queryFn: async () => 1 }));
    return null;
  }
  renderWithQuery(<Probe />, { client });
  expect(seen).toEqual({ kind: "pending" });
  await waitFor(() => expect(seen).toEqual({ kind: "fresh", value: 1 }));

  onlineManager.setOnline(false);
  await act(() => client.invalidateQueries({ queryKey: ["one"] }));
  await waitFor(() => expect(seen).toEqual({ kind: "stale", value: 1, reason: "refresh-paused" }));
});

it("reads each element of a real useQueries array the same way", async () => {
  const client = makeQueryClient();
  let seen: Array<ReturnType<typeof readEvidence<string>>> = [];
  function Probe() {
    const results = useQueries({ queries: [
      { queryKey: ["ok"], queryFn: async () => "a" },
      { queryKey: ["bad"], queryFn: async (): Promise<string> => { throw new Error("nope"); } },
    ] });
    seen = results.map((result) => readEvidence(result));
    return null;
  }
  renderWithQuery(<Probe />, { client });
  expect(seen.map((entry) => entry.kind)).toEqual(["pending", "pending"]);
  await waitFor(() => expect(seen.map((entry) => entry.kind)).toEqual(["fresh", "unavailable"]));
});
