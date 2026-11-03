import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { renderWithQuery } from "./test/harness";
import { ModUpdateBatchAction } from "./ModUpdateBatchAction";
import type { Mod, ModUpdateBatchReport, ModUpdateProgress, ModUpdateRow } from "./api";

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
const view = (verified = true) => <ModUpdateBatchAction game="gimi" rows={rows} mods={rows.map(({ modId: id, name }) => ({ id, name }))} verified={verified} checking={false} />;
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
  expect(screen.getByRole("status")).toHaveTextContent("Updating First… 0 of 3 processed.");
  expect(screen.getByRole("button", { name: "Applying Mod updates…" })).toBeDisabled();
  act(() => {
    progress({ phase: "completed", result: { ...rows[0], status: "updated" } });
    progress({ phase: "applying", item: rows[1] });
  });
  expect(screen.getByText("First: Updated")).toBeVisible();
  expect(screen.getByRole("status")).toHaveTextContent("Updating Broken… 1 of 3 processed.");
  const report: ModUpdateBatchReport = {
    stopped: false,
    results: [
      { ...rows[0], status: "updated" },
      { ...rows[1], status: "failed", reason: "download unavailable" },
      { ...rows[2], status: "updated" },
    ],
    conflicts: { conflicts: [{ hash: "abc", mod_ids: ["First", "Last"], sections: [] }], omitted: [], per_mod_count: { First: 1, Last: 1 } },
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
  await act(async () => resolve({ stopped: false, results: [{ ...rows[0], status: "unknown", reason: "rollback unresolved" }], conflicts: null, conflictError: "quarantine pending" }));
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
  rerender(<ModUpdateBatchAction game="srmi" rows={[]} mods={[]} verified checking={false} />);
  expect(screen.getByText("Mod updates are running for gimi.")).toBeVisible();
  act(() => progress({ phase: "completed", result: { ...rows[0], status: "updated" } }));
  expect(screen.queryByText("First: Updated")).not.toBeInTheDocument();
  await act(async () => resolve({ stopped: false, results: [{ ...rows[0], status: "updated" }], conflicts: { conflicts: [], omitted: [], per_mod_count: {} }, conflictError: null }));
  rerender(view());
  expect(screen.getByText("First: Updated")).toBeVisible();
});

it.each([0, 1])("qualifies the post-batch reading when Mods were omitted (%i known Conflicts)", async (count) => {
  renderWithQuery(view());
  await start();
  await act(async () => resolve({
    stopped: false, results: [],
    conflictError: "The full Conflict reading is unverified because the scan omitted Mods needing recovery.",
    conflicts: {
      conflicts: count ? [{ hash: "shared", mod_ids: ["First", "Last"], sections: [] }] : [],
      per_mod_count: {}, omitted: [{ mod_id: "Broken", cause: "unestablished" }],
    },
  }));
  expect(await screen.findByText(`${count} known Conflicts found; the full result is unverified.`)).toBeVisible();
  expect(screen.queryByText(`${count} Conflicts found after the batch.`)).not.toBeInTheDocument();
});

it("calls skipped in-flight outcomes processed", async () => {
  renderWithQuery(view());
  await start();
  act(() => {
    progress({ phase: "started", mods: rows });
    progress({ phase: "completed", result: { ...rows[0], status: "skipped", reason: "Recovery required" } });
    progress({ phase: "applying", item: rows[1] });
  });
  expect(screen.getByRole("status")).toHaveTextContent("Updating Broken… 1 of 3 processed.");
  expect(screen.getByRole("status")).not.toHaveTextContent("completed");
  expect(screen.getByText("First: Skipped — Recovery required")).toBeVisible();
});

it.each(["local", "manual"] as const)("names a %s Conflict participant from its Mod record", async (source) => {
  const participant: Mod = {
    id: "01JMM426000000000000000001", game: "gimi", name: "Recorded costume — 完整名称", source,
    libraryPath: "library/costume", enabled: true, gamebananaId: null, sourceUrl: null,
    author: null, version: null, screenshotUrl: null, reinstallRecovery: null, enabledTransitionRecovery: null,
  };
  renderWithQuery(<ModUpdateBatchAction game="gimi" rows={rows}
    mods={[{ id: "First", name: "Recorded GameBanana name" }, participant]} verified checking={false} />);
  await start();
  await act(async () => resolve({
    stopped: false, results: [], conflictError: null,
    conflicts: { conflicts: [{ hash: "mixed", mod_ids: ["First", participant.id], sections: [] }], omitted: [], per_mod_count: {} },
  }));
  expect(await screen.findByText(/^Hash mixed:/)).toHaveTextContent("Hash mixed: Recorded GameBanana name, Recorded costume — 完整名称");
  expect(screen.queryByText(new RegExp(participant.id))).not.toBeInTheDocument();
});

it("keeps the exact identifier for a Conflict participant in neither list", async () => {
  const missingId = "01JMM426000000000000000002";
  renderWithQuery(<ModUpdateBatchAction game="gimi" rows={rows}
    mods={[{ id: "First", name: "Recorded GameBanana name" }, { id: `${missingId}0`, name: "Similar identifier is not a match" }]}
    verified checking={false} />);
  await start();
  await act(async () => resolve({
    stopped: false, results: [], conflictError: null,
    conflicts: { conflicts: [{ hash: "missing", mod_ids: ["First", missingId], sections: [] }], omitted: [], per_mod_count: {} },
  }));
  expect(await screen.findByText(/^Hash missing:/)).toHaveTextContent(`Hash missing: Recorded GameBanana name, ${missingId}`);
});

it("names all-in-flight omissions without claiming recovery", async () => {
  renderWithQuery(view());
  await start();
  await act(async () => resolve({
    stopped: false, results: [],
    conflicts: { conflicts: [], per_mod_count: {}, omitted: [{ mod_id: "First", cause: "inFlight" }] },
    conflictError: "The full Conflict reading is unverified.",
  }));
  expect(await screen.findByRole("alert")).toHaveTextContent(/^Conflicts unverified: The full Conflict reading is unverified because the scan omitted Mods with enable\/disable transitions in flight\.$/);
  expect(screen.queryByText(/recovery/i)).not.toBeInTheDocument();
  expect(screen.getByText("0 known Conflicts found; the full result is unverified.")).toBeVisible();
});

it("names both omission causes in a mixed batch", async () => {
  renderWithQuery(view());
  await start();
  await act(async () => resolve({
    stopped: false, results: [],
    conflicts: { conflicts: [], per_mod_count: {}, omitted: [{ mod_id: "First", cause: "inFlight" }, { mod_id: "Broken", cause: "unestablished" }] },
    conflictError: "The full Conflict reading is unverified.",
  }));
  expect(await screen.findByRole("alert")).toHaveTextContent(/^Conflicts unverified: The full Conflict reading is unverified because the scan omitted Mods with enable\/disable transitions in flight and Mods needing recovery\.$/);
});
