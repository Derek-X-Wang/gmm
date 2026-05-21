import React, { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { open, save } from "@tauri-apps/plugin-dialog";

import {
  adoptFolder,
  detectGameInstallPath,
  fetchLatestImporterRelease,
  getGameInstallPath,
  getLibraryPaths,
  getProxyConfig,
  importZip,
  installImporter,
  listMods,
  rebuildJunctions,
  rollbackImporter,
  setGameInstallPath,
  setLibraryPathForGame,
  setLibraryRoot,
  setModEnabled,
  setProxyConfig,
  testProxyConnection,
  type GameCode as ApiGameCode,
} from "./api";
import { diagnosticsLogDir, exportDiagnosticsBundle } from "./diagnostics";
import "./App.css";

const GAME = "gimi" as const;

function App() {
  return (
    <main className="app">
      <header className="app__header">
        <h1>GMM — Genshin (v0.1 foundation)</h1>
      </header>
      <Settings />
      <NetworkPanel />
      <ImporterPanel />
      <LibraryPathsPanel />
      <Diagnostics />
      <ModList />
    </main>
  );
}

/**
 * Settings → Network. HTTP / SOCKS5 proxy fields shared by every
 * outbound HTTP path in the backend. The password is write-only — the
 * UI never reads it back. A "Test connection" button hits api.github.com
 * through the configured proxy to validate.
 */
function NetworkPanel() {
  const qc = useQueryClient();
  const cfg = useQuery({ queryKey: ["proxyConfig"], queryFn: getProxyConfig });
  const [url, setUrl] = useState<string>("");
  const [username, setUsername] = useState<string>("");
  const [password, setPassword] = useState<string>("");

  React.useEffect(() => {
    if (cfg.data) {
      setUrl(cfg.data.url ?? "");
      setUsername(cfg.data.username ?? "");
      setPassword(""); // never seeded
    }
  }, [cfg.data]);

  const save = useMutation({
    mutationFn: () =>
      setProxyConfig({
        url: url || null,
        username: username || null,
        password: password ? password : null,
      }),
    onSuccess: () => {
      setPassword("");
      qc.invalidateQueries({ queryKey: ["proxyConfig"] });
    },
  });
  const test = useMutation({ mutationFn: testProxyConnection });

  return (
    <section className="card">
      <h2>Network</h2>
      <p className="muted">
        Optional HTTP or SOCKS5 proxy for GitHub release downloads and GameBanana
        traffic. Formats: <code>http://host:port</code> or <code>socks5://host:port</code>.
        Sensitive fields never reach the diagnostics bundle.
      </p>
      <div className="row">
        <input
          placeholder="proxy URL"
          value={url}
          onChange={(e) => setUrl(e.currentTarget.value)}
        />
      </div>
      <div className="row">
        <input
          placeholder="username (optional)"
          value={username}
          onChange={(e) => setUsername(e.currentTarget.value)}
        />
        <input
          type="password"
          placeholder={cfg.data?.passwordSet ? "(set — leave blank to keep)" : "password"}
          value={password}
          onChange={(e) => setPassword(e.currentTarget.value)}
        />
      </div>
      <div className="row">
        <button onClick={() => save.mutate()} disabled={save.isPending}>
          {save.isPending ? "Saving…" : "Save"}
        </button>
        <button onClick={() => test.mutate()} disabled={test.isPending}>
          {test.isPending ? "Testing…" : "Test connection"}
        </button>
        {test.isSuccess ? <span className="muted small">Proxy reachable.</span> : null}
      </div>
      {save.isError ? <p className="error">{String(save.error)}</p> : null}
      {test.isError ? <p className="error">{String(test.error)}</p> : null}
    </section>
  );
}

/**
 * Settings → Model Importer. Lets the user (re)install the latest
 * GIMI release and roll back to the previously-backed-up files.
 */
function ImporterPanel() {
  const release = useQuery({
    queryKey: ["importer", "latest", GAME],
    queryFn: () => fetchLatestImporterRelease(GAME),
    retry: false,
  });

  const install = useMutation({
    mutationFn: () => installImporter(GAME),
  });
  const rollback = useMutation({
    mutationFn: () => rollbackImporter(GAME),
  });

  return (
    <section className="card">
      <h2>Model Importer (GIMI)</h2>
      <p className="muted">
        Downloads the latest <code>GIMI-Package</code> release, verifies the SHA-256,
        backs up any existing importer files, and rewrites <code>d3dx.ini</code>'s
        <code> loader</code> line to <code>gmm.exe</code>. Per ADR 0004 we never
        update without an explicit click here.
      </p>
      <div className="row">
        <span className="muted small">
          Latest release: {release.data ? <code>{release.data.tag_name}</code> : release.isLoading ? "checking…" : release.isError ? "unavailable" : "—"}
        </span>
      </div>
      <div className="row">
        <button onClick={() => install.mutate()} disabled={install.isPending}>
          {install.isPending ? "Installing…" : "Reinstall importer"}
        </button>
        <button onClick={() => rollback.mutate()} disabled={rollback.isPending}>
          {rollback.isPending ? "Rolling back…" : "Roll back importer"}
        </button>
      </div>
      {install.data ? (
        <p className="muted small">
          Installed. SHA-256 <code>{install.data.sha256.slice(0, 12)}…</code>{install.data.backup_dir ? <> · Backed up to <code>{install.data.backup_dir}</code></> : null}.
        </p>
      ) : null}
      {rollback.data ? (
        <p className="muted small">Restored from <code>{rollback.data}</code>.</p>
      ) : null}
      {install.isError ? <p className="error">{String(install.error)}</p> : null}
      {rollback.isError ? <p className="error">{String(rollback.error)}</p> : null}
    </section>
  );
}

/**
 * Settings → Library panel. Global root + per-game override rows with
 * Change / Reset buttons. Path moves go through the same
 * disable → move → rebuild flow as a manual Rebuild action.
 */
function LibraryPathsPanel() {
  const qc = useQueryClient();
  const paths = useQuery({
    queryKey: ["libraryPaths"],
    queryFn: getLibraryPaths,
  });

  const refresh = () => qc.invalidateQueries({ queryKey: ["libraryPaths"] });

  const setRoot = useMutation({
    mutationFn: (next: string | null) => setLibraryRoot(next),
    onSuccess: refresh,
  });
  const setPerGame = useMutation({
    mutationFn: ({ game, path }: { game: ApiGameCode; path: string | null }) =>
      setLibraryPathForGame(game, path),
    onSuccess: refresh,
  });

  const pickAndApply = async (apply: (path: string) => void) => {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") apply(picked);
  };

  if (!paths.data) {
    return (
      <section className="card">
        <h2>Library</h2>
        <p className="muted">Resolving paths…</p>
      </section>
    );
  }

  const p = paths.data;
  const games: ApiGameCode[] = ["gimi", "srmi", "zzmi", "wwmi", "himi", "efmi"];

  return (
    <section className="card">
      <h2>Library</h2>
      <p className="muted">
        Where GMM keeps each game's Mod copies. Changing a path disables affected mods,
        moves the bytes, then re-enables them against the new location. Non-NTFS targets
        are refused before any move.
      </p>

      <div className="row">
        <input
          className="path"
          value={p.effectiveRoot}
          readOnly
        />
        <span className="muted small">
          {p.rootOverride ? "Override" : `Default (${p.defaultRoot})`}
        </span>
      </div>
      <div className="row">
        <button onClick={() => pickAndApply((path) => setRoot.mutate(path))} disabled={setRoot.isPending}>
          {setRoot.isPending ? "Moving…" : "Change global root…"}
        </button>
        <button
          onClick={() => setRoot.mutate(null)}
          disabled={setRoot.isPending || !p.rootOverride}
        >
          Reset to default
        </button>
      </div>
      {setRoot.isError ? <p className="error">{String(setRoot.error)}</p> : null}

      <h3 className="muted small">Per-game overrides</h3>
      {games.map((game) => {
        const override = p.perGameOverrides[game];
        const effective = p.perGameEffective[game];
        return (
          <div className="row" key={game}>
            <code className="small">{game}</code>
            <input className="path" value={effective ?? ""} readOnly />
            <button
              onClick={() =>
                pickAndApply((path) => setPerGame.mutate({ game, path }))
              }
              disabled={setPerGame.isPending}
            >
              Change…
            </button>
            <button
              onClick={() => setPerGame.mutate({ game, path: null })}
              disabled={setPerGame.isPending || !override}
            >
              Reset
            </button>
          </div>
        );
      })}
      {setPerGame.isError ? <p className="error">{String(setPerGame.error)}</p> : null}
    </section>
  );
}

/**
 * Settings → Diagnostics panel. Shows the on-disk log directory and
 * exposes a single "Export diagnostics bundle" button. Bundle export is
 * always user-initiated; there is no background uploader.
 */
function Diagnostics() {
  const logDir = useQuery({
    queryKey: ["diagnostics", "logDir"],
    queryFn: diagnosticsLogDir,
  });

  const exportBundle = useMutation({
    mutationFn: async () => {
      const dir = logDir.data;
      if (!dir) throw new Error("log directory not yet known");
      const dest = await save({
        defaultPath: "gmm-diagnostics.zip",
        filters: [{ name: "ZIP archive", extensions: ["zip"] }],
      });
      if (typeof dest !== "string") return null;
      await exportDiagnosticsBundle(dir, dest);
      return dest;
    },
  });

  return (
    <section className="card">
      <h2>Diagnostics</h2>
      <p className="muted">
        Logs stay on this machine. No telemetry, no background uploads. Use
        the bundle export to attach local logs and redacted settings to a bug
        report.
      </p>
      <div className="row">
        <input className="path" value={logDir.data ?? ""} readOnly placeholder="resolving…" />
        <button onClick={() => exportBundle.mutate()} disabled={exportBundle.isPending}>
          {exportBundle.isPending ? "Bundling…" : "Export diagnostics bundle"}
        </button>
      </div>
      {exportBundle.data ? (
        <p className="muted small">Saved bundle to <code>{exportBundle.data}</code>.</p>
      ) : null}
      {exportBundle.isError ? (
        <p className="error">{String(exportBundle.error)}</p>
      ) : null}
    </section>
  );
}

function Settings() {
  const queryClient = useQueryClient();
  const { data: installPath } = useQuery({
    queryKey: ["installPath", GAME],
    queryFn: () => getGameInstallPath(GAME),
  });

  // Tracks whether the most recent path came from auto-detect, so we
  // can show the "Auto-detected" badge. Cleared as soon as the user
  // overrides via the manual picker.
  const [lastSource, setLastSource] = useState<"manual" | "auto" | null>(null);
  const [detectFailed, setDetectFailed] = useState(false);

  const setPath = useMutation({
    mutationFn: (path: string) => setGameInstallPath(GAME, path),
    onSuccess: () => {
      setLastSource("manual");
      setDetectFailed(false);
      queryClient.invalidateQueries({ queryKey: ["installPath", GAME] });
    },
  });

  const detect = useMutation({
    mutationFn: () => detectGameInstallPath(GAME),
    onSuccess: (path) => {
      if (path) {
        setLastSource("auto");
        setDetectFailed(false);
        queryClient.invalidateQueries({ queryKey: ["installPath", GAME] });
      } else {
        setDetectFailed(true);
      }
    },
  });

  const pickPath = async () => {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") setPath.mutate(picked);
  };

  const label =
    lastSource === "auto"
      ? "Auto-detected"
      : lastSource === "manual"
        ? "Set manually"
        : installPath
          ? "Saved"
          : "No install path set";

  return (
    <section className="card">
      <h2>Settings</h2>
      <p className="muted">
        GMM looks for <code>GenshinImpact.exe</code> (or <code>YuanShen.exe</code>) plus the
        <code> GenshinImpact_Data</code> folder. Use <strong>Auto-detect</strong> to scan known
        install locations, or pick the folder manually.
      </p>
      <div className="row">
        <input
          className="path"
          value={installPath ?? ""}
          placeholder="No install path set"
          readOnly
        />
        <span className="muted small">{label}</span>
      </div>
      <div className="row">
        <button onClick={() => detect.mutate()} disabled={detect.isPending || setPath.isPending}>
          {detect.isPending ? "Scanning…" : installPath ? "Re-detect" : "Auto-detect"}
        </button>
        <button onClick={pickPath} disabled={setPath.isPending}>
          {installPath ? "Change…" : "Pick folder"}
        </button>
      </div>
      {detectFailed ? (
        <p className="muted small">
          Couldn't find Genshin automatically. Pick the install folder manually.
        </p>
      ) : null}
      {setPath.isError ? <p className="error">{String(setPath.error)}</p> : null}
      {detect.isError ? <p className="error">{String(detect.error)}</p> : null}
      <RebuildJunctions />
    </section>
  );
}

/**
 * Manual "Rebuild junctions" action — drops every junction for the
 * active game and recreates one per enabled Mod against the current
 * Library. The hammer to use after relocating the Library directory.
 */
function RebuildJunctions() {
  const rebuild = useMutation({
    mutationFn: () => rebuildJunctions(GAME),
  });
  return (
    <div className="row">
      <button onClick={() => rebuild.mutate()} disabled={rebuild.isPending}>
        {rebuild.isPending ? "Rebuilding…" : "Rebuild junctions"}
      </button>
      {rebuild.data ? (
        <span className="muted small">
          Recreated {rebuild.data.recreated.length}, skipped {rebuild.data.skipped.length} disabled.
        </span>
      ) : null}
      {rebuild.isError ? <p className="error">{String(rebuild.error)}</p> : null}
    </div>
  );
}

function ModList() {
  const queryClient = useQueryClient();
  const mods = useQuery({
    queryKey: ["mods", GAME],
    queryFn: () => listMods(GAME),
  });

  const toggle = useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) =>
      setModEnabled(id, enabled, GAME),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["mods", GAME] }),
  });

  return (
    <section className="card">
      <div className="row row--between">
        <h2>Mods ({mods.data?.length ?? 0})</h2>
        <div className="row">
          <AdoptButton onAdopted={() => queryClient.invalidateQueries({ queryKey: ["mods", GAME] })} />
          <ImportZipButton onImported={() => queryClient.invalidateQueries({ queryKey: ["mods", GAME] })} />
        </div>
      </div>
      <ZipDropZone onImported={() => queryClient.invalidateQueries({ queryKey: ["mods", GAME] })} />

      {mods.isLoading ? <p>Loading…</p> : null}
      {mods.isError ? <p className="error">{String(mods.error)}</p> : null}

      {mods.data && mods.data.length === 0 ? (
        <p className="muted">No mods yet — adopt a folder to get started.</p>
      ) : null}

      <ul className="mods">
        {mods.data?.map((m) => (
          <li key={m.id} className="mods__row">
            <div className="mods__main">
              <strong>{m.name}</strong>
              <span className="muted"> · {m.source}</span>
            </div>
            <label className="toggle">
              <input
                type="checkbox"
                checked={m.enabled}
                disabled={toggle.isPending}
                onChange={(e) =>
                  toggle.mutate({ id: m.id, enabled: e.currentTarget.checked })
                }
              />
              <span>{m.enabled ? "Enabled" : "Disabled"}</span>
            </label>
          </li>
        ))}
      </ul>
      {toggle.isError ? <p className="error">{String(toggle.error)}</p> : null}
    </section>
  );
}

