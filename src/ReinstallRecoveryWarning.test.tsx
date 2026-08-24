import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";

import {
  ReinstallRecoveryNotices,
  ReinstallRecoveryWarning,
} from "./ReinstallRecoveryWarning";

const RECOVERY = {
  reason: "the recorded staging path identifies an unrelated directory",
  attemptedAt: "2026-08-23T12:00:00Z",
  attempts: 1,
  libraryPath: "D:\\GMM\\gimi\\01MOD",
  stagedPath: "D:\\GMM\\gimi\\.gmm-reinstall-01SWAP",
  quarantinePath: "D:\\GMM\\gimi\\.gmm-delete-01SWAP",
};

it("distinguishes a retryable obstruction from intervention without guessing", async () => {
  const retry = vi.fn();
  render(
    <ReinstallRecoveryWarning
      modName="Raiden"
      recovery={RECOVERY}
      pending={false}
      onRetry={retry}
    />,
  );

  const warning = screen.getByRole("region", {
    name: /interrupted reinstall recovery for raiden/i,
  });
  expect(warning).toHaveTextContent(/Retry may work:.*briefly locked.*device was unavailable/i);
  expect(warning).toHaveTextContent(/This needs you:.*moved or deleted.*permissions/i);
  expect(warning).toHaveTextContent(/cannot reliably distinguish/i);
  expect(warning).toHaveTextContent(/will not discard either recorded byte identity/i);
  expect(warning).toHaveTextContent(/witness says the old tree should be live/i);
  expect(warning).toHaveTextContent(/could not restore or verify it/i);
  expect(warning).toHaveTextContent(/will not load until recovery succeeds/i);
  expect(warning).toHaveTextContent(/enabled or disabled choice is unchanged/i);

  await userEvent.click(screen.getByRole("button", { name: /retry recovery/i }));
  expect(retry).toHaveBeenCalledTimes(1);
});

it("shows every recorded path as evidence and never as a deletion instruction", async () => {
  render(
    <ReinstallRecoveryWarning
      modName="Raiden"
      recovery={RECOVERY}
      pending={false}
      onRetry={() => {}}
    />,
  );

  await userEvent.click(screen.getByText(/recorded paths and recovery error/i));
  expect(screen.getByText(RECOVERY.libraryPath)).toBeInTheDocument();
  expect(screen.getByText(RECOVERY.stagedPath)).toBeInTheDocument();
  expect(screen.getByText(RECOVERY.quarantinePath)).toBeInTheDocument();
  expect(screen.getByText(RECOVERY.reason)).toBeInTheDocument();
  expect(screen.getByText(/not instructions to delete anything/i)).toBeInTheDocument();
});

it("changes text inside live regions that were already mounted", () => {
  const view = render(<ReinstallRecoveryNotices feedback={null} />);
  const status = screen.getByRole("status");
  const alert = screen.getByRole("alert");
  expect(status).toBeEmptyDOMElement();
  expect(alert).toBeEmptyDOMElement();

  view.rerender(
    <ReinstallRecoveryNotices
      feedback={{ kind: "recovered", modName: "Raiden" }}
    />,
  );
  expect(screen.getByRole("status")).toBe(status);
  expect(status).toHaveTextContent(/usable again/i);
  expect(status).not.toHaveFocus();
});
