import { onlineManager, type QueryClient } from "@tanstack/react-query";
import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import type { Mod, ModUpdateRow, ModVariants } from "./api";
import { makeQueryClient, renderWithQuery } from "./test/harness";

// Three readings in ModList used to state a value GMM never read: the Mod
// count beside the Library's own failure notice, an unread update row rendered
// as "no update", and an unread Variant list rendered as a single-folder Mod.
//
// Each case here is driven twice. A rejected command leaves `status: "error"`.
// Going offline with `onlineManager.setOnline(false)` leaves a read *paused*,
// which keeps `status: "success"`, keeps `data` and sets only `fetchStatus`, so
// `isFetching` is false exactly when the surface most needs to hedge. A test
// that only fails a refresh never reaches that case.

const { invoke, openDialog } = vi.hoisted(() => ({ invoke: vi.fn(), openDialog: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: openDialog }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn() }));

const { ModList } = await import("./ModList");

const outfit: Mod = {
  id: "mod-outfit", game: "srmi", name: "March Outfit", source: "gamebanana",
  libraryPath: "C:/Library/srmi/mod-outfit", enabled: false,
  gamebananaId: 42, sourceUrl: "https://gamebanana.com/mods/42", author: null,
  version: "1.0", screenshotUrl: null, reinstallRecovery: null, enabledTransitionRecovery: null,
};
const rawOutfit = {
  id: outfit.id, game: outfit.game, name: outfit.name, source: outfit.source,
  library_path: outfit.libraryPath, enabled: outfit.enabled,
  gamebanana_id: outfit.gamebananaId, source_url: outfit.sourceUrl, version: outfit.version,
  reinstall_recovery: null, enabled_transition_recovery: null,
};
const updateRow: ModUpdateRow = {
  modId: outfit.id, name: outfit.name, installedVersion: "1.0",
  upstreamVersion: "2.0", upstreamAhead: true, updateCheckEnabled: true,
};
const twoVariants: ModVariants = {
  variants: ["Red", "Blue"].map((name) => ({ id: name, mod_id: outfit.id, name, subpath: name })),
  activeVariantId: "Red",
};

let mods: () => unknown;
let updates: () => unknown;
let variants: () => unknown;

beforeEach(() => {
  mods = () => [rawOutfit];
  updates = () => [updateRow];
  variants = () => ({ variants: [], activeVariantId: null });
  invoke.mockImplementation(async (command: string) => {
    switch (command) {
      case "list_mods": return mods();
      case "list_mod_updates": return updates();
      case "list_variants": return variants();
      case "detect_conflicts": return { conflicts: [], omitted: [], per_mod_count: {} };
      default: return null;
    }
  });
});

afterEach(() => {
  onlineManager.setOnline(true);
});

/** A client whose established reads stay fresh, so only the query under test moves. */
function renderMods(prime?: (client: QueryClient) => void) {
  const client = makeQueryClient();
  client.setDefaultOptions({ queries: { retry: false, staleTime: 30_000 }, mutations: { retry: false } });
  prime?.(client);
  return renderWithQuery(<ModList game="srmi" displayName="Honkai: Star Rail" />, { client });
}

/** Pause one read by going offline and asking for it again. */
async function pause(client: QueryClient, queryKey: unknown[]) {
  onlineManager.setOnline(false);
  await act(() => client.invalidateQueries({ queryKey }));
}

// --- ModList::mods — the heading's Mod count ------------------------------

