import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import {
  avGuidance,
  detectAllGames,
  getGameInstallPath,
  type AvGuidance,
  type GameCode,
  type GameDetection,
  getLibraryPaths,
  installImporter,
  listSupportedGames,
  markOnboardingComplete,
  setGameInstallPath,
  setLibraryRoot,
  type GameSummary,
} from "./api";

/**
 * First-run onboarding wizard (slice 16-b / #24).
 *
 * Implements the 4-step linear flow specified in
 * `docs/design/onboarding.md`:
 *
 * 1. Welcome — AV/SmartScreen disclosure + acknowledgement gate.
 * 2. Game detection — parallel scan over every ported game.
 * 3. Library path — confirm default or pick a new root.
 * 4. Importer install — batched install of each per-game Model
 *    Importer.
 *
 * Skip-setup is reachable from every step. Either path
 * (`Finish` or `Skip setup`) calls `mark_onboarding_complete` with
 * the right `skipped` flag so the App router doesn't re-open the
 * wizard on next launch.
 */
export function OnboardingWizard({ onDone }: { onDone: (skipped: boolean) => void }) {
  const [step, setStep] = useState<1 | 2 | 3 | 4>(1);
  const [avAcknowledged, setAvAcknowledged] = useState(false);

  const close = useMutation({
    mutationFn: (skipped: boolean) => markOnboardingComplete(skipped),
    onSuccess: (_, skipped) => onDone(skipped),
  });

  const finish = () => close.mutate(false);
  const skip = () => close.mutate(true);

  return (
    <main className="onboarding">
      <header className="onboarding__header">
        <h1>GMM — first-run setup</h1>
        <span className="muted small">Step {step} of 4</span>
      </header>
      <section className="onboarding__body">
        {step === 1 ? (
          <WelcomeStep
            acknowledged={avAcknowledged}
            onAcknowledge={setAvAcknowledged}
          />
        ) : null}
        {step === 2 ? <DetectStep /> : null}
        {step === 3 ? <LibraryStep /> : null}
        {step === 4 ? <ImporterStep /> : null}
      </section>
      <footer className="onboarding__footer">
        <button
          className="onboarding__skip"
          onClick={skip}
          disabled={close.isPending}
        >
          Skip setup
        </button>
        <div className="row">
          {step > 1 ? (
            <button
              onClick={() => setStep((s) => Math.max(1, s - 1) as 1 | 2 | 3 | 4)}
              disabled={close.isPending}
            >
              ← Back
            </button>
          ) : null}
          {step < 4 ? (
            <button
              onClick={() => setStep((s) => Math.min(4, s + 1) as 1 | 2 | 3 | 4)}
              disabled={(step === 1 && !avAcknowledged) || close.isPending}
            >
              Continue →
            </button>
          ) : (
            <button onClick={finish} disabled={close.isPending}>
              {close.isPending ? "Saving…" : "Finish →"}
            </button>
          )}
        </div>
      </footer>
      {close.isError ? (
        <p className="error">{String(close.error)}</p>
      ) : null}
    </main>
  );
}

/** Step 1 — Welcome. Two-sentence Library-model copy + AV banner.
 *  Continue stays disabled until the user ticks the AV ack box. */
function WelcomeStep({
  acknowledged,
  onAcknowledge,
}: {
  acknowledged: boolean;
  onAcknowledge: (next: boolean) => void;
}) {
  const guidance = useQuery<AvGuidance>({
    queryKey: ["avGuidance"],
    queryFn: avGuidance,
  });

  return (
    <div className="card onboarding__welcome">
      {guidance.data ? (
        <div className="av-guidance" role="alert">
          <strong>{guidance.data.headline}</strong>
          <p>{guidance.data.body}</p>
          <ul>
            {guidance.data.exclusionSteps.map((step) => (
              <li key={step}>{step}</li>
            ))}
          </ul>
          <p className="muted small">
            See <code>{guidance.data.docPath}</code> for the full Defender
            / Norton / Bitdefender / Avast / AVG / ESET / Kaspersky guide
            and SmartScreen recovery steps.
          </p>
          <label className="toggle">
            <input
              type="checkbox"
              checked={acknowledged}
              onChange={(e) => onAcknowledge(e.currentTarget.checked)}
            />
            <span>I've read the AV note</span>
          </label>
        </div>
      ) : (
        <p className="muted">Loading antivirus guidance…</p>
      )}
      <h2>Welcome to GMM</h2>
      <p>
        GMM keeps every mod in a central Library, separate from your game
        installs. Enabling a mod links it into the game's <code>Mods/</code>
        {" "}folder; disabling removes the link, leaving your library copy
        untouched.
      </p>
    </div>
  );
}

