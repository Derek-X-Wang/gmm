import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { open, save } from "@tauri-apps/plugin-dialog";

import {
  adoptFolder,
  getGameInstallPath,
  importZip,
  listMods,
  setGameInstallPath,
  setModEnabled,
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
      <Diagnostics />
      <ModList />
    </main>
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

  const setPath = useMutation({
    mutationFn: (path: string) => setGameInstallPath(GAME, path),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["installPath", GAME] }),
  });

  const pickPath = async () => {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") setPath.mutate(picked);
  };

  return (
    <section className="card">
      <h2>Settings</h2>
      <p className="muted">Pick the directory that contains <code>GenshinImpact.exe</code> (or <code>YuanShen.exe</code>).</p>
      <div className="row">
        <input
          className="path"
          value={installPath ?? ""}
          placeholder="No install path set"
          readOnly
        />
        <button onClick={pickPath} disabled={setPath.isPending}>
          {setPath.isPending ? "Saving…" : "Pick folder"}
        </button>
      </div>
      {setPath.isError ? <p className="error">{String(setPath.error)}</p> : null}
    </section>
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
