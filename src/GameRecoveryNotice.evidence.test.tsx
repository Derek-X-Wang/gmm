import { onlineManager, type QueryClient } from "@tanstack/react-query";
import { act, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { renderWithQuery } from "./test/harness";
import { GameRecoveryNotice } from "./GameRecoveryNotice";

// This surface sits on the main screen, where the absence of a card is read as
// "this Game has no Model Importer recovery outstanding". It used to render
// nothing whenever no value had been established, so a read GMM never made
// looked exactly like a Game it had read and found healthy.
//
// Each case below is driven twice where both are reachable. A rejected command
// leaves `status: "error"`. Going offline with `onlineManager.setOnline(false)`
// leaves a read *paused*, which keeps `status: "success"`, keeps `data` and
// sets only `fetchStatus` — so `isFetching` is false exactly when the surface
// most needs to hedge, and a test that only fails a refresh never reaches that
// case. That is why the paused refresh used to render an old recovery report in
// full with its "previous report" hint hidden.

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const report = {
  reason: "Backup folder locked", gamePath: "C:/Game", backupPath: "C:/Backup",
  ownerUncertain: false, action: "retry", attempts: 1, attemptedAt: "2026-10-11",
};

const CARD = "Honkai: Star Rail Model Importer recovery";

beforeEach(() => { invoke.mockResolvedValue(null); });
afterEach(() => { onlineManager.setOnline(true); });

function renderNotice() {
  return renderWithQuery(
    <GameRecoveryNotice game="srmi" displayName="Honkai: Star Rail" onSettings={vi.fn()} />,
  );
}

/** Pause the reading by going offline and asking for it again. */
async function pause(client: QueryClient) {
  onlineManager.setOnline(false);
  await act(() => client.invalidateQueries({ queryKey: ["importer", "evacuation", "srmi"] }));
}

/** Pause the reading with no value at all: go offline and drop what was read. */
async function pauseFirstRead(client: QueryClient) {
  onlineManager.setOnline(false);
  await act(() => client.resetQueries({ queryKey: ["importer", "evacuation", "srmi"] }));
}

it("does not leave a paused first read of the recovery state silent", async () => {
  onlineManager.setOnline(false);
  renderNotice();

  const notice = await screen.findByText(/could not read whether .* Model Importer recovery needs attention/i);
  expect(notice).toHaveTextContent(/not a Game with nothing outstanding/i);
  expect(notice).toHaveTextContent(/waiting for a connection/i);
  expect(screen.queryByRole("region", { name: CARD })).not.toBeInTheDocument();
});

it("keeps a read that found nothing outstanding distinct from one it could not make", async () => {
  const { client } = renderNotice();
  await waitFor(() => expect(client.getQueryData(["importer", "evacuation", "srmi"])).toBeNull());
  // A read that landed and said nothing is outstanding still renders nothing.
  expect(screen.queryByRole("region", { name: CARD })).not.toBeInTheDocument();
  expect(screen.queryByRole("status")).not.toBeInTheDocument();

  await pauseFirstRead(client);

  const notice = await screen.findByText(/could not read whether .* Model Importer recovery needs attention/i);
  expect(notice).toHaveTextContent(/not a Game with nothing outstanding/i);
});

it("says the previous read found nothing outstanding when its refresh is paused", async () => {
  const { client } = renderNotice();
  await waitFor(() => expect(client.getQueryData(["importer", "evacuation", "srmi"])).toBeNull());

  await pause(client);

  const notice = await screen.findByText(/could not re-check whether .* Model Importer recovery needs attention/i);
  expect(notice).toHaveTextContent(/previous read found nothing outstanding/i);
  expect(notice).toHaveTextContent(/waiting for a connection/i);
});

it("marks an established recovery report as the previous one when its refresh is paused", async () => {
  invoke.mockResolvedValue(report);
  const { client } = renderNotice();
  const card = await screen.findByRole("region", { name: CARD });
  expect(card).toHaveTextContent("Backup folder locked");

  await pause(client);

  // The report GMM did read stays, with its route into Settings; what changes
  // is that it is no longer presented as the current state of the Game.
  await waitFor(() => expect(screen.getByText(/could not re-check .* Model Importer recovery/i)).toBeVisible());
  const stale = screen.getByRole("region", { name: CARD });
  expect(stale).toHaveTextContent("Backup folder locked");
  expect(stale).toHaveTextContent(/this is the previous recovery report/i);
  expect(stale).toHaveTextContent(/waiting for a connection/i);
  expect(screen.getByRole("button", { name: "Review Honkai: Star Rail recovery in Settings" })).toBeVisible();
  // Nothing is being checked while the refresh is paused, so do not say so.
  expect(screen.queryByText("Checking again; this is the previous recovery report.")).not.toBeInTheDocument();
});

it("keeps the previous-report hint while a refresh of the report is in flight", async () => {
  // This test is what guarantees the re-render behind the component's cached
  // fetch-status read: it re-renders on `fetchStatus` only because
  // `readEvidence` reads it. Without this, that coupling could break silently.
  invoke.mockResolvedValue(report);
  const { client } = renderNotice();
  await screen.findByRole("region", { name: CARD });

  let finish!: (value: unknown) => void;
  invoke.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  await act(async () => { void client.invalidateQueries({ queryKey: ["importer", "evacuation", "srmi"] }); });

  expect(await screen.findByText("Checking again; this is the previous recovery report.")).toBeVisible();

  await act(async () => finish(report));
  await waitFor(() => expect(
    screen.queryByText("Checking again; this is the previous recovery report."),
  ).not.toBeInTheDocument());
});

it("says nothing extra about recovery on an established, current read", async () => {
  invoke.mockResolvedValue(report);
  renderNotice();
  const card = await screen.findByRole("region", { name: CARD });
  expect(card).toHaveTextContent("Backup folder locked");
  expect(screen.queryByText(/could not re(ad|-check)/i)).not.toBeInTheDocument();
  expect(screen.queryByText(/previous recovery report/i)).not.toBeInTheDocument();
});
