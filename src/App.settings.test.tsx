// @ts-expect-error Node types are not part of the frontend; Vitest provides this test-only runtime.
import { readFileSync } from "node:fs";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import { makeQueryClient, renderWithQuery } from "./test/harness";
import { clickMenuItem, clickMenuItemsInOneTick } from "./test/menu";
import type { GameCode, GamePathStatus, GameSummary, SessionInfo } from "./api";

const { invoke, listen, unlisten } = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn(), unlisten: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn() }));
vi.mock("./diagnostics", () => ({ diagnosticsLogDir: vi.fn().mockResolvedValue("C:\\GMM\\logs"), exportDiagnosticsBundle: vi.fn() }));
vi.mock("./updater", () => {
  const idle = { kind: "idle" };
  return { updater: {
    getSnapshot: () => idle,
    subscribe: () => () => {},
    checkForUpdates: vi.fn(),
    install: vi.fn(),
    decline: vi.fn(),
    reportRestartError: vi.fn(),
  } };
});
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
let states: Partial<Record<GameCode, GamePathStatus>>;
let session: SessionInfo | null;
let events: Record<string, () => void>;
const scrollTo = vi.fn();

function renderApp() {
  const client = makeQueryClient();
  client.setDefaultOptions({ queries: { retry: false, gcTime: 300_000, staleTime: 30_000 } });
  return renderWithQuery(<App />, { client });
}
async function openGame() {
  await userEvent.click(await screen.findByRole("button", { name: "Settings for Honkai: Star Rail" }));
}
/** File → Settings…, the only global route into Settings since #351. */
async function openGlobal() {
  await screen.findByRole("tab", { name: "Honkai: Star Rail" });
  await clickMenuItem(listen, "settings");
}
function pageButton(name: string) {
  return within(screen.getByRole("navigation", { name: "Settings pages" })).getByRole("button", { name: new RegExp(name) });
}
function noMainContent() {
  expect(screen.queryByRole("region", { name: "Honkai: Star Rail header", hidden: true })).not.toBeInTheDocument();
  expect(screen.queryByRole("heading", { name: /^Mods \(/, hidden: true })).not.toBeInTheDocument();
  expect(document.querySelector(".dropzone")).toBeNull();
  expect(screen.queryByRole("button", { name: /Launch Honkai/, hidden: true })).not.toBeInTheDocument();
}

beforeEach(() => {
  paths = { srmi: "C:\\StarRail", zzmi: "C:\\ZZZ", wwmi: "C:\\Waves" };
  states = {};
  session = null;
  events = {};
  vi.stubGlobal("scrollTo", scrollTo);
  Object.defineProperty(window, "scrollY", { configurable: true, value: 0 });
  listen.mockImplementation(async (name: string, callback: () => void) => { events[name] = callback; return unlisten; });
  invoke.mockImplementation(async (command: string, args?: { game: GameCode; path: string }) => {
    switch (command) {
      case "is_onboarding_complete": return { complete: true, skipped: false };
      case "list_supported_games": return games;
      case "get_game_install_path": return paths[args!.game] ?? null;
      case "get_game_path_status": return states[args!.game] ?? {
        game: args!.game, savedPath: paths[args!.game] ?? null,
        gamePath: { status: paths[args!.game] ? "readable" : "noSavedPath" },
        modelImporter: { status: "unverified", path: null, error: "Not checked" },
      };
      case "set_game_install_path": paths[args!.game] = args!.path; return;
      case "get_proxy_config": return { url: null, username: null, passwordSet: false };
      case "attention_status": return { safeToProceed: true, reinstalls: [], enabledTransitions: [], importerEvacuations: [], stagedLibraryOperations: [], sessionLaunches: [], activeSession: null, libraryAudits: [], libraryRootOverlaps: [], modPathOverlaps: [] };
      case "get_startup_reconcile_status": return { finished: true, failures: [] };
      case "check_importer_update": return { available: false, installedVersion: null, pinned: false };
      case "get_library_paths": return { defaultRoot: "C:\\GMM\\library", effectiveRoot: "C:\\GMM\\library", rootOverride: null, perGameOverrides: {}, perGameEffective: {} };
      case "current_session": return session;
      case "clean_stale_session": return null;
      case "detect_conflicts": return { conflicts: [], per_mod_count: {} };
      case "audit_library": return { unreferenced: [], duplicates: [], totalBytes: 0 };
      case "list_mods":
      case "list_mod_updates":
      case "interrupted_session_launches": return [];
      default: return null;
    }
  });
});

it.each(["before a Settings visit", "after Back"])("hides the Settings root itself %s", async (stage) => {
  const style = document.createElement("style");
  style.textContent = readFileSync("src/App.css", "utf8");
  document.head.appendChild(style);
  try {
    const { container } = renderApp();
    await screen.findByRole("tab", { name: "Honkai: Star Rail" });
    const root = container.querySelector<HTMLElement>(".settings-screen")!;
    expect(root).toBeInTheDocument();

    if (stage === "after Back") {
      await openGame();
      expect(root).not.toHaveAttribute("hidden");
      expect(root).toBeVisible();
      expect(getComputedStyle(root).display).not.toBe("none");
      expect(root).toContainElement(screen.getByRole("textbox", { name: "Honkai: Star Rail install path" }));
      await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
    }

    // Check both parts: hidden on the root and App.css suppressing its display.
    // toBeVisible alone accepts hidden even if CSS makes the root display: block.
    expect(root).toBeInTheDocument();
    expect(root).toHaveAttribute("hidden");
    expect(root).not.toBeVisible();
    expect(getComputedStyle(root).display).toBe("none");
  } finally {
    style.remove();
  }
});

it.each(["global", "game"])("replaces all main content and preserves independent selection by %s arrival", async (arrival) => {
  renderApp();
  await screen.findByRole("tab", { name: "Honkai: Star Rail" });
  if (arrival === "global") await openGlobal();
  else await openGame();
  expect(screen.getByRole("heading", { name: "Settings", level: 1 })).toBeInTheDocument();
  expect(pageButton(arrival === "global" ? "GMM settings" : "Honkai: Star Rail")).toHaveAttribute("aria-current", "page");
  noMainContent();
  const navigation = screen.getByRole("navigation", { name: "Settings pages" });
  expect(within(navigation).getAllByRole("button")).toHaveLength(7);
  for (const game of games) {
    await userEvent.click(pageButton(game.displayName));
    expect(screen.getByRole("textbox", { name: `${game.displayName} install path` })).toBeInTheDocument();
    expect(screen.getByText("Return to Honkai: Star Rail")).toBeInTheDocument();
    noMainContent();
  }
  await userEvent.click(pageButton("GMM settings"));
  expect(screen.getByPlaceholderText("proxy URL")).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  expect(screen.getByRole("tab", { name: "Honkai: Star Rail" })).toHaveAttribute("aria-selected", "true");
  // The global arrival has no control of its own in the page since #351, so
  // its focus return falls through to the selected Game tab. The per-Game
  // arrival still returns to the control that was used.
  expect(arrival === "global"
    ? screen.getByRole("tab", { name: "Honkai: Star Rail" })
    : screen.getByRole("button", { name: "Settings for Honkai: Star Rail" })).toHaveFocus();
  expect(screen.getAllByRole("tab").map((tab) => tab.textContent)).toEqual(["Honkai: Star Rail", "Zenless Zone Zero", "Wuthering Waves"]);
});

it("adds an absent Game only on accepted save and returns to the original Game", async () => {
  const original = invoke.getMockImplementation()!;
  let reject = true;
  invoke.mockImplementation(async (command, args) => {
    if (command === "set_game_install_path" && reject) throw new Error("Rejected layout");
    return original(command, args);
  });
  renderApp();
  await userEvent.click(await screen.findByRole("button", { name: "Add game" }));
  await userEvent.click(screen.getByRole("button", { name: "Set up Genshin Impact" }));
  await userEvent.type(screen.getByRole("textbox", { name: "Genshin Impact install path" }), "C:\\Genshin{Enter}");
  expect(await screen.findByText("Rejected layout")).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  expect(screen.queryByRole("tab", { name: "Genshin Impact" })).not.toBeInTheDocument();
  await openGame();
  await userEvent.click(pageButton("Genshin Impact"));
  reject = false;
  await userEvent.type(screen.getByRole("textbox", { name: "Genshin Impact install path" }), "C:\\Genshin");
  await userEvent.click(screen.getByRole("button", { name: "Apply Genshin Impact install path" }));
  await waitFor(() => expect(paths.gimi).toBe("C:\\Genshin"));
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  expect(await screen.findByRole("tab", { name: "Genshin Impact" })).toHaveAttribute("aria-selected", "false");
  expect(screen.getByRole("tab", { name: "Honkai: Star Rail" })).toHaveAttribute("aria-selected", "true");
});

it("names missing Game entries in the Settings path details", async () => {
  const missing = ["C:\\StarRail\\StarRail.exe", "C:\\StarRail\\StarRail_Data"];
  states.srmi = { game: "srmi", savedPath: "C:\\StarRail",
    gamePath: { status: "requiredEntriesNotFound", paths: missing },
    modelImporter: { status: "unverified", path: null, error: "Game layout unverified" } };
  renderApp();
  await openGame();
  expect(await screen.findByText(`Missing Game entries: ${missing.join(", ")}`, { selector: "p" })).toBeVisible();
});

const outcomes: { gamePath: GamePathStatus["gamePath"]; modelImporter: GamePathStatus["modelImporter"]; line: string }[] = [
  { gamePath: { status: "noSavedPath" }, modelImporter: { status: "unverified", path: null, error: "no setting" }, line: "Absent · no saved path" },
  { gamePath: { status: "readable" }, modelImporter: { status: "installed" }, line: "SRMI installed · Game readable" },
  { gamePath: { status: "readable" }, modelImporter: { status: "requiredEntriesNotFound", paths: ["C:\\StarRail\\d3dx.ini"] }, line: "Game readable · Model Importer not installed" },
  { gamePath: { status: "savedPathNotFound", paths: ["C:\\StarRail"] }, modelImporter: { status: "unverified", path: null, error: "Missing Game" }, line: "Saved Game installation cannot be found · Model Importer unverified" },
  { gamePath: { status: "requiredEntriesNotFound", paths: ["C:\\StarRail\\StarRail.exe"] }, modelImporter: { status: "unverified", path: null, error: "Missing executable" }, line: "Required Game files or directories not found · Model Importer unverified" },
  { gamePath: { status: "unreadable", path: "C:\\StarRail", error: "Access denied" }, modelImporter: { status: "unverified", path: null, error: "Access denied" }, line: "Cannot tell · path unreadable; Model Importer unverified" },
  { gamePath: { status: "invalid", error: "Wrong path type" }, modelImporter: { status: "unverified", path: null, error: "Wrong layout" }, line: "Cannot verify Game layout · Model Importer unverified" },
  { gamePath: { status: "readable" }, modelImporter: { status: "unverified", path: "C:\\StarRail\\Core", error: "Sharing violation" }, line: "Game readable · Cannot verify Model Importer" },
];
it.each(outcomes)("renders the separate path-state row: $line", async ({ gamePath, modelImporter, line }) => {
  states.srmi = { game: "srmi", savedPath: "C:\\StarRail", gamePath, modelImporter };
  renderApp();
  await openGame();
  await waitFor(() => expect(pageButton("Honkai: Star Rail")).toHaveTextContent(line));
  if (gamePath.status === "savedPathNotFound") {
    expect(screen.getAllByText("Saved Game installation cannot be found · Model Importer unverified", { selector: "p" })).toHaveLength(2);
    expect(screen.queryByText("Saved installation cannot be found · Model Importer unverified")).not.toBeInTheDocument();
  }
  for (const other of outcomes.filter((value) => value.line !== line)) {
    expect(pageButton("Honkai: Star Rail")).not.toHaveTextContent(other.line);
  }
  if (gamePath.status === "invalid" || modelImporter.status === "unverified" && gamePath.status === "readable") {
    expect(pageButton("Honkai: Star Rail")).not.toHaveTextContent(/not installed|Absent|cannot be found/);
  }
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  expect(screen.getByRole("tab", { name: "Honkai: Star Rail" })).toBeInTheDocument();
});

it("keeps path, rebuild and Network feedback across page changes, return, and late rejection", async () => {
  const original = invoke.getMockImplementation()!;
  let rejectPath!: (error: Error) => void;
  invoke.mockImplementation(async (command, args) => {
    if (command === "set_game_install_path") return new Promise((_, reject) => { rejectPath = reject; });
    if (command === "rebuild_junctions") throw new Error("Rebuild failure retained");
    if (command === "set_proxy_config") throw new Error("Proxy failure retained");
    return original(command, args);
  });
  renderApp();
  await openGame();
  await userEvent.type(screen.getByRole("textbox", { name: "Honkai: Star Rail install path" }), "draft{Enter}");
  await userEvent.click(pageButton("GMM settings"));
  await userEvent.click(screen.getByRole("button", { name: "Save" }));
  expect(await screen.findByText("Proxy failure retained")).toBeVisible();
  await act(async () => rejectPath(new Error("Path failure retained")));
  await userEvent.click(pageButton("Honkai: Star Rail"));
  expect(screen.getByText("Path failure retained")).toBeVisible();
  await userEvent.click(screen.getByRole("button", { name: "Rebuild junctions" }));
  expect(await screen.findByText("Rebuild failure retained")).toBeVisible();
  await userEvent.click(pageButton("Genshin Impact"));
  await userEvent.click(pageButton("GMM settings"));
  expect(screen.getByText("Proxy failure retained")).toBeVisible();
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  await openGame();
  expect(screen.getByText("Path failure retained")).toBeVisible();
  expect(screen.getByText("Rebuild failure retained")).toBeVisible();
});

it("refreshes Game Session events while Settings is open without repeating stale cleanup", async () => {
  renderApp();
  await screen.findByRole("tab", { name: "Honkai: Star Rail" });
  const removed = unlisten.mock.calls.length;
  await openGame();
  // session-started, session-ended and the native menu channel (#351) — one
  // subscription each, and a Settings visit must not re-subscribe any of them.
  expect(listen).toHaveBeenCalledTimes(3);
  expect(unlisten).toHaveBeenCalledTimes(removed);
  session = { game: "zzmi", pid: 123, startedAt: "2026-10-11T00:00:00Z" } as SessionInfo;
  const reads = invoke.mock.calls.filter(([command]) => command === "current_session").length;
  await act(async () => events["session-started"]());
  await waitFor(() => expect(invoke.mock.calls.filter(([command]) => command === "current_session").length).toBeGreaterThan(reads));
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  expect(await screen.findByText("ZZMI is running")).toBeInTheDocument();
  expect(screen.queryByText(/ended unexpectedly/)).not.toBeInTheDocument();
  expect(invoke.mock.calls.filter(([command]) => command === "clean_stale_session")).toHaveLength(1);
  await openGame();
  session = null;
  await act(async () => events["session-ended"]());
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  await waitFor(() => expect(screen.queryByText("ZZMI is running")).not.toBeInTheDocument());
  expect(listen).toHaveBeenCalledTimes(3);
});

it.each(["button", "escape"])("restores the user's Mod-list scroll position on %s return", async (route) => {
  const original = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (command, args) => command === "list_mods" ? [{
    id: "mod-return", game: "srmi", name: "Return Outfit", source: "manual",
    library_path: "C:\\GMM\\library\\srmi\\mod-return", enabled: false,
    reinstall_recovery: null, enabled_transition_recovery: null,
  }] : command === "list_variants" ? { variants: [], activeVariantId: null } : original(command, args));
  renderApp();
  await screen.findByText("Return Outfit");
  await screen.findByRole("tab", { name: "Honkai: Star Rail" });
  Object.defineProperty(window, "scrollY", { configurable: true, value: 860 });
  await openGame();
  await userEvent.click(pageButton("Wuthering Waves"));
  if (route === "button") await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  else fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.getByText("Return Outfit")).toBeInTheDocument();
  expect(scrollTo).toHaveBeenLastCalledWith(0, 860);
  expect(screen.getByRole("tab", { name: "Honkai: Star Rail" })).toHaveAttribute("aria-selected", "true");
});

