import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { GamePresets } from "./GamePresets";
import { renderWithQuery } from "./test/harness";
import type { Preset } from "./api";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke, Channel: class {} }));
let saved: Preset[];
beforeEach(() => {
  saved = [];
  invoke.mockReset().mockImplementation((command: string, args: { game: string; name: string }) => {
    if (command === "list_presets") return Promise.resolve(saved.filter((preset) => preset.game === args.game));
    if (command === "save_preset") {
      if (saved.some((preset) => preset.game === args.game && preset.name === args.name)) {
        return Promise.reject({ kind: "other", message: `Preset name "${args.name}" is already taken. Choose another name.` });
      }
      const preset = { id: String(saved.length), game: args.game, name: args.name, createdAt: "now", modCount: 0 } as Preset;
      saved.push(preset);
      return Promise.resolve(preset);
    }
    throw new Error(`Unexpected command: ${command}`);
  });
});
async function save(name: string) {
  fireEvent.change(screen.getByRole("textbox", { name: "Preset name" }), { target: { value: name } });
  fireEvent.click(screen.getByRole("button", { name: "Save current setup as…" }));
}
it("saves the typed name and refreshes the listing, including a valid empty preset", async () => {
  const { client } = renderWithQuery(<GamePresets game="gimi" />);
  const invalidate = vi.spyOn(client, "invalidateQueries");
  await screen.findByText("No saved presets for this Game.");
  await save("Clean for patch day");
  expect(await screen.findByText("Clean for patch day — No Mods enabled")).toBeVisible();
  expect(invoke).toHaveBeenCalledWith("save_preset", { game: "gimi", name: "Clean for patch day" });
  expect(invalidate).toHaveBeenCalledWith({ queryKey: ["presets", "gimi"] });
  expect(screen.getByRole("textbox")).toHaveValue("");
});
it("surfaces the taken-name error and keeps the existing preset and name draft", async () => {
  saved = [{ id: "original", game: "gimi", name: "Screenshot", createdAt: "now", modCount: 2 }];
  renderWithQuery(<GamePresets game="gimi" />);
  await screen.findByText("Screenshot — 2 Mods enabled");
  await save("Screenshot");
  expect(await screen.findByRole("alert")).toHaveTextContent('Preset name "Screenshot" is already taken. Choose another name.');
  expect(screen.getByRole("textbox")).toHaveValue("Screenshot");
  expect(screen.getByText("Screenshot — 2 Mods enabled")).toBeVisible();
  expect(saved).toHaveLength(1);
});
it("lists only the selected Game's presets when the query Game changes", async () => {
  saved = [
    { id: "g", game: "gimi", name: "Genshin setup", createdAt: "now", modCount: 1 },
    { id: "s", game: "srmi", name: "Star Rail setup", createdAt: "now", modCount: 0 },
  ];
  const { rerender } = renderWithQuery(<GamePresets game="gimi" />);
  await screen.findByText("Genshin setup — 1 Mods enabled");
  rerender(<GamePresets game="srmi" />);
  expect(await screen.findByText("Star Rail setup — No Mods enabled")).toBeVisible();
  expect(screen.queryByText(/Genshin setup/)).not.toBeInTheDocument();
});
it("shows listing failures without claiming the Game has no presets", async () => {
  invoke.mockRejectedValue({ kind: "other", message: "Database unavailable" });
  renderWithQuery(<GamePresets game="gimi" />);
  expect(await screen.findByRole("alert")).toHaveTextContent("Database unavailable");
  expect(screen.queryByText("No saved presets for this Game.")).not.toBeInTheDocument();
  fireEvent.change(screen.getByRole("textbox", { name: "Preset name" }), { target: { value: "Available despite listing failure" } });
  await waitFor(() => expect(screen.getByRole("button", { name: "Save current setup as…" })).toBeEnabled());
});

function seedManagement() {
  saved = [{ id: "picked-by-id", game: "gimi", name: "Raiden team", createdAt: "now", modCount: 3 }];
  invoke.mockImplementation((command: string, args: { game: string; presetId: string; name: string }) => {
    if (command === "list_presets") return Promise.resolve([...saved]);
    if (command === "list_mods") return Promise.resolve([{ id: "enabled", enabled: true }, { id: "disabled", enabled: false }]);
    if (command === "rename_preset") { saved = saved.map((p) => p.id === args.presetId ? { ...p, name: args.name } : p); return Promise.resolve(); }
    if (command === "delete_preset") { saved = saved.filter((p) => p.id !== args.presetId); return Promise.resolve(); }
    if (command === "recapture_preset") { saved = saved.map((p) => p.id === args.presetId ? { ...p, modCount: 1 } : p); return Promise.resolve(); }
    throw new Error(`Unexpected command: ${command}`);
  });
}

