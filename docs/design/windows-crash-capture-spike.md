# Spike: can we capture a native crash on Windows?

Status: not started
Owner: runs on a Windows host, not on the maintainer's macOS machine
Gate for: ADR 0005 decisions 6 and 7

## Why this exists

GMM is written on macOS and only runs on Windows. When it dies from a
native fault there today, nothing is written down — the log simply stops.
The Rust panic hook shipped in `3e29d79` covers panics only; an access
violation kills the process before any of it runs.

This spike answers one question: **can a crash be captured at all on a
real Windows host, from the real shipped artefact?** Everything else in
ADR 0005 — dumps in the bundle, the opt-in button, Sentry as transport —
is downstream of the answer and must not be built before it.

Write the result down even if it is boring. A negative result is a real
result and saves the rest of the work.

## Rules

- Use the **packaged release build**, not `cargo run` and not a debug
  binary. The thing under test is the thing users install.
- **No network, no Sentry, no DSN, no account.** This spike proves
  capture. Transport is a separate decision that this gates.
- Run each crash probe **twice**, with a clean restart between runs. A
  crash handler that works once is not working.

## The experiment

Four probes, in order. Stop and record if one fails — later probes assume
earlier ones passed.

1. **Baseline fault in GMM's own code.** Force an access violation in the
   GMM process. A dump should appear on disk, complete and readable after
   the process is gone.
2. **Fault originating in a loaded DLL.** Load a purpose-built crash
   fixture DLL and fault inside it, with the real `3dmloader.dll` also
   mapped. This reproduces the shape that matters: a fault in foreign
   code inside our process. Use a fixture rather than trying to crash the
   real loader — we want a controlled fault, and the loader has no
   symbols anyway.
3. **Process lifecycle.** The crash reporter must start before the
   single-instance lock, finish writing, and exit without being orphaned.
   Then GMM must launch again normally — no "already running" error, no
   stale lock.
4. **Offline guarantee.** With upload disabled, confirm nothing leaves
   the machine. This is the property the whole privacy story rests on, so
   verify it rather than assuming it.

## Pass criteria

Agreed before running, so the result cannot be rationalised afterwards.
All of these, or it has not passed:

- Both crash classes captured, twice each.
- Dumps are readable after the process is gone.
- `3dmloader.dll` appears in the dump's module list, proving the fault
  was captured in the loader-mapped process shape.
- The matching release debug symbols resolve at least one GMM frame.
  The fixture resolving only to module+offset is expected and fine.
- No network traffic while upload is disabled.
- Windows Defender does not quarantine the reporter on that machine.
- GMM relaunches cleanly after each crash.

One clean machine passing Defender is baseline evidence, not proof that
false-positive rates are acceptable for users. Record it, don't over-read
it.

## Recording the result

Append the outcome to this file: `PASS`, `FAIL`, or `INCONCLUSIVE`, with
what was actually observed. Include a dump you captured, and note its
size — dump size drives the retention decision ADR 0005 leaves open.

`INCONCLUSIVE` is a legitimate answer and the most likely one on a first
attempt. A failed session usually means the harness was wrong, not that
the approach is impossible; ADR 0005 decision 6 says so explicitly. Do
not let one bad afternoon retire the idea.

## What happens next, by outcome

- **PASS** — land local capture only: dumps on disk with their own
  retention, listed at bundle export, and `README.md` plus the Settings
  screen amended in the same change to stop promising "no crash
  reporter". Then wait for a real crash and check the dump actually
  answers something.
- **FAIL or INCONCLUSIVE** — no transport, no Sentry dependency, no
  button. Fix the harness and retry, or accept panics-only for now.