it("retains audit rejection after its row disappears and after a Settings page change", async () => {
  const original = invoke.getMockImplementation()!;
  let orphan = true;
  invoke.mockImplementation(async (command, args) => {
    if (command === "audit_library") return { unreferenced: orphan ? [{ path: "C:\\GMM\\library\\srmi\\orphan", directoryName: "orphan", sizeBytes: 128 }] : [], duplicates: [], totalBytes: 128 };
    if (command === "recover_unreferenced_library_dir") { orphan = false; throw new Error("Recovery failure retained"); }
    return original(command, args);
  });
  renderApp();
  await openGame();
  await userEvent.click(await screen.findByRole("button", { name: "Recover…" }));
  await userEvent.type(screen.getByPlaceholderText("What is this mod called?"), "Recovered Outfit");
  await userEvent.click(screen.getByRole("button", { name: "Recover" }));
  expect(await screen.findByText(/Recovery failure retained/)).toBeVisible();
  await waitFor(() => expect(screen.queryByRole("button", { name: "Recover…" })).not.toBeInTheDocument());
  await userEvent.click(pageButton("GMM settings"));
  await userEvent.click(pageButton("Honkai: Star Rail"));
  expect(screen.getByText(/Recovery failure retained/)).toBeVisible();
});

it("restores the same Mod row when content above it changes during Settings", async () => {
  const original = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (command, args) => command === "list_mods" ? [{
    id: "mod-anchor", game: "srmi", name: "Anchor Outfit", source: "manual",
    library_path: "C:\\GMM\\library\\srmi\\anchor", enabled: false,
    reinstall_recovery: null, enabled_transition_recovery: null,
  }] : command === "list_variants" ? { variants: [], activeVariantId: null } : original(command, args));
  let rowTop = -40;
  const bounds = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
    return { top: this.dataset.modId ? rowTop : 0, bottom: this.dataset.modId ? rowTop + 180 : 0 } as DOMRect;
  });
  try {
    renderApp();
    await screen.findByText("Anchor Outfit");
    Object.defineProperty(window, "scrollY", { configurable: true, value: 860 });
    await openGame();
    Object.defineProperty(window, "scrollY", { configurable: true, value: 0 });
    rowTop = 20;
    await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
    expect(scrollTo).toHaveBeenLastCalledWith(0, 60);
  } finally {
    bounds.mockRestore();
  }
});

