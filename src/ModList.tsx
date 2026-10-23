import React, { useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { open } from "@tauri-apps/plugin-dialog";
import {
  adoptFolder,
  applyModUpdate,
  detectConflicts,
  importGamebanana,
  importZip,
  listMods,
  listModUpdates,
  listVariants,
  retryReinstallRecovery,
  retireInterruptedEnabledTransition,
  setActiveVariant,
  setModEnabled,
  setModUpdateCheckEnabled,
  type Conflict,
  type GameCode as ApiGameCode,
} from "./api";
import { SessionCheckNotice, useSessionCheck } from "./SessionCheckNotice";
import { useRecoveryOutcomes } from "./RecoveryOutcome";
import { EnabledTransitionRecoveryWarning } from "./EnabledTransitionRecoveryWarning";
import {
  ReinstallRecoveryNotices,
  ReinstallRecoveryWarning,
  type ReinstallRecoveryFeedback,
} from "./ReinstallRecoveryWarning";
import { CommandErrorNotice } from "./CommandErrorNotice";
import { commandFailureMessage } from "./commandError";
import { ModDetailPanel } from "./ModDetailPanel";

/**
 * Per-mod badge that surfaces an available upstream version + Apply
 * + per-mod opt-out toggle. Renders nothing for mods without a
 * gamebanana_id.
 */
function ModUpdateBadge({
  modId,
  game,
  disabled = false,
}: {
  modId: string;
  game: ApiGameCode;
  disabled?: boolean;
}) {
  const qc = useQueryClient();
  const rows = useQuery({
    queryKey: ["modUpdates", game],
    queryFn: () => listModUpdates(game),
  });
  const apply = useMutation({
    mutationFn: () => applyModUpdate(modId),
    onSettled: () => {
      qc.invalidateQueries({ queryKey: ["mods", game] });
      qc.invalidateQueries({ queryKey: ["modUpdates", game] });
      qc.invalidateQueries({ queryKey: ["variants", modId] });
      qc.invalidateQueries({ queryKey: ["conflicts", game] });
    },
  });
  const toggle = useMutation({
    mutationFn: (enabled: boolean) => setModUpdateCheckEnabled(modId, enabled),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["modUpdates", game] }),
  });

  const row = rows.data?.find((r) => r.modId === modId);
  if (!row) return <CommandErrorNotice error={apply.error} />;
  return (
    <>
      <span className="conflict-inline">
        {row.upstreamAhead ? (
          <button
            className="update-pill"
            onClick={() => apply.mutate()}
            disabled={disabled || apply.isPending}
            title={`Upstream ${row.upstreamVersion}`}
          >
            {apply.isPending ? "Applying…" : `Update → ${row.upstreamVersion}`}
          </button>
        ) : null}
        <label className="toggle" style={{ marginLeft: "0.4rem" }}>
          <input
            type="checkbox"
            checked={row.updateCheckEnabled}
            disabled={disabled || toggle.isPending}
            onChange={(e) => toggle.mutate(e.currentTarget.checked)}
          />
          <span className="muted small">check</span>
        </label>
      </span>
      <CommandErrorNotice error={apply.error} />
    </>
  );
}

