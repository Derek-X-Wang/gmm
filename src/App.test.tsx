// @ts-expect-error Node types are not part of the frontend; Vitest provides this test-only runtime.
import { readFileSync } from "node:fs";

import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";

import { renderWithQuery } from "./test/harness";
import { clickMenuItem } from "./test/menu";

const { invoke, openDialog, listen } = vi.hoisted(() => ({
  invoke: vi.fn(),
  openDialog: vi.fn(),
  listen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen }));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: (...args: unknown[]) => openDialog(...args),
  save: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn() }));
vi.mock("./diagnostics", () => ({
  diagnosticsLogDir: vi.fn().mockResolvedValue("C:\\GMM\\logs"),
  exportDiagnosticsBundle: vi.fn(),
}));
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
let libraryPathsError: {
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
let overlapRepair = false;
let stagedAttention = false;
const cleanAttentionReport = {
  safeToProceed: true,
  reinstalls: [],
  enabledTransitions: [],
  importerEvacuations: [],
  stagedLibraryOperations: [],
  sessionLaunches: [],
  activeSession: null,
  libraryAudits: [],
  libraryRootOverlaps: [],
  modPathOverlaps: [],
};

function ipcResult(command: string) {
  switch (command) {
    case "attention_status":
      return stagedAttention ? {
        ...cleanAttentionReport,
        safeToProceed: false,
        stagedLibraryOperations: [{
          id: "01STAGED", game: "srmi", operation: "import_zip",
          stagedPath: "C:\\GMM\\library\\srmi\\01STAGED", recoveryError: null,
        }],
      } : cleanAttentionReport;
    case "is_onboarding_complete":
      return { complete: true, skipped: false };
    case "list_supported_games":
      return [{ code: "gimi", displayName: "Genshin Impact" }];
    case "current_session":
    case "clean_stale_session":
      return null;
    case "get_game_install_path":
      return "C:/Game";
    case "get_game_path_status":
      return { game: "gimi", savedPath: "C:/Game", gamePath: { status: "readable" }, modelImporter: { status: "installed", record: { installedVersion: "1.0", origin: { state: "unknown" } } } };
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
      if (libraryPathsError) throw libraryPathsError;
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
        overlap_repair: overlapRepair,
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
  libraryPathsError = null;
  importerUpdate = {
    available: false,
    installedVersion: null,
    latestVersion: null,
    pinned: false,
    upstreamAhead: false,
    checkError: null,
  };
  relocationFailures = [];
  overlapRepair = false;
  stagedAttention = false;
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
  await userEvent.click(await screen.findByRole("button", { name: "Settings for Genshin Impact" }));

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
  await userEvent.click(await screen.findByRole("button", { name: "Settings for Genshin Impact" }));

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

it("shows a Library path failure instead of resolving forever", async () => {
  libraryPathsError = {
    kind: "invalidActiveVariant",
    message: "Could not read the configured Library paths.",
  };
  renderWithQuery(<App />);

  await clickMenuItem(listen, "settings");
  const heading = await screen.findByText(/could not resolve Library paths/i);
  const alert = heading.closest('[role="alert"]');
  expect(alert).not.toBeNull();
  expect(alert).toHaveAttribute(
    "data-command-failure-kind",
    "invalidActiveVariant",
  );
  expect(alert).toHaveTextContent(libraryPathsError.message);
  expect(screen.queryByText(/resolving paths/i)).not.toBeInTheDocument();
});

it("shows a latest-importer-release failure instead of calling it unavailable", async () => {
  importerReleaseError = {
    kind: "invalidActiveVariant",
    message: "The importer release lookup failed.",
  };
  renderWithQuery(<App />);
  await userEvent.click(await screen.findByRole("button", { name: "Settings for Genshin Impact" }));

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
  expect(alert).toHaveTextContent("Genshin Impact");
  expect(alert).not.toHaveTextContent(/Junctions (?:were )?repaired|successfully repaired/i);
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

  await clickMenuItem(listen, "settings");
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
  const library = screen.getByRole("region", { name: "Global Library root settings" });
  expect(within(library).getAllByRole("listitem")).toEqual(
    within(within(library).getByRole("alert")).getAllByRole("listitem"),
  );
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("set_library_root", { path: "C:\\Moved" }));
});

it("reports a setting-only overlap repair instead of presenting it as a successful move", async () => {
  overlapRepair = true;
  renderWithQuery(<App />);

  await clickMenuItem(listen, "settings");
  const changeRoot = await screen.findByRole("button", { name: /change global root/i });
  expect(
    screen.queryByText(/library setting changed, but existing mod folders were not moved/i),
    "setting-only repair feedback must not be shown before the repair finishes",
  ).not.toBeInTheDocument();
  await userEvent.click(changeRoot);

  const message = await screen.findByText(
    /library setting changed, but existing mod folders were not moved/i,
  );
  const alert = message.closest('[role="alert"]');
  expect(
    alert,
    "a setting-only overlap repair must produce visible action feedback",
  ).not.toBeNull();
  expect(alert).toHaveTextContent(/did not read the overlapping path/i);
  expect(alert).toHaveTextContent(/warning remains while any Mod is still recorded there/i);
});

it("shows the clean attention aggregate in the main app", async () => {
  renderWithQuery(<App />);
  const indicator = await screen.findByRole("button", { name: /No attention needed.*All Games/ });
  expect(indicator).toHaveAttribute("aria-expanded", "false");
  await userEvent.click(indicator);

  await waitFor(() => expect(
    screen.queryByText("Nothing needs attention in this report."),
    "the app must show the clean aggregate instead of hiding it",
  ).toBeInTheDocument());
  expect(invoke).toHaveBeenCalledWith("attention_status", undefined);
  expect(screen.queryByRole("region", { name: "Startup game link check" })).not.toBeInTheDocument();
});

it("keeps unrelated Game controls available when the aggregate needs attention", async () => {
  stagedAttention = true;
  renderWithQuery(<App />);
  const indicator = await screen.findByRole("button", { name: /Needs attention.*All Games/ });
  expect(indicator).toHaveAttribute("aria-expanded", "false");
  await userEvent.click(indicator);
  await screen.findByText("Staged Library operations (1)");
  expect(screen.getByRole("button", { name: "Launch Genshin Impact" })).toBeEnabled();
  expect(screen.getByRole("button", { name: "Import ZIP…" })).toBeEnabled();
});


it("renders the approved six main-screen slots in order with scoped notices and no configuration", async () => {
  startupFailures = [{ game: "gimi", kind: "invalidActiveVariant", error: variantError }];
  const implementation = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (command, args) => {
    if (command === "is_onboarding_complete") return { complete: true, skipped: true };
    if (command === "current_session") return { game: "srmi", pid: 123, started_at: "2026-10-11" };
    if (command === "interrupted_session_launches") return [{ id: "claim", game: "gimi", child_pid: null, started_at: "2026-10-11" }];
    if (command === "get_importer_evacuation_recovery") return { reason: "Importer backup locked", gamePath: "C:/Game", backupPath: "C:/Backup", ownerUncertain: false, action: "retry", attempts: 1, attemptedAt: "2026-10-11" };
    return implementation(command, args);
  });
  renderWithQuery(<App />);
  await screen.findByText("Importer backup locked");
  const main = screen.getByRole("main");
  const header = screen.getByRole("region", { name: "Genshin Impact header" });
  const global = screen.getByRole("region", { name: "Global status" });
  const notices = screen.getByRole("region", { name: "Game and operation notices" });
  // Six since #352: the window bar went in #351 — its wordmark duplicated the
  // OS title bar and both its actions are native menu items — and the
  // per-Game status is no longer a slot in the page at all. It is window
  // chrome now, pinned to the bottom in the `contentinfo` bar below.
  expect(Array.from(main.children)).toEqual([
    header,
    global,
    screen.getByRole("tablist", { name: "Game" }).parentElement!.parentElement,
    notices,
    screen.getByRole("heading", { name: "Add Mods for Genshin Impact" }).closest("section"),
    screen.getByRole("heading", { name: "Mod updates" }).closest("section"),
  ]);
  const statusBar = screen.getByRole("contentinfo");
  const status = screen.getByRole("region", { name: "Genshin Impact status" });
  expect(main).not.toContainElement(status);
  expect(statusBar).toContainElement(status);
  expect(main.nextElementSibling).toBe(statusBar);
  expect(within(header).getByRole("heading", { level: 1 })).toHaveTextContent("Genshin Impact");
  expect(within(header).getByRole("button", { name: "Game running…" })).toBeDisabled();
  expect(within(global).getByRole("region", { name: "Attention across all Games" })).toBeVisible();
  expect(global).toHaveTextContent("SRMI is running");
  expect(within(notices).getByRole("button", { name: "Resume setup" })).toBeEnabled();
  expect(within(notices).getByRole("button", { name: "Resume setup" })).toBeVisible();
  expect(within(notices).getByRole("button", { name: "Resume setup" }).closest("details")).toBeNull();
  expect(within(notices).getByRole("region", { name: "Interrupted GIMI launch" })).toBeVisible();
  expect(notices).toHaveTextContent("Genshin Impact (GIMI):");
  expect(notices).toHaveTextContent(variantError);
  expect(notices).not.toHaveTextContent(/Junctions (?:were )?repaired/i);
  expect(within(notices).getByRole("button", { name: "Review Genshin Impact recovery in Settings" })).toBeEnabled();
  expect(screen.getByText("Your Library has no Mods for Genshin Impact. Choose how to add your first Mod.")).toBeVisible();
  expect(screen.getByRole("heading", { name: "Add Mods for Genshin Impact" }).closest("section")!.querySelector("ul.mods")).toBeNull();
  expect(screen.getAllByRole("heading", { level: 3 }).map((heading) => heading.textContent)).toEqual([
    "Adopt for Genshin Impact", "Import ZIP for Genshin Impact", "GameBanana for Genshin Impact",
  ]);
  expect(main.querySelector("aside")).toBeNull();
  expect(main.querySelector("header.window-bar")).toBeNull();
  expect(screen.queryByRole("banner", { name: "GMM" })).not.toBeInTheDocument();
  expect(screen.queryByText("GMM")).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Settings" })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Run setup again" })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Retry Model Importer recovery" })).not.toBeInTheDocument();
  for (const name of ["Network", "GMM updates", "Importer recommendations", "Diagnostics", "Global Library root", "Genshin Impact install path", "Model Importer (GIMI)"]) {
    expect(screen.queryByRole("heading", { name })).not.toBeInTheDocument();
  }
  await userEvent.click(within(notices).getByRole("button", { name: "Review Genshin Impact recovery in Settings" }));
  expect(await screen.findByRole("button", { name: "Retry Model Importer recovery" })).toBeVisible();
});

