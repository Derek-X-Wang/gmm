import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import { makeQueryClient, renderWithQuery } from "./test/harness";
import type { GameCode, GameSummary } from "./api";

const { invoke, openDialog } = vi.hoisted(() => ({ invoke: vi.fn(), openDialog: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: openDialog }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn() }));
vi.mock("./diagnostics", () => ({ diagnosticsLogDir: vi.fn().mockResolvedValue("C:/GMM/logs"), exportDiagnosticsBundle: vi.fn() }));
vi.mock("./updater", () => {
  const idle = { kind: "idle" };
  return { updater: {
    getSnapshot: () => idle, subscribe: () => () => {}, checkForUpdates: vi.fn(),
    install: vi.fn(), decline: vi.fn(),
  } };
});
vi.mock("./LoaderVersionNote", () => ({ LoaderVersionNote: () => null }));
const { ModList } = await import("./ModList");
const { default: App } = await import("./App");

const outfit = {
  id: "mod-outfit", game: "srmi", name: "March Outfit", source: "local",
  library_path: "C:/Library/srmi/mod-outfit", enabled: false,
  gamebanana_id: null, source_url: null, version: null,
  reinstall_recovery: null, enabled_transition_recovery: null,
};
let mods: unknown[];
let modRead: () => unknown;
beforeEach(() => {
  mods = [];
  modRead = () => mods;
  openDialog.mockResolvedValue("C:/Downloads/outfit.zip");
  invoke.mockImplementation(async (command: string) => {
    if (command === "list_mods") return modRead();
    if (command === "detect_conflicts") return { conflicts: [], per_mod_count: {} };
    if (command === "list_variants") return { variants: [], activeVariantId: null };
    if (command === "list_mod_updates") return [];
    return null;
  });
});
function renderMods() {
  const client = makeQueryClient();
  client.setDefaultOptions({ queries: { retry: false, staleTime: 30_000 }, mutations: { retry: false } });
  return renderWithQuery(<ModList game="srmi" displayName="Honkai: Star Rail" />, { client });
}

