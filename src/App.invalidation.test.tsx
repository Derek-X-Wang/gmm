import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";

import type { ConflictReport, ImporterEvacuationRecovery, ModVariants } from "./api";
import { renderWithQuery } from "./test/harness";

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
vi.mock("./ImporterOriginPanel", () => ({ ImporterOriginPanel: () => null }));
vi.mock("./LoaderVersionNote", () => ({ LoaderVersionNote: () => null }));

const { default: App } = await import("./App");

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
  renderWithQuery(<App />);
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
  renderWithQuery(<App />);
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
  renderWithQuery(<App />);
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
  renderWithQuery(<App />);
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
  renderWithQuery(<App />);
  await userEvent.click(await screen.findByRole("button", { name: "Update → 2.0" }));
  expect(await screen.findByText("Update rollback failed")).toBeInTheDocument();
});

it("shows newly recorded reinstall recovery after a Mod update fails", async () => {
  mutations.apply_mod_update = () => {
    responses.list_mods = [{ ...modFixture(), reinstall_recovery: recoveryFixture }];
    throw new Error("Update rollback failed");
  };
  renderWithQuery(<App />);
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
  renderWithQuery(<App />);
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
  const { container } = renderWithQuery(<App />);
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
  renderWithQuery(<App />);
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
  renderWithQuery(<App />);
  const warning = await screen.findByRole("region", { name: "Interrupted reinstall recovery for Test Outfit" });
  await userEvent.click(within(warning).getByRole("button", { name: /Retry/ }));
  expect(await screen.findByText("New recorded reinstall obstruction")).toBeInTheDocument();
});
