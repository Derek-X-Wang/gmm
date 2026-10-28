import { invoke } from "./commandError";
import { updater } from "./updater";

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

/** Record the webview's connectivity signal without affecting app behavior. */
export async function logNetworkSignal(
  online: boolean,
  reason: "startup" | "online" | "offline",
): Promise<void> {
  try {
    await invoke("log_network_signal", { signal: { online, reason } });
  } catch {
    // Diagnostics must never throw into the app.
  }
}

let removeNetworkSignalListeners: (() => void) | undefined;

/** Observe once per app lifetime; page teardown and HMR remove the listeners. */
export function installNetworkSignalLogging(): () => void {
  if (typeof window === "undefined" || typeof navigator === "undefined") {
    return () => {};
  }
  if (removeNetworkSignalListeners) return removeNetworkSignalListeners;

  const onOnline = () => { void logNetworkSignal(true, "online"); };
  const onOffline = () => { void logNetworkSignal(false, "offline"); };
  const cleanup = () => {
    window.removeEventListener("online", onOnline);
    window.removeEventListener("offline", onOffline);
    window.removeEventListener("pagehide", cleanup);
    removeNetworkSignalListeners = undefined;
  };
  removeNetworkSignalListeners = cleanup;
  window.addEventListener("online", onOnline);
  window.addEventListener("offline", onOffline);
  window.addEventListener("pagehide", cleanup, { once: true });
  void logNetworkSignal(navigator.onLine, "startup");
  return cleanup;
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
  await invoke("export_diagnostics_bundle", { logDir, destPath, updaterError: updater.getLastError() });
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
