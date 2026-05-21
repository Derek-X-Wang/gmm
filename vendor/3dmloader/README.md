# Vendored: `3dmloader.dll`

Pre-built copy of `3dmloader.dll` from upstream
[`SpectrumQT/XXMI-Libs-Package`](https://github.com/SpectrumQT/XXMI-Libs-Package),
which is itself a fork of [`bo3b/3Dmigoto`](https://github.com/bo3b/3Dmigoto).

## Pinned version

| Field | Value |
|------:|-------|
| Upstream tag | `v0.8.8` |
| Upstream commit | (see [release page](https://github.com/SpectrumQT/XXMI-Libs-Package/releases/tag/v0.8.8)) |
| Released | 2026-03-28 |
| `3dmloader.dll` SHA-256 | `e3d59eb9647ef7c884e97792b2c58107b2bc4d42f9fc07047f81be7d4b81aef7` |
| File size | 20 480 bytes |

The upstream `Manifest.json` is included alongside the DLL so the vendored binary's signature can be verified against what the project author published.

## Why we vendor

ADR 0001 commits GMM to GPLv3 so the project can embed `3dmloader.dll` directly via Rust FFI rather than route through a process boundary. Vendoring the binary instead of fetching it at build time:

- removes a network dependency from every fresh build
- pins the exact version + bytes that the GMM source tree was tested against
- makes air-gapped builds and forks possible without coordination with upstream

The cost is one ~20 kB binary committed to the repo. We accept it.

## How to upgrade

When XXMI-Libs-Package ships a new release we want to track:

1. Verify the upstream tag (e.g. `v0.8.9`) on GitHub and skim its release notes for hook / inject API changes. **Any change to the four entry points means the FFI binding may need to change too.**
2. Download the new release zip:
   ```bash
   gh release download <tag> --repo SpectrumQT/XXMI-Libs-Package \
       --pattern '*.zip' --output xxmi-libs.zip
   ```
3. Extract only `3dmloader.dll` into `vendor/3dmloader/`, discarding `d3d11.dll` and `d3dcompiler_47.dll` (they belong to the per-game Model Importer packages, not to us):
   ```bash
   unzip -o xxmi-libs.zip 3dmloader.dll -d vendor/3dmloader/
   ```
4. Replace `Manifest.json` from the same release.
5. Re-fetch `LICENSE.GPL.txt` and `COPYING.txt` from `master` if they have changed upstream.
6. Update this README's "Pinned version" table (tag + SHA-256 + size + date).
7. Run `cargo xtask test-loader` on a Windows host to confirm the FFI binding still works against the new binary. If symbol changes broke anything, fix `crates/loader/src/ffi.rs` first.

## How to rebuild from source

Per the GPLv3 distribution requirements (LICENSE.GPL.txt § 6):

1. Clone the upstream repo at the pinned tag:
   ```bash
   git clone --depth 1 --branch v0.8.8 \
       https://github.com/SpectrumQT/XXMI-Libs-Package.git
   ```
2. Open `XXMI-Libs-Package.sln` in Visual Studio 2022 (the upstream project uses MSVC).
3. Build the `InjectorLib` project in `Release | x64`. The output is `3dmloader.dll`.
4. Compare its SHA-256 to the value in this README. Note that Microsoft compilers do not produce bit-identical builds across machines; if you need an authoritative reproduction, ask the upstream maintainer for the exact build configuration.

## License

`3dmloader.dll`, `Manifest.json`, `LICENSE.GPL.txt`, and `COPYING.txt` in this directory are GPLv3. The GMM project is GPLv3 (see top-level `LICENSE`), so the licenses match — see ADR 0001.

**Attribution required by GPLv3:** users who receive a GMM build that includes this DLL have the right to a copy of the corresponding source. The upstream source is at <https://github.com/SpectrumQT/XXMI-Libs-Package> at the pinned tag above. We satisfy GPLv3 § 6 by linking to that source — if upstream ever deletes the repository, we will mirror the source archive into this directory and update this section.