/** Step 2 — Game detection. Fires `detect_all_games` once on mount;
 *  the user can accept the auto-detected path, browse manually, or skip
 *  a game entirely. */
function DetectStep() {
  const qc = useQueryClient();
  const detect = useQuery<GameDetection[]>({
    queryKey: ["onboarding", "detectAllGames"],
    queryFn: detectAllGames,
    refetchOnWindowFocus: false,
  });

  // Per-game user overrides (manual browse or skip), keyed by game code.
  // `null` means the user has not touched this row yet — fall through to
  // the detect result. `"skip"` marks the row as skipped.
  const [overrides, setOverrides] = useState<Record<string, string | "skip" | null>>({});

  const setPath = useMutation({
    mutationFn: ({ code, path }: { code: GameCode; path: string }) =>
      setGameInstallPath(code, path),
    onSuccess: (_, vars) => {
      setOverrides((prev) => ({ ...prev, [vars.code]: vars.path }));
      qc.invalidateQueries({ queryKey: ["installPath", vars.code] });
    },
  });

  const pickFolderFor = async (code: GameCode) => {
    const picked = await openDialog({ directory: true, multiple: false });
    if (typeof picked === "string") setPath.mutate({ code, path: picked });
  };

  if (detect.isLoading) {
    return (
      <div className="card">
        <h2>Find your games</h2>
        <p className="muted">
          Scanning common install locations + the Windows registry…
        </p>
      </div>
    );
  }
  if (detect.isError) {
    return (
      <div className="card">
        <h2>Find your games</h2>
        <p className="error">
          We couldn't run automatic detection. Browse manually for each game
          you want to set up.
        </p>
      </div>
    );
  }

  return (
    <div className="card">
      <h2>Find your games</h2>
      <p className="muted">
        GMM scans common install locations + the Windows registry.
      </p>
      <ul className="onboarding__games">
        {detect.data?.map((row) => {
          const override = overrides[row.code];
          const skipped = override === "skip";
          const effective = override === "skip" ? null : override ?? row.detectedPath;
          return (
            <li key={row.code} className="onboarding__game-row">
              <div className="onboarding__game-meta">
                <strong>{row.displayName}</strong>
                <span className="muted small">
                  {skipped
                    ? "Skipped"
                    : effective
                      ? effective
                      : "Not found"}
                </span>
              </div>
              <div className="row">
                {!skipped ? (
                  <button onClick={() => pickFolderFor(row.code)}>
                    {effective ? "Change…" : "Browse…"}
                  </button>
                ) : null}
                <button
                  onClick={() =>
                    setOverrides((prev) => ({
                      ...prev,
                      [row.code]: skipped ? null : "skip",
                    }))
                  }
                >
                  {skipped ? "Add later" : "Skip"}
                </button>
              </div>
            </li>
          );
        })}
      </ul>
      <div className="row">
        <button
          onClick={() => detect.refetch()}
          disabled={detect.isFetching}
        >
          {detect.isFetching ? "Re-scanning…" : "Re-scan"}
        </button>
      </div>
    </div>
  );
}

/** Step 3 — Library path. Defers entirely to slice 15's resolver +
 *  `setLibraryRoot` move flow. The wizard only sets the global root;
 *  per-game overrides remain in the main Settings panel. */
function LibraryStep() {
  const paths = useQuery({
    queryKey: ["libraryPaths"],
    queryFn: getLibraryPaths,
  });
  const setRoot = useMutation({
    mutationFn: (next: string | null) => setLibraryRoot(next),
  });

  const pickFolder = async () => {
    const picked = await openDialog({ directory: true, multiple: false });
    if (typeof picked === "string") setRoot.mutate(picked);
  };

  return (
    <div className="card">
      <h2>Where should GMM keep your Library?</h2>
      <p className="muted">
        Your mods live here, separate from your game installs. Plan on ~5–20
        GB depending on how many mods you collect.
      </p>
      <div className="row">
        <input
          className="path"
          value={paths.data?.effectiveRoot ?? ""}
          placeholder="Resolving…"
          readOnly
        />
        <button onClick={pickFolder} disabled={setRoot.isPending}>
          {setRoot.isPending ? "Moving…" : "Change…"}
        </button>
      </div>
      <p className="muted small">
        Per-game overrides are available in Settings later.
      </p>
      {setRoot.isError ? (
        <p className="error">{String(setRoot.error)}</p>
      ) : null}
    </div>
  );
}

