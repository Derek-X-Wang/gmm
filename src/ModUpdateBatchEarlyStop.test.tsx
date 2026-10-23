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

it("offers Stop before any progress arrives and gives each batch its own frontend id", async () => {
  renderWithQuery(view);
  const ids: string[] = [];
  for (let attempt = 0; attempt < 2; attempt++) {
    const apply = screen.getByRole("button", { name: "Apply all Mod updates" });
    await waitFor(() => expect(apply).toBeEnabled());
    fireEvent.click(apply);
    const stop = await screen.findByRole("button", { name: "Stop after this Mod" });
    await waitFor(() => expect(invoke.mock.calls.filter(([command]) => command === "apply_all_mod_updates")).toHaveLength(attempt + 1));
    const batchId = invoke.mock.calls.filter(([command]) => command === "apply_all_mod_updates")[attempt][1].batchId;
    expect(batchId).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i);
    ids.push(batchId);
    fireEvent.click(stop);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("stop_mod_update_batch", { batchId }));
    await act(async () => resolve({ stopped: true, results: [], conflicts: null, conflictError: "Mods not attempted" }));
  }
  expect(ids[0]).not.toBe(ids[1]);
});
