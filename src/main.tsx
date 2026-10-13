import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import App from "./App";
import { ErrorBoundary } from "./ErrorBoundary";
import { installGlobalErrorHandlers } from "./diagnostics";
import { updater } from "./updater";
import { StartupUpdateNotice } from "./UpdatesPanel";

installGlobalErrorHandlers();

// Startup checks only. The global notice asks before downloading or installing.
void updater.checkForUpdates(true);

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      refetchOnWindowFocus: false,
      staleTime: 1000 * 30,
    },
  },
});

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      <QueryClientProvider client={queryClient}>
        <App />
        <StartupUpdateNotice />
      </QueryClientProvider>
    </ErrorBoundary>
  </React.StrictMode>,
);
