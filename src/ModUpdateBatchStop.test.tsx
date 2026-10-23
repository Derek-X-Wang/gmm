import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { renderWithQuery } from "./test/harness";
import { ModUpdateBatchAction } from "./ModUpdateBatchAction";
import type { ModUpdateBatchReport, ModUpdateProgress, ModUpdateRow } from "./api";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke, Channel: class { onmessage?: (event: ModUpdateProgress) => void } }));
const rows: ModUpdateRow[] = ["First", "Current", "Later"].map((name) => ({
  modId: name, name, installedVersion: "1", upstreamVersion: "2", upstreamAhead: true, updateCheckEnabled: true,
}));
let progress: (event: ModUpdateProgress) => void;
let resolve: (report: ModUpdateBatchReport) => void;
const view = <ModUpdateBatchAction game="gimi" rows={rows} verified checking={false} />;
beforeEach(() => {
  invoke.mockReset().mockImplementation((command: string, args?: { onProgress: { onmessage: typeof progress } }) => {
    if (command === "current_session") return Promise.resolve(null);
    if (command === "apply_all_mod_updates") {
      progress = args!.onProgress.onmessage;
      return new Promise<ModUpdateBatchReport>((yes) => { resolve = yes; });
    }
    if (command === "stop_mod_update_batch") return Promise.resolve();
    throw new Error(`Unexpected command: ${command}`);
  });
});
const start = async () => {
  const button = screen.getByRole("button", { name: "Apply all Mod updates" });
  await waitFor(() => expect(button).toBeEnabled());
  fireEvent.click(button);
  await waitFor(() => expect(progress).toBeTypeOf("function"));
  act(() => {
    progress({ phase: "started", mods: rows });
    progress({ phase: "completed", result: { ...rows[0], status: "updated" } });
    progress({ phase: "applying", item: rows[1] });
  });
};
it("offers an explicit boundary stop, names the waiting Mod, and sends only once", async () => {
  const { client } = renderWithQuery(view);
  const invalidate = vi.spyOn(client, "invalidateQueries");
  const setData = vi.spyOn(client, "setQueryData");
  await start();
  const stop = screen.getByRole("button", { name: "Stop after this Mod" });
  expect(stop).toBeEnabled();
  fireEvent.click(stop);
  fireEvent.click(stop);
  await waitFor(() => expect(stop).toBeDisabled());
  expect(screen.getByText("Stop requested. Waiting for Current to finish; later Mods will not start.")).toBeVisible();
  expect(invoke.mock.calls.filter(([command]) => command === "stop_mod_update_batch")).toEqual([["stop_mod_update_batch", { batchId: invoke.mock.calls.find(([command]) => command === "apply_all_mod_updates")![1].batchId }]]);
  expect(screen.getByText("First: Updated")).toBeVisible();
  act(() => progress({ phase: "checkingConflicts" }));
  expect(screen.queryByRole("button", { name: "Stop after this Mod" })).not.toBeInTheDocument();
  const report: ModUpdateBatchReport = {
    stopped: true,
    results: [{ ...rows[0], status: "updated" }, { ...rows[1], status: "updated" }, { ...rows[2], status: "skipped", reason: "Not started because you stopped the batch. The Library was not touched." }],
    conflicts: { conflicts: [], per_mod_count: {} },
    conflictError: "The full Conflict reading is unverified because the batch stopped with Mods not attempted.",
  };
  await act(async () => resolve(report));
  expect(screen.getByRole("status")).toHaveTextContent("Batch stopped.");
  expect(screen.getByText("Current: Updated")).toBeVisible();
  expect(screen.getByText(/^Later: Skipped/)).toHaveTextContent("The Library was not touched.");
  expect(screen.getByRole("alert")).toHaveTextContent("Conflicts unverified");
  expect(screen.queryByText("0 Conflicts found after the batch.")).not.toBeInTheDocument();
  expect(invalidate).toHaveBeenCalledWith({ queryKey: ["conflicts", "gimi"] });
  expect(setData).not.toHaveBeenCalledWith(["conflicts", "gimi"], report.conflicts);
});
it("withdraws Stop during the final Conflict scan and offers it again for the next batch", async () => {
  renderWithQuery(view);
  await start();
  act(() => {
    progress({ phase: "completed", result: { ...rows[1], status: "updated" } });
    progress({ phase: "applying", item: rows[2] });
    progress({ phase: "completed", result: { ...rows[2], status: "updated" } });
    progress({ phase: "checkingConflicts" });
  });
  expect(screen.getByRole("status")).toHaveTextContent("Checking Conflicts… 3 of 3 completed.");
  expect(screen.queryByRole("button", { name: "Stop after this Mod" })).not.toBeInTheDocument();
  expect(invoke.mock.calls.filter(([command]) => command === "stop_mod_update_batch")).toHaveLength(0);
  await act(async () => resolve({ stopped: false, results: rows.map((item) => ({ ...item, status: "updated" })), conflicts: { conflicts: [], per_mod_count: {} }, conflictError: null }));
  const apply = screen.getByRole("button", { name: "Apply all Mod updates" });
  await waitFor(() => expect(apply).toBeEnabled());
  fireEvent.click(apply);
  await waitFor(() => expect(invoke.mock.calls.filter(([command]) => command === "apply_all_mod_updates")).toHaveLength(2));
  expect(screen.getByRole("button", { name: "Stop after this Mod" })).toBeEnabled();
  expect(screen.getByRole("status")).toHaveTextContent("Starting updates… 0 of 3 completed.");
});
it("keeps stop delivery failure visible and allows a retry", async () => {
  renderWithQuery(view);
  await start();
  invoke.mockRejectedValueOnce(new Error("connection lost"));
  fireEvent.click(screen.getByRole("button", { name: "Stop after this Mod" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Stop request unverified: connection lost The batch may continue. Try again.");
  const stop = screen.getByRole("button", { name: "Stop after this Mod" });
  expect(stop).toBeEnabled();
  fireEvent.click(stop);
  await waitFor(() => expect(stop).toBeDisabled());
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});
it("does not expose a stale stop token on another Game and a late stop leaves a finished batch", async () => {
  const { rerender } = renderWithQuery(view);
  await start();
  rerender(<ModUpdateBatchAction game="srmi" rows={[]} verified checking={false} />);
  expect(screen.queryByRole("button", { name: "Stop after this Mod" })).not.toBeInTheDocument();
  rerender(view);
  fireEvent.click(screen.getByRole("button", { name: "Stop after this Mod" }));
  await act(async () => resolve({ stopped: false, results: rows.map((item) => ({ ...item, status: "updated" })), conflicts: { conflicts: [], per_mod_count: {} }, conflictError: null }));
  expect(screen.getByRole("status")).toHaveTextContent("Batch finished.");
  expect(screen.queryByRole("button", { name: "Stop after this Mod" })).not.toBeInTheDocument();
});

it("ignores a delayed stop error after the batch has already finished", async () => {
  renderWithQuery(view);
  await start();
  let rejectStop!: (error: Error) => void;
  invoke.mockImplementationOnce(() => new Promise<void>((_resolve, reject) => { rejectStop = reject; }));
  fireEvent.click(screen.getByRole("button", { name: "Stop after this Mod" }));
  await act(async () => resolve({ stopped: false, results: rows.map((item) => ({ ...item, status: "updated" })), conflicts: { conflicts: [], per_mod_count: {} }, conflictError: null }));
  await act(async () => rejectStop(new Error("late connection failure")));
  expect(screen.getByRole("status")).toHaveTextContent("Batch finished.");
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Stop after this Mod" })).not.toBeInTheDocument();
});
