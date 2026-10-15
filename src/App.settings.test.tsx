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
/** The Settings dialog, by its role rather than by a class name. */
function dialog() {
  return screen.getByRole("dialog", { name: "Settings" });
}
function mainScreen() {
  return document.querySelector<HTMLElement>("main[data-main-screen]");
}
function statusBar() {
  return document.querySelector<HTMLElement>("footer.status-bar");
}
/** Every main-screen landmark the old `noMainContent()` named. */
const MAIN_CONTENT = {
  launch: { role: "button", name: /Launch Honkai/ },
  header: { role: "region", name: "Honkai: Star Rail header" },
  mods: { role: "heading", name: /^(Mods \(|Add Mods for )/ },
} as const;

/**
 * Replaces #284's `noMainContent()` (#353).
 *
 * That assertion asked for no Launch control, no Mod heading and no dropzone
 * anywhere in the document, not even hidden — and it held because Settings
 * was a destination that unmounted the main screen. Settings is a modal
 * dialog now and the main screen stays in place behind it, so absence is no
 * longer the property on offer and an unchanged assertion would fail for the
 * right reason. What replaces it is the property the dialog actually owes:
 * the main screen is still there, whole, and cannot be used. Each element the
 * old assertion named is still named here — once as present behind the
 * dialog, once as unreachable — so nothing it covered went uncovered.
 *
 * Reachability here is the accessibility tree, which is what `hidden: true`
 * switches off and what tells "present behind" from "available". The pointer
 * and focus halves need the authored stylesheet and a real Tab press; they
 * have their own tests below.
 */
function mainScreenBehindDialogIsInert() {
  const main = mainScreen();
  const bar = statusBar();
  // Still mounted, and still whole: a dialog that unmounted the screen behind
  // it, or emptied it, fails here.
  expect(main).toBeInTheDocument();
  expect(bar).toBeInTheDocument();
  for (const { role, name } of Object.values(MAIN_CONTENT)) {
    expect(screen.getByRole(role, { name, hidden: true })).toBeInTheDocument();
  }
  expect(main!.querySelector(".dropzone")).not.toBeNull();
  expect(bar!).toContainElement(screen.getByRole("button", { name: "Settings for Honkai: Star Rail", hidden: true }));

  // …and out of reach. Both regions are inert and out of the accessibility
  // tree, so each control above is unreachable rather than gone.
  for (const region of [main!, bar!]) {
    expect(region).toHaveAttribute("inert");
    expect(region).toHaveAttribute("aria-hidden", "true");
  }
  for (const { role, name } of Object.values(MAIN_CONTENT)) {
    expect(screen.queryByRole(role, { name })).not.toBeInTheDocument();
  }
  expect(screen.queryByRole("button", { name: "Settings for Honkai: Star Rail" })).not.toBeInTheDocument();
  // The two landmarks go with them: a modal dialog is the only surface.
  expect(screen.queryByRole("main")).not.toBeInTheDocument();
  expect(screen.queryByRole("contentinfo")).not.toBeInTheDocument();

  // The dialog keeps the content rule it inherited from the full-screen
  // destination: no Launch control and no Mod list of its own (#284 round 2).
  const inside = within(dialog());
  expect(inside.queryByRole("button", { name: /^Launch /, hidden: true })).not.toBeInTheDocument();
  expect(inside.queryByRole("heading", { name: MAIN_CONTENT.mods.name, hidden: true })).not.toBeInTheDocument();
  expect(dialog().querySelector(".dropzone")).toBeNull();
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
      await userEvent.click(screen.getByRole("button", { name: "Close" }));
    }

    // Check both parts: hidden on the root and App.css suppressing its display.
    // toBeVisible alone accepts hidden even if CSS makes the root display: block.
    expect(root).toBeInTheDocument();
    expect(root).toHaveAttribute("hidden");
    expect(root).not.toBeVisible();
    expect(getComputedStyle(root).display).toBe("none");

    // The computed value above catches an authored rule that outranks the
    // hiding — which is the break the dialog made possible, since the scrim
    // now carries a `display` of its own for centring the panel (#353).
    //
    // It cannot catch the authored rule being deleted: jsdom's own user-agent
    // stylesheet implements `[hidden] { display: none }`, so the value stays
    // `none` with no author rule at all. In the browser it would not. There
    // the user-agent rule loses to any author `display` on the same element,
    // and `.settings-screen` has one — so `[hidden]`'s `!important` is what
    // keeps the surface and its retained pages hidden, and nothing about its
    // presence is observable from the rendered tree. Read the authored text
    // instead, the way `App.test.tsx` reads the status bar's light-mode rules.
    expect(
      readFileSync("src/App.css", "utf8"),
      "`[hidden]` must outrank `.settings-screen`'s own display, or the retired surface reappears",
    ).toMatch(/\[hidden\]\s*\{\s*display:\s*none\s*!important\s*;?\s*\}/);
  } finally {
    style.remove();
  }
});

