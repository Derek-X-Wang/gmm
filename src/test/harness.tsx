import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, type RenderOptions } from "@testing-library/react";
import type { ReactElement, ReactNode } from "react";

/**
 * Test-local QueryClient: retries off (so a rejected mutation surfaces
 * immediately instead of backing off for seconds) and no caching between
 * tests.
 *
 * `networkMode` matches src/main.tsx, including its deliberate asymmetry
 * (#310). Mutations use "always" because no mutation site checks `isPaused`
 * and the Library card's `useIsMutating` gate turns a paused local write into
 * a silent, indefinite lockup. Queries keep the library default so a paused
 * read still pauses, which is what src/queryEvidence.ts renders and what the
 * `onlineManager.setOnline(false)` tests across this suite exercise. Making
 * the two match would break both of those on purpose.
 */
export function makeQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: { retry: false, gcTime: 0, staleTime: 0 },
      mutations: { retry: false, networkMode: "always" },
    },
  });
}

function Wrapper({ children, client }: { children: ReactNode; client: QueryClient }) {
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

/** `render` with the TanStack Query provider already wired. */
export function renderWithQuery(
  ui: ReactElement,
  options?: RenderOptions & { client?: QueryClient },
) {
  const client = options?.client ?? makeQueryClient();
  return {
    client,
    ...render(ui, {
      wrapper: ({ children }) => <Wrapper client={client}>{children}</Wrapper>,
      ...options,
    }),
  };
}
