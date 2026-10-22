import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import { makeQueryClient, renderWithQuery } from "./test/harness";
import { ModList } from "./ModList";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));

const mod = (id: string, name: string) => ({
  id, name, game: "srmi", source: "local", enabled: false,
  library_path: `C:/Library/srmi/${id}`,
});
let mods: ReturnType<typeof mod>[];
let failRefresh: boolean;
let variants: boolean;

beforeEach(() => {
  mods = [mod("march", "March Outfit"), mod("dan", "Dan Heng Outfit"), mod("himeko", "Himeko Outfit")];
  failRefresh = false;
  variants = false;
  invoke.mockImplementation(async (command: string, args?: { modId?: string }) => {
    if (command === "list_mods") {
      if (failRefresh) throw new Error("Library unavailable");
      return mods;
    }
    if (command === "detect_conflicts") return { conflicts: [], per_mod_count: {} };
    if (command === "list_variants") return {
      variants: variants && args?.modId === "march" ? [
        { id: "red", mod_id: "march", name: "Red", subpath: "red" },
        { id: "blue", mod_id: "march", name: "Blue", subpath: "blue" },
      ] : [],
      activeVariantId: "red",
    };
    if (command === "list_mod_updates") return [];
    return null;
  });
});

async function renderMods() {
  const client = makeQueryClient();
  client.setDefaultOptions({ queries: { retry: false, staleTime: Infinity }, mutations: { retry: false } });
  const rendered = renderWithQuery(<ModList game="srmi" displayName="Honkai: Star Rail" />, { client });
  await screen.findByRole("listitem", { name: "March Outfit" });
  await waitFor(() => expect(screen.getAllByRole("checkbox", { name: "Disabled" })[0]).toBeEnabled());
  return rendered;
}

it("announces exactly one pointer-selected Mod while retaining inner control names", async () => {
  const user = userEvent.setup();
  await renderMods();
  const list = screen.getByRole("list");
  expect(list).not.toHaveAttribute("role");
  expect(list).not.toHaveAttribute("aria-label");
  expect(list).not.toHaveAttribute("aria-multiselectable");
  for (const row of within(list).getAllByRole("listitem")) {
    expect(row).not.toHaveAttribute("role");
    expect(row).not.toHaveAttribute("aria-current");
    expect(row).not.toHaveClass("mods__row--selected");
  }
  expect(list.querySelector("[role=gridcell]")).toBeNull();
  expect(within(list).getAllByRole("listitem", { current: false })).toHaveLength(3);
  await user.click(screen.getByText("March Outfit"));
  expect(within(list).getByRole("listitem", { current: true })).toHaveAccessibleName("March Outfit");
  expect(screen.getByRole("listitem", { name: "March Outfit" })).toHaveClass("mods__row--selected");
  await user.click(screen.getByText("Dan Heng Outfit"));
  expect(screen.getByRole("listitem", { name: "March Outfit" })).not.toHaveClass("mods__row--selected");
  expect(screen.getByRole("listitem", { name: "Dan Heng Outfit" })).toHaveClass("mods__row--selected");
  expect(within(list).getAllByRole("listitem", { current: true })).toHaveLength(1);
  expect(within(list).getByRole("listitem", { current: true })).toHaveAccessibleName("Dan Heng Outfit");
  expect(within(list).getAllByRole("checkbox", { name: "Disabled" })).toHaveLength(3);
});

it("moves focus and selection between rows with arrows without entering their controls", async () => {
  const user = userEvent.setup();
  await renderMods();
  const first = screen.getByRole("listitem", { name: "March Outfit" });
  const second = screen.getByRole("listitem", { name: "Dan Heng Outfit" });
  // Tab from the import toolbar enters the row itself, before its controls.
  screen.getByRole("button", { name: "Paste GameBanana URL…" }).focus();
  await user.tab();
  expect(first).toHaveFocus();
  expect(first).not.toHaveAttribute("aria-current");
  await user.keyboard("{ArrowDown}");
  expect(second).toHaveFocus();
  expect(second).toHaveAttribute("aria-current", "true");
  expect(first).not.toHaveAttribute("aria-current");
  expect(second).toHaveAttribute("tabindex", "0");
  expect(first).toHaveAttribute("tabindex", "0");
  await user.keyboard("{ArrowUp}");
  expect(first).toHaveFocus();
  expect(first).toHaveAttribute("aria-current", "true");
  expect(invoke.mock.calls.filter(([command]) => command === "set_mod_enabled")).toHaveLength(0);
});