it("keeps unavailable checks distinct from absence and preserves the saved-path repair route", async () => {
  const original = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (command, args) => {
    if (command === "get_game_path_status" && args.game === "srmi") throw new Error("Path inspection failed");
    if (command === "audit_library") throw new Error("Audit inspection failed");
    return original(command, args);
  });
  renderApp();
  await openGame();
  await waitFor(() => expect(pageButton("Honkai: Star Rail")).toHaveTextContent("Cannot tell · path check unavailable"));
  expect(pageButton("Honkai: Star Rail")).not.toHaveTextContent(/Absent|not installed/);
  expect(screen.getByRole("textbox", { name: "Honkai: Star Rail install path" })).toHaveValue("C:\\StarRail");
  expect((await screen.findAllByText("Path inspection failed"))[0]).toBeVisible();
  expect(await screen.findByText("Audit inspection failed")).toBeInTheDocument();
});

it.each([
  { command: "set_game_install_path", failed: false },
  { command: "set_game_install_path", failed: true },
  { command: "detect_game_install_path", failed: false },
  { command: "detect_game_install_path", failed: true },
])("refreshes path-state evidence on settlement of $command (rejected: $failed)", async ({ command, failed }) => {
  const original = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (name, args) => {
    if (name === command) {
      states.srmi = { game: "srmi", savedPath: "C:\\StarRail", gamePath: { status: "readable" }, modelImporter: { status: "installed" } };
      if (failed) throw new Error("Path operation rejected");
      return command === "detect_game_install_path" ? "C:\\StarRail" : null;
    }
    return original(name, args);
  });
  renderApp();
  await openGame();
  await waitFor(() => expect(pageButton("Honkai: Star Rail")).toHaveTextContent("Game readable · Cannot verify Model Importer"));
  if (command === "set_game_install_path") await userEvent.type(screen.getByRole("textbox", { name: "Honkai: Star Rail install path" }), "changed{Enter}");
  else await userEvent.click(screen.getByRole("button", { name: "Re-detect" }));
  await waitFor(() => expect(pageButton("Honkai: Star Rail")).toHaveTextContent("SRMI installed · Game readable"));
  if (failed) expect(screen.getByText("Path operation rejected")).toBeVisible();
});

