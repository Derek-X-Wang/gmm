import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import {
  acceptImporterOriginProposal,
  dismissImporterOrigin,
  importerOriginStatus,
  originSlug,
  restoreImporterOrigin,
  setImporterOriginOverride,
  setImporterRecommendationsEnabled,
  toOriginInput,
  type GameCode,
  type ImporterOriginRef,
  type InstalledOrigin,
  type OriginLayer,
} from "./api";

/** How each precedence layer is named to a user looking at a repo name. */
const LAYER_LABEL: Record<OriginLayer, string> = {
  userOverride: "your override",
  recommendedManifest: "GMM's recommendation",
  compiledInDefault: "GMM's built-in default",
};

function describeInstalled(installed: InstalledOrigin): string {
  switch (installed.state) {
    case "known":
      return `${installed.owner}/${installed.repo}`;
    // Never rendered as "nothing installed": those users hand-installed
    // their importers precisely because GMM could not help them (#99).
    case "unknown":
      // True whether the folder holds a hand-installed importer or
      // nothing at all, which is the same state as far as GMM's records
      // go (#99). Claiming either specifically would be a guess about a
      // machine GMM cannot see.
      return "whatever Model Importer files are already in your game folder";
    case "unreadable":
      return "an importer whose origin GMM can no longer read";
  }
}

/**
 * The per-game Importer Origin surface (ADR 0005 / #109).
 *
 * Everything about one game's origin lives here rather than being split
 * between a prompt and a Settings list: the resolved origin and where it
 * came from, the change GMM is proposing, the dismissals that answer
 * those proposals, and the user's own override.
 *
 * Two placement decisions worth naming, since #109 left the shape open:
 *
 * - **The proposal is inline, not a modal.** A modal would be answered
 *   on reflex, and this is the one prompt in GMM that rewrites a game
 *   directory. Inline, it sits next to the origin it is proposing to
 *   replace, and doing nothing is a valid answer that costs the user
 *   nothing.
 * - **Dismissals live here, on the affected game's surface**, per #95.
 *   With at most six games and a handful of declined origins there is
 *   not enough of it to justify a Settings section, and a dismissal the
 *   user can only find somewhere else is one they will never undo.
 *
 * The global recommendations switch is here too, marked as global. GMM
 * has no separate Settings page — it is one column of cards — and the
 * precedent is `ModUpdatesPanel`, which renders the global mod-update
 * switch in the panel it governs rather than in the per-game Settings
 * card.
 */
