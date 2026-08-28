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
let conflictError: {
  kind: "invalidActiveVariant";
  message: string;
} | null = null;
let importerReleaseError: {
  kind: "invalidActiveVariant";
  message: string;
} | null = null;
let launchError: {
  kind: "invalidActiveVariant";
  message: string;
} | null = null;
let importerPinError: {
  kind: "invalidActiveVariant";
  message: string;
} | null = null;
let importerUpdateError: {
  kind: "invalidActiveVariant";
  message: string;
} | null = null;
let importerUpdate = {
  available: false,
  installedVersion: null as string | null,
  latestVersion: null as string | null,
  pinned: false,
  upstreamAhead: false,
  checkError: null as string | null,
};
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
      return null;
    case "check_importer_update":
      if (importerUpdateError) throw importerUpdateError;
      return importerUpdate;
    case "launch_game":
      if (launchError) throw launchError;
      return null;
    case "av_guidance":
      return {
        headline: "Antivirus software may be blocking the launch",
        body: "Check your antivirus quarantine before trying again.",
        exclusionSteps: ["Restore the blocked file."],
        docPath: "docs/antivirus-and-smartscreen.md",
        sentinel: "AV-PATTERN: ",
      };
    case "set_importer_pinned":
      if (importerPinError) throw importerPinError;
      return null;
    case "fetch_latest_importer_release":
      if (importerReleaseError) throw importerReleaseError;
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
      if (conflictError) throw conflictError;
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
  importerReleaseError = null;
  launchError = null;
  importerPinError = null;
  importerUpdateError = null;
  importerUpdate = {
    available: false,
    installedVersion: null,
    latestVersion: null,
    pinned: false,
    upstreamAhead: false,
    checkError: null,
  };
  relocationFailures = [];
  openDialog.mockResolvedValue("C:\\Moved");
  invoke.mockImplementation((command: string) => Promise.resolve(ipcResult(command)));
});

it("preserves a non-AV launch failure's kind through the shared renderer", async () => {
  launchError = {
    kind: "invalidActiveVariant",
    message: "The selected Mod variant is unavailable.",
  };
  renderWithQuery(<App />);

  await userEvent.click(
    await screen.findByRole("button", { name: /Launch Genshin Impact/i }),
  );

  const message = await screen.findByText(launchError.message);
  expect(message).toHaveAttribute(
    "data-command-failure-kind",
    "invalidActiveVariant",
  );
});

it("preserves an AV-pattern launch failure's kind through the guidance renderer", async () => {
  launchError = {
    kind: "invalidActiveVariant",
    message: "AV-PATTERN: The selected Mod variant is unavailable.",
  };
  renderWithQuery(<App />);

  await userEvent.click(
    await screen.findByRole("button", { name: /Launch Genshin Impact/i }),
  );

  const headline = await screen.findByText(
    "Antivirus software may be blocking the launch",
  );
  const alert = headline.closest('[role="alert"]');
  expect(alert).not.toBeNull();
  expect(
    alert,
    "the AV guidance renderer must preserve the launch failure classification",
  ).toHaveAttribute("data-command-failure-kind", "invalidActiveVariant");
  expect(alert).toHaveTextContent("The selected Mod variant is unavailable.");
  expect(alert).not.toHaveTextContent("AV-PATTERN:");
});

it("renders a structured failure when changing an Importer Pin fails", async () => {
  importerUpdate = {
    available: false,
    installedVersion: "v8.8.9",
    latestVersion: "v8.8.9",
    pinned: false,
    upstreamAhead: false,
    checkError: null,
  };
  importerPinError = {
    kind: "invalidActiveVariant",
    message: "Could not save the Importer Pin.",
  };
  renderWithQuery(<App />);

  await userEvent.click(
    await screen.findByRole("button", { name: /Pin to current/i }),
  );

  const message = await screen.findByText(importerPinError.message);
  expect(message).toHaveAttribute(
    "data-command-failure-kind",
    "invalidActiveVariant",
  );
});

it("renders a structured failure when the Importer update query rejects", async () => {
  importerUpdateError = {
    kind: "invalidActiveVariant",
    message: "Could not read Importer update state.",
  };
  renderWithQuery(<App />);

  const heading = await screen.findByText(
    /could not check for a Model Importer update/i,
  );
  const alert = heading.closest('[role="alert"]');
  expect(alert).not.toBeNull();
  expect(alert).toHaveAttribute(
    "data-command-failure-kind",
    "invalidActiveVariant",
  );
  expect(alert).toHaveTextContent(importerUpdateError.message);
});

it("shows a latest-importer-release failure instead of calling it unavailable", async () => {
  importerReleaseError = {
    kind: "invalidActiveVariant",
    message: "The importer release lookup failed.",
  };
  renderWithQuery(<App />);

  const heading = await screen.findByText(
    /could not check the latest Model Importer release/i,
  );
  const alert = heading.closest('[role="alert"]');
  expect(alert).not.toBeNull();
  expect(alert).toHaveAttribute(
    "data-command-failure-kind",
    "invalidActiveVariant",
  );
  expect(alert).toHaveTextContent("The importer release lookup failed.");
  expect(screen.queryByText("unavailable")).not.toBeInTheDocument();
});

it("shows a conflict-detection failure instead of presenting unavailable data as no conflicts", async () => {
  conflictError = { kind: "invalidActiveVariant", message: variantError };
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
  expect(alert).toHaveAttribute("data-command-failure-kind", "invalidActiveVariant");
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
