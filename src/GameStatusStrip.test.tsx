import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import type { GamePathStatus, UpdateStatus } from "./api";
import { renderWithQuery } from "./test/harness";
import { GameStatusStrip } from "./GameStatusStrip";
import { ImporterPanel } from "./ImporterPanel";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
const savedPath = "C:/StarRail";
const readable: GamePathStatus = { pinnedVersion: null, game: "srmi", savedPath, gamePath: { status: "readable" }, modelImporter: { status: "installed", record: { installedVersion: "1.0", origin: { state: "unknown" } } } };
// No required entries were found, and no GMM install receipt exists.
const missingImporterEntries = ["d3dx.ini", "Core", "ShaderFixes"].map((entry) => `${savedPath}/${entry}`);
const absent: GamePathStatus = { ...readable, modelImporter: { status: "noImporterEntriesFound", paths: missingImporterEntries } };
const props = { game: "srmi" as const, displayName: "Honkai: Star Rail", savedPath, pathReadError: null, pathReadPending: false, onSettings: vi.fn() };

function expectNotInstalledStrip(strip: HTMLElement) {
  const heading = "Game and Model Importer (SRMI)";
  const line = "Game: Cannot tell — no GMM install recorded; no Model Importer entries found";
  const pathLine = `Saved Game path: ${savedPath}`;
  const detail = `Missing Model Importer entries: ${missingImporterEntries.join(", ")}`;
  // Allow only the specified content and its elements, including the Settings
  // route. Any added badge or icon fails even if it adds no visible text.
  expect(Array.from(strip.querySelectorAll("*"), (element) => [element.tagName, element.textContent])).toEqual([
    ["DIV", heading + line + pathLine + detail],
    ["DIV", heading + line],
    ["STRONG", heading],
    ["SPAN", line],
    ["SPAN", pathLine],
    ["CODE", savedPath],
    ["SPAN", detail],
    ["BUTTON", "Settings for Honkai: Star Rail"],
  ]);
}

let pathStatus: GamePathStatus;
let updateStatus: UpdateStatus;

beforeEach(() => {
  pathStatus = { ...readable };
  updateStatus = { available: false, installedVersion: "1.0", latestVersion: "2.0", pinned: false, upstreamAhead: false, checkError: null };
  invoke.mockReset().mockImplementation(async (command) => {
    if (command === "get_game_path_status") return pathStatus;
    throw new Error(`Unexpected command: ${command}`);
  });
  props.onSettings.mockClear();
});

it("renders uncertainty when no install record or required entries were found", async () => {
  pathStatus = absent;
  // Stale recorded metadata must not upgrade confirmed filesystem absence.
  pathStatus = { ...pathStatus, pinnedVersion: "1.0" };
  renderWithQuery(<GameStatusStrip {...props} />);
  const strip = screen.getByRole("region", { name: "Honkai: Star Rail status" });
  expect(await within(strip).findByText("Game: Cannot tell — no GMM install recorded; no Model Importer entries found")).toBeVisible();
  expect(invoke).not.toHaveBeenCalledWith("check_importer_update", expect.anything());
  expectNotInstalledStrip(strip);
  const settings = within(strip).getByRole("button", { name: "Settings for Honkai: Star Rail" });
  expect(settings).toHaveAttribute("data-settings-arrival", "game");
  await userEvent.click(settings);
  expect(props.onSettings).toHaveBeenCalledTimes(1);
});

it("keeps an existing wrong-type Model Importer entry distinct from absence", async () => {
  const error = "Model Importer entry C:/StarRail/Core must be a directory.";
  pathStatus = { ...readable, modelImporter: { status: "unverified", record: null, path: "C:/StarRail/Core", error } };
  // Recorded install metadata cannot upgrade failed filesystem evidence.
  pathStatus = { ...pathStatus, pinnedVersion: "1.0" };
  renderWithQuery(<GameStatusStrip {...props} />);
  const strip = screen.getByRole("region", { name: "Honkai: Star Rail status" });
  expect(await within(strip).findByText("Game: Game readable · Model Importer unverified")).toBeVisible();
  expect(within(strip).getByText(error)).toBeVisible();
  expect(strip).not.toHaveTextContent(/not installed|Model Importer healthy|Installed version:|Pinned/);
  await userEvent.click(within(strip).getByRole("button", { name: "Settings for Honkai: Star Rail" }));
  expect(props.onSettings).toHaveBeenCalledTimes(1);
});

