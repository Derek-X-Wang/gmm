import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { renderWithQuery } from "./test/harness";

// Mock the whole IPC surface the wizard imports. Every test drives the
// wizard purely through these, so nothing here can reach a real backend.
const detectAllGames = vi.fn();
const getGameInstallPath = vi.fn();
const getLibraryPaths = vi.fn();
const installImporter = vi.fn();
const listSupportedGames = vi.fn();
const markOnboardingComplete = vi.fn();
const setGameInstallPath = vi.fn();
const setLibraryRoot = vi.fn();
const avGuidance = vi.fn();
const openDialog = vi.fn();

vi.mock("./api", () => ({
  detectAllGames: (...a: unknown[]) => detectAllGames(...a),
  getGameInstallPath: (...a: unknown[]) => getGameInstallPath(...a),
  getLibraryPaths: (...a: unknown[]) => getLibraryPaths(...a),
  installImporter: (...a: unknown[]) => installImporter(...a),
  listSupportedGames: (...a: unknown[]) => listSupportedGames(...a),
  markOnboardingComplete: (...a: unknown[]) => markOnboardingComplete(...a),
  setGameInstallPath: (...a: unknown[]) => setGameInstallPath(...a),
  setLibraryRoot: (...a: unknown[]) => setLibraryRoot(...a),
  avGuidance: (...a: unknown[]) => avGuidance(...a),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: (...a: unknown[]) => openDialog(...a),
}));

const { OnboardingWizard } = await import("./OnboardingWizard");

beforeEach(() => {
  vi.clearAllMocks();
  avGuidance.mockResolvedValue({
    headline: "Windows may flag GMM as unknown",
    body: "We don't ship code signing yet.",
    exclusionSteps: ["Open Windows Security", "Add an exclusion"],
    docPath: "docs/antivirus-and-smartscreen.md",
    launchErrorPrefix: "AV_BLOCKED:",
  });
  detectAllGames.mockResolvedValue([
    { code: "gimi", displayName: "Genshin Impact", detectedPath: "C:\\Games\\Genshin" },
    { code: "srmi", displayName: "Star Rail", detectedPath: null },
  ]);
  listSupportedGames.mockResolvedValue([
    { code: "gimi", displayName: "Genshin Impact" },
    { code: "srmi", displayName: "Star Rail" },
  ]);
  getLibraryPaths.mockResolvedValue({
    defaultRoot: "C:\\Users\\me\\AppData\\Roaming\\GMM\\library",
    rootOverride: null,
    effectiveRoot: "C:\\Users\\me\\AppData\\Roaming\\GMM\\library",
    perGameOverrides: {},
    perGameEffective: {},
  });
  getGameInstallPath.mockResolvedValue(null);
  markOnboardingComplete.mockResolvedValue(undefined);
  setGameInstallPath.mockResolvedValue(undefined);
  setLibraryRoot.mockResolvedValue(undefined);
  installImporter.mockResolvedValue({ backupDir: null, sha256: "abc", rewroteFiles: [] });
});

/**
 * The AV acknowledgement checkbox on step 1. It only mounts once the
 * `av_guidance` query resolves, so this is async by necessity.
 */
function avCheckbox() {
  return screen.findByRole("checkbox");
}

function continueButton() {
  return screen.getByRole("button", { name: /continue/i });
}

/** Tick the AV box and walk forward to step `n`. */
async function advanceToStep(n: 2 | 3 | 4) {
  await userEvent.click(await avCheckbox());
  for (let i = 1; i < n; i += 1) {
    await userEvent.click(continueButton());
  }
}

describe("OnboardingWizard — step 1 gating", () => {
  it("starts on step 1 of 4", async () => {
    renderWithQuery(<OnboardingWizard onDone={vi.fn()} />);
    expect(await screen.findByText(/step 1 of 4/i)).toBeInTheDocument();
  });

  it("disables Continue until the AV note is acknowledged", async () => {
    renderWithQuery(<OnboardingWizard onDone={vi.fn()} />);

    expect(continueButton()).toBeDisabled();

    await userEvent.click(await avCheckbox());

    expect(continueButton()).toBeEnabled();
  });

  it("re-disables Continue if the user un-ticks the acknowledgement", async () => {
    renderWithQuery(<OnboardingWizard onDone={vi.fn()} />);

    await userEvent.click(await avCheckbox());
    expect(continueButton()).toBeEnabled();

    await userEvent.click(await avCheckbox());
    expect(continueButton()).toBeDisabled();
  });

  it("renders the AV guidance headline fetched from the backend", async () => {
    renderWithQuery(<OnboardingWizard onDone={vi.fn()} />);
    expect(
      await screen.findByText(/windows may flag gmm as unknown/i),
    ).toBeInTheDocument();
  });
});

describe("OnboardingWizard — navigation", () => {
  it("advances to the detection step and runs a scan", async () => {
    renderWithQuery(<OnboardingWizard onDone={vi.fn()} />);

    await advanceToStep(2);

    expect(await screen.findByText(/step 2 of 4/i)).toBeInTheDocument();
    await waitFor(() => expect(detectAllGames).toHaveBeenCalled());
  });

  it("shows a Back button from step 2 onward but not on step 1", async () => {
    renderWithQuery(<OnboardingWizard onDone={vi.fn()} />);

    expect(screen.queryByRole("button", { name: /back/i })).not.toBeInTheDocument();

    await advanceToStep(2);

    expect(screen.getByRole("button", { name: /back/i })).toBeInTheDocument();
  });

  it("Back returns to the previous step", async () => {
    renderWithQuery(<OnboardingWizard onDone={vi.fn()} />);

    await advanceToStep(2);
    await userEvent.click(screen.getByRole("button", { name: /back/i }));

    expect(await screen.findByText(/step 1 of 4/i)).toBeInTheDocument();
  });

  it("reaches step 4 and swaps Continue for Finish", async () => {
    renderWithQuery(<OnboardingWizard onDone={vi.fn()} />);

    await advanceToStep(4);

    expect(await screen.findByText(/step 4 of 4/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /finish/i })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /continue/i })).not.toBeInTheDocument();
  });
});

