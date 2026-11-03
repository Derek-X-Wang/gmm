import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { useQuery } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import { GamePresets } from "./GamePresets";
import { detectConflicts, type ConflictReport, type PresetApplyProgress, type PresetApplyReport } from "./api";
import { renderWithQuery } from "./test/harness";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke, Channel: class { onmessage?: (event: PresetApplyProgress) => void } }));
const empty: ConflictReport = { conflicts: [], per_mod_count: {}, omitted: [] };
let report: PresetApplyReport;
let conflicts: ConflictReport;
let applyCommand: (args: { game: string; onProgress: { onmessage: (event: PresetApplyProgress) => void } }) => Promise<PresetApplyReport>;
beforeEach(() => {
  conflicts = { conflicts: [1, 2].map((n) => ({ hash: String(n), mod_ids: ["m", "n"], sections: [] })), per_mod_count: {}, omitted: [] };
  report = {
    results: [
      { modId: "m", name: "Saved Mod", targetEnabled: true, status: "applied", reason: "Saved Mod is enabled with its recorded Variant selection." },
      { modId: "new", name: "Added yesterday", targetEnabled: false, status: "applied", reason: "Added yesterday is disabled because it is not in this preset, including Mods added after it was saved." },
    ], conflicts: empty, conflictError: null,
  };
  applyCommand = async () => { conflicts = empty; return report; };
  invoke.mockReset().mockImplementation((command: string, args: never) => {
    if (command === "list_presets") return Promise.resolve([{ id: "p", game: "gimi", name: "Screenshot", createdAt: "now", modCount: 1 }]);
    if (command === "apply_preset") return applyCommand(args);
    if (command === "detect_conflicts") return Promise.resolve(conflicts);
    throw new Error(`Unexpected command: ${command}`);
  });
});
function SharedConflicts() {
  const reading = useQuery({ queryKey: ["conflicts", "gimi"], queryFn: () => detectConflicts("gimi") });
  return <p>Shared Conflict count: {reading.data?.conflicts.length ?? "loading"}</p>;
}
async function apply() {
  fireEvent.click(await screen.findByRole("button", { name: "Apply Screenshot" }));
}

it("refreshes the shared pre-apply Conflict count unconditionally after applying", async () => {
  const { client } = renderWithQuery(<><GamePresets game="gimi" /><SharedConflicts /></>);
  const invalidation = vi.spyOn(client, "invalidateQueries");
  await screen.findByText("Shared Conflict count: 2");
  await apply();
  expect(await screen.findByText("Shared Conflict count: 0")).toBeVisible();
  expect(invalidation).toHaveBeenCalledWith({ queryKey: ["conflicts", "gimi"] });
  expect(invoke).toHaveBeenCalledWith("apply_preset", { game: "gimi", presetId: "p", onProgress: expect.any(Object) });
  expect(screen.getByText(/Added yesterday: applied/)).toHaveTextContent("disabled because it is not in this preset");
  expect(screen.getByText(/Applying a preset enables/)).toHaveTextContent("including Mods added after it was saved");
});

it("shows the in-flight Mod and completed outcomes while the command remains pending", async () => {
  let finish!: (value: PresetApplyReport) => void;
  let progress!: (event: PresetApplyProgress) => void;
  applyCommand = (args) => { progress = args.onProgress.onmessage; return new Promise((resolve) => { finish = resolve; }); };
  renderWithQuery(<GamePresets game="gimi" />);
  await apply();
  await waitFor(() => expect(progress).toBeDefined());
  act(() => {
    progress({ phase: "started", mods: report.results.map(({ modId, name, targetEnabled }) => ({ modId, name, targetEnabled })) });
    progress({ phase: "completed", result: report.results[0] });
    progress({ phase: "applying", item: report.results[1] });
  });
  expect(screen.getByRole("status")).toHaveTextContent("Applying Added yesterday… 1 of 2 Mods completed.");
  expect(screen.getByText(/Saved Mod: applied/)).toBeVisible();
  expect(screen.getByRole("button", { name: "Apply Screenshot" })).toBeDisabled();
  await act(async () => { finish(report); });
  expect(screen.getByRole("status")).toHaveTextContent("Preset apply finished");
});

