import { render, screen } from "@testing-library/react";
import { expect, it } from "vitest";

import { EnabledTransitionRecoveryWarning } from "./EnabledTransitionRecoveryWarning";

it("states the requested transition, automatic retry, and safety block", () => {
  render(
    <EnabledTransitionRecoveryWarning
      modName="Raiden"
      recovery={{
        intendedEnabled: false,
        reason: "the Junction directory is temporarily locked",
        attemptedAt: "2026-08-28T12:00:00Z",
        attempts: 1,
        junctionPath: "C:\\Games\\Genshin\\Mods\\Raiden",
        ownerUncertain: false,
        released: false,
      }}
      pending={false}
      onRetire={() => {}}
    />,
  );

  const warning = screen.getByRole("region", {
    name: /interrupted enable or disable recovery for raiden/i,
  });
  expect(warning).toHaveTextContent(/requested disable.*junction.*enabled flag agree/i);
  expect(warning).toHaveTextContent(/retry automatically.*next time it starts/i);
  expect(warning).toHaveTextContent(/blocks game launch.*mod or library changes/i);
  expect(warning).toHaveTextContent(/temporarily locked/i);
});

it("offers explicit retirement when producer identity is uncertain", () => {
  let retired = false;
  render(
    <EnabledTransitionRecoveryWarning
      modName="Raiden"
      recovery={{
        intendedEnabled: true,
        reason: "GMM cannot establish whether the original producer is still running",
        attemptedAt: "2026-08-28T12:00:00Z",
        attempts: 0,
        junctionPath: "C:\\Games\\Genshin\\Mods\\Raiden",
        ownerUncertain: true,
        released: false,
      }}
      pending={false}
      onRetire={() => {
        retired = true;
      }}
    />,
  );

  const button = screen.getByRole("button", {
    name: /I confirmed no other GMM is changing this Mod.*retire producer/i,
  });
  expect(screen.queryByText(/retry automatically.*next time it starts/i)).not.toBeInTheDocument();
  button.click();
  expect(retired).toBe(true);
});

const unresolved = {
  intendedEnabled: true,
  reason: "the recorded Junction parent changed filesystem identity",
  attemptedAt: "2026-08-28T12:00:00Z",
  attempts: 1,
  junctionPath: "C:\\Games\\Genshin\\Mods\\Raiden",
  ownerUncertain: false,
  released: false,
};

it("tells the user what release gives up and releases only after the second click", async () => {
  const { default: userEvent } = await import("@testing-library/user-event");
  let released = 0;
  render(
    <EnabledTransitionRecoveryWarning
      modName="Raiden"
      recovery={unresolved}
      pending={false}
      onRetire={() => {}}
      onRelease={() => {
        released += 1;
      }}
    />,
  );

  await userEvent.click(screen.getByRole("button", { name: /give up on this recovery/i }));
  expect(released).toBe(0);
  const confirmation = screen.getByRole("group", { name: /confirm release for raiden/i });
  expect(confirmation).toHaveTextContent(/will not know whether raiden is enabled/i);
  expect(confirmation).toHaveTextContent(/shows? unknown, not enabled or disabled/i);
  expect(confirmation).toHaveTextContent(/stops blocking game launch and library changes, in every game/i);
  expect(confirmation).toHaveTextContent(/will not treat it as something it created/i);

  await userEvent.click(screen.getByRole("button", { name: /keep waiting/i }));
  expect(released).toBe(0);
  expect(screen.queryByRole("group", { name: /confirm release/i })).not.toBeInTheDocument();

  await userEvent.click(screen.getByRole("button", { name: /give up on this recovery/i }));
  await userEvent.click(screen.getByRole("button", { name: /release and mark raiden as unknown/i }));
  expect(released).toBe(1);
});

it("does not offer release while the original producer may still be running", () => {
  render(
    <EnabledTransitionRecoveryWarning
      modName="Raiden"
      recovery={{ ...unresolved, ownerUncertain: true }}
      pending={false}
      onRetire={() => {}}
      onRelease={() => {}}
    />,
  );
  expect(screen.queryByRole("button", { name: /give up on this recovery/i })).not.toBeInTheDocument();
});

it("states the Mod's enabled state as unknown after release", () => {
  render(
    <EnabledTransitionRecoveryWarning
      modName="Raiden"
      recovery={{ ...unresolved, released: true }}
      pending={false}
      onRetire={() => {}}
      onRelease={() => {}}
    />,
  );
  const notice = screen.getByRole("region", { name: /unknown enabled state for raiden/i });
  expect(notice).toHaveTextContent(/does not know whether this mod is enabled/i);
  expect(notice).toHaveTextContent(/enable or disable the mod to settle it/i);
  expect(notice).not.toHaveTextContent(/until recovery succeeds/i);
  expect(screen.queryByRole("button", { name: /give up|retire/i })).not.toBeInTheDocument();
});
