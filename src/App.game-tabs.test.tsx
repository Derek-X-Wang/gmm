import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import type { GameCode, GameSummary } from "./api";
import { renderWithQuery } from "./test/harness";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn() }));
vi.mock("./diagnostics", () => ({ diagnosticsLogDir: vi.fn().mockResolvedValue("C:\\GMM\\logs"), exportDiagnosticsBundle: vi.fn() }));
vi.mock("./updater", () => ({ checkInteractively: vi.fn() }));
vi.mock("./LoaderVersionNote", () => ({ LoaderVersionNote: () => null }));

const { default: App } = await import("./App");
const games: GameSummary[] = [
  { code: "gimi", displayName: "Genshin Impact" },
  { code: "srmi", displayName: "Honkai: Star Rail" },
  { code: "zzmi", displayName: "Zenless Zone Zero" },
  { code: "wwmi", displayName: "Wuthering Waves" },
  { code: "himi", displayName: "Honkai Impact 3rd" },
  { code: "efmi", displayName: "Arknights: Endfield" },
];
let paths: Partial<Record<GameCode, string>>;

beforeEach(() => {
  paths = {};
  invoke.mockImplementation(async (command: string, args?: { game: GameCode; path: string }) => {
    switch (command) {
      case "is_onboarding_complete": return { complete: true, skipped: true };
      case "list_supported_games": return games;
      case "get_game_install_path": return paths[args!.game] ?? null;
      case "set_game_install_path": paths[args!.game] = args!.path; return null;
      case "attention_status": return {
        safeToProceed: true, reinstalls: [], enabledTransitions: [], importerEvacuations: [],
        stagedLibraryOperations: [], sessionLaunches: [], activeSession: null,
        libraryAudits: [], libraryRootOverlaps: [], modPathOverlaps: [],
      };
      case "get_startup_reconcile_status": return { finished: true, failures: [] };
      case "check_importer_update": return { available: false, installedVersion: null, pinned: false };
      case "get_proxy_config": return { url: null, username: null, passwordSet: false };
      case "get_library_paths": return {
        defaultRoot: "C:\\GMM\\library", effectiveRoot: "C:\\GMM\\library",
        rootOverride: null, perGameOverrides: {}, perGameEffective: {},
      };
      case "detect_conflicts": return { conflicts: [], per_mod_count: {} };
      case "mod_updates_globally_enabled": return true;
      case "list_mods":
      case "list_mod_updates":
      case "interrupted_session_launches": return [];
      default: return null;
    }
  });
});

function tabNames() {
  return screen.queryAllByRole("tab").map((tab) => tab.textContent);
}

it("shows only the three installed games as tabs and opens a remaining game's setup deliberately", async () => {
  paths = { srmi: "C:\\StarRail", zzmi: "C:\\ZZZ", wwmi: "C:\\WutheringWaves" };
  renderWithQuery(<App />);
  await screen.findByRole("tab", { name: "Honkai: Star Rail" });
  await waitFor(() => expect(tabNames()).toEqual([
    "Honkai: Star Rail", "Zenless Zone Zero", "Wuthering Waves",
  ]));
  expect(screen.getByRole("tab", { name: "Honkai: Star Rail" })).toHaveAttribute("aria-selected", "true");
  expect(screen.queryByRole("button", { name: "Set up Genshin Impact" })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Add game" }));
  const picker = screen.getByRole("region", { name: "Add a game" });
  expect(within(picker).getAllByRole("button").map((button) => button.textContent)).toEqual([
    "Set up Genshin Impact", "Set up Honkai Impact 3rd", "Set up Arknights: Endfield",
  ]);
  await userEvent.click(within(picker).getByRole("button", { name: "Set up Genshin Impact" }));
  expect(screen.getByRole("textbox", { name: "Genshin Impact install path" })).toHaveValue("");
  expect(screen.queryByRole("tab", { name: "Genshin Impact" })).not.toBeInTheDocument();
  await userEvent.type(screen.getByRole("textbox", { name: "Genshin Impact install path" }), "C:\\Genshin{Enter}");
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  await waitFor(() => expect(tabNames()).toEqual([
    "Genshin Impact", "Honkai: Star Rail", "Zenless Zone Zero", "Wuthering Waves",
  ]));
  expect(screen.getByRole("tab", { name: "Honkai: Star Rail" })).toHaveAttribute("aria-selected", "true");
  expect(invoke).toHaveBeenCalledWith("set_game_install_path", { game: "gimi", path: "C:\\Genshin" });
});

it("keeps every supported game reachable with no install paths and promotes the first saved path to a tab", async () => {
  renderWithQuery(<App />);
  await screen.findByRole("button", { name: "Add game" });
  expect(tabNames()).toEqual([]);
  expect(screen.getByText("No game install paths set. Add a game to get started.")).toBeInTheDocument();
  expect(screen.queryByRole("heading", { level: 1 })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: /^Launch / })).not.toBeInTheDocument();
  expect(screen.queryByRole("heading", { name: /^Mods \(/ })).not.toBeInTheDocument();
  for (const game of games) {
    await userEvent.click(screen.getByRole("button", { name: "Add game" }));
    await userEvent.click(screen.getByRole("button", { name: `Set up ${game.displayName}` }));
    expect(await screen.findByRole("textbox", { name: `${game.displayName} install path` })).toHaveValue("");
    if (game.code !== "efmi") await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  }
  await userEvent.type(screen.getByRole("textbox", { name: "Arknights: Endfield install path" }), "C:\\Endfield{Enter}");
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  await waitFor(() => expect(tabNames()).toEqual(["Arknights: Endfield"]));
  expect(screen.getByRole("tab", { name: "Arknights: Endfield" })).toHaveAttribute("aria-selected", "true");
});

