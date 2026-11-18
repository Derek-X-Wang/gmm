import { act, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";

import { renderWithQuery } from "./test/harness";

const currentSession = vi.fn();
vi.mock("./api", () => ({ currentSession: (...args: unknown[]) => currentSession(...args) }));

const { SessionCheckNotice, useSessionCheck } = await import("./SessionCheckNotice");

function Probe() {
  return <SessionCheckNotice check={useSessionCheck()} />;
}

beforeEach(() => {
  currentSession.mockReset();
});

// These strings are the contract the shared reading rule was generalised from.
it("renders the exact checking string while the first read is in flight", async () => {
  currentSession.mockReturnValue(new Promise(() => {}));
  renderWithQuery(<Probe />);
  expect((await screen.findByRole("status")).textContent)
    .toBe("Game Session unverified — checking whether a Game is running.");
});

it("renders the exact failure string, with the failure, when the session was never read", async () => {
  currentSession.mockRejectedValue(new Error("session store locked"));
  renderWithQuery(<Probe />);
  await waitFor(() => expect(screen.getByRole("status").textContent)
    .toBe("Cannot verify Game Session — session check failed. session store locked"));
});

it("renders the exact refresh-failed string, with the failure, when a read was established first", async () => {
  currentSession.mockResolvedValueOnce(null);
  const { client } = renderWithQuery(<Probe />);
  await waitFor(() => expect(currentSession).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument());

  currentSession.mockRejectedValue(new Error("session store locked"));
  await act(() => client.invalidateQueries({ queryKey: ["session"] }));
  await waitFor(() => expect(screen.getByRole("status").textContent)
    .toBe("Cannot verify Game Session — refresh failed; the previous result is unverified. session store locked"));
});

it("stays silent on an established read", async () => {
  currentSession.mockResolvedValue(null);
  renderWithQuery(<Probe />);
  await waitFor(() => expect(currentSession).toHaveBeenCalled());
  await waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument());
});