/** Step 4 — Importer install. One row per game whose install path has
 *  been set (auto-detected in Step 2 OR manually browsed). Sequential
 *  install via the existing slice-3 importer flow with per-row Retry
 *  on failure. */
function ImporterStep() {
  const supported = useQuery<GameSummary[]>({
    queryKey: ["supportedGames"],
    queryFn: listSupportedGames,
    refetchOnWindowFocus: false,
    staleTime: Infinity,
  });

  // Query each ported game's persisted install path. Step 2 has
  // already written either the detect result or the user's manual
  // pick into the games table; here we just read it back.
  const installPaths = useQuery({
    queryKey: ["onboarding", "installPaths", supported.data?.map((g) => g.code).join(",") ?? ""],
    enabled: !!supported.data,
    queryFn: async () => {
      const games = supported.data ?? [];
      const entries = await Promise.all(
        games.map(async (g) => [g.code, await getGameInstallPath(g.code)] as const),
      );
      return Object.fromEntries(entries) as Record<GameCode, string | null>;
    },
    refetchOnWindowFocus: false,
  });

  const candidates = useMemo<GameSummary[]>(
    () => (supported.data ?? []).filter((g) => !!installPaths.data?.[g.code]),
    [supported.data, installPaths.data],
  );

  const [selected, setSelected] = useState<Record<string, boolean>>({});
  useEffect(() => {
    if (candidates.length === 0) return;
    setSelected((prev) =>
      candidates.reduce<Record<string, boolean>>((acc, g) => {
        acc[g.code] = prev[g.code] ?? true;
        return acc;
      }, {}),
    );
  }, [candidates]);

  const [statuses, setStatuses] = useState<Record<string, "queued" | "installing" | "done" | string>>({});
  const [running, setRunning] = useState(false);

  const installOne = async (code: GameCode) => {
    setStatuses((prev) => ({ ...prev, [code]: "installing" }));
    try {
      await installImporter(code);
      setStatuses((prev) => ({ ...prev, [code]: "done" }));
    } catch (e) {
      setStatuses((prev) => ({
        ...prev,
        [code]: `error: ${String(e)}`,
      }));
    }
  };

  const runInstall = async () => {
    setRunning(true);
    for (const game of candidates) {
      if (!selected[game.code]) continue;
      // Skip rows already installed in a prior pass of this same
      // wizard session.
      if (statuses[game.code] === "done") continue;
      await installOne(game.code);
    }
    setRunning(false);
  };

  if (installPaths.isLoading || !supported.data) {
    return (
      <div className="card">
        <h2>Install Model Importers</h2>
        <p className="muted">Loading…</p>
      </div>
    );
  }

  if (candidates.length === 0) {
    return (
      <div className="card">
        <h2>Install Model Importers</h2>
        <p className="muted">
          No detected games to install for. You can install importers from
          the Model Importer panel later.
        </p>
      </div>
    );
  }

  return (
    <div className="card">
      <h2>Install Model Importers</h2>
      <p className="muted">
        One Model Importer (3dmigoto-derived DLL) per game. Updates apply
        only when you click here — per ADR 0004 we never silently update
        the importer.
      </p>
      <ul className="onboarding__importers">
        {candidates.map((g) => {
          const status = statuses[g.code];
          const isError = typeof status === "string" && status.startsWith("error:");
          return (
            <li key={g.code} className="onboarding__importer-row">
              <label className="toggle">
                <input
                  type="checkbox"
                  checked={selected[g.code] ?? true}
                  disabled={running}
                  onChange={(e) =>
                    setSelected((prev) => ({
                      ...prev,
                      [g.code]: e.currentTarget.checked,
                    }))
                  }
                />
                <strong>{g.displayName}</strong>
              </label>
              <div className="row">
                <span className={`muted small${isError ? " error" : ""}`}>
                  {status === "installing"
                    ? "Installing…"
                    : status === "done"
                      ? "✓ Done"
                      : isError
                        ? status
                        : "Queued"}
                </span>
                {isError && !running ? (
                  <button onClick={() => installOne(g.code)}>Retry</button>
                ) : null}
              </div>
            </li>
          );
        })}
      </ul>
      <div className="row">
        <button onClick={runInstall} disabled={running}>
          {running ? "Installing…" : "Install selected"}
        </button>
      </div>
    </div>
  );
}
