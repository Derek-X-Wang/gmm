// @ts-expect-error Node types are not part of the frontend; Vitest provides this test-only runtime.
import { readFileSync } from "node:fs";
import { focusManager, onlineManager } from "@tanstack/react-query";
import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import { makeQueryClient, renderWithQuery } from "./test/harness";
import type { AttentionReport } from "./api";

const { attentionStatus, invoke } = vi.hoisted(() => ({ attentionStatus: vi.fn(), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("./api", async (original) => ({ ...await original<typeof import("./api")>(), attentionStatus }));
const { AttentionView } = await import("./AttentionView");

const clean: AttentionReport = {
  safeToProceed: true,
  reinstalls: [], enabledTransitions: [], importerEvacuations: [],
  stagedLibraryOperations: [], sessionLaunches: [], activeSession: null,
  libraryAudits: [], libraryRootOverlaps: [], modPathOverlaps: [],
};

beforeEach(() => { attentionStatus.mockReset(); invoke.mockReset(); });

async function expand() {
  const button = screen.getByRole("button", { expanded: false });
  await userEvent.click(button);
  return screen.getByRole("region", { name: "All-Games attention report" });
}

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

  // Budget the initial report fetch, then fail immediately if its settled label is wrong.
  await waitFor(() => expect(screen.getByRole("region", { name: "Attention across all Games" }))
    .toHaveAttribute("aria-busy", "false"), { timeout: 10_000 });
  expect(screen.getByRole("button", { name: /Needs attention/ })).toBeInTheDocument();
  await expand();
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
  await screen.findByRole("button", { name: /No attention needed/ });
  await expand();
  await screen.findByText("Nothing needs attention in this report.");
  expect(screen.getByRole("region", { name: "All-Games attention report" }))
    .toHaveTextContent("All Games. Refresh after making changes to check again.");
  rerender(<AttentionView />);
  await act(async () => {
    window.dispatchEvent(new Event("focus"));
    window.dispatchEvent(new Event("online"));
    focusManager.setFocused(false);
    focusManager.setFocused(true);
    onlineManager.setOnline(false);
    onlineManager.setOnline(true);
    await new Promise((resolve) => setTimeout(resolve, 20));
  });
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
  const indicator = await screen.findByRole("button", { name: /Needs attention/ });
  expect(indicator).toHaveTextContent("All Games · 10 findings");
  expect(indicator).toHaveAttribute("aria-expanded", "false");
  expect(screen.queryByRole("heading")).not.toBeInTheDocument();
  await expand();
  await screen.findByRole("heading", { name: "Staged Library operations (1)" });
  for (const title of [
    "Mod reinstalls", "Enable or disable transitions", "Model Importer recovery",
    "Staged Library operations", "Interrupted Game launches", "Active Game Session",
    "Unreferenced Library folders", "Duplicate Mod records", "Library root overlaps", "Mod path overlaps",
  ]) {
    expect(screen.getByRole("heading", { name: `${title} (1)` })).toBeInTheDocument();
  }
  const region = screen.getByRole("region", { name: "All-Games attention report" });
  for (const evidence of [
    "C:\\Library\\mod", "C:\\Game\\Mods\\mod", "C:\\Game", "C:\\Backup", "C:\\Library\\staged",
    "2026-10-07T12:00:00Z", "Child PID: unavailable", "PID 42", "2026-10-07T12:01:00Z",
    "C:\\Library\\orphan", "C:\\Library\\duplicate", "C:\\Backup\\Library", "C:\\Backup\\mod", "Stranded Mod",
    "Review the affected Mod's reinstall recovery warning.",
    "Review the affected Mod's transition recovery warning.",
    "Review the affected Game's Model Importer recovery warning.",
    "restart GMM to retry recovery", "Review the interrupted launch warning", "Close the Game before changing its Mods.",
    "Inspect, recover, or explicitly delete these folders", "Review duplicate ownership", "Review configured roots", "Review stranded Mod records",
    "Reinstall still owns this Library path.", "A producer still owns this transition.",
    "Model Importer replacement still owns these paths.", "A producer still owns this staged Library path.",
    "Checked at", "each action checks its own availability",
  ]) expect(region).toHaveTextContent(evidence);
  expect(region.querySelector("time")?.dateTime).toMatch(/^\d{4}-/);
  expect(screen.getByRole("button", { name: "Refresh attention" })).toBeEnabled();
  expect(screen.getByRole("region", { name: "Attention across all Games" })).toHaveTextContent("Stranded Mod");
  expect(screen.queryByText("Nothing needs attention in this report.")).not.toBeInTheDocument();
});

it("shows an unavailable sub-report as an error even after a clean check", async () => {
  attentionStatus.mockResolvedValueOnce(clean).mockRejectedValueOnce({
    kind: "other", message: "Library audit unavailable: access denied",
  });
  renderWithQuery(<AttentionView />);
  await screen.findByRole("button", { name: /No attention needed/ });
  await expand();
  await screen.findByText("Nothing needs attention in this report.");
  await userEvent.click(screen.getByRole("button", { name: "Refresh attention" }));
  const alert = await screen.findByRole("alert");
  expect(alert).toHaveTextContent("Could not check attention");
  expect(alert).toHaveTextContent("Library audit unavailable: access denied");
  expect(screen.queryByText("Nothing needs attention in this report.")).not.toBeInTheDocument();
  expect(attentionStatus).toHaveBeenCalledTimes(2);
  expect(screen.getByText(/Previous snapshot/)).toBeInTheDocument();
  expect(screen.getByRole("button", { expanded: true })).toHaveTextContent("Attention unavailable");
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

it("keeps an initial check unknown and prevents duplicate refresh", async () => {
  const check = deferred<AttentionReport>();
  attentionStatus.mockReturnValue(check.promise);
  renderWithQuery(<AttentionView />);
  const indicator = screen.getByRole("button", { expanded: false });
  expect(indicator).toHaveTextContent("Checking attention…");
  expect(indicator).not.toHaveTextContent(/No attention needed|Needs attention|findings/);
  expect(screen.queryByRole("img", { name: "Warning" })).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Refresh attention" })).toBeDisabled();
  const region = await expand();
  expect(region).not.toHaveTextContent(/Nothing needs attention|Checked at/);
  await userEvent.click(screen.getByRole("button", { name: "Refresh attention" }));
  expect(attentionStatus).toHaveBeenCalledTimes(1);
  await act(async () => check.resolve(clean));
  await screen.findByText("Nothing needs attention in this report.");
});

it("keeps a first failed check unavailable without inventing either safety outcome", async () => {
  attentionStatus.mockRejectedValue({ kind: "other", message: "Check access denied" });
  renderWithQuery(<AttentionView />);
  const alert = await screen.findByRole("alert");
  expect(alert).toHaveTextContent("Check access denied");
  const indicator = screen.getByRole("button", { expanded: false });
  expect(indicator).toHaveTextContent("Attention unavailable");
  expect(indicator).not.toHaveTextContent(/No attention needed|Needs attention|findings/);
  expect(screen.queryByRole("img", { name: "Warning" })).not.toBeInTheDocument();
  const region = await expand();
  expect(region).not.toHaveTextContent(/Nothing needs attention|Checked at|Previous snapshot/);
  expect(within(region).queryByRole("heading", { level: 3 })).not.toBeInTheDocument();
  attentionStatus.mockResolvedValueOnce(clean);
  await userEvent.click(screen.getByRole("button", { name: "Refresh attention" }));
  await screen.findByText("Nothing needs attention in this report.");
  expect(attentionStatus).toHaveBeenCalledTimes(2);
});

it.each([true, false])("labels retained safe=%s reports as previous during refresh and failure", async (safeToProceed) => {
  const old: AttentionReport = { ...clean, safeToProceed,
    activeSession: safeToProceed ? null : { game: "efmi", pid: 42, startedAt: "2026-10-07" } };
  const check = deferred<AttentionReport>();
  attentionStatus.mockResolvedValueOnce(old).mockReturnValueOnce(check.promise);
  renderWithQuery(<AttentionView />);
  await screen.findByRole("button", { name: safeToProceed ? /No attention needed/ : /Needs attention/ });
  const region = await expand();
  const originalTime = region.querySelector("time")!.dateTime;
  await userEvent.click(screen.getByRole("button", { name: "Refresh attention" }));
  const indicator = screen.getByRole("button", { expanded: true });
  expect(indicator).toHaveTextContent("Refreshing attention…");
  expect(screen.getAllByRole("status").find((line) => line.textContent === "Refreshing attention…")).toBeInTheDocument();
  expect(indicator).not.toHaveTextContent(/No attention needed|Needs attention|findings/);
  expect(screen.queryByRole("img", { name: "Warning" })).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Refresh attention" })).toBeDisabled();
  expect(region).toHaveTextContent("Previous snapshot. Checked at");
  expect(region.querySelector("time")).toHaveAttribute("datetime", originalTime);
  expect(screen.queryByText("Nothing needs attention in this report.")).not.toBeInTheDocument();
  if (!safeToProceed) expect(within(region).getByRole("heading", { name: "Active Game Session (1)" })).toBeInTheDocument();
  await act(async () => check.reject({ kind: "other", message: "Refresh failed" }));
  await screen.findByRole("alert");
  expect(indicator).toHaveTextContent("Attention unavailable");
  expect(indicator).not.toHaveTextContent(/No attention needed|Needs attention|findings/);
  expect(screen.queryByRole("img", { name: "Warning" })).not.toBeInTheDocument();
  expect(region).toHaveTextContent("Previous snapshot. Checked at");
  expect(region.querySelector("time")).toHaveAttribute("datetime", originalTime);
  expect(screen.queryByText("Nothing needs attention in this report.")).not.toBeInTheDocument();
  attentionStatus.mockResolvedValueOnce(clean);
  await userEvent.click(screen.getByRole("button", { name: "Refresh attention" }));
  await screen.findByText("Nothing needs attention in this report.");
  expect(region).not.toHaveTextContent("Previous snapshot");
  expect(within(region).queryByRole("heading", { name: "Active Game Session (1)" })).not.toBeInTheDocument();
});

it.each([
  ["backend unsafe with zero findings", { ...clean, safeToProceed: false }, /Needs attention/, "0 findings"],
  ["backend safe with findings", { ...clean, activeSession: { game: "efmi", pid: 42, startedAt: "2026-10-07" } }, /No attention needed/, "1 finding"],
  ["active session alone", { ...clean, safeToProceed: false, activeSession: { game: "efmi", pid: 42, startedAt: "2026-10-07" } }, /Needs attention/, "1 finding"],
] as const)("uses the backend safety flag for %s", async (_name, report, label, count) => {
  attentionStatus.mockResolvedValue(report);
  renderWithQuery(<AttentionView />);
  const indicator = await screen.findByRole("button", { name: label });
  expect(indicator.querySelector(".attention-indicator__scope")?.textContent).toBe(`All Games · ${count}`);
  expect(indicator).not.toHaveTextContent(/broken Mods/i);
  expect(indicator).toHaveAttribute("aria-expanded", "false");
  expect(screen.queryByRole("heading")).not.toBeInTheDocument();
  expect(screen.queryByRole("img", { name: "Warning" }) !== null).toBe(report.safeToProceed === false);
  const region = await expand();
  if (report.activeSession) expect(within(region).getByRole("heading", { name: "Active Game Session (1)" })).toBeInTheDocument();
});

it.each([
  ["Conflict", ["conflicts", "gimi"], { conflicts: [{ hash: "abcd", modIds: ["a", "b"] }] }],
  ["available Mod update", ["modUpdates", "gimi"], [{ modId: "a", updateAvailable: true }]],
  ["available Importer update", ["importer", "update", "gimi"], { updateAvailable: true }],
  ["unreadable install path", ["gamePathStatus", "gimi"], { gamePath: { status: "unreadable", error: "Access denied" } }],
  ["failed path check", ["gamePathStatus", "gimi"], undefined],
] as const)("does not borrow a loud trigger from %s", async (_name, key, value) => {
  const client = makeQueryClient();
  client.setDefaultOptions({ queries: { retry: false, gcTime: Infinity } });
  if (value) client.setQueryData(key, value);
  else {
    await client.fetchQuery({ queryKey: key, queryFn: () => Promise.reject(new Error("Path check failed")) }).catch(() => {});
  }
  attentionStatus.mockResolvedValue(clean);
  renderWithQuery(<AttentionView />, { client });
  const indicator = await screen.findByRole("button", { name: /No attention needed/ });
  expect(indicator).toHaveTextContent("All Games · 0 findings");
  expect(screen.queryByRole("img", { name: "Warning" })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: /Needs attention/ })).not.toBeInTheDocument();
  expect(attentionStatus).toHaveBeenCalledWith(expect.objectContaining({ queryKey: ["attentionStatus"] }));
});

it("counts findings across repeated Mods and non-Mod ownership, without performing recovery on expansion", async () => {
  attentionStatus.mockResolvedValue({ ...clean, safeToProceed: false,
    reinstalls: [{ modId: "same", game: "gimi", libraryPath: "C:/Library/same", recovery: null }],
    enabledTransitions: [{ modId: "same", game: "gimi", intendedEnabled: false, junctionPath: "C:/Game/Mods/same", recovery: null }],
    activeSession: { game: "efmi", pid: 42, startedAt: "2026-10-07" },
  });
  renderWithQuery(<AttentionView />);
  const indicator = await screen.findByRole("button", { name: /Needs attention/ });
  expect(indicator).toHaveTextContent("3 findings");
  expect(indicator).not.toHaveTextContent(/broken|Mods/);
  const region = await expand();
  expect(within(region).getAllByRole("heading", { level: 3 })).toHaveLength(3);
  expect(attentionStatus).toHaveBeenCalledTimes(1);
  expect(invoke).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole("button", { name: "Collapse attention report" }));
  expect(indicator).toHaveFocus();
  expect(indicator).toHaveAttribute("aria-expanded", "false");
  expect(screen.queryByRole("region", { name: "All-Games attention report" })).not.toBeInTheDocument();
  expect(attentionStatus).toHaveBeenCalledTimes(1);
  expect(invoke).not.toHaveBeenCalled();
});