it.each([
  [{ ...readable, gamePath: { status: "unreadable", path: savedPath, error: "Access denied" } }, "Game: Cannot tell — install path unreadable · Model Importer unverified", "Access denied"],
  [absent, "Game: Cannot tell — no GMM install recorded; no Model Importer entries found", missingImporterEntries.join(", ")],
  [{ ...readable, modelImporter: { status: "unverified", record: null, path: "C:/StarRail/Core", error: "Sharing violation" } }, "Game: Game readable · Model Importer unverified", "Sharing violation"],
  [{ ...readable, gamePath: { status: "invalid", error: "Invalid layout" } }, "Game: Cannot verify Game layout · Model Importer unverified", "Invalid layout"],
])("reports verified path evidence without inferring an installed version: %s", async (value, line, detail) => {
  pathStatus = value as GamePathStatus;
  pathStatus = { ...pathStatus, pinnedVersion: "1.0" };
  renderWithQuery(<GameStatusStrip {...props} />);
  const strip = screen.getByRole("region", { name: "Honkai: Star Rail status" });
  expect(await within(strip).findByText(line as string)).toBeVisible();
  expect(strip).toHaveTextContent(detail as string);
  expect(strip).toHaveTextContent(savedPath);
  expect(strip).toHaveTextContent("Game and Model Importer (SRMI)");
  expect(strip).not.toHaveTextContent("Unhealthy");
  expect(strip).not.toHaveTextContent("Pinned");
  expect(within(strip).getByText(savedPath, { selector: "code" })).toBeVisible();
  if (line !== "Game: Cannot tell — no GMM install recorded; no Model Importer entries found") expect(strip).not.toHaveTextContent(/not installed|no saved/i);
  expect(strip).not.toHaveTextContent(/version|v1\./i);
  const settings = within(strip).getByRole("button", { name: "Settings for Honkai: Star Rail" });
  expect(settings).toHaveAttribute("data-settings-arrival", "game");
  await userEvent.click(settings);
  expect(props.onSettings).toHaveBeenCalledTimes(1);
  expect(invoke.mock.calls.map(([command]) => command)).toEqual(["get_game_path_status"]);
});

it.each([
  { state: "savedPathNotFound" as const, paths: [savedPath], message: "Saved installation cannot be found", other: "Required Game files or directories not found" },
  { state: "requiredEntriesNotFound" as const, paths: [`${savedPath}/StarRail.exe`, `${savedPath}/StarRail_Data`], message: "Required Game files or directories not found", other: "Saved installation cannot be found" },
])("distinguishes missing saved directory from missing Game entries: $state", async ({ state, paths, message, other }) => {
  pathStatus = { ...readable, gamePath: { status: state, paths }, modelImporter: { status: "unverified", record: null, path: null, error: "Game layout unverified" } };
  pathStatus = { ...pathStatus, pinnedVersion: "1.0" };
  renderWithQuery(<GameStatusStrip {...props} />);
  const strip = screen.getByRole("region", { name: "Honkai: Star Rail status" });
  expect(await within(strip).findByText(`Game: ${message} · Model Importer unverified`)).toBeVisible();
  expect(strip).not.toHaveTextContent(other);
  expect(strip).toHaveTextContent("Game and Model Importer (SRMI)");
  expect(strip).toHaveTextContent(`Saved Game path: ${savedPath}`);
  if (state === "requiredEntriesNotFound") {
    for (const missing of paths) expect(strip).not.toHaveTextContent(missing);
    expect(within(strip).getByRole("img", { name: "Warning" })).toBeVisible();
    expect(within(strip).getByRole("img", { name: "Warning" })).toHaveTextContent(/^⚠$/);
    expect(within(strip).getByText("Missing Game entries", { exact: true })).toBeVisible();
  } else {
    // The detail is a separate text node; the saved-path code cannot satisfy this.
    expect(within(strip).getByText(paths.join(", "), { selector: "span", exact: true })).toBeVisible();
    expect(within(strip).getByText(savedPath, { selector: "code", exact: true })).toBeVisible();
    expect(within(strip).queryByRole("img", { name: "Warning" })).not.toBeInTheDocument();
  }
  expect(strip).not.toHaveTextContent(/not installed|healthy|Installed version:|Pinned|Unhealthy/);
  expect(within(strip).getByRole("button", { name: "Settings for Honkai: Star Rail" })).toBeVisible();
});

