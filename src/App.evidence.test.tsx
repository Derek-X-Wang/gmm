import { onlineManager } from "@tanstack/react-query";
import { screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { GameCode, GameSummary } from "./api";
import { renderWithQuery } from "./test/harness";
import { clickMenuItem } from "./test/menu";

// Each test makes ONE read fail (a rejected command) and asserts what the
// surface that depends on it must stop claiming. A read that never succeeded
// is the case where there is no value at all to fall back on.

const { invoke, listen } = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn().mockResolvedValue(() => {}) }));
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
];
let paths: Partial<Record<GameCode, string>>;
let failing: Record<string, unknown>;

beforeEach(() => {
  paths = { gimi: "C:\\Games\\GIMI", srmi: "C:\\Games\\SRMI" };
  failing = {};
  invoke.mockImplementation(async (command: string, args?: { game: GameCode }) => {
    const failure = failing[args?.game ? `${command}:${args.game}` : command] ?? failing[command];
    if (failure !== undefined) throw failure;
    switch (command) {
      case "is_onboarding_complete": return { complete: true, skipped: false };
      case "list_supported_games": return games;
      case "get_game_install_path": return paths[args!.game] ?? null;
      case "get_game_path_status": return {
        game: args!.game, savedPath: paths[args!.game] ?? null,
        gamePath: { status: "readable" },
        modelImporter: { status: "noImporterEntriesFound", paths: ["d3dx.ini"] },
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
      case "detect_conflicts": return { conflicts: [], omitted: [], per_mod_count: {} };
      case "mod_updates_globally_enabled": return true;
      case "list_presets": return [];
      case "list_mods":
      case "list_mod_updates":
      case "interrupted_session_launches": return [];
      default: return null;
    }
  });
});

afterEach(() => {
  onlineManager.setOnline(true);
});

it("does not treat a paused first read of the onboarding status as unfinished setup", async () => {
  // Offline, the read does not fail: it pauses with no value and no error.
  onlineManager.setOnline(false);
  renderWithQuery(<App />);

  expect(await screen.findByRole("alert")).toHaveTextContent(/could not read whether setup is finished/i);
  expect(screen.queryByRole("heading", { name: "First-run setup" })).not.toBeInTheDocument();
});

it("does not replace the application with first-run setup when the onboarding status cannot be read", async () => {
  failing.is_onboarding_complete = "onboarding status unreadable";
  renderWithQuery(<App />);

  const notice = await screen.findByRole("alert");
  expect(screen.queryByRole("heading", { name: "First-run setup" })).not.toBeInTheDocument();
  expect(notice).toHaveTextContent(/could not read whether setup is finished/i);
  expect(notice).toHaveTextContent("onboarding status unreadable");
});

it("still shows first-run setup when an established onboarding status says setup is incomplete", async () => {
  invoke.mockImplementation(async (command: string) =>
    command === "is_onboarding_complete" ? { complete: false, skipped: false } : null);
  renderWithQuery(<App />);

  expect(await screen.findByRole("heading", { name: "First-run setup" })).toBeVisible();
});

it("says the supported Game registry is unavailable instead of rendering a fabricated tab strip", async () => {
  failing.list_supported_games = "registry unreadable";
  renderWithQuery(<App />);

  const notice = (await screen.findByText(/could not read the list of supported games/i)).closest('[role="alert"]');
  expect(notice).toHaveTextContent("registry unreadable");
  expect(screen.queryByRole("tab")).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: /^Launch / })).not.toBeInTheDocument();
});

it("keeps a Game in the tab strip, marked, when its saved install path cannot be read", async () => {
  failing["get_game_install_path:gimi"] = "gimi path unreadable";
  renderWithQuery(<App />);

  expect(await screen.findByText("Genshin Impact: gimi path unreadable")).toBeVisible();
  await waitFor(() => expect(screen.getAllByRole("tab")).toHaveLength(2));
  const tab = screen.getByRole("tab", { name: /Genshin Impact/ });
  expect(tab).toHaveTextContent(/path unverified/i);
  expect(screen.getByRole("tab", { name: "Honkai: Star Rail" })).not.toHaveTextContent(/unverified/i);
});

it("does not stay silent when the startup link check cannot be read", async () => {
  failing.get_startup_reconcile_status = "reconcile status unreadable";
  renderWithQuery(<App />);

  const panel = await screen.findByRole("region", { name: "Startup game link check" });
  await waitFor(() => expect(panel).toHaveTextContent(/could not confirm/i));
  expect(panel).toHaveTextContent("reconcile status unreadable");
});

it("stays silent on an established read of zero startup link failures", async () => {
  renderWithQuery(<App />);

  await screen.findAllByRole("tab");
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("get_startup_reconcile_status", undefined));
  expect(screen.queryByRole("region", { name: "Startup game link check" })).not.toBeInTheDocument();
});

it("does not claim no proxy password is saved when the Network configuration was not read", async () => {
  failing.get_proxy_config = "proxy config unreadable";
  renderWithQuery(<App />);
  await screen.findAllByRole("tab");
  await clickMenuItem(listen, "settings");

  const dialog = await screen.findByRole("dialog");
  expect(await within(dialog).findByText(/proxy config unreadable/)).toBeVisible();
  expect(within(dialog).queryByPlaceholderText("password")).not.toBeInTheDocument();
  expect(within(dialog).getByPlaceholderText(/saved status unknown/i)).toHaveAttribute("type", "password");
});
