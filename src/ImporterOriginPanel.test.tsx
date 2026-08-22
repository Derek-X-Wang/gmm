import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";

import { renderWithQuery } from "./test/harness";
import type { ImporterOriginStatus } from "./api";

const importerOriginStatus = vi.fn();
const setImporterOriginOverride = vi.fn();
const acceptImporterOriginProposal = vi.fn();
const dismissImporterOrigin = vi.fn();
const restoreImporterOrigin = vi.fn();
const setImporterRecommendationsEnabled = vi.fn();

vi.mock("./api", async () => {
  const actual = await vi.importActual<typeof import("./api")>("./api");
  return {
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

  await screen.findByText("SilentNightSound/GIMI-Package");
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
