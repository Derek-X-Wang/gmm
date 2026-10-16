// @ts-expect-error Node types are not part of the frontend; Vitest provides this test-only runtime.
import { readFileSync } from "node:fs";
import { act, fireEvent, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import { renderWithQuery } from "./test/harness";
import { memorySkipStorage, signedMetadata } from "./test/updaterStorage";
import { createUpdateController, type UpdateController } from "./updater";
import type { GameCode, GameSummary } from "./api";

const { invoke, listen, relaunch } = vi.hoisted(() => ({
  invoke: vi.fn(), listen: vi.fn(), relaunch: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch }));
// The real controller is the thing under test here, so only the Tauri call it
// makes is replaced. `check` is supplied per test instead.
vi.mock("@tauri-apps/plugin-updater", () => ({ check: vi.fn() }));

/**
 * One controller per test, behind a facade (#357).
 *
 * `updater` is a module singleton that the status bar and the Settings group
 * share, which is the property this file exercises: both surfaces must be
 * looking at the same offer. A singleton also carries its state between
 * tests, and `checkForUpdates` deliberately refuses to run once a download or
 * an install is in flight — so a test that installed would decide what the
 * next test could reach. The facade keeps the shared-instance property and
 * gives each test a fresh state.
 */
let live: UpdateController;
vi.mock("./updater", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./updater")>();
  return { ...actual, updater: {
    getSnapshot: () => live.getSnapshot(),
    getLastError: () => live.getLastError(),
    subscribe: (listener: () => void) => live.subscribe(listener),
    checkForUpdates: (background?: boolean) => live.checkForUpdates(background),
    install: () => live.install(),
    decline: () => live.decline(),
  } satisfies UpdateController };
});
vi.mock("./LoaderVersionNote", () => ({ LoaderVersionNote: () => null }));
const { default: App } = await import("./App");

const games: GameSummary[] = [
  { code: "gimi", displayName: "Genshin Impact" },
  { code: "srmi", displayName: "Honkai: Star Rail" },
];
let paths: Partial<Record<GameCode, string>>;
let unreadable: string | null;
let downloadAndInstall: ReturnType<typeof vi.fn>;

/** Reach the `available` state the way a launch does: a background check. */
async function offerUpdate(version = "0.5.0") {
  await act(() => live.checkForUpdates(true));
  expect(live.getSnapshot()).toMatchObject({ kind: "available", version });
}
const statusBar = () => document.querySelector<HTMLElement>("footer.status-bar");
const reading = () => screen.getByText(/^GMM update/);
const route = (options?: { hidden: boolean }) =>
  screen.getByRole("button", { name: "Review GMM updates in Settings", ...options });

