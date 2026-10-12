import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import { getGameInstallPath, type GameCode, type GameSummary } from "./api";
import { useQuery } from "@tanstack/react-query";
import { makeQueryClient, renderWithQuery } from "./test/harness";

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
      case "get_game_path_status": return {
        game: args!.game, savedPath: paths[args!.game] ?? null,
        gamePath: { status: paths[args!.game] ? "readable" : "noSavedPath" },
        modelImporter: paths[args!.game]
          ? { status: "requiredEntriesNotFound", paths: ["d3dx.ini"] }
          : { status: "unverified", path: null, error: "No saved path" },
      };
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
  renderCachedApp();
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
  expect(screen.queryByRole("heading", { name: "Settings", level: 1 })).not.toBeInTheDocument();
  expect(screen.getByRole("tab", { name: "Honkai: Star Rail" })).toHaveAttribute("aria-selected", "true");
  await userEvent.click(within(picker).getByRole("button", { name: "Set up Genshin Impact" }));
  expect(screen.getByRole("textbox", { name: "Genshin Impact install path" })).toHaveValue("");
  expect(screen.queryByRole("tab", { name: "Genshin Impact" })).not.toBeInTheDocument();
  expect(screen.getByText("Return to Honkai: Star Rail")).toBeVisible();
  expect(invoke.mock.calls.filter(([command]) => command.startsWith("set_") || command.startsWith("install_"))).toEqual([]);
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  expect(tabNames()).toEqual(["Honkai: Star Rail", "Zenless Zone Zero", "Wuthering Waves"]);
  expect(screen.getByRole("tab", { name: "Honkai: Star Rail" })).toHaveAttribute("aria-selected", "true");
  expect(screen.getByRole("button", { name: "Add game" })).toHaveFocus();
  await userEvent.click(screen.getByRole("button", { name: "Add game" }));
  await userEvent.click(screen.getByRole("button", { name: "Set up Genshin Impact" }));
  await userEvent.type(screen.getByRole("textbox", { name: "Genshin Impact install path" }), "C:\\Genshin{Enter}");
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  await waitFor(() => expect(tabNames()).toEqual([
    "Genshin Impact", "Honkai: Star Rail", "Zenless Zone Zero", "Wuthering Waves",
  ]));
  expect(screen.getByRole("tab", { name: "Honkai: Star Rail" })).toHaveAttribute("aria-selected", "true");
  expect(screen.getByRole("tab", { name: "Genshin Impact" })).toHaveAttribute("aria-selected", "false");
  expect(screen.getByRole("button", { name: "Add game" })).toHaveFocus();
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


it("keeps Add game available during a background saved-path refetch", async () => {
  paths = { srmi: "C:/StarRail" };
  const { client } = renderWithQuery(<App />);
  await screen.findByRole("button", { name: "Add game" });
  const implementation = invoke.getMockImplementation()!;
  let resolvePath!: (path: string) => void;
  invoke.mockImplementation(async (command, args) => {
    if (command === "get_game_install_path" && args.game === "srmi") return new Promise<string>((resolve) => { resolvePath = resolve; });
    return implementation(command, args);
  });
  let refetch!: Promise<void>;
  await act(async () => { refetch = client.invalidateQueries({ queryKey: ["installPath", "srmi"] }); });
  try {
    await waitFor(() => expect(client.isFetching({ queryKey: ["installPath", "srmi"] })).toBe(1));
    expect(screen.getByRole("button", { name: "Add game" })).toBeVisible();
    expect(screen.queryByText("Checking game install paths…")).not.toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Honkai: Star Rail" })).toHaveAttribute("aria-selected", "true");
    await userEvent.click(screen.getByRole("button", { name: "Add game" }));
    expect(screen.getByRole("button", { name: "Set up Genshin Impact" })).toBeVisible();
  } finally {
    await act(async () => { resolvePath("C:/StarRail"); await refetch; });
  }
});

