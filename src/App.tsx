import { useEffect, useLayoutEffect, useRef, useState } from "react";

import { WindowBar } from "./WindowBar";
import { GameHeader } from "./GameHeader";
import { GlobalHeaderLine } from "./GlobalHeaderLine";
import { ModList } from "./ModList";
import { GameTabs } from "./GameTabs";
import { GameStatusStrip } from "./GameStatusStrip";
import { GameRecoveryNotice } from "./GameRecoveryNotice";
import { AttentionView } from "./AttentionView";
import { SettingsDestination, type SettingsPage } from "./SettingsDestination";
import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import {
  avGuidance,
  cleanStaleSession,
  currentSession,
  interruptedSessionLaunches,
  isOnboardingComplete,
  launchGame,
  listSupportedGames,
  partitionLaunchError,
  resetOnboarding,
  retireInterruptedSessionLaunch,
  SESSION_ENDED_EVENT,
  SESSION_STARTED_EVENT,
  type AvGuidance,
  type GameSummary,
  type OnboardingStatus,
  type SessionInfo,
  checkModUpdatesNow,
  getGameInstallPath,
  getStartupReconcileStatus,
  listMods,
  listModUpdates,
  modUpdatesGloballyEnabled,
  setModUpdatesGloballyEnabled,
  type GameCode as ApiGameCode,
} from "./api";
import { InterruptedSessionLaunchWarning } from "./InterruptedSessionLaunchWarning";
import { OnboardingWizard } from "./OnboardingWizard";
import { CommandErrorNotice } from "./CommandErrorNotice";
import { commandFailureMessage, type CommandFailure } from "./commandError";
import "./App.css";

/**
 * Fallback used if `list_supported_games` ever returns empty (broken
 * backend wiring) or the query is still loading on first paint. Keeps
 * the UI usable instead of rendering an empty shell.
 */
const FALLBACK_GAMES: GameSummary[] = [
  { code: "gimi", displayName: "Genshin Impact" },
];

function App() {
  const qc = useQueryClient();
  const onboarding = useQuery<OnboardingStatus>({
    queryKey: ["onboarding", "status"],
    queryFn: isOnboardingComplete,
    refetchOnWindowFocus: false,
    staleTime: Infinity,
  });

  // The wizard can be force-opened from the main app (Help → Run setup
  // again, or the "Finish setup" banner's Resume button). The query
  // alone isn't enough — once `complete=true` we still need a way to
  // re-show the wizard without resetting state to "not complete".
  const [forceWizard, setForceWizard] = useState(false);
  const [setupError, setSetupError] = useState<unknown>(null);

  if (onboarding.isLoading) {
    return (
      <main className="app">
        <p className="muted">Loading…</p>
      </main>
    );
  }

  const status = onboarding.data;
  const showWizard = forceWizard || !status?.complete;

  if (showWizard) {
    return (
      <>
        <OnboardingWizard
          onFailure={setSetupError}
          onDone={() => {
            setSetupError(null);
            setForceWizard(false);
            qc.invalidateQueries({ queryKey: ["installPath"] });
            qc.invalidateQueries({ queryKey: ["gamePathStatus"] });
            qc.invalidateQueries({ queryKey: ["libraryPaths"] });
          }}
        />
        <CommandErrorNotice error={setupError} />
      </>
    );
  }

  return (
    <>
      <MainApp
        skipped={status?.skipped ?? false}
        onSetupFailure={setSetupError}
        onResumeWizard={() => {
          setSetupError(null);
          setForceWizard(true);
        }}
      />
      <CommandErrorNotice error={setupError} />
    </>
  );
}

