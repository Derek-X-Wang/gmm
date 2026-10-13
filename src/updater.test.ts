import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createUpdateController as createController, declinedBuildKey } from "./updater";
import { memorySkipStorage } from "./test/updaterStorage";
let storage = memorySkipStorage();
const createUpdateController = (check: Parameters<typeof createController>[0], store = storage) => createController(check, store);

vi.mock("@tauri-apps/plugin-updater", () => ({ check: vi.fn() }));

function fakeUpdate(version = "0.5.0") {
  return {
    version,
    body: "Release notes",
    rawJson: { url: `https://example.com/${version}.msi`, signature: `signed-${version}` },
    downloadAndInstall: vi.fn(async (onEvent) => {
      onEvent({ event: "Started", data: { contentLength: 100 } });
      onEvent({ event: "Progress", data: { chunkLength: 40 } });
      onEvent({ event: "Progress", data: { chunkLength: 60 } });
      onEvent({ event: "Finished" });
    }),
  };
}

beforeEach(() => { storage = memorySkipStorage(); });

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
    await controller.decline();
    await controller.install();
    expect(update.downloadAndInstall).not.toHaveBeenCalled();
    expect(storage.getItem(declinedBuildKey)).toContain("signed-0.5.0");
    expect(controller.getSnapshot()).toEqual({ kind: "declined", version: "0.5.0" });
    expect(update.downloadAndInstall).not.toHaveBeenCalled();
  });

  it("a fresh launch suppresses a skipped version but offers a different version", async () => {
    const skipped = fakeUpdate();
    const firstLaunch = createUpdateController(vi.fn().mockResolvedValue(skipped));
    await firstLaunch.checkForUpdates(true);
    await firstLaunch.decline();
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
    await controller.decline();
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

it("skipping build A never suppresses build B with the same application version", async () => {
  const a = fakeUpdate("0.4.0");
  const b = { ...fakeUpdate("0.4.0"), rawJson: { url: "https://example.com/alpha.3.msi", signature: "signed-alpha.3" } };
  const first = createUpdateController(vi.fn().mockResolvedValue(a));
  await first.checkForUpdates(true);
  await first.decline();
  const restarted = createUpdateController(vi.fn().mockResolvedValue(b));
  await restarted.checkForUpdates(true);
  expect(restarted.getSnapshot()).toMatchObject({ kind: "available", version: "0.4.0" });
  expect(b.downloadAndInstall).not.toHaveBeenCalled();
});

afterEach(() => vi.useRealTimers());
it("the controller keeps an unanswered offer without installing while time passes", async () => {
  vi.useFakeTimers();
  const update = fakeUpdate();
  const controller = createUpdateController(vi.fn().mockResolvedValue(update));
  await controller.checkForUpdates(true);
  expect(controller.getSnapshot()).toMatchObject({ kind: "available" });
  // Drain all currently scheduled work, including delays beyond the old 60s horizon.
  await vi.runOnlyPendingTimersAsync();
  for (const target of [window, document]) {
    for (const event of ["focus", "visibilitychange"]) {
      target.dispatchEvent(new Event(event));
      await vi.runOnlyPendingTimersAsync();
      expect(update.downloadAndInstall).not.toHaveBeenCalled();
    }
  }
  expect(update.downloadAndInstall).not.toHaveBeenCalled();
  expect(controller.getSnapshot()).toMatchObject({ kind: "available" });
  await controller.install();
  expect(update.downloadAndInstall).toHaveBeenCalledOnce();
});

it("missing signed artifact metadata fails toward offering the build again", async () => {
  const update = { version: "0.4.0", downloadAndInstall: vi.fn() };
  const first = createUpdateController(vi.fn().mockResolvedValue(update));
  await first.checkForUpdates(true);
  await first.decline();
  const restarted = createUpdateController(vi.fn().mockResolvedValue(update));
  await restarted.checkForUpdates(true);
  expect(restarted.getSnapshot()).toMatchObject({ kind: "available" });
  expect(update.downloadAndInstall).not.toHaveBeenCalled();
});

it.each([
  ["windows-x86_64", "new signature"],
  ["windows-x86_64", "new release URL"],
  ["windows-x86_64-msi", "new signature"],
  ["windows-x86_64-msi", "new release URL"],
])("a static manifest with %s's %s offers another build on the same version line", async (platform, change) => {
  const a = { ...fakeUpdate("0.4.0"), rawJson: { platforms: {
    "windows-x86_64-msi": { url: "https://example.com/alpha.2.msi", signature: "signed-alpha.2" },
    "windows-x86_64": { url: "https://example.com/alpha.2.msi", signature: "signed-alpha.2" },
  } } };
  const first = createUpdateController(vi.fn().mockResolvedValue(a));
  await first.checkForUpdates(true);
  await first.decline();
  // Reordering JSON keys is the same build and must stay quiet after restart.
  const reordered = { ...a, rawJson: { platforms: {
    "windows-x86_64": a.rawJson.platforms["windows-x86_64"],
    "windows-x86_64-msi": a.rawJson.platforms["windows-x86_64-msi"],
  } } };
  const same = createUpdateController(vi.fn().mockResolvedValue(reordered));
  await same.checkForUpdates(true);
  expect(same.getSnapshot()).toMatchObject({ kind: "declined", remembered: true });
  const b = { ...a, rawJson: { platforms: { ...a.rawJson.platforms,
    // Each platform contributes to identity, even when another sorts last.
    [platform]: change === "new signature"
      ? { url: "https://example.com/alpha.2.msi", signature: "signed-rebuilt" }
      : { url: "https://example.com/alpha.3.msi", signature: "signed-alpha.2" },
  } } };
  const different = createUpdateController(vi.fn().mockResolvedValue(b));
  await different.checkForUpdates(true);
  expect(different.getSnapshot()).toMatchObject({ kind: "available", version: "0.4.0" });
  expect(b.downloadAndInstall).not.toHaveBeenCalled();
});

it("an unidentifiable build is offered even when skip storage is unavailable", async () => {
  const update = { version: "0.4.0", downloadAndInstall: vi.fn() };
  const store = { getItem: vi.fn(() => { throw new Error("storage unavailable"); }), setItem: vi.fn() };
  const controller = createUpdateController(vi.fn().mockResolvedValue(update), store);
  await controller.checkForUpdates(true);
  expect(controller.getSnapshot()).toMatchObject({ kind: "available" });
  expect(update.downloadAndInstall).not.toHaveBeenCalled();
  await controller.decline();
  expect(controller.getSnapshot()).toEqual({ kind: "declined", version: "0.4.0", remembered: false });
  expect(store.getItem).not.toHaveBeenCalled();
  expect(store.setItem).not.toHaveBeenCalled();
  await controller.install();
  expect(update.downloadAndInstall).not.toHaveBeenCalled();
  const restarted = createUpdateController(vi.fn().mockResolvedValue(update), store);
  await restarted.checkForUpdates(true);
  expect(restarted.getSnapshot()).toMatchObject({ kind: "available" });
});

it.each([
  "not JSON",
  JSON.stringify({ version: "0.4.0" }),
  JSON.stringify({ version: "0.4.0", identity: 123 }),
])("an incompatible stored skip (%s) does not hide an update offer", async (stored) => {
  storage.setItem(declinedBuildKey, stored);
  const update = fakeUpdate("0.4.0");
  const controller = createUpdateController(vi.fn().mockResolvedValue(update));
  await controller.checkForUpdates(true);
  expect(controller.getSnapshot()).toMatchObject({ kind: "available", version: "0.4.0" });
  expect(storage.getItem(declinedBuildKey)).toBe(stored);
  expect(update.downloadAndInstall).not.toHaveBeenCalled();
});