it.each(["clean", "unsafe", "pending", "failed"])("expands and collapses from the indicator without fetching while %s", async (state) => {
  const pending = deferred<AttentionReport>();
  if (state === "pending") attentionStatus.mockReturnValue(pending.promise);
  else if (state === "failed") attentionStatus.mockRejectedValue(new Error("Check failed"));
  else attentionStatus.mockResolvedValue({ ...clean, safeToProceed: state === "clean" });
  renderWithQuery(<AttentionView />);
  const label = state === "pending" ? /Checking attention/ : state === "failed" ? /Attention unavailable/
    : state === "clean" ? /No attention needed/ : /Needs attention/;
  const indicator = await screen.findByRole("button", { name: label });
  expect(attentionStatus).toHaveBeenCalledTimes(1);
  await userEvent.click(indicator);
  expect(indicator).toHaveAttribute("aria-expanded", "true");
  expect(screen.getByRole("region", { name: "All-Games attention report" })).toBeVisible();
  expect(attentionStatus).toHaveBeenCalledTimes(1);
  await userEvent.click(indicator);
  expect(indicator).toHaveAttribute("aria-expanded", "false");
  expect(screen.queryByRole("region", { name: "All-Games attention report" })).not.toBeInTheDocument();
  expect(indicator).toHaveFocus();
  expect(attentionStatus).toHaveBeenCalledTimes(1);
  await userEvent.click(indicator);
  await userEvent.click(screen.getByRole("button", { name: "Collapse attention report" }));
  expect(indicator).toHaveAttribute("aria-expanded", "false");
  expect(attentionStatus).toHaveBeenCalledTimes(1);
  if (state === "pending") await act(async () => pending.resolve(clean));
});

