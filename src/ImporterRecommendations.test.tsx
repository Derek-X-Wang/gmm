import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import { makeQueryClient, renderWithQuery } from "./test/harness";
import { ImporterRecommendations } from "./ImporterRecommendations";
import { ImporterOriginPanel } from "./ImporterOriginPanel";
import { ImporterPanel } from "./ImporterPanel";
import type { GameCode, GameSummary } from "./api";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
const games: GameSummary[] = [
  { code: "gimi", displayName: "Genshin Impact" },
  { code: "srmi", displayName: "Honkai: Star Rail" },
];
let enabled: boolean;
const curated = { kind: "gitHubRelease", owner: "Curated", repo: "Package", asset_pattern: "Package.zip" };
const fallback = { ...curated, owner: "BuiltIn" };
const dismissed = { ...curated, repo: "DeclinedPackage" };
function renderPreference() {
  return renderWithQuery(<ImporterRecommendations games={games} onGamePage={vi.fn()} />);
}
beforeEach(() => {
  enabled = true;
  invoke.mockReset().mockImplementation(async (command, args: { enabled: boolean; game: GameCode }) => {
    switch (command) {
      case "importer_recommendations_enabled": return enabled;
      case "set_importer_recommendations_enabled": enabled = args.enabled; return;
      case "get_game_path_status": return {
        game: args.game, savedPath: "C:/Game", gamePath: { status: "readable" },
        modelImporter: enabled ? { status: "installed" } : { status: "unverified", path: "C:/Game/Core", error: "Path re-read after preference change" },
      };
      case "importer_origin_status": return {
        game: args.game, displayName: args.game, resolved: { state: "inEffect", origin: enabled ? curated : fallback, layer: enabled ? "recommendedManifest" : "compiledInDefault" },
        installed: { state: "known", ...fallback }, installTarget: { state: "installed", ...fallback },
        userOverride: { state: "notSet" }, compiledDefault: fallback,
        proposal: enabled ? { origin: curated, replaces: { state: "known", ...fallback }, reason: "Maintained package" } : null,
        dismissed: enabled ? [dismissed] : [], dismissalsError: null,
        recommendationsEnabled: enabled, recommendationsUnusableReason: null,
      };
      case "fetch_latest_importer_release": return { tag_name: enabled ? "curated-v2" : "built-in-v1" };
      case "check_importer_update": return { available: false, installedVersion: "1.0", pinned: false, latestVersion: "1.0", upstreamAhead: false, checkError: null };
      case "check_loader_update": return { shippedVersion: "0.9.2", latestVersion: "0.9.3", upstreamAhead: true, checkError: null };
      default: return null;
    }
  });
});

it("switches the all-Games preference and displays the saved value and explanation", async () => {
  renderPreference();
  const toggle = await screen.findByRole("checkbox", { name: /recommend/i });
  expect(toggle).toBeChecked();
  expect(screen.getByText(/Turning this off removes/)).toHaveTextContent(/dismissed recommendations stay saved/);
  await userEvent.click(toggle);
  expect(await screen.findByText("Recommendations preference saved.")).toBeVisible();
  await waitFor(() => expect(toggle).not.toBeChecked());
  expect(invoke).toHaveBeenCalledWith("set_importer_recommendations_enabled", { enabled: false });
});
it("shows loading without inventing a preference", () => {
  invoke.mockImplementation(() => new Promise(() => {}));
  renderPreference();
  expect(screen.getByText("Reading recommendations preference…")).toBeVisible();
  expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
});
it("shows preference read failure", async () => {
  invoke.mockRejectedValue(new Error("Preference unavailable"));
  renderPreference();
  expect(await screen.findByText("Preference unavailable")).toBeVisible();
  expect(screen.getByText("Recommendations preference unavailable.")).toBeVisible();
  expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
});
it("retains the previous value on an atomic rejected preference write", async () => {
  const original = invoke.getMockImplementation()!;
  invoke.mockImplementation((command, args) => {
    if (command === "set_importer_recommendations_enabled") throw { kind: "invalidActiveVariant", message: "Could not change recommendation settings." };
    return original(command, args);
  });
  renderPreference();
  await userEvent.click(await screen.findByRole("checkbox"));
  expect(await screen.findByText("Could not change recommendation settings.")).toHaveAttribute("data-command-failure-kind", "invalidActiveVariant");
  expect(screen.getByRole("checkbox")).toBeChecked();
  expect(screen.queryByText("Recommendations preference saved.")).not.toBeInTheDocument();
});
it("disables a pending preference write and reports completion", async () => {
  const original = invoke.getMockImplementation()!;
  let resolve!: () => void;
  invoke.mockImplementation((command, args) => command === "set_importer_recommendations_enabled" ? new Promise<void>((yes) => { resolve = () => { enabled = args.enabled; yes(); }; }) : original(command, args));
  renderPreference();
  await userEvent.click(await screen.findByRole("checkbox"));
  expect(screen.getByRole("checkbox")).toBeDisabled();
  expect(screen.getByText("Saving recommendations preference…")).toBeVisible();
  await act(async () => resolve());
  expect(await screen.findByText("Recommendations preference saved.")).toBeVisible();
  await waitFor(() => expect(screen.getByRole("checkbox")).not.toBeChecked());
});
it("links every Game to its proposals, dismissals and overrides", async () => {
  const onGamePage = vi.fn();
  renderWithQuery(<ImporterRecommendations games={games} onGamePage={onGamePage} />);
  for (const game of games) {
    await userEvent.click(screen.getByRole("button", { name: `View ${game.displayName} importer proposals, dismissals and overrides` }));
    expect(onGamePage).toHaveBeenLastCalledWith(game.code);
  }
});

