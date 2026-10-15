import { act, fireEvent, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { renderWithQuery } from "./test/harness";
import { StatusBarUpdateReading, UpdatesPanel } from "./UpdatesPanel";
import { createUpdateController as createController, type UpdateController } from "./updater";
import { memorySkipStorage, signedMetadata } from "./test/updaterStorage";
let storage = memorySkipStorage();
const createUpdateController = (check: Parameters<typeof createController>[0], store = storage) =>
  createController(check, store);

const { relaunch } = vi.hoisted(() => ({ relaunch: vi.fn() }));
vi.mock("@tauri-apps/plugin-updater", () => ({ check: vi.fn() }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch }));
const openSettings = vi.fn();
beforeEach(() => {
  storage = memorySkipStorage();
  relaunch.mockReset().mockResolvedValue(undefined);
  openSettings.mockReset();
});

/**
 * The two surfaces an offer reaches, around one controller (#357).
 *
 * Settings owns the consent actions. The status bar states what the check
 * found and routes here; the `<footer>` stands in for the real bar so the
 * reading can be addressed as its own region, the way `App.test.tsx`
 * addresses the bar by its `contentinfo` landmark.
 */
function renderSurfaces(controller: UpdateController, surfaces: "both" | "bar" = "both") {
  return renderWithQuery(<>
    {surfaces === "both" ? <UpdatesPanel controller={controller} /> : null}
    <footer><StatusBarUpdateReading controller={controller} onSettings={openSettings} /></footer>
  </>);
}
const bar = () => screen.getByRole("contentinfo");
/** Every control the bar offers. Consent must never be among them. */
const barControls = () => within(bar()).queryAllByRole("button").map((button) => button.textContent);

it("checks on demand and distinguishes idle, checking and current", async () => {
  let resolve!: (value: null) => void;
  const check = vi.fn(() => new Promise<null>((yes) => { resolve = yes; }));
  const controller = createUpdateController(check);
  renderWithQuery(<UpdatesPanel controller={controller} />);
  expect(screen.getByText(/Signatures are verified/)).toBeVisible();
  expect(check).not.toHaveBeenCalled();
  expect(screen.queryByText(/latest release/)).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Check for updates" }));
  expect(screen.getByRole("button", { name: "Checking…" })).toBeDisabled();
  await act(async () => resolve(null));
  expect(await screen.findByText("You're on the latest release.")).toBeVisible();
  expect(screen.queryByRole("button", { name: "Restart to apply" })).not.toBeInTheDocument();
});

it("requires consent before the install command, then explicit restart, with the bar only reading", async () => {
  let progress!: (event: unknown) => void;
  let finish!: () => void;
  const downloadAndInstall = vi.fn((callback: typeof progress) => {
    progress = callback; return new Promise<void>((yes) => { finish = yes; });
  });
  const controller = createUpdateController(vi.fn().mockResolvedValue({ version: "0.5.0", rawJson: signedMetadata("0.5.0"), downloadAndInstall }));
  renderSurfaces(controller);
  await act(() => controller.checkForUpdates(true));
  expect(downloadAndInstall).not.toHaveBeenCalled();
  expect(await screen.findByText(/is available/)).toHaveTextContent("0.5.0");
  expect(screen.queryByRole("button", { name: "Restart to apply" })).not.toBeInTheDocument();

  // The bar states the offer. Its only control is the route to this group:
  // no Install, no Skip, no dismissal, so the bar cannot consent for the
  // user and cannot answer the offer on their behalf (#346, ADR 0004, #357).
  expect(bar()).toHaveTextContent("GMM update available: 0.5.0");
  expect(barControls()).toEqual(["Review GMM updates in Settings"]);
  await userEvent.click(within(bar()).getByRole("button", { name: "Review GMM updates in Settings" }));
  expect(openSettings).toHaveBeenCalledOnce();
  expect(downloadAndInstall).not.toHaveBeenCalled();

  await userEvent.click(screen.getByRole("button", { name: "Install update" }));
  expect(downloadAndInstall).toHaveBeenCalledOnce();
  expect(screen.getByText("Downloading…")).toBeVisible();
  await act(() => progress({ event: "Started", data: { contentLength: 200 } }));
  await act(() => progress({ event: "Progress", data: { chunkLength: 50 } }));
  expect(screen.getByText("Downloading (25%)")).toBeVisible();
  // The bar says a download is in flight and leaves the meter to Settings.
  expect(bar()).toHaveTextContent("GMM update downloading…");
  expect(bar()).not.toHaveTextContent("25%");
  await act(() => progress({ event: "Finished" }));
  expect(screen.queryByRole("button", { name: "Restart to apply" })).not.toBeInTheDocument();
  expect(relaunch).not.toHaveBeenCalled();
  await act(async () => finish());
  expect(screen.getByText(/Update installed/)).toBeVisible();
  expect(relaunch).not.toHaveBeenCalled();
  // Still a reading: the restart is an explicit click in Settings as well.
  expect(bar()).toHaveTextContent("GMM update installed · restart to apply");
  expect(barControls()).toEqual(["Review GMM updates in Settings"]);
  await userEvent.click(screen.getByRole("button", { name: "Restart to apply" }));
  expect(relaunch).toHaveBeenCalledOnce();
});