it("renames the picked ID and refreshes only this Game's saved descriptions", async () => {
  seedManagement();
  const { client } = renderWithQuery(<GamePresets game="gimi" />);
  const invalidate = vi.spyOn(client, "invalidateQueries");
  fireEvent.click(await screen.findByRole("button", { name: "Rename Raiden team" }));
  fireEvent.change(screen.getByRole("textbox", { name: "New preset name" }), { target: { value: "Blue team" } });
  fireEvent.click(screen.getByRole("button", { name: "Save new name" }));
  expect(await screen.findByText("Blue team — 3 Mods enabled")).toBeVisible();
  expect(invoke).toHaveBeenCalledWith("rename_preset", { game: "gimi", presetId: "picked-by-id", name: "Blue team" });
  expect(invalidate.mock.calls).toEqual([[{ queryKey: ["presets", "gimi"] }]]);
});

it("requires delete confirmation with explicit reassurance that no Mod changes", async () => {
  seedManagement();
  renderWithQuery(<GamePresets game="gimi" />);
  fireEvent.click(await screen.findByRole("button", { name: "Delete saved preset Raiden team" }));
  expect(screen.getByRole("alertdialog", { name: "Delete saved preset" })).toHaveTextContent("Only the saved description is removed. No Mod is turned off and no Mod is changed.");
  expect(invoke.mock.calls.filter(([command]) => command === "delete_preset")).toHaveLength(0);
  fireEvent.click(screen.getByRole("button", { name: "Keep saved preset" }));
  expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
  expect(invoke.mock.calls.filter(([command]) => command === "delete_preset")).toHaveLength(0);
  fireEvent.click(screen.getByRole("button", { name: "Delete saved preset Raiden team" }));
  fireEvent.click(screen.getByRole("button", { name: "Delete saved description" }));
  expect(await screen.findByText("No saved presets for this Game.")).toBeVisible();
  expect(invoke).toHaveBeenCalledWith("delete_preset", { game: "gimi", presetId: "picked-by-id" });
});

it("requires re-capture confirmation showing previous and current enabled Mod counts", async () => {
  seedManagement();
  renderWithQuery(<GamePresets game="gimi" />);
  fireEvent.click(await screen.findByRole("button", { name: "Re-capture current setup into Raiden team" }));
  expect(await screen.findByText(/It holds 3 Mods now and will hold 1 Mods after re-capture/)).toBeVisible();
  expect(screen.getByRole("alertdialog", { name: "Replace saved setup" })).toHaveTextContent("The previous saved entries cannot be recovered. The name stays the same. No Mod is changed.");
  expect(invoke.mock.calls.filter(([command]) => command === "recapture_preset")).toHaveLength(0);
  fireEvent.click(screen.getByRole("button", { name: "Keep previous saved entries" }));
  expect(invoke.mock.calls.filter(([command]) => command === "recapture_preset")).toHaveLength(0);
  fireEvent.click(screen.getByRole("button", { name: "Re-capture current setup into Raiden team" }));
  await waitFor(() => expect(screen.getByRole("button", { name: "Replace saved entries with current setup" })).toBeEnabled());
  fireEvent.click(screen.getByRole("button", { name: "Replace saved entries with current setup" }));
  expect(await screen.findByText("Raiden team — 1 Mods enabled")).toBeVisible();
  expect(invoke).toHaveBeenCalledWith("recapture_preset", { game: "gimi", presetId: "picked-by-id" });
});

it("keeps the re-capture action unavailable when its current setup count cannot be read and hides the retained query error on cancel", async () => {
  seedManagement();
  const base = invoke.getMockImplementation()!;
  invoke.mockImplementation((command, args) => command === "list_mods" ? Promise.reject({ kind: "other", message: "Current setup unknown" }) : base(command, args));
  renderWithQuery(<GamePresets game="gimi" />);
  fireEvent.click(await screen.findByRole("button", { name: "Re-capture current setup into Raiden team" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Current setup unknown");
  expect(screen.getByRole("button", { name: "Replace saved entries with current setup" })).toBeDisabled();
  expect(invoke.mock.calls.filter(([command]) => command === "recapture_preset")).toHaveLength(0);
  fireEvent.click(screen.getByRole("button", { name: "Keep previous saved entries" }));
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

it.each([
  ["Rename Raiden team", "rename_preset", "Save new name"],
  ["Delete saved preset Raiden team", "delete_preset", "Delete saved description"],
  ["Re-capture current setup into Raiden team", "recapture_preset", "Replace saved entries with current setup"],
])("keeps the saved description visible when %s fails", async (open, command, confirm) => {
  seedManagement();
  const base = invoke.getMockImplementation()!;
  invoke.mockImplementation((cmd, args) => cmd === command ? Promise.reject({ kind: "other", message: "Operation refused" }) : base(cmd, args));
  renderWithQuery(<GamePresets game="gimi" />);
  fireEvent.click(await screen.findByRole("button", { name: open }));
  await waitFor(() => expect(screen.getByRole("button", { name: confirm })).toBeEnabled());
  fireEvent.click(screen.getByRole("button", { name: confirm }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Operation refused");
  expect(screen.getByText("Raiden team — 3 Mods enabled")).toBeVisible();
});
