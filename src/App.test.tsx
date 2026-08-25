import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";

import { renderWithQuery } from "./test/harness";

const { invoke, openDialog } = vi.hoisted(() => ({
  invoke: vi.fn(),
  openDialog: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: (...args: unknown[]) => openDialog(...args),
  save: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn() }));
vi.mock("./diagnostics", () => ({
  diagnosticsLogDir: vi.fn().mockResolvedValue("C:\\GMM\\logs"),
  exportDiagnosticsBundle: vi.fn(),
}));
vi.mock("./updater", () => ({
  checkInteractively: vi.fn(),
}));
vi.mock("./ImporterOriginPanel", () => ({ ImporterOriginPanel: () => null }));
vi.mock("./LibraryAuditWarning", () => ({ LibraryAuditWarning: () => null }));
vi.mock("./LoaderVersionNote", () => ({ LoaderVersionNote: () => null }));
vi.mock("./ReinstallRecoveryWarning", () => ({
  ReinstallRecoveryNotices: () => null,
  ReinstallRecoveryWarning: () => null,
}));

const { default: App } = await import("./App");

const variantError =
  'Mod "Broken Outfit" has an invalid active Variant selection. Select a valid Variant for this Mod, or reinstall it.';

let startupFailures: Array<{
  game: "gimi";
  kind: "invalidActiveVariant";
  error: string;
}> = [];
let conflictError: string | null = null;
let relocationFailures: Array<{
  mod_id: string;
  game: "gimi";
  kind: "invalidActiveVariant" | "other";
  error: string;
}> = [];

function ipcResult(command: string) {
  switch (command) {
    case "is_onboarding_complete":
      return { complete: true, skipped: false };
    case "list_supported_games":
      return [{ code: "gimi", displayName: "Genshin Impact" }];
    case "current_session":
    case "clean_stale_session":
    case "get_game_install_path":
    case "fetch_latest_importer_release":
    case "check_importer_update":
      return null;
    case "get_startup_reconcile_status":
      return { finished: true, failures: startupFailures };
    case "get_proxy_config":
      return { url: null, username: null, passwordSet: false };
    case "list_mod_updates":
      return [];
    case "mod_updates_globally_enabled":
      return true;
    case "get_library_paths":
      return {
        defaultRoot: "C:\\GMM\\library",
        rootOverride: null,
        effectiveRoot: "C:\\GMM\\library",
        perGameOverrides: {},
        perGameEffective: { gimi: "C:\\GMM\\library\\gimi" },
      };
    case "check_loader_update":
      return {
        shippedVersion: "0.9.2",
        latestVersion: "0.9.2",
        upstreamAhead: false,
        checkError: null,
      };
    case "list_mods":
      return [];
    case "detect_conflicts":
      if (conflictError) throw new Error(conflictError);
      return { conflicts: [], per_mod_count: {} };
    case "set_library_root":
      return {
        relocated: ["01INTERNALMODID"],
        moved_directories: ["C:\\Moved"],
        failed_junction_restores: relocationFailures,
      };
    default:
      return null;
  }
}

beforeEach(() => {
  vi.clearAllMocks();
  startupFailures = [];
  conflictError = null;
  relocationFailures = [];
  openDialog.mockResolvedValue("C:\\Moved");
  invoke.mockImplementation((command: string) => Promise.resolve(ipcResult(command)));
});

it("shows a conflict-detection failure instead of presenting unavailable data as no conflicts", async () => {
  conflictError = variantError;
  renderWithQuery(<App />);

  await waitFor(() =>
    expect(
      screen.queryByText(/could not check this game's mod conflicts/i),
      "the conflict detection failure must be visible instead of empty conflict data",
    ).toBeInTheDocument(),
  );
  const heading = screen.getByText(/could not check this game's mod conflicts/i);
  const alert = heading.closest('[role="alert"]');
  expect(alert).not.toBeNull();
  expect(alert).toHaveTextContent(/not a “no conflicts” result/i);
  expect(alert).toHaveTextContent("Broken Outfit");
  expect(alert).toHaveTextContent(/Select a valid Variant/i);
  expect(alert).toHaveTextContent(/reinstall it/i);
});

it("shows the affected game's corrupt Variant when startup reconcile aborts", async () => {
  startupFailures = [
    { game: "gimi", kind: "invalidActiveVariant", error: variantError },
  ];
  renderWithQuery(<App />);

  await waitFor(() =>
    expect(
      screen.queryByText(/could not finish checking your game links at startup/i),
      "the startup reconcile failure must be visible after the background pass",
    ).toBeInTheDocument(),
  );
  const heading = screen.getByText(/could not finish checking your game links at startup/i);
  const alert = heading.closest('[role="alert"]');
  expect(alert).not.toBeNull();
  expect(alert).toHaveTextContent(/GIMI/);
  expect(alert).toHaveTextContent("Broken Outfit");
  expect(alert).toHaveTextContent(/Select a valid Variant/i);
  expect(alert).toHaveTextContent(/reinstall it/i);
});

it("routes a relocation Variant failure to selection repair and never recommends rebuild", async () => {
  relocationFailures = [
    {
      mod_id: "01INTERNALMODID",
      game: "gimi",
      kind: "invalidActiveVariant",
      error: variantError,
    },
  ];
  renderWithQuery(<App />);

  await userEvent.click(await screen.findByRole("button", { name: /change global root/i }));

  await waitFor(() =>
    expect(
      screen.queryByText(/library moved, but some mods have an invalid variant selection/i),
      "the relocation failure must recommend Variant repair instead of Junction rebuild",
    ).toBeInTheDocument(),
  );
  const heading = screen.getByText(
    /library moved, but some mods have an invalid variant selection/i,
  );
  const alert = heading.closest('[role="alert"]');
  expect(alert).not.toBeNull();
  expect(alert).toHaveTextContent(/Rebuilding junctions cannot repair/i);
  expect(alert).toHaveTextContent(/Select a valid Variant/i);
  expect(alert).toHaveTextContent(/reinstall it/i);
  expect(alert).not.toHaveTextContent("01INTERNALMODID");
  expect(
    screen.queryByText(/Use “Rebuild junctions” on the game card below/i),
  ).not.toBeInTheDocument();
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("set_library_root", { path: "C:\\Moved" }));
});