it.each([null, undefined, "true"])("keeps a successful report with safety flag %s unavailable", async (flag) => {
  const report: Partial<AttentionReport> = { ...clean };
  if (flag === undefined) delete report.safeToProceed;
  else Object.assign(report, { safeToProceed: flag });
  attentionStatus.mockResolvedValue(report);
  renderWithQuery(<AttentionView />);
  const indicator = await screen.findByRole("button", { name: /Attention unavailable/ });
  expect(indicator).not.toHaveTextContent(/No attention needed|Needs attention|findings/);
  expect(screen.queryByRole("img", { name: "Warning" })).not.toBeInTheDocument();
  expect(screen.getByRole("status")).toHaveTextContent("Attention status is unavailable. Refresh to try again.");
  const region = await expand();
  expect(region).not.toHaveTextContent(/Nothing needs attention|previous report found nothing/);
  expect(screen.getByRole("button", { name: "Refresh attention" })).toBeEnabled();
});

it.each(["indicator", "report"])("consumes Escape at the %s before a window destination handler", async (target) => {
  const destinationEscape = vi.fn();
  window.addEventListener("keydown", destinationEscape);
  try {
    attentionStatus.mockResolvedValue(clean);
    renderWithQuery(<AttentionView />);
    const indicator = await screen.findByRole("button", { name: /No attention needed/ });
    indicator.focus();
    await userEvent.keyboard("{Enter}");
    expect(indicator).toHaveFocus();
    const region = screen.getByRole("region", { name: "All-Games attention report" });
    if (target === "report") within(region).getByRole("button", { name: "Collapse attention report" }).focus();
    destinationEscape.mockClear();
    await userEvent.keyboard("{Escape}");
    expect(indicator).toHaveAttribute("aria-expanded", "false");
    expect(indicator).toHaveFocus();
    expect(destinationEscape).not.toHaveBeenCalled();
    expect(attentionStatus).toHaveBeenCalledTimes(1);
  } finally { window.removeEventListener("keydown", destinationEscape); }
});

