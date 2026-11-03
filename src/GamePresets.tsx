import { useId, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { applyPreset, deletePreset, listMods, listPresets, recapturePreset, renamePreset, savePreset, type Preset, type GameCode, type PresetApplyMod, type PresetApplyProgress, type PresetApplyReport, type PresetApplyResult } from "./api";
import { CommandErrorNotice } from "./CommandErrorNotice";
import { commandFailureMessage } from "./commandError";

type ApplyState = {
  mods: PresetApplyMod[];
  results: PresetApplyResult[];
  reading: string;
  report?: PresetApplyReport;
  error?: string;
};

export function GamePresets({ game }: { game: GameCode }) {
  const qc = useQueryClient();
  const nameId = useId();
  const [name, setName] = useState("");
  const [action, setAction] = useState<{ kind: "rename" | "delete" | "recapture"; preset: Preset }>();
  const [renameDraft, setRenameDraft] = useState("");
  const renameId = useId();
  const recapturing = action?.kind === "recapture";
  const currentMods = useQuery({ queryKey: ["mods", game], queryFn: () => listMods(game), enabled: recapturing });
  const rename = useMutation({
    mutationFn: ({ id, name }: { id: string; name: string }) => renamePreset(game, id, name),
    onSuccess: () => { setAction(undefined); void qc.invalidateQueries({ queryKey: ["presets", game] }); },
  });
  const remove = useMutation({
    mutationFn: (id: string) => deletePreset(game, id),
    onSuccess: () => { setAction(undefined); void qc.invalidateQueries({ queryKey: ["presets", game] }); },
  });
  const recapture = useMutation({
    mutationFn: (id: string) => recapturePreset(game, id),
    onSuccess: () => { setAction(undefined); void qc.invalidateQueries({ queryKey: ["presets", game] }); },
  });
  const [applyState, setApplyState] = useState<ApplyState>();
  const presets = useQuery({ queryKey: ["presets", game], queryFn: () => listPresets(game) });
  const save = useMutation({
    mutationFn: (draft: string) => savePreset(game, draft),
    onSuccess: () => {
      setName("");
      void qc.invalidateQueries({ queryKey: ["presets", game] });
    },
  });
  const apply = useMutation({
    scope: { id: `conflicts-${game}` },
    mutationFn: (presetId: string) => applyPreset(game, presetId, (event: PresetApplyProgress) => {
      setApplyState((previous) => {
        const old = previous ?? { mods: [], results: [], reading: "Applying preset…" };
        switch (event.phase) {
          case "started": return { ...old, mods: event.mods };
          case "applying": return { ...old, reading: `Applying ${event.item.name}…` };
          case "completed": return { ...old, results: [...old.results, event.result] };
          case "checkingConflicts": return { ...old, reading: "Checking Conflicts…" };
        }
      });
    }),
    onMutate: () => setApplyState({ mods: [], results: [], reading: "Starting preset…" }),
    onSuccess: (report) => setApplyState((old) => ({ mods: old?.mods ?? [], results: report.results, report, reading: "Preset apply finished. Review each Mod's result below." })),
    onError: (error) => setApplyState((old) => ({
      mods: old?.mods ?? [], reading: "Preset result unavailable.", error: commandFailureMessage(error),
      results: (old?.mods ?? []).map((item) => old?.results.find((result) => result.modId === item.modId) ?? {
        ...item, status: "unknown", reason: `${item.name}: no result received. Check the Library and any recovery warnings before retrying.`,
      }),
    })),
    onSettled: () => {
      for (const key of ["mods", "conflicts", "modUpdates"]) {
        void qc.invalidateQueries({ queryKey: [key, game] });
      }
      for (const key of ["variants", "attentionStatus", "libraryAudit"]) {
        void qc.invalidateQueries({ queryKey: [key] });
      }
    },
  });
  const busy = save.isPending || apply.isPending || rename.isPending || remove.isPending || recapture.isPending;
  function choose(kind: "rename" | "delete" | "recapture", preset: Preset) {
    rename.reset(); remove.reset(); recapture.reset();
    setRenameDraft(preset.name);
    setAction({ kind, preset });
  }
  return <section className="card" aria-label="Saved presets">
    <h2>Saved presets</h2>
    <form onSubmit={(event) => { event.preventDefault(); save.mutate(name); }}>
      <label htmlFor={nameId}>Preset name</label>
      <input id={nameId} value={name} onChange={(event) => setName(event.target.value)} disabled={busy} />
      <button type="submit" disabled={busy || !name.trim()}>
        {save.isPending ? "Saving preset…" : "Save current setup as…"}
      </button>
    </form>
    <CommandErrorNotice error={save.error} heading="Could not save preset." />
    {save.isSuccess && <p role="status">Saved preset “{save.data.name}”.</p>}
    <CommandErrorNotice error={presets.error} heading="Could not list presets." />
    {presets.isPending && <p>Loading presets…</p>}
    {presets.isSuccess && presets.data.length === 0 && <p>No saved presets for this Game.</p>}
    {!!presets.data?.length && <p>Applying a preset enables its saved Mods and disables every other Mod for this Game, including Mods added after it was saved. Keep GMM open until it finishes.</p>}
    {presets.data && <ul>{presets.data.map((preset) => <li key={preset.id}>
      {preset.name} — {preset.modCount === 0 ? "No Mods enabled" : `${preset.modCount} Mods enabled`}
      {" "}<button disabled={busy} onClick={() => apply.mutate(preset.id)}>Apply {preset.name}</button>
      {" "}<button disabled={busy} onClick={() => choose("rename", preset)}>Rename {preset.name}</button>
      {" "}<button disabled={busy} onClick={() => choose("recapture", preset)}>Re-capture current setup into {preset.name}</button>
      {" "}<button disabled={busy} onClick={() => choose("delete", preset)}>Delete saved preset {preset.name}</button>
    </li>)}</ul>}
    <CommandErrorNotice error={rename.error} heading="Could not rename preset." />
    <CommandErrorNotice error={remove.error} heading="Could not delete saved preset." />
    <CommandErrorNotice error={recapture.error} heading="Could not replace saved setup." />
    {action?.kind === "rename" && <form onSubmit={(event) => { event.preventDefault(); rename.mutate({ id: action.preset.id, name: renameDraft }); }}>
      <label htmlFor={renameId}>New preset name</label>
      <input id={renameId} value={renameDraft} onChange={(event) => setRenameDraft(event.target.value)} disabled={busy} />
      <button type="submit" disabled={busy || !renameDraft.trim()}>Save new name</button>
      <button type="button" disabled={busy} onClick={() => setAction(undefined)}>Cancel rename</button>
    </form>}
    {action?.kind === "delete" && <div role="alertdialog" aria-label="Delete saved preset">
      <p>Delete saved preset “{action.preset.name}”? Only the saved description is removed. No Mod is turned off and no Mod is changed.</p>
      <button disabled={busy} onClick={() => remove.mutate(action.preset.id)}>Delete saved description</button>
      <button disabled={busy} onClick={() => setAction(undefined)}>Keep saved preset</button>
    </div>}
    {recapturing && <div role="alertdialog" aria-label="Replace saved setup">
      <CommandErrorNotice error={currentMods.error} heading="Could not read current setup." />
      {currentMods.isFetching && <p>Reading current setup…</p>}
      {currentMods.isSuccess && <p>Replace saved setup “{action.preset.name}”? It holds {action.preset.modCount} Mods now and will hold {currentMods.data.filter((mod) => mod.enabled).length} Mods after re-capture. The previous saved entries cannot be recovered. The name stays the same. No Mod is changed.</p>}
      <button disabled={busy || !currentMods.isSuccess || currentMods.isFetching} onClick={() => recapture.mutate(action.preset.id)}>Replace saved entries with current setup</button>
      <button disabled={busy} onClick={() => setAction(undefined)}>Keep previous saved entries</button>
    </div>}
    {applyState && <div>
      <p role="status">{applyState.reading} {apply.isPending && `${applyState.results.length} of ${applyState.mods.length} Mods completed.`}</p>
      {applyState.error && <p role="alert">{applyState.error}</p>}
      <ul>{applyState.results.map((result) => <li key={result.modId}>
        {result.name}: {result.status} — {result.reason}
      </li>)}</ul>
      {applyState.report?.conflictError && <p role="alert">Conflicts unverified: {applyState.report.conflictError}</p>}
      {applyState.report?.conflicts && <div>
        <p>{applyState.report.conflicts.conflicts.length} {applyState.report.conflictError ? "known Conflicts; the full reading is unverified." : "Conflicts found after applying the preset."}</p>
        <ul>{applyState.report.conflicts.conflicts.map((conflict) => <li key={conflict.hash}>
          Hash {conflict.hash}: {conflict.mod_ids.map((id) => applyState.results.find((result) => result.modId === id)?.name ?? id).join(", ")}
        </li>)}</ul>
      </div>}
    </div>}
  </section>;
}