it("renders the installed version and pin from the local status alone without a network request", async () => {
  pathStatus = { ...pathStatus, pinnedVersion: "1.0" };
  renderWithQuery(<GameStatusStrip {...props} />);
  const strip = screen.getByRole("region", { name: "Honkai: Star Rail status" });
  expect(await within(strip).findByText("Game: Game readable · Model Importer healthy")).toBeVisible();
  expect(strip).toHaveTextContent("Game and Model Importer (SRMI)");
  expect(await within(strip).findByText("Installed version:")).toHaveTextContent("1.0");
  expect(within(strip).getByLabelText("Importer Pin applies")).toHaveTextContent("Pinned");
  expect(strip).not.toHaveTextContent(/2\.0|current|Origin|github|Package/i);
  expect(within(strip).getAllByRole("button").map((button) => button.textContent)).toEqual(["Settings for Honkai: Star Rail"]);
  expect(within(strip).queryByRole("textbox")).not.toBeInTheDocument();
  expect(invoke.mock.calls.map(([command]) => command)).toEqual(["get_game_path_status"]);
  const calls = invoke.mock.calls.length;
  await userEvent.click(within(strip).getByRole("button"));
  expect(props.onSettings).toHaveBeenCalledTimes(1);
  expect(invoke.mock.calls).toHaveLength(calls);
});

it("shows no pin marker without an Importer Pin", async () => {
  renderWithQuery(<GameStatusStrip {...props} />);
  await screen.findByText("Installed version:");
  expect(screen.queryByLabelText("Importer Pin applies")).not.toBeInTheDocument();
  expect(screen.getByRole("status")).not.toHaveTextContent(/Pinned|Unpinned/);
});

it("keeps a null local installed version unknown", async () => {
  pathStatus = { ...readable, modelImporter: { status: "installed", record: { installedVersion: null, origin: { state: "unknown" } } } };
  renderWithQuery(<GameStatusStrip {...props} />);
  expect(await screen.findByText("Installed version unknown (not recorded).")).toBeVisible();
  expect(screen.getByRole("status")).not.toHaveTextContent(/2\.0|current|Installed version:/);
});

it("never renders a damaged importer record as an installed version", async () => {
  pathStatus = { ...readable, pinnedVersion: "1.0", modelImporter: {
    status: "damaged", record: { installedVersion: "9.8.7", origin: { state: "unknown" } },
    paths: ["C:/StarRail/Core"], error: null,
  } };
  renderWithQuery(<GameStatusStrip {...props} />);
  expect(await screen.findByText("Game: Game readable · Model Importer damaged")).toBeVisible();
  expect(screen.getByRole("status")).not.toHaveTextContent(/9\.8\.7|Installed version:|Pinned/);
});

it("never renders a pin marker from unverified state even with a receipt", async () => {
  pathStatus = { ...readable, pinnedVersion: "1.0", modelImporter: {
    status: "unverified", record: { installedVersion: "1.0", origin: { state: "unknown" } },
    path: "C:/StarRail/Core", error: "Wrong entry type",
  } };
  renderWithQuery(<GameStatusStrip {...props} />);
  expect(await screen.findByText("Game: Game readable · Model Importer unverified")).toBeVisible();
  expect(screen.queryByLabelText("Importer Pin applies")).not.toBeInTheDocument();
  expect(screen.getByRole("status")).not.toHaveTextContent(/Pinned|Installed version:|1\.0/);
});

it("keeps an unknown installed version distinct from an independent pin version", async () => {
  pathStatus = { ...readable, pinnedVersion: "0.9", modelImporter: {
    status: "installed", record: { installedVersion: null, origin: { state: "unknown" } },
  } };
  renderWithQuery(<GameStatusStrip {...props} />);
  expect(await screen.findByText("Installed version unknown (not recorded).")).toBeVisible();
  expect(screen.getByLabelText("Importer Pin applies")).toHaveTextContent("Pinned");
  expect(screen.getByRole("status")).not.toHaveTextContent(/0\.9|Installed version:|current/i);
});