it("skipping the offer silences the bar, survives a fresh launch, and a new version reads again", async () => {
  const downloadAndInstall = vi.fn().mockResolvedValue(undefined);
  const check = vi.fn().mockResolvedValue({ version: "0.5.0", rawJson: signedMetadata("0.5.0"), downloadAndInstall });
  const controller = createUpdateController(check);
  const view = renderSurfaces(controller);
  await act(() => controller.checkForUpdates(true));
  expect(downloadAndInstall).not.toHaveBeenCalled();
  expect(bar()).toHaveTextContent("GMM update available: 0.5.0");
  await userEvent.click(screen.getByRole("button", { name: "Skip this build" }));
  expect(downloadAndInstall).not.toHaveBeenCalled();
  expect(relaunch).not.toHaveBeenCalled();
  expect(screen.queryByRole("button", { name: "Install update" })).not.toBeInTheDocument();
  expect(screen.getByText(/Version 0.5.0 was skipped/)).toBeVisible();
  // The question has been answered, so the bar goes quiet rather than
  // repeating it; Settings keeps the declined outcome visible above.
  expect(bar()).toBeEmptyDOMElement();
  view.unmount();

  const nextLaunch = createUpdateController(check);
  const nextView = renderSurfaces(nextLaunch);
  await act(() => nextLaunch.checkForUpdates(true));
  expect(bar()).toBeEmptyDOMElement();
  expect(downloadAndInstall).not.toHaveBeenCalled();
  nextView.unmount();

  check.mockResolvedValue({ version: "0.6.0", rawJson: signedMetadata("0.6.0"), downloadAndInstall });
  const laterLaunch = createUpdateController(check);
  renderSurfaces(laterLaunch);
  await act(() => laterLaunch.checkForUpdates(true));
  expect(screen.getByText(/is available/)).toHaveTextContent("0.6.0");
  expect(bar()).toHaveTextContent("GMM update available: 0.6.0");
  expect(downloadAndInstall).not.toHaveBeenCalled();
});

it("Settings keeps the declined outcome visible and allows deliberate reconsideration", async () => {
  const downloadAndInstall = vi.fn();
  const controller = createUpdateController(vi.fn().mockResolvedValue({ version: "0.5.0", rawJson: signedMetadata("0.5.0"), downloadAndInstall }));
  renderWithQuery(<UpdatesPanel controller={controller} />);
  await userEvent.click(screen.getByRole("button", { name: "Check for updates" }));
  await userEvent.click(screen.getByRole("button", { name: "Skip this build" }));
  expect(screen.getByText(/Version 0.5.0 was skipped/)).toBeVisible();
  await userEvent.click(screen.getByRole("button", { name: "Check for updates" }));
  expect(screen.getByRole("button", { name: "Install update" })).toBeVisible();
  expect(downloadAndInstall).not.toHaveBeenCalled();
});

