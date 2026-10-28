import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import { makeQueryClient, renderWithQuery } from "./test/harness";

const { invoke, listen } = vi.hoisted(() => ({
  invoke: vi.fn(), listen: vi.fn().mockResolvedValue(() => {}),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn() }));
vi.mock("./diagnostics", () => ({ diagnosticsLogDir: vi.fn().mockResolvedValue("C:/GMM/logs"), exportDiagnosticsBundle: vi.fn() }));
vi.mock("./updater", () => {
  const idle = { kind: "idle" };
  return { updater: {
    getSnapshot: () => idle, subscribe: () => () => {}, checkForUpdates: vi.fn(),
    install: vi.fn(), decline: vi.fn(),
  } };
});
const { default: App } = await import("./App");

const outfit = {
  id: "mod-outfit", game: "gimi", name: "Amber Outfit", source: "local",
  library_path: "C:/Library/gimi/mod-outfit", enabled: false,
  gamebanana_id: null, source_url: null, version: null,
  reinstall_recovery: null, enabled_transition_recovery: null,
};
const transition = {
  intendedEnabled: true, reason: "Producer identity is unavailable",
  attemptedAt: "2026-10-07T12:00:00Z", attempts: 1,
  junctionPath: "C:/Game/Mods/Amber Outfit", ownerUncertain: true,
};
const reservation = { id: "launch-1", game: "gimi", child_pid: 4242, started_at: "2026-10-07T12:00:00Z" };
let sessionRead: () => Promise<{ game: "gimi"; pid: number; started_at: string } | null>;
let launchVisible: boolean;
let producerVisible: boolean;
let retire: () => Promise<void>;

beforeEach(() => {
  vi.clearAllMocks();
  sessionRead = async () => null;
  launchVisible = false;
  producerVisible = false;
  retire = async () => {};
  invoke.mockImplementation(async (command: string) => {
    switch (command) {
      case "is_onboarding_complete": return { complete: true, skipped: false };
      case "list_supported_games": return [{ code: "gimi", displayName: "Genshin Impact" }];
      case "get_game_install_path": return "C:/Game";
      case "get_game_path_status": return { game: "gimi", savedPath: "C:/Game", gamePath: { status: "readable" }, modelImporter: { status: "installed", record: { installedVersion: "1.0", origin: { state: "unknown" } } } };
      case "current_session": return sessionRead();
      case "interrupted_session_launches": return launchVisible ? [reservation] : [];
      case "retire_interrupted_session_launch":
      case "retire_interrupted_enabled_transition": return retire();
      case "list_mods": return [{ ...outfit, enabled_transition_recovery: producerVisible ? transition : null }];
      case "detect_conflicts": return { conflicts: [], omitted: [], per_mod_count: {} };
      case "list_variants": return { variants: [], activeVariantId: null };
      case "list_mod_updates": return [];
      case "get_startup_reconcile_status": return { finished: true, failures: [] };
      case "attention_status": return {
        safeToProceed: true, reinstalls: [], enabledTransitions: [], importerEvacuations: [],
        stagedLibraryOperations: [], sessionLaunches: [], activeSession: null,
        libraryAudits: [], libraryRootOverlaps: [], modPathOverlaps: [],
      };
      case "check_importer_update": return { available: false, installedVersion: null, pinned: false };
      case "get_proxy_config": return { url: null, username: null, passwordSet: false };
      case "get_library_paths": return { defaultRoot: "C:/Library", effectiveRoot: "C:/Library", rootOverride: null, perGameOverrides: {}, perGameEffective: {} };
      case "mod_updates_globally_enabled": return true;
      case "audit_library": return { unreferenced: [], duplicates: [], totalBytes: 0 };
      default: return null;
    }
  });
});

function renderApp() {
  const client = makeQueryClient();
  // Keep the successful cache until the test explicitly refreshes it. No mocked query states.
  client.setDefaultOptions({ queries: { retry: false, staleTime: 30_000 }, mutations: { retry: false } });
  return renderWithQuery(<App />, { client });
}

async function visitSettings() {
  const arrival = screen.getByRole("button", { name: "Settings for Genshin Impact" });
  await userEvent.click(arrival);
  const dialog = screen.getByRole("dialog", { name: "Settings" });
  const navigation = within(dialog).getByRole("navigation", { name: "Settings pages" });
  await userEvent.click(within(navigation).getByRole("button", { name: /GMM settings/ }));
  expect(within(dialog).getByRole("heading", { name: "Diagnostics" })).toBeVisible();
  await userEvent.click(within(navigation).getByRole("button", { name: /Genshin Impact/ }));
  await userEvent.click(within(dialog).getByRole("button", { name: "Close" }));
  expect(arrival).toHaveFocus();
}

const states = ["pending", "rejected", "stale inactive", "stale active"] as const;
it.each(states)("Launch gate reports an unverified session: %s", async (state) => {
  await assertUnknownGate("Launch", state);
});
it.each(states)("Mod gate reports an unverified session: %s", async (state) => {
  await assertUnknownGate("Mod", state);
});