it("refreshes the pin alone through the local status query key", async () => {
  const { client } = renderWithQuery(<GameStatusStrip {...props} />);
  await screen.findByText("Installed version:");
  for (const pinnedVersion of ["0.9", null]) {
    pathStatus = { ...readable, pinnedVersion };
    await act(() => client.invalidateQueries({ queryKey: ["gamePathStatus", "srmi"] }));
    expect(screen.getByText("Installed version:")).toHaveTextContent("1.0");
    await waitFor(() => {
      if (pinnedVersion) expect(screen.getByLabelText("Importer Pin applies")).toBeVisible();
      else expect(screen.queryByLabelText("Importer Pin applies")).not.toBeInTheDocument();
    });
  }
  expect(invoke.mock.calls.map(([command]) => command)).toEqual(Array(3).fill("get_game_path_status"));
});

it("withdraws a cached installed fact while the path refresh is checking", async () => {
  pathStatus = { ...pathStatus, pinnedVersion: "1.0" };
  const { client } = renderWithQuery(<GameStatusStrip {...props} />);
  await screen.findByText("Installed version:");
  let resolve!: (value: GamePathStatus) => void;
  invoke.mockImplementation(async (command) => command === "get_game_path_status"
    ? new Promise<GamePathStatus>((yes) => { resolve = yes; }) : updateStatus);
  let refresh!: Promise<void>;
  act(() => { refresh = client.invalidateQueries({ queryKey: ["gamePathStatus", "srmi"] }); });
  expect(await screen.findByText("Game: Checking Game install path · Model Importer unverified")).toBeVisible();
  expect(screen.getByRole("status")).not.toHaveTextContent(/healthy|1\.0|Installed version:|Pinned|Unhealthy/);
  await act(async () => { resolve(readable); await refresh; });
  expect(await screen.findByText("Installed version:")).toHaveTextContent("1.0");
});

it("refreshes path evidence and version and pin through the existing local query key", async () => {
  const { client } = renderWithQuery(<GameStatusStrip {...props} />);
  await screen.findByText("Installed version:");
  pathStatus = absent;
  await act(() => client.invalidateQueries({ queryKey: ["gamePathStatus", "srmi"] }));
  expect(await screen.findByText("Game: Cannot tell — no GMM install recorded; no Model Importer entries found")).toBeVisible();
  expect(screen.getByRole("status")).not.toHaveTextContent("Installed version:");
  pathStatus = { ...readable, pinnedVersion: "1.1", modelImporter: { status: "installed", record: { installedVersion: "1.1", origin: { state: "unknown" } } } };
  await act(() => client.invalidateQueries({ queryKey: ["gamePathStatus", "srmi"] }));
  expect(await screen.findByText("Installed version:")).toHaveTextContent("1.1");
  expect(screen.getByLabelText("Importer Pin applies")).toBeVisible();
});

it("changes the importer identity and metadata with the selected Game", async () => {
  invoke.mockImplementation(async (command, args) => command === "get_game_path_status"
    ? { ...readable, game: args.game, savedPath: args.game === "gimi" ? "C:/Genshin" : savedPath, modelImporter: { status: "installed", record: { installedVersion: args.game === "gimi" ? "8.0" : "1.0", origin: { state: "unknown" } } } }
    : { ...updateStatus, installedVersion: args.game === "gimi" ? "8.0" : "1.0" });
  const { rerender } = renderWithQuery(<GameStatusStrip {...props} />);
  await screen.findByText("Installed version:");
  rerender(<GameStatusStrip {...props} game="gimi" displayName="Genshin Impact" savedPath="C:/Genshin" />);
  const strip = screen.getByRole("region", { name: "Genshin Impact status" });
  expect(await within(strip).findByText("Installed version:")).toHaveTextContent("8.0");
  expect(strip).toHaveTextContent("Model Importer (GIMI)");
  expect(strip).toHaveTextContent("C:/Genshin");
  expect(strip).not.toHaveTextContent(/SRMI|1\.0|StarRail/);
  expect(invoke).toHaveBeenCalledWith("get_game_path_status", { game: "gimi" });
  expect(invoke).not.toHaveBeenCalledWith("check_importer_update", expect.anything());
});