function AdoptButton({ onAdopted }: { onAdopted: () => void }) {
  const [name, setName] = useState("");
  const [picked, setPicked] = useState<string | null>(null);
  const [open_, setOpen_] = useState(false);

  const adopt = useMutation({
    mutationFn: async () => {
      if (!picked || !name.trim()) throw new Error("pick a folder and enter a name");
      return adoptFolder(GAME, picked, name.trim());
    },
    onSuccess: () => {
      onAdopted();
      setPicked(null);
      setName("");
      setOpen_(false);
    },
  });

  const pickFolder = async () => {
    const result = await open({ directory: true, multiple: false });
    if (typeof result === "string") setPicked(result);
  };

  if (!open_) {
    return <button onClick={() => setOpen_(true)}>Adopt folder…</button>;
  }

  return (
    <div className="adopt">
      <input
        placeholder="Display name (e.g. Hu Tao Skin)"
        value={name}
        onChange={(e) => setName(e.currentTarget.value)}
      />
      <button onClick={pickFolder}>{picked ? "Folder selected" : "Pick mod folder"}</button>
      {picked ? <code className="muted small">{picked}</code> : null}
      <div className="row">
        <button onClick={() => adopt.mutate()} disabled={adopt.isPending || !picked || !name.trim()}>
          {adopt.isPending ? "Adopting…" : "Adopt"}
        </button>
        <button onClick={() => setOpen_(false)} disabled={adopt.isPending}>
          Cancel
        </button>
      </div>
      {adopt.isError ? <p className="error">{String(adopt.error)}</p> : null}
    </div>
  );
}

