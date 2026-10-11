import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import type { GamePathStatus } from "./api";
import { renderWithQuery } from "./test/harness";
import { GameStatusStrip } from "./GameStatusStrip";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
const savedPath = "C:/StarRail";
const readable: GamePathStatus = { game: "srmi", savedPath, gamePath: { status: "readable" }, modelImporter: { status: "installed" } };
const props = { game: "srmi" as const, displayName: "Honkai: Star Rail", savedPath, pathReadError: null, pathReadPending: false, onSettings: vi.fn() };

beforeEach(() => { invoke.mockResolvedValue(readable); props.onSettings.mockClear(); });

it.each([
  [{ ...readable, gamePath: { status: "unreadable", path: savedPath, error: "Access denied" } }, "Cannot tell — install path unreadable · Model Importer unverified", "Access denied"],
  [{ ...readable, gamePath: { status: "savedPathNotFound", paths: ["C:/StarRail/StarRail.exe"] } }, "Saved installation cannot be found · Model Importer unverified", "C:/StarRail/StarRail.exe"],
  [{ ...readable, modelImporter: { status: "requiredEntriesNotFound", paths: ["C:/StarRail/d3dx.ini"] } }, "Game readable · Model Importer not installed", "C:/StarRail/d3dx.ini"],
  [{ ...readable, modelImporter: { status: "unverified", path: "C:/StarRail/Core", error: "Sharing violation" } }, "Game readable · Model Importer unverified", "Sharing violation"],
  [{ ...readable, gamePath: { status: "invalid", error: "Invalid layout" } }, "Cannot verify Game layout · Model Importer unverified", "Invalid layout"],
])("reports verified path evidence without inferring an installed version: %s", async (value, line, detail) => {
  invoke.mockResolvedValue(value);
  renderWithQuery(<GameStatusStrip {...props} />);
  const strip = screen.getByRole("region", { name: "Honkai: Star Rail status" });
  expect(await within(strip).findByText(line as string)).toBeVisible();
  expect(strip).toHaveTextContent(detail as string);
  expect(strip).toHaveTextContent(savedPath);
  expect(within(strip).getByText(savedPath, { selector: "code" })).toBeVisible();
  if (line !== "Game readable · Model Importer not installed") expect(strip).not.toHaveTextContent(/not installed|no saved/i);
  expect(strip).not.toHaveTextContent(/version|v1\./i);
  const settings = within(strip).getByRole("button", { name: "Settings for Honkai: Star Rail" });
  expect(settings).toHaveAttribute("data-settings-arrival", "game");
  await userEvent.click(settings);
  expect(props.onSettings).toHaveBeenCalledTimes(1);
  expect(invoke.mock.calls.map(([command]) => command)).toEqual(["get_game_path_status"]);
});

it("shows checking while the status read is pending and preserves the saved path", async () => {
  let resolve!: (value: GamePathStatus) => void;
  invoke.mockImplementation(() => new Promise<GamePathStatus>((yes) => { resolve = yes; }));
  renderWithQuery(<GameStatusStrip {...props} />);
  expect(screen.getByText("Checking Game install path · Model Importer unverified")).toBeVisible();
  expect(screen.getByRole("region", { name: "Honkai: Star Rail status" })).toHaveTextContent(`Last saved path: ${savedPath}`);
  expect(screen.queryByText(/Model Importer not installed/)).not.toBeInTheDocument();
  await act(async () => resolve(readable));
  expect(await screen.findByText("Game readable · Model Importer details in Settings")).toBeVisible();
});

it("does not reuse a cached readable result after the saved-path read fails", async () => {
  const { rerender } = renderWithQuery(<GameStatusStrip {...props} />);
  await screen.findByText("Game readable · Model Importer details in Settings");
  rerender(<GameStatusStrip {...props} pathReadError={new Error("Settings database locked")} />);
  expect(screen.getByRole("status")).toHaveTextContent("Cannot tell — install path unreadable · Model Importer unverified");
  expect(screen.getByRole("status")).toHaveTextContent("Settings database locked");
  expect(screen.getByRole("status")).toHaveTextContent(`Last saved path: ${savedPath}`);
  expect(screen.queryByText(/Game readable/)).not.toBeInTheDocument();
});

it("does not reuse a cached readable result after the filesystem check fails", async () => {
  const { client } = renderWithQuery(<GameStatusStrip {...props} />);
  await screen.findByText("Game readable · Model Importer details in Settings");
  invoke.mockRejectedValue(new Error("Storage unavailable"));
  await act(() => client.invalidateQueries({ queryKey: ["gamePathStatus", "srmi"] }));
  await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Storage unavailable"));
  expect(screen.getByRole("status")).toHaveTextContent("Cannot tell — install path unreadable · Model Importer unverified");
  expect(screen.getByRole("status")).not.toHaveTextContent(/Game readable|not installed|version/i);
});
