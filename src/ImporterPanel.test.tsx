import { act, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { onlineManager, useQuery } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import { makeQueryClient, renderWithQuery } from "./test/harness";
import { ImporterPanel } from "./ImporterPanel";
import type { GamePathStatus, ImporterEvacuationRecovery, UpdateStatus } from "./api";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
let path: GamePathStatus;
let update: UpdateStatus;
let recovery: ImporterEvacuationRecovery | null;
let commands: Record<string, () => unknown>;
const recoveryValue: ImporterEvacuationRecovery = {
  action: "retry", ownerUncertain: false, reason: "Recovery locked",
  gamePath: "C:/Game", backupPath: "C:/Backup", attempts: 1, attemptedAt: "2026-10-11",
};
beforeEach(() => {
  path = { pinnedVersion: null, game: "gimi", savedPath: "C:/Game", gamePath: { status: "readable" }, modelImporter: { status: "installed", record: { installedVersion: "1.0", origin: { state: "unknown" } } } };
  update = { available: false, installedVersion: "1.0", latestVersion: "1.0", pinned: false, upstreamAhead: false, checkError: null };
  recovery = null;
  commands = {};
  invoke.mockReset().mockImplementation(async (command: string) => {
    if (commands[command]) return commands[command]();
    if (command === "get_game_path_status") return path;
    if (command === "check_importer_update") return update;
    if (command === "get_importer_evacuation_recovery") return recovery;
    if (command === "fetch_latest_importer_release") return { tag_name: "1.0" };
    if (command === "check_loader_update") return { shippedVersion: "1.0", latestVersion: "1.0", upstreamAhead: false, checkError: null };
    return null;
  });
});
function render() { return renderWithQuery(<ImporterPanel game="gimi" displayName="Genshin Impact" />); }
function deferred() {
  let resolve!: (value: unknown) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
it("keeps checking distinct and disables writes before filesystem and recovery checks complete", async () => {
  const pending = deferred();
  commands.get_game_path_status = () => pending.promise;
  commands.get_importer_evacuation_recovery = () => pending.promise;
  commands.fetch_latest_importer_release = () => pending.promise;
  commands.check_importer_update = () => pending.promise;
  render();
  expect(await screen.findByText("Checking Game and Model Importer…")).toBeVisible();
  expect(screen.getByText(/Latest release: checking/)).toBeVisible();
  expect(screen.getByRole("button", { name: "Importer installation unavailable" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "Roll back importer" })).toBeDisabled();
});
it("shows verified installed facts, unpinned state and the embedded Loader note", async () => {
  render();
  expect(await screen.findByText("GIMI installed · Game readable")).toBeVisible();
  expect(screen.getByText(/Installed/)).toHaveTextContent("1.0");
  expect(screen.getByText(/· Unpinned/)).toBeVisible();
  expect(screen.getByText(/GMM ships Loader/)).toBeVisible();
  expect(screen.getByRole("button", { name: "Pin to current" })).toBeEnabled();
});
it("allows an explicit install for a readable Game with confirmed absent importer, without inventing installed metadata", async () => {
  path.modelImporter = { status: "noImporterEntriesFound", paths: ["C:/Game/Core"] };
  render();
  expect(await screen.findByText("Cannot tell — no GMM install recorded; no Model Importer entries found")).toBeVisible();
  expect(screen.getByRole("button", { name: "Install importer" })).toBeEnabled();
  expect(screen.queryByText(/· Installed/)).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Pin to current" })).not.toBeInTheDocument();
});
it.each([
  ["no saved path", { status: "noSavedPath" }, { status: "unverified", record: null, path: null, error: "No path" }, "Set a readable Game install path before installing a Model Importer.", "Saved Game installation cannot be found"],
  ["Game entries missing", { status: "requiredEntriesNotFound", paths: ["C:/Game/Game.exe"] }, { status: "unverified", record: null, path: null, error: "Missing executable" }, "Required Game files or directories not found · Missing Game entries: C:/Game/Game.exe · Model Importer unverified", "Saved Game installation cannot be found"],
  ["saved path missing", { status: "savedPathNotFound", paths: ["C:/Game"] }, { status: "unverified", record: null, path: null, error: "Missing executable" }, "Saved Game installation cannot be found · Model Importer unverified", "Required Game files or directories not found"],
  ["unreadable Game", { status: "unreadable", path: "C:/Game", error: "Access denied" }, { status: "unverified", record: null, path: "C:/Game", error: "Access denied" }, "Cannot tell · Model Importer filesystem unverified", "Saved Game installation cannot be found"],
  ["invalid Game", { status: "invalid", error: "Wrong layout" }, { status: "unverified", record: null, path: null, error: "Wrong layout" }, "Cannot tell · Model Importer filesystem unverified", "Saved Game installation cannot be found"],
  ["unverified importer", { status: "readable" }, { status: "unverified", record: null, path: "C:/Game/Core", error: "Sharing violation" }, "Cannot tell · Model Importer filesystem unverified", "Saved Game installation cannot be found"],
] as const)("does not offer writes or infer a version for %s", async (_, gamePath, modelImporter, line, other) => {
  path = { ...path, gamePath: { ...gamePath }, modelImporter: { ...modelImporter } } as GamePathStatus;
  render();
  expect(await screen.findByText(line)).toBeVisible();
  expect(screen.queryByText(new RegExp(other))).not.toBeInTheDocument();
  expect(screen.getByRole("heading", { name: "Model Importer (GIMI)" })).toBeVisible();
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("get_game_path_status", { game: "gimi" }));
  expect(screen.getByRole("button", { name: "Importer installation unavailable" })).toBeDisabled();
  expect(screen.queryByText(/· Installed/)).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Pin to current" })).not.toBeInTheDocument();
  expect(screen.queryByText("Cannot tell — no GMM install recorded; no Model Importer entries found")).not.toBeInTheDocument();
});
it("retains the actual filesystem failure rather than declaring absence", async () => {
  commands.get_game_path_status = () => { throw new Error("Permission denied reading Core"); };
  render();
  expect(await screen.findByText("Permission denied reading Core")).toBeVisible();
  expect(screen.getByText("Cannot tell · Model Importer filesystem unverified")).toBeVisible();
  expect(screen.getByRole("button", { name: "Importer installation unavailable" })).toBeDisabled();
});
it.each(["fetch_latest_importer_release", "check_importer_update"])("never calls a rejected %s current", async (command) => {
  commands[command] = () => { throw new Error("Upstream unavailable"); };
  render();
  expect(await screen.findByText("Upstream unavailable")).toBeVisible();
  expect(screen.queryByText("Model Importer is current.")).not.toBeInTheDocument();
  if (command === "fetch_latest_importer_release") expect(screen.getByText(/Latest release: check failed/)).toBeVisible();
});
it("distinguishes an available update from a check failure and a pin", async () => {
  update.available = true; update.latestVersion = "2.0"; update.upstreamAhead = true;
  const { client } = render();
  expect(await screen.findByText("Update available: 2.0")).toBeVisible();
  expect(screen.getByRole("button", { name: "Apply update" })).toBeEnabled();
  update = { ...update, available: false, pinned: true };
  await act(() => client.invalidateQueries({ queryKey: ["importer", "update"] }));
  expect(await screen.findByText("Version prompts are suppressed by the Importer Pin.")).toBeVisible();
  expect(screen.getByRole("button", { name: "Unpin" })).toBeEnabled();
  update = { ...update, pinned: false, checkError: "Release asset ambiguous" };
  await act(() => client.invalidateQueries({ queryKey: ["importer", "update"] }));
  expect(await screen.findByText(/Release asset ambiguous/)).toBeVisible();
  expect(screen.queryByText("Model Importer is current.")).not.toBeInTheDocument();
});
it("reports no backup after rollback returns null", async () => {
  render();
  await userEvent.click(await screen.findByRole("button", { name: "Roll back importer" }));
  expect(await screen.findByText("No backup is available to roll back to.")).toBeVisible();
});
it.each([
  ["install_importer", "Reinstall importer", "Installing…", { sha256: "abcdef", backup_dir: "C:/Backup" }, "Installed. SHA-256"],
  ["rollback_importer", "Roll back importer", "Rolling back…", "C:/Backup", "Restored from"],
  ["set_importer_pinned", "Pin to current", "Saving pin…", null, "Importer Pin saved."],
  ["retry_importer_evacuation_recovery", "Retry Model Importer recovery", "Retrying recovery…", null, "Recovery retry completed."],
  ["retire_interrupted_importer_evacuation", "I confirmed no other GMM is changing this importer — retire producer", "Retiring producer…", null, "Importer recovery block released"],
] as const)("keeps %s pending, outcome and failure visible after its row disappears", async (command, label, pendingLabel, result, outcome) => {
  if (command.includes("evacuation")) recovery = { ...recoveryValue, action: command.startsWith("retire") ? "retireProducer" : "retry" };
  const pending = deferred(); commands[command] = () => pending.promise;
  render();
  await userEvent.click(await screen.findByRole("button", { name: label }));
  expect(screen.getByRole("button", { name: pendingLabel })).toBeDisabled();
  recovery = null;
  await act(async () => { pending.resolve(result); await pending.promise; });
  expect(await screen.findByText(new RegExp(outcome))).toBeVisible();
});
it.each(["retry_importer_evacuation_recovery", "retire_interrupted_importer_evacuation"])("retains rejected %s feedback after refreshed recovery disappears", async (command) => {
  recovery = { ...recoveryValue, action: command.startsWith("retire") ? "acknowledgeAndRelease" : "retry" };
  commands[command] = () => { recovery = null; throw new Error("Recovery outcome unverified"); };
  render();
  await userEvent.click(await screen.findByRole("button", { name: command.startsWith("retire") ? "I reviewed both locations — release the importer block" : "Retry Model Importer recovery" }));
  await waitFor(() => expect(screen.queryByRole("region", { name: /Interrupted Model/ })).not.toBeInTheDocument());
  expect(screen.getByText("Recovery outcome unverified")).toBeVisible();
});
// Two active projections and a fresh inactive Game cache exercise the real query
// boundary. The acceptance requires broad importer refresh and Game-scoped Origin.
function Projection({ name, reads }: { name: string; reads: Record<string, number> }) {
  const query = useQuery({ queryKey: [name === "origin" ? "importerOrigin" : "importer", name === "origin" ? "gimi" : name],
    queryFn: async () => { reads[name] = (reads[name] ?? 0) + 1; return reads[name]; } });
  return <output aria-label={name}>{query.data}</output>;
}
it.each(["install_importer", "rollback_importer", "retry_importer_evacuation_recovery", "retire_interrupted_importer_evacuation"])("settles all cached importer reads and this Game's Origin on successful and rejected %s", async (command) => {
  for (const rejects of [false, true]) {
    recovery = command.includes("evacuation") ? { ...recoveryValue, action: command.startsWith("retire") ? "retireProducer" : "retry" } : null;
    commands[command] = () => { if (rejects) throw new Error("Partial importer write"); return command === "install_importer" ? { sha256: "abc", backup_dir: null } : null; };
    const client = makeQueryClient();
    client.setDefaultOptions({ queries: { staleTime: 30_000, gcTime: 300_000, retry: false } });
    const reads: Record<string, number> = {};
    const view = renderWithQuery(<><ImporterPanel game="gimi" displayName="Genshin Impact" /><Projection name="main" reads={reads} /><Projection name="settings" reads={reads} /><Projection name="origin" reads={reads} /></>, { client });
    const inactiveKeys = [["importer", "latest", "srmi"], ["importerOrigin", "srmi"]];
    for (const key of inactiveKeys) client.setQueryData(key, "fresh");
    const label = command === "install_importer" ? "Reinstall importer" : command === "rollback_importer" ? "Roll back importer" : command.startsWith("retire") ? "I confirmed no other GMM is changing this importer — retire producer" : "Retry Model Importer recovery";
    await userEvent.click(await screen.findByRole("button", { name: label }));
    await waitFor(() => { for (const name of ["main", "settings", "origin"]) expect(reads[name]).toBe(2); });
    expect(client.getQueryState(inactiveKeys[0])?.isInvalidated).toBe(true);
    expect(client.getQueryState(inactiveKeys[1])?.isInvalidated).toBe(false);
    expect(invoke.mock.calls.filter(([name]) => name === "get_game_path_status")).toHaveLength(2);
    expect(invoke.mock.calls.filter(([name]) => name === "fetch_latest_importer_release").length).toBeGreaterThanOrEqual(2);
    if (rejects) expect(screen.getByText("Partial importer write")).toBeVisible();
    view.unmount(); client.clear(); invoke.mockClear();
  }
});

it.each([
  ["install_importer", "Reinstall importer"],
  ["rollback_importer", "Roll back importer"],
  ["set_importer_pinned", "Pin to current"],
])("retains structured %s rejection feedback", async (command, label) => {
  commands[command] = () => { throw { kind: "invalidActiveVariant", message: "Importer action rejected" }; };
  render();
  await userEvent.click(await screen.findByRole("button", { name: label }));
  expect(await screen.findByText("Importer action rejected")).toHaveAttribute("data-command-failure-kind", "invalidActiveVariant");
});
it("does not present stale installed facts as verified during or after a failed path refresh", async () => {
  const { client } = render();
  await screen.findByText("GIMI installed · Game readable");
  const pending = deferred(); commands.get_game_path_status = () => pending.promise;
  void client.invalidateQueries({ queryKey: ["gamePathStatus", "gimi"] });
  expect(await screen.findByText("Checking Game and Model Importer…")).toBeVisible();
  expect(screen.queryByText(/· Installed/)).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Importer installation unavailable" })).toBeDisabled();
  pending.reject(new Error("Path refresh unavailable"));
  expect(await screen.findByText("Path refresh unavailable")).toBeVisible();
  expect(screen.queryByText(/· Installed/)).not.toBeInTheDocument();
});
it("holds install and rollback when the recovery gate is unavailable", async () => {
  commands.get_importer_evacuation_recovery = () => { throw new Error("Recovery read unavailable"); };
  render();
  expect(await screen.findByText("Recovery read unavailable")).toBeVisible();
  expect(screen.getByRole("button", { name: "Reinstall importer" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "Roll back importer" })).toBeDisabled();
});

it.each([false, true])("writes the current version for pin or null for unpin (pinned: %s)", async (pinned) => {
  update.pinned = pinned;
  render();
  await userEvent.click(await screen.findByRole("button", { name: pinned ? "Unpin" : "Pin to current" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("set_importer_pinned", { game: "gimi", version: pinned ? null : "1.0" }));
  expect(await screen.findByText("Importer Pin saved.")).toBeVisible();
});

it("reports an unrecorded installed version without claiming current", async () => {
  update.installedVersion = null;
  const { client } = render();
  await screen.findByText("GIMI installed · Game readable");
  await waitFor(() => expect(client.isFetching()).toBe(0));
  expect(screen.queryByText("Model Importer is current.")).not.toBeInTheDocument();
  expect(screen.getByText("Installed version: unknown (not recorded).")).toBeVisible();
  expect(screen.queryByRole("button", { name: "Pin to current" })).not.toBeInTheDocument();
});

it("keeps a paused latest-release query unknown rather than declaring no release", () => {
  onlineManager.setOnline(false);
  const view = render();
  try {
    expect(view.client.getQueryState(["importer", "latest", "gimi"])?.fetchStatus).toBe("paused");
    expect(screen.queryByText(/No release available/)).not.toBeInTheDocument();
    expect(screen.getByText(/Latest release: checking/)).toBeVisible();
  } finally {
    view.unmount();
    view.client.clear();
    onlineManager.setOnline(true);
  }
});

it.each(["pending", "failed"])("does not label an unknown filesystem as an install invitation (%s)", async (state) => {
  commands.get_game_path_status = state === "pending" ? () => new Promise(() => {}) : () => { throw new Error("Path check failed"); };
  render();
  if (state === "failed") await screen.findByText("Path check failed");
  expect(screen.queryByRole("button", { name: "Install importer" })).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Importer installation unavailable" })).toBeDisabled();
});

it.each([
  [{ status: "installed", record: { installedVersion: "1.0", origin: { state: "unknown" } } }, "GIMI installed · Game readable", "Reinstall importer"],
  [{ status: "damaged", record: { installedVersion: "1.0", origin: { state: "unknown" } }, paths: ["C:/Game/Core"], error: null }, "Game readable · Model Importer damaged", "Reinstall importer"],
  [{ status: "noImporterEntriesFound", paths: ["C:/Game/d3dx.ini", "C:/Game/Core", "C:/Game/ShaderFixes"] }, "Cannot tell — no GMM install recorded; no Model Importer entries found", "Install importer"],
  [{ status: "unrecordedImporterPresent", paths: [] }, "Cannot tell — Model Importer files found; no GMM install recorded", "Reinstall importer"],
] as const)("keeps local install state %s and its explicit action distinct", async (modelImporter, line, action) => {
  path = { ...path, modelImporter } as GamePathStatus;
  commands.check_importer_update = () => new Promise(() => {});
  render();
  expect(await screen.findByText(line)).toBeVisible();
  expect(screen.getByRole("button", { name: action })).toBeEnabled();
  expect(screen.queryByText(/Model Importer not installed|never installed|GMM installed this/i)).not.toBeInTheDocument();
});

it("allows explicit installation after a local no-receipt no-entries observation", async () => {
  path.modelImporter = { status: "noImporterEntriesFound", paths: ["C:/Game/d3dx.ini", "C:/Game/Core", "C:/Game/ShaderFixes"] };
  render();
  expect(await screen.findByText("Cannot tell — no GMM install recorded; no Model Importer entries found")).toBeVisible();
  expect(screen.getByRole("button", { name: "Install importer" })).toBeEnabled();
});

it.each(["wwmi", "efmi"] as const)("blocks importer writes and explains the expected level for %s", async (game) => {
  const shallow = "C:/FictionalGames/Waves";
  path = { ...path, game, savedPath: shallow, pinnedVersion: "1.0",
    gamePath: { status: "savedPathAtWrongLevel", path: shallow, expectedSuffix: "Binaries/Win64" },
    modelImporter: { status: "unverified", record: null, path: null, error: "Wrong level" } };
  renderWithQuery(<ImporterPanel game={game} displayName="Fictional Game" />);
  expect(await screen.findByText("Saved Game path is at the wrong level · Choose the Game folder ending in Binaries/Win64. · Model Importer unverified")).toBeVisible();
  expect(screen.getByRole("button", { name: "Importer installation unavailable" })).toBeDisabled();
  expect(screen.queryByRole("button", { name: "Pin to current" })).not.toBeInTheDocument();
  expect(screen.queryByText(/Content|Shipping\.exe|Missing Game entries|Required Game files|· Installed/)).not.toBeInTheDocument();
});
