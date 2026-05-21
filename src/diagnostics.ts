import { invoke } from "@tauri-apps/api/core";

/**
 * Forward a frontend error to the Tauri-side JSON-lines logger. Never
 * throws — diagnostics must not become a source of cascading errors.
 */
export async function logFrontendError(
  message: string,
  stack?: string,
  route?: string,
): Promise<void> {
  try {
    await invoke("log_frontend_error", {
      error: { message, stack, route },
    });
  } catch {
    // Swallow — the global handlers below would otherwise loop on us.
  }
}

/** Returns the on-disk log directory GMM writes to. */
export async function diagnosticsLogDir(): Promise<string> {
  return invoke<string>("diagnostics_log_dir");
}

/** Build the diagnostics bundle ZIP at `destPath`. */
export async function exportDiagnosticsBundle(
  logDir: string,
  destPath: string,
): Promise<void> {
  await invoke("export_diagnostics_bundle", { logDir, destPath });
}

/**
 * Install global handlers that forward `window.onerror` and
 * `unhandledrejection` events to the backend logger. Idempotent.
 */
export function installGlobalErrorHandlers(): void {
  if ((window as Window & { __gmmHandlersInstalled?: boolean }).__gmmHandlersInstalled) {
    return;
  }
  (window as Window & { __gmmHandlersInstalled?: boolean }).__gmmHandlersInstalled = true;

  window.addEventListener("error", (event) => {
    void logFrontendError(
      event.message ?? "window.onerror",
      event.error?.stack,
      window.location.pathname,
    );
  });
  window.addEventListener("unhandledrejection", (event) => {
    const reason = event.reason;
    const message =
      typeof reason === "string"
        ? reason
        : reason?.message ?? "unhandledrejection";
    void logFrontendError(message, reason?.stack, window.location.pathname);
  });
}