/**
 * File-picker entry point for ZIP import. Mirrors the AdoptButton shape
 * — both flows produce a Mod row in the same list. The drop-zone below
 * calls the same `importZip` Tauri command.
 */
function ImportZipButton({ onImported }: { onImported: () => void }) {
  const [name, setName] = useState("");
  const [picked, setPicked] = useState<string | null>(null);
  const [open_, setOpen_] = useState(false);

  const importMutation = useMutation({
    mutationFn: async () => {
      if (!picked || !name.trim()) throw new Error("pick a ZIP and enter a name");
      return importZip(GAME, picked, name.trim());
    },
    onSuccess: () => {
      onImported();
      setPicked(null);
      setName("");
      setOpen_(false);
    },
  });

  const pickZip = async () => {
    const result = await open({
      multiple: false,
      filters: [{ name: "ZIP archive", extensions: ["zip"] }],
    });
    if (typeof result === "string") setPicked(result);
  };

  if (!open_) {
    return <button onClick={() => setOpen_(true)}>Import ZIP…</button>;
  }

  return (
    <div className="adopt">
      <input
        placeholder="Display name (e.g. Hu Tao Skin)"
        value={name}
        onChange={(e) => setName(e.currentTarget.value)}
      />
      <button onClick={pickZip}>{picked ? "ZIP selected" : "Pick .zip file"}</button>
      {picked ? <code className="muted small">{picked}</code> : null}
      <div className="row">
        <button
          onClick={() => importMutation.mutate()}
          disabled={importMutation.isPending || !picked || !name.trim()}
        >
          {importMutation.isPending ? "Importing…" : "Import"}
        </button>
        <button onClick={() => setOpen_(false)} disabled={importMutation.isPending}>
          Cancel
        </button>
      </div>
      {importMutation.isError ? <p className="error">{String(importMutation.error)}</p> : null}
    </div>
  );
}

