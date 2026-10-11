import { act, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import { renderWithQuery } from "./test/harness";
import { UpdatesPanel } from "./UpdatesPanel";

const { check, relaunch } = vi.hoisted(() => ({ check: vi.fn(), relaunch: vi.fn() }));
vi.mock("@tauri-apps/plugin-updater", () => ({ check }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch }));
beforeEach(() => { check.mockReset(); relaunch.mockReset().mockResolvedValue(undefined); });

it("checks the signed updater on demand and distinguishes idle, checking and current", async () => {
  let resolve!: (value: null) => void;
  check.mockImplementation(() => new Promise((yes) => { resolve = yes; }));
  renderWithQuery(<UpdatesPanel />);
  expect(screen.getByText(/Signatures are verified/)).toBeVisible();
  expect(check).not.toHaveBeenCalled();
  expect(screen.queryByText(/latest release/)).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Check for updates" }));
  expect(screen.getByRole("button", { name: "Checking…" })).toBeDisabled();
  await act(async () => resolve(null));
  expect(await screen.findByText("You're on the latest release.")).toBeVisible();
  expect(screen.queryByRole("button", { name: "Restart to apply" })).not.toBeInTheDocument();
});

it("shows available, known and unknown download progress, installed and explicit restart", async () => {
  let progress!: (event: unknown) => void;
  let finish!: () => void;
  check.mockResolvedValue({ version: "0.5.0", downloadAndInstall: (callback: typeof progress) => {
    progress = callback; return new Promise<void>((yes) => { finish = yes; });
  } });
  renderWithQuery(<UpdatesPanel />);
  await userEvent.click(screen.getByRole("button", { name: "Check for updates" }));
  expect(await screen.findByText(/is available/)).toHaveTextContent("0.5.0");
  await act(() => progress({ event: "Started", data: {} }));
  expect(screen.getByText("Downloading…")).toBeVisible();
  expect(screen.getByRole("button", { name: "Check for updates" })).toBeDisabled();
  await act(() => progress({ event: "Started", data: { contentLength: 200 } }));
  await act(() => progress({ event: "Progress", data: { chunkLength: 50 } }));
  expect(screen.getByText("Downloading (25%)")).toBeVisible();
  expect(relaunch).not.toHaveBeenCalled();
  await act(async () => { progress({ event: "Finished" }); finish(); });
  expect(screen.getByText(/Update downloaded/)).toBeVisible();
  await userEvent.click(screen.getByRole("button", { name: "Restart to apply" }));
  expect(relaunch).toHaveBeenCalledOnce();
});

it.each(["check", "download"])("shows %s failure without a latest-release claim or restart", async (stage) => {
  if (stage === "check") check.mockRejectedValue(new Error("Signature check unavailable"));
  else check.mockResolvedValue({ version: "0.5.0", downloadAndInstall: vi.fn().mockRejectedValue(new Error("Signature check unavailable")) });
  renderWithQuery(<UpdatesPanel />);
  await userEvent.click(screen.getByRole("button", { name: "Check for updates" }));
  expect(await screen.findByText("Signature check unavailable")).toBeVisible();
  expect(screen.queryByText(/latest release/)).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Restart to apply" })).not.toBeInTheDocument();
});
