import { render, screen, within } from "@testing-library/react";
import { expect, it } from "vitest";
import { GlobalHeaderLine } from "./GlobalHeaderLine";

it("keeps attention and cross-Game session context together", () => {
  render(<GlobalHeaderLine><button>Refresh attention</button><p role="status">WWMI is running</p></GlobalHeaderLine>);
  const global = screen.getByRole("region", { name: "Global status" });
  expect(within(global).getByRole("button", { name: "Refresh attention" })).toBeEnabled();
  expect(within(global).getByRole("status")).toHaveTextContent("WWMI is running");
});