it("shows all six installed games and omits the add control when none remain", async () => {
  paths = Object.fromEntries(games.map((game) => [game.code, `C:\\Games\\${game.code}`]));
  renderWithQuery(<App />);
  await screen.findByRole("tab", { name: "Arknights: Endfield" });
  expect(tabNames()).toEqual(games.map((game) => game.displayName));
  expect(screen.queryByRole("button", { name: "Add game" })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole("tab", { name: "Zenless Zone Zero" }));
  await userEvent.click(screen.getByRole("button", { name: "Settings for Zenless Zone Zero" }));
  expect(screen.getByRole("textbox", { name: "Zenless Zone Zero install path" })).toHaveValue("C:\\Games\\zzmi");
});

it("removes a tab when its saved path is cleared and keeps setup reachable", async () => {
  paths = { srmi: "C:\\StarRail" };
  renderWithQuery(<App />);
  await screen.findByRole("tab", { name: "Honkai: Star Rail" });
  await userEvent.click(screen.getByRole("button", { name: "Settings for Honkai: Star Rail" }));
  const path = screen.getByRole("textbox", { name: "Honkai: Star Rail install path" });
  await userEvent.clear(path);
  await userEvent.click(screen.getByRole("button", { name: "Apply Honkai: Star Rail install path" }));
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  await waitFor(() => expect(tabNames()).toEqual([]));
  await userEvent.click(screen.getByRole("button", { name: "Add game" }));
  expect(screen.getByRole("button", { name: "Set up Honkai: Star Rail" })).toBeInTheDocument();
});

it("adds a tab after auto-detect saves a previously unconfigured game's path", async () => {
  const implementation = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (command, args) => {
    if (command === "detect_game_install_path") {
      paths[args.game as GameCode] = "C:\\DetectedEndfield";
      return "C:\\DetectedEndfield";
    }
    return implementation(command, args);
  });
  renderWithQuery(<App />);
  await userEvent.click(await screen.findByRole("button", { name: "Add game" }));
  await userEvent.click(screen.getByRole("button", { name: "Set up Arknights: Endfield" }));
  await userEvent.click(screen.getByRole("button", { name: "Auto-detect" }));
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  await waitFor(() => expect(tabNames()).toEqual(["Arknights: Endfield"]));
  await userEvent.click(screen.getByRole("button", { name: "Settings for Arknights: Endfield" }));
  expect(screen.getByRole("textbox", { name: "Arknights: Endfield install path" })).toHaveValue("C:\\DetectedEndfield");
});

it("shows unavailable path reads explicitly and leaves the affected game's setup reachable", async () => {
  const implementation = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (command, args) => {
    if (command === "get_game_install_path" && args.game === "himi") throw new Error("Path read failed");
    return implementation(command, args);
  });
  renderWithQuery(<App />);
  const error = await screen.findByText("GMM could not read some game install paths.");
  expect(error.closest('[role="alert"]')).toHaveTextContent("Honkai Impact 3rd: Path read failed");
  expect(screen.queryByText("No game install paths set. Add a game to get started.")).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Add game" }));
  await userEvent.click(screen.getByRole("button", { name: "Set up Honkai Impact 3rd" }));
  expect(screen.getByRole("textbox", { name: "Honkai Impact 3rd install path" })).toBeInTheDocument();
});

