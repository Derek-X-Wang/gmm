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
it("lists only the selected Game's presets across the keyed Game surface", async () => {
  saved = [
    { id: "g", game: "gimi", name: "Genshin setup", createdAt: "now", modCount: 1 },
    { id: "s", game: "srmi", name: "Star Rail setup", createdAt: "now", modCount: 0 },
  ];
  const { rerender } = renderWithQuery(<GamePresets key="gimi" game="gimi" />);
  await screen.findByText("Genshin setup — 1 Mods enabled");
  rerender(<GamePresets key="srmi" game="srmi" />);
  expect(await screen.findByText("Star Rail setup — No Mods enabled")).toBeVisible();
  expect(screen.queryByText(/Genshin setup/)).not.toBeInTheDocument();
});
it("shows listing failures without claiming the Game has no presets", async () => {
  invoke.mockRejectedValue({ kind: "other", message: "Database unavailable" });
  renderWithQuery(<GamePresets game="gimi" />);
  expect(await screen.findByRole("alert")).toHaveTextContent("Database unavailable");
  expect(screen.queryByText("No saved presets for this Game.")).not.toBeInTheDocument();
  await waitFor(() => expect(screen.getByRole("button")).toBeDisabled());
});
