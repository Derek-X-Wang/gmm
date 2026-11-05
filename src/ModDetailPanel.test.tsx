// @ts-expect-error Node types are not part of the frontend; Vitest provides this test-only runtime.
import { readFileSync } from "node:fs";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ModDetailPanel } from "./ModDetailPanel";
import { ModList } from "./ModList";
import { makeQueryClient, renderWithQuery } from "./test/harness";
import type { Mod } from "./api";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));

const outfit: Mod = {
  id: "outfit", name: "March Outfit", game: "srmi", source: "gamebanana", enabled: false,
  libraryPath: "C:/Library/srmi/outfit", author: "Outfit Author", version: "1.0",
  gamebananaId: 123, sourceUrl: "https://gamebanana.com/mods/123",
  screenshotUrl: "https://images.gamebanana.com/img/ss/mods/outfit.jpg",
  reinstallRecovery: null, enabledTransitionRecovery: null,
};
let storedWidth: number | null;
let rawMods: object[];
let failWidthWrite: boolean;
let failWidthRead: boolean;
let screenshotAnswer: unknown;

beforeEach(() => {
  screenshotAnswer = { status: "unavailable", reason: "GameBanana answered with status 404." };
  storedWidth = null;
  failWidthWrite = false;
  failWidthRead = false;
  rawMods = [
    { id: "outfit", name: "March Outfit", game: "srmi", source: "local", library_path: "C:/Library/srmi/outfit", enabled: false, author: "First Author" },
    { id: "other", name: "Other Outfit", game: "srmi", source: "local", library_path: "C:/Library/srmi/other", enabled: false, author: "Second Author" },
  ];
  invoke.mockImplementation(async (command: string, args?: { width?: number }) => {
    if (command === "get_mod_panel_width") {
      if (failWidthRead) throw new Error("Width read unavailable");
      return storedWidth;
    }
    if (command === "set_mod_panel_width") {
      if (failWidthWrite) throw new Error("Width write unavailable");
      storedWidth = args!.width!;
      return null;
    }
    if (command === "get_mod_screenshot") return screenshotAnswer;
    if (command === "list_mods") return rawMods;
    if (command === "detect_conflicts") return { conflicts: [], omitted: [], per_mod_count: {} };
    if (command === "list_variants") return { variants: [], activeVariantId: null };
    if (command === "list_mod_updates") return [{ modId: "outfit", upstreamVersion: "2.0" }];
    return null;
  });
});

function renderPanel(mod: Mod | null = outfit) {
  const client = makeQueryClient();
  client.setDefaultOptions({ queries: { retry: false }, mutations: { retry: false } });
  return { ...renderWithQuery(<ModDetailPanel id="inspector" open mod={mod} onClose={vi.fn()} />, { client }), client };
}

it("shows recorded metadata using the Library-audit definition-list convention", async () => {
  renderPanel();
  const panel = screen.getByRole("complementary", { name: "March Outfit" });
  expect(panel.querySelector("dl")).toHaveClass("duplicate-mod-details");
  for (const fact of [outfit.libraryPath, "Outfit Author", "gamebanana", "1.0", "submission 123"]) {
    expect(within(panel).getByText(fact)).toBeVisible();
  }
  expect(await within(panel).findByText("2.0")).toBeVisible();
  expect(within(panel).getByRole("link", { name: "View submission" })).toHaveAttribute("href", outfit.sourceUrl);
  expect(within(panel).queryByRole("img")).not.toBeInTheDocument();
  expect(within(panel).queryByRole("checkbox")).not.toBeInTheDocument();
  expect(within(panel).queryByRole("radio")).not.toBeInTheDocument();
  expect(panel).not.toHaveTextContent(/Variant|Conflict|recovery/);
});

it.each([null, "", "   "])("omits unread fields instead of substituting values (%s)", (missing) => {
  const { container } = renderPanel({ ...outfit, source: "local", libraryPath: missing as unknown as string,
    author: missing, version: missing, screenshotUrl: missing, gamebananaId: null, sourceUrl: missing });
  expect([...container.querySelectorAll("dt")].map((term) => term.textContent)).toEqual(["Source"]);
  expect([...container.querySelectorAll("dd")].map((value) => value.textContent)).toEqual(["local"]);
  expect(screen.queryByRole("img")).not.toBeInTheDocument();
  expect(screen.queryByRole("link")).not.toBeInTheDocument();
});

it("states explicitly when no Mod is selected", () => {
  renderPanel(null);
  expect(within(screen.getByRole("complementary", { name: "Mod details" }))
    .getByText("Select a Mod to view its details.")).toBeVisible();
});

