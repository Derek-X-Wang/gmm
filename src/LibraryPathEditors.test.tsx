import { useState } from "react";
import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import { SettingsDestination, type SettingsPage } from "./SettingsDestination";
import type { GameSummary, LibraryPaths, MoveReport } from "./api";
import { makeQueryClient, renderWithQuery } from "./test/harness";

const { invoke, openDialog } = vi.hoisted(() => ({ invoke: vi.fn(), openDialog: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: openDialog }));

const games: GameSummary[] = [
  { code: "gimi", displayName: "Genshin Impact" },
  { code: "srmi", displayName: "Honkai: Star Rail" },
];
const report: MoveReport = { relocated: ["mod-a"], moved_directories: ["D:\\Library"], failed_junction_restores: [], overlap_repair: false };
let paths: LibraryPaths;
let read: () => Promise<LibraryPaths>;
let move: (command: string, args: { game?: string; path: string | null }) => Promise<MoveReport>;
const scopes = [
  { page: "gmm" as const, region: "Global Library root settings", label: "Global Library root", command: "set_library_root", reset: "Reset to default", initial: "C:\\GMM\\library" },
  { page: "gimi" as const, region: "Library for Genshin Impact", label: "Genshin Impact Library path", command: "set_library_path_for_game", reset: "Reset override", initial: "C:\\GMM\\library\\gimi" },
];
function Destination({ initial }: { initial: SettingsPage }) {
  const [page, setPage] = useState<SettingsPage | null>(initial);
  return <><button onClick={() => setPage(initial)}>Open Settings again</button>
    <SettingsDestination games={games} page={page} returnGame="Genshin Impact" onPageChange={setPage} onBack={() => setPage(null)} /></>;
}
function setup(page: SettingsPage) {
  const client = makeQueryClient();
  client.setDefaultOptions({ queries: { retry: false, staleTime: 30_000, gcTime: 300_000 } });
  return renderWithQuery(<Destination initial={page} />, { client });
}
async function region(name: string) { return within(await screen.findByRole("region", { name })); }
function nav(name: string) { return within(screen.getByRole("navigation", { name: "Settings pages" })).getByRole("button", { name: new RegExp(name) }); }
function writes() { return invoke.mock.calls.filter(([command]) => command === "set_library_root" || command === "set_library_path_for_game"); }
async function apply(label: string) {
  const field = await screen.findByRole("textbox", { name: label });
  await userEvent.clear(field);
  await userEvent.type(field, "D:\\Library{Enter}");
}
beforeEach(() => {
  paths = { defaultRoot: "C:\\GMM\\library", effectiveRoot: "C:\\GMM\\library", rootOverride: null,
    perGameOverrides: {}, perGameEffective: { gimi: "C:\\GMM\\library\\gimi", srmi: "C:\\GMM\\library\\srmi" }, overlaps: [], modOverlaps: [] };
  read = async () => structuredClone(paths);
  move = async (command, { game, path }) => {
    if (command === "set_library_root") {
      paths.rootOverride = path;
      paths.effectiveRoot = path ?? paths.defaultRoot;
      for (const entry of games) if (!paths.perGameOverrides[entry.code]) paths.perGameEffective[entry.code] = `${paths.effectiveRoot}\\${entry.code}`;
    } else {
      paths.perGameOverrides[game!] = path;
      paths.perGameEffective[game!] = path ?? `${paths.effectiveRoot}\\${game}`;
    }
    return report;
  };
  openDialog.mockResolvedValue(null);
  invoke.mockImplementation(async (command: string, args: { game?: string; path: string | null }) => {
    if (command === "get_library_paths") return read();
    if (command === "set_library_root" || command === "set_library_path_for_game") return move(command, args);
    if (command === "get_game_path_status") return { game: args.game, savedPath: null, gamePath: { status: "noSavedPath" }, modelImporter: { status: "unverified", path: null, error: "Not checked" } };
    if (command === "audit_library") return { unreferenced: [], duplicates: [], totalBytes: 0 };
    if (command === "get_proxy_config") return { url: null, username: null, passwordSet: false };
    return null;
  });
});

