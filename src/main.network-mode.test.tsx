import { MutationObserver, QueryClient, QueryObserver, onlineManager } from "@tanstack/react-query";
import { isValidElement } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { makeQueryClient } from "./test/harness";

// #310. GMM's two clients deliberately disagree about `networkMode`, in one
// direction only: "always" for mutations, the library default for queries.
// These tests read the real client objects the entry and the harness build, so
// reverting either half of the asymmetry fails here by name.

const { invoke, rendered } = vi.hoisted(() => ({ invoke: vi.fn(), rendered: [] as unknown[] }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("./updater", () => ({ updater: { checkForUpdates: vi.fn() } }));
vi.mock("./App", () => ({ default: () => null }));
vi.mock("./diagnostics", () => ({
  installGlobalErrorHandlers: vi.fn(),
  installNetworkSignalLogging: vi.fn(() => () => {}),
}));
vi.mock("react-dom/client", () => ({
  default: { createRoot: () => ({ render: (node: unknown) => { rendered.push(node); } }) },
}));

// Start the real entry once. The client under test is the object it hands to
// its own provider, not a copy of its options.
await import("./main");

function findClient(node: unknown): QueryClient | undefined {
  if (!isValidElement(node)) return undefined;
  const props = node.props as { client?: unknown; children?: unknown };
  if (props.client instanceof QueryClient) return props.client;
  const children = Array.isArray(props.children) ? props.children : [props.children];
  for (const child of children) {
    const found = findClient(child);
    if (found) return found;
  }
  return undefined;
}

function productionClient(): QueryClient {
  const client = rendered.map(findClient).find((candidate) => candidate !== undefined);
  expect(client, "the app entry renders a QueryClientProvider").toBeInstanceOf(QueryClient);
  return client!;
}

/** One local write, through `client`'s own defaults, awaited to its outcome. */
function runLocalWrite(client: QueryClient): Promise<string> {
  const observer = new MutationObserver<string, Error, void>(client, {
    mutationFn: async () => "written",
  });
  return observer.mutate();
}

afterEach(() => {
  onlineManager.setOnline(true);
});

it("runs a local mutation to completion through the production client while the online manager reports offline", async () => {
  const client = productionClient();
  onlineManager.setOnline(false);

  // Under the library default this promise never settles: the write is paused,
  // which is the LibraryPathEditors lockup.
  await expect(runLocalWrite(client)).resolves.toBe("written");
  expect(client.isMutating()).toBe(0);
});

it("leaves the production client's queries on the default network mode, so a paused read is still paused", async () => {
  const client = productionClient();
  onlineManager.setOnline(false);

  const observer = new QueryObserver<string, Error>(client, {
    queryKey: ["networkModeProbe"],
    queryFn: async () => "read",
  });
  const unsubscribe = observer.subscribe(() => {});
  try {
    // queryEvidence.ts renders this state honestly, and that path is live.
    await vi.waitFor(() => expect(observer.getCurrentResult().fetchStatus).toBe("paused"));
    expect(observer.getCurrentResult().status).toBe("pending");
  } finally {
    unsubscribe();
  }
});

it("keeps the test harness and the production client in agreement on the mutation network mode", () => {
  const production = productionClient().getDefaultOptions();
  const harness = makeQueryClient().getDefaultOptions();

  expect(production.mutations?.networkMode).toBe("always");
  expect(harness.mutations?.networkMode).toBe(production.mutations?.networkMode);
  // And in agreement on leaving queries alone.
  expect(production.queries?.networkMode).toBeUndefined();
  expect(harness.queries?.networkMode).toBeUndefined();
});

it("runs a local mutation to completion through the test harness client while the online manager reports offline", async () => {
  const client = makeQueryClient();
  onlineManager.setOnline(false);

  await expect(runLocalWrite(client)).resolves.toBe("written");
  expect(client.isMutating()).toBe(0);
});