it("keeps skipped, failed and unknown outcomes distinct and qualifies incomplete Conflict scans", async () => {
  report = { results: [
    { modId: "s", name: "Recovery Mod", targetEnabled: true, status: "skipped", reason: "Recovery Mod needs recovery." },
    { modId: "f", name: "Lost Variant", targetEnabled: true, status: "failed", reason: "Lost Variant: recorded Variant is gone." },
    { modId: "u", name: "Interrupted", targetEnabled: true, status: "unknown", reason: "Interrupted has an unverified final state." },
  ], conflicts: { ...empty, omitted: [{ mod_id: "s", cause: "unestablished" }, { mod_id: "u", cause: "unestablished" }] }, conflictError: "The full Conflict reading is unverified." };
  renderWithQuery(<GamePresets game="gimi" />);
  await apply();
  expect(await screen.findByText(/Recovery Mod: skipped/)).toBeVisible();
  expect(screen.getByText(/Lost Variant: failed/)).toBeVisible();
  expect(screen.getByText(/Interrupted: unknown/)).toBeVisible();
  expect(screen.getByRole("alert")).toHaveTextContent("Conflicts unverified");
  expect(screen.getByText("0 known Conflicts; the full reading is unverified.")).toBeVisible();
  expect(screen.queryByText("0 Conflicts found after applying the preset.")).not.toBeInTheDocument();
});

it("marks unreceived per-Mod results unknown after a transport failure and still refreshes shared Conflicts", async () => {
  applyCommand = async (args) => {
    args.onProgress.onmessage({ phase: "started", mods: report.results });
    args.onProgress.onmessage({ phase: "completed", result: report.results[0] });
    conflicts = empty;
    throw { kind: "other", message: "Result channel lost" };
  };
  renderWithQuery(<><GamePresets game="gimi" /><SharedConflicts /></>);
  await screen.findByText("Shared Conflict count: 2");
  await apply();
  expect(await screen.findByText(/Added yesterday: unknown/)).toBeVisible();
  expect(screen.getByText(/Saved Mod: applied/)).toBeVisible();
  expect(screen.getByRole("alert")).toHaveTextContent("Result channel lost");
  expect(await screen.findByText("Shared Conflict count: 0")).toBeVisible();
});

it("surfaces a pre-flight refusal without claiming any Mod changed", async () => {
  applyCommand = async () => { throw { kind: "other", message: "A Game Session is active. End it before applying a preset." }; };
  const { client } = renderWithQuery(<GamePresets game="gimi" />);
  const invalidate = vi.spyOn(client, "invalidateQueries");
  await apply();
  expect(await screen.findByRole("alert")).toHaveTextContent("A Game Session is active");
  expect(screen.queryByText(/: applied/)).not.toBeInTheDocument();
  expect(screen.queryByText(/Preset apply finished/)).not.toBeInTheDocument();
  expect(invalidate).toHaveBeenCalledWith({ queryKey: ["conflicts", "gimi"] });
});

it("a removed view does not cancel the pending command", async () => {
  let finish!: (value: PresetApplyReport) => void;
  let progress!: (event: PresetApplyProgress) => void;
  applyCommand = (args) => { progress = args.onProgress.onmessage; return new Promise((resolve) => { finish = resolve; }); };
  const { unmount, client } = renderWithQuery(<GamePresets game="gimi" />);
  const invalidate = vi.spyOn(client, "invalidateQueries");
  await apply();
  await waitFor(() => expect(progress).toBeDefined());
  unmount();
  await act(async () => { progress({ phase: "checkingConflicts" }); finish(report); });
  await waitFor(() => expect(invalidate).toHaveBeenCalledWith({ queryKey: ["conflicts", "gimi"] }));
});

it("applies different Games concurrently and serializes the same Game across mounted preset views", async () => {
  const running: { game: string; finish: (value: PresetApplyReport) => void }[] = [];
  applyCommand = (args) => new Promise((finish) => { running.push({ game: args.game, finish }); });
  renderWithQuery(<>
    <section aria-label="first"><GamePresets game="gimi" /></section>
    <section aria-label="queued"><GamePresets game="gimi" /></section>
    <section aria-label="parallel"><GamePresets game="srmi" /></section>
  </>);
  const button = (label: string) => within(screen.getByRole("region", { name: label })).getByRole("button", { name: "Apply Screenshot" });
  await waitFor(() => expect(screen.getAllByRole("button", { name: "Apply Screenshot" })).toHaveLength(3));
  fireEvent.click(button("first"));
  await waitFor(() => expect(running.map((item) => item.game)).toEqual(["gimi"]));
  fireEvent.click(button("queued"));
  fireEvent.click(button("parallel"));
  await waitFor(() => expect(running.map((item) => item.game)).toEqual(["gimi", "srmi"]));
  await act(async () => { running[0].finish(report); });
  await waitFor(() => expect(running.map((item) => item.game)).toEqual(["gimi", "srmi", "gimi"]));
  await act(async () => { running[1].finish(report); running[2].finish(report); });
});
