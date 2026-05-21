import React from "react";

import { logFrontendError } from "./diagnostics";

interface State {
  hasError: boolean;
  message?: string;
}

interface Props {
  children: React.ReactNode;
}

/**
 * Top-level error boundary. Catches render-time exceptions, forwards
 * them through the diagnostics logger (so they land in the same
 * JSON-lines stream as backend events with `source = frontend`), and
 * shows a minimal fallback UI the user can recover from by reloading.
 */
export class ErrorBoundary extends React.Component<Props, State> {
  state: State = { hasError: false };

  static getDerivedStateFromError(error: Error): State {
    return { hasError: true, message: error.message };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo): void {
    void logFrontendError(
      error.message,
      info.componentStack ?? error.stack,
      typeof window !== "undefined" ? window.location.pathname : undefined,
    );
  }

  render(): React.ReactNode {
    if (!this.state.hasError) return this.props.children;
    return (
      <main className="app">
        <section className="card">
          <h2>Something went wrong.</h2>
          <p className="muted">
            The error has been logged locally. Reload the window to continue,
            and use Settings → Diagnostics → Export bundle to attach the log
            to a bug report.
          </p>
          {this.state.message ? (
            <code className="muted small">{this.state.message}</code>
          ) : null}
        </section>
      </main>
    );
  }
}