export function ModList({ game, displayName }: { game: ApiGameCode; displayName: string }) {
  const queryClient = useQueryClient();
  const sectionRef = useRef<HTMLElement>(null);
  const [recoveryFeedback, setRecoveryFeedback] =
    useState<ReinstallRecoveryFeedback>(null);
  useEffect(() => setRecoveryFeedback(null), [game]);
  const mods = useQuery({
    queryKey: ["mods", game],
    queryFn: () => listMods(game),
  });
  const [selection, setSelection] = useState<{ game: ApiGameCode; id: string } | null>(null);
  // Keep identity through refreshes, but never resurrect a removed Mod or a
  // previous Game's selection. Unknown query data is not an empty Library.
  useEffect(() => {
    setSelection((previous) => previous && (previous.game !== game
      || (mods.data !== undefined && !mods.data.some((mod) => mod.id === previous.id)))
      ? null : previous);
  }, [game, mods.data]);
  const selectedId = selection?.game === game
    && mods.data?.some((mod) => mod.id === selection.id) ? selection.id : null;

  const renderedMods = mods.data ?? [];
  const rowRefs = useRef(new Map<string, HTMLLIElement>());
  const [inspectorOpen, setInspectorOpen] = useState(false);
  const [focusInspector, setFocusInspector] = useState(false);
  const inspectorId = React.useId();
  const selectionHintId = React.useId();
  const selectRow = (id: string) => {
    setSelection({ game, id });
    setFocusInspector(false);
    setInspectorOpen(true);
  };
  const navigateRows = (event: React.KeyboardEvent<HTMLLIElement>, id: string) => {
    // Inner controls keep their native keys, including Variant radio arrows.
    if (event.target !== event.currentTarget || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      selectRow(id);
      return;
    }
    const index = renderedMods.findIndex((mod) => mod.id === id);
    if (index < 0) return;
    const next = event.key === "ArrowDown" ? Math.min(index + 1, renderedMods.length - 1)
      : event.key === "ArrowUp" ? Math.max(index - 1, 0)
      : event.key === "Home" ? 0
      : event.key === "End" ? renderedMods.length - 1 : null;
    if (next === null) return;
    event.preventDefault();
    const nextId = renderedMods[next].id;
    selectRow(nextId);
    rowRefs.current.get(nextId)?.focus();
  };

  const conflicts = useQuery({
    queryKey: ["conflicts", game],
    queryFn: () => detectConflicts(game),
  });
  const check = useSessionCheck();
  const { session } = check;
  const sessionBlocked = !session.isSuccess || !!session.data;

  const toggle = useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) =>
      setModEnabled(id, enabled, game),
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ["mods", game] });
      queryClient.invalidateQueries({ queryKey: ["conflicts", game] });
    },
  });

  const retryRecovery = useMutation({
    mutationFn: ({ id }: { id: string; name: string }) => retryReinstallRecovery(id),
    onSuccess: (outcome, mod) => {
      // The callout/button can disappear after the refetch. Move focus to the
      // stable section, while the separately mounted live region announces.
      sectionRef.current?.focus();
      setRecoveryFeedback(
        outcome.status === "recovered" || outcome.status === "alreadyRecovered"
          ? {
              kind: outcome.status,
              modName: mod.name,
            }
          : {
              kind: "stillQuarantined",
              modName: mod.name,
              reason: outcome.recovery.reason,
            },
      );
    },
    onError: (error, mod) => {
      sectionRef.current?.focus();
      setRecoveryFeedback({
        kind: "stillQuarantined",
        modName: mod.name,
        reason: commandFailureMessage(error),
      });
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ["mods", game] });
      queryClient.invalidateQueries({ queryKey: ["conflicts", game] });
    },
  });

  const retirement = useRecoveryOutcomes();
  const retireEnabledTransition = useMutation({
    mutationFn: ({ id }: { id: string; name: string; game: ApiGameCode }) =>
      retireInterruptedEnabledTransition(id),
    onMutate: ({ name, game }) => retirement.start(game, `Retiring the transition producer for ${name}…`),
    onSuccess: (_, { name, game }) => retirement.finish(game,
      `Transition producer retired or already absent for ${name}. GMM did not verify process identity; refreshed Mod status shows whether deployment recovery is still pending.`),
    onError: (error, { game }) => retirement.finish(game,
      `Could not retire the transition producer: ${commandFailureMessage(error)}`, true),
    onSettled: (_, __, { game }) => {
      queryClient.invalidateQueries({ queryKey: ["mods", game] });
      queryClient.invalidateQueries({ queryKey: ["conflicts", game] });
    },
  });

  const confirmedEmpty = mods.isSuccess && mods.data.length === 0;

  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ["mods", game] });
    queryClient.invalidateQueries({ queryKey: ["libraryAudit", game] });
    queryClient.invalidateQueries({ queryKey: ["modUpdates", game] });
  };

  return (
    <section ref={sectionRef} className="card" tabIndex={-1}>
      <ReinstallRecoveryNotices feedback={recoveryFeedback} />
      {retirement.region(game, `${displayName} deployment recovery outcome`)}
      <div className={confirmedEmpty ? "mod-import-landing" : "row row--between"}>
        <h2>
          {confirmedEmpty ? `Add Mods for ${displayName}` : mods.data === undefined ? "Mods" : `Mods (${mods.data.length})`}
          {conflicts.data && conflicts.data.conflicts.length > 0 ? (
            <span className="conflict-badge"> · {conflicts.data.conflicts.length} conflict{conflicts.data.conflicts.length === 1 ? "" : "s"}</span>
          ) : null}
        </h2>
        {confirmedEmpty ? <p className="muted">Your Library has no Mods for {displayName}. Choose how to add your first Mod.</p> : null}
        <div className={confirmedEmpty ? "mod-import-routes" : "row"}>
          {/* Discard each import draft on a Game change, keeping ModList's own lifetime. */}
          <div className="mod-import-route">
            {confirmedEmpty ? <h3>Adopt for {displayName}</h3> : null}
            {confirmedEmpty ? <p className="muted small">Bring an existing Mod folder into your Library.</p> : null}
            <AdoptButton key={`adopt:${game}`} game={game} onAdopted={invalidate} />
          </div>
          <div className="mod-import-route">
            {confirmedEmpty ? <h3>Import ZIP for {displayName}</h3> : null}
            {confirmedEmpty ? <p className="muted small">Choose a downloaded Mod archive.</p> : null}
            <ImportZipButton key={`zip:${game}`} game={game} onImported={invalidate} />
          </div>
          <div className="mod-import-route">
            {confirmedEmpty ? <h3>GameBanana for {displayName}</h3> : null}
            {confirmedEmpty ? <p className="muted small">Paste a Mod URL or submission ID.</p> : null}
            <GameBananaImport key={`gamebanana:${game}`} game={game} onImported={invalidate} />
          </div>
        </div>
      </div>
      <ZipDropZone key={`drop:${game}`} game={game} displayName={displayName} onImported={invalidate} />

      {mods.isLoading ? <p>Loading…</p> : null}
      <CommandErrorNotice error={mods.error} />
      <CommandErrorNotice
        error={conflicts.error}
        heading="GMM could not check this game's Mod conflicts."
        detail="Conflict information is unavailable; this is not a “no conflicts” result."
      />

      <SessionCheckNotice check={check} />
      <div className="mod-workspace">
      <div className="mod-workspace__list">
      <p id={selectionHintId} className="muted small">
        Select a Mod to show its details. Use Up and Down to move between Mods, or Enter or Space to select.
      </p>
      {renderedMods.length > 0 ? <ul className="mods" aria-describedby={selectionHintId}>
        {renderedMods.map((m) => (
          <li
            key={m.id}
            data-mod-id={m.id}
            ref={(row) => {
              if (row) rowRefs.current.set(m.id, row);
              else rowRefs.current.delete(m.id);
            }}
            aria-label={m.name}
            aria-current={selectedId === m.id ? "true" : undefined}
            aria-controls={inspectorId}
            tabIndex={0}
            onClick={(event) => {
              selectRow(m.id);
              // A background click focuses the row; controls retain focus and
              // their ordinary action when the click bubbles through the row.
              if (!(event.target as HTMLElement).closest("a, button, input, label, select, textarea, summary")) {
                event.currentTarget.focus();
              }
            }}
            onKeyDown={(event) => navigateRows(event, m.id)}
            className={`mods__row${selectedId === m.id ? " mods__row--selected" : ""}${m.reinstallRecovery || m.enabledTransitionRecovery ? " mods__row--quarantined" : ""}`}
          >
            <div className="mods__main">
              <strong>{m.name}</strong>
              <span className="muted"> · {m.source}</span>
              {m.source === "gamebanana" && m.sourceUrl ? (
                <>
                  {" · "}
                  <a href={m.sourceUrl} target="_blank" rel="noreferrer" className="muted small">
                    View on GameBanana
                  </a>
                  {m.version ? <span className="muted small"> · v{m.version}</span> : null}
                  <ModUpdateBadge
                    modId={m.id}
                    game={game}
                    disabled={
                      m.reinstallRecovery !== null || m.enabledTransitionRecovery !== null
                    }
                  />
                </>
              ) : null}
              <ConflictBadge
                modId={m.id}
                conflicts={conflicts.data?.conflicts ?? []}
                count={conflicts.data?.per_mod_count[m.id] ?? 0}
              />
              <VariantSelector
                modId={m.id}
                game={game}
                disabled={
                  m.reinstallRecovery !== null || m.enabledTransitionRecovery !== null
                }
              />
              {m.reinstallRecovery ? (
                <ReinstallRecoveryWarning
                  modName={m.name}
                  recovery={m.reinstallRecovery}
                  pending={retryRecovery.isPending}
                  disabled={sessionBlocked}
                  onRetry={() => retryRecovery.mutate({ id: m.id, name: m.name })}
                />
              ) : null}
              {m.enabledTransitionRecovery ? (
                <EnabledTransitionRecoveryWarning
                  modName={m.name}
                  recovery={m.enabledTransitionRecovery}
                  pending={
                    retireEnabledTransition.isPending &&
                    retireEnabledTransition.variables?.id === m.id
                  }
                  onRetire={() =>
                    retireEnabledTransition.mutate({ id: m.id, name: m.name, game })
                  }
                />
              ) : null}
            </div>
            <div>
              <label className="toggle">
                <input
                  type="checkbox"
                  checked={m.enabled}
                  disabled={
                    toggle.isPending ||
                    sessionBlocked ||
                    m.reinstallRecovery !== null ||
                    m.enabledTransitionRecovery !== null
                  }
                  onChange={(e) =>
                    toggle.mutate({ id: m.id, enabled: e.currentTarget.checked })
                  }
                />
                <span>
                  {m.enabledTransitionRecovery
                    ? "Unavailable · deployment recovery pending"
                    : m.reinstallRecovery
                    ? m.reinstallRecovery.junctionWithdrawn
                      ? "Unavailable"
                      : "Unavailable · may still be loading"
                    : m.enabled
                      ? "Enabled"
                      : "Disabled"}
                </span>
              </label>
            </div>
          </li>
        ))}
      </ul> : null}
      <CommandErrorNotice error={toggle.error} />
      </div>
      <ModDetailPanel
        id={inspectorId}
        open={inspectorOpen}
        focusOnOpen={focusInspector}
        mod={renderedMods.find((mod) => mod.id === selectedId) ?? null}
        onClose={() => {
          setInspectorOpen(false);
          (selectedId ? rowRefs.current.get(selectedId) : sectionRef.current)?.focus();
        }}
      />
      </div>
      {!inspectorOpen && renderedMods.length > 0 ? <button type="button" className="mod-details-open" aria-controls={inspectorId}
        onClick={() => { setFocusInspector(true); setInspectorOpen(true); }}>Show Mod details</button> : null}
    </section>
  );
}