export function ImporterOriginPanel({
  game,
  displayName,
}: {
  game: GameCode;
  displayName: string;
}) {
  const qc = useQueryClient();
  const status = useQuery({
    queryKey: ["importerOrigin", game],
    queryFn: () => importerOriginStatus(game),
    retry: false,
  });

  const invalidate = () => {
    qc.invalidateQueries({ queryKey: ["importerOrigin", game] });
    // Accepting installs, and the override moves which repository the
    // badge is computed from, so the importer panel is stale either way.
    qc.invalidateQueries({ queryKey: ["importer"] });
  };

  const accept = useMutation({
    mutationFn: () => acceptImporterOriginProposal(game),
    onSuccess: invalidate,
  });
  const decline = useMutation({
    mutationFn: (origin: ImporterOriginRef) =>
      dismissImporterOrigin(game, toOriginInput(origin)),
    onSuccess: invalidate,
  });
  const undo = useMutation({
    mutationFn: (origin: ImporterOriginRef) =>
      restoreImporterOrigin(game, toOriginInput(origin)),
    onSuccess: invalidate,
  });
  const toggleRecommendations = useMutation({
    mutationFn: (enabled: boolean) => setImporterRecommendationsEnabled(enabled),
    // Every game's surface changes, not just this one.
    onSuccess: () => qc.invalidateQueries({ queryKey: ["importerOrigin"] }),
  });

  const data = status.data;
  // Bound once so the decline handler closes over a value rather than
  // re-narrowing `data.proposal` inside a callback.
  const proposalOrigin = data?.proposal?.origin;

  return (
    <section className="card" aria-label="Importer Origin">
      <h2>Importer Origin</h2>
      <p className="muted">
        Where {displayName}'s Model Importer comes from. GMM never hosts or
        maintains importer packages — it points at ones other people publish,
        and you can point it somewhere else.
      </p>

      {status.isError ? (
        <p className="error">{String(status.error)}</p>
      ) : null}

      {data ? (
        <>
          <div className="row">
            <span className="muted small">
              {data.resolved.state === "inEffect" ? (
                <>
                  In effect: <code>{originSlug(data.resolved.origin)}</code> ·{" "}
                  {LAYER_LABEL[data.resolved.layer]}
                </>
              ) : (
                <>No Importer Origin is in effect.</>
              )}
            </span>
            <label className="toggle">
              <input
                type="checkbox"
                checked={data.recommendationsEnabled}
                disabled={toggleRecommendations.isPending}
                onChange={(e) => toggleRecommendations.mutate(e.currentTarget.checked)}
              />
              <span>Recommend importer origins (all games)</span>
            </label>
          </div>

          {data.installTarget.state === "installed" &&
          data.resolved.state === "inEffect" &&
          originSlug(data.installTarget) !== originSlug(data.resolved.origin) ? (
            <p className="muted small">
              Updates stay on <code>{originSlug(data.installTarget)}</code>, the
              origin this install came from. Changing origin replaces the
              installed package rather than updating it, so it only happens when
              you say so.
            </p>
          ) : null}

          {data.resolved.state === "noneInEffect" ? (
            <div
              className="library-audit-warning"
              role="status"
              aria-label="No Importer Origin in effect"
            >
              <strong>
                GMM has no Model Importer origin for {displayName}.
              </strong>
              {data.resolved.reason ? <p>{data.resolved.reason}</p> : null}
              <p className="small">
                You can still use {displayName}: set an origin below and GMM will
                install from it. Nothing here blocks you.
              </p>
            </div>
          ) : null}

          {data.installTarget.state === "installedUnreadable" ? (
            <p className="error">
              GMM recorded a Model Importer install for {displayName} but can no
              longer read which Importer Origin it came from:{" "}
              {data.installTarget.error}. It will not install over it from a
              different origin, so Install and the update check are unavailable
              until you save an origin below — which also replaces the
              unreadable record.
            </p>
          ) : null}

          {data.recommendationsUnusableReason ? (
            <p className="error">
              GMM could not use the recommendation list it fetched:{" "}
              {data.recommendationsUnusableReason}
            </p>
          ) : null}

          {data.proposal && proposalOrigin ? (
            <div
              className="library-audit-warning"
              role="group"
              aria-label="GMM recommends a different Importer Origin"
            >
              <strong>
                GMM recommends {displayName}'s Model Importer come from{" "}
                <code>{originSlug(data.proposal.origin)}</code>.
              </strong>
              {data.proposal.reason ? <p>{data.proposal.reason}</p> : null}
              <p className="small">
                Switching installs <code>{originSlug(data.proposal.origin)}</code>{" "}
                into your game folder, replacing{" "}
                {describeInstalled(data.proposal.replaces)}. GMM backs up the
                existing files first and the install can be rolled back.
              </p>
              <div className="row">
                <button
                  onClick={() => accept.mutate()}
                  disabled={accept.isPending}
                >
                  {accept.isPending ? "Switching…" : "Switch and install"}
                </button>
                <button
                  onClick={() => decline.mutate(proposalOrigin)}
                  disabled={decline.isPending}
                >
                  Not now
                </button>
              </div>
              {accept.isError ? (
                <p className="error">{String(accept.error)}</p>
              ) : null}
            </div>
          ) : null}

          {data.dismissed.length > 0 ? (
            <div role="group" aria-label="Dismissed recommendations">
              <h3 className="muted small">Dismissed recommendations</h3>
              <ul className="mods">
                {data.dismissed.map((origin) => (
                  <li className="row row--between" key={originSlug(origin)}>
                    <code>{originSlug(origin)}</code>
                    <button
                      onClick={() => undo.mutate(origin)}
                      disabled={undo.isPending}
                    >
                      Undo
                    </button>
                  </li>
                ))}
              </ul>
            </div>
          ) : null}

          {data.dismissalsError ? (
            <p className="error">
              GMM could not read which recommendations you have dismissed:{" "}
              {data.dismissalsError}. It will offer them again rather than
              silently hiding them; dismissing one replaces the unreadable
              record.
            </p>
          ) : null}

          {data.userOverride.state === "unreadable" ? (
            <p className="error">
              GMM could not read the Importer Origin you saved for {displayName}:{" "}
              {data.userOverride.error}. It was stored as{" "}
              <code>{data.userOverride.raw}</code>. Save one again below to
              replace it.
            </p>
          ) : null}

          <OverrideEditor
            game={game}
            current={
              data.userOverride.state === "set" ? data.userOverride : null
            }
            fallback={data.compiledDefault}
            onChanged={invalidate}
          />
        </>
      ) : null}
    </section>
  );
}

