import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import App from "./App";
import { ErrorBoundary } from "./ErrorBoundary";
import { installGlobalErrorHandlers, installNetworkSignalLogging } from "./diagnostics";
import { updater } from "./updater";

installGlobalErrorHandlers();
const removeNetworkSignalLogging = installNetworkSignalLogging();
import.meta.hot?.dispose(removeNetworkSignalLogging);

// Startup checks only. Nothing downloads or installs without an explicit
// click in Settings; the status bar states what the check found (#357).
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
      </QueryClientProvider>
    </ErrorBoundary>
  </React.StrictMode>,
);
