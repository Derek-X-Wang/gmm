# Antivirus and SmartScreen

GMM is a Windows desktop app that injects a `3dmigoto`-derived Model Importer DLL
into a running gacha-game process. Windows Defender, third-party antivirus
products, and SmartScreen all flag that pattern as suspicious even though it
is exactly what the app is supposed to do. This page is the single source of
truth for that warning — the in-app guidance (see slice NEW-AV / #13) and the
first-run onboarding wizard (slice 16-b / #24) load their copy from this file.

## Why GMM looks suspicious

GMM does three things that read as malware-shaped to a generic heuristic:

- Holds `3dmloader.dll` (a 3dmigoto fork from `SpectrumQT/XXMI-Libs-Package`)
  in process and installs a Win32 CBT hook.
- Calls `LoadLibraryW` on a `d3d11.dll` proxy from inside another process'
  address space at game-launch time.
- Creates NTFS junctions from a game's `Mods/` directory into the GMM Library
  on demand (no admin rights, no Developer Mode — see ADR 0003).

None of these are malicious. They are also indistinguishable from the
behaviour of an actual DLL-injecting trainer or cheat tool when seen by a
signature-less heuristic.

## Why GMM is unsigned

We do not yet ship code signing. An Authenticode certificate is a recurring
cost we have deferred until v1 user numbers justify it; the XXMI ecosystem
GMM grew out of has the same gap. Until a signed build exists, every release
triggers SmartScreen's "Windows protected your PC" prompt on first launch.

You can verify a GMM build by comparing its SHA-256 against the value
attached to the release on GitHub. The release page lists the digest of the
installer and of the loose binaries.

## How to add an exclusion in Windows Defender

1. Open **Windows Security** (`Settings → Privacy & security → Windows
   Security`) → **Virus & threat protection** → **Manage settings** →
   **Add or remove exclusions**.
2. Click **Add an exclusion** → **Folder**, and pick the GMM install
   directory (default `%LocalAppData%\Programs\GMM\`).
3. Repeat for the GMM data directory (default `%AppData%\GMM\`) so the
   vendored `3dmloader.dll` and the per-game backups are not re-scanned on
   every launch.
4. Optionally exclude the `Mods/` directories inside each affected game's
   install so Defender does not scan every mod toggle.

Restart GMM after adding the exclusion. Defender does not re-evaluate
running processes until they relaunch.

## How to add an exclusion in common third-party antivirus products

The mechanism is the same shape in every product; only the menu copy
changes.

- **Norton 360 / Norton Security**: *Settings → Antivirus → Scans and
  Risks → Exclusions / Low Risks → Items to Exclude from Scans*. Add the
  GMM install directory and the GMM data directory.
- **Bitdefender**: *Protection → Antivirus → Settings → Manage
  Exceptions*. Add the same two folders.
- **Avast / AVG**: *Menu → Settings → General → Exceptions*. Add folder
  paths (Avast accepts wildcards, e.g. `%AppData%\GMM\*`).
- **ESET NOD32**: *Setup → Advanced setup → Detection engine → Exclusions
  → Performance exclusions*. Add the same two folders.
- **Kaspersky**: *Settings → Security → Threats and Exclusions → Manage
  exclusions*. Add the same two folders and select "Skip" for Object
  Action.

If your AV is not listed, search its docs for "scan exclusion" or
"trusted folder" — the concept is identical across products.

## What to do if the binary was auto-quarantined

If Defender or your AV removed `gmm.exe`, `3dmloader.dll`, or a Model
Importer DLL before you set up the exclusion, restore them first, then
add the exclusion so the restore is not re-quarantined immediately.

**Windows Defender:**

1. Open **Windows Security → Virus & threat protection → Protection
   history**.
2. Find the entry that names GMM, `3dmloader.dll`, or a `d3d11.dll`
   sitting inside the GMM install/data directory or a game install.
3. Click **Actions → Restore** (or **Allow on device** if Restore is
   greyed out).
4. Add the exclusion using the steps above.
5. Reinstall the Model Importer for any affected game from inside GMM
   (*Model Importer panel → Reinstall importer*) so the importer files
   are written fresh after the exclusion is in place.

**Third-party AV:**

Most products keep quarantined files in a vault inside the AV's own UI.
Look for *Quarantine*, *Virus Vault*, or *Threats* and restore from
there. After restoring, add the exclusion before relaunching GMM or your
game.

If the file cannot be restored (some AVs delete rather than quarantine),
reinstall GMM from the GitHub release and verify the SHA-256 matches the
release page.

## SmartScreen "Windows protected your PC"

On first launch you will see a SmartScreen prompt because the GMM
installer is unsigned.

1. Click **More info** on the SmartScreen prompt.
2. The dialog expands and now shows a **Run anyway** button. Click it.
3. The prompt does not repeat for subsequent launches of the same
   installer.

If your organisation has SmartScreen set to **Block**, only an
administrator can override. GMM cannot bypass this; you will need an
admin to allow the binary or to install GMM into a user-writable
location they have approved.

We deliberately do **not** ship a manifest workaround or use
application-compatibility shims to dodge SmartScreen. The right answer
is a signed binary, which we will ship when funding allows; the
short-term answer is to teach SmartScreen to trust this specific build.

## In-app surface

When a Launch action inside GMM fails with an OS-level error string
that matches a known AV / SmartScreen pattern (for example, "Operation
did not complete successfully because the file contains a virus", OS
error code 225 / `0x800700E1`, or a SmartScreen-related access denial),
the in-app error surfaces these exclusion instructions inline rather
than the raw error string.

The component renders the following copy (kept verbatim here so the
Rust `gmm_lib::core::av::guidance()` payload and the doc cannot drift —
tests in `src-tauri/tests/av.rs` enforce that):

- **Headline:** Antivirus or SmartScreen may have blocked the launch.
- **Body:** GMM loads a 3dmigoto-derived Model Importer DLL into your
  game's process so mods can take effect. Generic Defender + SmartScreen
  heuristics flag that shape, even though the binary is doing exactly
  what it is supposed to do.
- **Steps:**
  - Open Windows Security and add the GMM install + data folders to the
    Defender exclusion list (see *How to add an exclusion in Windows
    Defender* above).
  - Add an exclusion in your third-party AV (Norton / Bitdefender /
    Avast / AVG / ESET / Kaspersky) under its *Exceptions* or
    *Trusted folders* menu.
  - Restart GMM after adding the exclusion so Defender re-evaluates the
    process.
  - Restore from quarantine before re-adding the exclusion if your AV
    already removed the binary.

The same canonical text in this file is consumed by:

- The README's *Antivirus and SmartScreen* section.
- The in-app launch error component (slice NEW-AV / #13).
- The first-run onboarding wizard's AV disclosure step (slice 16-b /
  #24), which reuses this file's headline and read-more link.
