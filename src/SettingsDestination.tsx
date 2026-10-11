import React, { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  detectGameInstallPath, getGameInstallPath, getGamePathStatus, getProxyConfig,
  rebuildJunctions, setGameInstallPath, setProxyConfig, testProxyConnection,
  type GameCode, type GameCode as ApiGameCode, type GamePathStatus, type GameSummary,
} from "./api";
import { ImporterPanel } from "./ImporterPanel";
import { ImporterOriginPanel } from "./ImporterOriginPanel";
import { PathField } from "./PathField";
import { LibraryAuditWarning } from "./LibraryAuditWarning";
import { CommandErrorNotice } from "./CommandErrorNotice";

export type SettingsPage = GameCode | "gmm";

/** Destination owner outlives visits. Visited Settings pages retain drafts,
 * pending operations and feedback; the main screen itself is unmounted. */
export function SettingsDestination({ games, page, returnGame, onPageChange, onBack }: {
  games: GameSummary[];
  page: SettingsPage | null;
  returnGame: string | null;
  onPageChange: (page: SettingsPage) => void;
  onBack: () => void;
}) {
  const [visited, setVisited] = useState<Set<SettingsPage>>(new Set());
  const heading = useRef<HTMLHeadingElement>(null);
  const previousPage = useRef<SettingsPage | null>(null);
  const statuses = useQueries({ queries: games.map((game) => ({
    queryKey: ["gamePathStatus", game.code],
    queryFn: () => getGamePathStatus(game.code),
    enabled: page !== null,
  })) });
  useLayoutEffect(() => {
    if (page !== null) {
      setVisited((previous) => new Set([...previous, page]));
      if (previousPage.current === null) heading.current?.focus();
    }
    previousPage.current = page;
  }, [page]);
  useEffect(() => {
    if (page === null) return;
    const escape = (event: KeyboardEvent) => {
      // Library action forms consume Escape to cancel their own prompt first.
      if (event.key === "Escape" && !event.defaultPrevented) onBack();
    };
    window.addEventListener("keydown", escape);
    return () => window.removeEventListener("keydown", escape);
  }, [page, onBack]);
  const pages = new Set(visited);
  if (page !== null) pages.add(page);
  return (
    <div className="settings-screen" hidden={page === null}>
      <main className="app settings-destination">
        <header className="app__header row row--between">
          <h1 ref={heading} tabIndex={-1}>Settings</h1>
          <div>
            <button onClick={onBack}>Back to Mods</button>
            {returnGame ? <p className="muted small">Return to {returnGame}</p> : null}
          </div>
        </header>
        <div className="settings-layout">
          <nav className="settings-navigation" aria-label="Settings pages">
            <h2>Games</h2>
            {games.map((game, index) => {
              const status = statuses[index];
              const line = status.isPending ? "Checking Game and Model Importer…"
                : status.isError ? "Cannot tell · path check unavailable"
                : status.data ? gamePathLine(status.data) : "Cannot tell · path check unavailable";
              return <button key={game.code} aria-current={page === game.code ? "page" : undefined}
                onClick={() => onPageChange(game.code)}>
                <strong>{game.displayName}</strong>
                <span>{status.isFetching && status.data ? `Checking · Previous: ${line}` : line}</span>
              </button>;
            })}
            <h2>Application</h2>
            <button aria-current={page === "gmm" ? "page" : undefined} onClick={() => onPageChange("gmm")}>
              GMM settings
            </button>
          </nav>
          <div className="settings-content">
            {games.filter((game) => pages.has(game.code)).map((game) => (
              <div key={game.code} hidden={page !== game.code}>
                <h2>{game.displayName}</h2>
                <GamePathPage game={game.code} displayName={game.displayName} />
                <ImporterPanel game={game.code} displayName={game.displayName} />
                <ImporterOriginPanel game={game.code} displayName={game.displayName} />
              </div>
            ))}
            {pages.has("gmm") ? <div hidden={page !== "gmm"}>
              <h2>GMM settings</h2>
              <NetworkPanel />
            </div> : null}
          </div>
        </div>
      </main>
    </div>
  );
}