it("keeps keyboard re-entry on a row when a later Mod remains selected", async () => {
  const user = userEvent.setup();
  await renderMods();
  await user.click(screen.getByText("Himeko Outfit"));
  screen.getByRole("button", { name: "Paste GameBanana URL…" }).focus();
  await user.tab();
  expect(screen.getByRole("listitem", { name: "March Outfit" })).toHaveFocus();
  expect(screen.getByRole("listitem", { current: true })).toHaveAccessibleName("Himeko Outfit");
});

it("supports Home and End and keeps focus within list boundaries", async () => {
  const user = userEvent.setup();
  await renderMods();
  const first = screen.getByRole("listitem", { name: "March Outfit" });
  const last = screen.getByRole("listitem", { name: "Himeko Outfit" });
  first.focus();
  await user.keyboard("{End}{ArrowDown}");
  expect(last).toHaveFocus();
  expect(last).toHaveAttribute("aria-current", "true");
  await user.keyboard("{Home}{ArrowUp}");
  expect(first).toHaveFocus();
  expect(first).toHaveAttribute("aria-current", "true");
});

it.each(["{Enter}", " "])("selects a focused row with %s without toggling the Mod", async (key) => {
  const user = userEvent.setup();
  await renderMods();
  const first = screen.getByRole("listitem", { name: "March Outfit" });
  first.focus();
  await user.keyboard(key);
  expect(first).toHaveAttribute("aria-current", "true");
  expect(invoke.mock.calls.filter(([command]) => command === "set_mod_enabled")).toHaveLength(0);
});

it("lets Tab enter controls and Shift+Tab return to the selected row", async () => {
  const user = userEvent.setup();
  await renderMods();
  const row = screen.getByRole("listitem", { name: "March Outfit" });
  await user.click(screen.getByText("March Outfit"));
  expect(row).toHaveFocus();
  await user.tab();
  const checkbox = within(row).getByRole("checkbox", { name: "Disabled" });
  expect(checkbox).toHaveFocus();
  await user.keyboard(" ");
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("set_mod_enabled", { id: "march", enabled: true, game: "srmi" }));
  await waitFor(() => expect(checkbox).toBeEnabled());
  await user.tab({ shift: true });
  expect(row).toHaveFocus();
});

it("keeps Variant radio keys inside the row and preserves pointer control focus", async () => {
  variants = true;
  const user = userEvent.setup();
  await renderMods();
  const first = screen.getByRole("listitem", { name: "March Outfit" });
  await user.click(screen.getByText("March Outfit"));
  await user.tab();
  const red = within(first).getByRole("radio", { name: "Red" });
  const blue = within(first).getByRole("radio", { name: "Blue" });
  expect(red).toHaveFocus();
  await user.keyboard("{ArrowDown}");
  expect(blue).toHaveFocus();
  expect(first).toHaveAttribute("aria-current", "true");
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("set_active_variant", { modId: "march", variantId: "blue", game: "srmi" }));
  const second = screen.getByRole("listitem", { name: "Dan Heng Outfit" });
  const checkbox = within(second).getByRole("checkbox", { name: "Disabled" });
  await user.click(checkbox);
  expect(second).toHaveAttribute("aria-current", "true");
  expect(checkbox).toHaveFocus();
});

it("retains selection by Mod ID through a reordered refresh and a failed refresh", async () => {
  const user = userEvent.setup();
  const { client } = await renderMods();
  await user.click(screen.getByText("Dan Heng Outfit"));
  mods = [mods[2], { ...mods[1], name: "Renamed Outfit" }, mods[0]];
  await act(async () => { await client.invalidateQueries({ queryKey: ["mods", "srmi"] }); });
  await waitFor(() => expect(screen.getByRole("listitem", { current: true })).toHaveAccessibleName("Renamed Outfit"));
  failRefresh = true;
  await act(async () => { await client.invalidateQueries({ queryKey: ["mods", "srmi"] }); });
  expect(await screen.findByText("Library unavailable")).toBeVisible();
  await waitFor(() => expect(screen.getByRole("listitem", { current: true })).toHaveAccessibleName("Renamed Outfit"));
});