// The status bar (#352). jsdom computes no layout: `getBoundingClientRect`
// returns zeros and `position: fixed` is not implemented, so nothing below
// proves the rendered geometry. What these tests do prove is everything
// decidable without a layout engine — where the bar sits in the document,
// what it is allowed to contain, and that the pinning declarations reach it
// from the authored stylesheet. The rendered result at 1100x760 and at the
// 900x600 minimum (#351) is a Windows check, named in the PR.

/** Run `body` with the authored stylesheet in the document. */
async function withAppCss(body: () => Promise<void>): Promise<void> {
  const style = document.createElement("style");
  style.textContent = readFileSync("src/App.css", "utf8");
  document.head.appendChild(style);
  try {
    await body();
  } finally {
    style.remove();
  }
}

it("pins the per-Game status to the bottom of the window instead of the page flow", async () => {
  await withAppCss(async () => {
    renderWithQuery(<App />);
    const bar = await screen.findByRole("contentinfo");
    const main = screen.getByRole("main");

    // Outside `<main>`, after it, and holding the strip: a band left at the
    // end of the page flow fails here before any style is consulted.
    expect(bar.tagName).toBe("FOOTER");
    expect(main.nextElementSibling).toBe(bar);
    expect(bar).toContainElement(screen.getByRole("region", { name: "Genshin Impact status" }));

    const pinned = getComputedStyle(bar);
    expect(
      [pinned.position, pinned.bottom, pinned.left, pinned.right],
      "the bar is fixed to the viewport, not laid out in the page",
    ).toEqual(["fixed", "0px", "0px", "0px"]);
    // Fixed over scrolling content, so the surface has to be opaque: a
    // transparent bar renders the Mod list through its own text.
    expect(pinned.backgroundColor).toMatch(/^rgb\([^)]*\)$/);
    expect(pinned.borderTopWidth).not.toBe("0px");

    // The layout the bar is aligned to: `.app` fills the window and is not
    // re-centred (#367). This needs its own assertion, because the gutter
    // below stays correct whether or not the page caps its width — so a
    // returning cap is invisible there unless whoever re-adds it also re-adds
    // the old `calc()`, which a regression would not necessarily do.
    const page = getComputedStyle(main);
    expect(
      { cap: page.maxWidth, centred: [page.marginLeft, page.marginRight].includes("auto") },
      "the content layer fills the window: a `max-width` or an `auto` inline margin centres a column again",
    ).toEqual({ cap: "none", centred: false });

    // The bar spans the window and lines up with that page content on `.app`'s
    // own 1.5rem inline gutter. It used to add `max(0px, (100% - 880px) / 2)`
    // to re-centre itself on a column it was not inside; with the column gone,
    // the gutter is simply the page's.
    expect(
      pinned.paddingInline,
      "the bar's inline padding is the page's gutter, not a gutter computed from a page width",
    ).toBe("1.5rem");

    // The clearance invariant: `.app`'s bottom padding must be at least as
    // tall as the bar can grow, or the last card ends up underneath it.
    const tallest = parseFloat(pinned.maxHeight);
    expect(tallest).toBeGreaterThan(0);
    expect(
      parseFloat(getComputedStyle(main).paddingBottom),
      "`.app`'s bottom padding is the fixed bar's scroll clearance",
    ).toBeGreaterThanOrEqual(tallest);
  });
});

