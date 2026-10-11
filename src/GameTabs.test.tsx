import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { GameTabs } from "./GameTabs";

it("keeps checking, failed inventory and confirmed empty distinct with Add game beside known tabs", async () => {
  const game = { code: "srmi" as const, displayName: "Honkai: Star Rail" };
  const onAddGame = vi.fn();
  const onChange = vi.fn();
  const props = { games: [], remaining: [game], pending: true, pathsUnavailable: false, active: "srmi" as const, onAddGame, onChange };
  const { rerender } = render(<GameTabs {...props} />);
  expect(screen.getByText("Checking game install paths…")).toBeVisible();
  expect(screen.queryByText(/No game install paths set/)).not.toBeInTheDocument();
  rerender(<GameTabs {...props} pending={false} pathsUnavailable />);
  expect(screen.queryByText(/No game install paths set/)).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Add game" })).toBeEnabled();
  rerender(<GameTabs {...props} pending={false} />);
  expect(screen.getByText("No game install paths set. Add a game to get started.")).toBeVisible();
  await userEvent.click(screen.getByRole("button", { name: "Add game" }));
  await userEvent.click(screen.getByRole("button", { name: "Set up Honkai: Star Rail" }));
  expect(onAddGame).toHaveBeenCalledWith("srmi");
  expect(onChange).not.toHaveBeenCalled();
  rerender(<GameTabs {...props} games={[game]} remaining={[]} pending={false} />);
  await userEvent.click(screen.getByRole("tab", { name: "Honkai: Star Rail" }));
  expect(onChange).toHaveBeenCalledWith("srmi");
  expect(screen.queryByRole("button", { name: "Add game" })).not.toBeInTheDocument();
});
