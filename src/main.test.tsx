import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import type { UpdateController } from "./updater";

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

/**
 * Start the application for real and read the offer the launch produced.
 *
 * Since #357 the startup check paints nothing of its own: the shared
 * controller holds what the check found and the status bar inside `<App/>`
 * reads it, so the entry's own behaviour is observable in that controller
 * rather than in a card beside the application. `UpdatesPanel.test.tsx` and
 * `App.updates.test.tsx` own the two surfaces; this file owns the launch.
 */
async function startGmm(): Promise<UpdateController> {
  await act(async () => { await import("./main"); });
  return (await import("./updater")).updater;
}

it("the actual startup entry checks in the background and renders only the application", async () => {
  const downloadAndInstall = vi.fn().mockResolvedValue(undefined);
  check.mockResolvedValue({ version: "0.5.0", rawJson: { url: "https://example.com/alpha.2.msi", signature: "signed-alpha.2" }, downloadAndInstall });

  const updater = await startGmm();

  expect(check).toHaveBeenCalledOnce();
  expect(screen.getByText("Game Library")).toBeVisible();
  // The offer is held for the status bar to read. Nothing was downloaded, and
  // the entry mounted nothing beside the application: the fixed-position
  // notice that used to float in the corner is gone, so the launch cannot
  // paint over the window's own status bar or over the Settings dialog (#357).
  expect(updater.getSnapshot()).toMatchObject({ kind: "available", version: "0.5.0" });
  expect(downloadAndInstall).not.toHaveBeenCalled();
  expect(relaunch).not.toHaveBeenCalled();
  expect(document.body).toHaveTextContent("Game Library");
  expect(document.body).not.toHaveTextContent(/update/i);
  expect(screen.queryByRole("button")).not.toBeInTheDocument();
});

it("the startup entry keeps a failed background check without blocking the Library", async () => {
  check.mockRejectedValue(new Error("GitHub 503"));

  const updater = await startGmm();

  expect(screen.getByText("Game Library")).toBeVisible();
  // A launch that could not check is not a launch that found nothing: the
  // failure and its whole message are kept, for the bar's short reading and
  // for the Settings group's alert, and neither is painted here.
  expect(updater.getSnapshot()).toMatchObject({ kind: "error", failure: "check" });
  expect(updater.getLastError()).toBe("Update check failed: GitHub 503");
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  expect(relaunch).not.toHaveBeenCalled();
});

afterEach(() => vi.useRealTimers());

it("the actual startup entry respects a persisted skip after a simulated restart", async () => {
  const downloadAndInstall = vi.fn();
  check.mockResolvedValue({ version: "0.4.0", rawJson: { url: "https://example.com/alpha.2.msi", signature: "signed-alpha.2" }, downloadAndInstall });

  const updater = await startGmm();
  // The decline itself belongs to the Settings group's **Skip this build**
  // button (`UpdatesPanel.test.tsx`); what this file owns is the next launch.
  await act(() => updater.decline());
  expect(updater.getSnapshot()).toMatchObject({ kind: "declined", version: "0.4.0" });
  cleanup();
  vi.resetModules();

  const restarted = await startGmm();

  expect(check).toHaveBeenCalledTimes(2);
  expect(screen.getByText("Game Library")).toBeVisible();
  // Remembered across the restart, so the launch has nothing to offer and
  // the bar has nothing to read.
  expect(restarted.getSnapshot()).toEqual({ kind: "declined", version: "0.4.0", remembered: true });
  expect(downloadAndInstall).not.toHaveBeenCalled();
});

it("the actual startup offer stays unanswered without installing while time passes", async () => {
  vi.useFakeTimers();
  const downloadAndInstall = vi.fn().mockResolvedValue(undefined);
  check.mockResolvedValue({ version: "0.5.0", downloadAndInstall });

  const updater = await startGmm();

  expect(updater.getSnapshot()).toMatchObject({ kind: "available", version: "0.5.0" });
  // Advance five minutes of fake Date/performance time even with no queued timer.
  // tickAsync runs chained timeouts, interval ticks, animation frames and their
  // promise continuations due in this window; runOnlyPendingTimersAsync does not.
  // This is a bounded observation window, not a deadline for user consent.
  await act(async () => { await vi.advanceTimersByTimeAsync(300_000); });
  expect(downloadAndInstall).not.toHaveBeenCalled();
  for (const visibility of ["hidden", "visible"] as const) {
    const visibilityState = vi.spyOn(document, "visibilityState", "get").mockReturnValue(visibility);
    try {
      for (const target of [window, document]) {
        for (const event of ["focus", "visibilitychange"]) {
          await act(async () => {
            target.dispatchEvent(new Event(event));
            await vi.advanceTimersByTimeAsync(300_000);
          });
          expect(downloadAndInstall).not.toHaveBeenCalled();
        }
      }
    } finally {
      visibilityState.mockRestore();
    }
  }
  expect(downloadAndInstall).not.toHaveBeenCalled();
  // Still offered, never applied: only the install action the user reaches by
  // clicking in Settings moves it, and nothing in the launch reaches it.
  expect(updater.getSnapshot()).toMatchObject({ kind: "available", version: "0.5.0" });
  await act(() => updater.install());
  expect(downloadAndInstall).toHaveBeenCalledOnce();
});
