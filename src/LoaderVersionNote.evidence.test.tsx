import { onlineManager } from "@tanstack/react-query";
import { act, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { makeQueryClient, renderWithQuery } from "./test/harness";

const checkLoaderUpdate = vi.fn();

vi.mock("./api", () => ({
  checkLoaderUpdate: (...args: unknown[]) => checkLoaderUpdate(...args),
}));

const { LoaderVersionNote } = await import("./LoaderVersionNote");

const current = { shippedVersion: "v1.0.2", latestVersion: "v1.0.2", upstreamAhead: false, checkError: null };

// Braces matter: a returned mock would be run by vitest as the hook's cleanup.
beforeEach(() => {
  checkLoaderUpdate.mockReset();
});
afterEach(() => {
  onlineManager.setOnline(true);
});

it("does not claim the Loader is up to date when the command never succeeded", async () => {
  checkLoaderUpdate.mockRejectedValue("loader check command failed");

  renderWithQuery(<LoaderVersionNote />);

  const note = await screen.findByRole("status");
  expect(note).toHaveTextContent(/could not check the Loader version/i);
  expect(note).toHaveTextContent("loader check command failed");
  expect(note).not.toHaveTextContent(/up to date/i);
});

it("withdraws 'up to date' when a later refresh of the check fails", async () => {
  checkLoaderUpdate.mockResolvedValueOnce(current);
  const { client } = renderWithQuery(<LoaderVersionNote />);
  expect(await screen.findByRole("status")).toHaveTextContent(/up to date/i);

  checkLoaderUpdate.mockRejectedValue("refresh failed");
  await act(() => client.invalidateQueries({ queryKey: ["loader", "version"] }));

  await waitFor(() => expect(screen.getByRole("status")).not.toHaveTextContent(/up to date/i));
  expect(screen.getByRole("status")).toHaveTextContent(/could not confirm/i);
  expect(screen.getByRole("status")).toHaveTextContent("v1.0.2");
});

it("withdraws 'up to date' when the refresh is paused, which leaves status success and isFetching false", async () => {
  checkLoaderUpdate.mockResolvedValue(current);
  const client = makeQueryClient();
  renderWithQuery(<LoaderVersionNote />, { client });
  expect(await screen.findByRole("status")).toHaveTextContent(/up to date/i);

  onlineManager.setOnline(false);
  await act(() => client.invalidateQueries({ queryKey: ["loader", "version"] }));

  await waitFor(() => expect(screen.getByRole("status")).not.toHaveTextContent(/up to date/i));
  expect(screen.getByRole("status")).toHaveTextContent(/could not confirm/i);
});

it("says nothing while the first read is still in flight", () => {
  checkLoaderUpdate.mockReturnValue(new Promise(() => {}));

  renderWithQuery(<LoaderVersionNote />);

  expect(screen.queryByRole("status")).not.toBeInTheDocument();
});