it("keeps the status bar's only control the Settings link", async () => {
  renderWithQuery(<App />);
  const bar = await screen.findByRole("contentinfo");
  await within(bar).findByText("Game: Game readable · Model Importer healthy");

  // The specification forbids a path editor, Install/Update, rollback,
  // Importer Pin control, Origin editor and Library mutation control here.
  // Enumerating every interactive descendant catches all of them, and any
  // control nobody has thought of yet, instead of naming them one by one.
  const interactive = Array.from(bar.querySelectorAll([
    "a", "button", "input", "select", "textarea", "summary", "label",
    "[role=button]", "[role=link]", "[role=checkbox]", "[role=switch]",
    "[role=menuitem]", "[role=tab]", "[tabindex]", "[contenteditable]",
  ].join(", ")));
  expect(interactive.map((element) => [element.tagName, element.textContent])).toEqual([
    ["BUTTON", "Settings for Genshin Impact"],
  ]);
  // Load-bearing for focus return: the arrival marker is what the return
  // chain resolves after the Settings dialog closes.
  expect(interactive[0]).toHaveAttribute("data-settings-arrival", "game");
});

it("announces the status bar once, through the strip's own live region", async () => {
  renderWithQuery(<App />);
  const bar = await screen.findByRole("contentinfo");
  await within(bar).findByText("Game: Game readable · Model Importer healthy");

  // The landmark is the `<footer>` itself; the strip's summary is the single
  // live region. A live region on the bar as well would announce every status
  // change twice.
  expect(bar).not.toHaveAttribute("aria-live");
  expect(bar).not.toHaveAttribute("aria-atomic");
  const live = Array.from(bar.querySelectorAll("[aria-live], [role=status], [role=alert], [role=log]"));
  expect(live.map((element) => element.getAttribute("role"))).toEqual(["status"]);
  expect(within(bar).getByRole("status")).toHaveTextContent("Game and Model Importer (GIMI)");
});