describe("OnboardingWizard — completion", () => {
  it("Finish marks onboarding complete with skipped=false", async () => {
    const onDone = vi.fn();
    renderWithQuery(<OnboardingWizard onDone={onDone} />);

    await userEvent.click(await avCheckbox());
    await userEvent.click(continueButton());
    await userEvent.click(continueButton());
    await userEvent.click(continueButton());
    await userEvent.click(screen.getByRole("button", { name: /finish/i }));

    await waitFor(() => expect(markOnboardingComplete).toHaveBeenCalledWith(false));
    await waitFor(() => expect(onDone).toHaveBeenCalledWith(false));
  });

  it("Skip setup marks onboarding complete with skipped=true", async () => {
    const onDone = vi.fn();
    renderWithQuery(<OnboardingWizard onDone={onDone} />);

    await userEvent.click(screen.getByRole("button", { name: /skip setup/i }));

    await waitFor(() => expect(markOnboardingComplete).toHaveBeenCalledWith(true));
    await waitFor(() => expect(onDone).toHaveBeenCalledWith(true));
  });

  it("Skip setup is reachable from step 1 without acknowledging the AV note", async () => {
    renderWithQuery(<OnboardingWizard onDone={vi.fn()} />);

    // Continue is gated, Skip is not.
    expect(continueButton()).toBeDisabled();
    expect(screen.getByRole("button", { name: /skip setup/i })).toBeEnabled();
  });

  it("surfaces an error and does not call onDone when the backend rejects", async () => {
    const onDone = vi.fn();
    markOnboardingComplete.mockRejectedValue(new Error("db locked"));
    renderWithQuery(<OnboardingWizard onDone={onDone} />);

    await userEvent.click(screen.getByRole("button", { name: /skip setup/i }));

    expect(await screen.findByText(/db locked/i)).toBeInTheDocument();
    expect(onDone).not.toHaveBeenCalled();
  });
});

describe("OnboardingWizard — detection step", () => {
  it("lists a detected game with its path", async () => {
    renderWithQuery(<OnboardingWizard onDone={vi.fn()} />);

    await userEvent.click(await avCheckbox());
    await userEvent.click(continueButton());

    expect(await screen.findByText(/genshin impact/i)).toBeInTheDocument();
    expect(await screen.findByText(/C:\\Games\\Genshin/)).toBeInTheDocument();
  });

  it("still renders rows when detection returns nothing at all", async () => {
    detectAllGames.mockResolvedValue([]);
    renderWithQuery(<OnboardingWizard onDone={vi.fn()} />);

    await userEvent.click(await avCheckbox());
    await userEvent.click(continueButton());

    // Step 2 must remain usable — Continue is never gated here.
    expect(await screen.findByText(/step 2 of 4/i)).toBeInTheDocument();
    expect(continueButton()).toBeEnabled();
  });

  it("does not block the wizard when detection rejects", async () => {
    detectAllGames.mockRejectedValue(new Error("registry unreadable"));
    renderWithQuery(<OnboardingWizard onDone={vi.fn()} />);

    await userEvent.click(await avCheckbox());
    await userEvent.click(continueButton());

    expect(await screen.findByText(/step 2 of 4/i)).toBeInTheDocument();
    expect(continueButton()).toBeEnabled();
  });
});

describe("OnboardingWizard — importer step failures", () => {
  const structuredFailure = {
    kind: "invalidActiveVariant",
    message: "GMM could not read this Game's saved setup.",
  };

  it("shows one install-path failure without hiding another Game", async () => {
    getGameInstallPath.mockImplementation((game: string) =>
      game === "gimi"
        ? Promise.reject(structuredFailure)
        : Promise.resolve("C:\\Games\\Star Rail"),
    );
    renderWithQuery(<OnboardingWizard onDone={vi.fn()} />);

    await advanceToStep(4);

    const heading = await screen.findByText(
      /could not check Genshin Impact's install path/i,
    );
    const alert = heading.closest('[role="alert"]');
    expect(alert).not.toBeNull();
    expect(alert).toHaveAttribute(
      "data-command-failure-kind",
      "invalidActiveVariant",
    );
    expect(alert).toHaveTextContent(structuredFailure.message);
    expect(screen.getByText("Star Rail")).toBeInTheDocument();
    expect(
      screen.queryByText(/No detected games to install for/i),
    ).not.toBeInTheDocument();
  });

  it("keeps a structured importer-install failure for the shared renderer", async () => {
    getGameInstallPath.mockImplementation((game: string) =>
      Promise.resolve(game === "gimi" ? "C:\\Games\\Genshin" : null),
    );
    installImporter.mockRejectedValue(structuredFailure);
    renderWithQuery(<OnboardingWizard onDone={vi.fn()} />);

    await advanceToStep(4);
    await userEvent.click(
      await screen.findByRole("button", { name: /install selected/i }),
    );

    const message = await screen.findByText(structuredFailure.message);
    expect(message).toHaveAttribute(
      "data-command-failure-kind",
      "invalidActiveVariant",
    );
    expect(screen.getByRole("button", { name: /retry/i })).toBeEnabled();
  });
});
