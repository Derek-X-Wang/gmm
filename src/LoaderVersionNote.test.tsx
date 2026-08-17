import { screen } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";

import { renderWithQuery } from "./test/harness";

const checkLoaderUpdate = vi.fn();

vi.mock("./api", () => ({
  checkLoaderUpdate: (...args: unknown[]) => checkLoaderUpdate(...args),
}));

const { LoaderVersionNote } = await import("./LoaderVersionNote");

beforeEach(() => {
  checkLoaderUpdate.mockReset();
});

it("states which Loader GMM ships and what upstream has", async () => {
  checkLoaderUpdate.mockResolvedValue({
    shippedVersion: "v0.8.8",
    latestVersion: "v1.0.2",
    upstreamAhead: true,
    checkError: null,
  });

  renderWithQuery(<LoaderVersionNote />);

  const note = await screen.findByRole("status");
  expect(note).toHaveTextContent("v0.8.8");
  expect(note).toHaveTextContent("v1.0.2");
});

it("never offers an install, because the Loader ships inside GMM", async () => {
  checkLoaderUpdate.mockResolvedValue({
    shippedVersion: "v0.8.8",
    latestVersion: "v1.0.2",
    upstreamAhead: true,
    checkError: null,
  });

  renderWithQuery(<LoaderVersionNote />);

  await screen.findByRole("status");
  expect(screen.queryByRole("button")).not.toBeInTheDocument();
  // The pre-#78 copy told users to "re-run the importer install to
  // pull the new Loader package". That does nothing for the Loader.
  expect(screen.queryByText(/re-run the importer install/i)).not.toBeInTheDocument();
});

it("says it is current when upstream matches", async () => {
  checkLoaderUpdate.mockResolvedValue({
    shippedVersion: "v1.0.2",
    latestVersion: "v1.0.2",
    upstreamAhead: false,
    checkError: null,
  });

  renderWithQuery(<LoaderVersionNote />);

  const note = await screen.findByRole("status");
  expect(note).toHaveTextContent(/up to date/i);
});

it("reports a failed check instead of implying everything is fine", async () => {
  checkLoaderUpdate.mockResolvedValue({
    shippedVersion: "v0.8.8",
    latestVersion: null,
    upstreamAhead: false,
    checkError: "GitHub returned 503",
  });

  renderWithQuery(<LoaderVersionNote />);

  const note = await screen.findByRole("status");
  expect(note).toHaveTextContent("GitHub returned 503");
  expect(note).not.toHaveTextContent(/up to date/i);
  // We still know what we ship even when the check fails.
  expect(note).toHaveTextContent("v0.8.8");
});
