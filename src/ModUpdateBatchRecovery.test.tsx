import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { QueryClient, useQuery } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import { renderWithQuery } from "./test/harness";
import { ModUpdateBatchAction } from "./ModUpdateBatchAction";
import type { ConflictReport, ModUpdateBatchReport, ModUpdateProgress, ModUpdateRow } from "./api";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({
  invoke,
  Channel: class { onmessage?: (event: ModUpdateProgress) => void },
}));
const rows: ModUpdateRow[] = ["First", "Last"].map((name) => ({
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
const view = <ModUpdateBatchAction game="gimi" rows={rows} verified checking={false} />;
async function start() {
  const button = screen.getByRole("button", { name: "Apply all Mod updates" });
  await waitFor(() => expect(button).toBeEnabled());
  fireEvent.click(button);
  await waitFor(() => expect(progress).toBeTypeOf("function"));
}

// Observe the same shared key and cache defaults used by the mounted Mod list.
// No remount or focus event can rescue a missing invalidation in these tests.
function ConflictCount({ read }: { read: () => Promise<ConflictReport> }) {
  const query = useQuery({ queryKey: ["conflicts", "gimi"], queryFn: read });
  return <p>Shared Conflict count: {query.data?.conflicts.length}</p>;
}
it.each(["incomplete report", "IPC loss"])("refreshes the mounted Conflict reading after %s", async (failure) => {
  const client = new QueryClient({ defaultOptions: {
    queries: { retry: false, staleTime: 30_000, refetchOnWindowFocus: false, gcTime: 0 },
    mutations: { retry: false },
  } });
  client.setQueryData(["conflicts", "gimi"], {
    conflicts: [{ hash: "old1", mod_ids: [], sections: [] }, { hash: "old2", mod_ids: [], sections: [] }], omitted: [], per_mod_count: {},
  });
  const read = vi.fn().mockResolvedValue({ conflicts: [], omitted: [], per_mod_count: {} });
  renderWithQuery(<>{view}<ConflictCount read={read} /></>, { client });
  expect(screen.getByText("Shared Conflict count: 2")).toBeVisible();
  expect(read).not.toHaveBeenCalled();
  await start();
  act(() => progress({ phase: "started", mods: rows }));
  await act(async () => {
    if (failure === "IPC loss") reject(new Error("connection lost"));
    else resolve({ stopped: false, results: [], conflicts: { conflicts: [], omitted: [], per_mod_count: {} }, conflictError: "recovery pending" });
  });
  await waitFor(() => expect(read, "the mounted Conflict query must refetch after every batch outcome").toHaveBeenCalledTimes(1));
  expect(await screen.findByText("Shared Conflict count: 0")).toBeVisible();
});
it("shows a clean preflight refusal without inventing Unknown Mod outcomes or Library damage", async () => {
  renderWithQuery(view);
  await start();
  await act(async () => reject(new Error("An interrupted enable/disable transition needs recovery")));
  expect(await screen.findByText("Batch refused before updates started.")).toBeVisible();
  expect(screen.getByRole("alert")).toHaveTextContent("interrupted enable/disable transition");
  expect(screen.queryByText(/Unknown|Check the Library|Conflict check result unavailable/)).not.toBeInTheDocument();
});
it("keeps a started batch unknown when its final result is lost", async () => {
  renderWithQuery(view);
  await start();
  act(() => progress({ phase: "started", mods: rows }));
  await act(async () => reject(new Error("connection lost")));
  expect(await screen.findByText("Batch result unavailable.")).toBeVisible();
  expect(screen.getByText(/^First: Unknown/)).toHaveTextContent("Check the Library before retrying");
  expect(screen.getByText(/^Last: Unknown/)).toBeVisible();
});
it("displays skipped recovery separately from failures and keeps incomplete Conflicts qualified", async () => {
  renderWithQuery(view);
  await start();
  await act(async () => resolve({
    stopped: false,
    results: [{ ...rows[0], status: "skipped", reason: "Interrupted update recovery required" }, { ...rows[1], status: "updated" }],
    conflicts: { conflicts: [], omitted: [], per_mod_count: {} }, conflictError: "An enabled Mod is quarantined",
  }));
  expect(await screen.findByText("First: Skipped — Interrupted update recovery required")).toBeVisible();
  expect(screen.getByText("Last: Updated")).toBeVisible();
  expect(screen.getByRole("alert")).toHaveTextContent("Conflicts unverified");
  expect(screen.queryByText("0 Conflicts found after the batch.")).not.toBeInTheDocument();
});
