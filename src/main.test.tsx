import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { ReactNode } from "react";

const { check, relaunch, values } = vi.hoisted(() => ({ check: vi.fn(), relaunch: vi.fn(), values: new Map<string, string>() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async (command, args) => {
  if (command === "get_skipped_update") return values.get("skip") ?? null;
  if (command === "set_skipped_update") { values.set("skip", args.build); return; }
  throw new Error(`Unexpected command: ${command}`);
}) }));
vi.mock("@tauri-apps/plugin-updater", () => ({ check }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch }));
vi.mock("./App", () => ({ default: () => <p>Game Library</p> }));
vi.mock("./diagnostics", () => ({ installGlobalErrorHandlers: vi.fn() }));
vi.mock("react-dom/client", () => ({ default: { createRoot: () => ({ render: (node: ReactNode) => render(node) }) } }));

beforeEach(() => {
  vi.resetModules();
  values.clear();
  check.mockReset();
  relaunch.mockReset();
});

it("the actual startup entry checks and renders consent before issuing any install", async () => {
  const downloadAndInstall = vi.fn().mockResolvedValue(undefined);
  check.mockResolvedValue({ version: "0.5.0", rawJson: { url: "https://example.com/alpha.2.msi", signature: "signed-alpha.2" }, downloadAndInstall });
  await act(async () => { await import("./main"); });
  expect(check).toHaveBeenCalledOnce();
  expect(screen.getByText("Game Library")).toBeVisible();
  expect(downloadAndInstall).not.toHaveBeenCalled();
  expect(screen.getByText(/is available/)).toHaveTextContent("0.5.0");
  expect(relaunch).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole("button", { name: "Install update" }));
  expect(downloadAndInstall).toHaveBeenCalledOnce();
  expect(relaunch).not.toHaveBeenCalled();
  expect(screen.getByRole("button", { name: "Restart to apply" })).toBeVisible();
});

it("the startup entry surfaces a failed background check without blocking the Library", async () => {
  check.mockRejectedValue(new Error("GitHub 503"));
  await act(async () => { await import("./main"); });
  expect(screen.getByText("Game Library")).toBeVisible();
  expect(screen.getByRole("alert")).toHaveTextContent("Update check failed: GitHub 503");
  expect(screen.queryByText(/latest release/)).not.toBeInTheDocument();
  expect(relaunch).not.toHaveBeenCalled();
});

afterEach(() => vi.useRealTimers());

it("the actual startup entry respects a persisted skip after a simulated restart", async () => {
  const downloadAndInstall = vi.fn();
  check.mockResolvedValue({ version: "0.4.0", rawJson: { url: "https://example.com/alpha.2.msi", signature: "signed-alpha.2" }, downloadAndInstall });
  await act(async () => { await import("./main"); });
  await userEvent.click(screen.getByRole("button", { name: "Skip this build" }));
  expect(screen.getByText(/was skipped/)).toBeVisible();
  cleanup();
  vi.resetModules();
  await act(async () => { await import("./main"); });
  expect(check).toHaveBeenCalledTimes(2);
  expect(screen.getByText("Game Library")).toBeVisible();
  expect(screen.queryByLabelText("GMM update")).not.toBeInTheDocument();
  expect(downloadAndInstall).not.toHaveBeenCalled();
});

it("the actual startup offer stays unanswered without installing while time passes", async () => {
  vi.useFakeTimers();
  const downloadAndInstall = vi.fn().mockResolvedValue(undefined);
  check.mockResolvedValue({ version: "0.5.0", downloadAndInstall });
  await act(async () => { await import("./main"); });
  expect(screen.getByRole("button", { name: "Install update" })).toBeVisible();
  // Drain pending work without imposing a fixed deadline on consent.
  await act(async () => { await vi.runOnlyPendingTimersAsync(); });
  for (const visibility of ["hidden", "visible"] as const) {
    const visibilityState = vi.spyOn(document, "visibilityState", "get").mockReturnValue(visibility);
    for (const target of [window, document]) {
      for (const event of ["focus", "visibilitychange"]) {
        await act(async () => {
          target.dispatchEvent(new Event(event));
          await vi.runOnlyPendingTimersAsync();
        });
        expect(downloadAndInstall).not.toHaveBeenCalled();
      }
    }
    visibilityState.mockRestore();
  }
  expect(downloadAndInstall).not.toHaveBeenCalled();
  expect(screen.getByRole("button", { name: "Install update" })).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: "Install update" }));
  await act(async () => {});
  expect(downloadAndInstall).toHaveBeenCalledOnce();
});
