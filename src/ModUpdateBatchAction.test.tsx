import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { renderWithQuery } from "./test/harness";
import { ModUpdateBatchAction } from "./ModUpdateBatchAction";
import type { ModUpdateBatchReport, ModUpdateProgress, ModUpdateRow } from "./api";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({
  invoke,
  Channel: class { onmessage?: (event: ModUpdateProgress) => void },
}));
const rows: ModUpdateRow[] = ["First", "Broken", "Last"].map((name) => ({
  modId: name, name, installedVersion: "1", upstreamVersion: "2", upstreamAhead: true, updateCheckEnabled: true,
}));
let progress: (event: ModUpdateProgress) => void;
let resolve: (report: ModUpdateBatchReport) => void;
let reject: (error: Error) => void;
beforeEach(() => {
  progress = undefined!;
  invoke.mockReset().mockImplementation((command: string, args?: { onProgress: { onmessage: typeof progress } }) => {
    if (command === "current_session") return Promise.resolve(null);
    if (command === "apply_all_mod_updates") {
      progress = args!.onProgress.onmessage;
      return new Promise<ModUpdateBatchReport>((yes, no) => { resolve = yes; reject = no; });
    }
    throw new Error(`Unexpected command: ${command}`);
  });
});
const view = (verified = true) => <ModUpdateBatchAction game="gimi" rows={rows} verified={verified} checking={false} />;
const start = async () => {
  const button = await screen.findByRole("button", { name: "Apply all Mod updates" });
  await waitFor(() => expect(button).toBeEnabled());
  fireEvent.click(button);
  await waitFor(() => expect(progress).toBeTypeOf("function"));
};
it("names in-flight Mods, displays partial outcomes, and publishes the single backend Conflict reading", async () => {
  const { client } = renderWithQuery(view());
  const setData = vi.spyOn(client, "setQueryData");
  const invalidate = vi.spyOn(client, "invalidateQueries");
  await start();
  act(() => {
    progress({ phase: "started", mods: rows });
    progress({ phase: "applying", item: rows[0] });
  });
  expect(screen.getByRole("status")).toHaveTextContent("Updating First… 0 of 3 completed.");
  expect(screen.getByRole("button")).toBeDisabled();
  act(() => {
    progress({ phase: "completed", result: { ...rows[0], status: "updated" } });
    progress({ phase: "applying", item: rows[1] });
  });
  expect(screen.getByText("First: Updated")).toBeVisible();
  expect(screen.getByRole("status")).toHaveTextContent("Updating Broken… 1 of 3 completed.");
  const report: ModUpdateBatchReport = {
    results: [
      { ...rows[0], status: "updated" },
      { ...rows[1], status: "failed", reason: "download unavailable" },
      { ...rows[2], status: "updated" },
    ],
    conflicts: { conflicts: [{ hash: "abc", mod_ids: ["First", "Last"], sections: [] }], per_mod_count: { First: 1, Last: 1 } },
    conflictError: null,
  };
  await act(async () => resolve(report));
  expect(await screen.findByText("Broken: Failed — download unavailable")).toBeVisible();
  expect(screen.getByText("Last: Updated")).toBeVisible();
  expect(screen.getByText("1 Conflicts found after the batch.")).toBeVisible();
  expect(screen.getByText("Hash abc: First, Last")).toBeVisible();
  expect(setData).toHaveBeenCalledWith(["conflicts", "gimi"], report.conflicts);
  expect(invoke.mock.calls.filter(([command]) => command === "detect_conflicts")).toHaveLength(0);
  expect(invalidate).toHaveBeenCalledWith({ queryKey: ["mods", "gimi"] });
  expect(invalidate).toHaveBeenCalledWith({ queryKey: ["attentionStatus"] });
});
it("preserves established outcomes and labels unreceived results unknown after IPC failure", async () => {
  const { client } = renderWithQuery(view());
  const invalidate = vi.spyOn(client, "invalidateQueries");
  await start();
  act(() => progress({ phase: "completed", result: { ...rows[0], status: "updated" } }));
  await act(async () => reject(new Error("connection lost")));
  expect(await screen.findByText("First: Updated")).toBeVisible();
  expect(screen.getByText(/^Broken: Unknown/)).toHaveTextContent("No result received");
  expect(screen.getByText(/^Last: Unknown/)).toBeVisible();
  expect(screen.getByRole("alert")).toHaveTextContent("Conflict check result unavailable");
  expect(invalidate).toHaveBeenCalledWith({ queryKey: ["modUpdates", "gimi"] });
  expect(invalidate).toHaveBeenCalledWith({ queryKey: ["attentionStatus"] });
});
it("keeps rollback uncertainty and failed Conflict detection visible", async () => {
  renderWithQuery(view());
  await start();
  await act(async () => resolve({ results: [{ ...rows[0], status: "unknown", reason: "rollback unresolved" }], conflicts: null, conflictError: "quarantine pending" }));
  expect(await screen.findByText("First: Unknown — rollback unresolved")).toBeVisible();
  expect(screen.getByRole("alert")).toHaveTextContent("Conflicts unverified: quarantine pending");
  expect(screen.queryByText("0 Conflicts found after the batch.")).not.toBeInTheDocument();
});
it("blocks a live session and blocks a failed refresh even with retained inactive data", async () => {
  const { client } = renderWithQuery(view());
  await waitFor(() => expect(screen.getByRole("button")).toBeEnabled());
  act(() => client.setQueryData(["session"], { game: "gimi", pid: 1, startedAt: "now" }));
  await waitFor(() => expect(screen.getByRole("button")).toBeDisabled());
  expect(screen.getByText("End the Game Session before applying Mod updates.")).toBeVisible();
  act(() => client.setQueryData(["session"], null));
  invoke.mockRejectedValue(new Error("session unavailable"));
  await act(async () => { await client.invalidateQueries({ queryKey: ["session"] }); });
  expect(screen.getByRole("button")).toBeDisabled();
  expect(await screen.findByText(/Cannot verify Game Session/)).toBeVisible();
  fireEvent.click(screen.getByRole("button"));
  expect(invoke.mock.calls.filter(([command]) => command === "apply_all_mod_updates")).toHaveLength(0);
});
it("never offers Apply all from unverified update rows", async () => {
  renderWithQuery(view(false));
  await screen.findByText("Available Mod updates are unverified.");
  expect(screen.getByRole("button")).toBeDisabled();
});
it("keeps the running Game's results separate when switching Games", async () => {
  const { rerender } = renderWithQuery(view());
  await start();
  rerender(<ModUpdateBatchAction game="srmi" rows={[]} verified checking={false} />);
  expect(screen.getByText("Mod updates are running for gimi.")).toBeVisible();
  act(() => progress({ phase: "completed", result: { ...rows[0], status: "updated" } }));
  expect(screen.queryByText("First: Updated")).not.toBeInTheDocument();
  await act(async () => resolve({ results: [{ ...rows[0], status: "updated" }], conflicts: { conflicts: [], per_mod_count: {} }, conflictError: null }));
  rerender(view());
  expect(screen.getByText("First: Updated")).toBeVisible();
});