function AdoptButton({
  game,
  onAdopted,
}: {
  game: ApiGameCode;
  onAdopted: () => void;
}) {
  const [name, setName] = useState("");
  const [picked, setPicked] = useState<string | null>(null);
  const [open_, setOpen_] = useState(false);

  const adopt = useMutation({
    mutationFn: async () => {
      if (!picked || !name.trim()) throw new Error("pick a folder and enter a name");
      return adoptFolder(game, picked, name.trim());
    },
    onSuccess: () => {
      setPicked(null);
      setName("");
      setOpen_(false);
    },
    onSettled: onAdopted,
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
      <CommandErrorNotice error={adopt.error} />
    </div>
  );
}

/**
 * GameBanana URL paste form. User pastes a mod URL or bare submission
 * ID; backend resolves it via apiv11, downloads the first .zip, and
 * routes through the slice-1b ingest path. Network goes through the
 * configured proxy (slice 14).
 */
function GameBananaImport({
  game,
  onImported,
}: {
  game: ApiGameCode;
  onImported: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [input, setInput] = useState("");
  const ingest = useMutation({
    mutationFn: () => importGamebanana(game, input.trim()),
    onSuccess: () => {
      setInput("");
      setOpen(false);
    },
    onSettled: onImported,
  });

  if (!open) {
    return <button onClick={() => setOpen(true)}>Paste GameBanana URL…</button>;
  }
  return (
    <div className="adopt">
      <input
        placeholder="https://gamebanana.com/mods/1234567 or a bare ID"
        value={input}
        onChange={(e) => setInput(e.currentTarget.value)}
      />
      <div className="row">
        <button
          onClick={() => ingest.mutate()}
          disabled={ingest.isPending || !input.trim()}
        >
          {ingest.isPending ? "Resolving…" : "Import"}
        </button>
        <button onClick={() => setOpen(false)} disabled={ingest.isPending}>
          Cancel
        </button>
      </div>
      <CommandErrorNotice error={ingest.error} />
    </div>
  );
}

/**
 * Per-mod conflict count + click-to-expand list of the conflicting
 * hashes with which other Mods share them. v1 just shows the
 * information — priority-order resolution (MO2-style) is v1.1.
 */
function ConflictBadge({
  modId,
  conflicts,
  count,
}: {
  modId: string;
  conflicts: Conflict[];
  count: number;
}) {
  const [open, setOpen] = useState(false);
  if (!count) return null;
  const mine = conflicts.filter((c) => c.mod_ids.includes(modId));
  return (
    <span className="conflict-inline">
      <button
        className="conflict-pill"
        onClick={() => setOpen((v) => !v)}
        title="Click for details"
      >
        {count} conflict{count === 1 ? "" : "s"}
      </button>
      {open ? (
        <div className="conflict-detail">
          {mine.map((c) => {
            const others = c.mod_ids.filter((m) => m !== modId);
            return (
              <p key={c.hash} className="muted small">
                <code>{c.hash}</code> shared with {others.length} other mod{others.length === 1 ? "" : "s"}.
              </p>
            );
          })}
        </div>
      ) : null}
    </span>
  );
}

/**
 * Inline radio list of Variants for a Mod. Renders nothing for Mods
 * with 0 or 1 Variants — the AC explicitly says single-folder mods
 * see no UI change. Switching the active Variant retargets the
 * junction in place; no full toggle cycle needed.
 */
function VariantSelector({
  modId,
  game,
  disabled = false,
}: {
  modId: string;
  game: ApiGameCode;
  disabled?: boolean;
}) {
  const qc = useQueryClient();
  const v = useQuery({
    queryKey: ["variants", modId],
    queryFn: () => listVariants(modId),
  });
  const switchVariant = useMutation({
    mutationFn: (variantId: string) => setActiveVariant(modId, variantId, game),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["variants", modId] });
      qc.invalidateQueries({ queryKey: ["conflicts", game] });
    },
  });

  if (!v.data || v.data.variants.length < 2) return null;

  return (
    <div className="variants">
      {v.data.variants.map((variant) => {
        const active = variant.id === v.data!.activeVariantId;
        return (
          <label key={variant.id} className="variants__row">
            <input
              type="radio"
              name={`variant-${modId}`}
              checked={active}
              disabled={disabled || switchVariant.isPending || v.isFetching}
              onChange={() => switchVariant.mutate(variant.id)}
            />
            <span>{variant.name}</span>
          </label>
        );
      })}
      <CommandErrorNotice error={switchVariant.error} />
    </div>
  );
}