it("keeps the Mod list available across Settings even when inactive cache entries expire immediately", async () => {
  const original = invoke.getMockImplementation()!;
  let returning = false;
  invoke.mockImplementation(async (command, args) => {
    if (command === "list_mods") {
      if (returning) throw new Error("List read is now unavailable");
      return [{ id: "mod-retained", game: "srmi", name: "Retained Outfit", source: "manual",
        library_path: "C:\\GMM\\library\\srmi\\retained", enabled: false,
        reinstall_recovery: null, enabled_transition_recovery: null }];
    }
    if (command === "list_variants") return { variants: [], activeVariantId: null };
    return original(command, args);
  });
  const client = makeQueryClient();
  client.setDefaultOptions({ queries: { retry: false, gcTime: 0, staleTime: 30_000 } });
  renderWithQuery(<App />, { client });
  await screen.findByText("Retained Outfit");
  await openGame();
  await userEvent.click(pageButton("GMM settings"));
  returning = true;
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  expect(await screen.findByText("Retained Outfit")).toBeVisible();
  expect(screen.queryByText("List read is now unavailable")).not.toBeInTheDocument();
});

it("re-renders durable quarantine, launch reservations and audit findings after Back", async () => {
  const original = invoke.getMockImplementation()!;
  const auditPath = "C:\\GMM\\library\\srmi\\durable-orphan";
  invoke.mockImplementation(async (command, args) => {
    if (command === "list_mods") return [{
      id: "mod-durable", game: "srmi", name: "Durable Outfit", source: "manual",
      library_path: "C:\\GMM\\library\\srmi\\durable", enabled: true,
      enabled_transition_recovery: null,
      reinstall_recovery: { reason: "Deployment withdrawal failed", attemptedAt: "2026-10-11T00:00:00Z", attempts: 1,
        libraryPath: "C:\\GMM\\library\\srmi\\durable", stagedPath: "C:\\GMM\\library\\srmi\\stage",
        quarantinePath: "C:\\GMM\\library\\srmi\\old", junctionWithdrawn: false,
        junctionWithdrawalError: "Access denied" },
    }];
    if (command === "list_variants") return { variants: [], activeVariantId: null };
    if (command === "interrupted_session_launches") return [{ id: "durable-launch", game: "srmi", child_pid: null, started_at: "2026-10-11T00:00:00Z" }];
    if (command === "audit_library") return { unreferenced: [{ path: auditPath, directoryName: "durable-orphan", sizeBytes: 128 }], duplicates: [], totalBytes: 128 };
    if (command === "attention_status") return { ...await original(command, args), safeToProceed: false,
      libraryAudits: [{ game: "srmi", unreferenced: [{ path: auditPath }], duplicates: [] }] };
    return original(command, args);
  });
  renderApp();
  await screen.findByRole("region", { name: "Interrupted SRMI launch" });
  await screen.findByRole("region", { name: "Interrupted reinstall recovery for Durable Outfit" });
  const navigationCalls = invoke.mock.calls.length;
  await openGame();
  expect(await screen.findByRole("region", { name: "Unreferenced Library folders and duplicate Mod records" })).toHaveTextContent(auditPath);
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  expect(screen.getByRole("region", { name: "Interrupted reinstall recovery for Durable Outfit" })).toHaveTextContent("this Mod may still be loading");
  expect(screen.getByRole("region", { name: "Interrupted SRMI launch" })).toHaveTextContent("kept the launch reservation");
  const navigationReads = new Set([
    "audit_library", "current_session", "interrupted_session_launches", "detect_conflicts",
    "check_importer_update", "attention_status", "fetch_latest_importer_release",
    "importer_origin_status", "mod_updates_globally_enabled", "av_guidance",
  ]);
  const navigationCommands = invoke.mock.calls.slice(navigationCalls).map(([command]) => command);
  expect(navigationCommands.filter((command) =>
    !/^(get_|list_)/.test(command) && !navigationReads.has(command),
  )).toEqual([]);
  await userEvent.click(screen.getByRole("button", { name: /Needs attention.*All Games/ }));
  expect(within(screen.getByRole("region", { name: "Attention across all Games" })).getByText(auditPath)).toBeVisible();
  await openGame();
  expect(screen.getByRole("region", { name: "Unreferenced Library folders and duplicate Mod records" })).toHaveTextContent(auditPath);
});


