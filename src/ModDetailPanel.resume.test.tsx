import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import { ModList } from "./ModList";
import { makeQueryClient, renderWithQuery } from "./test/harness";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));

const makeMod = (id: string, name: string) => ({
  id, name, game: "srmi", source: "local", library_path: `C:/Library/srmi/${id}`,
  author: `${name} Author`, enabled: false,
});
let mods: ReturnType<typeof makeMod>[];
let failRefresh: boolean;
let failSave: boolean;
let storedWidth: number;

beforeEach(() => {
  mods = [makeMod("first", "First Outfit"), makeMod("second", "Second Outfit")];
  failRefresh = false;
  failSave = false;
  storedWidth = 460;
  invoke.mockImplementation(async (command: string, args?: { width?: number }) => {
    if (command === "list_mods") {
      if (failRefresh) throw new Error("Library read failed");
      return mods;
    }
    if (command === "detect_conflicts") return { conflicts: [], omitted: [], per_mod_count: {} };
    if (command === "list_variants") return { variants: [], activeVariantId: null };
    if (command === "list_mod_updates") return [];
    if (command === "get_mod_panel_width") return storedWidth;
    if (command === "set_mod_panel_width") {
      if (failSave) throw new Error("Width save failed");
      storedWidth = args!.width!;
    }
    return null;
  });
});

async function renderMods() {
  const client = makeQueryClient();
  client.setDefaultOptions({ queries: { retry: false, staleTime: Infinity }, mutations: { retry: false } });
  const rendered = renderWithQuery(<ModList game="srmi" displayName="Star Rail" />, { client });
  await screen.findByRole("listitem", { name: "First Outfit" });
  return { ...rendered, client };
}

it("reopens at the remembered width after a failed width change", async () => {
  failSave = true;
  const user = userEvent.setup();
  await renderMods();
  await user.click(screen.getByRole("listitem", { name: "First Outfit" }));
  await waitFor(() => expect(screen.getByRole("slider")).toHaveValue("460"));
  fireEvent.change(screen.getByRole("slider"), { target: { value: "420" } });
  expect(await screen.findByText("Width save failed")).toBeVisible();
  expect(storedWidth).toBe(460);
  await user.click(screen.getByRole("button", { name: "Close Mod details" }));
  await user.click(screen.getByRole("button", { name: "Show Mod details" }));
  expect(screen.getByRole("slider")).toHaveValue("460");
  expect(screen.getByRole("complementary")).toHaveStyle({ flexBasis: "460px" });
});

it("keeps panel detail on the selected identity through keyboard navigation and refresh", async () => {
  const user = userEvent.setup();
  const { client } = await renderMods();
  const first = screen.getByRole("listitem", { name: "First Outfit" });
  first.focus();
  await user.keyboard("{Enter}{ArrowDown}");
  expect(screen.getByRole("listitem", { name: "Second Outfit" })).toHaveFocus();
  expect(within(screen.getByRole("complementary", { name: "Second Outfit" }))
    .getByText("Second Outfit Author")).toBeVisible();
  expect(invoke.mock.calls.filter(([command]) => command === "set_mod_enabled")).toHaveLength(0);
  mods = [{ ...mods[1], name: "Renamed Outfit", author: "New Author" }, mods[0]];
  await act(async () => { await client.invalidateQueries({ queryKey: ["mods", "srmi"] }); });
  expect(within(await screen.findByRole("complementary", { name: "Renamed Outfit" }))
    .getByText("New Author")).toBeVisible();
  failRefresh = true;
  await act(async () => { await client.invalidateQueries({ queryKey: ["mods", "srmi"] }); });
  expect(await screen.findByText("Library read failed")).toBeVisible();
  expect(screen.getByRole("complementary", { name: "Renamed Outfit" })).toBeVisible();
  failRefresh = false;
  const removed = mods[0];
  mods = mods.slice(1);
  await act(async () => { await client.invalidateQueries({ queryKey: ["mods", "srmi"] }); });
  expect(await screen.findByText("Select a Mod to view its details.")).toBeVisible();
  mods = [removed, ...mods];
  await act(async () => { await client.invalidateQueries({ queryKey: ["mods", "srmi"] }); });
  expect(screen.getByRole("complementary", { name: "Mod details" })).toBeVisible();
  expect(screen.queryByRole("listitem", { current: true })).not.toBeInTheDocument();
});