/**
 * File-picker entry point for ZIP import. Mirrors the AdoptButton shape
 * — both flows produce a Mod row in the same list. The drop-zone below
 * calls the same `importZip` Tauri command.
 */
function ImportZipButton({
  game,
  onImported,
}: {
  game: ApiGameCode;
  onImported: () => void;
}) {
  const [name, setName] = useState("");
  const [picked, setPicked] = useState<string | null>(null);
  const [open_, setOpen_] = useState(false);

  const importMutation = useMutation({
    mutationFn: async () => {
      if (!picked || !name.trim()) throw new Error("pick a ZIP and enter a name");
      return importZip(game, picked, name.trim());
    },
    onSuccess: () => {
      setPicked(null);
      setName("");
      setOpen_(false);
    },
    onSettled: onImported,
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
      <CommandErrorNotice error={importMutation.error} />
    </div>
  );
}

/**
 * Drop-zone for ZIP import when HTML drag-and-drop exposes a filesystem path.
 * Webviews that omit that path can use the Import ZIP picker above.
 */
function ZipDropZone({
  game,
  displayName,
  onImported,
}: {
  game: ApiGameCode;
  displayName: string;
  onImported: () => void;
}) {
  const [hover, setHover] = useState(false);
  const [pendingPath, setPendingPath] = useState<string | null>(null);
  const [name, setName] = useState("");

  const importMutation = useMutation({
    mutationFn: async () => {
      if (!pendingPath || !name.trim()) throw new Error("enter a name");
      return importZip(game, pendingPath, name.trim());
    },
    onSuccess: () => {
      setPendingPath(null);
      setName("");
    },
    onSettled: onImported,
  });

  // Browser-level drag events toggle the hover highlight.
  const onDragOver = (e: React.DragEvent) => {
    e.preventDefault();
    setHover(true);
  };
  const onDragLeave = () => setHover(false);
  const onDrop = async (e: React.DragEvent) => {
    e.preventDefault();
    setHover(false);
    // Accept a ZIP only when the HTML layer supplies its filesystem path.
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
          <p className="muted small">Ready to import for {displayName}:</p>
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
          <CommandErrorNotice error={importMutation.error} />
        </div>
      ) : (
        <p className="muted small">Drop a <code>.zip</code> for {displayName} here, or use the Import button above.</p>
      )}
    </div>
  );
}
