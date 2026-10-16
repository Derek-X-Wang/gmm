# Remembering the pre-maximise window rectangle

GMM does not remember the rectangle a window occupied before it was maximised. A window
that was maximised when GMM last closed reopens maximised, and pressing Restore Down
returns it to the default 1100x760 at the top-left of the display rather than to whatever
size the user had chosen earlier.

This is deliberate, and it is explained where the behaviour lives, in
`src-tauri/src/core/window_state.rs`:

> A maximised window's saved size *is* its maximised rect, so applying it would make
> Restore Down a no-op: the window would un-maximise onto the same pixels it already
> covers. The pre-maximise size is deliberately not remembered (#369), so the default is
> what Restore Down returns to.

So the choice was never "remember the right rectangle or remember the wrong one". With one
stored rectangle, the options were a Restore Down that does nothing visible, or a Restore
Down that returns to the default. The default was chosen.

## Why this is out of scope

Ordinary Windows applications do remember both — the maximised flag and a separate restore
rectangle — so this is a genuine gap in polish rather than a misunderstanding. It is out of
scope because of what it costs, not because it would be wrong.

Taking it means persisting a second rectangle and restoring it underneath the maximised
state. That is a schema change to the saved window state plus new restore logic, inside the
module that has needed the most care in the application. `window_state.rs` took three
review rounds to settle, over three problems that all bite exactly here:

- **Coordinate spaces.** A plan has to be applied in the same space it was checked in.
  A second rectangle doubles the number of values that can be checked in one space and
  applied in another.
- **DPI.** The clamp has to follow the current display's scaling, which may differ from the
  scaling in force when the rectangle was saved.
- **Display selection.** The maximise target is the display the window most overlaps, which
  is not necessarily the display the restore rectangle was captured on.

A stored restore rectangle interacts with all three, and it only pays off in the single
moment a user presses Restore Down after a relaunch.

Nothing is broken today. Nothing is lost but the convenience, and the behaviour is
discoverable in the code.

## What would change the decision

Daily use. If working maximised and losing the chosen size becomes an actual irritation
rather than an observed asymmetry with other Windows applications, the trade flips — the
cost does not change, but the benefit stops being hypothetical.

If it is taken up later, these are the conditions it has to meet, carried over from the
closed issue:

- The pre-maximise rectangle is stored alongside the maximised flag.
- Reopening maximised and pressing Restore Down returns to that rectangle.
- An unreachable stored rectangle still falls back to the default, silently, as today.
- The existing guarantees hold: the plan is applied in the coordinate space it was checked
  in, the clamp follows the current display's scaling, and the maximise target is the
  display the window most overlaps.

## Prior requests

- #381 — Restore Down after a maximised relaunch returns to the default size, not the size
  before maximising. Observed on Windows during the v0.4.3-alpha.1 verification round
  (#377, check 06), and filed by the reporter as a question rather than a defect. Closed as
  out of scope.

## Related, and deliberately separate

- #369 established that the pre-maximise size is not remembered, which is the decision this
  record explains rather than revisits.
- Window geometry persistence itself works and is covered. This record is only about the
  second rectangle.
