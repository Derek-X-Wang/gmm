import { useEffect } from "react";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";

import type { LoaderVersionStatus, ProxyConfigPublic } from "./api";
import { renderWithQuery } from "./test/harness";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("./LibraryPathEditors", () => ({
  GlobalLibraryRootEditor: () => null,
  GameLibraryOverrideEditor: () => null,
}));
vi.mock("./UpdatesPanel", () => ({ UpdatesPanel: () => null }));
vi.mock("./ImporterRecommendations", () => ({ ImporterRecommendations: () => null }));
vi.mock("./Diagnostics.tsx", () => ({ Diagnostics: () => null }));

const { LoaderVersionNote } = await import("./LoaderVersionNote");
const { SettingsDestination } = await import("./SettingsDestination");

const upstreamAhead: LoaderVersionStatus = {
  shippedVersion: "v0.9.2",
  latestVersion: "v1.3.2",
  upstreamAhead: true,
  checkError: null,
};

let proxy: ProxyConfigPublic;
let loaderCheck: () => Promise<LoaderVersionStatus>;
let mounts = 0;

/**
 * The note and the Network panel live in different places in the real app —
 * the note under `ImporterPanel`, the panel in Settings — so the shared
 * QueryClient is the only thing that can connect them. The mount counter is
 * the point of this test: the note must recover while it stays mounted,
 * because a remount is exactly what relaunching GMM already does, and a test
 * that remounted would pass against the unfixed code (#496).
 */
function Surfaces() {
  useEffect(() => {
    mounts += 1;
  }, []);
  return (
    <>
      <section aria-label="Loader version">
        <LoaderVersionNote />
      </section>
      <SettingsDestination
        games={[]}
        page="gmm"
        returnGame={null}
        onPageChange={() => {}}
        onBack={() => {}}
      />
    </>
  );
}

function note() {
  return within(screen.getByRole("region", { name: "Loader version" }));
}

function loaderChecks() {
  return invoke.mock.calls.filter(([command]) => command === "check_loader_update").length;
}

beforeEach(() => {
  mounts = 0;
  proxy = { url: "http://dead.invalid:8080", username: null, passwordSet: false };
  loaderCheck = () => Promise.reject(new Error("proxy http://dead.invalid:8080 could not be reached"));
  invoke.mockReset();
  invoke.mockImplementation(async (command: string, payload?: { args?: ProxyConfigPublic }) => {
    if (command === "check_loader_update") return loaderCheck();
    if (command === "get_proxy_config") return proxy;
    if (command === "set_proxy_config") {
      proxy = { url: payload?.args?.url ?? null, username: payload?.args?.username ?? null, passwordSet: false };
      return proxy;
    }
    if (command === "get_app_version") return "0.0.0-test";
    throw new Error(`Unexpected command: ${command}`);
  });
});

it("retries a failed Loader check when the proxy configuration is saved, without being remounted", async () => {
  renderWithQuery(<Surfaces />);

  await waitFor(() =>
    expect(note().getByRole("status")).toHaveTextContent(/could not check the Loader version/i));
  expect(loaderChecks()).toBe(1);

  // The user removes the unreachable proxy and saves, exactly as on the
  // hardware round: the upstream read works again from here on.
  loaderCheck = () => Promise.resolve(upstreamAhead);
  const url = await screen.findByPlaceholderText("proxy URL");
  await userEvent.clear(url);
  await userEvent.click(within(url.closest("section")!).getByRole("button", { name: "Save" }));

  await waitFor(() => expect(note().getByRole("status")).toHaveTextContent("v1.3.2"));
  expect(note().getByRole("status")).toHaveTextContent(/GMM ships Loader/i);
  expect(note().getByRole("status")).not.toHaveTextContent(/could not check the Loader version/i);
  expect(loaderChecks()).toBe(2);
  expect(mounts).toBe(1);
});