it("refreshes the strip when the Settings importer install settles", async () => {
  pathStatus = absent;
  updateStatus.installedVersion = null;
  invoke.mockImplementation(async (command) => {
    if (command === "get_game_path_status") return pathStatus;
    if (command === "check_importer_update") return updateStatus;
    if (command === "install_importer") {
      pathStatus = { ...readable, modelImporter: { status: "installed", record: { installedVersion: "1.1", origin: { state: "unknown" } } } };
      updateStatus = { ...updateStatus, installedVersion: "1.1" };
      return { sha256: "abc123", backup_dir: null, rewrote_files: [] };
    }
    return null;
  });
  renderWithQuery(<><GameStatusStrip {...props} /><ImporterPanel game="srmi" displayName={props.displayName} /></>);
  const strip = screen.getByRole("region", { name: "Honkai: Star Rail status" });
  expect(await within(strip).findByText("Game: Cannot tell — no GMM install recorded; no Model Importer entries found")).toBeVisible();
  await userEvent.click(await screen.findByRole("button", { name: "Install importer" }));
  expect(await within(strip).findByText("Game: Game readable · Model Importer healthy")).toBeVisible();
  expect(await within(strip).findByText("Installed version:")).toHaveTextContent("1.1");
});

it("reports no saved path without claiming filesystem absence", async () => {
  pathStatus = { ...pathStatus, pinnedVersion: "1.0" };
  pathStatus = { ...readable, pinnedVersion: "1.0", savedPath: null, gamePath: { status: "noSavedPath" },
    modelImporter: { status: "unverified", record: null, path: null, error: "No saved path" } };
  renderWithQuery(<GameStatusStrip {...props} savedPath={null} />);
  expect(await screen.findByText("Game: No saved install path · Model Importer unverified")).toBeVisible();
  expect(screen.getByRole("status")).not.toHaveTextContent(/not installed|healthy|1\.0|current|Pinned|Unhealthy/);
  expect(screen.queryByText(savedPath)).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Settings for Honkai: Star Rail" })).toBeVisible();
});

it("shows checking while the status read is pending and preserves the saved path", async () => {
  pathStatus = { ...pathStatus, pinnedVersion: "1.0" };
  let resolve!: (value: GamePathStatus) => void;
  invoke.mockImplementation(async (command) => command === "get_game_path_status" ? new Promise<GamePathStatus>((yes) => { resolve = yes; }) : updateStatus);
  renderWithQuery(<GameStatusStrip {...props} />);
  expect(screen.getByText("Game: Checking Game install path · Model Importer unverified")).toBeVisible();
  expect(screen.getByRole("region", { name: "Honkai: Star Rail status" })).toHaveTextContent(`Last saved Game path: ${savedPath}`);
  expect(screen.queryByText(/Model Importer not installed/)).not.toBeInTheDocument();
  expect(invoke).not.toHaveBeenCalledWith("check_importer_update", expect.anything());
  expect(screen.getByRole("status")).not.toHaveTextContent(/healthy|Installed version:|1\.0|current|Pinned|Unhealthy/);
  await act(async () => resolve(readable));
  expect(await screen.findByText("Game: Game readable · Model Importer healthy")).toBeVisible();
});

it("does not reuse a cached readable result after the saved-path read fails", async () => {
  pathStatus = { ...pathStatus, pinnedVersion: "1.0" };
  const { rerender } = renderWithQuery(<GameStatusStrip {...props} />);
  await screen.findByText("Game: Game readable · Model Importer healthy");
  rerender(<GameStatusStrip {...props} pathReadError={new Error("Settings database locked")} />);
  expect(screen.getByRole("status")).toHaveTextContent("Game: Cannot tell — install path unreadable · Model Importer unverified");
  expect(screen.getByRole("status")).toHaveTextContent("Settings database locked");
  expect(screen.getByRole("status")).toHaveTextContent(`Last saved Game path: ${savedPath}`);
  expect(screen.queryByText(/Game readable/)).not.toBeInTheDocument();
  expect(screen.getByRole("status")).not.toHaveTextContent(/Pinned|Unhealthy/);
});

