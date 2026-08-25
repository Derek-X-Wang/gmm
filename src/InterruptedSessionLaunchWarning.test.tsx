import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";

import { InterruptedSessionLaunchWarning } from "./InterruptedSessionLaunchWarning";

it("surfaces launch uncertainty and requires an explicit closed-game confirmation", async () => {
  const retire = vi.fn();
  const view = render(
    <InterruptedSessionLaunchWarning
      launch={{
        id: "01JLAUNCH",
        game: "gimi",
        childPid: null,
        startedAt: "2026-08-24T00:00:00Z",
      }}
      pending={false}
      onRetire={retire}
    />,
  );

  const warning = screen.getByRole("region", { name: /interrupted gimi launch/i });
  expect(warning).toHaveTextContent(/stopped before it could record the game process/i);
  expect(warning).toHaveTextContent(/cannot determine whether a game.*is still running/i);
  expect(warning).toHaveTextContent(/kept the launch reservation.*Library untouched/i);
  expect(warning).not.toHaveTextContent(/gimi is running/i);

  await userEvent.click(
    screen.getByRole("button", { name: /I confirmed the game is closed.*retire reservation/i }),
  );
  expect(retire).toHaveBeenCalledTimes(1);

  view.rerender(
    <InterruptedSessionLaunchWarning
      launch={{
        id: "01JLAUNCH",
        game: "gimi",
        childPid: 4242,
        startedAt: "2026-08-24T00:00:00Z",
      }}
      pending={false}
      onRetire={retire}
    />,
  );
  expect(warning).toHaveTextContent(/PID 4242.*may now belong to a different process/i);
});