it("keeps #348's warning sign and wording for missing Game entries in the bar", async () => {
  const missing = ["C:/Game/GenshinImpact.exe", "C:/Game/GenshinImpact_Data"];
  const implementation = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (command, args) => {
    if (command === "get_game_path_status") return {
      game: "gimi",
      savedPath: "C:/Game",
      gamePath: { status: "requiredEntriesNotFound", paths: missing },
      modelImporter: { status: "unverified", record: null, path: null, error: "Game layout unverified" },
    };
    return implementation(command, args);
  });
  renderWithQuery(<App />);
  const bar = await screen.findByRole("contentinfo");

  expect(await within(bar).findByText(
    "Game: Required Game files or directories not found · Model Importer unverified",
  )).toBeVisible();
  // Exact sign and exact words, so a different glyph, a different label or
  // different wording fails rather than only an absent one.
  expect(within(bar).getByRole("img", { name: "Warning" })).toHaveTextContent(/^⚠$/);
  expect(within(bar).getByText("Missing Game entries", { exact: true })).toBeVisible();
  for (const path of missing) expect(bar).not.toHaveTextContent(path);

  // No fact disappears without a route to it: the bar's link opens that
  // Game's Settings page, where the missing paths remain.
  await userEvent.click(within(bar).getByRole("button", { name: "Settings for Genshin Impact" }));
  expect(await screen.findByRole("textbox", { name: "Genshin Impact install path" })).toBeVisible();
});

