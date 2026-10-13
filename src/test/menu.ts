import { act, waitFor } from "@testing-library/react";

import { MENU_ACTION_EVENT, type MenuAction } from "../api";

type ListenMock = {
  mock: { calls: unknown[][] };
};

/**
 * Click an item in the native File/Help menu (#351).
 *
 * The menu bar lives in the OS, built by `src-tauri/src/menu.rs`, so no part
 * of it is in the document and no test can click it. What a test can
 * reproduce exactly is the one thing a click produces: the event Rust
 * forwards on `MENU_ACTION_EVENT`. The ids come from `MenuAction`, which
 * `src-tauri/tests/menu.rs` cross-checks against the Rust constants, so an
 * id that stopped matching fails there rather than passing silently here.
 *
 * Throws when nothing is listening, so a dropped subscription is a failure
 * instead of a no-op that every assertion then reads as "the item did
 * nothing, as expected".
 */
export async function clickMenuItem(listen: ListenMock, action: MenuAction): Promise<void> {
  const found = () => listen.mock.calls.filter(([name]) => name === MENU_ACTION_EVENT);
  await waitFor(() => {
    if (found().length === 0) {
      throw new Error(
        `nothing is listening on ${MENU_ACTION_EVENT}: the native menu cannot reach the app`,
      );
    }
  });
  const subscriptions = found();
  // The newest subscription belongs to the live render; earlier ones were
  // unlistened on unmount, which the mock does not record.
  const handler = subscriptions[subscriptions.length - 1][1] as (event: {
    payload: MenuAction;
  }) => void;
  await act(async () => {
    handler({ payload: action });
  });
}