beforeEach(() => {
  paths = { gimi: "C:\\Genshin", srmi: "C:\\StarRail" };
  unreadable = null;
  relaunch.mockReset().mockResolvedValue(undefined);
  downloadAndInstall = vi.fn().mockResolvedValue(undefined);
  live = createUpdateController(
    vi.fn().mockResolvedValue({ version: "0.5.0", rawJson: signedMetadata("0.5.0"), downloadAndInstall }),
    memorySkipStorage(),
  );
  vi.stubGlobal("scrollTo", vi.fn());
  listen.mockResolvedValue(() => {});
  invoke.mockImplementation(async (command: string, args?: { game: GameCode }) => {
    switch (command) {
      case "is_onboarding_complete": return { complete: true, skipped: false };
      case "list_supported_games": return games;
      case "get_game_install_path": return paths[args!.game] ?? null;
      case "get_game_path_status": return {
        game: args!.game, savedPath: paths[args!.game] ?? null,
        gamePath: unreadable !== null && paths[args!.game]
          ? { status: "unreadable", path: paths[args!.game], error: unreadable }
          : { status: paths[args!.game] ? "readable" : "noSavedPath" },
        modelImporter: paths[args!.game] && unreadable === null
          ? { status: "installed" }
          : { status: "unverified", path: null, error: "Not checked" },
      };
      case "get_proxy_config": return { url: null, username: null, passwordSet: false };
      case "attention_status": return { safeToProceed: true, reinstalls: [], enabledTransitions: [], importerEvacuations: [], stagedLibraryOperations: [], sessionLaunches: [], activeSession: null, libraryAudits: [], libraryRootOverlaps: [], modPathOverlaps: [] };
      case "get_startup_reconcile_status": return { finished: true, failures: [] };
      case "check_importer_update": return { available: false, installedVersion: "1.2.3", pinned: false };
      case "get_library_paths": return { defaultRoot: "C:\\GMM\\library", effectiveRoot: "C:\\GMM\\library", rootOverride: null, perGameOverrides: {}, perGameEffective: {} };
      case "current_session":
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
it("reads an available update in the status bar, after the Game facts, and routes to GMM updates", async () => {
  renderWithQuery(<App />);
  const bar = await screen.findByRole("contentinfo");
  await within(bar).findByText("Game: Game readable · Model Importer healthy");
  expect(within(bar).queryByText(/^GMM update/)).not.toBeInTheDocument();

  await offerUpdate();

  // In the bar, not floating over it, and after the Game facts: their
  // decided payload and wording keep the first row (#285, #324, #348).
  expect(bar).toContainElement(reading());
  expect(screen.getByRole("main")).not.toContainElement(reading());
  expect(reading()).toHaveTextContent("GMM update available: 0.5.0");
  expect(Array.from(bar.children).map((child) => child.className))
    .toEqual(["game-status-strip", "status-bar__update"]);
  expect(within(bar).getByText("Game: Game readable · Model Importer healthy")).toBeVisible();
  expect(within(bar).getByRole("button", { name: "Settings for Genshin Impact" })).toBeVisible();

  // The route reaches the group that already owns the install decision, and
  // the decision is still a click of its own there (#346, ADR 0004).
  expect(downloadAndInstall).not.toHaveBeenCalled();
  await userEvent.click(route());
  const dialog = screen.getByRole("dialog", { name: "Settings" });
  expect(within(dialog).getByRole("heading", { name: "GMM updates" })).toBeVisible();
  expect(within(dialog).getByText(/is available/)).toHaveTextContent("0.5.0");
  expect(downloadAndInstall).not.toHaveBeenCalled();
  await userEvent.click(within(dialog).getByRole("button", { name: "Install update" }));
  expect(downloadAndInstall).toHaveBeenCalledOnce();
});

it("returns focus to the bar's update route after the dialog closes", async () => {
  renderWithQuery(<App />);
  await screen.findByRole("contentinfo");
  await offerUpdate();

  await userEvent.click(route());
  expect(screen.getByRole("dialog", { name: "Settings" })).toBeVisible();
  fireEvent.keyDown(window, { key: "Escape" });

  // The arrival marker on the reading's control is what the return chain
  // resolves, so the user lands back where they left rather than on <body>.
  expect(route()).toHaveFocus();
  expect(document.body).not.toHaveFocus();
});

it("keeps every update surface inside the window the dialog is drawn over", async () => {
  const { container } = renderWithQuery(<App />);
  await screen.findByRole("contentinfo");
  await offerUpdate();

  // Nothing floats. The render root holds the Settings scrim, the main
  // screen and the bar — a fixed-position card outside all three, which is
  // what #357 removed, would appear here as a fourth child.
  expect(Array.from(container.children).map((child) => child.className))
    .toEqual(["settings-screen", "app", "status-bar"]);
  expect(container.querySelectorAll('[style*="position"], [style*="z-index"], [style*="zIndex"]')).toHaveLength(0);

  await userEvent.click(screen.getByRole("button", { name: "Settings for Genshin Impact" }));

  // With the dialog open the reading is still in the bar, and the bar is
  // inert with the main screen: it cannot be clicked while the screen behind
  // the dialog is inert, and it is out of the accessibility tree with it, so
  // it is not reachable by mouse while unreachable by keyboard.
  expect(statusBar()).toContainElement(screen.getByText(/^GMM update/));
  expect(statusBar()).toHaveAttribute("inert");
  expect(statusBar()).toHaveAttribute("aria-hidden", "true");
  expect(screen.queryByRole("button", { name: "Review GMM updates in Settings" })).not.toBeInTheDocument();
  expect(route({ hidden: true }).closest("[inert]")).toBe(statusBar());
  // The dialog is the only surface with update controls while it is open.
  const dialog = screen.getByRole("dialog", { name: "Settings" });
  expect(dialog.closest("[inert]")).toBeNull();
  expect(statusBar()).not.toContainElement(dialog);
});

/**
 * jsdom computes no layout, so nothing here proves the rendered height. What
 * it does prove is the authored bound: the bar keeps a `max-height` with its
 * own scroll, and `.app`'s bottom padding is still sized for the whole of it,
 * with the new reading present beside a long wrapped Game status. The
 * rendered result at the 900x600 minimum is a Windows check, named in the PR.
 */
it("bounds the bar with an update reading beside a long Game status instead of growing", async () => {
  unreadable = "Access is denied. (os error 5) while reading "
    + "C:\\Program Files\\Genshin Impact\\Genshin Impact game\\GenshinImpact_Data\\Persistent";
  const style = document.createElement("style");
  style.textContent = readFileSync("src/App.css", "utf8");
  document.head.appendChild(style);
  try {
    renderWithQuery(<App />);
    const bar = await screen.findByRole("contentinfo");
    await within(bar).findByText("Game: Cannot tell — install path unreadable · Model Importer unverified");
    await offerUpdate();
    expect(bar).toContainElement(reading());
    expect(bar).toHaveTextContent(unreadable);

    const pinned = getComputedStyle(bar);
    const tallest = parseFloat(pinned.maxHeight);
    // A bounded height, and the bar's own scroll for what does not fit: the
    // reading adds a row, and the bar must scroll rather than grow over the
    // page it is pinned in front of.
    expect(tallest, "the bar needs a bounded height, or a long status grows it over the page").toBeGreaterThan(0);
    expect(["auto", "scroll"], "what does not fit scrolls inside the bar").toContain(pinned.overflowY);
    // And the clearance still covers the whole of that height, so the last
    // card cannot end up underneath the bar.
    expect(
      parseFloat(getComputedStyle(screen.getByRole("main")).paddingBottom),
      "`.app`'s bottom padding is the fixed bar's scroll clearance",
    ).toBeGreaterThanOrEqual(tallest);
  } finally {
    style.remove();
  }
});

it("still reads an available update when no Game is configured", async () => {
  paths = {};
  renderWithQuery(<App />);
  await screen.findByRole("button", { name: "Add game" });
  // No Game, so no Game facts and no bar at all until there is something to
  // read: the reading is the bar's render gate as well as its content.
  expect(statusBar()).toBeNull();

  await offerUpdate();

  const bar = screen.getByRole("contentinfo");
  expect(bar).toContainElement(reading());
  expect(within(bar).queryByRole("region", { name: /status$/ })).not.toBeInTheDocument();
  expect(within(bar).getAllByRole("button").map((button) => button.textContent))
    .toEqual(["Review GMM updates in Settings"]);
});
