# 0005 — Crash reporting: one build, off by default

Date: 2026-08-16
Status: Accepted

## Context

GMM is developed on macOS and only runs meaningfully on Windows. That gap
is the slowest part of the debug loop: when GMM dies on a user's — or the
maintainer's — Windows machine, the evidence dies with the process.

The existing diagnostics are good but survivor-biased. `core::diagnostics`
writes JSON-lines logs and `Export diagnostics bundle` packages the last 7
days plus a redacted settings snapshot. Both assume the process lives long
enough to write and to be exported. They do not:

- The tracing writer is non-blocking, so the tail of a crashing run can be
  lost.
- A native fault (access violation, abort) produces no log line at all and
  no artefact of any kind.

That second case is not hypothetical for GMM specifically. Per ADR 0001 we
load `3dmloader.dll` — third-party GPLv3 C++ — into our own process via FFI,
and `runtime::launch` keeps it mapped for the lifetime of a Game Session.
When something faults on that path today, we learn nothing.

Against that, `README.md` publishes an explicit promise: GMM "does not phone
home. There is no telemetry, no crash reporter, no background uploader."
That promise is an asset with a modding audience already primed to distrust
binaries that touch their game directories, and `docs/antivirus-and-smartscreen.md`
documents that GMM is routinely flagged by Defender as it is.

## Considered alternatives

- **Remote telemetry on by default.** Reverses a published promise for an
  app with effectively no userbase (first alpha released 2026-08-13). Buys
  nothing today and spends trust we will want later.
- **Two builds: a clean release plus a maintainer diagnostic build.**
  Preserves the promise exactly and keeps the crash-reporter machinery out
  of users' hands. Rejected: for a Windows-only app the maintainer cannot
  easily reproduce, divergent builds mean the artefact under test stops
  being the artefact shipped. That correctness risk outweighs the tidiness,
  and it doubles pipeline maintenance permanently.
- **Local crash artefacts only, no remote reporting ever.** Genuinely
  attractive — most of the debugging win, no network, no consent question,
  no added antivirus surface. Rejected only because it forecloses opt-in
  user crash reporting later without a second decision; kept as the fallback
  if the antivirus cost below proves real.

## Decision

**One build, for everyone, with crash reporting compiled in and inert
unless explicitly switched on.**

1. **Local crash evidence is always on, and never leaves the machine.** A
   crash leaves an artefact on disk that survives the process. That artefact
   joins the existing diagnostics bundle, so a user who chooses to file a bug
   report now carries the evidence with them. This is the part that helps
   everyone and requires no consent, because nothing is transmitted.
2. **Remote reporting (Sentry) ships in the same binary, off by default.**
   It transmits only when deliberately enabled. The maintainer enables it on
   their own Windows machine; users never trigger it by installing or running
   GMM.
3. **The README promise is amended, not quietly broken.** It becomes: GMM
   does not phone home unless you turn it on, and it is off when you install
   it. A promise we can keep beats a stronger one we have to walk back.
4. **Sentry is treated as transport, not as the thing to prove.** Local
   capture must be demonstrated to work on Windows — including for a fault
   originating inside a dynamically loaded DLL — before remote delivery is
   wired up. If capture does not work, the remote half is worthless.
5. **No user-facing telemetry beyond crashes, and no default-on reporting**,
   until there is a real userbase and a deliberate opt-in decision. That is a
   future ADR, not an implicit consequence of this one.

## Consequences

- The shipped binary contains crash-reporting machinery even while disabled.
  This adds behaviour antivirus heuristics dislike — self-spawn, IPC, dump
  files — to an executable that already performs DLL injection and junction
  manipulation. This is the accepted cost of not forking the build, and it
  is the trigger to revisit: if Defender flags GMM measurably more often, we
  fall back to local-only artefacts.
- Crash artefacts can contain arbitrary process memory: file paths with the
  Windows username, mod names, and anything else resident at fault time. They
  are categorically more sensitive than the reviewable text logs we ship
  today. They stay local by default for exactly this reason, and anything
  transmitted must be scrubbed on the same terms as the existing bundle.
- The crash reporter re-enters the GMM executable as a child process. It must
  initialise before the single-instance lock in `lib.rs`, or the reporter will
  be rejected as a duplicate instance and silently capture nothing.
- Release builds need debug information retained for crash artefacts to be
  readable at all. We do not have symbols for `3dmloader.dll` — the vendored
  copy is a prebuilt binary — so faults inside the loader will resolve to a
  module offset, not a function name. Loader crashes become *locatable*, not
  fully legible.
- Some crash classes remain uncapturable by design: fail-fast termination,
  hangs and deadlocks, forced process termination, and any crash of the game
  process itself. Model Importer problems live in the game process and are out
  of scope entirely — this ADR is about GMM's own failures.
