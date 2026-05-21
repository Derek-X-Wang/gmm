# 0001 — GMM is GPLv3, embeds 3dmloader.dll via Rust FFI

Date: 2026-05-20
Status: Accepted

## Context

GMM must inject a 3dmigoto-derived model importer (GIMI, SRMI, ZZMI, WWMI, HIMI, EFMI) into the target game process so mods load at runtime. The injection logic lives in `3dmloader.dll`, part of `SpectrumQT/XXMI-Libs-Package`, which is itself a fork of `bo3b/3Dmigoto`. The entire toolchain is GPLv3.

XXMI Launcher (Python) consumes `3dmloader.dll` via dlopen + ctypes — i.e. dynamic linking. Under any sane reading of GPLv3, dynamic linking propagates the license. XXMI is itself GPLv3, so this is not a problem there.

GMM has the same architectural requirement: it must hold the loader DLL in-process for the lifetime of the modded game session, calling Hook/Inject entry points. There is no zero-coupling escape hatch short of a separate subprocess or a clean-room rewrite.

## Considered alternatives

- **MIT/Apache GMM, isolate the GPLv3 loader in a `gmm-loader.exe` subprocess.** Process boundary firewalls the license. Doable, but introduces a two-binary footprint and an IPC channel for hook/inject state that XXMI did not need. Real engineering cost for a license benefit that buys us nothing (GMM has no proprietary downstream and no plausible relicensing future).
- **MIT GMM, write a clean-room loader in pure Rust + C++.** Removes all GPLv3 dependencies. Estimated months of native-Windows hook/inject work across six games. The current maintainer has no native-Windows background. High reliability tail.
- **Defer launching entirely — require the user to install XXMI separately for the loader.** Punts the question but contradicts the standalone-reimplementation decision (ADR 0002).

## Decision

GMM is GPLv3-licensed and embeds `3dmloader.dll` directly via Rust FFI. The full app is one binary. We commit to staying in license alignment with the rest of the 3dmigoto ecosystem.

## Consequences

- GMM source must remain GPLv3-compatible. We cannot link proprietary libraries. We cannot relicense without rewriting the loader subsystem.
- We track upstream `XXMI-Libs-Package` releases for `3dmloader.dll` updates (consumed via build-time download or vendored binary).
- We are free to study (but not copy) XXMI Launcher's Python source, which is also GPLv3 — reimplementations in Rust are fine and conventional.
- The "small Tauri binary" goal is preserved at the GMM layer; the GPLv3 footprint is the loader DLL only, not a full Python runtime.