it.each([
  ["install_importer", "Install importer"],
  ["rollback_importer", "Roll back importer"],
  ["set_importer_pinned", "Pin to current"],
  ["retire_interrupted_importer_evacuation", "I confirmed no other GMM is changing this importer"],
  ["retry_importer_evacuation_recovery", "Retry Model Importer recovery"],
])("refreshes cached Settings path-state after %s", async (writer, button) => {
  const original = invoke.getMockImplementation()!;
  states.srmi = { game: "srmi", savedPath: "C:\\StarRail", gamePath: { status: "readable" },
    modelImporter: writer === "set_importer_pinned" ? { status: "installed" } : { status: "requiredEntriesNotFound", paths: ["C:\\StarRail\\d3dx.ini"] } };
  let changed = false;
  invoke.mockImplementation(async (command, args) => {
    if (command === "check_importer_update") return { available: false, installedVersion: "1.0", pinned: false };
    if (command === "get_importer_evacuation_recovery" && writer.includes("evacuation") && !changed) return {
      reason: "Interrupted importer", attemptedAt: "2026-10-11T00:00:00Z", attempts: 1,
      gamePath: "C:\\StarRail", backupPath: "C:\\backup", ownerUncertain: writer.startsWith("retire"),
      action: writer.startsWith("retire") ? "retireProducer" : "retry",
    };
    if (command === writer) {
      changed = true;
      // For pin, simulate files removed externally while the cached page is open.
      // The setting write must refresh what Settings can verify about the files.
      states.srmi = { ...states.srmi!, modelImporter: writer === "set_importer_pinned"
        ? { status: "requiredEntriesNotFound", paths: ["C:\\StarRail\\d3dx.ini"] } : { status: "installed" } };
      return command === "rollback_importer" ? "C:\\backup" : command === "install_importer" ? { sha256: "abc123", backup_dir: null, rewrote_files: [] } : null;
    }
    return original(command, args);
  });
  renderApp();
  await openGame();
  await waitFor(() => expect(pageButton("Honkai: Star Rail")).toHaveTextContent(writer === "set_importer_pinned" ? "SRMI installed" : "Model Importer not installed"));
  await userEvent.click(await screen.findByRole("button", { name: new RegExp(button) }));
  await waitFor(() => expect(changed).toBe(true));
  await waitFor(() => expect(pageButton("Honkai: Star Rail")).toHaveTextContent(writer === "set_importer_pinned"
    ? "Game readable · Model Importer not installed" : "SRMI installed · Game readable"));
  expect(pageButton("Honkai: Star Rail")).not.toHaveTextContent(writer === "set_importer_pinned"
    ? "SRMI installed" : "Model Importer not installed");
});

it.each(["delete", "recover"])("Escape cancels the Library %s prompt before leaving Settings", async (action) => {
  const original = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (command, args) => command === "audit_library" ? {
    unreferenced: [{ path: "C:\\GMM\\library\\srmi\\orphan", directoryName: "orphan", sizeBytes: 128 }], duplicates: [], totalBytes: 128,
  } : original(command, args));
  renderApp();
  await openGame();
  const audit = await screen.findByRole("region", { name: "Unreferenced Library folders and duplicate Mod records" });
  await userEvent.click(within(audit).getByRole("button", { name: action === "delete" ? "Delete…" : "Recover…" }));
  expect(within(audit).getByRole("button", { name: "Cancel" })).toBeVisible();
  await userEvent.keyboard("{Escape}");
  expect(screen.getByRole("heading", { name: "Settings", level: 1 })).toBeVisible();
  expect(within(audit).queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument();
  expect(within(audit).getByRole("button", { name: action === "delete" ? "Delete…" : "Recover…" })).toHaveFocus();
});

it("keeps keyboard focus on the Game navigation button after changing pages", async () => {
  renderApp();
  await openGame();
  const next = pageButton("Genshin Impact");
  await userEvent.tab(); // Back to Mods
  await userEvent.tab(); // First Game row
  expect(next).toHaveFocus();
  await userEvent.keyboard("{Enter}");
  expect(next).toHaveFocus();
  expect(screen.getByRole("textbox", { name: "Genshin Impact install path" })).toBeVisible();
});

it("refreshes cached Settings path-state when setup finishes", async () => {
  states.srmi = { game: "srmi", savedPath: "C:\\StarRail", gamePath: { status: "readable" }, modelImporter: { status: "requiredEntriesNotFound", paths: ["d3dx.ini"] } };
  renderApp();
  await openGame();
  await waitFor(() => expect(pageButton("Honkai: Star Rail")).toHaveTextContent("Model Importer not installed"));
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  await clickMenuItem(listen, "run-setup");
  await screen.findByRole("heading", { name: "GMM — first-run setup" });
  states.srmi = { ...states.srmi, modelImporter: { status: "installed" } };
  await userEvent.click(screen.getByRole("button", { name: "Skip setup" }));
  await openGame();
  await waitFor(() => expect(pageButton("Honkai: Star Rail")).toHaveTextContent("SRMI installed · Game readable"));
});

it("refreshes cached Settings uncertainty after importer installation rejects", async () => {
  const original = invoke.getMockImplementation()!;
  states.srmi = { game: "srmi", savedPath: "C:\\StarRail", gamePath: { status: "readable" }, modelImporter: { status: "installed" } };
  invoke.mockImplementation(async (command, args) => {
    if (command === "install_importer") {
      states.srmi = { ...states.srmi!, modelImporter: { status: "unverified", path: "C:\\StarRail\\Core", error: "Restoration incomplete" } };
      throw new Error("Install rollback failed");
    }
    return original(command, args);
  });
  renderApp();
  await openGame();
  await waitFor(() => expect(pageButton("Honkai: Star Rail")).toHaveTextContent("SRMI installed"));
  await userEvent.click(await screen.findByRole("button", { name: "Reinstall importer" }));
  await screen.findByText("Install rollback failed");
  await waitFor(() => expect(pageButton("Honkai: Star Rail")).toHaveTextContent("Game readable · Cannot verify Model Importer"));
});

