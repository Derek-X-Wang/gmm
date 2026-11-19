import { onlineManager, type QueryClient } from "@tanstack/react-query";
import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { GameCode, GameSummary } from "./api";
import { renderWithQuery } from "./test/harness";
import { clickMenuItem } from "./test/menu";

// Each test makes ONE read fail (a rejected command) and asserts what the
// surface that depends on it must stop claiming. A read that never succeeded
// is the case where there is no value at all to fall back on.
//
// A rejected command leaves `status: "error"`. Going offline with
// `onlineManager.setOnline(false)` leaves a read *paused* instead, which keeps
// `status: "success"`, keeps `data` and sets only `fetchStatus` — so
// `isFetching` is false exactly when a surface most needs to hedge, and a test
// that only fails a refresh never reaches that case. The two helpers below
// drive it: `pause` for an established read, `pauseFirstRead` for one with no
// value at all.

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
let launches: unknown[];

beforeEach(() => {
  paths = { gimi: "C:\\Games\\GIMI", srmi: "C:\\Games\\SRMI" };
  failing = {};
  launches = [];
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
      case "interrupted_session_launches": return launches;
      case "list_mods":
      case "list_mod_updates": return [];
      default: return null;
    }
  });
});

afterEach(() => {
  onlineManager.setOnline(true);
});

/** Pause one established read: go offline, then ask for it again. */
async function pause(client: QueryClient, queryKey: unknown[]) {
  onlineManager.setOnline(false);
  await act(() => client.invalidateQueries({ queryKey }));
}

/**
 * Pause one read with no value at all: go offline, drop what was read, and let
 * the observer's own re-read pause. This is the case `?? value` fallbacks hide,
 * and it is reached without touching the other reads this screen needs.
 */
async function pauseFirstRead(client: QueryClient, queryKey: unknown[]) {
  onlineManager.setOnline(false);
  await act(() => client.resetQueries({ queryKey }));
}

const reservation = {
  id: "launch-1", game: "gimi", child_pid: 4242, started_at: "2026-01-02T03:04:05Z",
};

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

// --- useSessionPresentation::interruptedLaunches -------------------------
// An outstanding launch reservation locks the Library, and only the user's
// explicit confirmation retires one. An unread list used to render exactly
// like a read list with nothing in it: no warnings, no explanation, no route
// to that confirmation.

it("says the launch reservations could not be read instead of showing no interrupted launch", async () => {
  failing.interrupted_session_launches = "launch reservations unreadable";
  renderWithQuery(<App />);

  const notice = await screen.findByRole("region", { name: "Interrupted launch reservations" });
  expect(notice).toHaveTextContent(/could not read whether an interrupted Game launch is outstanding/i);
  expect(notice).toHaveTextContent("launch reservations unreadable");
});

it("says the launch reservations could not be read when their first read is paused", async () => {
  const { client } = renderWithQuery(<App />);
  await screen.findAllByRole("tab");

  await pauseFirstRead(client, ["interruptedSessionLaunches"]);

  const notice = await screen.findByRole("region", { name: "Interrupted launch reservations" });
  expect(notice).toHaveTextContent(/could not read whether an interrupted Game launch is outstanding/i);
  expect(notice).toHaveTextContent(/waiting for a connection/i);
});

