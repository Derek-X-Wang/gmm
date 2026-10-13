import { act, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import { renderWithQuery } from "./test/harness";
import { StartupUpdateNotice, UpdatesPanel } from "./UpdatesPanel";
import { createUpdateController } from "./updater";

const { relaunch } = vi.hoisted(() => ({ relaunch: vi.fn() }));
vi.mock("@tauri-apps/plugin-updater", () => ({ check: vi.fn() }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch }));
beforeEach(() => { localStorage.clear(); relaunch.mockReset().mockResolvedValue(undefined); });

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
  const controller = createUpdateController(vi.fn().mockResolvedValue({ version: "0.5.0", downloadAndInstall }));
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
  const check = vi.fn().mockResolvedValue({ version: "0.5.0", downloadAndInstall });
  const controller = createUpdateController(check);
  const view = renderWithQuery(<StartupUpdateNotice controller={controller} />);
  await act(() => controller.checkForUpdates(true));
  expect(downloadAndInstall).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole("button", { name: "Skip this version" }));
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
  check.mockResolvedValue({ version: "0.6.0", downloadAndInstall });
  const laterLaunch = createUpdateController(check);
  renderWithQuery(<StartupUpdateNotice controller={laterLaunch} />);
  await act(() => laterLaunch.checkForUpdates(true));
  expect(screen.getByText(/is available/)).toHaveTextContent("0.6.0");
  expect(downloadAndInstall).not.toHaveBeenCalled();
});

it("Settings keeps the declined outcome visible and allows deliberate reconsideration", async () => {
  const downloadAndInstall = vi.fn();
  const controller = createUpdateController(vi.fn().mockResolvedValue({ version: "0.5.0", downloadAndInstall }));
  renderWithQuery(<UpdatesPanel controller={controller} />);
  await userEvent.click(screen.getByRole("button", { name: "Check for updates" }));
  await userEvent.click(screen.getByRole("button", { name: "Skip this version" }));
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
  const controller = createUpdateController(vi.fn().mockResolvedValue({ version: "0.5.0", downloadAndInstall }));
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
  const controller = createUpdateController(vi.fn().mockResolvedValue({ version: "0.5.0", downloadAndInstall }));
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
