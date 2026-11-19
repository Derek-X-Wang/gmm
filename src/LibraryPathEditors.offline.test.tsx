import { onlineManager } from "@tanstack/react-query";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { GlobalLibraryRootEditor } from "./LibraryPathEditors";
import type { GameSummary, LibraryPaths, MoveReport } from "./api";
import { renderWithQuery } from "./test/harness";

// #310. The Library card derives `busy` from `useIsMutating({ mutationKey: moveKey })`,
// and no mutation site anywhere checks `isPaused`. If a Library-root write is
// paused by the online manager, every control in the card stays disabled for
// as long as the webview says it is offline, with nothing on screen to say why.
// Setting a Library root is a local filesystem and SQLite operation, so it must
// run regardless of what the webview believes about connectivity.

const { invoke, openDialog } = vi.hoisted(() => ({ invoke: vi.fn(), openDialog: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: openDialog }));

const games: GameSummary[] = [
  { code: "gimi", displayName: "Genshin Impact" },
  { code: "srmi", displayName: "Honkai: Star Rail" },
];
const report: MoveReport = {
  relocated: ["mod-a"], moved_directories: ["D:\\Library"],
  failed_junction_restores: [], overlap_repair: false,
};
let paths: LibraryPaths;

beforeEach(() => {
  paths = {
    defaultRoot: "C:\\GMM\\library", effectiveRoot: "C:\\GMM\\library", rootOverride: null,
    perGameOverrides: {}, perGameEffective: { gimi: "C:\\GMM\\library\\gimi", srmi: "C:\\GMM\\library\\srmi" },
    overlaps: [], modOverlaps: [],
  };
  openDialog.mockResolvedValue(null);
  invoke.mockImplementation(async (command: string, args: { path: string | null }) => {
    if (command === "get_library_paths") return structuredClone(paths);
    if (command === "set_library_root") {
      paths.rootOverride = args.path;
      paths.effectiveRoot = args.path ?? paths.defaultRoot;
      return report;
    }
    return null;
  });
});

afterEach(() => {
  onlineManager.setOnline(true);
});

it("moves the Library root and re-enables the card's controls after the webview reports itself offline mid-session", async () => {
  const { client } = renderWithQuery(<GlobalLibraryRootEditor games={games} onGamePage={() => {}} />);
  const field = await screen.findByRole("textbox", { name: "Global Library root" });

  // The card is already showing its paths when the webview fires `offline`.
  onlineManager.setOnline(false);
  await userEvent.clear(field);
  await userEvent.type(field, "D:\\Library{Enter}");

  // The write ran against the local filesystem, and the card reported it. The
  // result is only rendered once `busy` is false, so this is also the proof
  // that the `useIsMutating` gate released.
  expect(await screen.findByText("Library move completed.")).toBeVisible();
  expect(invoke).toHaveBeenCalledWith("set_library_root", { path: "D:\\Library" });
  await waitFor(() => expect(client.isMutating()).toBe(0));
  expect(field).toBeEnabled();
  expect(screen.getByRole("button", { name: "Change global root…" })).toBeEnabled();
  expect(screen.queryByText("Moving…")).not.toBeInTheDocument();
});