it("keeps importer groups only on each visited Game page, in order, while preserving hidden drafts and outcomes", async () => {
  const original = invoke.getMockImplementation()!;
  const origin = { kind: "gitHubRelease", owner: "Example", repo: "Package", asset_pattern: "Package.zip" };
  invoke.mockImplementation(async (command, args) => {
    if (command === "importer_origin_status") return {
      game: args.game, displayName: "Honkai: Star Rail", resolved: { state: "inEffect", origin, layer: "compiledInDefault" },
      installTarget: { state: "installed", ...origin }, installed: { state: "known", ...origin }, userOverride: { state: "notSet" },
      compiledDefault: origin, proposal: { origin, reason: "Origin replacement", replaces: { state: "unknown" } },
      dismissed: [], dismissalsError: null, recommendationsEnabled: true, recommendationsUnusableReason: null,
    };
    if (command === "check_importer_update") return { available: false, installedVersion: "1.0", pinned: true, latestVersion: "2.0", upstreamAhead: true, checkError: null };
    return original(command, args);
  });
  states.srmi = { game: "srmi", savedPath: "C:/StarRail", gamePath: { status: "readable" }, modelImporter: { status: "installed" } };
  renderApp();
  await screen.findByRole("tab", { name: "Honkai: Star Rail" });
  expect(screen.queryByRole("heading", { name: /Model Importer \(/ })).not.toBeInTheDocument();
  expect(screen.queryByRole("region", { name: "Importer Origin" })).not.toBeInTheDocument();
  await openGame();
  const heading = await screen.findByRole("heading", { name: "Model Importer (SRMI)" });
  const gamePage = heading.closest("section")!.parentElement!;
  expect(within(gamePage as HTMLElement).getAllByRole("heading", { level: 2 }).map((value) => value.textContent).slice(0, 4)).toEqual([
    "Honkai: Star Rail", "Honkai: Star Rail install path", "Model Importer (SRMI)", "Importer Origin",
  ]);
  expect(await screen.findByRole("button", { name: "Switch and install" })).toBeEnabled();
  expect(screen.queryByRole("button", { name: "Apply update" })).not.toBeInTheDocument();
  await userEvent.type(screen.getByLabelText("Owner"), "draft-owner");
  await userEvent.click(pageButton("GMM settings"));
  expect(screen.queryByRole("region", { name: "Importer Origin" })).not.toBeInTheDocument();
  expect(gamePage).toHaveAttribute("hidden");
  await userEvent.click(pageButton("Honkai: Star Rail"));
  expect(screen.getByLabelText("Owner")).toHaveValue("draft-owner");
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  expect(screen.queryByRole("heading", { name: /Model Importer \(/ })).not.toBeInTheDocument();
  await openGame();
  expect(screen.getByLabelText("Owner")).toHaveValue("draft-owner");
});

it.each(["retry_importer_evacuation_recovery", "accept_importer_origin_proposal"])("keeps hidden %s observers alive and retains late failure after its recovery or proposal disappears", async (writer) => {
  const original = invoke.getMockImplementation()!;
  const origin = { kind: "gitHubRelease", owner: "Example", repo: "Package", asset_pattern: "Package.zip" };
  let changed = false;
  let reject!: (error: Error) => void;
  states.srmi = { game: "srmi", savedPath: "C:/StarRail", gamePath: { status: "readable" }, modelImporter: { status: "installed" } };
  invoke.mockImplementation(async (command, args) => {
    if (command === writer) return new Promise((_, no) => { reject = no; });
    if (command === "get_importer_evacuation_recovery") return writer.includes("evacuation") && !changed ? {
      gamePath: "C:/StarRail", backupPath: "C:/Backup", reason: "Pending recovery", action: "retry", ownerUncertain: false, attempts: 1, attemptedAt: "2026-10-11",
    } : null;
    if (command === "importer_origin_status") return {
      game: args.game, displayName: "Honkai: Star Rail", resolved: { state: "inEffect", origin, layer: "compiledInDefault" },
      installTarget: { state: "installed", ...origin }, installed: { state: "known", ...origin }, userOverride: { state: "notSet" }, compiledDefault: origin,
      proposal: !writer.includes("evacuation") && !changed ? { origin, reason: null, replaces: { state: "unknown" } } : null,
      dismissed: [], dismissalsError: null, recommendationsEnabled: true, recommendationsUnusableReason: null,
    };
    return original(command, args);
  });
  renderApp();
  await openGame();
  await userEvent.click(await screen.findByRole("button", { name: writer.includes("evacuation") ? "Retry Model Importer recovery" : "Switch and install" }));
  await userEvent.click(pageButton("GMM settings"));
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  changed = true;
  await act(async () => reject(new Error("Late importer failure retained")));
  await openGame();
  expect(await screen.findByText("Late importer failure retained")).toBeVisible();
  expect(screen.queryByRole("button", { name: "Retry Model Importer recovery" })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Switch and install" })).not.toBeInTheDocument();
});

it("keeps application groups only on GMM in the approved order and links back to each Game", async () => {
  renderApp();
  await screen.findByRole("tab", { name: "Honkai: Star Rail" });
  expect(screen.getByRole("heading", { name: "Mod updates" })).toBeVisible();
  for (const name of ["Network", "GMM updates", "Importer recommendations", "Diagnostics"]) {
    expect(screen.queryByRole("heading", { name })).not.toBeInTheDocument();
  }
  await clickMenuItem(listen, "settings");
  const content = screen.getByRole("heading", { name: "Global Library root" }).parentElement!.parentElement!;
  expect(within(content).getAllByRole("heading", { level: 2 }).map((heading) => heading.textContent)).toEqual([
    "GMM settings", "Global Library root", "Network", "GMM updates", "Importer recommendations", "Diagnostics",
  ]);
  await userEvent.click(screen.getByRole("button", { name: "View Honkai: Star Rail importer proposals, dismissals and overrides" }));
  expect(pageButton("Honkai: Star Rail")).toHaveAttribute("aria-current", "page");
  expect(screen.queryByRole("checkbox", { name: /Recommend importer origins/ })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "View GMM importer recommendations" }));
  expect(pageButton("GMM settings")).toHaveAttribute("aria-current", "page");
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  expect(screen.getByRole("heading", { name: "Mod updates" })).toBeVisible();
  for (const name of ["Network", "GMM updates", "Importer recommendations", "Diagnostics"]) {
    expect(screen.queryByRole("heading", { name })).not.toBeInTheDocument();
  }
});

it("preserves Network saved credentials, edits, pending actions and successful password clearing", async () => {
  const original = invoke.getMockImplementation()!;
  let config = { url: "socks5://saved:1080", username: "saved-user", passwordSet: true };
  let completeSave!: () => void;
  let completeTest!: () => void;
  invoke.mockImplementation(async (command, args) => {
    if (command === "get_proxy_config") return config;
    if (command === "set_proxy_config") return new Promise<void>((yes) => { completeSave = () => { config = { url: args.args.url, username: args.args.username, passwordSet: true }; yes(); }; });
    if (command === "test_proxy_connection") return new Promise<void>((yes) => { completeTest = yes; });
    return original(command, args);
  });
  renderApp();
  await clickMenuItem(listen, "settings");
  const password = await screen.findByPlaceholderText("(set — leave blank to keep)");
  expect(password).toHaveValue("");
  expect(password).toHaveAttribute("type", "password");
  expect(screen.getByPlaceholderText("proxy URL")).toHaveValue("socks5://saved:1080");
  expect(screen.getByPlaceholderText("username (optional)")).toHaveValue("saved-user");
  await userEvent.clear(screen.getByPlaceholderText("proxy URL"));
  await userEvent.type(screen.getByPlaceholderText("proxy URL"), "http://edited:8080");
  await userEvent.clear(screen.getByPlaceholderText("username (optional)"));
  await userEvent.type(screen.getByPlaceholderText("username (optional)"), "edited-user");
  await userEvent.type(password, "new-secret");
  await userEvent.click(screen.getByRole("button", { name: "Save" }));
  expect(screen.getByRole("button", { name: "Saving…" })).toBeDisabled();
  expect(invoke).toHaveBeenCalledWith("set_proxy_config", { args: { url: "http://edited:8080", username: "edited-user", password: "new-secret" } });
  await act(async () => completeSave());
  await waitFor(() => expect(password).toHaveValue(""));
  expect(screen.getByPlaceholderText("proxy URL")).toHaveValue("http://edited:8080");
  await userEvent.click(screen.getByRole("button", { name: "Test connection" }));
  expect(screen.getByRole("button", { name: "Testing…" })).toBeDisabled();
  await act(async () => completeTest());
  expect(await screen.findByText("Proxy reachable.")).toBeVisible();
  expect(invoke).toHaveBeenCalledWith("test_proxy_connection", undefined);
});

it("keeps Network loading, unset and unavailable states distinct", async () => {
  const original = invoke.getMockImplementation()!;
  let resolve!: (value: unknown) => void;
  invoke.mockImplementation(async (command, args) => command === "get_proxy_config" ? new Promise((yes) => { resolve = yes; }) : original(command, args));
  const { client } = renderApp();
  await clickMenuItem(listen, "settings");
  expect(screen.getByText("Reading Network configuration…")).toBeVisible();
  await act(async () => resolve({ url: null, username: null, passwordSet: false }));
  expect(await screen.findByPlaceholderText("password")).toHaveValue("");
  expect(screen.getByPlaceholderText("proxy URL")).toHaveValue("");
  invoke.mockImplementation(async (command, args) => { if (command === "get_proxy_config") throw new Error("Network configuration unreadable"); return original(command, args); });
  await act(() => client.invalidateQueries({ queryKey: ["proxyConfig"] }));
  expect(await screen.findByText("Network configuration unreadable")).toBeVisible();
  expect(screen.queryByText("Reading Network configuration…")).not.toBeInTheDocument();
});

it("keeps Network connection failures visible", async () => {
  const original = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (command, args) => { if (command === "test_proxy_connection") throw new Error("Proxy connection refused"); return original(command, args); });
  renderApp();
  await clickMenuItem(listen, "settings");
  await userEvent.click(screen.getByRole("button", { name: "Test connection" }));
  expect(await screen.findByText("Proxy connection refused")).toBeVisible();
  expect(screen.queryByText("Proxy reachable.")).not.toBeInTheDocument();
});

