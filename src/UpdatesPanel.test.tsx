import { act, fireEvent, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { renderWithQuery } from "./test/harness";
import { StartupUpdateNotice, UpdatesPanel } from "./UpdatesPanel";
import { createUpdateController as createController } from "./updater";
import { memorySkipStorage, signedMetadata } from "./test/updaterStorage";
let storage = memorySkipStorage();
const createUpdateController = (check: Parameters<typeof createController>[0]) => createController(check, storage);

const { relaunch } = vi.hoisted(() => ({ relaunch: vi.fn() }));
vi.mock("@tauri-apps/plugin-updater", () => ({ check: vi.fn() }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch }));
beforeEach(() => { storage = memorySkipStorage(); relaunch.mockReset().mockResolvedValue(undefined); });

it("checks on demand and distinguishes idle, checking and current", async () => {
  let resolve!: (value: null) => void;
  const check = vi.fn(() => new Promise<null>((yes) => { resolve = yes; }));
  const controller = createUpdateController(check);
  renderWithQuery(<UpdatesPanel controller={controller} />);
  expect(screen.getByText(/Signatures are verified/)).toBeVisible();
  expect(check).not.toHaveBeenCalled();
  expect(screen.queryByText(/latest release/)).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Check for updates" }));
  expect(screen.getByRole("button", { name: "Checking…" })).toBeDisabled();
  await act(async () => resolve(null));
  expect(await screen.findByText("You're on the latest release.")).toBeVisible();
  expect(screen.queryByRole("button", { name: "Restart to apply" })).not.toBeInTheDocument();
});

it.each(["settings", "startup"])("%s requires consent before the install command, then explicit restart", async (entry) => {
  let progress!: (event: unknown) => void;
  let finish!: () => void;
  const downloadAndInstall = vi.fn((callback: typeof progress) => {
    progress = callback; return new Promise<void>((yes) => { finish = yes; });
  });
  const controller = createUpdateController(vi.fn().mockResolvedValue({ version: "0.5.0", rawJson: signedMetadata("0.5.0"), downloadAndInstall }));
  if (entry === "settings") {
    renderWithQuery(<UpdatesPanel controller={controller} />);
    await userEvent.click(screen.getByRole("button", { name: "Check for updates" }));
  } else {
    renderWithQuery(<StartupUpdateNotice controller={controller} />);
    await act(() => controller.checkForUpdates(true));
  }
  expect(downloadAndInstall).not.toHaveBeenCalled();
  expect(await screen.findByText(/is available/)).toHaveTextContent("0.5.0");
  expect(screen.queryByRole("button", { name: "Restart to apply" })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Install update" }));
  expect(downloadAndInstall).toHaveBeenCalledOnce();
  expect(screen.getByText("Downloading…")).toBeVisible();
  await act(() => progress({ event: "Started", data: { contentLength: 200 } }));
  await act(() => progress({ event: "Progress", data: { chunkLength: 50 } }));
  expect(screen.getByText("Downloading (25%)")).toBeVisible();
  await act(() => progress({ event: "Finished" }));
  expect(screen.queryByRole("button", { name: "Restart to apply" })).not.toBeInTheDocument();
  expect(relaunch).not.toHaveBeenCalled();
  await act(async () => finish());
  expect(screen.getByText(/Update installed/)).toBeVisible();
  expect(relaunch).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole("button", { name: "Restart to apply" }));
  expect(relaunch).toHaveBeenCalledOnce();
});

it("skipping in the startup prompt survives a fresh launch and a new version still prompts", async () => {
  const downloadAndInstall = vi.fn().mockResolvedValue(undefined);
  const check = vi.fn().mockResolvedValue({ version: "0.5.0", rawJson: signedMetadata("0.5.0"), downloadAndInstall });
  const controller = createUpdateController(check);
  const view = renderWithQuery(<StartupUpdateNotice controller={controller} />);
  await act(() => controller.checkForUpdates(true));
  expect(downloadAndInstall).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole("button", { name: "Skip this build" }));
  expect(downloadAndInstall).not.toHaveBeenCalled();
  expect(relaunch).not.toHaveBeenCalled();
  expect(screen.queryByRole("button", { name: "Install update" })).not.toBeInTheDocument();
  expect(screen.getByText(/Version 0.5.0 was skipped/)).toBeVisible();
  await userEvent.click(screen.getByRole("button", { name: "Close update notice" }));
  expect(screen.queryByLabelText("GMM update")).not.toBeInTheDocument();
  view.unmount();
  const nextLaunch = createUpdateController(check);
  const nextView = renderWithQuery(<StartupUpdateNotice controller={nextLaunch} />);
  await act(() => nextLaunch.checkForUpdates(true));
  expect(screen.queryByLabelText("GMM update")).not.toBeInTheDocument();
  expect(downloadAndInstall).not.toHaveBeenCalled();
  nextView.unmount();
  check.mockResolvedValue({ version: "0.6.0", rawJson: signedMetadata("0.6.0"), downloadAndInstall });
  const laterLaunch = createUpdateController(check);
  renderWithQuery(<StartupUpdateNotice controller={laterLaunch} />);
  await act(() => laterLaunch.checkForUpdates(true));
  expect(screen.getByText(/is available/)).toHaveTextContent("0.6.0");
  expect(downloadAndInstall).not.toHaveBeenCalled();
});

