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

// The two halves of `networkMode` disagree on purpose; keep them that way (#310).
//
// Mutations: "always". Almost every mutation is a local filesystem, SQLite or
// process operation, and no mutation site anywhere checks `isPaused`. The
// deciding case is the Library card, which derives `busy` from
// `useIsMutating({ mutationKey: moveKey })`: a paused Library-root write
// disables every control in that card for as long as the webview claims to be
// offline, with no message at all. Mutation retry defaults to 0, so a
// genuinely network-bearing write now fails once and reports instead.
//
// Queries: the library default ("online"). A paused read is already rendered
// honestly by src/queryEvidence.ts, whose `refresh-paused` and `read-paused`
// reasons are live, exercised behaviour. "always" here would make those
// branches unreachable. Do not "tidy" this into a matching pair.
const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      refetchOnWindowFocus: false,
      staleTime: 1000 * 30,
    },
    mutations: {
      networkMode: "always",
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
