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
   crash leaves an artefact on disk that survives the process. Nothing is
   transmitted, so this part needs no consent.
2. **Crash dumps are included in the diagnostics bundle, and shown at export
   time.** The bundle is the route a user takes to send us evidence
   voluntarily, so it is the route by which their process memory would reach
   us. Dumps are listed explicitly when the bundle is built and are easy to
   leave out. Included, never silent.
3. **Remote reporting (Sentry) ships in the same binary, off by default, and
   is enabled by an explicit user action.** Not a hidden switch: whoever turns
   it on must be able to see what they are turning on. That deliberate act
   *is* the consent, which places the whole weight of this decision on the
   wording next to the control — see the disclosure requirement below.
4. **We transmit whole crash dumps, and we say so plainly.** Considered and
   rejected: transmitting only derived metadata (fault address, module list,
   no memory). It is the safer artefact, but it answers "where" and not
   "why", and the crashes we most need to solve are inside a vendored DLL we
   have no symbols for — precisely where surrounding memory is the only
   evidence left. GMM is a personal project today; debugging capability
   outweighs the privacy exposure of an artefact that only leaves the machine
   when someone deliberately makes it.
5. **A dump is not sanitizable, and we do not pretend otherwise.** It is a
   snapshot of process memory and can contain anything resident at fault
   time — including the proxy password that `SettingsSnapshot::redacted`
   carefully strips from `settings.json`. Some Windows crash classes do not
   run filtering hooks at all, so even best-effort scrubbing is not a
   promise we can keep. The honest boundary is disclosure plus an explicit
   opt-in, not a redactor.
6. **Sentry is transport, not the thing to prove.** Local capture must be
   demonstrated on Windows — including a fault originating inside a
   dynamically loaded DLL, with `3dmloader.dll` mapped — before any remote
   delivery is wired up. A failed spike means "no transport yet", not "the
   approach is impossible"; a bad experiment is the likelier explanation.
7. **Before remote delivery is even considered, a real crash must prove the
   local artefact useful.** If reading the dump on disk already answers the
   question, remote delivery is convenience rather than capability, and can
   wait longer still.

## Consequences

- **The disclosure carries the entire consent burden.** Because enabling is a
  visible action rather than a maintainer-only switch, the text beside it must
  state plainly that crash dumps can contain personal data — file paths with
  the Windows username, mod names, and other process memory. Vague wording
  here is the failure mode that matters; there is no redactor behind it to
  catch what the sentence fails to say.
- **`README.md` and the Settings screen must be amended when local capture
  lands, not when Sentry does.** Both currently promise "no crash reporter",
  and that becomes untrue the moment a dump exists on disk — network or no
  network. They must describe the local artefact and the remote upload as two
  separate things.
- The shipped binary contains crash-reporting machinery even while disabled.
  This adds behaviour antivirus heuristics dislike — self-spawn, IPC, dump
  files — to an executable that already performs DLL injection and junction
  manipulation. Accepted cost of not forking the build, and the trigger to
  revisit: if Defender flags GMM measurably more often, we fall back to
  local-only artefacts.
- Dumps need a retention and deletion story of their own. They are larger and
  more sensitive than logs, so they cannot simply inherit the 14-day log
  window without a deliberate decision.
- The crash reporter re-enters the GMM executable as a child process. It must
  initialise before the single-instance lock in `lib.rs`, or the reporter will
  be rejected as a duplicate instance and silently capture nothing.
- Release builds need debug information retained for crash artefacts to be
  readable at all, and that debug information must be **archived per release**.
  A dump we cannot decode six months later is worse than no dump, because it
  costs the privacy exposure and returns nothing.
- We have no symbols for `3dmloader.dll` — the vendored copy is a prebuilt
  binary — so faults inside the loader resolve to a module offset, not a
  function name. Loader crashes become *locatable*, not fully legible.
- Some crash classes remain uncapturable by design: fail-fast termination,
  hangs and deadlocks, forced process termination, and any crash of the game
  process itself. Model Importer problems live in the game process and are out
  of scope entirely — this ADR is about GMM's own failures.
- If GMM ever stops being a personal project, decision 4 is the one to
  revisit first. It is justified by today's scale, not by principle.
