import { afterEach, beforeEach, expect, it, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("./updater", () => ({ updater: { checkForUpdates: vi.fn() } }));
vi.mock("./App", () => ({ default: () => null }));
vi.mock("react-dom/client", () => ({
  default: { createRoot: () => ({ render: vi.fn() }) },
}));

beforeEach(() => {
  vi.resetModules();
  invoke.mockReset().mockResolvedValue(undefined);
});

afterEach(() => {
  window.dispatchEvent(new Event("pagehide"));
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it.each([true, false])("the app entry records startup online=%s and both connectivity events", async (online) => {
  vi.spyOn(navigator, "onLine", "get").mockReturnValue(online);
  await import("./main");

  expect(invoke).toHaveBeenNthCalledWith(1, "log_network_signal", {
    signal: { online, reason: "startup" },
  });
  window.dispatchEvent(new Event("online"));
  expect(invoke).toHaveBeenNthCalledWith(2, "log_network_signal", {
    signal: { online: true, reason: "online" },
  });
  window.dispatchEvent(new Event("offline"));
  expect(invoke).toHaveBeenNthCalledWith(3, "log_network_signal", {
    signal: { online: false, reason: "offline" },
  });
  expect(invoke).toHaveBeenCalledTimes(3);
});

it("registers once and removes connectivity listeners on app teardown", async () => {
  await import("./main");
  const { installNetworkSignalLogging } = await import("./diagnostics");
  const remove = installNetworkSignalLogging();
  expect(installNetworkSignalLogging()).toBe(remove);
  expect(invoke).toHaveBeenCalledTimes(1);
  window.dispatchEvent(new Event("online"));
  window.dispatchEvent(new Event("offline"));
  expect(invoke).toHaveBeenCalledTimes(3);

  window.dispatchEvent(new Event("pagehide"));
  window.dispatchEvent(new Event("online"));
  window.dispatchEvent(new Event("offline"));
  expect(invoke).toHaveBeenCalledTimes(3);
});

it("keeps logging failures out of startup and connectivity event handlers", async () => {
  invoke.mockRejectedValue(new Error("logging unavailable"));
  await expect(import("./main")).resolves.toBeDefined();
  window.dispatchEvent(new Event("online"));
  window.dispatchEvent(new Event("offline"));
  const { logNetworkSignal } = await import("./diagnostics");
  await expect(logNetworkSignal(false, "offline")).resolves.toBeUndefined();
  expect(invoke).toHaveBeenCalledTimes(4);
});

it.each(["window", "navigator"])("skips registration when %s is absent", async (name) => {
  const { installNetworkSignalLogging } = await import("./diagnostics");
  vi.stubGlobal(name, undefined);
  expect(installNetworkSignalLogging()).toBeTypeOf("function");
  expect(invoke).not.toHaveBeenCalled();
  vi.unstubAllGlobals();
});