async function assertUnknownGate(surface: "Launch" | "Mod", state: typeof states[number]) {
  if (state === "pending") sessionRead = () => new Promise(() => {});
  if (state === "rejected") sessionRead = async () => { throw new Error("Session lookup unavailable"); };
  if (state === "stale active") sessionRead = async () => ({ game: "gimi", pid: 4242, started_at: reservation.started_at });
  const { client, container } = renderApp();
  await screen.findByText("Amber Outfit");
  if (state.startsWith("stale")) {
    await waitFor(() => expect(client.getQueryState(["session"])?.status).toBe("success"));
    sessionRead = async () => { throw new Error("Session lookup unavailable"); };
    await act(async () => { await client.invalidateQueries({ queryKey: ["session"] }); });
  }
  const control = surface === "Launch"
    ? container.querySelector<HTMLButtonElement>(".game-header__launch")!
    : screen.getByRole("checkbox", { name: "Disabled" });
  const ownSurface = surface === "Launch"
    ? container.querySelector<HTMLElement>(".main-screen__notices")!
    : screen.getByText("Amber Outfit").closest("section")!;
  await waitFor(() => expect(control).toBeDisabled());
  const reason = state === "pending" ? /Game Session unverified — checking/
    : state.startsWith("stale") ? /Cannot verify Game Session — refresh failed; the previous result is unverified/
      : /Cannot verify Game Session — session check failed/;
  expect(within(ownSurface).getByText(reason)).toBeVisible();
  if (state !== "pending") expect(ownSurface).toHaveTextContent("Session lookup unavailable");
  if (state === "stale inactive") expect(client.getQueryData(["session"])).toBeNull();
  if (state === "stale active") {
    expect(client.getQueryData(["session"])).toMatchObject({ pid: 4242 });
    expect(screen.queryByText("GIMI is running")).not.toBeInTheDocument();
  }
  await visitSettings();
  expect(control).toBeDisabled();
  expect(invoke.mock.calls.some(([command]) => command === "launch_game" || command === "set_mod_enabled")).toBe(false);
}

const recoveries = ["launch", "producer"] as const;
function recoveryCase(kind: typeof recoveries[number]) {
  if (kind === "launch") launchVisible = true;
  else producerVisible = true;
  return {
    label: kind === "launch" ? "Genshin Impact launch recovery outcome" : "Genshin Impact deployment recovery outcome",
    button: kind === "launch" ? /I confirmed the game is closed — retire reservation/ : /I confirmed no other GMM is changing this Mod — retire producer/,
    warning: kind === "launch" ? "Interrupted GIMI launch" : "Interrupted enable or disable recovery for Amber Outfit",
    query: kind === "launch" ? ["interruptedSessionLaunches"] : ["mods", "gimi"],
    remove: () => { launchVisible = false; producerVisible = false; },
  };
}

it.each(recoveries)("retains failed %s retirement outside the removed warning, refresh and Settings", async (kind) => {
  const recovery = recoveryCase(kind);
  retire = async () => { throw new Error("Retirement failed"); };
  const { client } = renderApp();
  const button = await screen.findByRole("button", { name: recovery.button });
  const region = screen.getByRole("region", { name: recovery.label });
  expect(region).toBeEmptyDOMElement();
  await userEvent.click(button);
  expect(await screen.findByText(/Could not retire.*Retirement failed/)).toBeVisible();
  recovery.remove();
  await act(async () => { await client.invalidateQueries({ queryKey: recovery.query }); });
  await waitFor(() => expect(screen.queryByRole("region", { name: recovery.warning })).not.toBeInTheDocument());
  expect(region).toHaveFocus();
  expect(region).toHaveTextContent("Retirement failed");
  expect(within(region).getByRole("alert")).toHaveTextContent("Could not retire");
  await act(async () => { await client.invalidateQueries({ queryKey: recovery.query }); });
  await visitSettings();
  expect(screen.getByRole("region", { name: recovery.label })).toBe(region);
  expect(region).toHaveTextContent("Retirement failed");
});

it.each(recoveries.flatMap((kind) => ["retired", "already absent"].map((outcome) => ({ kind, outcome }))))(
  "retains $kind retirement outcome and stable focus when $outcome",
  async ({ kind, outcome }) => {
    const recovery = recoveryCase(kind);
    let finish!: () => void;
    retire = () => new Promise<void>((resolve) => { finish = resolve; });
    const { client } = renderApp();
    const button = await screen.findByRole("button", { name: recovery.button });
    const region = screen.getByRole("region", { name: recovery.label });
    // The backend's void response cannot distinguish retired from already absent.
    if (outcome === "already absent") recovery.remove();
    await userEvent.click(button);
    await waitFor(() => expect(region).toHaveTextContent("Retiring"));
    expect(button).toBeDisabled();
    await act(async () => { recovery.remove(); finish(); });
    await waitFor(() => expect(screen.queryByRole("region", { name: recovery.warning })).not.toBeInTheDocument());
    expect(region).toHaveTextContent("retired or already absent");
    expect(region).toHaveTextContent("GMM did not verify process identity");
    expect(region).toHaveAttribute("aria-live", "polite");
    expect(region).toHaveFocus();
    await act(async () => { await client.invalidateQueries({ queryKey: recovery.query }); });
    await visitSettings();
    expect(region).toHaveTextContent("retired or already absent");
  },
);

it.each(recoveries)("does not take focus out of Settings when pending %s retirement settles", async (kind) => {
  const recovery = recoveryCase(kind);
  let finish!: () => void;
  retire = () => new Promise<void>((resolve) => { finish = resolve; });
  renderApp();
  await userEvent.click(await screen.findByRole("button", { name: recovery.button }));
  await userEvent.click(screen.getByRole("button", { name: "Settings for Genshin Impact" }));
  const dialog = screen.getByRole("dialog", { name: "Settings" });
  const focused = document.activeElement;
  await act(async () => { recovery.remove(); finish(); });
  await waitFor(() => expect(screen.queryByRole("button", { name: recovery.button, hidden: true })).not.toBeInTheDocument());
  expect(dialog).toContainElement(focused as HTMLElement);
  expect(focused).toHaveFocus();
  await userEvent.click(within(dialog).getByRole("button", { name: "Close" }));
  expect(screen.getByRole("region", { name: recovery.label })).toHaveTextContent("retired or already absent");
});
