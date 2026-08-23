# Orchestration: one model coordinates, another implements

How multi-issue work is run on this repo when the coordinating model's quota is the
scarce resource and the implementing model's is not. The coordinator decides,
dispatches, verifies and merges. The implementer writes every line of production code
and every cold review.

This is a working practice, not a rule of the project. It is written down because most
of it was learned by losing a cycle to it.

## Roles

| Role | Does |
| --- | --- |
| **Coordinator** | Orders the queue, writes dispatch specs, adjudicates review findings, verifies claims independently, merges, files follow-ups. |
| **Implementer** | One issue per worker. Writes the fix and its tests. Gets CI green before reporting done. |
| **Cold reviewer** | Reviews a diff it did not write, against a prompt naming what to be suspicious of. |

The rule underneath the table: **never let one model both write a change and be the
only reviewer of it.** Implementer and reviewer may be the same model; they must not be
the same context.

## The loop, per issue

1. **Fresh worktree, fresh worker.** Never reuse either across issues, or across rework
   rounds on one issue. A reused context carries stale assumptions into new work.
2. **Dispatch a spec, not a ticket link.** See the anatomy below.
3. **Worker reports done with CI green.** Green CI is the floor, not the finish line.
4. **The coordinator verifies by mutation, personally.** Break the guard the fix
   installed, watch the right test go red, restore it, watch it go green. If it never
   goes red, the fix has no test.
5. **Cold review of the diff**, given the evidence already gathered.
6. **Merge, release the worker, delete the worktree.** Then dispatch the next one.

Review findings become either a rework round on the same PR with a fresh worker, or a
new issue — see [when to split](#when-to-split-instead-of-grinding).

## Anatomy of a dispatch that works

Roughly a page. The parts that consistently earn their space:

- **The defect in behavioural terms** — file and line, and what a user experiences.
- **Context the worker would otherwise rediscover** — what landed recently, which
  machinery to reuse, what a sibling PR already established.
- **Verified facts, marked as such**: "this is proven, do not re-derive."
- **An explicit out-of-scope list**, naming other in-flight work and its files.
- **The standing rules below**, repeated in full every time.
- **Ask, don't guess** — escalate rather than assume.

State what wins when sources conflict: *the issue's acceptance criteria beat the
dispatch.* Briefs go stale as PRs land around them, and a worker that follows a stale
dispatch over a current issue wastes a round.

## Standing rules, in every dispatch

**Mutation-prove every test.** Break the production guard, watch the test go red,
restore, watch it go green. Report the exact mutation and the exact assertion that
fired. A test you cannot make fail is not coverage.

**Rebuild everything the tests actually run.** In this repo the concurrency tests spawn
a separately built `concurrency-probe` binary, so `cargo test` alone leaves a source
mutation unseen and every test wrongly green. Run `cargo build --workspace` after any
mutation. This single omission cost eight separate cycles.

**Read the whole test output.** A failure at the bottom of `cargo test --workspace` is
easy to miss; a red CI has been shipped from reading only the first screen.

**Never tell the user something the code cannot establish.** One PR needed four rounds
in which each round fixed the code correctly and then misdescribed the result —
promising a retry that would never happen, naming a cleanup path that was not the
user's. The code being right is half the job.

**State what you could not verify.** "No manual screen-reader session was run." "I could
not mount a second volume." An honest gap is worth more than a confident summary,
because the coordinator can act on it.

## What cold review is for

Not a second opinion on style. It exists to catch the class of defect that passes CI: a
test that cannot fail, a message that lies, a fix that moves a bug rather than closing
it.

Two things make a review land:

- **Supply the mutations already run**, so the reviewer builds on them instead of
  repeating them.
- **Name what is already decided**, so it does not re-litigate an accepted trade-off.

Then ask it to attack specifics rather than "review this":

```text
Judge specifically:
1. Is the race closed, or narrowed? Say plainly if it is irreducible.
2. Does refusal stay resumable — every caller, not just the happy one?
3. Do the assertions pin the property, or would a future refactor satisfy
   them while still failing to announce?
```

## Defect patterns worth hunting

- **Looked like coverage, asserted nothing.** Found and removed roughly a dozen times in
  one run: a regex over button labels, a substring search for a constant's name, a
  hand-maintained registry the check trusts, a timeout test asserting only that
  *something* failed.
- **Two durable steps with a window between them**, where whatever survives a crash is
  invisible to the repair path.
- **The report and the guard disagree.** One filters by scope and the other does not, so
  the user is told a real thing is junk and handed an error no refresh will clear.
- **The fix moved the lie.** Each round closes a defect and the message describing it
  becomes untrue in a new way.

## When to split instead of grinding

Rework rounds are healthy while each round finds a defect that *predates* it. They have
gone wrong when round N's finding is a consequence of round N−1's fix, twice running —
that pattern can continue indefinitely.

At that point:

- Merge the part that is proven.
- Move the unresolved interactions to their own issue, designed once rather than patched
  per round.
- Record in a PR comment what shipped proven versus what was deferred, so nobody later
  mistakes the split for an oversight.
- Say plainly in the follow-up whether the deferred items can lose data. That is what
  decides their priority.

## Coordination hygiene

- **Sequence overlapping work.** Check which files each queued issue touches against
  what is in flight. Two workers branching from a base that is about to move is how
  evidence gets gathered against code that no longer exists.
- **Pull before creating a worktree.** A stale local `main` sends every worker off a base
  that is one merge behind.
- **Consume the message queue properly.** Peeking without acknowledging replays the same
  message forever and hides real questions behind it.
- **Verify the branch is on the remote before deleting a worktree.**
- **Never delete a worktree that is not yours.** Check for unmerged commits first.
