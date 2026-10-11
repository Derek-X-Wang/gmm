import { act, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import { renderWithQuery } from "./test/harness";
import { GameRecoveryNotice } from "./GameRecoveryNotice";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
beforeEach(() => { invoke.mockResolvedValue(null); });

it("keeps a healthy Game free of a recovery card", async () => {
  const { client } = renderWithQuery(<GameRecoveryNotice game="srmi" displayName="Honkai: Star Rail" onSettings={vi.fn()} />);
  await act(async () => { await client.refetchQueries({ queryKey: ["importer", "evacuation", "srmi"] }); });
  expect(screen.queryByRole("region", { name: "Honkai: Star Rail Model Importer recovery" })).not.toBeInTheDocument();
});

it("names the Game and recovery reason and routes its only action to Settings", async () => {
  invoke.mockResolvedValue({ reason: "Backup folder locked", gamePath: "C:/Game", backupPath: "C:/Backup", ownerUncertain: false, action: "retry", attempts: 1, attemptedAt: "2026-10-11" });
  const onSettings = vi.fn();
  renderWithQuery(<GameRecoveryNotice game="srmi" displayName="Honkai: Star Rail" onSettings={onSettings} />);
  const notice = await screen.findByRole("region", { name: "Honkai: Star Rail Model Importer recovery" });
  expect(notice).toHaveTextContent("Backup folder locked");
  expect(screen.getAllByRole("button")).toHaveLength(1);
  await userEvent.click(screen.getByRole("button", { name: "Review Honkai: Star Rail recovery in Settings" }));
  expect(onSettings).toHaveBeenCalledTimes(1);
  expect(invoke.mock.calls.map(([command]) => command)).toEqual(["get_importer_evacuation_recovery"]);
});

it("keeps an unavailable recovery check distinct from a healthy result", async () => {
  invoke.mockRejectedValue(new Error("Recovery database unreadable"));
  renderWithQuery(<GameRecoveryNotice game="srmi" displayName="Honkai: Star Rail" onSettings={vi.fn()} />);
  const error = await screen.findByRole("alert");
  expect(error).toHaveTextContent("Could not check Honkai: Star Rail Model Importer recovery");
  expect(error).toHaveTextContent("Recovery database unreadable");
});