it("refreshes the other Game's Origin, importer and path surfaces and preserves dismissals", async () => {
  const client = makeQueryClient();
  client.setDefaultOptions({ queries: { retry: false, staleTime: 30_000, gcTime: 300_000 } });
  renderWithQuery(<>
    <ImporterRecommendations games={games} onGamePage={vi.fn()} />
    {games.map((game) => <div key={game.code} data-testid={game.code}>
      <ImporterPanel game={game.code} displayName={game.displayName} />
      <ImporterOriginPanel game={game.code} displayName={game.displayName} />
    </div>)}
  </>, { client });
  const other = within(screen.getByTestId("srmi"));
  expect(await other.findByText(/In effect:/)).toHaveTextContent("Curated/Package");
  expect(await other.findByText(/Latest release:/)).toHaveTextContent("curated-v2");
  expect(await other.findByText("SRMI installed · Game readable")).toBeVisible();
  expect(await other.findByRole("group", { name: "Dismissed recommendations" })).toHaveTextContent("Curated/DeclinedPackage");
  expect(other.getByRole("group", { name: "GMM recommends a different Importer Origin" })).toBeVisible();
  await waitFor(() => expect(client.isFetching()).toBe(0));
  // The preference is global; neither mounted Game may stay on a fresh cached layer.
  await userEvent.click(screen.getByRole("checkbox", { name: /Recommend importer origins/ }));
  await waitFor(() => expect(other.getByText(/In effect:/)).toHaveTextContent("BuiltIn/Package"));
  await waitFor(() => expect(other.getByText(/Latest release:/)).toHaveTextContent("built-in-v1"));
  expect(await other.findByText("Path re-read after preference change")).toBeVisible();
  expect(other.queryByText("SRMI installed · Game readable")).not.toBeInTheDocument();
  expect(other.queryByRole("group", { name: "GMM recommends a different Importer Origin" })).not.toBeInTheDocument();
  expect(other.queryByRole("group", { name: "Dismissed recommendations" })).not.toBeInTheDocument();
  expect(other.getByRole("button", { name: "Save origin" })).toBeEnabled();
  await waitFor(() => expect(screen.getByRole("checkbox", { name: /Recommend importer origins/ })).toBeEnabled());
  await userEvent.click(screen.getByRole("checkbox", { name: /Recommend importer origins/ }));
  expect(await other.findByRole("group", { name: "Dismissed recommendations" })).toHaveTextContent("Curated/DeclinedPackage");
  // Only the upstream informational Loader note is present; this action installs a Model Importer.
  expect(other.queryByRole("button", { name: /Apply.*Loader|Update.*Loader/ })).not.toBeInTheDocument();
});