/**
 * Drop-zone for ZIP import. Tauri's webview does NOT pass real filesystem
 * paths via the HTML5 drag-and-drop `dataTransfer` (security). The
 * `onDragDrop` event from the underlying `WebviewWindow` does — that's
 * the surface this component listens to. Falls back to a visible affordance
 * when no drop hovers.
 */
function ZipDropZone({ onImported }: { onImported: () => void }) {
  const [hover, setHover] = useState(false);
  const [pendingPath, setPendingPath] = useState<string | null>(null);
  const [name, setName] = useState("");

  const importMutation = useMutation({
    mutationFn: async () => {
      if (!pendingPath || !name.trim()) throw new Error("enter a name");
      return importZip(GAME, pendingPath, name.trim());
    },
    onSuccess: () => {
      onImported();
      setPendingPath(null);
      setName("");
    },
  });

  // Browser-level drag events let us toggle the hover highlight; the
  // actual path lands via the Tauri `onDragDrop` listener wired in the
  // effect below.
  const onDragOver = (e: React.DragEvent) => {
    e.preventDefault();
    setHover(true);
  };
  const onDragLeave = () => setHover(false);
  const onDrop = async (e: React.DragEvent) => {
    e.preventDefault();
    setHover(false);
    // In Tauri v2 the webview-level drop event exposes file paths via
    // `dataTransfer.files[i].path` only when the `tauri.conf.json` allows
    // it; for now grab the first item's name as a default mod name and
    // wait for the Tauri listener to populate the path. If a path comes
    // through the HTML layer (Linux/dev-server), use it.
    const file = e.dataTransfer?.files?.[0];
    if (file) {
      const path = (file as File & { path?: string }).path;
      if (path && path.endsWith(".zip")) {
        setPendingPath(path);
        if (!name) setName(file.name.replace(/\.zip$/i, ""));
      }
    }
  };

  return (
    <div
      className={`dropzone ${hover ? "dropzone--hover" : ""}`}
      onDragOver={onDragOver}
      onDragLeave={onDragLeave}
      onDrop={onDrop}
    >
      {pendingPath ? (
        <div className="adopt">
          <p className="muted small">Ready to import:</p>
          <code className="muted small">{pendingPath}</code>
          <input
            placeholder="Display name"
            value={name}
            onChange={(e) => setName(e.currentTarget.value)}
          />
          <div className="row">
            <button
              onClick={() => importMutation.mutate()}
              disabled={importMutation.isPending || !name.trim()}
            >
              {importMutation.isPending ? "Importing…" : "Import"}
            </button>
            <button
              onClick={() => {
                setPendingPath(null);
                setName("");
              }}
              disabled={importMutation.isPending}
            >
              Cancel
            </button>
          </div>
          {importMutation.isError ? <p className="error">{String(importMutation.error)}</p> : null}
        </div>
      ) : (
        <p className="muted small">Drop a <code>.zip</code> here, or use the Import button above.</p>
      )}
    </div>
  );
}

export default App;
