import { act, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import { renderWithQuery } from "./test/harness";
import { Diagnostics } from "./Diagnostics.tsx";

const { invoke, save } = vi.hoisted(() => ({ invoke: vi.fn(), save: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ save }));
beforeEach(() => {
  invoke.mockReset().mockImplementation(async (command) => command === "diagnostics_log_dir" ? "C:/GMM/logs" : undefined);
  save.mockReset().mockResolvedValue("C:/Exports/report.zip");
});

it("distinguishes directory loading and available and keeps the path read-only", async () => {
  let resolve!: (value: string) => void;
  invoke.mockImplementation(() => new Promise((yes) => { resolve = yes; }));
  renderWithQuery(<Diagnostics />);
  expect(screen.getByText("Reading log directory…")).toBeVisible();
  expect(screen.getByRole("button", { name: "Export diagnostics bundle" })).toBeDisabled();
  await act(async () => resolve("C:/GMM/logs"));
  expect(await screen.findByDisplayValue("C:/GMM/logs")).toHaveAttribute("readonly");
  expect(save).not.toHaveBeenCalled();
  expect(invoke.mock.calls.map(([command]) => command)).toEqual(["diagnostics_log_dir"]);
});
it("reports directory failure instead of an empty available path", async () => {
  invoke.mockRejectedValue(new Error("Log directory denied"));
  renderWithQuery(<Diagnostics />);
  expect(await screen.findByText("Log directory denied")).toBeVisible();
  expect(screen.getByText("Log directory unavailable.")).toBeVisible();
  expect(screen.getByRole("button", { name: "Export diagnostics bundle" })).toBeDisabled();
});
it("cancels the local dialog without exporting or reporting an artifact", async () => {
  save.mockResolvedValue(null);
  renderWithQuery(<Diagnostics />);
  await screen.findByDisplayValue("C:/GMM/logs");
  await userEvent.click(screen.getByRole("button", { name: "Export diagnostics bundle" }));
  expect(await screen.findByText("Export cancelled.")).toBeVisible();
  expect(invoke).not.toHaveBeenCalledWith("export_diagnostics_bundle", expect.anything());
  expect(screen.queryByText(/Saved bundle to/)).not.toBeInTheDocument();
});
it("exports only to the chosen local artifact and shows pending and completion", async () => {
  let finish!: () => void;
  invoke.mockImplementation(async (command) => command === "diagnostics_log_dir" ? "C:/GMM/logs" : new Promise<void>((yes) => { finish = yes; }));
  renderWithQuery(<Diagnostics />);
  await screen.findByDisplayValue("C:/GMM/logs");
  await userEvent.click(screen.getByRole("button", { name: "Export diagnostics bundle" }));
  expect(screen.getByRole("button", { name: "Bundling…" })).toBeDisabled();
  expect(save).toHaveBeenCalledWith({ defaultPath: "gmm-diagnostics.zip", filters: [{ name: "ZIP archive", extensions: ["zip"] }] });
  expect(invoke).toHaveBeenCalledWith("export_diagnostics_bundle", { logDir: "C:/GMM/logs", destPath: "C:/Exports/report.zip" });
  await act(async () => finish());
  expect(await screen.findByText(/Saved bundle to/)).toHaveTextContent("C:/Exports/report.zip");
});
it.each(["dialog", "export"])("shows %s failure without claiming a saved bundle", async (stage) => {
  if (stage === "dialog") save.mockRejectedValue(new Error("Local export refused"));
  else invoke.mockImplementation(async (command) => { if (command === "diagnostics_log_dir") return "C:/GMM/logs"; throw new Error("Local export refused"); });
  renderWithQuery(<Diagnostics />);
  await screen.findByDisplayValue("C:/GMM/logs");
  await userEvent.click(screen.getByRole("button", { name: "Export diagnostics bundle" }));
  expect(await screen.findByText("Local export refused")).toBeVisible();
  expect(screen.queryByText(/Saved bundle to/)).not.toBeInTheDocument();
});