it("clears selection when the Mod disappears and does not resurrect it on reappearance", async () => {
  const user = userEvent.setup();
  const { client } = await renderMods();
  await user.click(screen.getByText("March Outfit"));
  const removed = mods[0];
  mods = mods.slice(1);
  await act(async () => { await client.invalidateQueries({ queryKey: ["mods", "srmi"] }); });
  await waitFor(() => expect(screen.queryByRole("listitem", { name: "March Outfit" })).not.toBeInTheDocument());
  expect(screen.queryByRole("listitem", { current: true })).not.toBeInTheDocument();
  expect(screen.getByRole("listitem", { name: "Dan Heng Outfit" })).toHaveAttribute("tabindex", "0");
  mods = [removed, ...mods];
  await act(async () => { await client.invalidateQueries({ queryKey: ["mods", "srmi"] }); });
  await screen.findByRole("listitem", { name: "March Outfit" });
  expect(screen.queryByRole("listitem", { current: true })).not.toBeInTheDocument();
});

it("clears selection for an empty refresh", async () => {
  const user = userEvent.setup();
  const { client } = await renderMods();
  await user.click(screen.getByText("March Outfit"));
  mods = [];
  await act(async () => { await client.invalidateQueries({ queryKey: ["mods", "srmi"] }); });
  await waitFor(() => expect(screen.queryByRole("list")).not.toBeInTheDocument());
  mods = [mod("march", "March Outfit")];
  await act(async () => { await client.invalidateQueries({ queryKey: ["mods", "srmi"] }); });
  expect(await screen.findByRole("listitem", { name: "March Outfit" })).not.toHaveAttribute("aria-current");
});

it("clears selection on Game change even with the same Mod ID and when returning", async () => {
  const user = userEvent.setup();
  const { client, rerender } = await renderMods();
  await user.click(screen.getByText("March Outfit"));
  // Cached destination data exercises the immediate render, without a loading gap.
  client.setQueryData(["mods", "gimi"], client.getQueryData(["mods", "srmi"]));
  rerender(<ModList game="gimi" displayName="Genshin Impact" />);
  expect(screen.queryByRole("listitem", { current: true })).not.toBeInTheDocument();
  fireEvent.click(screen.getByText("Dan Heng Outfit"));
  rerender(<ModList game="srmi" displayName="Honkai: Star Rail" />);
  expect(screen.queryByRole("listitem", { current: true })).not.toBeInTheDocument();
});


it("navigates rendered Mods past non-Mod siblings and follows refreshed order", async () => {
  const user = userEvent.setup();
  const { client } = await renderMods();
  const first = screen.getByRole("listitem", { name: "March Outfit" });
  const second = screen.getByRole("listitem", { name: "Dan Heng Outfit" });
  const last = screen.getByRole("listitem", { name: "Himeko Outfit" });
  const separator = document.createElement("li");
  separator.textContent = "Loading more…";
  second.before(separator);
  first.focus();
  await user.keyboard("{ArrowDown}");
  expect(second).toHaveFocus();
  expect(second).toHaveAttribute("aria-current", "true");
  separator.remove();
  mods = [mods[2], mods[0], mods[1]];
  await act(async () => { await client.invalidateQueries({ queryKey: ["mods", "srmi"] }); });
  await user.keyboard("{ArrowUp}");
  expect(first).toHaveFocus();
  await user.keyboard("{ArrowUp}");
  expect(last).toHaveFocus();
  expect(last).toHaveAttribute("aria-current", "true");
});

it("ignores Shift+Arrow on a focused row", async () => {
  const user = userEvent.setup();
  await renderMods();
  const first = screen.getByRole("listitem", { name: "March Outfit" });
  await user.click(first);
  await user.keyboard("{Shift>}{ArrowDown}{/Shift}");
  expect(first).toHaveFocus();
  expect(first).toHaveAttribute("aria-current", "true");
});

it("does not render a Mod list while the query is pending", () => {
  invoke.mockImplementation((command: string) => command === "list_mods"
    ? new Promise(() => {}) : Promise.resolve(null));
  renderWithQuery(<ModList game="srmi" displayName="Honkai: Star Rail" />);
  expect(screen.getByText("Loading…")).toBeVisible();
  expect(screen.queryByRole("list")).not.toBeInTheDocument();
});

it("does not render a Mod list after failure without cached data", async () => {
  failRefresh = true;
  const client = makeQueryClient();
  client.setDefaultOptions({ queries: { retry: false }, mutations: { retry: false } });
  renderWithQuery(<ModList game="srmi" displayName="Honkai: Star Rail" />, { client });
  expect(await screen.findByText("Library unavailable")).toBeVisible();
  expect(screen.queryByRole("list")).not.toBeInTheDocument();
});
