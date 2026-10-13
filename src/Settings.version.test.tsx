import { beforeEach, expect, it, vi } from "vitest";
import { screen } from "@testing-library/react";
import { renderWithQuery } from "./test/harness";
import buildConfig from "../src-tauri/tauri.conf.json";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("./LibraryPathEditors", () => ({ GlobalLibraryRootEditor: () => null, GameLibraryOverrideEditor: () => null }));
vi.mock("./UpdatesPanel", () => ({ UpdatesPanel: () => null }));
vi.mock("./ImporterRecommendations", () => ({ ImporterRecommendations: () => null }));
vi.mock("./Diagnostics.tsx", () => ({ Diagnostics: () => null }));
const { SettingsDestination } = await import("./SettingsDestination");

function renderSettings() {
  return renderWithQuery(<SettingsDestination games={[]} page="gmm" returnGame={null}
    onPageChange={() => {}} onBack={() => {}} />);
}

beforeEach(() => {
  invoke.mockImplementation(async (command: string) => {
    if (command === "get_app_version") return buildConfig.version;
    if (command === "get_proxy_config") return { url: null, username: null, passwordSet: false };
    throw new Error(`Unexpected command: ${command}`);
  });
});

it("shows the running build version in GMM Settings through the real invoke wrapper", async () => {
  renderSettings();
  expect(await screen.findByText(`Version ${buildConfig.version}`)).toBeVisible();
  expect(invoke).toHaveBeenCalledWith("get_app_version", undefined);
});

it("follows a different running build without changing the display code", async () => {
  const nextBuild = `${buildConfig.version}-version-test`;
  const original = invoke.getMockImplementation()!;
  invoke.mockImplementation((command: string) => command === "get_app_version"
    ? Promise.resolve(nextBuild) : original(command));
  renderSettings();
  expect(await screen.findByText(`Version ${nextBuild}`)).toBeVisible();
  expect(screen.queryByText(`Version ${buildConfig.version}`)).not.toBeInTheDocument();
});

it("keeps a version read failure visible without inventing a version", async () => {
  const original = invoke.getMockImplementation()!;
  invoke.mockImplementation((command: string) => command === "get_app_version"
    ? Promise.reject(new Error("Version read failed")) : original(command));
  renderSettings();
  expect(await screen.findByText("Version read failed")).toBeVisible();
  expect(screen.queryByText(`Version ${buildConfig.version}`)).not.toBeInTheDocument();
});
