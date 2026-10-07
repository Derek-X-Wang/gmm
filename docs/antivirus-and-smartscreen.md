# Antivirus and SmartScreen

GMM is a Windows desktop app that injects a `3dmigoto`-derived Model Importer DLL
into a running gacha-game process. Windows Defender, third-party antivirus
products, and SmartScreen all flag that pattern as suspicious even though it
is exactly what the app is supposed to do. This page is the canonical
long-form guide referenced by the in-app launch guidance and first-run
onboarding wizard. Their short labels are maintained separately in
`src-tauri/src/core/av.rs`; see *In-app surface* below.

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

You can verify a GMM build by comparing its SHA-256 against the value on the
release page. In PowerShell, replace the example path below with the path to
the artefact you downloaded, then compare the command's `Hash` value with the
matching SHA-256 in the release's **Verify the download** table:

```powershell
Get-FileHash -LiteralPath <downloaded-file> -Algorithm SHA256
```

The release workflow lists every release artefact reported by the bundler,
including installer packages and their updater signature files. GMM does not
ship loose binaries outside its installer packages.

## How to add an exclusion in Windows Defender

1. Open **Windows Security** (`Settings → Privacy & security → Windows
   Security`) → **Virus & threat protection** → **Manage settings** →
   **Add or remove exclusions**.
2. Click **Add an exclusion** → **Folder**, and pick the GMM install
   directory (the current MSI default is `%ProgramFiles%\GMM\`, verified
   by the Windows installer smoke job). Select the folder that actually
   contains your installed `GMM.exe`: right-click the GMM Start menu
   shortcut → **Open file location**, then right-click the shortcut there
   → **Properties** and read **Target**. Use that executable's parent
   folder if your installation differs from the default.
3. Repeat for the GMM data directory (default `%AppData%\GMM\`) so the
   vendored `3dmloader.dll` and the per-game backups are not re-scanned on
   every launch. This is your Windows roaming app-data folder, independent
   of where the MSI installed GMM.
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
the in-app error surfaces a short guidance summary and this document's
path, with the OS error available under **Underlying error**.

The launch-error component renders the following Rust-owned summary.
The onboarding wizard reuses the same summary and document path. These
short labels deliberately omit paths and menu details; those live in
the long-form instructions above. `src-tauri/tests/av.rs` checks that
the headline, body and each label are mentioned in this document. That
substring check establishes copy coverage, not semantic agreement: it does
not prove a path or menu instruction correct. Review changes to the
summary and long-form guide together when their meaning changes.

- **Headline:** Antivirus or SmartScreen may have blocked the launch.
- **Body:** GMM loads a 3dmigoto-derived Model Importer DLL into your
  game's process so mods can take effect. Generic Defender + SmartScreen
  heuristics flag that shape, even though the binary is doing exactly
  what it is supposed to do.
- **Steps:**
  - Open Windows Security
  - Add an exclusion
  - Restart GMM after adding the exclusion
  - Restore from quarantine before re-adding the exclusion

`AV_GUIDANCE_DOC` embeds this document for coverage checks; `guidance()`
constructs the rendered payload from Rust constants, not by parsing the
Markdown. The README maintains a separate brief guide and links here.