it("Settings keeps the declined outcome visible and allows deliberate reconsideration", async () => {
  const downloadAndInstall = vi.fn();
  const controller = createUpdateController(vi.fn().mockResolvedValue({ version: "0.5.0", rawJson: signedMetadata("0.5.0"), downloadAndInstall }));
  renderWithQuery(<UpdatesPanel controller={controller} />);
  await userEvent.click(screen.getByRole("button", { name: "Check for updates" }));
  await userEvent.click(screen.getByRole("button", { name: "Skip this build" }));
  expect(screen.getByText(/Version 0.5.0 was skipped/)).toBeVisible();
  await userEvent.click(screen.getByRole("button", { name: "Check for updates" }));
  expect(screen.getByRole("button", { name: "Install update" })).toBeVisible();
  expect(downloadAndInstall).not.toHaveBeenCalled();
});

it.each(["settings", "startup"])("%s shows check failure separately from up to date and can retry", async (entry) => {
  const check = vi.fn().mockRejectedValue(new Error("network unreachable"));
  const controller = createUpdateController(check);
  renderWithQuery(entry === "startup" ? <StartupUpdateNotice controller={controller} /> : <UpdatesPanel controller={controller} />);
  await act(() => controller.checkForUpdates(entry === "startup"));
  expect(screen.getByRole("alert")).toHaveTextContent("Update check failed: network unreachable");
  expect(screen.queryByText(/latest release/)).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Restart to apply" })).not.toBeInTheDocument();
  check.mockResolvedValue(null);
  await userEvent.click(screen.getByRole("button", { name: entry === "startup" ? "Retry update check" : "Check for updates" }));
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  if (entry === "settings") expect(screen.getByText("You're on the latest release.")).toBeVisible();
});

it("shows download failure without a current-release claim or restart", async () => {
  const downloadAndInstall = vi.fn().mockRejectedValue(new Error("signature mismatch"));
  const controller = createUpdateController(vi.fn().mockResolvedValue({ version: "0.5.0", rawJson: signedMetadata("0.5.0"), downloadAndInstall }));
  renderWithQuery(<StartupUpdateNotice controller={controller} />);
  await act(() => controller.checkForUpdates(true));
  expect(downloadAndInstall).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole("button", { name: "Install update" }));
  expect(screen.getByRole("alert")).toHaveTextContent("Update installation failed: signature mismatch");
  expect(screen.queryByText(/latest release/)).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Restart to apply" })).not.toBeInTheDocument();
  expect(relaunch).not.toHaveBeenCalled();
});

it("a failed restart keeps the explicit restart action without offering another install", async () => {
  const downloadAndInstall = vi.fn().mockResolvedValue(undefined);
  const controller = createUpdateController(vi.fn().mockResolvedValue({ version: "0.5.0", rawJson: signedMetadata("0.5.0"), downloadAndInstall }));
  renderWithQuery(<StartupUpdateNotice controller={controller} />);
  await act(() => controller.checkForUpdates(true));
  await userEvent.click(screen.getByRole("button", { name: "Install update" }));
  relaunch.mockRejectedValueOnce(new Error("process unavailable"));
  await userEvent.click(screen.getByRole("button", { name: "Restart to apply" }));
  expect(screen.getByRole("alert")).toHaveTextContent("Restart failed: process unavailable");
  expect(screen.queryByRole("button", { name: "Install update" })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Restart to apply" }));
  expect(relaunch).toHaveBeenCalledTimes(2);
  expect(downloadAndInstall).toHaveBeenCalledOnce();
});

afterEach(() => vi.useRealTimers());
it("Settings keeps an unanswered offer without installing while time passes", async () => {
  vi.useFakeTimers();
  const downloadAndInstall = vi.fn().mockResolvedValue(undefined);
  const controller = createUpdateController(vi.fn().mockResolvedValue({ version: "0.5.0", downloadAndInstall }));
  renderWithQuery(<UpdatesPanel controller={controller} />);
  fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
  await act(async () => {});
  expect(screen.getByRole("button", { name: "Install update" })).toBeVisible();
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
  expect(screen.getByRole("button", { name: "Install update" })).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: "Install update" }));
  await act(async () => {});
  expect(downloadAndInstall).toHaveBeenCalledOnce();
});

it("an offline startup error can be closed without retrying or losing diagnostics", async () => {
  const check = vi.fn().mockRejectedValue(new Error("offline"));
  const controller = createUpdateController(check);
  renderWithQuery(<StartupUpdateNotice controller={controller} />);
  await act(() => controller.checkForUpdates(true));
  expect(screen.getByRole("alert")).toHaveTextContent("offline");
  await userEvent.click(screen.getByRole("button", { name: "Close update notice" }));
  expect(screen.queryByLabelText("GMM update")).not.toBeInTheDocument();
  expect(check).toHaveBeenCalledOnce();
  expect(controller.getLastError()).toBe("Update check failed: offline");
});

it.each(["settings", "startup"])("%s explains that an unidentifiable skipped build will be offered again", async (entry) => {
  const downloadAndInstall = vi.fn();
  const controller = createUpdateController(vi.fn().mockResolvedValue({ version: "0.5.0", downloadAndInstall }));
  renderWithQuery(entry === "startup" ? <StartupUpdateNotice controller={controller} /> : <UpdatesPanel controller={controller} />);
  await act(() => controller.checkForUpdates(entry === "startup"));
  await userEvent.click(screen.getByRole("button", { name: "Skip this build" }));
  expect(screen.getByText(/This build could not be identified/)).toHaveTextContent("It will be offered again on launch.");
  expect(controller.getSnapshot()).toEqual({ kind: "declined", version: "0.5.0", remembered: false });
  expect(storage.getItem("updater.skippedBuild")).toBeNull();
  expect(downloadAndInstall).not.toHaveBeenCalled();
});
