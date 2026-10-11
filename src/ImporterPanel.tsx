import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  checkImporterUpdate, fetchLatestImporterRelease, getImporterEvacuationRecovery,
  installImporter, rollbackImporter, setImporterPinned,
  retryImporterEvacuationRecovery, retireInterruptedImporterEvacuation,
  getGamePathStatus, type GameCode,
} from "./api";
import { ImporterEvacuationRecoveryWarning } from "./ImporterEvacuationRecoveryWarning";
import { LoaderVersionNote } from "./LoaderVersionNote";
import { CommandErrorNotice } from "./CommandErrorNotice";

/** Per-Game importer configuration and evacuation recovery in Settings. */
export function ImporterPanel({
  game,
  displayName,
}: {
  game: GameCode;
  displayName: string;
}) {
  const qc = useQueryClient();
  const path = useQuery({
    queryKey: ["gamePathStatus", game],
    queryFn: () => getGamePathStatus(game),
    retry: false,
  });
  const release = useQuery({
    queryKey: ["importer", "latest", game],
    queryFn: () => fetchLatestImporterRelease(game),
    retry: false,
  });
  const update = useQuery({
    queryKey: ["importer", "update", game],
    queryFn: () => checkImporterUpdate(game),
    retry: false,
  });
  const evacuation = useQuery({
    queryKey: ["importer", "evacuation", game],
    queryFn: () => getImporterEvacuationRecovery(game),
    retry: false,
  });

  const invalidate = () => {
    qc.invalidateQueries({ queryKey: ["importer"] });
    qc.invalidateQueries({ queryKey: ["importerOrigin", game] });
    qc.invalidateQueries({ queryKey: ["gamePathStatus", game] });
  };

  const install = useMutation({
    mutationFn: () => installImporter(game),
    onSettled: invalidate,
  });
  const rollback = useMutation({
    mutationFn: () => rollbackImporter(game),
    onSettled: invalidate,
  });
  const pin = useMutation({
    mutationFn: (version: string | null) => setImporterPinned(game, version),
    onSuccess: invalidate,
  });
  const retireEvacuation = useMutation({
    mutationFn: () => retireInterruptedImporterEvacuation(game),
    onSettled: invalidate,
  });
  const retryEvacuation = useMutation({
    mutationFn: () => retryImporterEvacuationRecovery(game),
    onSettled: invalidate,
  });

  const importerLabel = game.toUpperCase();
  const readable = !path.isError && !path.isFetching && path.data?.gamePath.status === "readable";
  const installed = readable && path.data?.modelImporter.status === "installed";
  const absent = readable && path.data?.modelImporter.status === "requiredEntriesNotFound";
  const available = !update.isError && !update.isFetching && !update.data?.checkError && update.data?.available && !update.data.pinned;
  const changing = install.isPending || rollback.isPending || pin.isPending || retireEvacuation.isPending || retryEvacuation.isPending;
  const blocked = changing || (!installed && !absent) || evacuation.isError || evacuation.isFetching || evacuation.data !== null;
  const verifiedVersion = installed && !update.isError && !update.isFetching ? update.data?.installedVersion : null;
  const pathState = path.isPending || path.isFetching ? "Checking Game and Model Importer…"
    : path.isError ? "Cannot tell · Model Importer filesystem unverified"
    : installed ? `${importerLabel} installed · Game readable`
    : absent ? "Game readable · Model Importer not installed"
    : path.data?.gamePath.status === "noSavedPath" ? "Set a readable Game install path before installing a Model Importer."
    : path.data?.gamePath.status === "savedPathNotFound" ? "Saved installation cannot be found · Model Importer unverified"
    : "Cannot tell · Model Importer filesystem unverified";
  const filesystem = path.data?.modelImporter;

  return (
    <section className="card">
      <h2>
        Model Importer ({importerLabel})
        {available ? <span className="update-badge"> · update</span> : null}
      </h2>
      <p className="muted">
        Downloads the latest <code>{importerLabel}-Package</code> release for{" "}
        {displayName}, verifies the SHA-256, backs up any existing importer
        files, and rewrites <code>d3dx.ini</code>'s <code>loader</code> line
        to <code>gmm.exe</code>. Per ADR 0004 we never update without an
        explicit click here.
      </p>
      <p role="status">{pathState}</p>
      {filesystem?.status === "unverified" ? <p className="muted small">{filesystem.error}</p> : null}
      <CommandErrorNotice error={path.error} />
      <div className="row">
        <span className="muted small">
          Latest release: {release.isError ? "check failed" : release.isFetching ? "checking…" : release.data ? <code>{release.data.tag_name}</code> : "No release available"}
        </span>
        {verifiedVersion ? (
          <span className="muted small">
            · Installed <code>{verifiedVersion}</code>
          </span>
        ) : null}
        {verifiedVersion ? <span className="muted small"> · {update.data?.pinned ? "Pinned" : "Unpinned"}</span> : null}
      </div>
      <CommandErrorNotice
        error={release.error}
        heading="GMM could not check the latest Model Importer release."
      />
      <CommandErrorNotice
        error={update.error}
        heading="GMM could not check for a Model Importer update."
      />
      <CommandErrorNotice
        error={evacuation.error}
        heading="GMM could not validate Model Importer recovery state."
      />
      {evacuation.data ? (
        <ImporterEvacuationRecoveryWarning
          displayName={displayName}
          recovery={evacuation.data}
          pending={retireEvacuation.isPending || retryEvacuation.isPending}
          onRetry={() => retryEvacuation.mutate()}
          onRetire={() => retireEvacuation.mutate()}
        />
      ) : null}
      <div className="row">
        <button
          onClick={() => install.mutate()}
          disabled={blocked}
        >
          {install.isPending ? "Installing…" : available ? "Apply update" : installed ? "Reinstall importer" : "Install importer"}
        </button>
        <button
          onClick={() => rollback.mutate()}
          disabled={blocked}
        >
          {rollback.isPending ? "Rolling back…" : "Roll back importer"}
        </button>
        {verifiedVersion ? (
          <button
            onClick={() =>
              pin.mutate(update.data?.pinned ? null : update.data?.installedVersion ?? null)
            }
            disabled={blocked}
          >
            {pin.isPending ? "Saving pin…" : update.data?.pinned ? "Unpin" : "Pin to current"}
          </button>
        ) : null}
      </div>
      {update.isPending || update.isFetching ? <p role="status">Checking Model Importer update…</p>
        : !release.isError && !release.isFetching && !update.isError && !update.data?.checkError && installed ? <p role="status">
          {update.data?.pinned ? "Version prompts are suppressed by the Importer Pin."
            : available ? `Update available: ${update.data?.latestVersion}`
            : update.data?.latestVersion ? "Model Importer is current." : "No upstream version available."}
        </p> : null}
      <LoaderVersionNote />
      <CommandErrorNotice error={retireEvacuation.error} />
      <CommandErrorNotice error={retryEvacuation.error} />
      {pin.isSuccess ? <p role="status">Importer Pin saved.</p> : null}
      {retryEvacuation.isSuccess ? <p role="status">Recovery retry completed. See the refreshed recovery state above.</p> : null}
      {retireEvacuation.isSuccess ? <p role="status">Importer recovery block released; this does not claim files were restored.</p> : null}
      {rollback.isSuccess && rollback.data === null ? <p role="status">No backup is available to roll back to.</p> : null}
      {install.data ? (
        <p className="muted small">
          Installed. SHA-256 <code>{install.data.sha256.slice(0, 12)}…</code>{install.data.backup_dir ? <> · Backed up to <code>{install.data.backup_dir}</code></> : null}.
        </p>
      ) : null}
      {rollback.data ? (
        <p className="muted small">Restored from <code>{rollback.data}</code>.</p>
      ) : null}
      {update.data?.checkError ? (
        <p className="error">
          Couldn't check for an importer update: {update.data.checkError}
        </p>
      ) : null}
      <CommandErrorNotice error={install.error} />
      <CommandErrorNotice error={rollback.error} />
      <CommandErrorNotice error={pin.error} />
    </section>
  );
}
