import { beforeEach, describe, expect, it, vi } from "vitest";

// The updater module talks to two Tauri plugins. Both are mocked so the
// state machine can be driven deterministically without a backend.
const check = vi.fn();
const relaunch = vi.fn();

vi.mock("@tauri-apps/plugin-updater", () => ({
  check: (...args: unknown[]) => check(...args),
}));
vi.mock("@tauri-apps/plugin-process", () => ({
  relaunch: (...args: unknown[]) => relaunch(...args),
}));

const { checkAndInstallSilently, checkInteractively } = await import("./updater");

type StateKind = string;

/** Collect the `kind` of every state the FSM passes through. */
function recorder() {
  const kinds: StateKind[] = [];
  const states: unknown[] = [];
  return {
    kinds,
    states,
    onState: (s: { kind: StateKind }) => {
      kinds.push(s.kind);
      states.push(s);
    },
  };
}

/** Build a fake `Update` whose downloadAndInstall replays progress events. */
function fakeUpdate(opts: { version?: string; body?: string; contentLength?: number } = {}) {
  return {
    version: opts.version ?? "0.2.0",
    body: opts.body ?? null,
    downloadAndInstall: vi.fn(
      async (onEvent: (e: Record<string, unknown>) => void) => {
        onEvent({ event: "Started", data: { contentLength: opts.contentLength ?? 100 } });
        onEvent({ event: "Progress", data: { chunkLength: 40 } });
        onEvent({ event: "Progress", data: { chunkLength: 60 } });
        onEvent({ event: "Finished", data: {} });
      },
    ),
  };
}

beforeEach(() => {
  check.mockReset();
  relaunch.mockReset();
});

describe("checkInteractively", () => {
  it("reports up-to-date when the backend returns no update", async () => {
    check.mockResolvedValue(null);
    const rec = recorder();

    await checkInteractively(rec.onState);

    expect(rec.kinds).toEqual(["checking", "up-to-date"]);
  });

  it("walks checking → available → downloading → installed on a real update", async () => {
    check.mockResolvedValue(fakeUpdate({ version: "9.9.9" }));
    const rec = recorder();

    await checkInteractively(rec.onState);

    expect(rec.kinds[0]).toBe("checking");
    expect(rec.kinds).toContain("available");
    expect(rec.kinds).toContain("downloading");
    expect(rec.kinds[rec.kinds.length - 1]).toBe("installed");
  });

  it("surfaces the offered version so the UI can name it", async () => {
    check.mockResolvedValue(fakeUpdate({ version: "1.2.3" }));
    const rec = recorder();

    await checkInteractively(rec.onState);

    const available = rec.states.find(
      (s): s is { kind: string; version: string } =>
        (s as { kind: string }).kind === "available",
    );
    expect(available?.version).toBe("1.2.3");
  });

  it("accumulates downloaded bytes across progress events", async () => {
    check.mockResolvedValue(fakeUpdate({ contentLength: 100 }));
    const rec = recorder();

    await checkInteractively(rec.onState);

    const downloads = rec.states.filter(
      (s): s is { kind: string; downloaded: number; total: number | null } =>
        (s as { kind: string }).kind === "downloading",
    );
    // Started(0) → Progress(+40) → Progress(+60)
    expect(downloads[downloads.length - 1]?.downloaded).toBe(100);
    expect(downloads[downloads.length - 1]?.total).toBe(100);
  });

  it("reports an error state instead of throwing when the check fails", async () => {
    check.mockRejectedValue(new Error("network unreachable"));
    const rec = recorder();

    await expect(checkInteractively(rec.onState)).resolves.toBeUndefined();

    expect(rec.kinds).toEqual(["checking", "error"]);
    const err = rec.states[rec.states.length - 1] as { message: string };
    expect(err.message).toContain("network unreachable");
  });

  it("reports an error state when the download fails midway", async () => {
    check.mockResolvedValue({
      version: "0.2.0",
      body: null,
      downloadAndInstall: vi.fn(async () => {
        throw new Error("signature mismatch");
      }),
    });
    const rec = recorder();

    await checkInteractively(rec.onState);

    expect(rec.kinds[rec.kinds.length - 1]).toBe("error");
    expect(
      (rec.states[rec.states.length - 1] as { message: string }).message,
    ).toContain("signature mismatch");
  });

  it("never relaunches on its own — that stays the caller's decision", async () => {
    check.mockResolvedValue(fakeUpdate());
    const rec = recorder();

    await checkInteractively(rec.onState);

    expect(relaunch).not.toHaveBeenCalled();
  });
});

describe("checkAndInstallSilently", () => {
  it("does nothing when there is no update", async () => {
    check.mockResolvedValue(null);

    await checkAndInstallSilently();

    expect(relaunch).not.toHaveBeenCalled();
  });

  it("installs and relaunches when an update exists", async () => {
    const update = fakeUpdate();
    check.mockResolvedValue(update);

    await checkAndInstallSilently();

    expect(update.downloadAndInstall).toHaveBeenCalledOnce();
    expect(relaunch).toHaveBeenCalledOnce();
  });

  it("swallows failures so a broken update check can never block startup", async () => {
    check.mockRejectedValue(new Error("GitHub 503"));

    await expect(checkAndInstallSilently()).resolves.toBeUndefined();
    expect(relaunch).not.toHaveBeenCalled();
  });

  it("does not relaunch when the install itself fails", async () => {
    check.mockResolvedValue({
      version: "0.2.0",
      body: null,
      downloadAndInstall: vi.fn(async () => {
        throw new Error("disk full");
      }),
    });

    await expect(checkAndInstallSilently()).resolves.toBeUndefined();
    expect(relaunch).not.toHaveBeenCalled();
  });
});