export function gamePathLine(status: GamePathStatus): string {
  switch (status.gamePath.status) {
    case "noSavedPath": return "Absent · no saved path";
    case "savedPathNotFound": return "Saved installation cannot be found · Model Importer unverified";
    case "unreadable": return "Cannot tell · path unreadable; Model Importer unverified";
    case "invalid": return "Cannot verify Game layout · Model Importer unverified";
    case "readable":
      switch (status.modelImporter.status) {
        case "installed": return `${status.game.toUpperCase()} installed · Game readable`;
        case "requiredEntriesNotFound": return "Game readable · Model Importer not installed";
        case "unverified": return "Game readable · Cannot verify Model Importer";
      }
  }
}

function GamePathExplanation({ game }: { game: GameCode }) {
  const status = useQuery({ queryKey: ["gamePathStatus", game], queryFn: () => getGamePathStatus(game) });
  const value = status.data;
  let details: string | undefined;
  if (value) {
    const path = value.gamePath;
    if (path.status === "savedPathNotFound") details = path.paths.join(", ");
    else if (path.status === "invalid" || path.status === "unreadable") details = path.error;
    else if (value.modelImporter.status === "unverified") details = value.modelImporter.error;
    else if (value.modelImporter.status === "requiredEntriesNotFound") details = value.modelImporter.paths.join(", ");
  }
  return <div role="status">
    <p>{status.isPending ? "Checking Game and Model Importer…" : status.isError
      ? "Cannot tell · path check unavailable" : value ? gamePathLine(value) : "Cannot tell · path check unavailable"}</p>
    {value && (status.isFetching || status.isError) ? <p className="muted small">
      {status.isFetching ? "Checking again; the result below is previous." : "The details below are from the previous path check."}
    </p> : null}
    {details ? <p className="muted small">{details}</p> : null}
    <CommandErrorNotice error={status.error} />
  </div>;
}

/**
 * Settings → Network. HTTP / SOCKS5 proxy fields shared by every
 * outbound HTTP path in the backend. The password is write-only — the
 * UI never reads it back. A "Test connection" button hits api.github.com
 * through the configured proxy to validate.
 */
function NetworkPanel() {
  const qc = useQueryClient();
  const cfg = useQuery({ queryKey: ["proxyConfig"], queryFn: getProxyConfig });
  const [url, setUrl] = useState<string>("");
  const [username, setUsername] = useState<string>("");
  const [password, setPassword] = useState<string>("");

  React.useEffect(() => {
    if (cfg.data) {
      setUrl(cfg.data.url ?? "");
      setUsername(cfg.data.username ?? "");
      setPassword(""); // never seeded
    }
  }, [cfg.data]);

  const save = useMutation({
    mutationFn: () =>
      setProxyConfig({
        url: url || null,
        username: username || null,
        password: password ? password : null,
      }),
    onSuccess: () => setPassword(""),
    onSettled: () => qc.invalidateQueries({ queryKey: ["proxyConfig"] }),
  });
  const test = useMutation({
    mutationFn: testProxyConnection,
    meta: { noInvalidationReason: "Connection probe; no persisted query state to refresh." },
  });

  return (
    <section className="card">
      <h2>Network</h2>
      {cfg.isPending ? <p>Reading Network configuration…</p> : null}
      <CommandErrorNotice error={cfg.error} />
      <p className="muted">
        Optional HTTP or SOCKS5 proxy for GitHub release downloads and GameBanana
        traffic. Formats: <code>http://host:port</code> or <code>socks5://host:port</code>.
        Sensitive fields never reach the diagnostics bundle.
      </p>
      <div className="row">
        <input
          placeholder="proxy URL"
          value={url}
          onChange={(e) => setUrl(e.currentTarget.value)}
        />
      </div>
      <div className="row">
        <input
          placeholder="username (optional)"
          value={username}
          onChange={(e) => setUsername(e.currentTarget.value)}
        />
        <input
          type="password"
          placeholder={cfg.data?.passwordSet ? "(set — leave blank to keep)" : "password"}
          value={password}
          onChange={(e) => setPassword(e.currentTarget.value)}
        />
      </div>
      <div className="row">
        <button onClick={() => save.mutate()} disabled={save.isPending}>
          {save.isPending ? "Saving…" : "Save"}
        </button>
        <button onClick={() => test.mutate()} disabled={test.isPending}>
          {test.isPending ? "Testing…" : "Test connection"}
        </button>
        {test.isSuccess ? <span className="muted small">Proxy reachable.</span> : null}
      </div>
      <CommandErrorNotice error={save.error} />
      <CommandErrorNotice error={test.error} />
    </section>
  );
}

