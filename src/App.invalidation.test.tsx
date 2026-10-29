import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";

import type { ConflictReport, ImporterEvacuationRecovery, ModVariants } from "./api";
import { makeQueryClient, renderWithQuery } from "./test/harness";
import { clickMenuItem } from "./test/menu";

const { invoke, openDialog, listen } = vi.hoisted(() => ({
  invoke: vi.fn(),
  openDialog: vi.fn(),
  listen: vi.fn().mockResolvedValue(() => { }),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: openDialog, save: vi.fn() }));
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
vi.mock("./LoaderVersionNote", () => ({ LoaderVersionNote: () => null }));

const { default: App } = await import("./App");

function renderApp() {
  const client = makeQueryClient();
  // Match main.tsx: fresh cached data must not conceal missing invalidation.
  client.setDefaultOptions({
    queries: { retry: false, gcTime: 0, staleTime: 30_000 },
    mutations: { retry: false },
  });
  return renderWithQuery(<App />, { client });
}

function modFixture(enabled = false) {
  return {
    id: "mod-outfit",
    game: "gimi",
    name: "Test Outfit",
    source: "gamebanana",
    library_path: "C:\\GMM\\library\\gimi\\mod-outfit",
    enabled,
    gamebanana_id: 123,
    source_url: "https://gamebanana.com/mods/123",
    version: "1.0",
    reinstall_recovery: null,
    enabled_transition_recovery: null,
  };
}

function variantsFixture(prefix: string): ModVariants {
  return {
    variants: ["Red", "Blue"].map((color) => ({
      id: `${prefix}-${color}`,
      mod_id: "mod-outfit",
      name: `${prefix} ${color}`,
      subpath: color,
    })),
    activeVariantId: prefix === "Old" ? "Old-Red" : null,
  };
}

const recoveryFixture = {
  reason: "Original deployment is locked",
  attemptedAt: "2026-10-07T12:00:00Z",
  attempts: 1,
  libraryPath: "C:\\GMM\\library\\gimi\\mod-outfit",
  stagedPath: "C:\\GMM\\library\\gimi\\.gmm-reinstall-stage",
  quarantinePath: "C:\\GMM\\library\\gimi\\.gmm-delete-old",
  junctionWithdrawn: true,
  junctionWithdrawalError: null,
};

let responses: Record<string, unknown>;
let mutations: Record<string, (args: Record<string, unknown>) => unknown>;

beforeEach(() => {
  responses = {
    attention_status: {
      safeToProceed: true,
      reinstalls: [], enabledTransitions: [], importerEvacuations: [],
      stagedLibraryOperations: [], sessionLaunches: [], activeSession: null,
      libraryAudits: [], libraryRootOverlaps: [], modPathOverlaps: [],
    },
    is_onboarding_complete: { complete: true, skipped: false },
    list_supported_games: [{ code: "gimi", displayName: "Genshin Impact" }],
    current_session: null,
    clean_stale_session: null,
    interrupted_session_launches: [],
    get_game_install_path: "C:/Game",
    get_game_path_status: { game: "gimi", savedPath: "C:/Game", gamePath: { status: "readable" }, modelImporter: { status: "installed" } },
    get_startup_reconcile_status: { finished: true, failures: [] },
    check_importer_update: { available: false, installedVersion: null, pinned: false },
    get_importer_evacuation_recovery: null,
    get_proxy_config: { url: null, username: null, passwordSet: false },
    mod_updates_globally_enabled: true,
    list_mod_updates: [{
      modId: "mod-outfit", name: "Test Outfit", installedVersion: "1.0",
      upstreamVersion: "2.0", upstreamAhead: true, updateCheckEnabled: true,
    }],
    get_library_paths: {
      defaultRoot: "C:\\GMM\\library", rootOverride: null,
      effectiveRoot: "C:\\GMM\\library", perGameOverrides: {},
      perGameEffective: { gimi: "C:\\GMM\\library\\gimi" },
    },
    audit_library: { unreferenced: [], duplicates: [], totalBytes: 0 },
    list_mods: [modFixture()],
    list_variants: variantsFixture("Old"),
    detect_conflicts: { conflicts: [], omitted: [], per_mod_count: {} },
    av_guidance: { headline: "Launch blocked", body: "", exclusionSteps: [], sentinel: "AV-PATTERN: " },
  };
  mutations = {};
  openDialog.mockResolvedValue("C:\\Downloads\\outfit.zip");
  invoke.mockImplementation(async (command: string, args: Record<string, unknown>) =>
    mutations[command] ? mutations[command](args) : responses[command] ?? null,
  );
});

it.each([false, true])("shows deployment recovery immediately after a failed toggle (initially enabled: %s)", async (enabled) => {
  responses.list_mods = [modFixture(enabled)];
  mutations.set_mod_enabled = () => {
    responses.list_mods = [{
      ...modFixture(enabled),
      enabled_transition_recovery: {
        intendedEnabled: !enabled, reason: "Junction recovery is locked",
        attemptedAt: "2026-10-07T12:00:00Z", attempts: 1,
        junctionPath: "C:\\Game\\Mods\\Test Outfit", ownerUncertain: false,
      },
    }];
    throw new Error("Toggle failed");
  };
  renderApp();
  await userEvent.click(await screen.findByRole("checkbox", { name: enabled ? "Enabled" : "Disabled" }));
  await screen.findByText("Toggle failed");
  expect(await screen.findByRole("region", { name: "Interrupted enable or disable recovery for Test Outfit" })).toHaveTextContent("Junction recovery is locked");
  expect(screen.getByRole("checkbox", { name: /Unavailable · deployment recovery pending/ })).toBeDisabled();
});