it("does not reuse a cached readable result after the filesystem check fails", async () => {
  pathStatus = { ...pathStatus, pinnedVersion: "1.0" };
  const { client } = renderWithQuery(<GameStatusStrip {...props} />);
  await screen.findByText("Game: Game readable · Model Importer healthy");
  invoke.mockImplementation(async (command) => { if (command === "get_game_path_status") throw new Error("Storage unavailable"); return updateStatus; });
  await act(() => client.invalidateQueries({ queryKey: ["gamePathStatus", "srmi"] }));
  await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Storage unavailable"));
  expect(screen.getByRole("status")).toHaveTextContent("Game: Cannot tell — path check unavailable · Model Importer unverified");
  expect(screen.getByRole("status")).not.toHaveTextContent(/Game readable|not installed|version|Pinned|Unhealthy/i);
});


it("keeps checking while the saved-path read is pending despite a completed status check", async () => {
  pathStatus = { ...pathStatus, pinnedVersion: "1.0" };
  const { client } = renderWithQuery(<GameStatusStrip {...props} pathReadPending />);
  await waitFor(() => expect(client.getQueryData(["gamePathStatus", "srmi"])).toEqual(pathStatus));
  await waitFor(() => expect(screen.getByRole("region", { name: "Honkai: Star Rail status" })).toHaveAttribute("aria-busy", "true"));
  expect(screen.getByRole("status")).toHaveTextContent("Game: Checking Game install path · Model Importer unverified");
  expect(screen.getByRole("status")).toHaveTextContent(`Last saved Game path: ${savedPath}`);
  expect(screen.queryByText(/Game readable/)).not.toBeInTheDocument();
  expect(screen.getByRole("status")).not.toHaveTextContent(/Pinned|Unhealthy/);
});

it("keeps a saved-path read failure ahead of a pending read", async () => {
  renderWithQuery(<GameStatusStrip {...props} pathReadPending pathReadError={new Error("Settings database locked")} />);
  expect(screen.getByRole("status")).toHaveTextContent("Game: Cannot tell — install path unreadable · Model Importer unverified");
  expect(screen.getByRole("status")).toHaveTextContent("Settings database locked");
  expect(screen.getByRole("status")).toHaveTextContent(`Last saved Game path: ${savedPath}`);
  expect(screen.queryByText(/Checking Game install path/)).not.toBeInTheDocument();
});

it("keeps a rejected status check ahead of a pending saved-path read", async () => {
  invoke.mockImplementation(async (command) => { if (command === "get_game_path_status") throw new Error("Path check channel closed"); return updateStatus; });
  const { rerender } = renderWithQuery(<GameStatusStrip {...props} />);
  await screen.findByText("Path check channel closed");
  rerender(<GameStatusStrip {...props} pathReadPending />);
  expect(screen.getByRole("status")).toHaveTextContent("Game: Cannot tell — path check unavailable · Model Importer unverified");
  expect(screen.getByRole("status")).toHaveTextContent("Path check channel closed");
  expect(screen.getByRole("status")).toHaveTextContent(`Last saved Game path: ${savedPath}`);
  expect(screen.queryByText(/Checking Game install path/)).not.toBeInTheDocument();
});

it.each([
  { pathReadPending: true },
  { pathReadError: new Error("Settings database locked") },
])("withdraws cached absence and pin when the saved path is unverified: %s", async (pathProps) => {
  pathStatus = absent;
  pathStatus = { ...pathStatus, pinnedVersion: "1.0" };
  const { rerender } = renderWithQuery(<GameStatusStrip {...props} />);
  await screen.findByText("Game: Cannot tell — no GMM install recorded; no Model Importer entries found");
  expectNotInstalledStrip(screen.getByRole("region", { name: "Honkai: Star Rail status" }));
  rerender(<GameStatusStrip {...props} {...pathProps} />);
  expect(screen.getByRole("status")).toHaveTextContent("Model Importer unverified");
  expect(screen.getByRole("status")).not.toHaveTextContent(/Unhealthy|not installed|Pinned|Installed version:/);
});