function GamePathPage({
  game,
  displayName,
}: {
  game: ApiGameCode;
  displayName: string;
}) {
  const queryClient = useQueryClient();
  const pathQuery = useQuery({
    queryKey: ["installPath", game],
    queryFn: () => getGameInstallPath(game),
  });

  const installPath = pathQuery.data;

  // Tracks whether the most recent path came from auto-detect, so we
  // can show the "Auto-detected" badge. Cleared as soon as the user
  // overrides it manually. Each Game owns its badge across page changes.
  const [lastSource, setLastSource] = useState<"manual" | "auto" | null>(null);
  const [detectFailed, setDetectFailed] = useState(false);
  useEffect(() => {
    setLastSource(null);
    setDetectFailed(false);
  }, [game]);

  const setPath = useMutation({
    mutationFn: (path: string) => setGameInstallPath(game, path),
    onSuccess: () => {
      setLastSource("manual");
      setDetectFailed(false);
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ["installPath", game] });
      queryClient.invalidateQueries({ queryKey: ["gamePathStatus", game] });
      queryClient.invalidateQueries({ queryKey: ["onboarding", "installPaths"] });
    },
  });

  const detect = useMutation({
    mutationFn: () => detectGameInstallPath(game),
    onSuccess: (path) => {
      if (path) {
        setLastSource("auto");
        setDetectFailed(false);
      } else {
        setDetectFailed(true);
      }
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ["installPath", game] });
      queryClient.invalidateQueries({ queryKey: ["gamePathStatus", game] });
      queryClient.invalidateQueries({ queryKey: ["onboarding", "installPaths"] });
    },
  });

  const label = pathQuery.isPending ? "Reading saved path…"
    : pathQuery.isError ? "Saved path unavailable"
    : lastSource === "auto"
      ? "Auto-detected"
      : lastSource === "manual"
        ? "Set manually"
        : installPath
          ? "Saved"
          : "No install path set";

  return (
    <section className="card">
      <h2>{displayName} install path</h2>
      <GamePathExplanation game={game} />
      {pathQuery.isPending ? <p>Reading saved install path…</p> : null}
      <CommandErrorNotice error={pathQuery.error} />
      <p className="muted">
        GMM looks for the {displayName} executable and required Game files. Use <strong>Auto-detect</strong> to scan
        known install locations, or type or pick the folder manually.
      </p>
      <div className="row">
        <PathField
          key={game}
          label={`${displayName} install path`}
          value={installPath ?? ""}
          placeholder={pathQuery.isPending ? "Reading saved path…" : pathQuery.isError ? "Saved path unavailable" : "No install path set"}
          disabled={pathQuery.isPending || setPath.isPending || detect.isPending}
          pickerLabel={installPath ? "Change…" : "Pick folder"}
          onApply={(path) => setPath.mutate(path)}
        />
        <span className="muted small">{label}</span>
      </div>
      <div className="row">
        <button onClick={() => detect.mutate()} disabled={detect.isPending || setPath.isPending}>
          {detect.isPending ? "Scanning…" : installPath ? "Re-detect" : "Auto-detect"}
        </button>
      </div>
      {detectFailed ? (
        <p className="muted small">
          Couldn't find {displayName} automatically. Pick the install folder
          manually.
        </p>
      ) : null}
      <CommandErrorNotice error={setPath.error} />
      <CommandErrorNotice error={detect.error} />
      <LibraryAuditWarning game={game} />
      <RebuildJunctions game={game} />
    </section>
  );
}

/**
 * Manual "Rebuild junctions" action — drops every junction for the
 * active game and recreates one per enabled Mod against the current
 * Library. The hammer to use after relocating the Library directory.
 */
function RebuildJunctions({ game }: { game: ApiGameCode }) {
  const qc = useQueryClient();
  const rebuild = useMutation({
    mutationFn: () => rebuildJunctions(game),
    onSettled: () => qc.invalidateQueries({ queryKey: ["mods", game] }),
  });
  return (
    <div className="row">
      <button onClick={() => rebuild.mutate()} disabled={rebuild.isPending}>
        {rebuild.isPending ? "Rebuilding…" : "Rebuild junctions"}
      </button>
      {rebuild.data ? (
        <span className="muted small">
          Recreated {rebuild.data.recreated.length}, removed {rebuild.data.removed.length} stranded,
          skipped {rebuild.data.skipped.length} disabled, left {rebuild.data.quarantined.length}{" "}
          quarantined untouched.
        </span>
      ) : null}
      <CommandErrorNotice error={rebuild.error} />
    </div>
  );
}