it("replaces obsolete Variant choices after applying a Mod update", async () => {
  mutations.apply_mod_update = () => {
    responses.list_variants = variantsFixture("New");
    responses.list_mods = [{ ...modFixture(), version: "2.0" }];
    responses.list_mod_updates = [];
  };
  renderApp();
  // Variants mount after the startup and Mod queries, with no separate loading indicator.
  // Allow startup headroom only here; keep the update assertions on their normal budget.
  expect(await screen.findByRole("radio", { name: "Old Red" }, { timeout: 10_000 })).toBeChecked();
  await userEvent.click(await screen.findByRole("button", { name: "Update → 2.0" }));
  const newVariant = await screen.findByRole("radio", { name: "New Blue" });
  expect(screen.queryByRole("radio", { name: "Old Red" })).not.toBeInTheDocument();
  expect(screen.queryByRole("radio", { name: "Old Blue" })).not.toBeInTheDocument();
  expect(screen.getByRole("radio", { name: "New Red" })).not.toBeChecked();
  await userEvent.click(newVariant);
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("set_active_variant", {
    modId: "mod-outfit", variantId: "New-Blue", game: "gimi",
  }));
});

it("refreshes displayed Conflicts after applying a Mod update", async () => {
  responses.list_mods = [modFixture(true)];
  mutations.apply_mod_update = () => {
    responses.detect_conflicts = {
      conflicts: [{ hash: "deadbeef", mod_ids: ["mod-outfit", "mod-other"], sections: [] }],
      omitted: [], per_mod_count: { "mod-outfit": 1 },
    } satisfies ConflictReport;
  };
  renderApp();
  await screen.findByRole("radio", { name: "Old Red" });
  await userEvent.click(await screen.findByRole("button", { name: "Update → 2.0" }));
  expect(await screen.findByRole("button", { name: "1 conflict" })).toBeInTheDocument();
});

it("disables obsolete Variant choices while the replacement list is loading", async () => {
  let updating = false;
  let finishRefresh!: (variants: ModVariants) => void;
  const replacement = new Promise<ModVariants>((resolve) => { finishRefresh = resolve; });
  mutations.list_variants = () => updating ? replacement : variantsFixture("Old");
  mutations.apply_mod_update = () => { updating = true; };
  renderApp();
  const obsolete = await screen.findByRole("radio", { name: "Old Blue" });
  await userEvent.click(await screen.findByRole("button", { name: "Update → 2.0" }));
  try {
    await waitFor(() => expect(obsolete).toBeDisabled());
  } finally {
    finishRefresh(variantsFixture("New"));
  }
  expect(await screen.findByRole("radio", { name: "New Blue" })).toBeEnabled();
});

it("shows a Mod update failure even if its update badge disappears", async () => {
  mutations.apply_mod_update = () => {
    responses.list_mod_updates = [];
    throw new Error("Update rollback failed");
  };
  renderApp();
  await userEvent.click(await screen.findByRole("button", { name: "Update → 2.0" }));
  expect(await screen.findByText("Update rollback failed")).toBeInTheDocument();
});

