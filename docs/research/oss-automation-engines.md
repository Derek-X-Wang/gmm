# Research: which OSS automation engine should the HSR dailies bot build on?

Ticket: [Derek-X-Wang/gmm#82](https://github.com/Derek-X-Wang/gmm/issues/82) (part of the `wayfinder` map, [#81](https://github.com/Derek-X-Wang/gmm/issues/81))

Date: 2026-08-15

## Question

Which existing open-source game-automation engine, if any, should GMM build the Star Rail
dailies bot on top of — and can we legally and technically embed it? GMM is
`GPL-3.0-or-later` (see [ADR 0001](../adr/0001-gplv3-and-embed-3dmloader.md)), runs on
Windows, is Tauri 2 + Rust + React, and has no language-purity constraint: a C/C++ library
via FFI or a bundled sidecar process is allowed if it earns its place.

## Method

Every claim below is sourced from the project's own repository (README, `LICENSE` file, or
GitHub API metadata fetched directly — license, `pushed_at`, latest release) or, for license
compatibility questions, the FSF's own GPL FAQ. No secondary blog posts or aggregator pages
were used as the source of a claim; where search results surfaced a candidate, the claim
itself was verified against the candidate's own repo before inclusion.

## Comparison table

| Candidate | License | Language / ABI | Input path | Vision approach | Windows-host viability | Maintenance (as of 2026‑08‑15) |
|---|---|---|---|---|---|---|
| **MaaFramework** | LGPL-3.0 | C++20 core, C ABI; official bindings for Python/Node/Go/C#/**Rust** (`maa-framework-rs`) | Win32 desktop controller (GDI/DXGI capture, 9 input-injection modes) **and** ADB controller | Template matching + OCR (PaddleOCR-family ONNX, bundled) + ONNX custom models | Native — ships Windows x86_64/aarch64 builds; Win32 controller targets exactly a native PC window | v5.12.3 released 2026‑08‑01; repo pushed 2026‑08‑15; 4.7k★/529 forks |
| **MaaAssistantArknights (MAA)** | AGPL-3.0-only (no added terms found) | C++20 app (not a library); UI in C# | ADB (Android/emulator) + Win32 for desktop clients | OpenCV template match, PaddleOCR/ChineseOCR_lite, ONNX Runtime models | Yes, ships Windows builds, but it is Arknights-specific and not built for reuse as a component | Very active: pushed 2026‑08‑15, 22.6k★, 19.9k commits |
| **March7thAssistant** | GPL-3.0 | Python (PyQt-Fluent-Widgets GUI) | Win32 desktop client automation via `PyAutoGUI` + a "云·星穹铁道" cloud-client mode; no MaaFramework dependency | `RapidOCR` + template match (`OpenCV`), ONNX Runtime / OpenVINO acceleration | Native Windows exe releases, closest prior art (HSR-specific) | Active: pushed 2026‑08‑15, 11.2k★, HSR support is current by construction |
| **better-genshin-impact (BetterGI)** | GPL-3.0 | C# / .NET 8 | Win32 `SendInput`, standalone WPF app, admin-rights required | Custom CV pipeline + OCR, no game-memory access | Windows 10/11 only, PC-client prior art (Genshin, not HSR) | Active: pushed 2026‑08‑15, 14.8k★ |
| **uiautomator2** | MIT | Python client + on-device Java/Kotlin HTTP-RPC server | ADB only; drives Android's accessibility tree, not pixels | None (element-tree, not vision) | Only reaches an Android target (device or emulator) via ADB — no Windows PC-client path | Active: pushed 2026‑08‑07, 8.3k★ |
| **Airtest / Poco** (NetEase) | Apache-2.0 | Python | ADB (Android), Windows via `pywin32`, iOS via `iOS-Tagent` | Template matching (Airtest, image-based); Poco adds engine element-tree access for Unity/Cocos | Airtest: pushed 2026‑03‑23 (5 months stale). **Poco: pushed 2024‑01‑08 — effectively unmaintained** | Airtest: moderate but slowing. Poco: stale (~2.5 yrs) |
| **Appium** | Apache-2.0 | TypeScript/JS core, WebDriver protocol, drivers per platform | Depends on driver: `WinAppDriver` for Windows desktop (Microsoft's own repo, MIT) | None built in — accessibility-tree/WebDriver locators only, no template match/OCR | Windows desktop via WinAppDriver, but that driver is stale — Microsoft's own repo shows `pushed_at` 2025‑04‑14, `activeRepoStatus: false`, 1,155 open issues | Appium core itself very active (pushed 2026‑08‑14), but the Windows driver it would need is effectively unmaintained |
| **scrcpy + raw ADB** | Apache-2.0 | C, FFmpeg/SDL2 | ADB screen mirroring + input injection, Android-only; no Windows PC-client path | None — it is a transport/display layer, not a vision engine | Runs as a Windows *host* app talking to an Android *target* — irrelevant to a native Windows PC-client bot | Very active (pushed 2026‑08‑14) but solves the wrong problem here |
| **Write it ourselves (Rust)** | N/A (ours) | Rust, native | `SendInput`/`SetCursorPos` via `windows-rs`, or a Win32 crate; screen capture via Windows.Graphics.Capture or GDI | Whatever we choose to write: template matching via `image`/`imageproc`, OCR via `tesseract-rs` bindings or an ONNX Runtime crate (`ort`) running a fetched PaddleOCR model | Native, exactly what we build | N/A — the cost is ours |

## Per-candidate detail

### MaaFramework (and MAA proper)

MaaFramework (`MaaXYZ/MaaFramework`) is the generalized, game-agnostic successor engine
extracted from MaaAssistantArknights — the README describes it as "an automation black-box
testing framework based on image recognition."
[github.com/MaaXYZ/MaaFramework](https://github.com/MaaXYZ/MaaFramework)

- **License**: LGPL-3.0, confirmed via GitHub's license API
  (`"key": "lgpl-3.0", "spdx_id": "LGPL-3.0"`).
  [api.github.com/repos/MaaXYZ/MaaFramework](https://api.github.com/repos/MaaXYZ/MaaFramework)
  LGPL-3.0 is materially easier to embed than GPL/AGPL: it permits dynamic linking (or static
  linking with the LGPLv3 relinking accommodation) from a differently-licensed program without
  forcing GMM's own code under the LGPL — GMM only has to preserve LGPL notices and give users
  the ability to relink against a modified MaaFramework. GMM is already GPL-3.0-or-later, so
  this is a non-issue either way, but it means the license question here is easier than it was
  for `3dmloader.dll` (ADR 0001).
- **Language/ABI**: C++20 core exposing a stable C ABI, with official bindings for Python,
  Node.js, Go, C#, and — critically — **Rust**, via `MaaXYZ/maa-framework-rs`
  (published to crates.io as `maa-framework`, LGPL-3.0, actively pushed 2026‑08‑11).
  [github.com/MaaXYZ/maa-framework-rs](https://github.com/MaaXYZ/maa-framework-rs)
  A Tauri-specific starter also exists (`MaaXYZ/maa-framework-tauri-template`) but is
  **archived** and last touched 2024‑05‑19 — evidence of the pattern, not something to build
  on directly.
  [github.com/MaaXYZ/maa-framework-tauri-template](https://github.com/MaaXYZ/maa-framework-tauri-template)
- **Input path**: MaaFramework documents seven controller types, including a **Win32
  controller purpose-built for native Windows desktop applications** — six screencap methods
  (GDI through DXGI variants, because "different programs on Win32 handle rendering
  differently, so there is no universal method") and nine input-injection methods (Seize,
  SendMessage, PostThreadMessage, an Interception-driver mode, etc.), plus an ADB controller
  for Android/emulator targets and a gamepad controller.
  [docs/en_us/2.4-ControlMethods.md](https://github.com/MaaXYZ/MaaFramework/blob/main/docs/en_us/2.4-ControlMethods.md)
  This is the only candidate whose engine treats "drive a native Windows game window" as a
  first-class, documented target rather than an Android-emulation afterthought.
- **Vision approach**: template matching plus OCR (PaddleOCR-family ONNX models, bundled) plus
  support for custom ONNX models via a documented "Pipeline" JSON protocol. The Windows
  x86_64 release artifact (`MAA-win-x86_64-v5.12.3.zip`) is **69.2 MB**, which includes the
  bundled OCR models — a concrete, checkable number rather than a guess.
  [github.com/MaaXYZ/MaaFramework/releases/tag/v5.12.3](https://github.com/MaaXYZ/MaaFramework/releases/tag/v5.12.3)
- **Maintenance signal**: latest release v5.12.3 published 2026‑08‑01 (two weeks before this
  research); repo `pushed_at` 2026‑08‑15 (today); 2,938 commits, 4.7k stars, 529 forks,
  15 subscribers, org of ~35 related repos (tooling, GUI wrappers, per-game resource packs
  for other titles like Arknights, Limbus Company, Blue Archive, Honkai Impact 3rd).
  **No first-party or well-known community HSR "Pipeline" resource pack was found** in the
  MaaXYZ org or via search — HSR support is not current out of the box; the pipeline/task
  definitions for HSR dailies would have to be authored from scratch even if MaaFramework is
  adopted as the engine.
- **The honest downside**: it is a general black-box testing framework, not an HSR bot — GMM
  would still need to author every HSR-specific screen/flow definition. Documentation and
  community are overwhelmingly Chinese-language and Arknights-centric; the Rust binding is
  small (9 stars, 3 forks) and comparatively unproven at scale versus the C#/Python bindings.
  The Win32 controller's own docs concede there is "no universal method" for rendering/input
  on arbitrary Windows programs — expect to spend real time picking the right
  screencap/input combination for HSR's client specifically, and to revisit it if the client's
  renderer changes.

### MaaAssistantArknights (MAA proper)

The original, Arknights-specific application MaaFramework was extracted from.
[github.com/MaaAssistantArknights/MaaAssistantArknights](https://github.com/MaaAssistantArknights/MaaAssistantArknights)

- **License**: AGPL-3.0-only. Confirmed via GitHub's license API and by fetching the raw
  `LICENSE` file directly — it is the unmodified AGPLv3 text with **no additional terms**
  (no game-specific restrictions, no added clauses under GPLv3 §7).
  [api.github.com/repos/MaaAssistantArknights/MaaAssistantArknights](https://api.github.com/repos/MaaAssistantArknights/MaaAssistantArknights),
  [raw LICENSE](https://raw.githubusercontent.com/MaaAssistantArknights/MaaAssistantArknights/dev-v2/LICENSE)
- **License compatibility with GMM (GPL-3.0-or-later)**: per the FSF's own GPL FAQ, "Each of
  these licenses explicitly permits linking with code under the other license. You can always
  link GPLv3-covered modules with AGPLv3-covered modules, and vice versa."
  [gnu.org/licenses/gpl-faq.html](https://www.gnu.org/licenses/gpl-faq.html#AGPLv3CompatibleWithGPLv3)
  So it is *not legally blocked* — but per both licenses' §13, the combined work has to
  satisfy the requirements of **both** licenses simultaneously, which in practice means the
  combination inherits AGPL's network-source-disclosure obligation (§13) on top of everything
  GPLv3 already requires. GMM is a local desktop app, not a network service, so that
  obligation may rarely bite in practice — but it is a real, additional obligation ADR-0001
  did not have to consider, and it would attach to the automation component specifically.
- **Why it's not the pick anyway**: MAA is a finished Arknights application, not a reusable
  library — same architecture as MaaFramework underneath (it *is* built on MaaFramework's
  predecessor lineage) but with Arknights UI/pipeline baked in. Adopting it would mean forking
  an Arknights bot rather than building on the general-purpose engine it was extracted from.
  MaaFramework is the better unit to embed; MAA is useful only as a working reference for how
  a production Maa-based bot is structured.
- **Maintenance**: extremely active — `pushed_at` 2026‑08‑15 (today), 22.6k★, 19,868 commits
  on `dev-v2`, 659 open issues, Discord/QQ/Telegram channels live.

### March7thAssistant

The closest prior art: an HSR-specific, Windows-native automation tool.
[github.com/moesnow/March7thAssistant](https://github.com/moesnow/March7thAssistant)

- **License**: GPL-3.0, confirmed via GitHub's license API.
  [api.github.com/repos/moesnow/March7thAssistant](https://api.github.com/repos/moesnow/March7thAssistant)
  Directly compatible with GMM's GPL-3.0-or-later — no compatibility question at all.
- **Language/ABI**: pure Python (PyQt-Fluent-Widgets for the GUI). There is no C ABI and no
  Rust bindings — the only integration path is spawning it (or a Python runtime derived from
  it) as a **sidecar process** and talking over stdio/IPC, or porting its logic. It does not
  depend on MaaFramework or any other shared engine underneath; its automation stack is
  hand-built on `OpenCV` + `PyAutoGUI` + `RapidOCR`, with `ONNX Runtime`/`OpenVINO` for model
  acceleration.
  [raw README.md](https://raw.githubusercontent.com/moesnow/March7thAssistant/main/README.md)
- **Input path**: Win32 desktop automation of the native HSR PC client via `PyAutoGUI`
  (keyboard/mouse simulation), with an additional "云·星穹铁道" (cloud-streamed HSR) mode for
  background/headless/Docker execution. This is a genuine Windows PC-client automation tool,
  not an ADB/emulator tool — the single closest match to GMM's stated target (same game, same
  host, same input surface).
- **Vision approach**: `RapidOCR` for text recognition plus OpenCV template matching, with
  ONNX Runtime/OpenVINO for inference acceleration.
- **Maintenance signal**: `pushed_at` 2026‑08‑15 (today), 11.2k★, 350 forks, 91 open issues —
  actively maintained and, because it targets HSR specifically, its flow definitions track
  game patches by construction (unlike MaaFramework, which would need HSR pipelines authored
  from scratch).
- **The honest downside**: it is a Python desktop application, not a library — embedding it
  into a Rust/Tauri app means bundling a Python interpreter and its dependency tree (OpenCV,
  ONNX Runtime, PyQt) as a sidecar, which is exactly the "second toolchain" cost ticket #88
  asks about. Its flow logic (screens, coordinates, task sequencing) lives in Python source,
  not a declarative format GMM could read/patch independently, so any adaptation means reading
  and modifying someone else's Python codebase rather than authoring data. There's also no
  guarantee its GPL-3.0 pipeline/task code decomposes cleanly from its GUI — it would likely
  need surgery to strip out the standalone-app parts (auto-updater, its own GUI, Discord
  notification hooks) if the goal is "borrow the HSR flows" rather than "run their whole app."

### better-genshin-impact (BetterGI)

PC-client prior art for a *different* HiYoYo game (Genshin Impact), useful as an architecture
reference rather than a direct dependency.
[github.com/babalae/better-genshin-impact](https://github.com/babalae/better-genshin-impact)

- **License**: GPL-3.0, confirmed via GitHub's license API — compatible with GMM.
  [api.github.com/repos/babalae/better-genshin-impact](https://api.github.com/repos/babalae/better-genshin-impact)
- **Language/ABI**: C# / .NET 8, standalone WPF desktop app. No C ABI, no library packaging —
  same "spawn a sidecar or port the logic" integration story as March7thAssistant, except the
  sidecar here would be a .NET runtime instead of Python.
- **Input path**: Win32 `SendInput` against the native game window; requires administrator
  privileges to simulate mouse/keyboard input; explicitly does not read/write game memory or
  modify game files (a design constraint GMM's own ticket #81 already independently adopted).
- **Vision approach**: custom CV pipeline plus OCR, no third-party engine dependency.
- **Maintenance**: `pushed_at` 2026‑08‑15 (today), 14.8k★, 3,407 commits — very active, but
  for the wrong game (Genshin, not Star Rail); its value here is purely as a second working
  example of "Win32 SendInput + vision, no memory access" architecture, reinforcing that this
  is a well-trodden and legally survivable pattern for HoYoverse titles specifically.

### uiautomator2

Android accessibility-tree automation. Directly usable only if the emulator substrate is
chosen (which ticket #81 explicitly frames as *not* the destination).
[github.com/openatx/uiautomator2](https://github.com/openatx/uiautomator2)

- **License**: MIT, confirmed via GitHub's license API — no compatibility concerns at all.
  [api.github.com/repos/openatx/uiautomator2](https://api.github.com/repos/openatx/uiautomator2)
- **Language/ABI**: Python client talking JSON-RPC over HTTP to an on-device server (Java/
  Kotlin, open-sourced separately). No C ABI, no Rust bindings.
- **Input path**: ADB only, driving Google's UiAutomator2 accessibility framework — this reads
  and taps the Android **view hierarchy**, not pixels, and only ever reaches an Android
  target (device or emulator). It has no path to a native Windows PC-client window at all.
- **Vision approach**: none — it is element-tree automation, not vision-based. HSR (like most
  Unity-engine gacha games) does not expose a meaningful native Android view hierarchy for
  its in-game UI, which is typically one opaque rendering surface — this is the same practical
  limitation that pushed Poco toward engine-specific SDK integration instead of raw
  UiAutomator.
- **Maintenance**: `pushed_at` 2026‑08‑07, 8.3k★ — active, but architecturally irrelevant
  unless GMM commits to the Android-emulator substrate, and even then it would only automate
  Android chrome around the game, not the game's own rendered UI.

### Airtest / Poco (NetEase)

- **License**: Apache-2.0 for both, confirmed via GitHub's license API — no compatibility
  concerns.
  [api.github.com/repos/AirtestProject/Airtest](https://api.github.com/repos/AirtestProject/Airtest),
  [api.github.com/repos/AirtestProject/Poco](https://api.github.com/repos/AirtestProject/Poco)
- **Language/ABI**: both pure Python. No C ABI, no Rust bindings — same sidecar-or-port story
  as the Python/C# candidates above.
- **Input path**: Airtest supports ADB (Android), `pywin32` (Windows), and an iOS agent —
  genuinely cross-platform, including a real Windows path.
- **Vision approach**: Airtest does template-image matching. Poco is the complementary
  piece — it walks the **live UI-element hierarchy** of supported game engines (Unity3D,
  Cocos2d-x, native Android/iOS) via an in-process SDK, which is more precise than template
  matching *if* the target game ships (or can be made to ship) the Poco SDK — HSR does not,
  and getting HoYoverse to embed a third-party debug SDK in a live client is not realistic.
- **Maintenance — the deciding factor against this pair**: Airtest's repo was last pushed
  2026‑03‑23 (about five months stale as of this research). **Poco's repo was last pushed
  2024‑01‑08 — effectively unmaintained for roughly two and a half years**, confirmed via
  GitHub API `pushed_at`.
  [api.github.com/repos/AirtestProject/Poco](https://api.github.com/repos/AirtestProject/Poco)
  NetEase's own AirtestIDE product has moved on from active open-source investment in this
  pairing; picking it now means picking a slowing/stalled dependency for a feature meant to
  survive future HSR patches.

### Appium

- **License**: Apache-2.0 for Appium core (one dependency package, `@appium/logger`, uses
  ISC — both are GPL-compatible, non-issue), confirmed via GitHub's license API.
  [api.github.com/repos/appium/appium](https://api.github.com/repos/appium/appium)
- **Language/ABI**: TypeScript/JavaScript core implementing the W3C WebDriver protocol, with
  per-platform "driver" plugins. No C ABI; a Rust integration would mean either running a
  Node.js sidecar server or implementing a WebDriver HTTP client in Rust to talk to it.
- **Input path / Windows viability**: Windows desktop automation requires the **WinAppDriver**
  plugin, developed by Microsoft as a *separate* repository (MIT license). Checked directly:
  `microsoft/WinAppDriver`'s `pushed_at` is **2025‑04‑14** (well over a year stale as of this
  research), it carries `"activeRepoStatus": "false"` in its own repo custom properties, and
  has 1,155 open issues.
  [api.github.com/repos/microsoft/WinAppDriver](https://api.github.com/repos/microsoft/WinAppDriver)
  Appium core itself is very active (`pushed_at` 2026‑08‑14), but the specific piece GMM would
  need — the Windows desktop driver — is not.
- **Vision approach**: none built in. Appium/WebDriver locators are accessibility-tree/
  element-based (like uiautomator2), not template-match or OCR based. HSR's rendered game UI
  is not exposed as accessible Windows UI Automation elements any more than it is as an
  Android view hierarchy, so Appium would need a vision layer bolted on top regardless — at
  which point Appium's WebDriver machinery is pure overhead for this use case.
- **Verdict basis**: wrong shape of tool (element-based, not vision-based) *and* the one
  Windows-relevant driver is stale. Ruled out on both grounds independently.

### scrcpy + raw ADB

- **License**: Apache-2.0, confirmed via GitHub's license API — no compatibility concerns.
  [api.github.com/repos/Genymobile/scrcpy](https://api.github.com/repos/Genymobile/scrcpy)
- **Language/ABI**: C, built on FFmpeg + SDL2. Runs as a Windows host application, but its
  entire purpose is mirroring and controlling an **Android** device/emulator over ADB — it
  provides no vision layer, no OCR, no template matching, and no path whatsoever to a native
  Windows PC-client window. It is a transport/display primitive, not a game-automation engine.
- **Maintenance**: extremely active (`pushed_at` 2026‑08‑14, 147.7k★, largest project in this
  survey by a wide margin) — but that popularity is for its actual purpose (Android screen
  mirroring), not this one.
- **Verdict**: solves the substrate problem ticket #81 explicitly said is *not* the
  destination (Android/emulator input), and contributes nothing toward the Windows PC-client
  path even if the emulator substrate were chosen later, since it still provides no vision or
  flow layer on top of the raw pixels/input it moves.

### Baseline: write it ourselves, in Rust

- **What we'd actually be writing**: Win32 screen capture (GDI or `Windows.Graphics.Capture`
  via `windows-rs`), input injection (`SendInput` via `windows-rs`), template matching
  (`image`/`imageproc` crates, straightforward), OCR (either shell out to Tesseract, use
  `tesseract-rs` FFI bindings, or run a fetched ONNX OCR model via the `ort` crate — the same
  PaddleOCR-family models MAA/MaaFramework/March7th already use, since those are published
  separately on Hugging Face/PaddleOCR's own release channels, not something any of the above
  engines invented), a flow/pipeline definition format (ours to design — directly answers
  ticket #87's "what is a flow" question on our own terms), and a scheduler/recovery loop.
- **Honest downside**: every capability the survey asked about — "screen capture,
  template/OCR matching, input dispatch, flow/task definition language, scheduling,
  recovery" — is individually easy and collectively a real amount of work, most of it in the
  "no universal method" territory MaaFramework's own docs warn about for Win32 capture/input
  (different games render and accept input differently; getting HSR's specific client working
  reliably, including under DirectX overlay/fullscreen-exclusive quirks, is exactly the kind
  of native-Windows engineering ADR-0001 already flagged as a weak spot for this project — "no
  native-Windows background" was cited there as a reason to embed rather than rewrite). Vision
  robustness (handling resolution/DPI scaling, UI theme, animation timing) is the part every
  surveyed engine has years of accumulated bug-fixing behind that a fresh Rust implementation
  would not. This is the only candidate with zero HSR-specific or even genre-specific
  precedent baked in — 100% of patch-rot resilience would have to be earned from scratch.
  It is also the only option with **zero new license obligations**, and the only one that
  produces a single, fully GMM-controlled Rust artifact with no sidecar or FFI surface at all.

## Recommendation

**Adopt MaaFramework as the underlying engine, consumed via its Rust binding
(`maa-framework-rs`) and its Win32 controller**, and author HSR-specific pipeline/flow
definitions ourselves (there is no ready-made HSR resource pack to inherit). Do not adopt MAA,
March7thAssistant, or BetterGI wholesale as dependencies — treat March7thAssistant
specifically as a **reference implementation** to study for HSR-specific screen sequences and
edge cases (its GPL-3.0 license means its logic can legally inform a from-scratch
implementation of GMM's own pipelines, the same "study but don't copy" posture ADR-0001
already established for XXMI Launcher).

**Why MaaFramework over the alternatives:**

1. It is the only candidate that is simultaneously (a) a real library with a C ABI and an
   official, currently-maintained Rust binding — not a Python/C# standalone app requiring a
   sidecar and a second language runtime — and (b) built with a first-class Win32 desktop
   controller that treats native Windows PC-client automation as a primary target, not an
   ADB-only afterthought.
2. Its license (LGPL-3.0) is the easiest of any credible candidate to embed into a
   GPL-3.0-or-later Rust binary: dynamic/FFI linking is explicitly permitted without pulling
   GMM's own code under a stricter copyleft, unlike AGPL-3.0 (MAA), which — while technically
   linkable per the FSF — would additionally attach AGPL's network-source-disclosure term to
   the combined work.
3. It ships bundled OCR (~69 MB release artifact, a real number, not an estimate) and
   template-matching out of the box, which is most of the "would we otherwise write
   ourselves" list from the ticket, while leaving flow/task authoring — which is HSR-specific
   regardless of engine choice — to us.
4. It is the most actively maintained engine in the survey by recency of release (v5.12.3,
   2026‑08‑01) among the ones that are actual libraries rather than finished apps.

**What this recommendation does NOT resolve, and hands to the blocked tickets:**

- **#88** (process/crate topology, GPLv3 obligations of what's linked): this research
  establishes MaaFramework as LGPL-3.0 and FFI/Rust-bindable, but not whether it should be
  vendored, statically linked, or run as a sidecar — that's a topology decision, not a license
  one, and #88 should make it with LGPLv3 §4's relinking requirement in mind (whichever
  topology is chosen must let a user swap in a modified MaaFramework build).
- **#87** (scripted vs. LLM-in-loop brain): MaaFramework's Pipeline protocol is a deterministic
  JSON flow format; it does not itself decide whether an LLM sits in the loop. That decision
  is independent of this engine choice — MaaFramework's plugin/custom-recognition hooks would
  be the seam if a hybrid is chosen later.
- **HSR pipeline authoring cost**: unlike March7thAssistant, MaaFramework buys us the engine,
  not the flows. Building and maintaining HSR-specific Pipeline JSON (login, claim,
  assignments, stamina spend, exit) is real, ongoing work that this recommendation does not
  make disappear — it only avoids re-inventing screen capture, input injection, and OCR/
  template matching to get there.

## Sources

- [github.com/MaaXYZ/MaaFramework](https://github.com/MaaXYZ/MaaFramework) — README, license
- [api.github.com/repos/MaaXYZ/MaaFramework](https://api.github.com/repos/MaaXYZ/MaaFramework) — license, activity metadata
- [docs/en_us/2.4-ControlMethods.md](https://github.com/MaaXYZ/MaaFramework/blob/main/docs/en_us/2.4-ControlMethods.md) — controller types
- [github.com/MaaXYZ/MaaFramework/releases/tag/v5.12.3](https://github.com/MaaXYZ/MaaFramework/releases/tag/v5.12.3) — latest release, asset sizes
- [github.com/MaaXYZ/maa-framework-rs](https://github.com/MaaXYZ/maa-framework-rs) — Rust binding
- [github.com/MaaXYZ/maa-framework-tauri-template](https://github.com/MaaXYZ/maa-framework-tauri-template) — Tauri template (archived)
- [github.com/MaaAssistantArknights/MaaAssistantArknights](https://github.com/MaaAssistantArknights/MaaAssistantArknights) — README
- [api.github.com/repos/MaaAssistantArknights/MaaAssistantArknights](https://api.github.com/repos/MaaAssistantArknights/MaaAssistantArknights) — license, activity metadata
- [raw LICENSE (MAA)](https://raw.githubusercontent.com/MaaAssistantArknights/MaaAssistantArknights/dev-v2/LICENSE) — full AGPLv3 text, no added terms
- [gnu.org/licenses/gpl-faq.html#AGPLv3CompatibleWithGPLv3](https://www.gnu.org/licenses/gpl-faq.html#AGPLv3CompatibleWithGPLv3) — FSF on AGPL/GPL linking
- [github.com/moesnow/March7thAssistant](https://github.com/moesnow/March7thAssistant) — README
- [api.github.com/repos/moesnow/March7thAssistant](https://api.github.com/repos/moesnow/March7thAssistant) — license, activity metadata
- [raw README.md (March7thAssistant)](https://raw.githubusercontent.com/moesnow/March7thAssistant/main/README.md)
- [github.com/babalae/better-genshin-impact](https://github.com/babalae/better-genshin-impact) — README
- [api.github.com/repos/babalae/better-genshin-impact](https://api.github.com/repos/babalae/better-genshin-impact) — license, activity metadata
- [github.com/openatx/uiautomator2](https://github.com/openatx/uiautomator2) — README
- [api.github.com/repos/openatx/uiautomator2](https://api.github.com/repos/openatx/uiautomator2) — license, activity metadata
- [github.com/AirtestProject/Airtest](https://github.com/AirtestProject/Airtest) — README
- [api.github.com/repos/AirtestProject/Airtest](https://api.github.com/repos/AirtestProject/Airtest) — license, activity metadata
- [github.com/AirtestProject/Poco](https://github.com/AirtestProject/Poco) — README
- [api.github.com/repos/AirtestProject/Poco](https://api.github.com/repos/AirtestProject/Poco) — license, activity metadata (`pushed_at: 2024-01-08`)
- [github.com/appium/appium](https://github.com/appium/appium) — README
- [api.github.com/repos/appium/appium](https://api.github.com/repos/appium/appium) — license, activity metadata
- [api.github.com/repos/microsoft/WinAppDriver](https://api.github.com/repos/microsoft/WinAppDriver) — activity metadata, `activeRepoStatus: false`
- [github.com/Genymobile/scrcpy](https://github.com/Genymobile/scrcpy) — README
- [api.github.com/repos/Genymobile/scrcpy](https://api.github.com/repos/Genymobile/scrcpy) — license, activity metadata
- [Derek-X-Wang/gmm#81](https://github.com/Derek-X-Wang/gmm/issues/81), [#82](https://github.com/Derek-X-Wang/gmm/issues/82), [#87](https://github.com/Derek-X-Wang/gmm/issues/87), [#88](https://github.com/Derek-X-Wang/gmm/issues/88) — ticket context
- [docs/adr/0001-gplv3-and-embed-3dmloader.md](../adr/0001-gplv3-and-embed-3dmloader.md) — GMM's existing GPLv3 posture
