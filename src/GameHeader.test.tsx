import { render, screen, within } from "@testing-library/react";
import { expect, it } from "vitest";
import { GameHeader } from "./GameHeader";

it("names the selected Game beside its primary Launch action", () => {
  render(<GameHeader displayName="Honkai: Star Rail" action={<button>Launch Honkai: Star Rail</button>} />);
  const header = screen.getByRole("region", { name: "Honkai: Star Rail header" });
  expect(within(header).getByRole("heading", { level: 1 })).toHaveTextContent("Honkai: Star Rail");
  expect(within(header).getByRole("button", { name: "Launch Honkai: Star Rail" })).toBeEnabled();
});