it("shows newly recorded reinstall recovery after a Mod update fails", async () => {
  mutations.apply_mod_update = () => {
    responses.list_mods = [{ ...modFixture(), reinstall_recovery: recoveryFixture }];
    throw new Error("Update rollback failed");
  };
  renderApp();
  await userEvent.click(await screen.findByRole("button", { name: "Update → 2.0" }));
  expect(await screen.findByRole("region", { name: "Interrupted reinstall recovery for Test Outfit" })).toHaveTextContent("Original deployment is locked");
  expect(screen.getByText("Update rollback failed")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Update → 2.0" })).toBeDisabled();
});

it.each(["retry", "retireProducer"] as const)("refreshes changed Model Importer recovery after a failed %s", async (action) => {
  const recovery: ImporterEvacuationRecovery = {
    reason: "Previous importer obstruction", attemptedAt: "2026-10-07T12:00:00Z", attempts: 1,
    gamePath: "C:\\Game", backupPath: "C:\\GMM\\backups\\gimi\\old",
    ownerUncertain: action === "retireProducer", action,
  };
  responses.get_importer_evacuation_recovery = recovery;
  mutations[action === "retry" ? "retry_importer_evacuation_recovery" : "retire_interrupted_importer_evacuation"] = () => {
    responses.get_importer_evacuation_recovery = { ...recovery, action: "retry", ownerUncertain: false, reason: "New recorded importer obstruction", attempts: 2 };
    throw new Error("Importer recovery failed");
  };
  renderApp();
  await userEvent.click(await screen.findByRole("button", { name: "Settings for Genshin Impact" }));
  await userEvent.click(await screen.findByRole("button", {
    name: action === "retry" ? "Retry Model Importer recovery" : /I confirmed no other GMM is changing this importer/,
  }));
  // Scoped to the dialog since #353: Settings is a modal dialog now, so the
  // main screen's own recovery notice for the same evacuation stays mounted
  // behind it and reports the same reason. This is what the Settings copy
  // shows, not whichever copy a document-wide query happened to reach.
  const settings = within(screen.getByRole("dialog", { name: "Settings" }));
  expect(await settings.findByText("New recorded importer obstruction")).toBeInTheDocument();
  expect(settings.queryByText("Previous importer obstruction")).not.toBeInTheDocument();
  // The stale reason is gone from the whole window, not merely re-scoped out
  // of view: the main screen reads the same refreshed query.
  expect(screen.queryByText("Previous importer obstruction")).not.toBeInTheDocument();
});

it.each(["adopt", "zip", "drop", "gamebanana"] as const)("refreshes the orphan Library audit after a failed %s import", async (entry) => {
  mutations[entry === "adopt" ? "adopt_folder" : entry === "gamebanana" ? "import_gamebanana" : "import_zip"] = () => {
    responses.audit_library = {
      unreferenced: [{ path: "C:\\GMM\\library\\gimi\\orphan", directoryName: "orphan", sizeBytes: 128 }],
      duplicates: [], totalBytes: 128,
    };
    throw new Error("Import failed; partial bytes retained");
  };
  const { container } = renderApp();
  await screen.findByRole("radio", { name: "Old Red" });
  await userEvent.click(screen.getByRole("button", { name: "Settings for Genshin Impact" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("audit_library", { game: "gimi" }));
  await userEvent.click(screen.getByRole("button", { name: "Close" }));
  if (entry === "drop") {
    const file = Object.assign(new File(["zip"], "outfit.zip"), { path: "C:\\Downloads\\outfit.zip" });
    fireEvent.drop(container.querySelector(".dropzone")!, { dataTransfer: { files: [file] } });
  } else {
    await userEvent.click(screen.getByRole("button", {
      name: entry === "adopt" ? "Adopt folder…" : entry === "zip" ? "Import ZIP…" : "Paste GameBanana URL…",
    }));
    if (entry !== "gamebanana") {
      await userEvent.click(screen.getByRole("button", { name: entry === "adopt" ? "Pick mod folder" : "Pick .zip file" }));
    }
  }
  await userEvent.type(screen.getByPlaceholderText(entry === "gamebanana"
    ? "https://gamebanana.com/mods/1234567 or a bare ID"
    : entry === "drop" ? "Display name" : "Display name (e.g. Hu Tao Skin)"), entry === "gamebanana" ? "123" : "Partial Outfit");
  const auditReads = invoke.mock.calls.filter(([command]) => command === "audit_library").length;
  await userEvent.click(screen.getByRole("button", { name: entry === "adopt" ? "Adopt" : "Import" }));
  await waitFor(() => expect(invoke.mock.calls.filter(([command]) => command === "audit_library").length).toBeGreaterThan(auditReads));
  expect(await screen.findByText("Import failed; partial bytes retained")).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Settings for Genshin Impact" }));
  expect(await screen.findByRole("region", { name: "Unreferenced Library folders and duplicate Mod records" })).toHaveTextContent("orphan");
});

it("shows a retained launch reservation immediately after launch fails", async () => {
  mutations.launch_game = () => {
    responses.interrupted_session_launches = [{ id: "launch-claim", game: "gimi", child_pid: null, started_at: "2026-10-07T12:00:00Z" }];
    throw new Error("Launch failed; reservation retained");
  };
  renderApp();
  await screen.findByRole("radio", { name: "Old Red" });
  await userEvent.click(screen.getByRole("button", { name: "Launch Genshin Impact" }));
  expect(await screen.findByRole("button", { name: "I confirmed the game is closed — retire reservation" })).toBeInTheDocument();
});

it("refreshes reinstall recovery even when its retry rejects", async () => {
  responses.list_mods = [{ ...modFixture(), reinstall_recovery: recoveryFixture }];
  responses.detect_conflicts = { conflicts: [{ hash: "recover-hash", mod_ids: ["mod-outfit", "other"] }], omitted: [], per_mod_count: { "mod-outfit": 1 } };
  mutations.retry_reinstall_recovery = () => {
    responses.detect_conflicts = { conflicts: [], omitted: [], per_mod_count: {} };
    responses.list_mods = [{ ...modFixture(), reinstall_recovery: { ...recoveryFixture, reason: "New recorded reinstall obstruction", attempts: 2 } }];
    throw new Error("Reinstall recovery failed");
  };
  renderApp();
  const warning = await screen.findByRole("region", { name: "Interrupted reinstall recovery for Test Outfit" });
  expect(screen.getByRole("button", { name: "1 conflict" })).toBeEnabled();
  await userEvent.click(within(warning).getByRole("button", { name: /Retry/ }));
  expect(await screen.findByText("New recorded reinstall obstruction")).toBeInTheDocument();
  await waitFor(() => expect(screen.queryByRole("button", { name: "1 conflict" })).not.toBeInTheDocument());
});


it.each(["root", "perGame"] as const)("refreshes persisted paths and Mod state after a failed Library %s move", async (field) => {
  mutations[field === "root" ? "set_library_root" : "set_library_path_for_game"] = () => {
    responses.get_library_paths = {
      defaultRoot: "C:\\GMM\\library", rootOverride: "D:\\Library",
      effectiveRoot: "D:\\Library", perGameOverrides: { gimi: "D:\\Library\\gimi" },
      perGameEffective: { gimi: "D:\\Library\\gimi" },
    };
    responses.list_mods = [modFixture(false)];
    throw new Error("Library move failed");
  };
  responses.list_mods = [modFixture(true)];
  renderApp();
  await screen.findByRole("checkbox", { name: "Enabled" });
  if (field === "root") await clickMenuItem(listen, "settings");
  else await userEvent.click(screen.getByRole("button", { name: "Settings for Genshin Impact" }));
  const library = await screen.findByRole("region", { name: field === "root" ? "Global Library root settings" : "Library for Genshin Impact" });
  await userEvent.click(field === "root"
    ? within(library).getByRole("button", { name: "Change global root…" })
    : within(library).getAllByRole("button", { name: "Change…" })[0]);
  expect(await screen.findByDisplayValue(field === "root" ? "D:\\Library" : "D:\\Library\\gimi")).toBeInTheDocument();
  expect(screen.getByText("Library move failed")).toBeVisible();
  await userEvent.click(screen.getByRole("button", { name: "Close" }));
  expect(await screen.findByRole("checkbox", { name: "Disabled" })).toBeEnabled();
});

it("shows partially recorded upstream versions after Check now fails", async () => {
  mutations.check_mod_updates_now = () => {
    responses.list_mod_updates = [{
      modId: "mod-outfit", name: "Test Outfit", installedVersion: "1.0",
      upstreamVersion: "3.0", upstreamAhead: true, updateCheckEnabled: true,
    }];
    throw new Error("Batch check failed");
  };
  renderApp();
  await screen.findByRole("button", { name: "Update → 2.0" });
  await userEvent.click(screen.getByRole("button", { name: "Check now" }));
  expect(await screen.findByRole("button", { name: "Update → 3.0" })).toBeInTheDocument();
  expect(screen.getByText("Batch check failed")).toBeInTheDocument();
});

it("refreshes Mod recovery changed during a failed Junction rebuild", async () => {
  mutations.rebuild_junctions = () => {
    responses.list_mods = [{ ...modFixture(), reinstall_recovery: recoveryFixture }];
    throw new Error("Rebuild failed");
  };
  renderApp();
  await screen.findByRole("radio", { name: "Old Red" });
  await userEvent.click(screen.getByRole("button", { name: "Settings for Genshin Impact" }));
  await userEvent.click(screen.getByRole("button", { name: "Rebuild junctions" }));
  expect(await screen.findByText("Rebuild failed")).toBeVisible();
  await userEvent.click(screen.getByRole("button", { name: "Close" }));
  expect(await screen.findByRole("region", { name: "Interrupted reinstall recovery for Test Outfit" })).toBeInTheDocument();
});


it.each(["install", "rollback"] as const)("refreshes the displayed Importer Origin after %s", async (action) => {
  mutations[action === "install" ? "install_importer" : "rollback_importer"] = () => {
    const origin = { kind: "gitHubRelease", owner: "Example", repo: "Restored-Package", asset_pattern: "Package.zip" };
    responses.importer_origin_status = {
      game: "gimi", displayName: "Genshin Impact",
      resolved: { state: "inEffect", origin, layer: "compiledInDefault" },
      installTarget: { state: "installed", ...origin }, installed: { state: "known", ...origin },
      userOverride: { state: "notSet" }, compiledDefault: origin, proposal: null,
      dismissed: [], dismissalsError: null, recommendationsEnabled: true,
      recommendationsUnusableReason: null,
    };
    return action === "install" ? { sha256: "abc", rewrote_files: [], backup_dir: null } : null;
  };
  renderApp();
  await screen.findByRole("radio", { name: "Old Red" });
  await userEvent.click(screen.getByRole("button", { name: "Settings for Genshin Impact" }));
  await userEvent.click(screen.getByRole("button", { name: action === "install" ? "Reinstall importer" : "Roll back importer" }));
  expect((await screen.findAllByText("Example/Restored-Package"))[0]).toBeInTheDocument();
});


const overlapRefusal = String.raw`GMM cannot use "D:\\Rejected Path" as a Library root because it overlaps the Model Importer backup tree at "C:\\GMM\\backups". Importer backups and their sidecar markers are app-owned bookkeeping that GMM writes outside the Library writer fence, so the two trees must stay disjoint: a Library root may neither sit inside the backup tree nor contain it. Choose a Library root that does not overlap "C:\\GMM\\backups". No Library bytes were moved.`;

const pathFields = [
  { label: "Global Library root", command: "set_library_root", initial: "C:\\GMM\\library", picked: "Change global root…" },
  { label: "Genshin Impact Library path", command: "set_library_path_for_game", initial: "C:\\GMM\\library\\gimi", picked: "Change…" },
  { label: "Genshin Impact install path", command: "set_game_install_path", initial: "C:\\Games\\Genshin", picked: "Change…" },
];

it.each(pathFields)("applies typed $label only after Enter", async ({ label, command, initial }) => {
  responses.get_game_install_path = "C:\\Games\\Genshin";
  mutations[command] = ({ path }) => {
    const paths = responses.get_library_paths as { effectiveRoot: string; perGameEffective: Record<string, string> };
    if (command === "set_library_root") paths.effectiveRoot = path as string;
    else if (command === "set_library_path_for_game") paths.perGameEffective.gimi = path as string;
    else responses.get_game_install_path = path;
    return {};
  };
  renderApp();
  if (command === "set_library_root") await clickMenuItem(listen, "settings");
  else await userEvent.click(await screen.findByRole("button", { name: "Settings for Genshin Impact" }));
  const field = await screen.findByDisplayValue(initial);
  expect(field).not.toHaveAttribute("readonly");
  await userEvent.clear(field);
  await userEvent.type(field, "D:\\Typed Path");
  expect(field).toHaveValue("D:\\Typed Path");
  expect(invoke.mock.calls.filter(([name]) => name === command)).toHaveLength(0);
  await userEvent.type(field, "{Enter}");
  await waitFor(() => expect(invoke).toHaveBeenCalledWith(command, command === "set_library_root"
    ? { path: "D:\\Typed Path" } : { game: "gimi", path: "D:\\Typed Path" }));
  expect(await screen.findByLabelText(label)).toHaveValue("D:\\Typed Path");
  expect(invoke.mock.calls.filter(([name]) => name === command)).toHaveLength(1);
});

it.each(pathFields)("shows backend refusal for typed $label", async ({ label, command, initial }) => {
  responses.get_game_install_path = "C:\\Games\\Genshin";
  const message = command === "set_library_root"
    ? overlapRefusal
    : "This path does not exist or cannot be used.";
  mutations[command] = () => { throw { kind: "other", message }; };
  renderApp();
  if (command === "set_library_root") await clickMenuItem(listen, "settings");
  else await userEvent.click(await screen.findByRole("button", { name: "Settings for Genshin Impact" }));
  const field = await screen.findByDisplayValue(initial);
  expect(field).not.toHaveAttribute("readonly");
  await userEvent.clear(field);
  await userEvent.type(field, "D:\\Rejected Path");
  await userEvent.click(screen.getByRole("button", { name: `Apply ${label}` }));
  expect(await screen.findByText(message)).toBeInTheDocument();
  expect(invoke).toHaveBeenCalledWith(command, command === "set_library_root"
    ? { path: "D:\\Rejected Path" } : { game: "gimi", path: "D:\\Rejected Path" });
});


it.each(pathFields)("keeps the folder picker working for $label", async ({ label, command, picked }) => {
  responses.get_game_install_path = "C:\\Games\\Genshin";
  openDialog.mockResolvedValue("E:\\Picked Path");
  mutations[command] = ({ path }) => {
    const paths = responses.get_library_paths as { effectiveRoot: string; perGameEffective: Record<string, string> };
    if (command === "set_library_root") paths.effectiveRoot = path as string;
    else if (command === "set_library_path_for_game") paths.perGameEffective.gimi = path as string;
    else responses.get_game_install_path = path;
    return {};
  };
  renderApp();
  if (command === "set_library_root") await clickMenuItem(listen, "settings");
  else await userEvent.click(await screen.findByRole("button", { name: "Settings for Genshin Impact" }));
  const field = await screen.findByLabelText(label);
  await userEvent.type(field, "unfinished");
  await userEvent.click(within(field.closest(".row") as HTMLElement).getByRole("button", { name: picked }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith(command, command === "set_library_root"
    ? { path: "E:\\Picked Path" } : { game: "gimi", path: "E:\\Picked Path" }));
  expect(openDialog).toHaveBeenCalledWith({ directory: true, multiple: false });
  await waitFor(() => expect(screen.getByLabelText(label)).toHaveValue("E:\\Picked Path"));
});

it("does not apply a path when the picker is cancelled", async () => {
  openDialog.mockResolvedValue(null);
  renderApp();
  await clickMenuItem(listen, "settings");
  const field = await screen.findByLabelText("Global Library root");
  await userEvent.type(field, "draft");
  await userEvent.click(screen.getByRole("button", { name: "Change global root…" }));
  expect(invoke.mock.calls.filter(([name]) => name === "set_library_root")).toHaveLength(0);
  expect(field).toHaveValue("C:\\GMM\\librarydraft");
  await userEvent.click(screen.getByRole("button", { name: "Close" }));
  expect(await screen.findByDisplayValue("C:\\GMM\\logs")).toHaveAttribute("readonly");
});


it("shows the persisted proxy configuration after a partially failed save", async () => {
  mutations.set_proxy_config = () => {
    responses.get_proxy_config = { url: "http://persisted-proxy:8080", username: null, passwordSet: false };
    throw new Error("Proxy settings partly saved");
  };
  renderApp();
  await clickMenuItem(listen, "settings");
  const url = await screen.findByPlaceholderText("proxy URL");
  await userEvent.type(url, "http://draft-proxy:8080");
  await userEvent.click(screen.getByRole("button", { name: "Save" }));
  expect(await screen.findByDisplayValue("http://persisted-proxy:8080")).toBeInTheDocument();
  expect(screen.getByText("Proxy settings partly saved")).toBeInTheDocument();
});

it("refreshes partial onboarding completion while retaining the setup error", async () => {
  responses.is_onboarding_complete = { complete: false, skipped: false };
  mutations.mark_onboarding_complete = () => {
    responses.is_onboarding_complete = { complete: true, skipped: false };
    throw new Error("Setup completion partly saved");
  };
  renderApp();
  await userEvent.click(await screen.findByRole("button", { name: "Skip setup" }));
  expect(await screen.findByRole("heading", { name: "Genshin Impact" })).toBeInTheDocument();
  expect(screen.getByText("Setup completion partly saved")).toBeInTheDocument();
});

it("refreshes partial onboarding reset while retaining the setup error", async () => {
  mutations.reset_onboarding = () => {
    responses.is_onboarding_complete = { complete: false, skipped: false };
    throw new Error("Setup reset partly saved");
  };
  renderApp();
  await screen.findByRole("heading", { name: "Genshin Impact" });
  // Help → Run setup again is a native menu item since #351.
  await clickMenuItem(listen, "run-setup");
  expect(await screen.findByRole("heading", { name: "First-run setup" })).toBeInTheDocument();
  expect(screen.getByText("Setup reset partly saved")).toBeInTheDocument();
});


it("shows the update-check row of a newly imported GameBanana Mod", async () => {
  mutations.import_gamebanana = () => {
    const imported = { ...modFixture(), id: "mod-new", name: "New Outfit" };
    responses.list_mods = [imported];
    responses.list_mod_updates = [{
      modId: "mod-new", name: "New Outfit", installedVersion: "1.0",
      upstreamVersion: "4.0", upstreamAhead: true, updateCheckEnabled: true,
    }];
    return imported;
  };
  renderApp();
  await screen.findByRole("button", { name: "Update → 2.0" });
  await userEvent.click(screen.getByRole("button", { name: "Paste GameBanana URL…" }));
  await userEvent.type(screen.getByPlaceholderText("https://gamebanana.com/mods/1234567 or a bare ID"), "123");
  await userEvent.click(screen.getByRole("button", { name: "Import" }));
  expect(await screen.findByRole("button", { name: "Update → 4.0" })).toBeInTheDocument();
});

it("keeps row recovery, Variants, Conflict disclosures and Mod updates under separate gates", async () => {
  responses.list_mods = [modFixture(), {
    ...modFixture(), id: "broken-mod", name: "Broken Outfit", source: "manual",
    gamebanana_id: null, source_url: null, reinstall_recovery: recoveryFixture,
  }];
  responses.detect_conflicts = {
    conflicts: [{ hash: "shared-hash", mod_ids: ["mod-outfit", "broken-mod"] }],
    omitted: [], per_mod_count: { "mod-outfit": 1, "broken-mod": 1 },
  };
  mutations.set_active_variant = () => { throw new Error("Variant change denied"); };
  mutations.apply_mod_update = () => { throw new Error("Update refused"); };
  mutations.retry_reinstall_recovery = () => { throw new Error("Recovery still locked"); };
  const { container } = renderApp();
  await screen.findByRole("region", { name: "Interrupted reinstall recovery for Broken Outfit" });
  const healthy = container.querySelector('[data-mod-id="mod-outfit"]')! as HTMLElement;
  const broken = container.querySelector('[data-mod-id="broken-mod"]')! as HTMLElement;
  const recovery = within(broken).getByRole("region", { name: "Interrupted reinstall recovery for Broken Outfit" });
  const noticeSlot = screen.getByRole("region", { name: "Game and operation notices" });
  expect(noticeSlot).not.toContainElement(recovery);
  expect(within(broken).getByRole("checkbox", { name: "Unavailable" })).toBeDisabled();
  expect(within(broken).getByRole("radio", { name: "Old Blue" })).toBeDisabled();
  expect(within(healthy).getByRole("checkbox", { name: "Disabled" })).toBeEnabled();
  await userEvent.click(within(healthy).getByRole("button", { name: "1 conflict" }));
  expect(healthy).toHaveTextContent("shared-hash shared with 1 other mod.");
  await userEvent.click(within(healthy).getByRole("radio", { name: "Old Blue" }));
  expect(await within(healthy).findByText("Variant change denied")).toBeVisible();
  expect(within(healthy).getByRole("button", { name: "Update → 2.0" })).toBeEnabled();
  await userEvent.click(within(healthy).getByRole("button", { name: "Update → 2.0" }));
  expect(await within(healthy).findByText("Update refused")).toBeVisible();
  expect(within(healthy).getByRole("radio", { name: "Old Blue" })).toBeEnabled();
  expect(within(healthy).getByRole("checkbox", { name: "Disabled" })).toBeEnabled();
  expect(healthy).toHaveTextContent("shared-hash shared with 1 other mod.");
  expect(within(recovery).getByRole("button", { name: /Retry/ })).toBeEnabled();
  await userEvent.click(within(recovery).getByRole("button", { name: /Retry/ }));
  expect(await screen.findByText("Recovery still needs intervention for Broken Outfit: Recovery still locked")).toBeVisible();
  expect(within(healthy).getByRole("button", { name: "Update → 2.0" })).toBeEnabled();
  expect(within(healthy).getByRole("radio", { name: "Old Blue" })).toBeEnabled();
  expect(invoke).toHaveBeenCalledWith("set_active_variant", { modId: "mod-outfit", variantId: "Old-Blue", game: "gimi" });
  expect(invoke).toHaveBeenCalledWith("apply_mod_update", { modId: "mod-outfit" });
  expect(invoke).toHaveBeenCalledWith("retry_reinstall_recovery", { modId: "broken-mod" });
});

it.each(["quarantine", "session"])("keeps navigation, Settings, diagnostics and another Game usable with %s attention", async (kind) => {
  responses.list_supported_games = [
    { code: "gimi", displayName: "Genshin Impact" },
    { code: "srmi", displayName: "Honkai: Star Rail" },
  ];
  const session = { game: "gimi", pid: 321, started_at: "2026-10-16" };
  responses.current_session = kind === "session" ? session : null;
  responses.list_mods = [modFixture(), {
    ...modFixture(), id: "broken-mod", name: "Broken Outfit",
    reinstall_recovery: recoveryFixture,
  }];
  mutations.list_mods = ({ game }) => game === "gimi" ? responses.list_mods : [{
    ...modFixture(), game: "srmi", id: "other-game-mod", name: "Other Game Outfit",
  }];
  responses.attention_status = {
    ...(responses.attention_status as object), safeToProceed: false,
    activeSession: kind === "session" ? { ...session, startedAt: session.started_at } : null,
    reinstalls: kind === "quarantine" ? [{ game: "gimi", modId: "broken-mod", libraryPath: recoveryFixture.libraryPath, recovery: recoveryFixture }] : [],
  };
  const { container } = renderApp();
  await screen.findByRole("button", { name: /Needs attention/ });
  const healthy = container.querySelector<HTMLElement>('[data-mod-id="mod-outfit"]')!;
  const broken = container.querySelector<HTMLElement>('[data-mod-id="broken-mod"]')!;
  expect(within(broken).getByRole("checkbox", { name: "Unavailable" })).toBeDisabled();
  const toggle = within(healthy).getByRole("checkbox", { name: "Disabled" });
  if (kind === "session") {
    expect(toggle).toBeDisabled();
    expect(screen.getByRole("button", { name: "Game running…" })).toBeDisabled();
  } else {
    expect(toggle).toBeEnabled();
    // A clean sibling still reaches its backend gate despite the loud aggregate.
    mutations.set_mod_enabled = () => { throw new Error("Sibling backend refusal"); };
    await userEvent.click(toggle);
    expect(await screen.findByText("Sibling backend refusal")).toBeVisible();
  }
  const otherGame = screen.getByRole("tab", { name: "Honkai: Star Rail" });
  expect(otherGame).toBeEnabled();
  await userEvent.click(otherGame);
  expect(await screen.findByRole("heading", { name: "Honkai: Star Rail", level: 1 })).toBeVisible();
  await screen.findByText("Other Game Outfit");
  const otherMod = within(container.querySelector<HTMLElement>('[data-mod-id="other-game-mod"]')!)
    .getByRole("checkbox", { name: "Disabled" });
  if (kind === "session") expect(otherMod).toBeDisabled();
  else {
    expect(otherMod).toBeEnabled();
    expect(screen.getByRole("button", { name: "Launch Honkai: Star Rail" })).toBeEnabled();
  }
  await userEvent.click(screen.getByRole("button", { name: "Settings for Honkai: Star Rail" }));
  const settings = screen.getByRole("dialog", { name: "Settings" });
  expect(await within(settings).findByRole("textbox", { name: "Honkai: Star Rail install path" })).toBeEnabled();
  await userEvent.click(within(settings).getByRole("button", { name: /GMM settings/ }));
  expect(await within(settings).findByRole("textbox", { name: "Global Library root" })).toBeEnabled();
  const diagnostics = await within(settings).findByRole("button", { name: "Export diagnostics bundle" });
  expect(diagnostics).toBeEnabled();
  await userEvent.click(diagnostics);
  expect(await within(settings).findByText("Export cancelled.")).toBeVisible();
  await userEvent.click(within(settings).getByRole("button", { name: "Close" }));
  expect(screen.getByRole("tab", { name: "Honkai: Star Rail" })).toHaveAttribute("aria-selected", "true");
  expect(screen.getByRole("button", { name: "Settings for Honkai: Star Rail" })).toHaveFocus();
});

it("retains recovery feedback, focus and mounted announcements after a row clears, refreshes and a Settings visit", async () => {
  responses.list_mods = [{ ...modFixture(), reinstall_recovery: recoveryFixture }];
  mutations.retry_reinstall_recovery = () => {
    responses.list_mods = [modFixture()];
    throw new Error("Recovery completed only partially");
  };
  const { container, client } = renderApp();
  const warning = await screen.findByRole("region", { name: "Interrupted reinstall recovery for Test Outfit" });
  const section = warning.closest<HTMLElement>("section.card")!;
  const live = section.querySelector<HTMLElement>('[role="alert"].reinstall-action-notice')!;
  expect(live).toBeInTheDocument();
  await userEvent.click(within(warning).getByRole("button", { name: /Retry/ }));
  const feedback = "Recovery still needs intervention for Test Outfit: Recovery completed only partially";
  expect(await screen.findByText(feedback)).toBeVisible();
  await waitFor(() => expect(screen.queryByRole("region", { name: "Interrupted reinstall recovery for Test Outfit" })).not.toBeInTheDocument());
  expect(section).toHaveFocus();
  expect(live).toHaveTextContent(feedback);
  await client.refetchQueries({ queryKey: ["mods", "gimi"] });
  expect(live).toHaveTextContent(feedback);
  await userEvent.click(screen.getByRole("button", { name: "Settings for Genshin Impact" }));
  const settings = screen.getByRole("dialog", { name: "Settings" });
  await userEvent.click(within(settings).getByRole("button", { name: /GMM settings/ }));
  await userEvent.click(within(settings).getByRole("button", { name: "Close" }));
  expect(screen.getByRole("button", { name: "Settings for Genshin Impact" })).toHaveFocus();
  expect(container).toContainElement(live);
  expect(live).toHaveTextContent(feedback);
  expect(screen.getByText(feedback)).toBeVisible();
});


it("refreshes Game Session context after Launch succeeds", async () => {
  mutations.launch_game = () => {
    const session = { game: "gimi", pid: 321, started_at: "2026-10-11" };
    responses.current_session = session;
    return session;
  };
  renderApp();
  await screen.findByRole("radio", { name: "Old Red" });
  await userEvent.click(screen.getByRole("button", { name: "Launch Genshin Impact" }));
  expect(await screen.findByText("GIMI is running")).toBeVisible();
  expect(screen.getByRole("region", { name: "Global status" })).toHaveTextContent("GIMI is running");
  expect(screen.getByRole("button", { name: "Game running…" })).toBeDisabled();
  expect(screen.getByRole("checkbox", { name: "Disabled" })).toBeDisabled();
});

it.each([false, true])("refreshes this Game's Mods and Conflicts after changing enabled state from %s", async (enabled) => {
  responses.list_mods = [modFixture(enabled)];
  responses.detect_conflicts = { conflicts: [{ hash: "toggle-hash", mod_ids: ["mod-outfit", "other"] }], omitted: [], per_mod_count: { "mod-outfit": 1 } };
  mutations.set_mod_enabled = () => {
    responses.list_mods = [modFixture(!enabled)];
    responses.detect_conflicts = { conflicts: [], omitted: [], per_mod_count: {} };
    throw new Error("Toggle partly persisted");
  };
  renderApp();
  await screen.findByRole("radio", { name: "Old Red" });
  expect(screen.getByRole("button", { name: "1 conflict" })).toBeEnabled();
  await userEvent.click(screen.getByRole("checkbox", { name: enabled ? "Enabled" : "Disabled" }));
  expect(await screen.findByText("Toggle partly persisted")).toBeVisible();
  await waitFor(() => expect(screen.getByRole("checkbox", { name: enabled ? "Disabled" : "Enabled" })).toBeEnabled());
  await waitFor(() => expect(screen.queryByRole("button", { name: "1 conflict" })).not.toBeInTheDocument());
  expect(invoke).toHaveBeenCalledWith("set_mod_enabled", { id: "mod-outfit", enabled: !enabled, game: "gimi" });
});

it.each(["empty", "populated"] as const)("keeps Mod updates after the %s content", async (layout) => {
  responses.list_mods = layout === "empty" ? [] : [modFixture()];
  responses.list_mod_updates = [];
  renderApp();
  const content = (await screen.findByRole("heading", { name: layout === "empty" ? "Add Mods for Genshin Impact" : "Mods (1)" })).closest("section")!;
  const updates = screen.getByRole("heading", { name: "Mod updates" }).closest("section")!;
  expect(content.compareDocumentPosition(updates) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
});

const importCases = (["empty", "populated"] as const).flatMap((layout) =>
  (["adopt", "zip", "drop", "gamebanana"] as const).flatMap((entry) =>
    (["success", "failure"] as const).map((outcome) => ({ layout, entry, outcome }))),
);
it.each(importCases)("settles Mods, Library audit and Mod updates for $entry in $layout on $outcome", async ({ layout, entry, outcome }) => {
  responses.list_supported_games = [
    { code: "gimi", displayName: "Genshin Impact" },
    { code: "srmi", displayName: "Honkai: Star Rail" },
  ];
  const initial = layout === "empty" ? [] : [{ ...modFixture(), game: "srmi" }];
  responses.list_mod_updates = [];
  let changed = false;
  mutations.list_mods = (args) => args.game === "srmi" ? changed ? [{ ...modFixture(), id: "mod-new", name: "Imported Star Rail Outfit", game: "srmi" }] : initial : [];
  const command = entry === "adopt" ? "adopt_folder" : entry === "gamebanana" ? "import_gamebanana" : "import_zip";
  mutations[command] = () => {
    changed = true;
    responses.list_mod_updates = [{ modId: "mod-new", name: "Imported Star Rail Outfit", installedVersion: "1.0", upstreamVersion: "4.0", upstreamAhead: true, updateCheckEnabled: true }];
    responses.audit_library = { unreferenced: [{ path: "C:/Library/srmi/partial", directoryName: "partial", sizeBytes: 128 }], duplicates: [], totalBytes: 128 };
    if (outcome === "failure") throw new Error("Import partially persisted");
    return { ...modFixture(), id: "mod-new", game: "srmi", name: "Imported Star Rail Outfit" };
  };
  const { container } = renderApp();
  await userEvent.click(await screen.findByRole("tab", { name: "Honkai: Star Rail" }));
  const heading = await screen.findByRole("heading", { name: layout === "empty" ? "Add Mods for Honkai: Star Rail" : "Mods (1)" });
  if (layout === "empty") expect(within(heading.closest("section")!).getAllByRole("heading", { level: 3 }).map((item) => item.textContent)).toEqual([
    "Adopt for Honkai: Star Rail", "Import ZIP for Honkai: Star Rail", "GameBanana for Honkai: Star Rail",
  ]);
  // Visit Settings once so its production audit observer has a fresh cached result.
  await userEvent.click(screen.getByRole("button", { name: "Settings for Honkai: Star Rail" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("audit_library", { game: "srmi" }));
  await userEvent.click(screen.getByRole("button", { name: "Close" }));
  if (entry === "drop") {
    fireEvent.drop(container.querySelector(".dropzone")!, { dataTransfer: { files: [Object.assign(new File(["zip"], "outfit.zip"), { path: "C:/Downloads/outfit.zip" })] } });
  } else {
    await userEvent.click(screen.getByRole("button", { name: entry === "adopt" ? "Adopt folder…" : entry === "zip" ? "Import ZIP…" : "Paste GameBanana URL…" }));
    if (entry !== "gamebanana") await userEvent.click(screen.getByRole("button", { name: entry === "adopt" ? "Pick mod folder" : "Pick .zip file" }));
  }
  const field = screen.getByPlaceholderText(entry === "gamebanana" ? "https://gamebanana.com/mods/1234567 or a bare ID" : entry === "drop" ? "Display name" : "Display name (e.g. Hu Tao Skin)");
  await userEvent.clear(field);
  await userEvent.type(field, entry === "gamebanana" ? "123" : "Imported Star Rail Outfit");
  const auditReads = invoke.mock.calls.filter(([name, args]) => name === "audit_library" && args.game === "srmi").length;
  await userEvent.click(screen.getByRole("button", { name: entry === "adopt" ? "Adopt" : "Import" }));
  expect(invoke).toHaveBeenCalledWith(command, { args: entry === "gamebanana" ? { game: "srmi", urlOrId: "123" } : {
    game: "srmi", [entry === "adopt" ? "sourcePath" : "zipPath"]: entry === "drop" ? "C:/Downloads/outfit.zip" : "C:\\Downloads\\outfit.zip", name: "Imported Star Rail Outfit",
  } });
  expect(await screen.findByText("Imported Star Rail Outfit")).toBeVisible();
  expect(await screen.findByRole("button", { name: "Update → 4.0" })).toBeVisible();
  await waitFor(() => expect(invoke.mock.calls.filter(([name, args]) => name === "audit_library" && args.game === "srmi").length).toBeGreaterThan(auditReads));
  if (outcome === "failure") expect(await screen.findByText("Import partially persisted")).toBeVisible();
  else expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Settings for Honkai: Star Rail" }));
  expect(await screen.findByRole("region", { name: "Unreferenced Library folders and duplicate Mod records" })).toHaveTextContent("partial");
});

it("lands on the installed Game after finishing the real setup wizard", async () => {
  responses.is_onboarding_complete = { complete: false, skipped: false };
  responses.list_supported_games = [
    { code: "gimi", displayName: "Genshin Impact" },
    { code: "srmi", displayName: "Honkai: Star Rail" },
  ];
  responses.detect_all_games = [{ code: "srmi", displayName: "Honkai: Star Rail", detectedPath: "C:/StarRail" }];
  mutations.get_game_install_path = (args) => args.game === "srmi" ? "C:/StarRail" : null;
  responses.list_mods = [];
  responses.list_mod_updates = [];
  mutations.mark_onboarding_complete = () => { responses.is_onboarding_complete = { complete: true, skipped: false }; };
  renderApp();
  await userEvent.click(await screen.findByRole("checkbox", { name: "I've read the AV note" }));
  await userEvent.click(screen.getByRole("button", { name: "Continue →" }));
  await screen.findByRole("heading", { name: "Find your games" });
  await userEvent.click(screen.getByRole("button", { name: "Continue →" }));
  await screen.findByRole("heading", { name: "Where should GMM keep your Library?" });
  await userEvent.click(screen.getByRole("button", { name: "Continue →" }));
  await userEvent.click(await screen.findByRole("button", { name: "Install selected" }));
  await screen.findByText("✓ Done");
  await userEvent.click(screen.getByRole("button", { name: "Finish →" }));
  expect(await screen.findByRole("tab", { name: "Honkai: Star Rail" })).toHaveAttribute("aria-selected", "true");
  expect(screen.queryByRole("tab", { name: "Genshin Impact" })).not.toBeInTheDocument();
  expect(await screen.findByRole("heading", { name: "Add Mods for Honkai: Star Rail" })).toBeVisible();
  expect(screen.getByRole("button", { name: "Launch Honkai: Star Rail" })).toBeEnabled();
  expect(invoke).toHaveBeenCalledWith("mark_onboarding_complete", { skipped: false });
});

it("leads with Add game and setup access when completion has no saved install path", async () => {
  responses.get_game_install_path = null;
  renderApp();
  expect(await screen.findByText("No game install paths set. Add a game to get started.")).toBeVisible();
  expect(screen.getByRole("button", { name: "Add game" })).toBeEnabled();
  // Setup access moved to Help → Run setup again in the native menu (#351).
  expect(screen.queryByRole("button", { name: "Run setup again" })).not.toBeInTheDocument();
  expect(screen.queryByRole("heading", { name: "Genshin Impact" })).not.toBeInTheDocument();
  expect(screen.queryByRole("heading", { name: /^Mods \(/ })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Adopt folder…" })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Import ZIP…" })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Paste GameBanana URL…" })).not.toBeInTheDocument();
  expect(screen.queryByText("Test Outfit")).not.toBeInTheDocument();
  expect(screen.queryByText("Drop a .zip for Genshin Impact here")).not.toBeInTheDocument();
  expect(screen.queryByText(/setup complete/i)).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: /Launch/ })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Add game" }));
  await userEvent.click(screen.getByRole("button", { name: "Set up Genshin Impact" }));
  expect(await screen.findByRole("heading", { name: "Genshin Impact install path" })).toBeVisible();
  // A user with no saved path must still be able to re-run setup; the only
  // route left is the menu.
  await clickMenuItem(listen, "run-setup");
  expect(await screen.findByRole("heading", { name: "First-run setup" })).toBeVisible();
});

it("keeps Resume setup available after skipping and returns through the same wizard", async () => {
  responses.is_onboarding_complete = { complete: true, skipped: true };
  renderApp();
  await userEvent.click(await screen.findByRole("button", { name: "Resume setup" }));
  expect(await screen.findByRole("heading", { name: "First-run setup" })).toBeVisible();
  expect(invoke.mock.calls.some(([command]) => command === "reset_onboarding")).toBe(false);
  mutations.mark_onboarding_complete = () => { responses.is_onboarding_complete = { complete: true, skipped: true }; };
  await userEvent.click(screen.getByRole("button", { name: "Skip setup" }));
  expect(await screen.findByRole("button", { name: "Resume setup" })).toBeEnabled();
});