it("shows check failure separately from up to date and can retry", async () => {
  const check = vi.fn().mockRejectedValue(new Error("network unreachable"));
  const controller = createUpdateController(check);
  renderSurfaces(controller);
  await act(() => controller.checkForUpdates(true));
  expect(screen.getByRole("alert")).toHaveTextContent("Update check failed: network unreachable");
  expect(screen.queryByText(/latest release/)).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Restart to apply" })).not.toBeInTheDocument();
  // A launch that could not check must not read like a launch that found
  // nothing. The bar names the failed check and keeps the message — and the
  // retry — in Settings, where the whole error is already shown.
  expect(bar()).toHaveTextContent("GMM update check unavailable");
  expect(bar()).not.toHaveTextContent("network unreachable");
  expect(barControls()).toEqual(["Review GMM updates in Settings"]);
  check.mockResolvedValue(null);
  await userEvent.click(screen.getByRole("button", { name: "Check for updates" }));
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  expect(screen.getByText("You're on the latest release.")).toBeVisible();
  expect(bar()).toBeEmptyDOMElement();
});

it("shows download failure without a current-release claim or restart", async () => {
  const downloadAndInstall = vi.fn().mockRejectedValue(new Error("signature mismatch"));
  const controller = createUpdateController(vi.fn().mockResolvedValue({ version: "0.5.0", rawJson: signedMetadata("0.5.0"), downloadAndInstall }));
  renderSurfaces(controller);
  await act(() => controller.checkForUpdates(true));
  expect(downloadAndInstall).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole("button", { name: "Install update" }));
  expect(screen.getByRole("alert")).toHaveTextContent("Update installation failed: signature mismatch");
  expect(screen.queryByText(/latest release/)).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Restart to apply" })).not.toBeInTheDocument();
  expect(relaunch).not.toHaveBeenCalled();
  // The build is not installed, and the bar says so rather than falling
  // silent on a failure that left the update unapplied.
  expect(bar()).toHaveTextContent("GMM update installation failed");
  expect(bar()).not.toHaveTextContent("signature mismatch");
});

it("a failed restart keeps the explicit restart action without offering another install", async () => {
  const downloadAndInstall = vi.fn().mockResolvedValue(undefined);
  const controller = createUpdateController(vi.fn().mockResolvedValue({ version: "0.5.0", rawJson: signedMetadata("0.5.0"), downloadAndInstall }));
  renderSurfaces(controller);
  await act(() => controller.checkForUpdates(true));
  await userEvent.click(screen.getByRole("button", { name: "Install update" }));
  relaunch.mockRejectedValueOnce(new Error("process unavailable"));
  await userEvent.click(screen.getByRole("button", { name: "Restart to apply" }));
  expect(screen.getByRole("alert")).toHaveTextContent("Restart failed: process unavailable");
  expect(screen.queryByRole("button", { name: "Install update" })).not.toBeInTheDocument();
  expect(bar()).toHaveTextContent("GMM update installed · restart to apply");
  await userEvent.click(screen.getByRole("button", { name: "Restart to apply" }));
  expect(relaunch).toHaveBeenCalledTimes(2);
  expect(downloadAndInstall).toHaveBeenCalledOnce();
});

afterEach(() => vi.useRealTimers());
it("keeps an unanswered offer without installing while time passes", async () => {
  vi.useFakeTimers();
  const downloadAndInstall = vi.fn().mockResolvedValue(undefined);
  const controller = createUpdateController(vi.fn().mockResolvedValue({ version: "0.5.0", downloadAndInstall }));
  renderSurfaces(controller);
  fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
  await act(async () => {});
  expect(screen.getByRole("button", { name: "Install update" })).toBeVisible();
  expect(bar()).toHaveTextContent("GMM update available: 0.5.0");
  // Advance five minutes of fake Date/performance time even with no queued timer.
  // tickAsync runs chained timeouts, interval ticks, animation frames and their
  // promise continuations due in this window; runOnlyPendingTimersAsync does not.
  // This is a bounded observation window, not a deadline for user consent.
  await act(async () => { await vi.advanceTimersByTimeAsync(300_000); });
  expect(downloadAndInstall).not.toHaveBeenCalled();
  for (const visibility of ["hidden", "visible"] as const) {
    const visibilityState = vi.spyOn(document, "visibilityState", "get").mockReturnValue(visibility);
    try {
      for (const target of [window, document]) {
        for (const event of ["focus", "visibilitychange"]) {
          await act(async () => {
            target.dispatchEvent(new Event(event));
            await vi.advanceTimersByTimeAsync(300_000);
          });
          expect(downloadAndInstall).not.toHaveBeenCalled();
        }
      }
    } finally {
      visibilityState.mockRestore();
    }
  }
  expect(downloadAndInstall).not.toHaveBeenCalled();
  // Nothing in the bar could have installed it either: the reading waited
  // with the offer and the route is the only thing it offers to click.
  expect(barControls()).toEqual(["Review GMM updates in Settings"]);
  fireEvent.click(within(bar()).getByRole("button", { name: "Review GMM updates in Settings" }));
  await act(async () => {});
  expect(downloadAndInstall).not.toHaveBeenCalled();
  expect(screen.getByRole("button", { name: "Install update" })).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: "Install update" }));
  await act(async () => {});
  expect(downloadAndInstall).toHaveBeenCalledOnce();
});