it("does not invent a submission id when only its URL is known", () => {
  renderPanel({ ...outfit, gamebananaId: null });
  expect(screen.queryByText(/submission \d/)).not.toBeInTheDocument();
  expect(screen.getByRole("link", { name: "View submission" })).toHaveAttribute("href", outfit.sourceUrl);
});

it("links a known submission even when its stored URL is absent", () => {
  renderPanel({ ...outfit, sourceUrl: null });
  expect(screen.getByRole("link", { name: "View submission" })).toHaveAttribute("href", "https://gamebanana.com/mods/123");
});

it("does not invent upstream version while its read is unresolved", () => {
  const implementation = invoke.getMockImplementation()!;
  invoke.mockImplementation((command, args) => command === "list_mod_updates" ? new Promise(() => {}) : implementation(command, args));
  renderPanel();
  expect(screen.queryByText("Last seen upstream")).not.toBeInTheDocument();
});

it("restores remembered width after closing and reopening in a fresh mount", async () => {
  const first = renderPanel();
  await waitFor(() => expect(first.client.isFetching()).toBe(0));
  fireEvent.change(screen.getByRole("slider", { name: "Panel width" }), { target: { value: "460" } });
  await waitFor(() => expect(storedWidth).toBe(460));
  first.unmount();
  const reopened = renderPanel();
  await waitFor(() => expect(reopened.client.isFetching()).toBe(0));
  expect(screen.getByRole("slider", { name: "Panel width" })).toHaveValue("460");
  expect(screen.getByRole("complementary")).toHaveStyle({ flexBasis: "460px" });
});

it("reports width read and write failures without claiming persistence", async () => {
  failWidthRead = true;
  failWidthWrite = true;
  renderPanel();
  expect(await screen.findByText("Width read unavailable")).toBeVisible();
  fireEvent.change(screen.getByRole("slider"), { target: { value: "420" } });
  expect(await screen.findByText("Width write unavailable")).toBeVisible();
  expect(storedWidth).toBeNull();
});

afterEach(() => { vi.useRealTimers(); });

it.each(["pointer", "keyboard"])("persists many %s width changes with exactly one write after settling", async (method) => {
  const { client } = renderPanel();
  await waitFor(() => expect(client.isFetching()).toBe(0));
  vi.useFakeTimers();
  const slider = screen.getByRole("slider");
  if (method === "pointer") fireEvent.pointerDown(slider);
  for (let value = 440; value <= 480; value++) {
    if (method === "keyboard") fireEvent.keyDown(slider, { key: "ArrowRight" });
    // jsdom does not implement the range input's native arrow-key default action.
    fireEvent.change(slider, { target: { value: String(value) } });
    if (method === "keyboard") fireEvent.keyUp(slider, { key: "ArrowRight" });
    expect(screen.getByRole("complementary")).toHaveStyle({ flexBasis: `${value}px` });
  }
  if (method === "pointer") fireEvent.pointerUp(slider);
  await act(async () => { await vi.advanceTimersByTimeAsync(249); });
  expect(invoke.mock.calls.filter(([command]) => command === "set_mod_panel_width")).toHaveLength(0);
  await act(async () => { await vi.advanceTimersByTimeAsync(1); });
  expect(invoke.mock.calls.filter(([command]) => command === "set_mod_panel_width")).toHaveLength(1);
  expect(storedWidth).toBe(480);
});

it("flushes a pending width on blur without writing again after settling", async () => {
  const { client } = renderPanel();
  await waitFor(() => expect(client.isFetching()).toBe(0));
  vi.useFakeTimers();
  const slider = screen.getByRole("slider");
  fireEvent.change(slider, { target: { value: "420" } });
  await act(async () => { fireEvent.blur(slider); });
  expect(invoke.mock.calls.filter(([command]) => command === "set_mod_panel_width")).toHaveLength(1);
  expect(storedWidth).toBe(420);
  await act(async () => { await vi.advanceTimersByTimeAsync(250); });
  expect(invoke.mock.calls.filter(([command]) => command === "set_mod_panel_width")).toHaveLength(1);
  expect(storedWidth).toBe(420);
});