it("links the expansion control to its keyboard-reachable report and returns focus on Escape", async () => {
  attentionStatus.mockResolvedValue(clean);
  renderWithQuery(<AttentionView />);
  const indicator = await screen.findByRole("button", { name: /No attention needed/ });
  indicator.focus();
  await userEvent.keyboard("{Enter}");
  const region = screen.getByRole("region", { name: "All-Games attention report" });
  expect(indicator).toHaveAttribute("aria-controls", region.id);
  expect(indicator).toHaveAttribute("aria-expanded", "true");
  await userEvent.tab();
  expect(screen.getByRole("button", { name: "Refresh attention" })).toHaveFocus();
  await userEvent.tab();
  expect(within(region).getByRole("button", { name: "Collapse attention report" })).toHaveFocus();
  await userEvent.keyboard("{Escape}");
  expect(indicator).toHaveFocus();
  expect(indicator).toHaveAttribute("aria-expanded", "false");
});

it("does not poll or automatically refresh when other query data changes", async () => {
  attentionStatus.mockResolvedValue(clean);
  vi.useFakeTimers();
  try {
    const { client } = renderWithQuery(<AttentionView />);
    await act(async () => { await vi.advanceTimersByTimeAsync(100); });
    expect(screen.getByRole("button", { name: /No attention needed/ })).toBeInTheDocument();
    await act(async () => {
      client.setQueryData(["mods", "gimi"], []);
      client.setQueryData(["session"], null);
      await client.invalidateQueries({ queryKey: ["gamePathStatus"] });
      await vi.advanceTimersByTimeAsync(300_000);
    });
    expect(attentionStatus).toHaveBeenCalledTimes(1);
  } finally { vi.useRealTimers(); }
});