/**
 * The body of the `prefers-color-scheme: light` block that repaints the
 * Settings panel, with comments stripped. jsdom evaluates no media query, so
 * the block is unreachable through `getComputedStyle` on a rendered control;
 * reading the authored rules is the same route `App.test.tsx` takes for the
 * status bar's light-mode rules.
 */
function lightPanelRules(): string {
  const authored: string = readFileSync("src/App.css", "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
  const block = authored.indexOf("@media (prefers-color-scheme: light)", authored.indexOf(".settings-dialog {"));
  const open = authored.indexOf("{", block);
  let depth = 0;
  for (let index = open; index < authored.length; index += 1) {
    if (authored[index] === "{") depth += 1;
    if (authored[index] === "}" && (depth -= 1) === 0) return authored.slice(open + 1, index);
  }
  throw new Error("the Settings panel's light-mode block is never closed");
}

it("repaints every control in the light Settings panel, not only the navigation", () => {
  // The panel is the one light surface in the application: `:root` keeps the
  // main screen dark in both schemes, so this is the only place a control is
  // painted on a light background. The base `button` is a neutral fill with
  // `color: inherit`, and `inherit` inside the panel is its own `#18181b` —
  // dark text on a dark fill, about 1.14:1 against WCAG's 4.5:1. A light rule
  // that names only `.settings-navigation button` leaves Close, Apply,
  // Change…, Re-detect, Rebuild junctions and Export diagnostics bundle
  // unreadable, so the panel has to repaint the control itself (#367).
  //
  // The probe's only ancestor is the panel, so a rule scoped to the
  // navigation cannot satisfy this test.
  const panel = document.createElement("div");
  panel.className = "settings-dialog";
  const control = document.createElement("button");
  panel.append(control);

  const repaints = [...lightPanelRules().matchAll(/([^{}]+)\{([^{}]*)\}/g)]
    .filter(([, selectors]) => selectors.split(",").some((raw) => control.matches(raw.trim())))
    .map(([, , declarations]) => declarations);
  expect(
    repaints,
    "an ordinary control in the panel needs a light-mode rule of its own",
  ).not.toHaveLength(0);

  // A stated foreground, not an inherited one: the control paints a fill, so
  // its text colour cannot be left to whatever the panel sets. This reads the
  // declaration rather than the computed value on purpose — jsdom resolves
  // `color: inherit` against `<body>`, so it would report a perfectly
  // readable black and hide the exact shape of the defect.
  const declarations = repaints.join(";");
  expect(
    {
      foreground: /(^|;)\s*color\s*:\s*(?!inherit\b)\S/.test(declarations),
      fill: /(^|;)\s*background(-color)?\s*:\s*(?!inherit\b)\S/.test(declarations),
    },
    "a painted fill with an inherited foreground is dark text on a dark button here",
  ).toEqual({ foreground: true, fill: true });
});

it.each(["global", "game"])("keeps the main screen inert behind the dialog and preserves independent selection by %s arrival", async (arrival) => {
  renderApp();
  await screen.findByRole("tab", { name: "Honkai: Star Rail" });
  if (arrival === "global") await openGlobal();
  else await openGame();
  expect(screen.getByRole("heading", { name: "Settings", level: 1 })).toBeInTheDocument();
  expect(pageButton(arrival === "global" ? "GMM settings" : "Honkai: Star Rail")).toHaveAttribute("aria-current", "page");
  mainScreenBehindDialogIsInert();
  const navigation = screen.getByRole("navigation", { name: "Settings pages" });
  expect(within(navigation).getAllByRole("button")).toHaveLength(7);
  for (const game of games) {
    await userEvent.click(pageButton(game.displayName));
    expect(screen.getByRole("textbox", { name: `${game.displayName} install path` })).toBeInTheDocument();
    expect(screen.getByText("Return to Honkai: Star Rail")).toBeInTheDocument();
    mainScreenBehindDialogIsInert();
  }
  await userEvent.click(pageButton("GMM settings"));
  expect(screen.getByPlaceholderText("proxy URL")).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Close" }));
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
  await userEvent.click(screen.getByRole("button", { name: "Close" }));
  expect(screen.queryByRole("tab", { name: "Genshin Impact" })).not.toBeInTheDocument();
  await openGame();
  await userEvent.click(pageButton("Genshin Impact"));
  reject = false;
  await userEvent.type(screen.getByRole("textbox", { name: "Genshin Impact install path" }), "C:\\Genshin");
  await userEvent.click(screen.getByRole("button", { name: "Apply Genshin Impact install path" }));
  await waitFor(() => expect(paths.gimi).toBe("C:\\Genshin"));
  await userEvent.click(screen.getByRole("button", { name: "Close" }));
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
    expect(within(dialog()).queryByText(/Missing Game entries:/)).not.toBeInTheDocument();
    expect(within(dialog()).getByText(gamePath.paths.join(", "), { selector: "p", exact: true })).toBeVisible();
  }
  for (const other of outcomes.filter((value) => value.line !== line)) {
    expect(pageButton("Honkai: Star Rail")).not.toHaveTextContent(other.line);
  }
  if (gamePath.status === "invalid" || modelImporter.status === "unverified" && gamePath.status === "readable") {
    expect(pageButton("Honkai: Star Rail")).not.toHaveTextContent(/not installed|Absent|cannot be found/);
  }
  await userEvent.click(screen.getByRole("button", { name: "Close" }));
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
  await userEvent.click(screen.getByRole("button", { name: "Close" }));
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
  await userEvent.click(screen.getByRole("button", { name: "Close" }));
  expect(await screen.findByText("ZZMI is running")).toBeInTheDocument();
  expect(screen.queryByText(/ended unexpectedly/)).not.toBeInTheDocument();
  expect(invoke.mock.calls.filter(([command]) => command === "clean_stale_session")).toHaveLength(1);
  await openGame();
  session = null;
  await act(async () => events["session-ended"]());
  await userEvent.click(screen.getByRole("button", { name: "Close" }));
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
  if (route === "button") await userEvent.click(screen.getByRole("button", { name: "Close" }));
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
    await userEvent.click(screen.getByRole("button", { name: "Close" }));
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
  await userEvent.click(screen.getByRole("button", { name: "Close" }));
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
  await userEvent.click(screen.getByRole("button", { name: "Close" }));
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
  await userEvent.tab(); // Close
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
  await userEvent.click(screen.getByRole("button", { name: "Close" }));
  await clickMenuItem(listen, "run-setup");
  await screen.findByRole("heading", { name: "First-run setup" });
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
  await userEvent.click(screen.getByRole("button", { name: "Close" }));
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
  await userEvent.click(screen.getByRole("button", { name: "Close" }));
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
  await userEvent.click(screen.getByRole("button", { name: "Close" }));
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
  mainScreenBehindDialogIsInert();
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
  expect(await screen.findByRole("heading", { name: "First-run setup" })).toBeVisible();
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
  expect(screen.queryByRole("heading", { name: "First-run setup" })).not.toBeInTheDocument();
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
  await userEvent.click(screen.getByRole("button", { name: "Close" }));

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
  await userEvent.click(screen.getByRole("button", { name: "Close" }));

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

  if (route === "button") await userEvent.click(screen.getByRole("button", { name: "Close" }));
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
  await userEvent.click(screen.getByRole("button", { name: "Close" }));

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
  expect(await screen.findByRole("heading", { name: "First-run setup" })).toBeVisible();
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

// The per-Game Settings link moved into the bottom status bar in #352, so it
// is no longer inside `<main>`. The return chain resolves
// `[data-settings-arrival="..."]` across the whole document and then falls
// back to the selected Game tab, Add game, and the main region, so none of
// these answers may change because the link moved.
it.each([
  ["game", "button"], ["game", "escape"],
  ["global", "button"], ["global", "escape"],
  ["add", "button"], ["add", "escape"],
])("returns focus to the %s arrival control on %s return", async (arrival, route) => {
  renderApp();
  await screen.findByRole("tab", { name: "Honkai: Star Rail" });

  if (arrival === "game") await openGame();
  else if (arrival === "global") await openGlobal();
  else {
    await userEvent.click(screen.getByRole("button", { name: "Add game" }));
    await userEvent.click(screen.getByRole("button", { name: "Set up Genshin Impact" }));
  }
  expect(screen.getByRole("heading", { name: "Settings", level: 1 })).toBeVisible();

  if (route === "button") await userEvent.click(screen.getByRole("button", { name: "Close" }));
  else fireEvent.keyDown(window, { key: "Escape" });

  const expected = arrival === "game"
    ? screen.getByRole("button", { name: "Settings for Honkai: Star Rail" })
    : arrival === "add"
      ? screen.getByRole("button", { name: "Add game" })
      : screen.getByRole("tab", { name: "Honkai: Star Rail" });
  expect(expected).toHaveFocus();
  expect(document.body).not.toHaveFocus();
  // Specifically for the moved control: it is reached from outside `<main>`.
  if (arrival === "game") {
    expect(screen.getByRole("main")).not.toContainElement(expected);
    expect(screen.getByRole("contentinfo")).toContainElement(expected);
  }
});

// ---- #353 — Settings as a modal dialog over the main screen ----
//
// The containment below did not exist before #353. Settings was a destination
// that unmounted the main screen, so nothing behind it could be clicked or
// focused. The dialog leaves the screen mounted, and `inert`, `aria-modal`
// and the dialog's own focus cycle are what replace the unmount.

/** Run `body` with the authored stylesheet in the document. jsdom computes no
 * layout, but it does apply the cascade, so the pointer half of inertness is
 * observable from the real stylesheet rather than from a stub. */
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

it("refuses pointer input to the main screen and the status bar behind the dialog", async () => {
  await withAppCss(async () => {
    renderApp();
    await screen.findByRole("tab", { name: "Honkai: Star Rail" });
    const launch = screen.getByRole("button", { name: /Launch Honkai/ });
    const link = screen.getByRole("button", { name: "Settings for Honkai: Star Rail" });
    expect(getComputedStyle(launch).pointerEvents).not.toBe("none");

    await userEvent.click(link);

    // Both regions refuse the pointer, and so does every control inside them:
    // `pointer-events` inherits, so one declaration on the inert region covers
    // the whole screen rather than a list of controls someone has to keep up
    // to date.
    for (const element of [mainScreen()!, statusBar()!, launch, link]) {
      expect(getComputedStyle(element).pointerEvents).toBe("none");
    }
    await expect(userEvent.click(launch)).rejects.toThrow(/pointer-events: none/);
    await expect(userEvent.click(link)).rejects.toThrow(/pointer-events: none/);
    expect(invoke.mock.calls.filter(([command]) => command === "launch_game")).toEqual([]);

    // The dialog's own controls are unaffected by the rule above…
    await userEvent.click(pageButton("GMM settings"));
    expect(pageButton("GMM settings")).toHaveAttribute("aria-current", "page");

    // …and Close gives the window back, on the same nodes: a click that was
    // refused a moment ago now opens Settings again.
    await userEvent.click(screen.getByRole("button", { name: "Close" }));
    for (const element of [mainScreen()!, statusBar()!, launch, link]) {
      expect(getComputedStyle(element).pointerEvents).not.toBe("none");
    }
    await userEvent.click(link);
    expect(dialog()).toBeVisible();
  });
});

it("holds Tab inside the dialog instead of letting it reach the window behind", async () => {
  renderApp();
  await screen.findByRole("tab", { name: "Honkai: Star Rail" });
  await openGame();
  // Leave a retained page behind as well, so the cycle has a hidden region to
  // stay out of too — the property `LibraryPathEditors.test.tsx` proves for
  // the pages, held here for the dialog as a whole.
  await userEvent.click(pageButton("GMM settings"));
  const retained = document.querySelector<HTMLElement>('[data-settings-page="srmi"]')!;
  expect(retained).toHaveAttribute("hidden");

  const visited = new Set<Element>();
  for (let press = 0; press < 30; press++) {
    await userEvent.tab();
    const focused = document.activeElement as HTMLElement;
    expect(dialog()).toContainElement(focused);
    expect(mainScreen()).not.toContainElement(focused);
    expect(statusBar()).not.toContainElement(focused);
    expect(retained.contains(focused)).toBe(false);
    expect(focused).not.toBe(document.body);
    visited.add(focused);
  }
  // A cycle of one would satisfy every assertion above while trapping the
  // user on a single control.
  expect(visited.size).toBeGreaterThan(1);

  // Backwards too: Shift+Tab off the dialog's first control wraps inside it
  // rather than stepping out into the screen behind.
  for (let press = 0; press < 12; press++) {
    await userEvent.tab({ shift: true });
    expect(dialog()).toContainElement(document.activeElement as HTMLElement);
  }

  // And it recovers. Focus parked outside the dialog — which is where the
  // browser leaves it the moment the main screen becomes inert — is pulled
  // back in by the next Tab instead of walking on through the main screen.
  screen.getByRole("button", { name: /Launch Honkai/, hidden: true }).focus();
  await userEvent.tab();
  expect(dialog()).toContainElement(document.activeElement as HTMLElement);
});

/**
 * The dialog's whole tab cycle, walked from the heading with real Tab presses
 * until it wraps. Every stop is asserted to be inside the dialog, so the walk
 * is a containment check over the entire cycle as well as a list of its stops;
 * the 30-press loop above holds the same property under sustained pressing.
 */
async function tabCycle(): Promise<HTMLElement[]> {
  screen.getByRole("heading", { name: "Settings", level: 1 }).focus();
  const stops: HTMLElement[] = [];
  for (let press = 0; press < 200; press += 1) {
    await userEvent.tab();
    const focused = document.activeElement as HTMLElement;
    expect(dialog()).toContainElement(focused);
    if (focused === stops[0]) return stops;
    stops.push(focused);
  }
  throw new Error("the Settings dialog's tab cycle never wrapped");
}

it("makes each radio group in the dialog one tab stop, at the checked radio", async () => {
  // The real duplicate-Mod-record selector from `LibraryAuditWarning`, not a
  // synthetic fixture, so this stays true if that markup changes (#360).
  //
  // Three records in the first group, so the checked one can be neither the
  // first radio nor the last and the before/after assertions below cannot
  // both be satisfied by accident. Two groups, so a cycle that collapsed
  // every radio in the dialog into one stop is not mistaken for one that
  // groups by `name` — these radios sit in a `fieldset`, with no form owner
  // at all, so the name is the whole of what separates them.
  const SHARED = "C:\\GMM\\library\\srmi\\01SHARED";
  const ALSO_SHARED = "C:\\GMM\\library\\srmi\\01ALSO";
  const record = (path: string, id: string, name: string) => ({
    id, game: "srmi", name, source: "manual", libraryPath: path, junctionDirName: name,
    enabled: false, createdAt: "2026-08-23T00:00:00Z", gamebananaId: null, sourceUrl: null,
    author: null, version: null, upstreamVersion: null, updateCheckEnabled: true,
    screenshotUrl: null, variants: [], reinstallInProgress: false, fingerprint: `${id}-fingerprint`,
  });
  const original = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (command: string, args?: { game: GameCode; path: string }) => {
    if (command === "audit_library" && args?.game === "srmi") return {
      game: "srmi", unreferenced: [], totalBytes: 0, duplicates: [
        { path: SHARED, mods: [
          record(SHARED, "01ONE", "First record"),
          record(SHARED, "01TWO", "Second record"),
          record(SHARED, "01THREE", "Third record"),
        ] },
        { path: ALSO_SHARED, mods: [
          record(ALSO_SHARED, "01FOUR", "Fourth record"),
          record(ALSO_SHARED, "01FIVE", "Fifth record"),
        ] },
      ],
    };
    return original(command, args);
  });
  renderApp();
  await screen.findByRole("tab", { name: "Honkai: Star Rail" });
  await openGame();
  const groups = await screen.findAllByRole("group", { name: "Select the one Mod record to keep" });
  expect(groups).toHaveLength(2);
  const radios = groups.map((group) => within(group).getAllByRole<HTMLInputElement>("radio"));
  expect(radios.map((group) => group.length)).toEqual([3, 2]);
  const radiosInCycle = async () =>
    (await tabCycle()).filter((stop) => (stop as HTMLInputElement).type === "radio");
  const checked = () => radios.flat().filter((radio) => radio.checked);

  // Nothing is preselected here — the audit never chooses a keeper — and the
  // group is still reachable, at its first radio, as it is in a browser.
  expect(await radiosInCycle()).toEqual([radios[0][0], radios[1][0]]);
  // Tabbing in, through and out of a group selects nothing.
  expect(checked()).toEqual([]);

  await userEvent.click(radios[0][1]);
  expect(radios[0][1]).toBeChecked();
  expect(radios[0][1]).toHaveFocus();

  // Tab leaves the group entirely rather than stepping onto the next,
  // unchecked record — "Resolve…" is the control after the group, and it is
  // enabled exactly because a record is now selected.
  const resolve = within(groups[0].closest("li")!).getByRole("button", { name: "Resolve…" });
  await userEvent.tab();
  expect(document.activeElement).toBe(resolve);

  // Shift+Tab is the mirror image: back into the group at the checked radio,
  // not at the last one.
  await userEvent.tab({ shift: true });
  expect(document.activeElement).toBe(radios[0][1]);

  // One stop per group wherever the checked radio sits inside it, and the
  // selection is untouched by the whole walk.
  expect(await radiosInCycle()).toEqual([radios[0][1], radios[1][0]]);
  expect(checked()).toEqual([radios[0][1]]);
});

/**
 * Every control in the dialog a user must be able to reach, enumerated from
 * the accessibility tree rather than from `tabStops`'s own selector — so this
 * cannot agree with the production filter by construction, and `queryAllByRole`
 * is also what leaves the retained `hidden` page's controls out. Radios are
 * excluded on purpose: a group contributes one stop, not one per radio, which
 * is the property the test above holds.
 *
 * Disabled controls are filtered because they are in the tree but out of the
 * tab order — "Resolve…" before a record is chosen is exactly that case.
 */
function reachableControlsRequired(): HTMLElement[] {
  const roles = ["button", "textbox", "combobox", "checkbox", "listbox", "link", "spinbutton"] as const;
  return roles
    .flatMap((role) => within(dialog()).queryAllByRole(role))
    .filter((control) => !(control as HTMLInputElement).disabled);
}

/** Enough of a control to recognise which one went missing. */
function controlName(control: HTMLElement): string {
  const input = control as HTMLInputElement;
  return `${control.tagName.toLowerCase()} ${control.getAttribute("aria-label") ?? input.placeholder
    ?? control.textContent?.trim().slice(0, 40) ?? ""}`.trim();
}

it.each(["Honkai: Star Rail", "GMM settings"])("keeps every control on the %s page in the tab cycle, not merely contained", async (page) => {
  // Containment is not reachability. `tabCycle()` proves Tab stays inside the
  // dialog and that the walk wraps; neither notices a control that silently
  // leaves the cycle, because a `tabStops()` that dropped a whole class of
  // element keeps Tab inside the dialog and still wraps — the control just
  // becomes unreachable by keyboard. A mutation excluding `input[type=text]`
  // left the whole suite green while the install-path field could no longer
  // be reached at all (#360 review).
  //
  // Asserting a literal list of every stop would be stronger still, and was
  // rejected: it would have to be rewritten by any unrelated control added to
  // any Settings page, and three other PRs are editing these pages. The
  // property asserted here needs no such edit — it holds for whatever the
  // panel contains, on both the per-Game page and the Application page, which
  // between them hold every kind of control the dialog uses.
  renderApp();
  await screen.findByRole("tab", { name: "Honkai: Star Rail" });
  await openGame();
  if (page === "GMM settings") {
    await userEvent.click(pageButton(page));
    await screen.findByPlaceholderText("proxy URL");
  }
  const required = reachableControlsRequired();
  expect(required.length).toBeGreaterThan(5);

  const stops = await tabCycle();
  expect(required.filter((control) => !stops.includes(control)).map(controlName)).toEqual([]);
});

it("closes on Escape only when nothing inside the dialog has consumed it", async () => {
  renderApp();
  await openGame();

  // The Library action forms cancel their own prompt on Escape, and they also
  // stop propagation, so the dialog's window handler never sees those presses
  // at all — which is why the test above them passes either way. This is the
  // other half of the contract, and the half the handler itself has to honour:
  // a consumer that prevents the default without stopping propagation. Capture
  // phase, so it runs before the dialog's own listener, exactly as a handler
  // nested inside the dialog would.
  const consume = (event: KeyboardEvent) => event.preventDefault();
  window.addEventListener("keydown", consume, true);
  try {
    fireEvent.keyDown(window, { key: "Escape" });
    expect(dialog()).toBeVisible();
    expect(screen.getByRole("textbox", { name: "Honkai: Star Rail install path" })).toBeVisible();
  } finally {
    window.removeEventListener("keydown", consume, true);
  }

  // Unconsumed, the same press closes it — so the assertion above is about the
  // ordering, not about Escape having been wired up at all.
  fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Settings for Honkai: Star Rail" })).toHaveFocus();
});

it("is a modal dialog whose only dismissal is Close", async () => {
  renderApp();
  await openGame();
  const panel = dialog();
  const title = screen.getByRole("heading", { name: "Settings", level: 1 });

  expect(panel).toHaveAttribute("aria-modal", "true");
  expect(panel).toHaveAttribute("aria-labelledby", title.id);
  expect(panel).toContainElement(title);
  // Not a second `main` landmark: the main screen behind the dialog owns that
  // one, and two would make `main` ambiguous for a screen reader as much as
  // for a test.
  expect(panel.tagName).toBe("DIV");

  // The dialog's own chrome holds exactly one control, and it is Close. OK,
  // Cancel and Apply are not merely absent by name — there is nowhere in the
  // dialog's chrome for a second dismissal to be.
  const header = panel.querySelector("header")!;
  expect(within(header).getAllByRole("button").map((button) => button.textContent)).toEqual(["Close"]);
  // Nothing anywhere in the dialog claims a reversal of a committed setting.
  // Each editor commits its own change through its own mutation before the
  // user reaches any button, so a Cancel here could not cancel (#326).
  expect(within(panel).queryByRole("button", { name: /^(OK|Apply|Cancel|Discard|Revert|Done)$/ })).not.toBeInTheDocument();

  await userEvent.click(within(header).getByRole("button", { name: "Close" }));
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
});

it("keeps the status bar on screen behind the dialog and live again on Close", async () => {
  renderApp();
  const bar = await screen.findByRole("contentinfo");
  await within(bar).findByText("Game: Game readable · Model Importer unverified");

  await openGame();

  // It stays. The bar is part of the window the dialog is drawn over, so a bar
  // that vanished would change the shape of the window behind the dialog and
  // pop back on Close, and `.app`'s bottom padding — the bar's scroll
  // clearance — would be left holding a gap with nothing in it (#352).
  // Unmounting it would also take the arrival marker that the focus return
  // resolves after Close.
  expect(statusBar()).toBe(bar);
  expect(bar).toBeVisible();
  expect(bar).toHaveTextContent("Game: Game readable · Model Importer unverified");
  expect(bar.querySelector('[data-settings-arrival="game"]')).not.toBeNull();
  // …inert with the main screen, so a visible bar is not a live second route
  // into Settings while Settings is already open.
  expect(bar).toHaveAttribute("inert");
  expect(bar).toHaveAttribute("aria-hidden", "true");
  expect(screen.queryByRole("contentinfo")).not.toBeInTheDocument();

  fireEvent.keyDown(window, { key: "Escape" });

  expect(screen.getByRole("contentinfo")).toBe(bar);
  expect(bar).not.toHaveAttribute("inert");
  expect(bar).not.toHaveAttribute("aria-hidden");
  expect(within(bar).getByRole("button", { name: "Settings for Honkai: Star Rail" })).toHaveFocus();
});

/**
 * Sizing (#353). jsdom computes no layout: it resolves no `min()`, applies no
 * `max-height`, and `getBoundingClientRect` returns zeros. Nothing here proves
 * the dialog's rendered size, or that the main screen is visible around it.
 * What it does prove is that the authored stylesheet bounds the dialog inside
 * the window with an inset and gives it its own scroll, instead of letting it
 * fill the window like the destination it replaced. The rendered result at
 * 1100x760 and at the 900x600 minimum (#351) is a Windows check, named in the
 * PR.
 */
it("bounds the dialog inside an inset scrim instead of filling the window", async () => {
  await withAppCss(async () => {
    renderApp();
    await openGame();
    const scrim = document.querySelector<HTMLElement>(".settings-screen")!;
    expect(scrim).toContainElement(dialog());

    const over = getComputedStyle(scrim);
    expect([over.position, over.inset, over.display]).toEqual(["fixed", "0px", "grid"]);
    // Above `.status-bar`'s own `z-index: 20`, or the pinned bar would draw
    // over the dialog it is supposed to be behind.
    expect(Number(over.zIndex)).toBeGreaterThan(20);
    // The inset is what leaves the main screen visible around the dialog, so
    // it reads as a dialog rather than a page. 2rem on each side puts the
    // panel at 836x536 inside the 900x600 window minimum (#351).
    expect(parseFloat(over.padding)).toBeGreaterThan(0);
    // The scrim scrolls, and that scroll does not chain into the main screen
    // behind it.
    expect([over.overflow, over.overscrollBehavior]).toEqual(["auto", "contain"]);

    const sized = getComputedStyle(dialog());
    // Bounded by the window rather than given a fixed size, and scrolling
    // inside itself so a long Settings page cannot push it past the edge.
    expect(sized.width).toBe("min(1120px, 100%)");
    expect(sized.maxHeight).toBe("100%");
    expect(sized.overflow).toBe("auto");
    // Not resizable (#353). Adding a resize affordance fails here.
    expect(sized.resize).toBe("none");
  });
});
