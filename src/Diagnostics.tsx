import { useMutation, useQuery } from "@tanstack/react-query";
import { save } from "@tauri-apps/plugin-dialog";
import { diagnosticsLogDir, exportDiagnosticsBundle } from "./diagnostics";
import { CommandErrorNotice } from "./CommandErrorNotice";

/**
 * Settings → Diagnostics panel. Shows the on-disk log directory and
 * exposes a single "Export diagnostics bundle" button. Bundle export is
 * always user-initiated; there is no background uploader.
 */
export function Diagnostics() {
  const logDir = useQuery({
    queryKey: ["diagnostics", "logDir"],
    queryFn: diagnosticsLogDir,
  });

  const exportBundle = useMutation({
    meta: { noInvalidationReason: "Writes a user-selected diagnostics artifact; displayed log directory does not change." },
    mutationFn: async () => {
      const dir = logDir.data;
      if (!dir) throw new Error("log directory not yet known");
      const dest = await save({
        defaultPath: "gmm-diagnostics.zip",
        filters: [{ name: "ZIP archive", extensions: ["zip"] }],
      });
      if (typeof dest !== "string") return null;
      await exportDiagnosticsBundle(dir, dest);
      return dest;
    },
  });

  return (
    <section className="card">
      <h2>Diagnostics</h2>
      <p className="muted">
        Logs stay on this machine. No telemetry, no background uploads. Use
        the bundle export to attach local logs and redacted settings to a bug
        report.
      </p>
      {logDir.isPending ? <p role="status">Reading log directory…</p> : null}
      {logDir.isError ? <p role="status">Log directory unavailable.</p> : null}
      {logDir.data && (logDir.isFetching || logDir.isError) ? <p className="muted small">The log directory below is from the previous check.</p> : null}
      <CommandErrorNotice error={logDir.error} />
      <div className="row">
        <input className="path" aria-label="Log directory" value={logDir.data ?? ""} readOnly placeholder={logDir.isError ? "unavailable" : "resolving…"} />
        <button onClick={() => exportBundle.mutate()} disabled={exportBundle.isPending || !logDir.isSuccess || logDir.isFetching}>
          {exportBundle.isPending ? "Bundling…" : "Export diagnostics bundle"}
        </button>
      </div>
      {exportBundle.isSuccess && exportBundle.data === null ? <p role="status">Export cancelled.</p> : null}
      {exportBundle.data ? (
        <p className="muted small">Saved bundle to <code>{exportBundle.data}</code>.</p>
      ) : null}
      <CommandErrorNotice error={exportBundle.error} />
    </section>
  );
}