it("does not call pending path reads an empty install list", async () => {
  const implementation = invoke.getMockImplementation()!;
  let resolvePath!: (path: string) => void;
  const pendingPath = new Promise<string>((resolve) => { resolvePath = resolve; });
  invoke.mockImplementation(async (command, args) => {
    if (command === "get_game_install_path" && args.game === "efmi") return pendingPath;
    return implementation(command, args);
  });
  renderWithQuery(<App />);
  try {
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("get_game_install_path", { game: "efmi" }));
    await screen.findByText("Checking game install paths…");
    expect(screen.queryByRole("heading", { level: 1 })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Launch / })).not.toBeInTheDocument();
    expect(screen.queryByText("No game install paths set. Add a game to get started.")).not.toBeInTheDocument();
  } finally {
    resolvePath("C:\\Endfield");
  }
  await waitFor(() => expect(tabNames()).toEqual(["Arknights: Endfield"]));
  expect(screen.queryByText("Checking game install paths…")).not.toBeInTheDocument();
});


it("chooses the first installed Game after all concurrent path reads finish", async () => {
  paths = { gimi: "C:\\Genshin", srmi: "C:\\StarRail" };
  const implementation = invoke.getMockImplementation()!;
  let resolveFirst!: (path: string) => void;
  invoke.mockImplementation(async (command, args) => {
    if (command === "get_game_install_path" && args.game === "gimi") return new Promise<string>((resolve) => { resolveFirst = resolve; });
    return implementation(command, args);
  });
  renderWithQuery(<App />);
  await screen.findByRole("tab", { name: "Honkai: Star Rail" });
  await screen.findByText("Checking game install paths…");
  await act(async () => resolveFirst(paths.gimi!));
  await waitFor(() => expect(screen.getByRole("tab", { name: "Genshin Impact" })).toHaveAttribute("aria-selected", "true"));
  expect(screen.getByRole("tab", { name: "Honkai: Star Rail" })).toHaveAttribute("aria-selected", "false");
});


it("retains last known tab membership and its repair route after a saved-path read rejects", async () => {
  paths = { srmi: "C:/StarRail" };
  const { client } = renderWithQuery(<App />);
  await screen.findByRole("tab", { name: "Honkai: Star Rail" });
  const implementation = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (command, args) => {
    if (command === "get_game_install_path" && args.game === "srmi") throw new Error("Saved path read denied");
    return implementation(command, args);
  });
  await act(() => client.invalidateQueries({ queryKey: ["installPath", "srmi"] }));
  const failure = await screen.findByText("Honkai: Star Rail: Saved path read denied");
  expect(failure.closest('[role="alert"]')).toBeVisible();
  expect(tabNames()).toEqual(["Honkai: Star Rail"]);
  expect(screen.getByRole("tab", { name: "Honkai: Star Rail" })).toHaveAttribute("aria-selected", "true");
  const strip = screen.getByRole("region", { name: "Honkai: Star Rail status" });
  expect(strip).toHaveTextContent("Cannot tell — install path unreadable · Model Importer unverified");
  expect(strip).toHaveTextContent("C:/StarRail");
  expect(strip).not.toHaveTextContent(/not installed|no saved|Absent/i);
  expect(screen.queryByText(/No game install paths set/)).not.toBeInTheDocument();
  await userEvent.click(within(strip).getByRole("button", { name: "Settings for Honkai: Star Rail" }));
  expect(screen.getByRole("textbox", { name: "Honkai: Star Rail install path" })).toBeVisible();
});

it.each(["unreadable", "savedPathNotFound", "importerAbsent"])("keeps the configured tab and repair route when filesystem state is %s", async (state) => {
  paths = { srmi: "C:/StarRail" };
  const implementation = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (command, args) => {
    if (command === "get_game_path_status") return {
      game: args.game, savedPath: paths[args.game as GameCode] ?? null,
      gamePath: state === "unreadable" ? { status: "unreadable", path: "C:/StarRail", error: "Access denied" }
        : state === "savedPathNotFound" ? { status: "savedPathNotFound", paths: ["C:/StarRail/StarRail.exe"] } : { status: "readable" },
      modelImporter: state === "importerAbsent" ? { status: "requiredEntriesNotFound", paths: ["C:/StarRail/d3dx.ini"] }
        : { status: "unverified", path: null, error: "Cannot inspect files" },
    };
    return implementation(command, args);
  });
  renderWithQuery(<App />);
  const line = state === "unreadable" ? "Cannot tell — install path unreadable · Model Importer unverified"
    : state === "savedPathNotFound" ? "Saved installation cannot be found · Model Importer unverified" : "Game readable · Model Importer not installed";
  const strip = await screen.findByRole("region", { name: "Honkai: Star Rail status" });
  expect(await within(strip).findByText(line)).toBeVisible();
  expect(tabNames()).toEqual(["Honkai: Star Rail"]);
  expect(strip).toHaveTextContent("C:/StarRail");
  expect(strip).not.toHaveTextContent(/version|no saved/i);
  await userEvent.click(within(strip).getByRole("button", { name: "Settings for Honkai: Star Rail" }));
  expect(screen.getByRole("textbox", { name: "Honkai: Star Rail install path" })).toHaveValue("C:/StarRail");
});
