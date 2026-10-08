import { beforeEach, expect, it, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const {
  adoptFolder,
  attentionStatus,
  detectConflicts,
  importGamebanana,
  importZip,
  partitionLaunchError,
  recoverUnreferencedLibraryDir,
  resolveDuplicateMods,
  retryReinstallRecovery,
  setProxyConfig,
} = await import("./api");
const { CommandFailure } = await import("./commandError");

const RAW_MOD = {
  id: "01MOD",
  game: "gimi",
  name: "Test Mod",
  source: "manual",
  library_path: "C:\\GMM\\library\\gimi\\01MOD",
  enabled: false,
};

beforeEach(() => {
  vi.clearAllMocks();
  invoke.mockResolvedValue(RAW_MOD);
});

it("preserves a command failure's classification and user-facing message", async () => {
  invoke.mockRejectedValueOnce({
    kind: "invalidActiveVariant",
    message: "Select a valid Variant for this Mod, or reinstall it.",
  });

  const failure = await detectConflicts("gimi").catch((error) => error);

  expect(failure).toBeInstanceOf(CommandFailure);
  expect(failure).toMatchObject({
    kind: "invalidActiveVariant",
    message: "Select a valid Variant for this Mod, or reinstall it.",
  });
});

it("partitions launch presentation without discarding failure classification", () => {
  const partitioned = partitionLaunchError(
    {
      kind: "invalidActiveVariant",
      message: "AV-PATTERN: Select a valid Variant.",
    },
    "AV-PATTERN: ",
  );

  expect(partitioned.isAvPattern).toBe(true);
  expect(partitioned.failure).toBeInstanceOf(CommandFailure);
  expect(partitioned.failure).toMatchObject({
    kind: "invalidActiveVariant",
    message: "Select a valid Variant.",
  });
});

it.each([
  [
    "retryReinstallRecovery",
    () => retryReinstallRecovery("01MOD"),
    "retry_reinstall_recovery",
    { modId: "01MOD" },
  ],
  [
    "adoptFolder",
    () => adoptFolder("gimi", "C:\\source", "Adopted"),
    "adopt_folder",
    { args: { game: "gimi", sourcePath: "C:\\source", name: "Adopted" } },
  ],
  [
    "importZip",
    () => importZip("srmi", "C:\\downloads\\mod.zip", "Imported"),
    "import_zip",
    {
      args: {
        game: "srmi",
        zipPath: "C:\\downloads\\mod.zip",
        name: "Imported",
      },
    },
  ],
  [
    "importGamebanana",
    () => importGamebanana("zzmi", "https://gamebanana.com/mods/123"),
    "import_gamebanana",
    {
      args: {
        game: "zzmi",
        urlOrId: "https://gamebanana.com/mods/123",
      },
    },
  ],
  [
    "recoverUnreferencedLibraryDir",
    () => recoverUnreferencedLibraryDir("gimi", "C:\\GMM\\orphan", "Recovered"),
    "recover_unreferenced_library_dir",
    {
      args: {
        game: "gimi",
        path: "C:\\GMM\\orphan",
        name: "Recovered",
      },
    },
  ],
  [
    "resolveDuplicateMods",
    () => resolveDuplicateMods("01KEEPER", [
      { id: "01KEEPER", fingerprint: "keeper-fingerprint" },
      { id: "01REJECTED", fingerprint: "rejected-fingerprint" },
    ]),
    "resolve_duplicate_mods",
    {
      args: {
        keeperId: "01KEEPER",
        reviewedMods: [
          { id: "01KEEPER", fingerprint: "keeper-fingerprint" },
          { id: "01REJECTED", fingerprint: "rejected-fingerprint" },
        ],
      },
    },
  ],
  [
    "setProxyConfig",
    () =>
      setProxyConfig({
        url: "http://127.0.0.1:8080",
        username: "alice",
        password: null,
      }),
    "set_proxy_config",
    {
      args: {
        url: "http://127.0.0.1:8080",
        username: "alice",
        password: null,
      },
    },
  ],
] as const)(
  "%s sends the Tauri command's real invocation envelope",
  async (_, call, command, envelope) => {
    await call();

    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith(command, envelope);
  },
);

it("calls attention_status and normalizes its nested session wire fields", async () => {
  invoke.mockResolvedValueOnce({
    safeToProceed: false,
    reinstalls: [], enabledTransitions: [], importerEvacuations: [],
    stagedLibraryOperations: [{
      id: "01STAGED", game: "gimi", operation: "import_zip",
      stagedPath: "C:\\GMM\\library\\gimi\\01STAGED", recoveryError: "Retirement failed",
    }],
    sessionLaunches: [{ id: "01LAUNCH", game: "srmi", child_pid: 42, started_at: "2026-10-07T12:00:00Z" }],
    activeSession: { game: "gimi", pid: 123, started_at: "2026-10-07T12:01:00Z" },
    libraryAudits: [], libraryRootOverlaps: [], modPathOverlaps: [],
  });
  const report = await attentionStatus();
  expect(invoke).toHaveBeenCalledWith("attention_status", undefined);
  expect(report.activeSession).toEqual({ game: "gimi", pid: 123, startedAt: "2026-10-07T12:01:00Z" });
  expect(report.sessionLaunches).toEqual([{ id: "01LAUNCH", game: "srmi", childPid: 42, startedAt: "2026-10-07T12:00:00Z" }]);
  expect(report.stagedLibraryOperations[0]).toMatchObject({ recoveryError: "Retirement failed" });
});

it("propagates unavailable attention sub-reports instead of returning a healthy default", async () => {
  invoke.mockRejectedValueOnce({ kind: "other", message: "Cannot audit SRMI Library" });
  await expect(attentionStatus()).rejects.toMatchObject({ kind: "other", message: "Cannot audit SRMI Library" });
});
