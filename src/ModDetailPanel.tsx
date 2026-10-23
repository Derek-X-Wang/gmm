import { useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { getModPanelWidth, listModUpdates, setModPanelWidth, type Mod } from "./api";
import { CommandErrorNotice } from "./CommandErrorNotice";

const widthKey = ["modPanelWidth"];
const WIDTH_SAVE_DELAY_MS = 250;

export function ModDetailPanel({ id, open, mod, onClose, focusOnOpen = false }: {
  id: string;
  open: boolean;
  mod: Mod | null;
  onClose: () => void;
  focusOnOpen?: boolean;
}) {
  const closeRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (open && focusOnOpen) closeRef.current?.focus();
  }, [open, focusOnOpen]);
  const client = useQueryClient();
  const remembered = useQuery({ queryKey: widthKey, queryFn: getModPanelWidth, enabled: open });
  const [requestedWidth, setRequestedWidth] = useState<number | null>(null);
  // A failed save is a draft, not a remembered preference. Closing discards it.
  useEffect(() => {
    if (!open) setRequestedWidth(null);
  }, [open]);
  const width = requestedWidth ?? remembered.data ?? 320;
  const save = useMutation({
    mutationFn: setModPanelWidth,
    // Range changes must reach SQLite in order, even on a slow connection.
    scope: { id: "modPanelWidth" },
    meta: { noInvalidationReason: "Only panel width changes; success writes its cached preference directly from bounded input." },
    onSuccess: (_, value) => client.setQueryData(widthKey, value),
  });
  const pendingWidth = useRef<number | null>(null);
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const commitWidth = () => {
    if (saveTimer.current !== null) clearTimeout(saveTimer.current);
    saveTimer.current = null;
    if (pendingWidth.current !== null) {
      save.mutate(pendingWidth.current);
      pendingWidth.current = null;
    }
  };
  const commitWidthRef = useRef(commitWidth);
  useEffect(() => { commitWidthRef.current = commitWidth; });
  useEffect(() => () => {
    // Flush while the app lives; window teardown can end IPC before it completes.
    commitWidthRef.current();
  }, []);
  const hasUpstream = open && mod?.source === "gamebanana";
  const updates = useQuery({
    queryKey: ["modUpdates", mod?.game],
    queryFn: () => listModUpdates(mod!.game),
    enabled: hasUpstream,
  });
  const upstreamVersion = updates.data?.find((row) => row.modId === mod?.id)?.upstreamVersion;

  const submissionUrl = mod?.sourceUrl?.trim()
    || (mod?.gamebananaId != null ? `https://gamebanana.com/mods/${mod.gamebananaId}` : null);

  if (!open) return <div id={id} hidden />;

  return (
    <aside id={id} className="mod-detail-panel" aria-labelledby={`${id}-heading`}
      style={{ flexBasis: `${width}px` }}>
      <div className="row row--between">
        <h2 id={`${id}-heading`}>{mod ? mod.name : "Mod details"}</h2>
        <button ref={closeRef} type="button" onClick={() => { commitWidth(); onClose(); }} aria-label="Close Mod details">Close</button>
      </div>
      {mod ? <>
        {/* Same definition-list convention and shared styling as DuplicateModDetails.
            Audit-only identity, recovery and Variant facts do not belong here. */}
        <dl className="duplicate-mod-details mod-detail-panel__facts small">
          {mod.libraryPath?.trim() ? <div><dt>Library path</dt><dd><code>{mod.libraryPath}</code></dd></div> : null}
          {mod.author?.trim() ? <div><dt>Author</dt><dd>{mod.author}</dd></div> : null}
          <div><dt>Source</dt><dd>{mod.source}</dd></div>
          {mod.version?.trim() ? <div><dt>Version</dt><dd>{mod.version}</dd></div> : null}
          {upstreamVersion?.trim() ? <div><dt>Last seen upstream</dt><dd>{upstreamVersion}</dd></div> : null}
          {mod.source === "gamebanana" && (mod.gamebananaId != null || mod.sourceUrl?.trim()) ?
            <div><dt>GameBanana</dt><dd>
              {mod.gamebananaId != null ? <span>submission {mod.gamebananaId}</span> : null}
              {submissionUrl ? <> <a href={submissionUrl} target="_blank" rel="noreferrer">View submission</a></> : null}
            </dd></div> : null}
        </dl>
      </> : <p>Select a Mod to view its details.</p>}
      <label className="mod-detail-panel__width">
        Panel width
        <input type="range" min="240" max="560" step="1" value={width}
          aria-valuetext={`${width} CSS pixels`}
          onChange={(event) => {
            const next = Number(event.currentTarget.value);
            setRequestedWidth(next);
            pendingWidth.current = next;
            if (saveTimer.current !== null) clearTimeout(saveTimer.current);
            saveTimer.current = setTimeout(commitWidth, WIDTH_SAVE_DELAY_MS);
          }} onBlur={commitWidth} />
      </label>
      {hasUpstream && <CommandErrorNotice error={updates.error}
        heading="GMM could not read this Mod's upstream version."
        detail="Upstream information is unavailable; this is not an “up to date” result. Any last seen upstream version is from an earlier reading." />}
      <CommandErrorNotice error={remembered.error} heading="GMM could not read the remembered panel width." />
      <CommandErrorNotice error={save.error} heading="GMM could not remember the panel width." />
    </aside>
  );
}