// ---- #351 — the native File/Help menu ----

it.each([
  { action: "settings", panel: "Global Library root" },
  { action: "check-updates", panel: "GMM updates" },
  { action: "diagnostics", panel: "Diagnostics" },
] as const)("routes the $action menu item to the Settings GMM page", async ({ action, panel }) => {
  renderApp();
  await screen.findByRole("tab", { name: "Honkai: Star Rail" });
  expect(screen.queryByRole("heading", { name: panel })).not.toBeInTheDocument();

  await clickMenuItem(listen, action);

  expect(screen.getByRole("heading", { name: "Settings", level: 1 })).toBeVisible();
  // Check for updates and Diagnostics are routes to panels that already live
  // on the GMM page. Landing on a Game page, or not opening at all, fails.
  expect(pageButton("GMM settings")).toHaveAttribute("aria-current", "page");
  for (const game of games) {
    expect(pageButton(game.displayName)).not.toHaveAttribute("aria-current");
  }
  expect(screen.getByRole("heading", { name: panel })).toBeVisible();
  noMainContent();
});

it("resets onboarding from the menu once, and not again while the reset is pending", async () => {
  const original = invoke.getMockImplementation()!;
  let finishReset!: () => void;
  invoke.mockImplementation(async (command, args) => command === "reset_onboarding"
    ? new Promise<void>((yes) => { finishReset = yes; })
    : original(command, args));
  renderApp();
  await screen.findByRole("tab", { name: "Honkai: Star Rail" });

  await clickMenuItem(listen, "run-setup");
  const resets = () => invoke.mock.calls.filter(([command]) => command === "reset_onboarding");
  expect(resets()).toHaveLength(1);
  // The in-page control was disabled while pending. The menu item cannot grey
  // itself out, so a second click must not start a second reset.
  await clickMenuItem(listen, "run-setup");
  expect(resets()).toHaveLength(1);

  await act(async () => finishReset());
  expect(await screen.findByRole("heading", { name: "GMM — first-run setup" })).toBeVisible();
});