it("keeps settled width writes ordered despite out-of-order latency", async () => {
  const { client } = renderPanel();
  await waitFor(() => expect(client.isFetching()).toBe(0));
  const implementation = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (command, args) => {
    if (command !== "set_mod_panel_width") return implementation(command, args);
    await new Promise((resolve) => setTimeout(resolve, args.width === 440 ? 1000 : 10));
    storedWidth = args.width;
    return null;
  });
  vi.useFakeTimers();
  fireEvent.change(screen.getByRole("slider"), { target: { value: "440" } });
  await act(async () => { await vi.advanceTimersByTimeAsync(250); });
  fireEvent.change(screen.getByRole("slider"), { target: { value: "480" } });
  await act(async () => { await vi.advanceTimersByTimeAsync(250); });
  await act(async () => { await vi.advanceTimersByTimeAsync(1100); });
  expect(client.getQueryData(["modPanelWidth"])).toBe(480);
  expect(storedWidth).toBe(480);
  expect(invoke.mock.calls.filter(([command]) => command === "set_mod_panel_width").map(([, args]) => args.width)).toEqual([440, 480]);
});

it("opens beside the live list, follows selection, closes with row focus, and reopens at its width", async () => {
  const user = userEvent.setup();
  const client = makeQueryClient();
  const rendered = renderWithQuery(<ModList game="srmi" displayName="Star Rail" />, { client });
  const first = await screen.findByRole("listitem", { name: "March Outfit" });
  const second = screen.getByRole("listitem", { name: "Other Outfit" });
  expect(screen.queryByRole("complementary")).not.toBeInTheDocument();
  const hint = screen.getByText("Select a Mod to show its details. Use Up and Down to move between Mods, or Enter or Space to select.");
  expect(hint).toBeVisible();
  expect(hint).toHaveClass("muted", "small");
  expect(hint).not.toHaveClass("visually-hidden");
  expect(screen.getByRole("list")).toHaveAccessibleDescription(hint.textContent!);
  for (const row of [first, second]) expect(row).not.toHaveAttribute("aria-describedby");
  await user.click(first);
  let panel = screen.getByRole("complementary", { name: "March Outfit" });
  expect(first).toHaveAttribute("aria-controls", panel.id);
  expect(second).toHaveAttribute("aria-controls", panel.id);
  expect(first).not.toHaveAttribute("role");
  expect(screen.queryByRole("grid")).not.toBeInTheDocument();
  expect(screen.queryByRole("option")).not.toBeInTheDocument();
  expect(screen.getByRole("list")).toBeVisible();
  expect(within(panel).getByText("First Author")).toBeVisible();
  await waitFor(() => expect(within(second).getByRole("checkbox")).toBeEnabled());
  await user.click(within(second).getByRole("checkbox"));
  panel = screen.getByRole("complementary", { name: "Other Outfit" });
  expect(within(panel).getByText("Second Author")).toBeVisible();
  expect(invoke).toHaveBeenCalledWith("set_mod_enabled", { id: "other", enabled: true, game: "srmi" });
  fireEvent.change(screen.getByRole("slider"), { target: { value: "420" } });
  await user.click(screen.getByRole("button", { name: "Close Mod details" }));
  expect(second).toHaveFocus();
  expect(screen.queryByRole("complementary")).not.toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Show Mod details" }));
  expect(screen.getByRole("slider")).toHaveValue("420");
  expect(screen.getByRole("complementary", { name: "Other Outfit" })).toBeVisible();
  rawMods = [];
  await act(async () => { await client.invalidateQueries({ queryKey: ["mods", "srmi"] }); });
  await waitFor(() => expect(screen.getByText("Select a Mod to view its details.")).toBeVisible());
  rendered.unmount();
});

it("can open an explicit empty inspector before selecting a row", async () => {
  renderWithQuery(<ModList game="srmi" displayName="Star Rail" />);
  await screen.findByRole("listitem", { name: "March Outfit" });
  await userEvent.click(screen.getByRole("button", { name: "Show Mod details" }));
  expect(screen.getByText("Select a Mod to view its details.")).toBeVisible();
  expect(screen.getByRole("button", { name: "Close Mod details" })).toHaveFocus();
});