it("withdraws cached absence while the status refresh is checking", async () => {
  pathStatus = absent;
  pathStatus = { ...pathStatus, pinnedVersion: "1.0" };
  const { client } = renderWithQuery(<GameStatusStrip {...props} />);
  await screen.findByText("Game: Cannot tell — no GMM install recorded; no Model Importer entries found");
  let resolve!: (value: GamePathStatus) => void;
  invoke.mockImplementation(async (command) => command === "get_game_path_status"
    ? new Promise<GamePathStatus>((yes) => { resolve = yes; }) : updateStatus);
  let refresh!: Promise<void>;
  act(() => { refresh = client.invalidateQueries({ queryKey: ["gamePathStatus", "srmi"] }); });
  expect(await screen.findByText("Game: Checking Game install path · Model Importer unverified")).toBeVisible();
  expect(screen.getByRole("status")).not.toHaveTextContent(/Unhealthy|not installed|Pinned|Installed version:/);
  await act(async () => { resolve(pathStatus); await refresh; });
  expect(await screen.findByText("Game: Cannot tell — no GMM install recorded; no Model Importer entries found")).toBeVisible();
  expectNotInstalledStrip(screen.getByRole("region", { name: "Honkai: Star Rail status" }));
});

it("keeps a failed status check ahead of a held recheck without reusing cached absence", async () => {
  pathStatus = absent;
  pathStatus = { ...pathStatus, pinnedVersion: "1.0" };
  const { client } = renderWithQuery(<GameStatusStrip {...props} />);
  await screen.findByText("Game: Cannot tell — no GMM install recorded; no Model Importer entries found");
  invoke.mockImplementation(async (command) => {
    if (command === "get_game_path_status") throw new Error("Storage unavailable");
    return updateStatus;
  });
  await act(() => client.invalidateQueries({ queryKey: ["gamePathStatus", "srmi"] }));
  expect(await screen.findByText("Storage unavailable")).toBeVisible();
  expect(screen.getByRole("status")).not.toHaveTextContent(/Unhealthy|not installed|Pinned|Installed version:/);
  let resolve!: (value: GamePathStatus) => void;
  invoke.mockImplementation(async (command) => command === "get_game_path_status"
    ? new Promise<GamePathStatus>((yes) => { resolve = yes; }) : updateStatus);
  let refresh!: Promise<void>;
  act(() => { refresh = client.invalidateQueries({ queryKey: ["gamePathStatus", "srmi"] }); });
  await waitFor(() => expect(screen.getByRole("region", { name: "Honkai: Star Rail status" })).toHaveAttribute("aria-busy", "true"));
  expect(screen.getByRole("status")).toHaveTextContent("Game: Cannot tell — path check unavailable · Model Importer unverified");
  expect(screen.getByRole("status")).toHaveTextContent("Storage unavailable");
  expect(screen.getByRole("status")).not.toHaveTextContent(/Checking Game install path|Unhealthy|not installed|Pinned|Installed version:/);
  await act(async () => { resolve(readable); await refresh; });
  expect(await screen.findByText("Game: Game readable · Model Importer healthy")).toBeVisible();
});

it.each([
  [{ status: "installed", record: { installedVersion: "v1", origin: { state: "unknown" } } }, "Game readable · Model Importer healthy"],
  [{ status: "damaged", record: { installedVersion: "v1", origin: { state: "unknown" } }, paths: ["C:/StarRail/Core"], error: null }, "Game readable · Model Importer damaged"],
  [{ status: "noImporterEntriesFound", paths: missingImporterEntries }, "Cannot tell — no GMM install recorded; no Model Importer entries found"],
  [{ status: "unrecordedImporterPresent", paths: [] }, "Cannot tell — Model Importer files found; no GMM install recorded"],
] as const)("renders local install state %s without upstream metadata", async (modelImporter, line) => {
  pathStatus = { ...readable, modelImporter } as GamePathStatus;
  invoke.mockImplementation((command) => command === "get_game_path_status" ? Promise.resolve(pathStatus) : new Promise(() => {}));
  renderWithQuery(<GameStatusStrip {...props} />);
  expect(await screen.findByText(`Game: ${line}`)).toBeVisible();
  expect(screen.getByRole("status")).not.toHaveTextContent(/not installed|never installed|GMM installed this/i);
});