it("declares only the dark colour scheme for native widgets", () => {
  const authored: string = readFileSync("src/App.css", "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
  const root = authored.match(/:root\s*\{([^{}]*)\}/)![1];
  expect(root.match(/(?:^|;)\s*color-scheme\s*:\s*([^;]+)/)?.[1].trim()).toBe("dark");
});

it("forbids OS colour-scheme overrides throughout the stylesheet", () => {
  const authored: string = readFileSync("src/App.css", "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
  expect(authored, "GMM uses one dark palette regardless of the OS theme").not.toMatch(/prefers-color-scheme/i);
});

it("gives the status bar one opaque dark surface and readable inherited text", () => {
  // Read authored colours: jsdom does not evaluate theme media queries and
  // can resolve `inherit` against body instead of the actual parent.
  const authored: string = readFileSync("src/App.css", "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
  const bar = document.createElement("footer");
  bar.className = "status-bar";
  const other = document.createElement("footer");
  const rules = [...authored.matchAll(/([^{}]+)\{([^{}]*)\}/g)]
    .filter(([, selectors]) => selectors.split(",").some((raw) => {
      const selector = raw.trim();
      return bar.matches(selector) && !other.matches(selector);
    }));
  expect(rules, "one unconditional dark rule owns the bar's surface").toHaveLength(1);
  expect(rules[0][2]).toMatch(/(?:^|;)\s*background\s*:\s*#18181b\s*;/);
  expect(rules[0][2]).toMatch(/(?:^|;)\s*border-top\s*:\s*1px solid #2e2e36\s*;/);
  expect(rules[0][2]).not.toMatch(/(?:^|;)\s*color\s*:/);
  const root = authored.match(/:root\s*\{([^{}]*)\}/)![1];
  expect(root).toMatch(/(?:^|;)\s*color\s*:\s*#e5e5e5\s*;/);
});