it("keeps a failed menu reset on the main screen instead of losing it with the menu", async () => {
  const original = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (command, args) => {
    if (command === "reset_onboarding") throw new Error("Setup reset refused");
    return original(command, args);
  });
  renderApp();
  await screen.findByRole("tab", { name: "Honkai: Star Rail" });

  await clickMenuItem(listen, "run-setup");

  // A failure stays a persistent error on the main screen; the menu it was
  // started from is already closed and can hold nothing.
  expect(await screen.findByText("Setup reset refused")).toBeVisible();
  expect(screen.queryByRole("heading", { name: "GMM — first-run setup" })).not.toBeInTheDocument();
  expect(screen.getByRole("tab", { name: "Honkai: Star Rail" })).toBeVisible();
  // Still retryable: the failure is a notice, not a dead end.
  await clickMenuItem(listen, "run-setup");
  expect(invoke.mock.calls.filter(([command]) => command === "reset_onboarding")).toHaveLength(2);
});

it("falls back to Add game when a menu-opened visit returns with no Game tab", async () => {
  paths = {};
  renderApp();
  await screen.findByRole("button", { name: "Add game" });

  await clickMenuItem(listen, "settings");
  expect(screen.getByRole("heading", { name: "Settings", level: 1 })).toBeVisible();
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));

  // No Game is installed, so there is no selected tab either. Focus must not
  // be left on <body>: the next Tab press would restart from the top.
  expect(screen.getByRole("button", { name: "Add game" })).toHaveFocus();
});

it("falls back to the main region when a menu-opened visit returns before any control exists", async () => {
  const original = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (command, args) => command === "get_game_install_path"
    ? new Promise(() => {})
    : original(command, args));
  renderApp();
  // Install paths never resolve, so the tab strip shows neither a tab nor
  // Add game — the menu is reachable before either exists.
  expect(await screen.findByText("Checking game install paths…")).toBeVisible();

  await clickMenuItem(listen, "settings");
  expect(screen.getByRole("heading", { name: "Settings", level: 1 })).toBeVisible();
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));

  expect(screen.queryByRole("tab")).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Add game" })).not.toBeInTheDocument();
  const main = document.querySelector("main[data-main-screen]");
  expect(main).not.toBeNull();
  expect(main).toHaveFocus();
  expect(document.body).not.toHaveFocus();
});

// The menu is reachable from every state, including from inside an open
// Settings visit. The in-page controls never were: the Settings button was
// unmounted with the main screen. Every assumption the old handlers made
// about when they could run has to be re-proved for the menu.

it.each(["button", "escape"])("keeps the main-screen return point when a menu action arrives mid-visit, on %s return", async (route) => {
  const original = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (command, args) => command === "list_mods" ? [{
    id: "mod-return", game: "srmi", name: "Return Outfit", source: "manual",
    library_path: "C:\\GMM\\library\\srmi\\mod-return", enabled: false,
    reinstall_recovery: null, enabled_transition_recovery: null,
  }] : command === "list_variants" ? { variants: [], activeVariantId: null } : original(command, args));
  renderApp();
  await screen.findByText("Return Outfit");
  await screen.findByRole("tab", { name: "Honkai: Star Rail" });

  Object.defineProperty(window, "scrollY", { configurable: true, value: 860 });
  await clickMenuItem(listen, "settings");
  // The main screen is unmounted now, so this is the Settings surface's own
  // scroll position and there is no `[data-mod-id]` row to anchor to.
  Object.defineProperty(window, "scrollY", { configurable: true, value: 150 });
  await clickMenuItem(listen, "diagnostics");
  expect(pageButton("GMM settings")).toHaveAttribute("aria-current", "page");

  if (route === "button") await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  else fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.getByText("Return Outfit")).toBeInTheDocument();
  expect(scrollTo).toHaveBeenLastCalledWith(0, 860);
  expect(screen.getByRole("tab", { name: "Honkai: Star Rail" })).toHaveAttribute("aria-selected", "true");
});

it("keeps the arrival control when a menu action arrives during a per-Game visit", async () => {
  renderApp();
  await screen.findByRole("tab", { name: "Honkai: Star Rail" });

  await openGame();
  await clickMenuItem(listen, "check-updates");
  expect(pageButton("GMM settings")).toHaveAttribute("aria-current", "page");
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));

  // Entry was the status-bar link, so that is where focus belongs. A menu
  // action during the visit must not re-point the return at the `"global"`
  // arrival, which has no control in the page at all.
  expect(screen.getByRole("button", { name: "Settings for Honkai: Star Rail" })).toHaveFocus();
});

it("starts one reset when two run-setup events arrive before React commits", async () => {
  const original = invoke.getMockImplementation()!;
  let finishReset!: () => void;
  invoke.mockImplementation(async (command, args) => command === "reset_onboarding"
    ? new Promise<void>((yes) => { finishReset = yes; })
    : original(command, args));
  renderApp();
  await screen.findByRole("tab", { name: "Honkai: Star Rail" });

  // No flush between the two deliveries. `reopenSetup.isPending` is a render
  // value, so both would read `false` and `reset_onboarding` — which re-runs
  // first-run setup — would run twice concurrently.
  await clickMenuItemsInOneTick(listen, "run-setup", "run-setup");

  expect(invoke.mock.calls.filter(([command]) => command === "reset_onboarding")).toHaveLength(1);
  await act(async () => finishReset());
  expect(await screen.findByRole("heading", { name: "GMM — first-run setup" })).toBeVisible();
});

it("moves focus out of a Settings page the menu is about to hide", async () => {
  renderApp();
  await screen.findByRole("tab", { name: "Honkai: Star Rail" });

  await openGame();
  const editor = screen.getByRole("textbox", { name: "Honkai: Star Rail install path" });
  editor.focus();
  expect(editor).toHaveFocus();

  await clickMenuItem(listen, "diagnostics");

  // The Game page is hidden now. Nothing in the page moved focus — the menu
  // is not a control that can take it — so focus would sit on a hidden
  // element and the next Tab would restart at the top of the window.
  expect(editor).not.toBeVisible();
  expect(editor).not.toHaveFocus();
  expect(screen.getByRole("heading", { name: "Settings", level: 1 })).toHaveFocus();
});

it("leaves focus on the Settings navigation control the user clicked", async () => {
  renderApp();
  await screen.findByRole("tab", { name: "Honkai: Star Rail" });

  await openGame();
  await userEvent.click(pageButton("GMM settings"));

  // The navigation button stays visible, so moving focus to the heading
  // would take it away from the control the user is operating.
  expect(pageButton("GMM settings")).toHaveFocus();
  expect(screen.getByRole("heading", { name: "Settings", level: 1 })).not.toHaveFocus();
});