it("keeps a known launch reservation and its confirmation reachable when the refresh fails", async () => {
  launches = [reservation];
  const { client } = renderWithQuery(<App />);
  await screen.findByRole("region", { name: "Interrupted GIMI launch" });

  failing.interrupted_session_launches = "launch reservations unreadable";
  await act(() => client.invalidateQueries({ queryKey: ["interruptedSessionLaunches"] }));

  const notice = await screen.findByRole("region", { name: "Interrupted launch reservations" });
  expect(notice).toHaveTextContent(/could not re-read whether an interrupted Game launch is outstanding/i);
  expect(notice).toHaveTextContent("launch reservations unreadable");
  const warning = screen.getByRole("region", { name: "Interrupted GIMI launch" });
  await userEvent.click(within(warning).getByRole("button", { name: /retire reservation/i }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith(
    "retire_interrupted_session_launch", { id: reservation.id },
  ));
});

it("marks a known launch reservation as from the previous read when the refresh is paused", async () => {
  launches = [reservation];
  const { client } = renderWithQuery(<App />);
  await screen.findByRole("region", { name: "Interrupted GIMI launch" });

  await pause(client, ["interruptedSessionLaunches"]);

  const notice = await screen.findByRole("region", { name: "Interrupted launch reservations" });
  expect(notice).toHaveTextContent(/could not re-read whether an interrupted Game launch is outstanding/i);
  expect(notice).toHaveTextContent(/waiting for a connection/i);
  const warning = screen.getByRole("region", { name: "Interrupted GIMI launch" });
  expect(within(warning).getByRole("button", { name: /retire reservation/i })).toBeEnabled();
});

it("says nothing extra about launch reservations on an established, current read", async () => {
  launches = [reservation];
  renderWithQuery(<App />);

  expect(await screen.findByRole("region", { name: "Interrupted GIMI launch" })).toBeVisible();
  expect(screen.queryByRole("region", { name: "Interrupted launch reservations" })).not.toBeInTheDocument();
});

// --- ModUpdatesPanel::global — the weekly Mod check preference -----------
// The checkbox used to read `global.data ?? true`, so an unread preference
// stated that GMM checks weekly for Mod updates, and offered a write derived
// from that invented value.

const weeklyCheck = { name: "Weekly check enabled" };

it("does not state the weekly Mod check is enabled when the preference could not be read", async () => {
  failing.mod_updates_globally_enabled = "weekly check preference unreadable";
  renderWithQuery(<App />);

  const notice = await screen.findByText(/could not read whether the weekly Mod check is enabled/i);
  expect(notice).toBeVisible();
  expect(screen.getByText("weekly check preference unreadable")).toBeVisible();
  expect(screen.queryByRole("checkbox", weeklyCheck)).not.toBeInTheDocument();
});

it("does not state the weekly Mod check is enabled when the first read of the preference is paused", async () => {
  const { client } = renderWithQuery(<App />);
  expect(await screen.findByRole("checkbox", weeklyCheck)).toBeChecked();

  await pauseFirstRead(client, ["modUpdates", "globalEnabled"]);

  const notice = await screen.findByText(/could not read whether the weekly Mod check is enabled/i);
  expect(notice).toHaveTextContent(/waiting for a connection/i);
  expect(screen.queryByRole("checkbox", weeklyCheck)).not.toBeInTheDocument();
});

it("stops offering a write of the weekly Mod check when its refresh fails", async () => {
  const { client } = renderWithQuery(<App />);
  expect(await screen.findByRole("checkbox", weeklyCheck)).toBeEnabled();

  failing.mod_updates_globally_enabled = "weekly check preference unreadable";
  await act(() => client.invalidateQueries({ queryKey: ["modUpdates", "globalEnabled"] }));

  await waitFor(() => expect(screen.getByRole("checkbox", weeklyCheck)).toBeDisabled());
  // The value a read did establish stays shown; it is now marked as not re-read.
  expect(screen.getByRole("checkbox", weeklyCheck)).toBeChecked();
  expect(screen.getByText(/could not re-read whether the weekly Mod check is enabled/i)).toBeVisible();
  expect(screen.getByText("weekly check preference unreadable")).toBeVisible();
});

it("stops offering a write of the weekly Mod check when its refresh is paused", async () => {
  const { client } = renderWithQuery(<App />);
  expect(await screen.findByRole("checkbox", weeklyCheck)).toBeEnabled();

  await pause(client, ["modUpdates", "globalEnabled"]);

  await waitFor(() => expect(screen.getByRole("checkbox", weeklyCheck)).toBeDisabled());
  expect(screen.getByRole("checkbox", weeklyCheck)).toBeChecked();
  const notice = screen.getByText(/could not re-read whether the weekly Mod check is enabled/i);
  expect(notice).toHaveTextContent(/waiting for a connection/i);
});

it("leaves the weekly Mod check checkbox usable on an established, current read", async () => {
  renderWithQuery(<App />);

  const toggle = await screen.findByRole("checkbox", weeklyCheck);
  expect(toggle).toBeChecked();
  expect(toggle).toBeEnabled();
  expect(screen.queryByText(/whether the weekly Mod check is enabled/i)).not.toBeInTheDocument();
  await userEvent.click(toggle);
  await waitFor(() => expect(invoke).toHaveBeenCalledWith(
    "set_mod_updates_globally_enabled", { enabled: false },
  ));
});
