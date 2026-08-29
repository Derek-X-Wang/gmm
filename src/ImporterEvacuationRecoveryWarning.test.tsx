import { render, screen } from "@testing-library/react";
import { expect, it } from "vitest";

import { ImporterEvacuationRecoveryWarning } from "./ImporterEvacuationRecoveryWarning";

it("explains the partial importer evacuation and automatic rollback", () => {
  render(
    <ImporterEvacuationRecoveryWarning
      displayName="Genshin Impact"
      recovery={{
        reason: "the backup directory is temporarily locked",
        attemptedAt: "2026-08-28T12:00:00Z",
        attempts: 1,
        gamePath: "C:\\Games\\Genshin",
        backupPath: "D:\\GMM\\backups\\gimi\\backup",
        ownerUncertain: false,
      }}
      pending={false}
      onRetry={() => {}}
      onRetire={() => {}}
    />,
  );

  const warning = screen.getByRole("region", {
    name: /interrupted model importer recovery for genshin impact/i,
  });
  expect(warning).toHaveTextContent(/game directory may still contain only part/i);
  expect(warning).toHaveTextContent(/fix the problem described below.*retry the recorded rollback here/i);
  expect(warning).toHaveTextContent(/blocks launching genshin impact.*another importer/i);
  expect(warning).toHaveTextContent(/temporarily locked/i);
  expect(warning).toHaveTextContent(/C:\\Games\\Genshin/i);
  expect(warning).toHaveTextContent(/D:\\GMM\\backups\\gimi\\backup/i);
});

it("offers explicit producer retirement only when identity is uncertain", () => {
  let retired = false;
  render(
    <ImporterEvacuationRecoveryWarning
      displayName="Genshin Impact"
      recovery={{
        reason: "GMM cannot establish whether the original importer producer is still running",
        attemptedAt: "2026-08-28T12:00:00Z",
        attempts: 0,
        gamePath: "C:\\Games\\Genshin",
        backupPath: "D:\\GMM\\backups\\gimi\\backup",
        ownerUncertain: true,
      }}
      pending={false}
      onRetry={() => {}}
      onRetire={() => {
        retired = true;
      }}
    />,
  );

  const button = screen.getByRole("button", {
    name: /I confirmed no other GMM is changing this importer.*retire producer/i,
  });
  expect(screen.queryByText(/retry the recorded rollback automatically/i)).not.toBeInTheDocument();
  button.click();
  expect(retired).toBe(true);
});

it("offers an in-session retry after an ordinary recovery failure", () => {
  let retried = false;
  render(
    <ImporterEvacuationRecoveryWarning
      displayName="Genshin Impact"
      recovery={{
        reason: "the backup directory is temporarily locked",
        attemptedAt: "2026-08-28T12:00:00Z",
        attempts: 1,
        gamePath: "C:\\Games\\Genshin",
        backupPath: "D:\\GMM\\backups\\gimi\\backup",
        ownerUncertain: false,
      }}
      pending={false}
      onRetire={() => {}}
      onRetry={() => {
        retried = true;
      }}
    />,
  );

  const retry = screen.getByRole("button", { name: /retry model importer recovery/i });
  expect(screen.queryByText(/next time it starts/i)).not.toBeInTheDocument();
  retry.click();
  expect(retried).toBe(true);
});