it("an offline startup check reads as unavailable without retrying or losing diagnostics", async () => {
  const check = vi.fn().mockRejectedValue(new Error("offline"));
  const controller = createUpdateController(check);
  renderSurfaces(controller);
  await act(() => controller.checkForUpdates(true));
  // The bar's short reading and the whole message in Settings, from one
  // check: the bar never retries on its own and nothing is lost by its
  // being short (#348's pattern — the fact in the bar, the detail behind
  // its route).
  expect(bar()).toHaveTextContent("GMM update check unavailable");
  expect(within(bar()).queryByRole("alert")).not.toBeInTheDocument();
  expect(screen.getByRole("alert")).toHaveTextContent("Update check failed: offline");
  expect(check).toHaveBeenCalledOnce();
  expect(controller.getLastError()).toBe("Update check failed: offline");
});

it("explains that an unidentifiable skipped build will be offered again", async () => {
  const downloadAndInstall = vi.fn();
  const controller = createUpdateController(vi.fn().mockResolvedValue({ version: "0.5.0", downloadAndInstall }));
  renderSurfaces(controller);
  await act(() => controller.checkForUpdates(true));
  await userEvent.click(screen.getByRole("button", { name: "Skip this build" }));
  expect(screen.getByText(/This build could not be identified/)).toHaveTextContent("It will be offered again on launch.");
  expect(controller.getSnapshot()).toEqual({ kind: "declined", version: "0.5.0", remembered: false });
  expect(storage.getItem("updater.skippedBuild")).toBeNull();
  expect(downloadAndInstall).not.toHaveBeenCalled();
  // An answered offer, so the bar is quiet here too. The re-offer next
  // launch is what brings the reading back, not a second prompt now.
  expect(bar()).toBeEmptyDOMElement();
});

it("the bar says nothing before an offer, when the build is current, or after a decline", async () => {
  let resolve!: (value: null) => void;
  const check = vi.fn(() => new Promise<null>((yes) => { resolve = yes; }));
  const controller = createUpdateController(check);
  const view = renderSurfaces(controller, "bar");
  expect(bar()).toBeEmptyDOMElement();
  const checking = act(() => controller.checkForUpdates(true));
  expect(bar()).toBeEmptyDOMElement();
  await act(async () => resolve(null));
  await checking;
  expect(bar()).toBeEmptyDOMElement();
  view.unmount();

  // A decline that could not be remembered is still a decline: Settings
  // explains that the build returns next launch, and the bar stays quiet
  // instead of turning a storage failure into a second prompt.
  const unwritable = { getItem: () => null, setItem: () => { throw new Error("storage unavailable"); } };
  const offer = { version: "0.5.0", rawJson: signedMetadata("0.5.0"), downloadAndInstall: vi.fn() };
  const declining = createUpdateController(vi.fn().mockResolvedValue(offer), unwritable);
  renderSurfaces(declining, "bar");
  await act(() => declining.checkForUpdates(true));
  expect(bar()).toHaveTextContent("GMM update available: 0.5.0");
  await act(() => declining.decline());
  expect(declining.getSnapshot()).toMatchObject({ kind: "error", failure: "rememberSkip" });
  expect(bar()).toBeEmptyDOMElement();
  expect(offer.downloadAndInstall).not.toHaveBeenCalled();
});

it("no longer exports a floating notice for anything to mount", async () => {
  // The fixed-position card is gone, not relocated (#357). `tsc` already
  // fails on an import of it, so an absent export is what makes "no floating
  // update card anywhere" a property of the module rather than a promise
  // about one call site.
  const module = await import("./UpdatesPanel");
  expect(Object.keys(module).sort()).toEqual(["StatusBarUpdateReading", "UpdatesPanel", "useUpdateReading"]);
});