function MainApp({
  skipped,
  onResumeWizard,
  onSetupFailure,
}: {
  skipped: boolean;
  onResumeWizard: () => void;
  onSetupFailure: (error: unknown) => void;
}) {
  const qc = useQueryClient();
  const games = useQuery<GameSummary[]>({
    queryKey: ["supportedGames"],
    queryFn: listSupportedGames,
    // The registry is static at build time; refetch on focus is wasteful.
    refetchOnWindowFocus: false,
    staleTime: Infinity,
  });
  const list = (games.data ?? FALLBACK_GAMES).length > 0 ? games.data ?? FALLBACK_GAMES : FALLBACK_GAMES;
  // Share Settings' query keys so path entry, detection, and setup refresh
  // the tab strip through their existing invalidation paths.
  const installPaths = useQueries({
    queries: list.map((game) => ({
      queryKey: ["installPath", game.code],
      queryFn: () => getGameInstallPath(game.code),
    })),
  });
  // Query data retains the last successful saved path when a refresh rejects.
  // Filesystem health and Model Importer presence do not determine tab membership.
  const installed = list.filter((_, index) => Boolean(installPaths[index].data?.trim()));
  const remaining = list.filter((game) => !installed.some((entry) => entry.code === game.code));
  const pathsPending = games.isPending || installPaths.some((path) => path.isPending);
  const pathErrors = installPaths.flatMap((path, index) => path.isError
    ? [`${list[index].displayName}: ${commandFailureMessage(path.error)}`]
    : []);
  const [selectedGame, setActiveGame] = useState<ApiGameCode | null>(null);
  // Settings never writes this selection. Freeze the first installed Game
  // once paths load so adding an earlier registry entry cannot steal it.
  const active = installed.find((game) => game.code === selectedGame) ?? installed[0] ?? list[0];
  const hasActiveGame = installed.some((game) => game.code === active.code);
  const activePath = installPaths[list.findIndex((game) => game.code === active.code)];
  const activeGame = active.code;
  useEffect(() => {
    if (!pathsPending && selectedGame === null && installed.length > 0) setActiveGame(installed[0].code);
  }, [selectedGame, installed, pathsPending]);

  const [settingsPage, setSettingsPage] = useState<SettingsPage | null>(null);
  const returnPosition = useRef(0);
  const returnControl = useRef<string | null>(null);
  const returnAnchor = useRef<{ id: string; top: number } | null>(null);
  // Keep the current list's data observed across navigation. A long Settings
  // visit must not evict it and clamp the return scroll to an empty screen.
  useQuery({ queryKey: ["mods", activeGame], queryFn: () => listMods(activeGame) });
  const { staleSession, dismissStaleSession } = useSessionLifecycle();
  const sessionSurface = useSessionPresentation({ staleSession, onDismiss: dismissStaleSession });
  const launchSurface = useLaunchGame({ game: activeGame, displayName: active.displayName });
  const openSettings = (page: SettingsPage, control = "game") => {
    returnControl.current = control;
    returnPosition.current = window.scrollY;
    const rows = Array.from(document.querySelectorAll<HTMLElement>("[data-mod-id]"));
    const anchor = rows.find((row) => row.getBoundingClientRect().bottom > 0);
    returnAnchor.current = anchor ? { id: anchor.dataset.modId!, top: anchor.getBoundingClientRect().top } : null;
    setSettingsPage(page);
  };
  useLayoutEffect(() => {
    if (settingsPage === null) {
      if (returnControl.current === null) return;
      const anchor = Array.from(document.querySelectorAll<HTMLElement>("[data-mod-id]"))
        .find((row) => row.dataset.modId === returnAnchor.current?.id);
      const top = anchor && returnAnchor.current
        ? window.scrollY + anchor.getBoundingClientRect().top - returnAnchor.current.top
        : returnPosition.current;
      window.scrollTo(0, top);
      const control = document.querySelector<HTMLElement>(`[data-settings-arrival="${returnControl.current}"]`)
        ?? document.querySelector<HTMLElement>('.game-tabs__tab[aria-selected="true"]');
      control?.focus({ preventScroll: true });
    } else window.scrollTo(0, 0);
  }, [settingsPage]);

  const reopenSetup = useMutation({
    mutationFn: () => resetOnboarding(),
    onSuccess: onResumeWizard,
    onError: onSetupFailure,
    onSettled: () => qc.invalidateQueries({ queryKey: ["onboarding", "status"] }),
  });

  return (
    <>
      <SettingsDestination
        games={list}
        page={settingsPage}
        returnGame={hasActiveGame ? active.displayName : null}
        onPageChange={setSettingsPage}
        onBack={() => setSettingsPage(null)}
      />
      {settingsPage === null ? (
        <main className="app">
          <WindowBar
            onSettings={() => openSettings("gmm", "global")}
            onSetup={() => reopenSetup.mutate()}
            setupPending={reopenSetup.isPending}
          />
          {hasActiveGame ? <GameHeader displayName={active.displayName} action={launchSurface.action} /> : null}
          <GlobalHeaderLine>
            <AttentionView />
            {sessionSurface.context}
          </GlobalHeaderLine>
          <GameTabs
            games={installed}
            remaining={remaining}
            pending={pathsPending}
            pathsUnavailable={pathErrors.length > 0}
            active={activeGame}
            onChange={setActiveGame}
            onAddGame={(game) => openSettings(game, "add")}
          />
          {hasActiveGame ? <GameStatusStrip
            game={activeGame}
            displayName={active.displayName}
            savedPath={activePath.data ?? null}
            pathReadError={activePath.error}
            pathReadPending={activePath.isFetching}
            onSettings={() => openSettings(activeGame)}
          /> : null}
          <div className="main-screen__notices" role="region" aria-label="Game and operation notices">
            {skipped ? (
              <section className="card finish-setup-banner" role="status">
                <div>
                  <strong>Setup isn't finished yet.</strong>
                  <span className="muted small">
                    {" "}You skipped one or more steps. Resume any time.
                  </span>
                </div>
                <button onClick={onResumeWizard}>Resume setup</button>
              </section>
            ) : null}
            <StartupReconcileNotice games={list} />
            {sessionSurface.notices}
            {hasActiveGame ? launchSurface.notice : null}
            {hasActiveGame ? <GameRecoveryNotice game={activeGame} displayName={active.displayName}
              onSettings={() => openSettings(activeGame)} /> : null}
            {pathErrors.length > 0 ? (
              <OperationFailureNotice
                heading="GMM could not read some game install paths."
                detail="Use a game tab or Add game to check the affected paths in Settings."
                errors={pathErrors}
              />
            ) : null}
          </div>
          {hasActiveGame ? <>
            <ModList game={activeGame} displayName={active.displayName} />
            <ModUpdatesPanel game={activeGame} />
          </> : null}
        </main>
      ) : null}
    </>
  );
}

