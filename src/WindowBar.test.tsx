import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { WindowBar } from "./WindowBar";

it("keeps global Settings and Run setup again in the window bar with independent actions", async () => {
  const onSettings = vi.fn();
  const onSetup = vi.fn();
  const { rerender } = render(<WindowBar onSettings={onSettings} onSetup={onSetup} setupPending={false} />);
  const bar = screen.getByRole("banner", { name: "GMM" });
  expect(bar).toHaveTextContent("GMM");
  const settings = screen.getByRole("button", { name: "Settings" });
  expect(bar).toContainElement(settings);
  expect(settings).toHaveAttribute("data-settings-arrival", "global");
  await userEvent.click(settings);
  expect(onSettings).toHaveBeenCalledTimes(1);
  expect(screen.getByRole("button", { name: "Run setup again" })).toBeVisible();
  expect(screen.getByRole("button", { name: "Run setup again" }).closest("details")).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: "Run setup again" }));
  expect(onSetup).toHaveBeenCalledTimes(1);
  rerender(<WindowBar onSettings={onSettings} onSetup={onSetup} setupPending />);
  expect(screen.getByRole("button", { name: "Run setup again" })).toBeDisabled();
  expect(settings).toBeEnabled();
});