it("withdraws the Mod count when a refresh of the Library fails", async () => {
  const { client } = renderMods();
  expect(await screen.findByRole("heading", { name: "Mods (1)" })).toBeVisible();

  mods = () => { throw new Error("Library unavailable"); };
  await act(() => client.invalidateQueries({ queryKey: ["mods", "srmi"] }));

  await waitFor(() => expect(screen.queryByRole("heading", { name: /Mods \(/ })).not.toBeInTheDocument());
  expect(screen.getByRole("heading", { name: "Mods" })).toBeVisible();
  expect(screen.getByText("Library unavailable")).toBeVisible();
  expect(screen.getByText(/could not refresh this Game's Mod Library/i)).toBeVisible();
  // The Mods it did read stay listed; it is the count that is withdrawn.
  expect(screen.getByText("March Outfit")).toBeVisible();
});

it("withdraws the Mod count when the Library refresh is paused, which leaves status success and isFetching false", async () => {
  const { client } = renderMods();
  expect(await screen.findByRole("heading", { name: "Mods (1)" })).toBeVisible();

  await pause(client, ["mods", "srmi"]);

  await waitFor(() => expect(screen.queryByRole("heading", { name: /Mods \(/ })).not.toBeInTheDocument());
  expect(screen.getByRole("heading", { name: "Mods" })).toBeVisible();
  const notice = screen.getByText(/could not refresh this Game's Mod Library/i);
  expect(notice).toHaveTextContent(/waiting for a connection/i);
  expect(screen.getByText("March Outfit")).toBeVisible();
});

it("does not leave a paused first read of the Library silent", async () => {
  onlineManager.setOnline(false);
  renderMods();

  const notice = await screen.findByText(/could not read this Game's Mod Library/i);
  expect(notice).toHaveTextContent(/not an empty Library/i);
  expect(notice).toHaveTextContent(/waiting for a connection/i);
  expect(screen.queryByRole("heading", { name: /Mods \(/ })).not.toBeInTheDocument();
  expect(screen.queryByRole("heading", { name: "Add Mods for Honkai: Star Rail" })).not.toBeInTheDocument();
});

it("keeps the Mod count on an established, current read", async () => {
  renderMods();
  expect(await screen.findByRole("heading", { name: "Mods (1)" })).toBeVisible();
  expect(screen.queryByText(/could not re(ad|fresh) this Game's Mod Library/i)).not.toBeInTheDocument();
});

// --- ModList / ModUpdateBadge::rows — the update badge --------------------

it("says the update status could not be read instead of rendering a Mod with no update", async () => {
  updates = () => { throw new Error("Update rows unavailable"); };
  renderMods();

  const notice = await screen.findByText(/Update status unknown/i);
  expect(notice).toHaveTextContent("Update rows unavailable");
  expect(screen.queryByRole("button", { name: /^Update → / })).not.toBeInTheDocument();
});

it("says the update status could not be read when the first read of the update rows is paused", async () => {
  onlineManager.setOnline(false);
  renderMods((client) => client.setQueryData(["mods", "srmi"], [outfit]));

  const notice = await screen.findByText(/Update status unknown/i);
  expect(notice).toHaveTextContent(/waiting for a connection/i);
  expect(screen.queryByRole("button", { name: /^Update → / })).not.toBeInTheDocument();
});

it("keeps the per-Mod update check usable when the update rows could not be read", async () => {
  updates = () => { throw new Error("Update rows unavailable"); };
  renderMods();

  const group = await screen.findByRole("group", { name: "Set update checks for March Outfit" });
  await userEvent.click(within(group).getByRole("button", { name: "Don't check" }));

  await waitFor(() => expect(invoke).toHaveBeenCalledWith(
    "set_mod_update_check_enabled", { modId: outfit.id, enabled: false },
  ));
});

it("marks an update offer as unverified when the update-row refresh is paused", async () => {
  const { client } = renderMods();
  expect(await screen.findByRole("button", { name: "Update → 2.0" })).toBeVisible();

  await pause(client, ["modUpdates", "srmi"]);

  await waitFor(() => expect(screen.getByText(/update status unverified/i)).toBeVisible());
  // The offer GMM did read stays; it is now marked as not re-read.
  expect(screen.getByRole("button", { name: "Update → 2.0" })).toBeVisible();
});

it("says nothing extra about the update status on an established, current read", async () => {
  renderMods();
  expect(await screen.findByRole("button", { name: "Update → 2.0" })).toBeVisible();
  expect(screen.getByRole("checkbox", { name: "check" })).toBeChecked();
  expect(screen.queryByText(/Update status unknown|update status unverified/i)).not.toBeInTheDocument();
});

// --- ModList / VariantSelector::v — the Variant list ----------------------

it("says the Variant list could not be read instead of rendering a single-folder Mod", async () => {
  variants = () => { throw new Error("Variant list unavailable"); };
  renderMods();

  const notice = await screen.findByText(/could not read this Mod's Variants/i);
  expect(notice).toHaveTextContent("Variant list unavailable");
  expect(screen.queryByRole("radio")).not.toBeInTheDocument();
});

it("says the Variant list could not be read when its first read is paused", async () => {
  onlineManager.setOnline(false);
  renderMods((client) => client.setQueryData(["mods", "srmi"], [outfit]));

  const notice = await screen.findByText(/could not read this Mod's Variants/i);
  expect(notice).toHaveTextContent(/waiting for a connection/i);
  expect(screen.queryByRole("radio")).not.toBeInTheDocument();
});

it("marks the Variant choice as unverified when the Variant refresh is paused", async () => {
  variants = () => twoVariants;
  const { client } = renderMods();
  expect(await screen.findByRole("radio", { name: "Red" })).toBeChecked();

  await pause(client, ["variants", outfit.id]);

  await waitFor(() => expect(screen.getByText(/could not refresh this Mod's Variants/i)).toBeVisible());
  expect(screen.getByRole("radio", { name: "Red" })).toBeChecked();
});

it("stays silent when the refresh of a Mod with no Variant choice is paused", async () => {
  // Deliberately silent. The selector renders once per Mod row, so a line here
  // would land on every single-folder row at once, and a previous read did
  // establish that there is no choice to offer. Only the never-read case speaks.
  const { client } = renderMods();
  await screen.findByText("March Outfit");
  await waitFor(() => expect(screen.queryByRole("radio")).not.toBeInTheDocument());

  await pause(client, ["variants", outfit.id]);

  await waitFor(() => expect(client.getQueryState(["variants", outfit.id])?.fetchStatus).toBe("paused"));
  expect(screen.queryByText(/Variants/i)).not.toBeInTheDocument();
  expect(screen.queryByRole("radio")).not.toBeInTheDocument();
});

it("holds off a Variant switch while a refresh of the Variant list is in flight", async () => {
  // This test is what guarantees the re-render behind VariantSelector's cached
  // fetch-status read: the component re-renders on `fetchStatus` only because
  // `readEvidence` reads it. Without this, that coupling could break silently.
  variants = () => twoVariants;
  const { client } = renderMods();
  expect(await screen.findByRole("radio", { name: "Red" })).toBeEnabled();

  let finish!: (value: ModVariants) => void;
  variants = () => new Promise((resolve) => { finish = resolve; });
  await act(async () => { void client.invalidateQueries({ queryKey: ["variants", outfit.id] }); });

  await waitFor(() => expect(screen.getByRole("radio", { name: "Blue" })).toBeDisabled());
  expect(screen.getByRole("radio", { name: "Red" })).toBeDisabled();

  await act(async () => finish(twoVariants));
  await waitFor(() => expect(screen.getByRole("radio", { name: "Blue" })).toBeEnabled());
});
