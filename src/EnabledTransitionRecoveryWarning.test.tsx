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
      }}
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