/**
 * Fail closed at the smallest independently safe unit, then say what could
 * not be computed and preserve the backend's repair route. Conflict checks,
 * per-game startup reconcile, and per-Mod relocation restore all use this
 * presentation so uncertainty never turns into an empty/healthy answer.
 */
function OperationFailureNotice({
  heading,
  detail,
  errors,
}: {
  heading: string;
  detail: string;
  errors: string[];
}) {
  return (
    <div className="error" role="alert">
      <strong>{heading}</strong>
      <p>{detail}</p>
      <ul>
        {errors.map((error, index) => (
          <li key={`${index}:${error}`} className="small">
            {error}
          </li>
        ))}
      </ul>
    </div>
  );
}

function StartupReconcileNotice({ games }: { games: GameSummary[] }) {
  const status = useQuery({
    queryKey: ["startupReconcileStatus"],
    queryFn: getStartupReconcileStatus,
    refetchInterval: (query) => (query.state.data?.finished ? false : 250),
  });
  const failures = status.data?.failures ?? [];
  if (failures.length === 0) return null;

  return (
    <section className="card" aria-label="Startup game link check">
      <OperationFailureNotice
        heading="GMM could not finish checking your game links at startup."
        detail="The affected game was left unchanged; other games were checked independently."
        errors={failures.map(
          (failure) => `${games.find((game) => game.code === failure.game)?.displayName ?? failure.game.toUpperCase()} (${failure.game.toUpperCase()}): ${failure.error}`,
        )}
      />
    </section>
  );
}

/**
 * Sticky session surface. It shows active sessions, an evicted stale session,
 * and interrupted launch reservations that require user confirmation.
 */
