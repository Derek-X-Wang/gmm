import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";

import type { ConflictReport, ImporterEvacuationRecovery, ModVariants } from "./api";
import { makeQueryClient, renderWithQuery } from "./test/harness";

const { invoke, openDialog } = vi.hoisted(() => ({
  invoke: vi.fn(),
  openDialog: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => { }) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: openDialog, save: vi.fn() }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn() }));
vi.mock("./diagnostics", () => ({
  diagnosticsLogDir: vi.fn().mockResolvedValue("C:\\GMM\\logs"),
  exportDiagnosticsBundle: vi.fn(),
}));
vi.mock("./updater", () => ({ checkInteractively: vi.fn() }));
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
    get_game_install_path: null,
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
    detect_conflicts: { conflicts: [], per_mod_count: {} },
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
  expect(await screen.findByRole("radio", { name: "Old Red" })).toBeChecked();
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
      per_mod_count: { "mod-outfit": 1 },
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
  await userEvent.click(await screen.findByRole("button", {
    name: action === "retry" ? "Retry Model Importer recovery" : /I confirmed no other GMM is changing this importer/,
  }));
  expect(await screen.findByText("New recorded importer obstruction")).toBeInTheDocument();
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
  await userEvent.click(screen.getByRole("button", { name: entry === "adopt" ? "Adopt" : "Import" }));
  expect(await screen.findByRole("region", { name: "Unreferenced Library folders and duplicate Mod records" })).toHaveTextContent("orphan");
  expect(screen.getByText("Import failed; partial bytes retained")).toBeInTheDocument();
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
  mutations.retry_reinstall_recovery = () => {
    responses.list_mods = [{ ...modFixture(), reinstall_recovery: { ...recoveryFixture, reason: "New recorded reinstall obstruction", attempts: 2 } }];
    throw new Error("Reinstall recovery failed");
  };
  renderApp();
  const warning = await screen.findByRole("region", { name: "Interrupted reinstall recovery for Test Outfit" });
  await userEvent.click(within(warning).getByRole("button", { name: /Retry/ }));
  expect(await screen.findByText("New recorded reinstall obstruction")).toBeInTheDocument();
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
  const library = screen.getByRole("heading", { name: "Library" }).closest("section")!;
  await userEvent.click(field === "root"
    ? within(library).getByRole("button", { name: "Change global root…" })
    : within(library).getAllByRole("button", { name: "Change…" })[0]);
  expect(await screen.findByDisplayValue("D:\\Library")).toBeInTheDocument();
  expect(await screen.findByRole("checkbox", { name: "Disabled" })).toBeEnabled();
  expect(screen.getByText("Library move failed")).toBeInTheDocument();
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
  await userEvent.click(screen.getByRole("button", { name: "Rebuild junctions" }));
  expect(await screen.findByRole("region", { name: "Interrupted reinstall recovery for Test Outfit" })).toBeInTheDocument();
  expect(screen.getByText("Rebuild failed")).toBeInTheDocument();
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
  await userEvent.click(screen.getByRole("button", { name: action === "install" ? "Reinstall importer" : "Roll back importer" }));
  expect(await screen.findByText("Example/Restored-Package")).toBeInTheDocument();
});


const overlapRefusal = String.raw`GMM cannot use "D:\\Rejected Path" as a Library root because it overlaps the Model Importer backup tree at "C:\\GMM\\backups". Importer backups and their sidecar markers are app-owned bookkeeping that GMM writes outside the Library writer fence, so the two trees must stay disjoint: a Library root may neither sit inside the backup tree nor contain it. Choose a Library root that does not overlap "C:\\GMM\\backups". No Library bytes were moved.`;

const pathFields = [
  { label: "Global Library root", command: "set_library_root", initial: "C:\\GMM\\library", picked: "Change global root…" },
  { label: "gimi Library path", command: "set_library_path_for_game", initial: "C:\\GMM\\library\\gimi", picked: "Change…" },
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
  const field = await screen.findByLabelText("Global Library root");
  await userEvent.type(field, "draft");
  await userEvent.click(screen.getByRole("button", { name: "Change global root…" }));
  expect(invoke.mock.calls.filter(([name]) => name === "set_library_root")).toHaveLength(0);
  expect(field).toHaveValue("C:\\GMM\\librarydraft");
  expect(await screen.findByDisplayValue("C:\\GMM\\logs")).toHaveAttribute("readonly");
});


it("shows the persisted proxy configuration after a partially failed save", async () => {
  mutations.set_proxy_config = () => {
    responses.get_proxy_config = { url: "http://persisted-proxy:8080", username: null, passwordSet: false };
    throw new Error("Proxy settings partly saved");
  };
  renderApp();
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
  expect(await screen.findByRole("heading", { name: "GMM — Genshin Impact" })).toBeInTheDocument();
  expect(screen.getByText("Setup completion partly saved")).toBeInTheDocument();
});

it("refreshes partial onboarding reset while retaining the setup error", async () => {
  mutations.reset_onboarding = () => {
    responses.is_onboarding_complete = { complete: false, skipped: false };
    throw new Error("Setup reset partly saved");
  };
  renderApp();
  await userEvent.click(await screen.findByRole("button", { name: "Run setup again" }));
  expect(await screen.findByRole("heading", { name: "GMM — first-run setup" })).toBeInTheDocument();
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