const authored = () => readFileSync("src/App.css", "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
it("pins the selected row rail declaration", () => {
  expect(authored()).toMatch(/\.mods__row--selected\s*\{\s*box-shadow:\s*inset 4px 0 0 currentColor;\s*\}/);
});
it("pins the separate keyboard focus outline", () => {
  expect(authored()).toMatch(/\.mods__row:focus-visible\s*\{\s*outline:\s*3px solid currentColor;\s*outline-offset:\s*2px;\s*\}/);
});
it("pins the forced-colors selection fallback", () => {
  expect(authored()).toMatch(/@media\s*\(forced-colors:\s*active\)\s*\{\s*\.mods__row--selected\s*\{\s*border-inline-start:\s*5px double Highlight;\s*padding-inline-start:\s*calc\(1rem - 4px\);\s*\}/);
});
it("gives the list 400 CSS pixels and makes the inspector yield first", () => {
  expect(authored()).toMatch(/\.mod-workspace__list\s*\{\s*flex:\s*1 0 400px;/);
  expect(authored()).toMatch(/\.mod-detail-panel\s*\{\s*flex:\s*0 1 320px;\s*min-width:\s*0;/);
});

it("pins slider bounds to the Rust panel width constants", () => {
  const rust = readFileSync("src-tauri/src/core/mod_panel.rs", "utf8");
  const tsx = readFileSync("src/ModDetailPanel.tsx", "utf8");
  const bounds = tsx.match(/type="range" min="(\d+)" max="(\d+)"/)!;
  expect(Number(bounds[1])).toBe(Number(rust.match(/const MIN_WIDTH: u32 = (\d+);/)![1]));
  expect(Number(bounds[2])).toBe(Number(rust.match(/const MAX_WIDTH: u32 = (\d+);/)![1]));
});

it.each([false, true])("reports unavailable upstream readings including failed refetch with cached data (%s)", async (cached) => {
  const implementation = invoke.getMockImplementation()!;
  const rendered = cached ? renderPanel() : null;
  if (cached) await screen.findByText("2.0");
  invoke.mockImplementation((command, args) => command === "list_mod_updates"
    ? Promise.reject(new Error("Upstream read failed")) : implementation(command, args));
  if (rendered) {
    await act(async () => { await rendered.client.invalidateQueries({ queryKey: ["modUpdates"] }); });
  } else renderPanel();
  expect(await screen.findByText("Upstream read failed")).toBeVisible();
  expect(screen.getByText(/Upstream information is unavailable; this is not an “up to date” result/)).toBeVisible();
  if (cached) expect(screen.getByText("2.0")).toBeVisible();
});


it("removes the upstream error when selecting a local Mod in the same Game", async () => {
  const implementation = invoke.getMockImplementation()!;
  invoke.mockImplementation((command, args) => command === "list_mod_updates"
    ? Promise.reject(new Error("Upstream read failed")) : implementation(command, args));
  rawMods = [{ ...rawMods[0], source: "gamebanana" }, rawMods[1]];
  const user = userEvent.setup();
  renderWithQuery(<ModList game="srmi" displayName="Star Rail" />);
  await user.click(await screen.findByRole("listitem", { name: "March Outfit" }));
  const heading = "GMM could not read this Mod's upstream version.";
  expect(await within(screen.getByRole("complementary", { name: "March Outfit" })).findByText(heading)).toBeVisible();
  await user.click(screen.getByRole("listitem", { name: "Other Outfit" }));
  expect(screen.getByRole("complementary", { name: "Other Outfit" })).toBeVisible();
  expect(screen.queryByText(heading)).not.toBeInTheDocument();
  expect(screen.queryByText("Upstream read failed")).not.toBeInTheDocument();
});

it("flushes a pending width when the panel unmounts while the app lives", async () => {
  const panel = renderPanel();
  await waitFor(() => expect(panel.client.isFetching()).toBe(0));
  vi.useFakeTimers();
  fireEvent.change(screen.getByRole("slider"), { target: { value: "420" } });
  fireEvent.change(screen.getByRole("slider"), { target: { value: "480" } });
  expect(invoke.mock.calls.filter(([command]) => command === "set_mod_panel_width")).toHaveLength(0);
  await act(async () => { panel.unmount(); });
  expect(invoke).toHaveBeenCalledWith("set_mod_panel_width", { width: 480 });
  expect(panel.client.getQueryData(["modPanelWidth"])).toBe(480);
  expect(storedWidth).toBe(480);
  await act(async () => { await vi.advanceTimersByTimeAsync(250); });
  expect(invoke.mock.calls.filter(([command]) => command === "set_mod_panel_width")).toHaveLength(1);
});

// ---- Screenshot (#437) -----------------------------------------------------

const DATA_URI = "data:image/png;base64,iVBORw0KGgo=";
const screenshotCalls = () => invoke.mock.calls.filter(([command]) => command === "get_mod_screenshot");
const figureOf = (container: HTMLElement) => container.querySelector("figure.mod-detail-panel__screenshot");

it.each([null, "", "   "])("renders nothing and asks for nothing when the Mod has no screenshot (%j)", async (missing) => {
  const { container, client } = renderPanel({ ...outfit, screenshotUrl: missing });
  await waitFor(() => expect(client.isFetching()).toBe(0));
  expect(figureOf(container)).toBeNull();
  expect(screen.queryByText(/screenshot/i)).not.toBeInTheDocument();
  expect(screenshotCalls()).toHaveLength(0);
});

it("shows the bounded box at once, before the Core has answered", async () => {
  const implementation = invoke.getMockImplementation()!;
  invoke.mockImplementation((command, args) => command === "get_mod_screenshot" ? new Promise(() => {}) : implementation(command, args));
  const { container } = renderPanel();
  expect(figureOf(container)).toBeInTheDocument();
  expect(within(figureOf(container) as HTMLElement).getByText("Loading screenshot…")).toBeVisible();
  expect(screen.queryByRole("img")).not.toBeInTheDocument();
  expect(screen.queryByText("No screenshot available")).not.toBeInTheDocument();
});

it("renders the Core's data URI and never the remote address", async () => {
  screenshotAnswer = { status: "image", dataUri: DATA_URI };
  const { container } = renderPanel();
  const image = await screen.findByRole("img", { name: "Screenshot of March Outfit" });
  expect(image).toHaveAttribute("src", DATA_URI);
  expect(container.innerHTML).not.toContain(outfit.screenshotUrl);
  expect(invoke).toHaveBeenCalledWith("get_mod_screenshot", { modId: "outfit" });
});

it("says the image is unavailable, with the reason, and does not blame the Mod", async () => {
  screenshotAnswer = { status: "unavailable", reason: "Proxy unreachable: connection refused." };
  const { container } = renderPanel();
  const figure = figureOf(container) as HTMLElement;
  expect(await within(figure).findByText("No screenshot available")).toBeVisible();
  expect(within(figure).getByText("Proxy unreachable: connection refused.")).toBeVisible();
  expect(within(figure).getByText("Proxy unreachable: connection refused.")).toHaveClass("small");
  expect(screen.queryByRole("img")).not.toBeInTheDocument();
  expect(screen.getByRole("complementary")).not.toHaveTextContent(/faulty|broken|corrupt|damaged|invalid Mod/i);
});

it("reads a failed request as an unavailable image, not a faulty Mod", async () => {
  const implementation = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (command, args) => {
    if (command === "get_mod_screenshot") throw new Error("IPC closed");
    return implementation(command, args);
  });
  const { container } = renderPanel();
  const figure = figureOf(container) as HTMLElement;
  expect(await within(figure).findByText("No screenshot available")).toBeVisible();
  expect(within(figure).getByText("GMM could not ask for the screenshot.")).toBeVisible();
});

it("places the screenshot below the heading and above the facts", async () => {
  const { container } = renderPanel();
  await waitFor(() => expect(screen.getByText("No screenshot available")).toBeVisible());
  const heading = screen.getByRole("heading", { name: "March Outfit" });
  const figure = figureOf(container) as HTMLElement;
  const facts = container.querySelector("dl") as HTMLElement;
  expect(heading.compareDocumentPosition(figure) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(figure.compareDocumentPosition(facts) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
});

it("asks again for the Mod that is now selected", async () => {
  const first = renderPanel();
  await waitFor(() => expect(screenshotCalls()).toHaveLength(1));
  first.rerender(<ModDetailPanel id="inspector" open
    mod={{ ...outfit, id: "other", name: "Other Outfit", screenshotUrl: "https://images.gamebanana.com/img/ss/mods/other.jpg" }}
    onClose={vi.fn()} />);
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("get_mod_screenshot", { modId: "other" }));
});

it("bounds the screenshot box and its image, whatever the panel width", () => {
  const authored: string = readFileSync("src/App.css", "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
  const rule = (selector: string) => {
    const matches = [...authored.matchAll(/([^{}]+)\{([^{}]*)\}/g)]
      .filter(([, selectors]) => selectors.split(",").some((raw) => raw.trim() === selector));
    expect(matches, `one rule owns ${selector}`).toHaveLength(1);
    return matches[0][2];
  };
  // The invariant is that a bound exists, not its pixel count.
  expect(rule(".mod-detail-panel__screenshot")).toMatch(/(?:^|;)\s*height\s*:\s*\d+px\s*;/);
  expect(rule(".mod-detail-panel__screenshot")).toMatch(/(?:^|;)\s*overflow\s*:\s*hidden\s*;/);
  const image = rule(".mod-detail-panel__screenshot img");
  expect(image).toMatch(/(?:^|;)\s*max-width\s*:\s*100%\s*;/);
  expect(image).toMatch(/(?:^|;)\s*max-height\s*:\s*100%\s*;/);
  expect(image).toMatch(/(?:^|;)\s*object-fit\s*:\s*contain\s*;/);
});