function useSessionLifecycle() {
  const queryClient = useQueryClient();
  useQuery({ queryKey: ["session"], queryFn: currentSession, refetchOnWindowFocus: true });
  const [staleSession, setStaleSession] = useState<SessionInfo | null>(null);
  // On first mount, ask the backend to clear any orphan row left by a
  // GMM crash mid-session. If we got something back, surface it.
  useEffect(() => {
    cleanStaleSession()
      .then((evicted) => {
        if (evicted) setStaleSession(evicted);
        queryClient.invalidateQueries({ queryKey: ["session"] });
        queryClient.invalidateQueries({ queryKey: ["interruptedSessionLaunches"] });
      })
      .catch(() => {
        /* best-effort */
      });
  }, [queryClient]);

  // Listen for backend session events; refetch the session query.
  useEffect(() => {
    const unlisteners: Promise<UnlistenFn>[] = [
      listen(SESSION_STARTED_EVENT, () => {
        queryClient.invalidateQueries({ queryKey: ["session"] });
      }),
      listen(SESSION_ENDED_EVENT, () => {
        queryClient.invalidateQueries({ queryKey: ["session"] });
      }),
    ];
    return () => {
      unlisteners.forEach((p) => {
        p.then((u) => u()).catch(() => {});
      });
    };
  }, [queryClient]);

  return { staleSession, dismissStaleSession: () => setStaleSession(null) };
}

function useSessionPresentation({ staleSession, onDismiss }: {
  staleSession: SessionInfo | null;
  onDismiss: () => void;
}) {
  const queryClient = useQueryClient();
  const session = useQuery({
    queryKey: ["session"],
    queryFn: currentSession,
    refetchOnWindowFocus: true,
  });
  const interruptedLaunches = useQuery({
    queryKey: ["interruptedSessionLaunches"],
    queryFn: interruptedSessionLaunches,
    refetchOnWindowFocus: true,
  });
  const retireLaunch = useMutation({
    mutationFn: retireInterruptedSessionLaunch,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["interruptedSessionLaunches"] });
      queryClient.invalidateQueries({ queryKey: ["session"] });
    },
  });

  return {
    notices: <>
      {(interruptedLaunches.data ?? []).map((launch) => (
        <InterruptedSessionLaunchWarning
          key={launch.id}
          launch={launch}
          pending={retireLaunch.isPending && retireLaunch.variables === launch.id}
          error={
            retireLaunch.isError && retireLaunch.variables === launch.id
              ? commandFailureMessage(retireLaunch.error)
              : undefined
          }
          onRetire={() => retireLaunch.mutate(launch.id)}
        />
      ))}
    </>,
    context: <>
      {staleSession && !session.data ? (
        <section className="card session-banner session-banner--stale">
          <strong>{staleSession.game.toUpperCase()} ended unexpectedly last time.</strong>
          <span className="muted">
            GMM cleaned up the orphaned session record (started {staleSession.startedAt}).
          </span>
          <button onClick={onDismiss}>Dismiss</button>
        </section>
      ) : null}
      {session.data ? (
        <section className="card session-banner session-banner--active" role="status">
          <strong>{session.data.game.toUpperCase()} is running</strong>
          <span className="muted">
            Mod changes are locked. Close the game to re-enable editing. (PID {session.data.pid})
          </span>
        </section>
      ) : null}
    </>,
  };
}

/** Hook surfaces whether a session is currently active. */
function useSessionActive(): boolean {
  const session = useQuery({
    queryKey: ["session"],
    queryFn: currentSession,
  });
  return !!session.data;
}