it("moves to the remaining installed Game when Settings clears the selected Game's path", async () => {
  paths = { gimi: "C:/Genshin", srmi: "C:/StarRail" };
  renderWithQuery(<App />);
  await userEvent.click(await screen.findByRole("tab", { name: "Honkai: Star Rail" }));
  expect(screen.getByRole("tab", { name: "Honkai: Star Rail" })).toHaveAttribute("aria-selected", "true");
  await userEvent.click(screen.getByRole("button", { name: "Settings for Honkai: Star Rail" }));
  await userEvent.clear(screen.getByRole("textbox", { name: "Honkai: Star Rail install path" }));
  await userEvent.click(screen.getByRole("button", { name: "Apply Honkai: Star Rail install path" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("set_game_install_path", { game: "srmi", path: "" }));
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  await waitFor(() => expect(tabNames()).toEqual(["Genshin Impact"]));
  expect(screen.getByRole("tab", { name: "Genshin Impact" })).toHaveAttribute("aria-selected", "true");
  expect(screen.getByRole("region", { name: "Genshin Impact header" })).toBeVisible();
  expect(screen.getByRole("region", { name: "Genshin Impact status" })).toBeVisible();
  expect(await screen.findByRole("heading", { name: "Add Mods for Genshin Impact" })).toBeVisible();
});

it("does not invent a return Game when Settings opens with no configured Games", async () => {
  renderWithQuery(<App />);
  await screen.findByRole("button", { name: "Add game" });
  await userEvent.click(screen.getByRole("button", { name: "Settings" }));
  expect(screen.getByRole("heading", { name: "Settings", level: 1 })).toBeVisible();
  expect(screen.getByRole("button", { name: "Back to Mods" })).toBeVisible();
  expect(screen.queryByText(/^Return to /)).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  expect(tabNames()).toEqual([]);
  expect(screen.getByText("No game install paths set. Add a game to get started.")).toBeVisible();
});

it("agrees with the Settings row that a rejected path check is unavailable", async () => {
  paths = { srmi: "C:/StarRail" };
  const implementation = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (command, args) => {
    if (command === "get_game_path_status") throw new Error("Path check channel closed");
    return implementation(command, args);
  });
  renderWithQuery(<App />);
  const strip = await screen.findByRole("region", { name: "Honkai: Star Rail status" });
  expect(await within(strip).findByText("Cannot tell — path check unavailable · Model Importer unverified")).toBeVisible();
  expect(strip).toHaveTextContent("Path check channel closed");
  expect(strip).toHaveTextContent("C:/StarRail");
  expect(strip).not.toHaveTextContent(/unreadable|not installed|version/i);
  await userEvent.click(within(strip).getByRole("button", { name: "Settings for Honkai: Star Rail" }));
  const row = within(screen.getByRole("navigation", { name: "Settings pages" })).getByRole("button", { name: /Honkai: Star Rail/ });
  await waitFor(() => expect(row).toHaveTextContent("Cannot tell · path check unavailable"));
  expect(row).not.toHaveTextContent(/unreadable|not installed|version/i);
  expect(screen.getByRole("textbox", { name: "Honkai: Star Rail install path" })).toHaveValue("C:/StarRail");
});

// Keep reads fresh across destination changes so a mount refetch cannot mask a
// missing mutation invalidation. The aggregate uses the wizard's production key.
function renderCachedApp() {
  const client = makeQueryClient();
  client.setDefaultOptions({ queries: { retry: false, gcTime: 300_000, staleTime: Infinity } });
  return renderWithQuery(<App />, { client });
}

function OnboardingPathRead() {
  const paths = useQuery({
    queryKey: ["onboarding", "installPaths", games.map((game) => game.code).join(",")],
    queryFn: async () => Object.fromEntries(await Promise.all(games.map(async (game) =>
      [game.code, { path: await getGameInstallPath(game.code), error: null }]))),
  });
  return <output aria-label="Onboarding Genshin path">{paths.data?.gimi.path ?? "No saved path"}</output>;
}

it.each([
  { command: "set_game_install_path", rejected: false },
  { command: "set_game_install_path", rejected: true },
  { command: "detect_game_install_path", rejected: false },
  { command: "detect_game_install_path", rejected: true },
])("adds no tab from an attempted $command and settles all path reads (rejected: $rejected)", async ({ command, rejected }) => {
  paths = { srmi: "C:/StarRail", wwmi: "C:/Waves" };
  const original = invoke.getMockImplementation()!;
  let settle!: () => void;
  const pending = new Promise<void>((resolve) => { settle = resolve; });
  invoke.mockImplementation(async (name, args) => {
    if (name === command) {
      await pending;
      if (rejected) throw new Error("Install path rejected");
      paths.gimi = "C:/Genshin";
      return command === "detect_game_install_path" ? paths.gimi : null;
    }
    return original(name, args);
  });
  const client = makeQueryClient();
  client.setDefaultOptions({ queries: { retry: false, gcTime: 300_000, staleTime: Infinity } });
  renderWithQuery(<><App /><OnboardingPathRead /></>, { client });
  await userEvent.click(await screen.findByRole("tab", { name: "Wuthering Waves" }));
  await waitFor(() => expect(screen.getByLabelText("Onboarding Genshin path")).toHaveTextContent("No saved path"));
  await userEvent.click(screen.getByRole("button", { name: "Add game" }));
  await userEvent.click(screen.getByRole("button", { name: "Set up Genshin Impact" }));
  const navigation = screen.getByRole("navigation", { name: "Settings pages" });
  const gameRow = within(navigation).getByRole("button", { name: /Genshin Impact/ });
  await waitFor(() => expect(gameRow).toHaveTextContent("Absent · no saved path"));
  const calls = (name: string, game: GameCode) => invoke.mock.calls.filter(([command, args]) => command === name && args?.game === game).length;
  const before = {
    path: calls("get_game_install_path", "gimi"),
    aggregate: calls("get_game_install_path", "srmi"),
    status: calls("get_game_path_status", "gimi"),
  };
  if (command === "set_game_install_path") await userEvent.type(screen.getByRole("textbox", { name: "Genshin Impact install path" }), "C:/Genshin{Enter}");
  else await userEvent.click(screen.getByRole("button", { name: "Auto-detect" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith(command, command === "set_game_install_path"
    ? { game: "gimi", path: "C:/Genshin" } : { game: "gimi" }));
  try {
    expect(screen.getByRole("textbox", { name: "Genshin Impact install path" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
    expect(tabNames()).toEqual(["Honkai: Star Rail", "Wuthering Waves"]);
    expect(screen.getByRole("tab", { name: "Wuthering Waves" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("button", { name: "Add game" })).toHaveFocus();
    expect(screen.getByLabelText("Onboarding Genshin path")).toHaveTextContent("No saved path");
    await userEvent.click(screen.getByRole("button", { name: "Add game" }));
    await userEvent.click(screen.getByRole("button", { name: "Set up Genshin Impact" }));
    expect(screen.getByText("Return to Wuthering Waves")).toBeVisible();
  } finally {
    await act(async () => settle());
  }
  // These are backend read calls, including another Game read by the aggregate.
  // They must occur while Settings is still open, before any remount can help.
  await waitFor(() => {
    expect(calls("get_game_install_path", "gimi")).toBeGreaterThanOrEqual(before.path + 2);
    expect(calls("get_game_install_path", "srmi")).toBeGreaterThan(before.aggregate);
    expect(calls("get_game_path_status", "gimi")).toBeGreaterThan(before.status);
  });
  if (rejected) {
    expect(await screen.findByText("Install path rejected")).toBeVisible();
    expect(gameRow).toHaveTextContent("Absent · no saved path");
    expect(screen.getByLabelText("Onboarding Genshin path")).toHaveTextContent("No saved path");
  } else {
    await waitFor(() => expect(gameRow).toHaveTextContent("Game readable · Model Importer not installed"));
    expect(screen.getByRole("textbox", { name: "Genshin Impact install path" })).toHaveValue("C:/Genshin");
    expect(screen.getByLabelText("Onboarding Genshin path")).toHaveTextContent("C:/Genshin");
  }
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  expect(tabNames()).toEqual(rejected ? ["Honkai: Star Rail", "Wuthering Waves"] : ["Genshin Impact", "Honkai: Star Rail", "Wuthering Waves"]);
  expect(screen.getByRole("tab", { name: "Wuthering Waves" })).toHaveAttribute("aria-selected", "true");
  expect(screen.getByRole("button", { name: "Add game" })).toHaveFocus();
  expect(screen.getByRole("button", { name: "Resume setup" })).toBeVisible();
  expect(invoke.mock.calls.filter(([name]) => /^(install_|mark_onboarding_complete$|reset_onboarding$)/.test(name))).toEqual([]);
  await userEvent.click(screen.getByRole("button", { name: "Resume setup" }));
  expect(screen.getByRole("heading", { name: "GMM — first-run setup" })).toBeVisible();
});

it.each(["global", "game", "add"])("restores the selected Game, Mod anchor and %s arrival focus on Back and Escape", async (arrival) => {
  paths = { srmi: "C:/StarRail", wwmi: "C:/Waves" };
  const original = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (name, args) => name === "list_mods" && args.game === "wwmi" ? [{
    id: "add-return", game: "wwmi", name: "Return Outfit", source: "manual",
    library_path: "C:/GMM/library/wwmi/add-return", enabled: false,
    reinstall_recovery: null, enabled_transition_recovery: null,
  }] : name === "list_variants" ? { variants: [], activeVariantId: null } : original(name, args));
  let rowTop = -40;
  const bounds = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
    return { top: this.dataset.modId ? rowTop : 0, bottom: this.dataset.modId ? rowTop + 180 : 0 } as DOMRect;
  });
  const initialScroll = window.scrollY;
  try {
    renderCachedApp();
    await userEvent.click(await screen.findByRole("tab", { name: "Wuthering Waves" }));
    await screen.findByText("Return Outfit");
    for (const route of ["button", "escape"]) {
      rowTop = -40;
      Object.defineProperty(window, "scrollY", { configurable: true, value: 860 });
      const name = arrival === "global" ? "Settings" : arrival === "game" ? "Settings for Wuthering Waves" : "Add game";
      await userEvent.click(screen.getByRole("button", { name }));
      if (arrival === "add") await userEvent.click(screen.getByRole("button", { name: "Set up Genshin Impact" }));
      expect(screen.getByRole("heading", { name: "Settings", level: 1 })).toHaveFocus();
      expect(screen.getByText("Return to Wuthering Waves")).toBeVisible();
      await userEvent.click(within(screen.getByRole("navigation", { name: "Settings pages" })).getByRole("button", { name: /Arknights: Endfield/ }));
      Object.defineProperty(window, "scrollY", { configurable: true, value: 0 });
      rowTop = 20;
      if (route === "button") await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
      else fireEvent.keyDown(window, { key: "Escape" });
      expect(screen.getByText("Return Outfit")).toBeVisible();
      expect(window.scrollTo).toHaveBeenLastCalledWith(0, 60);
      expect(screen.getByRole("tab", { name: "Wuthering Waves" })).toHaveAttribute("aria-selected", "true");
      expect(screen.getByRole("button", { name })).toHaveFocus();
      expect(tabNames()).toEqual(["Honkai: Star Rail", "Wuthering Waves"]);
    }
  } finally {
    bounds.mockRestore();
    Object.defineProperty(window, "scrollY", { configurable: true, value: initialScroll });
  }
});

it("returns focus to the selected tab when saving the last absent Game removes Add game", async () => {
  paths = Object.fromEntries(games.filter((game) => game.code !== "gimi").map((game) => [game.code, `C:/Games/${game.code}`]));
  renderCachedApp();
  await userEvent.click(await screen.findByRole("tab", { name: "Wuthering Waves" }));
  await userEvent.click(screen.getByRole("button", { name: "Add game" }));
  await userEvent.click(screen.getByRole("button", { name: "Set up Genshin Impact" }));
  await userEvent.type(screen.getByRole("textbox", { name: "Genshin Impact install path" }), "C:/Genshin{Enter}");
  await waitFor(() => expect(screen.getByRole("textbox", { name: "Genshin Impact install path" })).toHaveValue("C:/Genshin"));
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  expect(tabNames()).toEqual(games.map((game) => game.displayName));
  expect(screen.queryByRole("button", { name: "Add game" })).not.toBeInTheDocument();
  expect(screen.getByRole("tab", { name: "Wuthering Waves" })).toHaveAttribute("aria-selected", "true");
  expect(screen.getByRole("tab", { name: "Wuthering Waves" })).toHaveFocus();
});