it("puts only the global editor on GMM and only the named override on each Game, with reciprocal links", async () => {
  setup("gmm");
  const root = await region(scopes[0].region);
  expect(await root.findByRole("textbox", { name: scopes[0].label })).toHaveValue(paths.effectiveRoot);
  expect(root.getByText(/Default root:/)).toHaveTextContent(paths.defaultRoot);
  expect(root.getByText("Global override: Not set")).toBeVisible();
  expect(root.getByRole("button", { name: "Reset to default" })).toBeDisabled();
  expect(root.getByText(/Games that inherit this root:/)).toHaveTextContent(/Genshin Impact.*Honkai: Star Rail/);
  expect(screen.queryByRole("textbox", { name: /Library path/ })).toBeNull();
  await userEvent.click(root.getByRole("link", { name: "Library override for Genshin Impact" }));
  const game = await region(scopes[1].region);
  expect(await game.findByRole("textbox", { name: scopes[1].label })).toHaveValue(paths.perGameEffective.gimi);
  expect(game.getByText("Inherited from the global Library root")).toBeVisible();
  expect(game.getByRole("button", { name: "Reset override" })).toBeDisabled();
  expect(game.queryByText(/absent|not found/i)).toBeNull();
  expect(screen.queryByRole("textbox", { name: "Global Library root" })).toBeNull();
  await userEvent.click(game.getByRole("link", { name: "View global Library root" }));
  await userEvent.click((await region(scopes[0].region)).getByRole("link", { name: "Library override for Honkai: Star Rail" }));
  expect(await screen.findByRole("textbox", { name: "Honkai: Star Rail Library path" })).toHaveValue(paths.perGameEffective.srmi);
  expect(screen.queryByRole("textbox", { name: scopes[1].label })).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  expect(writes()).toHaveLength(0);
});

