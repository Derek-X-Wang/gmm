import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { open } from "@tauri-apps/plugin-dialog";

import {
  adoptFolder,
  getGameInstallPath,
  listMods,
  setGameInstallPath,
  setModEnabled,
} from "./api";
import "./App.css";

const GAME = "gimi" as const;

function App() {
  return (
    <main className="app">
      <header className="app__header">
        <h1>GMM — Genshin (v0.1 foundation)</h1>
      </header>
      <Settings />
      <ModList />
    </main>
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
        <AdoptButton onAdopted={() => queryClient.invalidateQueries({ queryKey: ["mods", GAME] })} />
      </div>

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

export default App;
