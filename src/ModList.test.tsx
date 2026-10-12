import { act, fireEvent, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import { makeQueryClient, renderWithQuery } from "./test/harness";

const { invoke, openDialog } = vi.hoisted(() => ({ invoke: vi.fn(), openDialog: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: openDialog }));
const { ModList } = await import("./ModList");

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
  const heading = await screen.findByRole("heading", { name: "Add Mods for Honkai: Star Rail" });
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

const routes = ["adopt", "zip", "gamebanana", "drop"] as const;
async function openRoute(entry: typeof routes[number], container: HTMLElement) {
  if (entry === "drop") {
    fireEvent.drop(container.querySelector(".dropzone")!, { dataTransfer: { files: [Object.assign(new File(["zip"], "outfit.zip"), { path: "C:/Downloads/outfit.zip" })] } });
  } else {
    await userEvent.click(screen.getByRole("button", { name: entry === "adopt" ? "Adopt folder…" : entry === "zip" ? "Import ZIP…" : "Paste GameBanana URL…" }));
    if (entry !== "gamebanana") await userEvent.click(screen.getByRole("button", { name: entry === "adopt" ? "Pick mod folder" : "Pick .zip file" }));
  }
  await userEvent.type(screen.getByPlaceholderText(entry === "gamebanana" ? "https://gamebanana.com/mods/1234567 or a bare ID" : entry === "drop" ? "Display name" : "Display name (e.g. Hu Tao Skin)"), entry === "gamebanana" ? "123" : "March Outfit");
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
  await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(invoke.mock.calls.some(([command]) => command === "adopt_folder" || command === "import_zip")).toBe(false);
});
