import { useId, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { applyPreset, listPresets, savePreset, type GameCode, type PresetApplyMod, type PresetApplyProgress, type PresetApplyReport, type PresetApplyResult } from "./api";
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
  return <section className="card" aria-label="Saved presets">
    <h2>Saved presets</h2>
    <form onSubmit={(event) => { event.preventDefault(); save.mutate(name); }}>
      <label htmlFor={nameId}>Preset name</label>
      <input id={nameId} value={name} onChange={(event) => setName(event.target.value)} disabled={save.isPending || apply.isPending} />
      <button type="submit" disabled={save.isPending || apply.isPending || !name.trim()}>
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
      {" "}<button disabled={apply.isPending || save.isPending} onClick={() => apply.mutate(preset.id)}>Apply {preset.name}</button>
    </li>)}</ul>}
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
