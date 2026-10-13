import { beforeEach, describe, expect, it, vi } from "vitest";
import { createUpdateController, declinedVersionKey } from "./updater";

vi.mock("@tauri-apps/plugin-updater", () => ({ check: vi.fn() }));

function fakeUpdate(version = "0.5.0") {
  return {
    version,
    body: "Release notes",
    downloadAndInstall: vi.fn(async (onEvent) => {
      onEvent({ event: "Started", data: { contentLength: 100 } });
      onEvent({ event: "Progress", data: { chunkLength: 40 } });
      onEvent({ event: "Progress", data: { chunkLength: 60 } });
      onEvent({ event: "Finished" });
    }),
  };
}

beforeEach(() => localStorage.clear());

describe("update consent", () => {
  it.each([true, false])("check (background=%s) stops before any download or install", async (background) => {
    const update = fakeUpdate();
    const controller = createUpdateController(vi.fn().mockResolvedValue(update));
    await controller.checkForUpdates(background);
    expect(update.downloadAndInstall).not.toHaveBeenCalled();
    expect(controller.getSnapshot()).toMatchObject({ kind: "available", version: "0.5.0" });
    await controller.install();
    expect(update.downloadAndInstall).toHaveBeenCalledOnce();
    expect(controller.getSnapshot()).toEqual({ kind: "installed" });
  });

  it("declining persists the version and never installs, including a stale install action", async () => {
    const update = fakeUpdate();
    const controller = createUpdateController(vi.fn().mockResolvedValue(update));
    await controller.checkForUpdates(true);
    controller.decline();
    await controller.install();
    expect(update.downloadAndInstall).not.toHaveBeenCalled();
    expect(localStorage.getItem(declinedVersionKey)).toBe("0.5.0");
    expect(controller.getSnapshot()).toEqual({ kind: "declined", version: "0.5.0" });
    expect(update.downloadAndInstall).not.toHaveBeenCalled();
  });

  it("a fresh launch suppresses a skipped version but offers a different version", async () => {
    const skipped = fakeUpdate();
    const firstLaunch = createUpdateController(vi.fn().mockResolvedValue(skipped));
    await firstLaunch.checkForUpdates(true);
    firstLaunch.decline();
    const nextLaunch = createUpdateController(vi.fn().mockResolvedValue(skipped));
    await nextLaunch.checkForUpdates(true);
    expect(nextLaunch.getSnapshot()).toEqual({ kind: "declined", version: "0.5.0", remembered: true });
    expect(skipped.downloadAndInstall).not.toHaveBeenCalled();
    const newer = fakeUpdate("0.6.0");
    const laterLaunch = createUpdateController(vi.fn().mockResolvedValue(newer));
    await laterLaunch.checkForUpdates(true);
    expect(laterLaunch.getSnapshot()).toMatchObject({ kind: "available", version: "0.6.0" });
    expect(newer.downloadAndInstall).not.toHaveBeenCalled();
    // A deliberate check can reconsider the declined release.
    await nextLaunch.checkForUpdates();
    expect(nextLaunch.getSnapshot()).toMatchObject({ kind: "available", version: "0.5.0" });
    expect(skipped.downloadAndInstall).not.toHaveBeenCalled();
  });

  it("reports storage failure without installing or claiming the choice was saved", async () => {
    const update = fakeUpdate();
    const storage = { getItem: () => null, setItem: () => { throw new Error("storage unavailable"); } };
    const controller = createUpdateController(vi.fn().mockResolvedValue(update), storage);
    await controller.checkForUpdates(true);
    controller.decline();
    expect(controller.getSnapshot()).toMatchObject({ kind: "error", message: expect.stringContaining("could not remember") });
    await controller.install();
    expect(update.downloadAndInstall).not.toHaveBeenCalled();
  });

  it("reports unreadable decline storage rather than offering a previously declined release", async () => {
    const update = fakeUpdate();
    const storage = { getItem: () => { throw new Error("storage unavailable"); }, setItem: vi.fn() };
    const controller = createUpdateController(vi.fn().mockResolvedValue(update), storage);
    await controller.checkForUpdates(true);
    expect(controller.getSnapshot()).toMatchObject({ kind: "error" });
    expect(update.downloadAndInstall).not.toHaveBeenCalled();
  });

  it("serializes duplicate checks and installs and reports accumulated progress", async () => {
    let resolveCheck!: (update: ReturnType<typeof fakeUpdate>) => void;
    const check = vi.fn(() => new Promise<ReturnType<typeof fakeUpdate>>((resolve) => { resolveCheck = resolve; }));
    const controller = createUpdateController(check);
    const first = controller.checkForUpdates(true);
    await controller.checkForUpdates();
    expect(check).toHaveBeenCalledOnce();
    const update = fakeUpdate();
    resolveCheck(update);
    await first;
    const states: unknown[] = [];
    controller.subscribe(() => states.push(controller.getSnapshot()));
    const install = controller.install();
    await controller.install();
    await controller.checkForUpdates();
    await install;
    expect(update.downloadAndInstall).toHaveBeenCalledOnce();
    expect(check).toHaveBeenCalledOnce();
    expect(states).toContainEqual({ kind: "downloading", downloaded: 100, total: 100 });
  });

  it("does not claim installation succeeded at download Finished if the installer fails", async () => {
    const update = fakeUpdate();
    update.downloadAndInstall.mockImplementation(async (onEvent) => {
      onEvent({ event: "Finished" });
      throw new Error("installer failed");
    });
    const controller = createUpdateController(vi.fn().mockResolvedValue(update));
    const states: unknown[] = [];
    controller.subscribe(() => states.push(controller.getSnapshot()));
    await controller.checkForUpdates();
    await controller.install();
    expect(states).not.toContainEqual({ kind: "installed" });
    expect(controller.getSnapshot()).toEqual({ kind: "error", message: "Update installation failed: installer failed" });
  });
});
