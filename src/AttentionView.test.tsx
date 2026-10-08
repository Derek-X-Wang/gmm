import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import { renderWithQuery } from "./test/harness";
import type { AttentionReport } from "./api";

const { attentionStatus } = vi.hoisted(() => ({ attentionStatus: vi.fn() }));
vi.mock("./api", () => ({ attentionStatus }));
const { AttentionView } = await import("./AttentionView");

const clean: AttentionReport = {
  safeToProceed: true,
  reinstalls: [], enabledTransitions: [], importerEvacuations: [],
  stagedLibraryOperations: [], sessionLaunches: [], activeSession: null,
  libraryAudits: [], libraryRootOverlaps: [], modPathOverlaps: [],
};

beforeEach(() => attentionStatus.mockReset());

it("names a staged Library operation, its path, and its recovery obstruction", async () => {
  attentionStatus.mockResolvedValue({
    ...clean,
    safeToProceed: false,
    stagedLibraryOperations: [{
      id: "01STAGED", game: "srmi", operation: "import_zip",
      stagedPath: "C:\\GMM\\library\\srmi\\01STAGED",
      recoveryError: "Witness retirement failed: database is locked",
    }],
  });
  renderWithQuery(<AttentionView />);

  await waitFor(() => expect(
    screen.queryByText("Staged Library operations (1)"),
    "the aggregate must expose staged Library ownership with no existing frontend surface",
  ).toBeInTheDocument());
  const region = screen.getByRole("region", { name: "Attention across all Games" });
  expect(region).toHaveTextContent("SRMI");
  expect(region).toHaveTextContent("import_zip");
  expect(region).toHaveTextContent("C:\\GMM\\library\\srmi\\01STAGED");
  expect(region).toHaveTextContent("Witness retirement failed: database is locked");
  expect(screen.queryByText("Nothing needs attention in this report.")).not.toBeInTheDocument();
});

it("checks a clean report once and refreshes only when requested", async () => {
  attentionStatus.mockResolvedValue(clean);
  const { rerender, client } = renderWithQuery(<AttentionView />);
  await screen.findByText("Nothing needs attention in this report.");
  rerender(<AttentionView />);
  window.dispatchEvent(new Event("focus"));
  window.dispatchEvent(new Event("online"));
  expect(attentionStatus).toHaveBeenCalledTimes(1);
  expect(client.getQueryState(["attentionStatus"])?.status).toBe("success");
  await userEvent.click(screen.getByRole("button", { name: "Refresh attention" }));
  await waitFor(() => expect(attentionStatus).toHaveBeenCalledTimes(2));
});

it("shows all ten outstanding surfaces together across Games", async () => {
  const report: AttentionReport = {
    ...clean, safeToProceed: false,
    reinstalls: [{ modId: "01MOD", game: "gimi", libraryPath: "C:\\Library\\mod", recovery: null }],
    enabledTransitions: [{ modId: "01MOD", game: "srmi", intendedEnabled: false, junctionPath: "C:\\Game\\Mods\\mod", recovery: null }],
    importerEvacuations: [{ game: "zzmi", gamePath: "C:\\Game", backupPath: "C:\\Backup", recovery: null }],
    stagedLibraryOperations: [{ id: "01STAGED", game: "wwmi", operation: "adopt", stagedPath: "C:\\Library\\staged", recoveryError: null }],
    sessionLaunches: [{ id: "01LAUNCH", game: "himi", childPid: null, startedAt: "2026-10-07T12:00:00Z" }],
    activeSession: { game: "efmi", pid: 42, startedAt: "2026-10-07T12:01:00Z" },
    libraryAudits: [{
      game: "gimi", totalBytes: 1,
      unreferenced: [{ directoryName: "orphan", path: "C:\\Library\\orphan", sizeBytes: 1 }],
      duplicates: [{ path: "C:\\Library\\duplicate", mods: [] }],
    }],
    libraryRootOverlaps: [{ game: null, path: "C:\\Backup\\Library", backups: "C:\\Backup" }],
    modPathOverlaps: [{ game: "srmi", modId: "01STRANDED", modName: "Stranded Mod", path: "C:\\Backup\\mod", backups: "C:\\Backup" }],
  };
  attentionStatus.mockResolvedValue(report);
  renderWithQuery(<AttentionView />);
  await screen.findByRole("heading", { name: "Staged Library operations (1)" });
  for (const title of [
    "Mod reinstalls", "Enable or disable transitions", "Model Importer recovery",
    "Staged Library operations", "Interrupted Game launches", "Active Game Session",
    "Unreferenced Library folders", "Duplicate Mod records", "Library root overlaps", "Mod path overlaps",
  ]) {
    expect(screen.getByRole("heading", { name: `${title} (1)` })).toBeInTheDocument();
  }
  expect(screen.getByRole("region", { name: "Attention across all Games" })).toHaveTextContent("Stranded Mod");
  expect(screen.queryByText("Nothing needs attention in this report.")).not.toBeInTheDocument();
});

it("shows an unavailable sub-report as an error even after a clean check", async () => {
  attentionStatus.mockResolvedValueOnce(clean).mockRejectedValueOnce({
    kind: "other", message: "Library audit unavailable: access denied",
  });
  renderWithQuery(<AttentionView />);
  await screen.findByText("Nothing needs attention in this report.");
  await userEvent.click(screen.getByRole("button", { name: "Refresh attention" }));
  const alert = await screen.findByRole("alert");
  expect(alert).toHaveTextContent("Could not check attention");
  expect(alert).toHaveTextContent("Library audit unavailable: access denied");
  expect(screen.queryByText("Nothing needs attention in this report.")).not.toBeInTheDocument();
  expect(attentionStatus).toHaveBeenCalledTimes(2);
});
