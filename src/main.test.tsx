import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import type { ReactNode } from "react";

const { check, relaunch } = vi.hoisted(() => ({ check: vi.fn(), relaunch: vi.fn() }));
vi.mock("@tauri-apps/plugin-updater", () => ({ check }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch }));
vi.mock("./App", () => ({ default: () => <p>Game Library</p> }));
vi.mock("./diagnostics", () => ({ installGlobalErrorHandlers: vi.fn() }));
vi.mock("react-dom/client", () => ({ default: { createRoot: () => ({ render: (node: ReactNode) => render(node) }) } }));

beforeEach(() => {
  vi.resetModules();
  localStorage.clear();
  check.mockReset();
  relaunch.mockReset();
});

it("the actual startup entry checks and renders consent before issuing any install", async () => {
  const downloadAndInstall = vi.fn().mockResolvedValue(undefined);
  check.mockResolvedValue({ version: "0.5.0", downloadAndInstall });
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