it.each(scopes)("keeps resolving distinct on $page", async ({ page, region: name }) => {
  read = () => new Promise(() => {});
  setup(page);
  const panel = await region(name);
  expect(panel.getByText("Resolving paths…")).toBeVisible();
  expect(panel.queryByRole("textbox")).toBeNull();
  expect(panel.queryByText(/absent|not found/i)).toBeNull();
});
it.each(scopes)("keeps an unreadable path error distinct on $page", async ({ page, region: name }) => {
  read = async () => { throw new Error("Cannot read Library settings: Access denied"); };
  setup(page);
  const panel = await region(name);
  expect(await panel.findByText("Cannot read Library settings: Access denied")).toBeVisible();
  expect(panel.queryByRole("textbox")).toBeNull();
  expect(panel.queryByText(/absent|not found/i)).toBeNull();
});
it.each(scopes)("shows an override and resets it on $page", async ({ page, region: name, label, reset, command }) => {
  paths.rootOverride = "E:\\Global";
  paths.effectiveRoot = "E:\\Global";
  paths.perGameOverrides.gimi = "E:\\Genshin";
  paths.perGameEffective.gimi = "E:\\Genshin";
  setup(page);
  const panel = await region(name);
  expect(await panel.findByRole("textbox", { name: label })).toHaveValue(page === "gmm" ? "E:\\Global" : "E:\\Genshin");
  expect(panel.getByText(page === "gmm" ? "Global override: E:\\Global" : "Override for Genshin Impact")).toBeVisible();
  await userEvent.click(panel.getByRole("button", { name: reset }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith(command, page === "gmm" ? { path: null } : { game: "gimi", path: null }));
  expect(await panel.findByText("Library move completed.")).toBeVisible();
  expect(panel.getByRole("button", { name: reset })).toBeDisabled();
});
it.each(scopes)("retains a dirty draft across navigation without a move on $page", async ({ page, label }) => {
  setup(page);
  await userEvent.type(await screen.findByRole("textbox", { name: label }), "draft");
  expect(screen.getByRole("button", { name: `Apply ${label}` })).toBeEnabled();
  await userEvent.click(nav(page === "gmm" ? "Genshin Impact" : "GMM settings"));
  await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  await userEvent.click(screen.getByRole("button", { name: "Open Settings again" }));
  expect(screen.getByRole("textbox", { name: label })).toHaveValue(`${page === "gmm" ? paths.effectiveRoot : paths.perGameEffective.gimi}draft`);
  expect(writes()).toHaveLength(0);
});
it.each(scopes.flatMap((scope) => [false, true].map((resetting) => ({ ...scope, resetting }))))("keeps $page pending and late rejection after navigation (reset: $resetting)", async ({ page, region: name, label, reset, resetting }) => {
  paths.rootOverride = paths.effectiveRoot;
  paths.perGameOverrides.gimi = paths.perGameEffective.gimi;
  let reject!: (error: Error) => void;
  move = () => new Promise((_, fail) => { reject = fail; });
  setup(page);
  const panel = await region(name);
  await panel.findByRole("textbox", { name: label });
  if (resetting) await userEvent.click(panel.getByRole("button", { name: reset })); else await apply(label);
  expect(panel.getByText(resetting ? "Resetting…" : "Moving…")).toBeVisible();
  expect(panel.getByRole("textbox", { name: label })).toBeDisabled();
  expect(panel.getByRole("button", { name: reset })).toBeDisabled();
  await userEvent.click(nav(page === "gmm" ? "Genshin Impact" : "GMM settings"));
  // The other editor shares the move guard even though its page is mounted later.
  expect(await screen.findByRole("textbox", { name: page === "gmm" ? scopes[1].label : scopes[0].label })).toBeDisabled();
  await act(async () => reject(new Error("Move refused: target is not NTFS")));
  await userEvent.click(nav(page === "gmm" ? "GMM settings" : "Genshin Impact"));
  expect(panel.getByText("Move refused: target is not NTFS")).toBeVisible();
  expect(panel.queryByText("Library move completed.")).toBeNull();
});
it.each(scopes)("accepts a move with the existing scope command on $page", async ({ page, label, command, region: name }) => {
  setup(page);
  await apply(label);
  await waitFor(() => expect(invoke).toHaveBeenCalledWith(command, page === "gmm" ? { path: "D:\\Library" } : { game: "gimi", path: "D:\\Library" }));
  const panel = await region(name);
  expect(await panel.findByText("Library move completed.")).toBeVisible();
  expect(panel.getByRole("textbox", { name: label })).toHaveValue("D:\\Library");
  expect(paths.perGameEffective.srmi).toBe(page === "gmm" ? "D:\\Library\\srmi" : "C:\\GMM\\library\\srmi");
  expect(writes()).toHaveLength(1);
});
it.each(scopes)("retains setting-only overlap repair on $page", async ({ page, label, region: name }) => {
  move = async () => ({ ...report, relocated: [], moved_directories: [], overlap_repair: true });
  setup(page);
  await apply(label);
  const panel = await region(name);
  expect(await panel.findByText("Library setting changed, but existing Mod folders were not moved.")).toBeVisible();
  expect(panel.queryByText("Library move completed.")).toBeNull();
});
it.each(scopes)("reports each Junction failure and invalid Variant separately on $page", async ({ page, label, region: name }) => {
  move = async () => ({ ...report, failed_junction_restores: [
    { game: "gimi", mod_id: "locked-a", kind: "other", error: "Junction locked A" },
    { game: "gimi", mod_id: "locked-b", kind: "other", error: "Junction locked B" },
    { game: "gimi", mod_id: "variant-c", kind: "invalidActiveVariant", error: "Saved Variant invalid C" },
  ] });
  setup(page);
  await apply(label);
  const panel = await region(name);
  expect(await panel.findByText(/locked-a: Junction locked A/)).toBeVisible();
  expect(panel.getByText(/locked-b: Junction locked B/)).toBeVisible();
  expect(panel.getByText(/GIMI: Saved Variant invalid C/)).toBeVisible();
  expect(panel.getByText(/cannot repair a saved Variant selection/)).toBeVisible();
  expect(panel.queryByText("Library move completed.")).toBeNull();
});
it.each(scopes)("preserves picker commitment and cancellation on $page", async ({ page, label, command }) => {
  setup(page);
  const field = await screen.findByRole("textbox", { name: label });
  await userEvent.type(field, "draft");
  const picker = within(field.closest(".row")!).getByRole("button", { name: page === "gmm" ? "Change global root…" : "Change…" });
  await userEvent.click(picker);
  expect(writes()).toHaveLength(0);
  expect(field).toHaveValue(`${page === "gmm" ? paths.effectiveRoot : paths.perGameEffective.gimi}draft`);
  openDialog.mockResolvedValue("D:\\Picked");
  await userEvent.click(picker);
  await waitFor(() => expect(invoke).toHaveBeenCalledWith(command, page === "gmm" ? { path: "D:\\Picked" } : { game: "gimi", path: "D:\\Picked" }));
  expect(openDialog).toHaveBeenCalledWith({ directory: true, multiple: false });
  expect(field).toHaveValue("D:\\Picked");
});

it.each(scopes.flatMap((scope) => [false, true].flatMap((resetting) => [false, true].map((rejecting) => ({ ...scope, resetting, rejecting })))))
("settles broad reads including inactive Games on $page (reset: $resetting, rejection: $rejecting)", async ({ page, label, reset, resetting, rejecting }) => {
  paths.rootOverride = paths.effectiveRoot;
  paths.perGameOverrides.gimi = paths.perGameEffective.gimi;
  const { client } = setup(page);
  const keys = [["libraryPaths"], ...["mods", "libraryAudit", "conflicts"].flatMap((family) => games.map((game) => [family, game.code]))];
  for (const key of keys.filter((key) => key[0] !== "libraryPaths")) client.setQueryData(key, { previous: true });
  expect(client.getQueryState(["mods", "srmi"])?.isInvalidated).toBe(false);
  const original = move;
  move = async (command, args) => { const result = await original(command, args); if (rejecting) throw new Error("Move persisted before failure"); return result; };
  if (resetting) await userEvent.click(await screen.findByRole("button", { name: reset })); else await apply(label);
  await screen.findByText(rejecting ? "Move persisted before failure" : "Library move completed.");
  for (const key of keys.filter((key) => key[0] !== "libraryPaths")) {
    const cached = client.getQueryCache().find({ queryKey: key, exact: true })!;
    if (cached.getObserversCount() === 0) expect(cached.state.isInvalidated, key.join("/")).toBe(true);
  }
  // Inactive queries must fetch persisted state even within production's cache window.
  for (const family of ["mods", "libraryAudit", "conflicts"]) {
    const queryFn = vi.fn().mockResolvedValue({ current: true });
    expect(await client.fetchQuery({ queryKey: [family, "srmi"], queryFn })).toEqual({ current: true });
    expect(queryFn).toHaveBeenCalledTimes(1);
  }
  expect(invoke.mock.calls.filter(([command]) => command === "get_library_paths").length).toBeGreaterThan(1);
});

it("names only inheriting Games and leaves an override unchanged by a global move", async () => {
  paths.perGameOverrides.srmi = "E:\\StarRail";
  paths.perGameEffective.srmi = "E:\\StarRail";
  setup("gmm");
  const panel = await region(scopes[0].region);
  await panel.findByRole("textbox", { name: scopes[0].label });
  expect(panel.getByText(/Games that inherit this root:/)).toHaveTextContent("Genshin Impact");
  expect(panel.getByText(/Games that inherit this root:/)).not.toHaveTextContent("Honkai: Star Rail");
  expect(panel.getByRole("link", { name: "Library override for Honkai: Star Rail" })).toBeVisible();
  await apply(scopes[0].label);
  await panel.findByText("Library move completed.");
  expect(paths.perGameEffective.gimi).toBe("D:\\Library\\gimi");
  expect(paths.perGameEffective.srmi).toBe("E:\\StarRail");
  expect(invoke).not.toHaveBeenCalledWith("set_library_path_for_game", expect.anything());
});

it("keeps overlap warnings with the affected Game and names the global repair route", async () => {
  paths.perGameOverrides.srmi = "C:\\Backups\\StarRail";
  paths.overlaps = [{ game: "srmi", path: "C:\\Backups\\StarRail", backups: "C:\\Backups" }];
  paths.modOverlaps = [{ game: "srmi", modId: "stranded", modName: "Star Rail outfit", path: "C:\\Backups\\StarRail\\stranded", backups: "C:\\Backups" }];
  setup("gimi");
  expect((await region(scopes[1].region)).queryByRole("region", { name: "Unsafe Library paths" })).toBeNull();
  await userEvent.click(nav("Honkai: Star Rail"));
  expect(await screen.findByRole("region", { name: "Unsafe Library paths" })).toHaveTextContent("Star Rail outfit");
  await userEvent.click(screen.getByRole("link", { name: "View global Library root" }));
  expect((await region(scopes[0].region)).queryByRole("region", { name: "Unsafe Library paths" })).toBeNull();
});

it.each(scopes)("retains a refused move even if the following Library read fails on $page", async ({ page, label, region: name }) => {
  setup(page);
  move = async () => { read = async () => { throw new Error("Library read failed after move"); }; throw new Error("Move refused by recovery ownership guard"); };
  await apply(label);
  const panel = await region(name);
  expect(await panel.findByText("Library read failed after move")).toBeVisible();
  expect(panel.getByText("Move refused by recovery ownership guard")).toBeVisible();
  expect(panel.queryByText(/absent|not found/i)).toBeNull();
});

it.each(["page", "back"])("keeps the sibling audit after the Library group and outside hidden-page accessibility and tab order on %s", async (route) => {
  const original = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (command, args) => command === "audit_library" && args.game === "gimi"
    ? { unreferenced: [{ path: "C:\\GMM\\library\\gimi\\orphan", directoryName: "orphan", sizeBytes: 128 }], duplicates: [], totalBytes: 128 }
    : original(command, args));
  setup("gimi");
  const library = await screen.findByRole("region", { name: scopes[1].region });
  const audit = await screen.findByRole("region", { name: "Unreferenced Library folders and duplicate Mod records" });
  expect(audit.parentElement).toBe(library.parentElement);
  expect(library.nextElementSibling).toBe(audit);
  expect(within(audit).getByRole("button", { name: "Inspect" })).toBeVisible();
  if (route === "page") await userEvent.click(nav("GMM settings"));
  else await userEvent.click(screen.getByRole("button", { name: "Back to Mods" }));
  expect(audit).toBeInTheDocument();
  expect(audit).not.toBeVisible();
  expect(screen.queryByRole("region", { name: "Unreferenced Library folders and duplicate Mod records" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Inspect" })).toBeNull();
  const focusCycle = new Set<Element>();
  for (let index = 0; index < 30; index++) {
    await userEvent.tab();
    const focused = document.activeElement!;
    expect(audit.contains(focused)).toBe(false);
    if (focusCycle.has(focused)) break;
    focusCycle.add(focused);
  }
  expect(focusCycle.size).toBeGreaterThan(1);
  // Retained audit controls reappear on the same mounted node.
  if (route === "page") await userEvent.click(nav("Genshin Impact"));
  else await userEvent.click(screen.getByRole("button", { name: "Open Settings again" }));
  expect(screen.getByRole("region", { name: "Unreferenced Library folders and duplicate Mod records" })).toBe(audit);
  expect(within(audit).getByRole("button", { name: "Inspect" })).toBeVisible();
});