/**
 * The per-game override editor (layer 1).
 *
 * Three boxes rather than a URL, because ADR 0005 rejected an arbitrary
 * download URL: it costs the version string, which silently disables
 * both the Importer Pin and the update badge. The fields are validated
 * in Rust so the frontend and the manifest are held to one rule.
 */
function OverrideEditor({
  game,
  current,
  fallback,
  onChanged,
}: {
  game: GameCode;
  current: ImporterOriginRef | null;
  fallback: ImporterOriginRef | null;
  onChanged: () => void;
}) {
  const [owner, setOwner] = useState("");
  const [repo, setRepo] = useState("");
  const [assetPattern, setAssetPattern] = useState("");

  const save = useMutation({
    mutationFn: () =>
      setImporterOriginOverride(game, { owner, repo, assetPattern }),
    onSuccess: () => {
      setOwner("");
      setRepo("");
      setAssetPattern("");
      onChanged();
    },
  });
  const clear = useMutation({
    mutationFn: () => setImporterOriginOverride(game, null),
    onSuccess: onChanged,
  });

  return (
    <div>
      <h3 className="muted small">Your own origin</h3>
      {current ? (
        <p className="muted small">
          Currently <code>{originSlug(current)}</code>. Clearing it returns this
          game to GMM's recommendation
          {fallback ? (
            <>
              , then to <code>{originSlug(fallback)}</code>
            </>
          ) : null}
          .
        </p>
      ) : (
        <p className="muted small">
          Point this game at a GitHub release of your choosing. Your own origin
          outranks GMM's recommendation and its built-in default.
        </p>
      )}
      <div className="row">
        <label>
          Owner
          <input
            value={owner}
            onChange={(e) => setOwner(e.currentTarget.value)}
            placeholder="SilentNightSound"
          />
        </label>
        <label>
          Repository
          <input
            value={repo}
            onChange={(e) => setRepo(e.currentTarget.value)}
            placeholder="GIMI-Package"
          />
        </label>
        <label>
          Asset pattern
          <input
            value={assetPattern}
            onChange={(e) => setAssetPattern(e.currentTarget.value)}
            placeholder="GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip"
          />
        </label>
      </div>
      <div className="row">
        <button onClick={() => save.mutate()} disabled={save.isPending}>
          {save.isPending ? "Saving…" : "Save origin"}
        </button>
        {current ? (
          <button onClick={() => clear.mutate()} disabled={clear.isPending}>
            Clear override
          </button>
        ) : null}
      </div>
      {save.isError ? <p className="error">{String(save.error)}</p> : null}
      {clear.isError ? <p className="error">{String(clear.error)}</p> : null}
    </div>
  );
}
