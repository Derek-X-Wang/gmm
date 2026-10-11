import { act, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";

import { renderWithQuery } from "./test/harness";
import type { ImporterOriginStatus } from "./api";

const importerOriginStatus = vi.fn();
const getGamePathStatus = vi.fn();
const setImporterOriginOverride = vi.fn();
const acceptImporterOriginProposal = vi.fn();
const dismissImporterOrigin = vi.fn();
const restoreImporterOrigin = vi.fn();
const setImporterRecommendationsEnabled = vi.fn();

vi.mock("./api", async () => {
  const actual = await vi.importActual<typeof import("./api")>("./api");
  return {
    getGamePathStatus: (...a: unknown[]) => getGamePathStatus(...a),
    originSlug: actual.originSlug,
    toOriginInput: actual.toOriginInput,
    importerOriginStatus: (...a: unknown[]) => importerOriginStatus(...a),
    setImporterOriginOverride: (...a: unknown[]) => setImporterOriginOverride(...a),
    acceptImporterOriginProposal: (...a: unknown[]) => acceptImporterOriginProposal(...a),
    dismissImporterOrigin: (...a: unknown[]) => dismissImporterOrigin(...a),
    restoreImporterOrigin: (...a: unknown[]) => restoreImporterOrigin(...a),
    setImporterRecommendationsEnabled: (...a: unknown[]) =>
      setImporterRecommendationsEnabled(...a),
  };
});

const { ImporterOriginPanel } = await import("./ImporterOriginPanel");

const gimiPackage = {
  kind: "gitHubRelease" as const,
  owner: "SilentNightSound",
  repo: "GIMI-Package",
  asset_pattern: "GIMI-PACKAGE-v\\d+\\.zip",
};
const theFork = {
  kind: "gitHubRelease" as const,
  owner: "Curated",
  repo: "GIMI-Fork",
  asset_pattern: "GIMI-PACKAGE-v\\d+\\.zip",
};

function status(overrides: Partial<ImporterOriginStatus> = {}): ImporterOriginStatus {
  return {
    game: "gimi",
    displayName: "Genshin Impact",
    resolved: { state: "inEffect", origin: gimiPackage, layer: "compiledInDefault" },
    installTarget: { state: "installed", ...gimiPackage },
    installed: { state: "known", ...gimiPackage },
    userOverride: { state: "notSet" },
    compiledDefault: gimiPackage,
    proposal: null,
    dismissed: [],
    dismissalsError: null,
    recommendationsEnabled: true,
    recommendationsUnusableReason: null,
    ...overrides,
  };
}

function render() {
  return renderWithQuery(
    <ImporterOriginPanel game="gimi" displayName="Genshin Impact" />,
  );
}

beforeEach(() => {
  importerOriginStatus.mockReset();
  getGamePathStatus.mockReset().mockResolvedValue({ game: "gimi", savedPath: "C:/Game", gamePath: { status: "readable" }, modelImporter: { status: "installed" } });
  setImporterOriginOverride.mockReset().mockResolvedValue(undefined);
  acceptImporterOriginProposal.mockReset().mockResolvedValue({
    backup_dir: "C:/backups/1",
    sha256: "abc",
    rewrote_files: [],
  });
  dismissImporterOrigin.mockReset().mockResolvedValue(undefined);
  restoreImporterOrigin.mockReset().mockResolvedValue(undefined);
  setImporterRecommendationsEnabled.mockReset().mockResolvedValue(undefined);
});

it("says which origin is in effect and which layer it came from", async () => {
  importerOriginStatus.mockResolvedValue(status());
  render();

  await screen.findByText(/Installed origin:/);
  const panel = screen.getByRole("region", { name: /importer origin/i });
  expect(panel).toHaveTextContent(/built-in default/i);
});

it("offers a proposed switch, says what accepting will do, and applies nothing on its own", async () => {
  importerOriginStatus.mockResolvedValue(
    status({
      proposal: {
        origin: theFork,
        reason: "The original package stopped receiving fixes.",
        replaces: { state: "known", ...gimiPackage },
      },
    }),
  );
  render();

  const proposal = await screen.findByRole("group", { name: /recommend/i });
  expect(proposal).toHaveTextContent("Curated/GIMI-Fork");
  // The reason is the difference between a prompt someone can evaluate
  // and one they dismiss on reflex.
  expect(proposal).toHaveTextContent("The original package stopped receiving fixes.");
  // The prompt has to say plainly that it rewrites the game directory.
  expect(proposal).toHaveTextContent(/replac/i);
  expect(proposal).toHaveTextContent(/back(s)? up|backup/i);

  expect(acceptImporterOriginProposal).not.toHaveBeenCalled();
  expect(setImporterOriginOverride).not.toHaveBeenCalled();
});

it("installs from the proposed origin when the user accepts", async () => {
  importerOriginStatus.mockResolvedValue(
    status({
      proposal: { origin: theFork, reason: null, replaces: { state: "unknown" } },
    }),
  );
  render();

  await userEvent.click(await screen.findByRole("button", { name: /switch and install/i }));

  await waitFor(() => expect(acceptImporterOriginProposal).toHaveBeenCalledWith("gimi"));
});

it("offers no way to record an origin without installing", async () => {
  importerOriginStatus.mockResolvedValue(
    status({
      proposal: { origin: theFork, reason: null, replaces: { state: "unknown" } },
    }),
  );
  render();

  await screen.findByRole("group", { name: /recommend/i });
  // Explicitly rejected in #109: it books an origin and a version for
  // files GMM has never seen, and everything downstream trusts that.
  expect(
    screen.queryByRole("button", { name: /just record|without installing|mark as/i }),
  ).not.toBeInTheDocument();
});

it("declines the origin the user was actually shown", async () => {
  importerOriginStatus.mockResolvedValue(
    status({
      proposal: { origin: theFork, reason: null, replaces: { state: "unknown" } },
    }),
  );
  render();

  await userEvent.click(await screen.findByRole("button", { name: /not now/i }));

  await waitFor(() =>
    expect(dismissImporterOrigin).toHaveBeenCalledWith("gimi", {
      owner: "Curated",
      repo: "GIMI-Fork",
      assetPattern: "GIMI-PACKAGE-v\\d+\\.zip",
    }),
  );
  expect(acceptImporterOriginProposal).not.toHaveBeenCalled();
});

it("renders a structured failure when declining a recommendation fails", async () => {
  dismissImporterOrigin.mockRejectedValue({
    kind: "invalidActiveVariant",
    message: "Could not save the dismissal.",
  });
  importerOriginStatus.mockResolvedValue(
    status({
      proposal: { origin: theFork, reason: null, replaces: { state: "unknown" } },
    }),
  );
  render();

  await userEvent.click(await screen.findByRole("button", { name: /not now/i }));

  const failure = await screen.findByText("Could not save the dismissal.");
  expect(failure).toHaveAttribute(
    "data-command-failure-kind",
    "invalidActiveVariant",
  );
});

it("shows dismissed origins on the game's own surface and can undo one", async () => {
  importerOriginStatus.mockResolvedValue(status({ dismissed: [theFork] }));
  render();

  const dismissed = await screen.findByRole("group", { name: /dismissed/i });
  expect(dismissed).toHaveTextContent("Curated/GIMI-Fork");

  await userEvent.click(await screen.findByRole("button", { name: /undo/i }));
  await waitFor(() =>
    expect(restoreImporterOrigin).toHaveBeenCalledWith("gimi", {
      owner: "Curated",
      repo: "GIMI-Fork",
      assetPattern: "GIMI-PACKAGE-v\\d+\\.zip",
    }),
  );
});

it("renders a structured failure when undoing a dismissal fails", async () => {
  restoreImporterOrigin.mockRejectedValue({
    kind: "invalidActiveVariant",
    message: "Could not restore the recommendation.",
  });
  importerOriginStatus.mockResolvedValue(status({ dismissed: [theFork] }));
  render();

  await userEvent.click(await screen.findByRole("button", { name: /undo/i }));

  const failure = await screen.findByText("Could not restore the recommendation.");
  expect(failure).toHaveAttribute(
    "data-command-failure-kind",
    "invalidActiveVariant",
  );
});

it("warns without blocking when no origin is in effect, and still offers the override", async () => {
  importerOriginStatus.mockResolvedValue(
    status({
      resolved: {
        state: "noneInEffect",
        reason: "No maintained package is known right now.",
      },
      installTarget: {
        state: "noneInEffect",
        reason: "No maintained package is known right now.",
      },
      installed: { state: "unknown" },
      compiledDefault: null,
    }),
  );
  render();

  const warning = await screen.findByRole("status", { name: /no importer origin/i });
  expect(warning).toHaveTextContent("No maintained package is known right now.");
  expect(warning).toHaveTextContent(/supply|set|choose/i);

  // Not a block: the control that fixes it is right there and enabled.
  expect(screen.getByLabelText(/owner/i)).toBeEnabled();
  expect(screen.getByRole("button", { name: /save origin/i })).toBeEnabled();
});

it("sets a per-game override from what the user typed", async () => {
  importerOriginStatus.mockResolvedValue(status());
  render();

  await userEvent.type(await screen.findByLabelText(/owner/i), "me");
  await userEvent.type(screen.getByLabelText(/repository/i), "my-GIMI");
  await userEvent.type(screen.getByLabelText(/asset pattern/i), "PKG-v1.zip");
  await userEvent.click(screen.getByRole("button", { name: /save origin/i }));

  await waitFor(() =>
    expect(setImporterOriginOverride).toHaveBeenCalledWith("gimi", {
      owner: "me",
      repo: "my-GIMI",
      assetPattern: "PKG-v1.zip",
    }),
  );
});

it("clears the override back to following the recommendation", async () => {
  importerOriginStatus.mockResolvedValue(
    status({
      userOverride: { state: "set", ...theFork },
      resolved: { state: "inEffect", origin: theFork, layer: "userOverride" },
    }),
  );
  render();

  await userEvent.click(await screen.findByRole("button", { name: /clear override/i }));

  await waitFor(() =>
    expect(setImporterOriginOverride).toHaveBeenCalledWith("gimi", null),
  );
});

it("surfaces an override GMM cannot read instead of showing an empty box", async () => {
  importerOriginStatus.mockResolvedValue(
    status({
      userOverride: {
        state: "unreadable",
        raw: '{"kind":"localZip"}',
        error: "unknown variant",
      },
      resolved: { state: "noneInEffect", reason: "GMM could not read it." },
      installTarget: { state: "noneInEffect", reason: "GMM could not read it." },
    }),
  );
  render();

  expect(await screen.findByText(/unknown variant/i)).toBeInTheDocument();
});

it("surfaces dismissal state GMM cannot read rather than reporting none", async () => {
  importerOriginStatus.mockResolvedValue(
    status({ dismissalsError: "expected value at line 1 column 1" }),
  );
  render();

  expect(
    await screen.findByText(/expected value at line 1 column 1/i),
  ).toBeInTheDocument();
});

it("switches recommendations off for every game from one control", async () => {
  importerOriginStatus.mockResolvedValue(status());
  render();

  const toggle = await screen.findByRole("checkbox", { name: /recommend/i });
  expect(toggle).toBeChecked();
  await userEvent.click(toggle);

  await waitFor(() =>
    expect(setImporterRecommendationsEnabled).toHaveBeenCalledWith(false),
  );
});

it("renders a structured failure when the recommendations switch fails", async () => {
  setImporterRecommendationsEnabled.mockRejectedValue({
    kind: "invalidActiveVariant",
    message: "Could not change recommendation settings.",
  });
  importerOriginStatus.mockResolvedValue(status());
  render();

  await userEvent.click(
    await screen.findByRole("checkbox", { name: /recommend/i }),
  );

  const failure = await screen.findByText(
    "Could not change recommendation settings.",
  );
  expect(failure).toHaveAttribute(
    "data-command-failure-kind",
    "invalidActiveVariant",
  );
});

it("offers no prompt and no dismissal list while recommendations are off", async () => {
  importerOriginStatus.mockResolvedValue(
    status({ recommendationsEnabled: false, proposal: null, dismissed: [] }),
  );
  render();

  await screen.findByRole("button", { name: /save origin/i });
  expect(screen.queryByRole("group", { name: /recommend/i })).not.toBeInTheDocument();
  expect(screen.queryByRole("group", { name: /dismissed/i })).not.toBeInTheDocument();
  // The user's own origin is untouched by the switch, so the editor stays.
  expect(screen.getByRole("button", { name: /save origin/i })).toBeEnabled();
});


it("refreshes a rejected Origin install and keeps its error when the proposal disappears", async () => {
  importerOriginStatus.mockResolvedValue(status({
    proposal: { origin: theFork, reason: null, replaces: { state: "unknown" } },
  }));
  acceptImporterOriginProposal.mockImplementation(async () => {
    importerOriginStatus.mockResolvedValue(status({ proposal: null }));
    throw new Error("Origin install failed");
  });
  render();
  await userEvent.click(await screen.findByRole("button", { name: "Switch and install" }));
  await waitFor(() => expect(screen.queryByRole("button", { name: "Switch and install" })).not.toBeInTheDocument());
  expect(screen.getByText("Origin install failed")).toBeInTheDocument();
});

it.each([false, true])("refreshes the live path-state after an Origin installation settles (rejects: %s)", async (rejects) => {
  importerOriginStatus.mockResolvedValue(status({ proposal: { origin: theFork, reason: null, replaces: { state: "unknown" } } }));
  if (rejects) acceptImporterOriginProposal.mockRejectedValue(new Error("Origin install failed"));
  const { client } = render();
  client.setQueryDefaults(["gamePathStatus"], { gcTime: 300_000, staleTime: 30_000 });
  await userEvent.click(await screen.findByRole("button", { name: /switch and install/i }));
  await waitFor(() => expect(getGamePathStatus).toHaveBeenCalledTimes(2));
});

it("shows loading rather than a successful empty Origin result", async () => {
  importerOriginStatus.mockImplementation(() => new Promise(() => {}));
  render();
  expect(screen.getByText("Reading Importer Origin…")).toBeVisible();
  expect(screen.queryByText("No Origin Proposal.")).not.toBeInTheDocument();
});
it("shows the Origin read error when no proposal can be read", async () => {
  importerOriginStatus.mockRejectedValue(new Error("Origin query unavailable"));
  render();
  expect(await screen.findByText("Origin query unavailable")).toBeVisible();
  expect(screen.getByText("Importer Origin unavailable.")).toBeVisible();
  expect(screen.queryByText("No Origin Proposal.")).not.toBeInTheDocument();
});
it.each(["userOverride", "recommendedManifest", "compiledInDefault"] as const)("shows the %s precedence and distinguishes installed origin from a future target", async (layer) => {
  importerOriginStatus.mockResolvedValue(status({ resolved: { state: "inEffect", origin: theFork, layer } }));
  render();
  expect(await screen.findByText(/Installed origin:/)).toHaveTextContent("SilentNightSound/GIMI-Package");
  expect(screen.getByText(/Future install target after an explicit origin change:/)).toHaveTextContent("Curated/GIMI-Fork");
  expect(screen.getByText(/Updates stay on/)).toHaveTextContent("SilentNightSound/GIMI-Package");
});
it("keeps unknown installed origin neutral and distinct from not installed", async () => {
  importerOriginStatus.mockResolvedValue(status({ installed: { state: "unknown" }, proposal: null }));
  render();
  expect(await screen.findByText("Installed origin: unknown (not recorded).")).toBeVisible();
  expect(screen.queryByText(/not installed/)).not.toBeInTheDocument();
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});
it("keeps unreadable installed origin and unusable recommendation errors visible with no proposal", async () => {
  importerOriginStatus.mockResolvedValue(status({
    installed: { state: "unreadable", raw: "old-record", error: "Installed origin corrupt" },
    installTarget: { state: "installedUnreadable", raw: "old-record", error: "Installed origin corrupt" },
    recommendationsUnusableReason: "Unsupported manifest", proposal: null,
  }));
  render();
  expect(await screen.findByText(/Installed origin unreadable:/)).toHaveTextContent("Installed origin corrupt");
  expect(screen.getByText(/Unsupported manifest/)).toBeVisible();
  expect(screen.getByText(/will not install over it/)).toBeVisible();
});
it("distinguishes empty override, proposal and dismissal states", async () => {
  importerOriginStatus.mockResolvedValue(status());
  render();
  expect(await screen.findByText("No override set.")).toBeVisible();
  expect(screen.getByText("No Origin Proposal.")).toBeVisible();
  expect(screen.getByText("No dismissed recommendations.")).toBeVisible();
  expect(screen.queryByRole("button", { name: /Clear override/ })).not.toBeInTheDocument();
});
it.each(["accept", "decline", "undo", "save", "clear", "recommendations"])("shows %s pending and outcome without losing errors to a disappearing proposal", async (action) => {
  const config = status({
    proposal: { origin: theFork, reason: "New maintained origin", replaces: { state: "known", ...gimiPackage } },
    dismissed: [theFork], userOverride: { state: "set", ...theFork },
  });
  importerOriginStatus.mockResolvedValue(config);
  const mock = { accept: acceptImporterOriginProposal, decline: dismissImporterOrigin, undo: restoreImporterOrigin, save: setImporterOriginOverride, clear: setImporterOriginOverride, recommendations: setImporterRecommendationsEnabled }[action]!;
  let resolve!: (value: unknown) => void;
  mock.mockImplementation(() => new Promise((yes) => { resolve = yes; }));
  render();
  const labels: Record<string, string> = { accept: "Switch and install", decline: "Not now", undo: "Undo", save: "Save origin", clear: "Clear override" };
  await userEvent.click(action === "recommendations" ? await screen.findByRole("checkbox", { name: /recommend/i }) : await screen.findByRole("button", { name: labels[action] }));
  const pending: Record<string, string> = { accept: "Switching…", decline: "Declining…", undo: "Restoring…", save: "Saving…", clear: "Clearing…" };
  expect(action === "recommendations" ? screen.getByRole("checkbox", { name: /recommend/i }) : screen.getByRole("button", { name: pending[action] })).toBeDisabled();
  importerOriginStatus.mockResolvedValue(status());
  resolve({ sha256: "abc", backup_dir: null });
  const outcomes: Record<string, string> = { accept: "Origin installation completed.", decline: "Recommendation declined.", undo: "Recommendation restored.", save: "Origin override saved.", clear: "Origin override cleared.", recommendations: "Recommendations preference saved." };
  expect(await screen.findByText(outcomes[action])).toBeVisible();
});
it.each(["decline", "undo", "save", "clear"])("preserves the atomic-success invalidation policy for rejected %s", async (action) => {
  importerOriginStatus.mockResolvedValue(status({ proposal: { origin: theFork, reason: null, replaces: { state: "unknown" } }, dismissed: [theFork], userOverride: { state: "set", ...theFork } }));
  const mock = { decline: dismissImporterOrigin, undo: restoreImporterOrigin, save: setImporterOriginOverride, clear: setImporterOriginOverride }[action]!;
  mock.mockRejectedValue(new Error("Atomic Origin write rejected"));
  const { client } = render();
  const keys = [["importer", "latest", "gimi"]];
  for (const key of keys) { client.setQueryDefaults(key, { gcTime: 300_000, staleTime: 30_000 }); client.setQueryData(key, "cached"); }
  const labels: Record<string, string> = { decline: "Not now", undo: "Undo", save: "Save origin", clear: "Clear override" };
  await userEvent.click(await screen.findByRole("button", { name: labels[action] }));
  expect(await screen.findByText("Atomic Origin write rejected")).toBeVisible();
  for (const key of keys) expect(client.getQueryState(key)?.isInvalidated).toBe(false);
  expect(getGamePathStatus).toHaveBeenCalledTimes(1);
});
it.each(["decline", "undo", "save", "clear"])("refreshes all importer reads and this Game's path and Origin after atomic %s success", async (action) => {
  importerOriginStatus.mockResolvedValue(status({ proposal: { origin: theFork, reason: null, replaces: { state: "unknown" } }, dismissed: [theFork], userOverride: { state: "set", ...theFork } }));
  const { client } = render();
  const keys = [["importer", "latest", "srmi"]];
  for (const key of keys) { client.setQueryDefaults(key, { gcTime: 300_000, staleTime: 30_000 }); client.setQueryData(key, "cached"); }
  const labels: Record<string, string> = { decline: "Not now", undo: "Undo", save: "Save origin", clear: "Clear override" };
  await userEvent.click(await screen.findByRole("button", { name: labels[action] }));
  await waitFor(() => { for (const key of keys) expect(client.getQueryState(key)?.isInvalidated).toBe(true); });
  expect(importerOriginStatus).toHaveBeenCalledTimes(2);
  expect(getGamePathStatus).toHaveBeenCalledTimes(2);
});

it.each(["checking", "error", "unreadable", "noSavedPath", "unverified importer"])("does not invite an Origin installation over %s filesystem state", async (state) => {
  importerOriginStatus.mockResolvedValue(status({ proposal: { origin: theFork, reason: null, replaces: { state: "unknown" } } }));
  if (state === "checking") getGamePathStatus.mockImplementation(() => new Promise(() => {}));
  else if (state === "error") getGamePathStatus.mockRejectedValue(new Error("Path cannot be checked"));
  else getGamePathStatus.mockResolvedValue({
    game: "gimi", savedPath: "C:/Game",
    gamePath: { status: state === "unverified importer" ? "readable" : state },
    modelImporter: { status: "unverified", path: "C:/Game", error: "Cannot tell" },
  });
  render();
  expect(await screen.findByRole("button", { name: "Switch and install" })).toBeDisabled();
  expect(acceptImporterOriginProposal).not.toHaveBeenCalled();
  expect(screen.getByRole("button", { name: "Save origin" })).toBeEnabled();
});

it.each(["absent", "checking", "failed", "unverified"])("qualifies recorded Origin when importer files are %s", async (state) => {
  importerOriginStatus.mockResolvedValue(status());
  if (state === "checking") getGamePathStatus.mockImplementation(() => new Promise(() => {}));
  else if (state === "failed") getGamePathStatus.mockRejectedValue(new Error("Path check failed"));
  else getGamePathStatus.mockResolvedValue({
    game: "gimi", savedPath: "C:/Game", gamePath: { status: "readable" },
    modelImporter: state === "absent" ? { status: "requiredEntriesNotFound", paths: ["C:/Game/Core"] }
      : { status: "unverified", path: "C:/Game/Core", error: "Access denied" },
  });
  const { client } = render();
  await screen.findByRole("button", { name: "Save origin" });
  if (state !== "checking") await waitFor(() => expect(client.isFetching()).toBe(0));
  expect(screen.queryByText(/^Installed origin:/)).not.toBeInTheDocument();
  expect(screen.getByText(/Recorded origin:/)).toHaveTextContent("SilentNightSound/GIMI-Package");
  expect(screen.getByText(/Recorded origin:/)).toHaveTextContent(state === "absent" ? "Model Importer files not installed" : "Model Importer files unverified");
});

function deferredOriginCheck() {
  let resolve!: (value: unknown) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

it("suppresses all confident empty Origin claims during and after a failed refresh", async () => {
  importerOriginStatus.mockResolvedValue(status());
  const { client } = render();
  await screen.findByText("No Origin Proposal.");
  expect(screen.getByText("No dismissed recommendations.")).toBeVisible();
  expect(screen.getByText("No override set.")).toBeVisible();
  const pending = deferredOriginCheck();
  importerOriginStatus.mockReturnValue(pending.promise);
  let refresh!: Promise<void>;
  act(() => { refresh = client.invalidateQueries({ queryKey: ["importerOrigin", "gimi"] }); });
  await screen.findByText("The Origin details below are from the previous check.");
  for (const label of ["No Origin Proposal.", "No dismissed recommendations.", "No override set."]) {
    expect(screen.queryByText(label)).not.toBeInTheDocument();
  }
  await act(async () => { pending.reject(new Error("Origin refresh failed")); await refresh; });
  expect(await screen.findByText("Origin refresh failed")).toBeVisible();
  expect(await screen.findByText("The Origin details below are from the previous check.")).toBeVisible();
  for (const label of ["No Origin Proposal.", "No dismissed recommendations.", "No override set."]) {
    expect(screen.queryByText(label)).not.toBeInTheDocument();
  }
  importerOriginStatus.mockResolvedValue(status());
  await act(() => client.invalidateQueries({ queryKey: ["importerOrigin", "gimi"] }));
  expect(await screen.findByText("No Origin Proposal.")).toBeVisible();
  expect(screen.getByText("No dismissed recommendations.")).toBeVisible();
  expect(screen.getByText("No override set.")).toBeVisible();
});

it("keeps a retained Origin Proposal labelled as previous and disables install until a successful refresh", async () => {
  importerOriginStatus.mockResolvedValue(status({ proposal: { origin: theFork, reason: null, replaces: { state: "unknown" } } }));
  const { client } = render();
  await waitFor(() => expect(screen.getByRole("button", { name: "Switch and install" })).toBeEnabled());
  const pending = deferredOriginCheck();
  importerOriginStatus.mockReturnValue(pending.promise);
  let refresh!: Promise<void>;
  act(() => { refresh = client.invalidateQueries({ queryKey: ["importerOrigin", "gimi"] }); });
  expect(await screen.findByText("The Origin details below are from the previous check.")).toBeVisible();
  expect(screen.getByRole("button", { name: "Switch and install" })).toBeDisabled();
  await act(async () => { pending.reject(new Error("Origin refresh failed")); await refresh; });
  expect(await screen.findByText("Origin refresh failed")).toBeVisible();
  expect(await screen.findByText("The Origin details below are from the previous check.")).toBeVisible();
  expect(screen.getByRole("button", { name: "Switch and install" })).toBeDisabled();
  importerOriginStatus.mockResolvedValue(status({ proposal: { origin: theFork, reason: null, replaces: { state: "unknown" } } }));
  await act(() => client.invalidateQueries({ queryKey: ["importerOrigin", "gimi"] }));
  await waitFor(() => expect(screen.getByRole("button", { name: "Switch and install" })).toBeEnabled());
});

it("holds Origin install while retained readable path data is rechecked", async () => {
  importerOriginStatus.mockResolvedValue(status({ proposal: { origin: theFork, reason: null, replaces: { state: "unknown" } } }));
  const { client } = render();
  await waitFor(() => expect(screen.getByRole("button", { name: "Switch and install" })).toBeEnabled());
  const pending = deferredOriginCheck();
  getGamePathStatus.mockReturnValue(pending.promise);
  let refresh!: Promise<void>;
  act(() => { refresh = client.invalidateQueries({ queryKey: ["gamePathStatus", "gimi"] }); });
  await waitFor(() => expect(screen.getByRole("button", { name: "Switch and install" })).toBeDisabled());
  await userEvent.click(screen.getByRole("button", { name: "Switch and install" }));
  expect(acceptImporterOriginProposal).not.toHaveBeenCalled();
  await act(async () => {
    pending.resolve({ game: "gimi", savedPath: "C:/Game", gamePath: { status: "readable" }, modelImporter: { status: "installed" } });
    await refresh;
  });
  await waitFor(() => expect(screen.getByRole("button", { name: "Switch and install" })).toBeEnabled());
});

it("names the resolved origin as the ordinary install target before any origin has been installed", async () => {
  importerOriginStatus.mockResolvedValue(status({
    installed: { state: "unknown" },
    installTarget: { state: "resolved", origin: theFork, layer: "recommendedManifest" },
    resolved: { state: "inEffect", origin: theFork, layer: "recommendedManifest" },
  }));
  render();
  await screen.findByRole("button", { name: "Save origin" });
  expect(screen.queryByText(/Future install target after an explicit origin change:/)).not.toBeInTheDocument();
  expect(screen.getByText(/Install target:/)).toHaveTextContent("Curated/GIMI-Fork");
});
