import { useId, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listPresets, savePreset, type GameCode } from "./api";
import { CommandErrorNotice } from "./CommandErrorNotice";

export function GamePresets({ game }: { game: GameCode }) {
  const qc = useQueryClient();
  const nameId = useId();
  const [name, setName] = useState("");
  const presets = useQuery({ queryKey: ["presets", game], queryFn: () => listPresets(game) });
  const save = useMutation({
    mutationFn: (draft: string) => savePreset(game, draft),
    onSuccess: () => {
      setName("");
      void qc.invalidateQueries({ queryKey: ["presets", game] });
    },
  });
  return <section className="card" aria-label="Saved presets">
    <h2>Saved presets</h2>
    <form onSubmit={(event) => { event.preventDefault(); save.mutate(name); }}>
      <label htmlFor={nameId}>Preset name</label>
      <input id={nameId} value={name} onChange={(event) => setName(event.target.value)} disabled={save.isPending} />
      <button type="submit" disabled={save.isPending || !name.trim()}>
        {save.isPending ? "Saving preset…" : "Save current setup as…"}
      </button>
    </form>
    <CommandErrorNotice error={save.error} heading="Could not save preset." />
    {save.isSuccess && <p role="status">Saved preset “{save.data.name}”.</p>}
    <CommandErrorNotice error={presets.error} heading="Could not list presets." />
    {presets.isPending && <p>Loading presets…</p>}
    {presets.isSuccess && presets.data.length === 0 && <p>No saved presets for this Game.</p>}
    {presets.data && <ul>{presets.data.map((preset) => <li key={preset.id}>
      {preset.name} — {preset.modCount === 0 ? "No Mods enabled" : `${preset.modCount} Mods enabled`}
    </li>)}</ul>}
  </section>;
}