it("leads confirmed-empty content with three named import routes then the ZIP drop zone", async () => {
  const { container } = renderMods();
  // Wait for the Library result; a missing empty-state heading should then fail immediately.
  await waitFor(() => expect(screen.queryByText("Loading…")).not.toBeInTheDocument(), { timeout: 10_000 });
  const heading = screen.getByRole("heading", { name: "Add Mods for Honkai: Star Rail" });
  const section = heading.closest("section")!;
  expect(within(section).getAllByRole("heading", { level: 3 }).map((entry) => entry.textContent)).toEqual([
    "Adopt for Honkai: Star Rail", "Import ZIP for Honkai: Star Rail", "GameBanana for Honkai: Star Rail",
  ]);
  const routes = within(section).getAllByRole("button");
  expect(routes.map((button) => button.textContent)).toEqual(["Adopt folder…", "Import ZIP…", "Paste GameBanana URL…"]);
  expect(section.querySelector("ul.mods")).toBeNull();
  expect(container.querySelector(".dropzone")).toHaveTextContent("Drop a .zip for Honkai: Star Rail here");
  expect(routes[2].compareDocumentPosition(container.querySelector(".dropzone")!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
});

it("omits the Mod count while the Mod query is unresolved", async () => {
  let finish!: (value: unknown[]) => void;
  modRead = () => new Promise((resolve) => { finish = resolve; });
  renderMods();
  await screen.findByText("Loading…");
  expect(screen.queryByRole("heading", { name: /Mods \(/ })).not.toBeInTheDocument();
  expect(screen.getByRole("heading", { name: "Mods" })).toBeVisible();
  await act(async () => finish([outfit]));
  expect(await screen.findByRole("heading", { name: "Mods (1)" })).toBeVisible();
});

it("omits the Mod count after the first Mod query fails", async () => {
  modRead = () => { throw new Error("Library unavailable"); };
  renderMods();
  expect(await screen.findByText("Library unavailable")).toBeVisible();
  expect(screen.queryByRole("heading", { name: /Mods \(/ })).not.toBeInTheDocument();
  expect(screen.getByRole("heading", { name: "Mods" })).toBeVisible();
});

it("never claims an empty Library while the Mod query is pending", async () => {
  let finish!: (value: unknown[]) => void;
  modRead = () => new Promise((resolve) => { finish = resolve; });
  renderMods();
  await screen.findByText("Loading…");
  expect(screen.queryByRole("heading", { name: "Add Mods for Honkai: Star Rail" })).not.toBeInTheDocument();
  expect(screen.queryByText(/Your Library has no Mods/)).not.toBeInTheDocument();
  await act(async () => finish([outfit]));
  expect(await screen.findByText("March Outfit")).toBeVisible();
});

it.each([false, true])("never claims an empty Library after a failed Mod query (previous empty result: %s)", async (previousEmpty) => {
  if (!previousEmpty) modRead = () => { throw new Error("Library unavailable"); };
  const { client } = renderMods();
  if (previousEmpty) {
    await screen.findByRole("heading", { name: "Add Mods for Honkai: Star Rail" });
    modRead = () => { throw new Error("Library unavailable"); };
    await act(async () => { await client.invalidateQueries({ queryKey: ["mods", "srmi"] }); });
  }
  expect(await screen.findByText("Library unavailable")).toBeVisible();
  expect(screen.queryByRole("heading", { name: "Add Mods for Honkai: Star Rail" })).not.toBeInTheDocument();
  expect(screen.queryByText(/Your Library has no Mods/)).not.toBeInTheDocument();
});

it("keeps populated content, the import toolbar and drop zone", async () => {
  mods = [outfit];
  const { container } = renderMods();
  const heading = await screen.findByRole("heading", { name: "Mods (1)" });
  expect(screen.queryByRole("heading", { name: "Add Mods for Honkai: Star Rail" })).not.toBeInTheDocument();
  expect(screen.getByText("March Outfit")).toBeVisible();
  expect(screen.getByRole("checkbox", { name: "Disabled" })).toBeEnabled();
  expect(within(heading.parentElement!).getByRole("button", { name: "Import ZIP…" })).toBeEnabled();
  expect(container.querySelector(".dropzone")!.compareDocumentPosition(screen.getByText("March Outfit")) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
});

it("keeps rows, Variants and both recovery warnings visible when the Conflict query fails", async () => {
  mods = [{
    ...outfit,
    reinstall_recovery: {
      reason: "Original deployment is locked", attemptedAt: "2026-10-07T12:00:00Z", attempts: 1,
      libraryPath: outfit.library_path, stagedPath: "C:/Library/srmi/.gmm-reinstall-stage",
      quarantinePath: "C:/Library/srmi/.gmm-delete-old", junctionWithdrawn: true,
      junctionWithdrawalError: null,
    },
    enabled_transition_recovery: {
      intendedEnabled: true, reason: "Producer identity is unavailable",
      attemptedAt: "2026-10-07T12:00:00Z", attempts: 1,
      junctionPath: "C:/Game/Mods/March Outfit", ownerUncertain: true,
    },
  }];
  const implementation = invoke.getMockImplementation()!;
  invoke.mockImplementation((command, args) => {
    if (command === "detect_conflicts") return Promise.reject(new Error("Conflict scan unavailable"));
    if (command === "list_variants") return Promise.resolve({
      variants: ["Red", "Blue"].map((name) => ({ id: name, mod_id: outfit.id, name, subpath: name })),
      activeVariantId: "Red",
    });
    return implementation(command, args);
  });
  renderMods();
  const error = await screen.findByText("GMM could not check this game's Mod conflicts.");
  expect(error.closest('[role="alert"]')).toHaveTextContent("Conflict scan unavailable");
  expect(error.closest('[role="alert"]')).toHaveTextContent("Conflict information is unavailable; this is not a “no conflicts” result.");
  expect(screen.getByText("March Outfit")).toBeVisible();
  expect(await screen.findByRole("radio", { name: "Red" })).toBeChecked();
  expect(screen.getByRole("radio", { name: "Blue" })).toBeDisabled();
  const reinstall = screen.getByRole("region", { name: "Interrupted reinstall recovery for March Outfit" });
  expect(within(reinstall).getByText("Unavailable — interrupted reinstall needs recovery")).toBeVisible();
  expect(within(reinstall).getByRole("button", { name: "Retry recovery" })).toBeEnabled();
  const transition = screen.getByRole("region", { name: "Interrupted enable or disable recovery for March Outfit" });
  expect(within(transition).getByText("Unavailable — deployment recovery is still pending")).toBeVisible();
  expect(within(transition).getByRole("button", { name: /retire producer/ })).toBeEnabled();
});

const routes = ["adopt", "zip", "gamebanana", "drop"] as const;
async function openRoute(entry: typeof routes[number], container: HTMLElement, filePath = "C:/Downloads/outfit.zip") {
  if (entry === "drop") {
    fireEvent.drop(container.querySelector(".dropzone")!, { dataTransfer: { files: [Object.assign(new File(["zip"], filePath.split("/").pop()!), { path: filePath })] } });
  } else {
    await userEvent.click(screen.getByRole("button", { name: entry === "adopt" ? "Adopt folder…" : entry === "zip" ? "Import ZIP…" : "Paste GameBanana URL…" }));
    if (entry !== "gamebanana") await userEvent.click(screen.getByRole("button", { name: entry === "adopt" ? "Pick mod folder" : "Pick .zip file" }));
  }
  const input = screen.getByPlaceholderText(entry === "gamebanana" ? "https://gamebanana.com/mods/1234567 or a bare ID" : entry === "drop" ? "Display name" : "Display name (e.g. Hu Tao Skin)");
  await userEvent.clear(input);
  await userEvent.type(input, entry === "gamebanana" ? "123" : "March Outfit");
}

it.each(routes)("keeps %s confirmation, pending state and failure visible through the first-Mod layout switch", async (entry) => {
  let reject!: (error: Error) => void;
  const pending = new Promise((_, rejectPromise) => { reject = rejectPromise; });
  const implementation = invoke.getMockImplementation()!;
  const command = entry === "adopt" ? "adopt_folder" : entry === "gamebanana" ? "import_gamebanana" : "import_zip";
  invoke.mockImplementation((name, args) => name === command ? pending : implementation(name, args));
  const { container, client } = renderMods();
  await screen.findByRole("heading", { name: "Add Mods for Honkai: Star Rail" });
  await openRoute(entry, container);
  if (entry === "drop") expect(screen.getByText("Ready to import for Honkai: Star Rail:")).toBeVisible();
  expect(invoke.mock.calls.filter(([name]) => name === command)).toHaveLength(0);
  await userEvent.click(screen.getByRole("button", { name: entry === "adopt" ? "Adopt" : "Import" }));
  expect(await screen.findByRole("button", { name: entry === "adopt" ? "Adopting…" : entry === "gamebanana" ? "Resolving…" : "Importing…" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "Cancel" })).toBeDisabled();
  mods = [outfit];
  await act(async () => { await client.invalidateQueries({ queryKey: ["mods", "srmi"] }); });
  expect(await screen.findByText("March Outfit")).toBeVisible();
  expect(screen.getByRole("button", { name: "Cancel" })).toBeDisabled();
  await act(async () => reject(new Error("Import refused")));
  expect(await screen.findByText("Import refused")).toBeVisible();
  expect(screen.getByRole("button", { name: "Cancel" })).toBeEnabled();
  await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument();
  expect(invoke.mock.calls.filter(([name]) => name === command)).toHaveLength(1);
});

it.each(["adopt", "zip"] as const)("a cancelled %s picker does not import or enable confirmation", async (entry) => {
  openDialog.mockResolvedValue(null);
  const { container } = renderMods();
  await screen.findByRole("heading", { name: "Add Mods for Honkai: Star Rail" });
  await openRoute(entry, container);
  expect(screen.getByRole("button", { name: entry === "adopt" ? "Adopt" : "Import" })).toBeDisabled();
  expect(container.querySelector("[data-command-failure-kind]")).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(invoke.mock.calls.some(([command]) => command === "adopt_folder" || command === "import_zip")).toBe(false);
});

// Exercise the production App selection paths and real ModList children. Only
// the desktop boundary is mocked, so a parent remount cannot hide a missing reset.
function renderImportApp() {
  const games: GameSummary[] = [
    { code: "gimi", displayName: "Genshin Impact" },
    { code: "srmi", displayName: "Honkai: Star Rail" },
  ];
  const paths: Partial<Record<GameCode, string>> = { gimi: "C:/Genshin", srmi: "C:/StarRail" };
  const implementation = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (command: string, args?: { game: GameCode; path: string }) => {
    switch (command) {
      case "is_onboarding_complete": return { complete: true, skipped: false };
      case "list_supported_games": return games;
      case "get_game_install_path": return paths[args!.game] || null;
      case "set_game_install_path": paths[args!.game] = args!.path; return null;
      case "get_game_path_status": return {
        game: args!.game, savedPath: paths[args!.game] || null,
        gamePath: { status: paths[args!.game] ? "readable" : "noSavedPath" },
        modelImporter: { status: "unverified", record: null, path: null, error: "Not checked" },
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
        defaultRoot: "C:/GMM/library", effectiveRoot: "C:/GMM/library",
        rootOverride: null, perGameOverrides: {}, perGameEffective: {},
      };
      case "mod_updates_globally_enabled": return true;
      case "adopt_folder":
      case "import_zip":
      case "import_gamebanana": return outfit;
      case "interrupted_session_launches": return [];
      default: return implementation(command, args);
    }
  });
  const client = makeQueryClient();
  client.setDefaultOptions({ queries: { retry: false, staleTime: 30_000 }, mutations: { retry: false } });
  return renderWithQuery(<App />, { client });
}

const gameChanges = ["tab click", "Settings path clear"] as const;
it.each(routes.flatMap((entry) => gameChanges.map((change) => ({ entry, change }))))(
  "discards the $entry draft on $change before accepting a new Game's import",
  async ({ entry, change }) => {
    const command = entry === "adopt" ? "adopt_folder" : entry === "gamebanana" ? "import_gamebanana" : "import_zip";
    const { container } = renderImportApp();
    await userEvent.click(await screen.findByRole("tab", { name: "Honkai: Star Rail" }));
    await screen.findByRole("heading", { name: "Add Mods for Honkai: Star Rail" });
    await openRoute(entry, container);
    expect(screen.getByRole("button", { name: entry === "adopt" ? "Adopt" : "Import" })).toBeEnabled();

    if (change === "tab click") {
      await userEvent.click(screen.getByRole("tab", { name: "Genshin Impact" }));
    } else {
      await userEvent.click(screen.getByRole("button", { name: "Settings for Honkai: Star Rail" }));
      await userEvent.clear(screen.getByRole("textbox", { name: "Honkai: Star Rail install path" }));
      await userEvent.click(screen.getByRole("button", { name: "Apply Honkai: Star Rail install path" }));
      await waitFor(() => expect(invoke).toHaveBeenCalledWith("set_game_install_path", { game: "srmi", path: "" }));
      await userEvent.click(screen.getByRole("button", { name: "Close" }));
      await waitFor(() => expect(screen.queryByRole("tab", { name: "Honkai: Star Rail" })).not.toBeInTheDocument());
    }
    expect(screen.getByRole("tab", { name: "Genshin Impact" })).toHaveAttribute("aria-selected", "true");
    await screen.findByRole("heading", { name: "Add Mods for Genshin Impact" });
    expect(screen.queryByRole("button", { name: entry === "adopt" ? "Adopt" : "Import" })).not.toBeInTheDocument();
    expect(screen.queryByDisplayValue(entry === "gamebanana" ? "123" : "March Outfit")).not.toBeInTheDocument();
    expect(screen.queryByText("C:/Downloads/outfit.zip")).not.toBeInTheDocument();
    expect(invoke.mock.calls.filter(([name]) => name === command)).toEqual([]);

    // Reopening must reveal a truly empty draft, not just hide a stale file.
    if (entry !== "drop") {
      await userEvent.click(screen.getByRole("button", { name: entry === "adopt" ? "Adopt folder…" : entry === "zip" ? "Import ZIP…" : "Paste GameBanana URL…" }));
      expect(screen.getByRole("button", { name: entry === "adopt" ? "Adopt" : "Import" })).toBeDisabled();
      expect(screen.getByPlaceholderText(entry === "gamebanana" ? "https://gamebanana.com/mods/1234567 or a bare ID" : "Display name (e.g. Hu Tao Skin)")).toHaveValue("");
      if (entry !== "gamebanana") expect(screen.getByRole("button", { name: entry === "adopt" ? "Pick mod folder" : "Pick .zip file" })).toBeVisible();
      await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    }
    openDialog.mockResolvedValue("C:/Downloads/genshin.zip");
    await openRoute(entry, container, "C:/Downloads/genshin.zip");
    await userEvent.click(screen.getByRole("button", { name: entry === "adopt" ? "Adopt" : "Import" }));
    await waitFor(() => expect(invoke.mock.calls.filter(([name]) => name === command)).toHaveLength(1));
    expect(invoke).toHaveBeenCalledWith(command, { args: entry === "gamebanana"
      ? { game: "gimi", urlOrId: "123" }
      : entry === "adopt" ? { game: "gimi", sourcePath: "C:/Downloads/genshin.zip", name: "March Outfit" }
        : { game: "gimi", zipPath: "C:/Downloads/genshin.zip", name: "March Outfit" } });
  },
);

it.each(["adopt", "zip"] as const)("discards a late %s picker result after the Game changes", async (entry) => {
  let resolve!: (path: string) => void;
  openDialog.mockReturnValue(new Promise<string>((finish) => { resolve = finish; }));
  renderImportApp();
  await userEvent.click(await screen.findByRole("tab", { name: "Honkai: Star Rail" }));
  await userEvent.click(screen.getByRole("button", { name: entry === "adopt" ? "Adopt folder…" : "Import ZIP…" }));
  await userEvent.type(screen.getByPlaceholderText("Display name (e.g. Hu Tao Skin)"), "Star Rail draft");
  await userEvent.click(screen.getByRole("button", { name: entry === "adopt" ? "Pick mod folder" : "Pick .zip file" }));
  await userEvent.click(screen.getByRole("tab", { name: "Genshin Impact" }));
  await act(async () => resolve("C:/Downloads/star-rail.zip"));
  await userEvent.click(screen.getByRole("button", { name: entry === "adopt" ? "Adopt folder…" : "Import ZIP…" }));
  expect(screen.getByPlaceholderText("Display name (e.g. Hu Tao Skin)")).toHaveValue("");
  await userEvent.type(screen.getByPlaceholderText("Display name (e.g. Hu Tao Skin)"), "Genshin draft");
  expect(screen.getByRole("button", { name: entry === "adopt" ? "Adopt" : "Import" })).toBeDisabled();
  expect(screen.queryByText("C:/Downloads/star-rail.zip")).not.toBeInTheDocument();
  expect(invoke.mock.calls.filter(([name]) => name === "adopt_folder" || name === "import_zip")).toEqual([]);
});

it("retains recovery feedback during same-Game imports and clears it on a Game change", async () => {
  const recovery = {
    reason: "Original deployment is locked", attemptedAt: "2026-10-07T12:00:00Z", attempts: 1,
    libraryPath: outfit.library_path, stagedPath: "C:/Library/srmi/.gmm-reinstall-stage",
    quarantinePath: "C:/Library/srmi/.gmm-delete-old", junctionWithdrawn: true,
    junctionWithdrawalError: null,
  };
  mods = [{ ...outfit, reinstall_recovery: recovery }];
  const implementation = invoke.getMockImplementation()!;
  invoke.mockImplementation((command, args) => {
    if (command === "retry_reinstall_recovery") {
      mods = [outfit];
      return Promise.resolve({ status: "recovered" });
    }
    return implementation(command, args);
  });
  const view = renderMods();
  await userEvent.click(await screen.findByRole("button", { name: "Retry recovery" }));
  const message = "Recovered the interrupted reinstall for March Outfit. The Mod is usable again.";
  expect(await screen.findByText(message)).toBeVisible();
  await openRoute("zip", view.container);
  expect(screen.getByText(message)).toBeVisible();
  await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(screen.getByText(message)).toBeVisible();
  view.rerender(<ModList game="gimi" displayName="Genshin Impact" />);
  expect(screen.queryByText(message)).not.toBeInTheDocument();
  view.rerender(<ModList game="srmi" displayName="Honkai: Star Rail" />);
  expect(screen.queryByText(message)).not.toBeInTheDocument();
});