/** Per-game Launch button. Disabled while a session is already active. */
function useLaunchGame({
  game,
  displayName,
}: {
  game: ApiGameCode;
  displayName: string;
}) {
  const queryClient = useQueryClient();
  const sessionActive = useSessionActive();

  // Lazy-load the structured AV / SmartScreen guidance only when we
  // need it (i.e. a launch failed with an AV-pattern error). The
  // payload is small + cheap; we still avoid the IPC round-trip on
  // happy-path launches.
  const guidance = useQuery<AvGuidance>({
    queryKey: ["avGuidance"],
    queryFn: avGuidance,
    enabled: false,
  });

  const launch = useMutation({
    mutationFn: () => launchGame(game),
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ["session"] });
      queryClient.invalidateQueries({ queryKey: ["interruptedSessionLaunches"] });
    },
    onError: () => {
      // Fetch guidance on demand so the inline exclusion steps land in
      // the next render. The classifier might say not-AV; we still
      // refetch — it's idempotent and tiny.
      guidance.refetch();
    },
  });

  const partitioned = launch.isError
    ? partitionLaunchError(launch.error, guidance.data?.sentinel ?? "AV-PATTERN: ")
    : null;

  return {
    action: <button
      className="game-header__launch"
      onClick={() => launch.mutate()}
      disabled={sessionActive || launch.isPending}
    >
      {sessionActive ? "Game running…" : launch.isPending ? "Launching…" : `Launch ${displayName}`}
    </button>,
    notice: <>
      {partitioned && partitioned.isAvPattern && guidance.data ? (
        <AvGuidanceCallout
          guidance={guidance.data}
          failure={partitioned.failure}
        />
      ) : partitioned ? (
        <CommandErrorNotice error={partitioned.failure} />
      ) : null}
    </>,
  };
}

/**
 * Inline AV / SmartScreen guidance shown when `launch_game` fails with
 * a classified AV-pattern error. Content comes verbatim from
 * `docs/antivirus-and-smartscreen.md` (slice NEW-AV / #13) via the
 * `av_guidance` Tauri command so the wizard (#24) and this component
 * cannot drift.
 */
function AvGuidanceCallout({
  guidance,
  failure,
}: {
  guidance: AvGuidance;
  failure: CommandFailure;
}) {
  return (
    <div
      className="av-guidance error"
      role="alert"
      data-command-failure-kind={failure.kind}
    >
      <strong>{guidance.headline}</strong>
      <p>{guidance.body}</p>
      <ul>
        {guidance.exclusionSteps.map((step) => (
          <li key={step}>{step}</li>
        ))}
      </ul>
      <p className="muted small">
        See{" "}
        <code>{guidance.docPath}</code>
        {" "}
        for the full guide (Defender, Norton, Bitdefender, Avast, AVG,
        ESET, Kaspersky, plus SmartScreen recovery).
      </p>
      <details>
        <summary className="muted small">Underlying error</summary>
        <pre className="muted small">{failure.message}</pre>
      </details>
    </div>
  );
}

/**
 * Settings → Mod updates. Global toggle, "Check now" button, and a
 * status line showing how many mods have upstream updates available.
 * Per-mod badges live in each Mod row (see ModUpdateBadge).
 */
function ModUpdatesPanel({ game }: { game: ApiGameCode }) {
  const qc = useQueryClient();
  const global = useQuery({
    queryKey: ["modUpdates", "globalEnabled"],
    queryFn: modUpdatesGloballyEnabled,
  });
  const rows = useQuery({
    queryKey: ["modUpdates", game],
    queryFn: () => listModUpdates(game),
  });
  const check = useMutation({
    mutationFn: () => checkModUpdatesNow(game),
    onSettled: () => qc.invalidateQueries({ queryKey: ["modUpdates", game] }),
  });
  const toggleGlobal = useMutation({
    mutationFn: (enabled: boolean) => setModUpdatesGloballyEnabled(enabled),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["modUpdates", "globalEnabled"] }),
  });
  const ahead = rows.data?.filter((r) => r.upstreamAhead).length ?? 0;
  return (
    <section className="card">
      <h2>
        Mod updates
        {ahead > 0 ? <span className="update-badge"> · {ahead}</span> : null}
      </h2>
      <p className="muted">
        Weekly poll of every <code>source = gamebanana</code> Mod. Updates never apply
        automatically — click a badge on the Mod row to review + Apply. Pin a Mod's
        check off via its row toggle. Network goes through the configured proxy.
      </p>
      <div className="row">
        <button onClick={() => check.mutate()} disabled={check.isPending}>
          {check.isPending ? "Checking…" : "Check now"}
        </button>
        <label className="toggle">
          <input
            type="checkbox"
            checked={global.data ?? true}
            disabled={toggleGlobal.isPending}
            onChange={(e) => toggleGlobal.mutate(e.currentTarget.checked)}
          />
          <span>Weekly check enabled</span>
        </label>
      </div>
      <CommandErrorNotice error={check.error} />
    </section>
  );
}

export default App;
