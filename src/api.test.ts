import { beforeEach, expect, it, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const {
  adoptFolder,
  importGamebanana,
  importZip,
  recoverUnreferencedLibraryDir,
  resolveDuplicateMods,
  retryReinstallRecovery,
  setProxyConfig,
} = await import("./api");

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
    () => resolveDuplicateMods("01KEEPER", ["01KEEPER", "01REJECTED"]),
    "resolve_duplicate_mods",
    {
      args: {
        keeperId: "01KEEPER",
        reviewedModIds: ["01KEEPER", "01REJECTED"],
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