it("keeps loud and quiet controls equally compact without a heavy fill", async () => {
  const css: string = readFileSync("src/App.css", "utf8");
  const style = document.createElement("style");
  style.textContent = css;
  document.head.appendChild(style);
  try {
    attentionStatus.mockResolvedValueOnce(clean).mockResolvedValueOnce({ ...clean, safeToProceed: false });
    renderWithQuery(<AttentionView />);
    const indicator = await screen.findByRole("button", { name: /No attention needed/ });
    const quiet = getComputedStyle(indicator);
    const size = [quiet.padding, quiet.fontSize, quiet.fontWeight, quiet.borderWidth];
    await userEvent.click(screen.getByRole("button", { name: "Refresh attention" }));
    await screen.findByRole("button", { name: /Needs attention/ });
    const loud = getComputedStyle(indicator);
    expect([loud.padding, loud.fontSize, loud.fontWeight, loud.borderWidth]).toEqual(size);
    expect(indicator).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByRole("img", { name: "Warning" })).toBeInTheDocument();

    // jsdom can return transparent/no image on repeated computed-style reads.
    // Read the authored rules matching the real control;
    // the single dark palette applies regardless of the OS theme.
    const quietControl = document.createElement("button");
    quietControl.className = "attention-indicator__toggle";
    const authoredRules = css.replace(/\/\*[\s\S]*?\*\//g, "");
    const controls = authoredRules.match(/\.attention-indicator__refresh\s*\{([^{}]*)\}/)![1];
    expect(controls).toMatch(/(?:^|;)\s*background:\s*transparent\s*;/);
    expect(controls).toMatch(/(?:^|;)\s*color:\s*inherit\s*;/);
    expect(controls).toMatch(/(?:^|;)\s*border:\s*1px solid #71717a\s*;/);
    const loudRules = [...authoredRules.matchAll(/([^{}]+)\{([^{}]*)\}/g)].filter(([, selectors]) =>
      selectors.split(",").some((selector) => indicator.matches(selector.trim()) && !quietControl.matches(selector.trim())));
    expect(loudRules).toHaveLength(1);
    expect(loudRules[0][2]).toMatch(/border-color:\s*#fbbf24\s*;/);
    expect(loudRules[0][2]).toMatch(/(?:^|;)\s*color:\s*#fbbf24\s*;/);
    const scope = authoredRules.match(/\.attention-indicator__scope\s*\{([^{}]*)\}/)![1];
    expect(scope).toMatch(/(?:^|;)\s*color:\s*#a1a1aa\s*;/);
    for (const [, , declarations] of loudRules) {
      const properties = declarations.split(";").map((declaration) => declaration.trim()).filter(Boolean)
        .map((declaration) => declaration.split(":")[0].trim()).sort();
      expect(properties, "Loud changes only warning tint; the dark palette retains the quiet footprint and no fill")
        .toEqual(["border-color", "color"]);
    }
  } finally { style.remove(); }
});

it("retains recorded recovery reasons and exact owned paths", async () => {
  attentionStatus.mockResolvedValue({ ...clean, safeToProceed: false,
    reinstalls: [{ modId: "r", game: "gimi", libraryPath: "C:/Library/r", recovery: {
      reason: "Recorded reinstall obstruction", attemptedAt: "2026-10-07", attempts: 1,
      libraryPath: "C:/Library/r", stagedPath: "C:/Library/staged", quarantinePath: "C:/Library/old",
      junctionWithdrawn: false, junctionWithdrawalError: "Access denied",
    } }],
    enabledTransitions: [{ modId: "t", game: "srmi", intendedEnabled: true, junctionPath: "C:/SRMI/Mods/t", recovery: {
      reason: "Recorded transition obstruction", attemptedAt: "2026-10-07", attempts: 1,
      intendedEnabled: true, junctionPath: "C:/SRMI/Mods/t", ownerUncertain: true,
    } }],
    importerEvacuations: [{ game: "zzmi", gamePath: "C:/ZZMI/Game", backupPath: "C:/ZZMI/Backup", recovery: {
      reason: "Recorded importer obstruction", attemptedAt: "2026-10-07", attempts: 1,
      gamePath: "C:/ZZMI/Game", backupPath: "C:/ZZMI/Backup", ownerUncertain: false, action: "retry",
    } }],
  });
  renderWithQuery(<AttentionView />);
  await screen.findByRole("button", { name: /Needs attention/ });
  const region = await expand();
  for (const text of ["Recorded reinstall obstruction", "Recorded transition obstruction", "Recorded importer obstruction",
    "C:/Library/r", "C:/SRMI/Mods/t", "C:/ZZMI/Game", "C:/ZZMI/Backup", "Enabling"])
    expect(region).toHaveTextContent(text);
});

it("reuses the mounted snapshot across remounts until explicit refresh", async () => {
  const client = makeQueryClient();
  client.setDefaultOptions({ queries: { retry: false, gcTime: Infinity } });
  attentionStatus.mockResolvedValue(clean);
  const first = renderWithQuery(<AttentionView />, { client });
  await screen.findByRole("button", { name: /No attention needed/ });
  first.unmount();
  renderWithQuery(<AttentionView />, { client });
  await screen.findByRole("button", { name: /No attention needed/ });
  await expand();
  expect(attentionStatus).toHaveBeenCalledTimes(1);
  await userEvent.click(screen.getByRole("button", { name: "Refresh attention" }));
  await waitFor(() => expect(attentionStatus).toHaveBeenCalledTimes(2));
});
