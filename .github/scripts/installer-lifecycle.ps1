<#
.SYNOPSIS
    MSI upgrade, repair and uninstall against realistic user state.

.DESCRIPTION
    `installer-smoke.ps1` covers install → launch → uninstall on a clean
    machine. Upgrade is the path every *existing* user takes, and it was
    untested — as were repair and the uninstall policy (#57).

    An upgrade that duplicates the install, orphans app data, fails to
    replace a locked file, or silently drops an Importer Pin would ship
    unnoticed, because a fresh install is exactly what a broken upgrade
    looks like to a clean-install smoke test.

    Runs after `updater-e2e.ps1` in the same job and **reuses the two
    MSIs that script already built**. Building a third and fourth
    `tauri build --release` in a sibling job would roughly double the
    Windows CI bill for overlapping coverage — the triage on #57 called
    this out and it is the reason this is a second script rather than a
    second job.

    Sequence:
      1. install version N (9.9.0)
      2. launch it once so migrations run
      3. seed realistic state through GMM's own Core: a Library entry, an
         enabled Mod, a live Junction into a game directory, a recorded
         game install path, and an Importer Pin
      4. assert exactly one Add/Remove Programs entry, and zero startup
         registrations
      5. upgrade to N+1 (9.9.1) and assert it replaced rather than
         duplicated — one entry, one install directory, binaries moved
      6. assert every seeded invariant survived
      7. delete a shipped binary, run an MSI repair, assert the binary is
         restored byte-for-byte and the user data is untouched
      8. uninstall, and assert the documented policy — install directory
         gone, `%APPDATA%\GMM` kept, Junctions left in place

    The uninstall policy is documented in README.md under "Uninstalling".
    Asserting it here is what stops it drifting silently.

    See issue #57, ADR 0003 (the Library is user data) and ADR 0004 (the
    Importer Pin is an account-safety escape hatch).
#>

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$Work = Join-Path $RepoRoot "ci-updater"
$AppData = Join-Path $env:APPDATA "GMM"
$LogDir = Join-Path $RepoRoot "ci-diagnostics"
# Deliberately not under %APPDATA%: a game directory is somewhere else on
# the disk, and the uninstall policy turns on that distinction.
$GameDir = Join-Path $RepoRoot "ci-fake-game"
New-Item -ItemType Directory -Force -Path $LogDir | Out-Null

# Must match `IPC_READY_MARKER` in src-tauri/src/core/diagnostics.rs.
$IpcReadyMarker = "gmm-ipc-ready"

function Write-Section($msg) {
    Write-Host ""
    Write-Host "=== $msg ===" -ForegroundColor Cyan
}

trap {
    Write-Host "INSTALLER LIFECYCLE FAILED: $_" -ForegroundColor Red
    if (Test-Path (Join-Path $AppData "logs")) {
        Get-ChildItem (Join-Path $AppData "logs") -Filter *.log | ForEach-Object {
            Write-Host "--- $($_.Name) ---"
            Get-Content $_.FullName -Tail 60
        }
    }
    exit 1
}

function Get-InstalledExe {
    $candidates = @(
        (Join-Path $env:LOCALAPPDATA "Programs\GMM\GMM.exe"),
        (Join-Path ${env:ProgramFiles} "GMM\GMM.exe"),
        (Join-Path ${env:ProgramFiles(x86)} "GMM\GMM.exe")
    )
    $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
}

function Invoke-Msi($arguments, $logName) {
    $log = Join-Path $LogDir $logName
    $p = Start-Process msiexec.exe `
        -ArgumentList ($arguments + @("/quiet", "/norestart", "/l*v", "`"$log`"")) `
        -Wait -PassThru
    if ($p.ExitCode -ne 0) {
        if (Test-Path $log) { Get-Content $log -Tail 60 }
        throw "msiexec $($arguments -join ' ') exited $($p.ExitCode)"
    }
}

# Launch the installed app and wait until it proves the frontend reached
# the backend (#54's marker), then stop it so nothing holds gmm.db.
function Assert-AppStarts($exe) {
    $proc = Start-Process $exe -PassThru
    try {
        $logs = Join-Path $AppData "logs"
        $deadline = (Get-Date).AddSeconds(90)
        while ((Get-Date) -lt $deadline) {
            if ((Test-Path $logs) -and
                (Get-ChildItem $logs -Filter *.log -ErrorAction SilentlyContinue |
                    ForEach-Object { Get-Content $_.FullName -Raw -ErrorAction SilentlyContinue } |
                    Select-String -SimpleMatch $IpcReadyMarker -Quiet)) {
                Write-Host "app reached a working state (IPC marker seen)"
                return
            }
            if ($proc.HasExited) {
                throw "GMM exited with code $($proc.ExitCode) before reaching a working state"
            }
            Start-Sleep -Milliseconds 500
        }
        throw "timed out waiting for GMM to reach a working state"
    } finally {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
        Start-Sleep -Seconds 3
    }
}

# Every Add/Remove Programs entry whose DisplayName is exactly GMM.
# Both hives and the WOW node, because which one the MSI writes to
# depends on install scope and that is not what this asserts.
function Get-UninstallEntries {
    $roots = @(
        "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*",
        "HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*",
        "HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*"
    )
    @(Get-ItemProperty -Path $roots -ErrorAction SilentlyContinue |
        Where-Object { $_.PSObject.Properties.Name -contains "DisplayName" -and $_.DisplayName -eq "GMM" })
}

# Anything that would make GMM launch itself at logon.
function Get-StartupRegistrations {
    $found = @()
    $runKeys = @(
        "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Run",
        "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\RunOnce",
        "HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Run",
        "HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\RunOnce"
    )
    foreach ($key in $runKeys) {
        if (-not (Test-Path $key)) { continue }
        $props = Get-ItemProperty -Path $key
        foreach ($p in $props.PSObject.Properties) {
            if ($p.Name -like "PS*") { continue }
            if ("$($p.Name) $($p.Value)" -match "GMM") {
                $found += "$key\$($p.Name) = $($p.Value)"
            }
        }
    }
    foreach ($dir in @($env:APPDATA, $env:ProgramData)) {
        $startup = Join-Path $dir "Microsoft\Windows\Start Menu\Programs\Startup"
        if (Test-Path $startup) {
            Get-ChildItem $startup -ErrorAction SilentlyContinue |
                Where-Object { $_.Name -match "GMM" } |
                ForEach-Object { $found += $_.FullName }
        }
    }
    @($found)
}

function Invoke-Fixture($op) {
    & $Fixture --data-dir $AppData --game-dir $GameDir $op
    if ($LASTEXITCODE -ne 0) {
        throw "lifecycle-fixture $op failed with exit code $LASTEXITCODE"
    }
}

# ---------------------------------------------------------------------
Write-Section "Locate the MSIs updater-e2e.ps1 built"

$oldMsi = (Get-ChildItem (Join-Path $Work "old") -Filter *.msi -ErrorAction SilentlyContinue |
    Select-Object -First 1)
$newMsi = (Get-ChildItem (Join-Path $Work "new") -Filter *.msi -ErrorAction SilentlyContinue |
    Select-Object -First 1)
if (-not $oldMsi -or -not $newMsi) {
    throw "expected one MSI under $Work\old and one under $Work\new — this script " +
          "reuses the bundles updater-e2e.ps1 built and must run after it"
}
Write-Host "old: $($oldMsi.Name)"
Write-Host "new: $($newMsi.Name)"

# Release rather than debug on purpose: `tauri build` has already
# compiled this whole dependency graph in release, so building the
# fixture reuses it and costs seconds. A debug build would recompile
# every dependency from scratch in a second profile.
$Fixture = Join-Path $RepoRoot "src-tauri\target\release\lifecycle-fixture.exe"
if (-not (Test-Path $Fixture)) {
    throw "lifecycle-fixture.exe missing at $Fixture — run " +
          "``cargo build --release -p gmm-lifecycle-fixture`` before this script"
}

# ---------------------------------------------------------------------
Write-Section "Clean slate"

if (Get-InstalledExe) { Invoke-Msi @("/x", "`"$($newMsi.FullName)`"") "msi-precleanup.log" }
if (Test-Path $AppData) { Remove-Item $AppData -Recurse -Force }
if (Test-Path $GameDir) { Remove-Item $GameDir -Recurse -Force }
New-Item -ItemType Directory -Force -Path (Join-Path $GameDir "Mods") | Out-Null

$before = Get-UninstallEntries
if ($before.Count -ne 0) {
    throw "expected no GMM install to begin with, found $($before.Count) entries"
}

# ---------------------------------------------------------------------
Write-Section "Install version N and give it realistic state"

Invoke-Msi @("/i", "`"$($oldMsi.FullName)`"") "msi-lifecycle-install-old.log"
$exe = Get-InstalledExe
if (-not $exe) { throw "GMM.exe not found after installing the old version" }
$installDir = Split-Path -Parent $exe
Write-Host "installed to $installDir"
Assert-AppStarts $exe

# Through Core's own API, so what is asserted later is the state GMM
# really produces — not a canary file standing in for it.
Invoke-Fixture "seed"

# ---------------------------------------------------------------------
Write-Section "Exactly one install, and nothing at startup"

$entries = Get-UninstallEntries
if ($entries.Count -ne 1) {
    $entries | ForEach-Object { Write-Host "  $($_.DisplayName) $($_.DisplayVersion) $($_.PSPath)" }
    throw "expected exactly 1 Add/Remove Programs entry after a fresh install, found $($entries.Count)"
}
Write-Host "one entry: $($entries[0].DisplayName) $($entries[0].DisplayVersion)"

# Restated from #57's "no duplicate startup registrations". The Tauri WiX
# template writes none at all, so the honest assertion is zero rather
# than "no duplicates" — which would pass vacuously forever.
$startup = Get-StartupRegistrations
if ($startup.Count -ne 0) {
    $startup | ForEach-Object { Write-Host "  $_" }
    throw "GMM registered $($startup.Count) startup entries; it is expected to register none"
}
Write-Host "no startup registrations, as expected"

# ---------------------------------------------------------------------
Write-Section "Upgrade to version N+1"

$exeHashBefore = (Get-FileHash $exe -Algorithm SHA256).Hash
Invoke-Msi @("/i", "`"$($newMsi.FullName)`"") "msi-lifecycle-upgrade.log"

$exe = Get-InstalledExe
if (-not $exe) { throw "GMM.exe missing after the upgrade" }
$installedVersion = (Get-Item $exe).VersionInfo.ProductVersion
Write-Host "installed version after upgrade: $installedVersion"
if (-not $installedVersion.StartsWith("9.9.1")) {
    throw "the upgrade did not take: exe reports $installedVersion, expected 9.9.1"
}

# "Binaries actually replaced" — the version string alone could come from
# a stale resource, so compare the bytes too.
$exeHashAfter = (Get-FileHash $exe -Algorithm SHA256).Hash
if ($exeHashAfter -eq $exeHashBefore) {
    throw "GMM.exe is byte-identical across the upgrade — the new build was not laid down"
}

# The failure #71's pinned UpgradeCode exists to prevent: an upgrade that
# is really a second, side-by-side install.
$entries = Get-UninstallEntries
if ($entries.Count -ne 1) {
    $entries | ForEach-Object { Write-Host "  $($_.DisplayName) $($_.DisplayVersion) $($_.PSPath)" }
    throw "upgrade produced $($entries.Count) Add/Remove Programs entries; a side-by-side " +
          "install is exactly what a broken UpgradeCode looks like"
}
if ((Split-Path -Parent $exe) -ne $installDir) {
    throw "the upgrade installed to $(Split-Path -Parent $exe) instead of replacing $installDir"
}
$startup = Get-StartupRegistrations
if ($startup.Count -ne 0) { throw "the upgrade added $($startup.Count) startup registrations" }
Write-Host "one entry, one install directory, binaries replaced"

Write-Section "Every seeded invariant survived the upgrade"
Invoke-Fixture "verify"
Assert-AppStarts $exe

# ---------------------------------------------------------------------
Write-Section "Repair restores binaries without touching user data"

# The realistic damage: a shipped file goes missing (antivirus quarantine
# is the common cause for this app — see docs/antivirus-and-smartscreen.md).
Remove-Item $exe -Force
if (Test-Path $exe) { throw "could not remove $exe to simulate damage" }

Invoke-Msi @("/f", "`"$($newMsi.FullName)`"") "msi-lifecycle-repair.log"

if (-not (Test-Path $exe)) { throw "repair did not restore $exe" }
$exeHashRepaired = (Get-FileHash $exe -Algorithm SHA256).Hash
if ($exeHashRepaired -ne $exeHashAfter) {
    throw "repair restored a different binary than the upgrade installed"
}
Write-Host "binary restored byte-for-byte"

Invoke-Fixture "verify"
Assert-AppStarts $exe

# ---------------------------------------------------------------------
Write-Section "Uninstall matches the documented policy"

$dbBefore = (Get-Item (Join-Path $AppData "gmm.db")).Length
Invoke-Msi @("/x", "`"$($newMsi.FullName)`"") "msi-lifecycle-uninstall.log"

# 1. The install directory goes.
if (Get-InstalledExe) { throw "GMM.exe still present after uninstall" }
$entries = Get-UninstallEntries
if ($entries.Count -ne 0) {
    throw "uninstall left $($entries.Count) Add/Remove Programs entries behind"
}

# 2. %APPDATA%\GMM stays. Per ADR 0003 the Library is the user's source
#    of truth and portable user data; the MSI never installed it and
#    cannot know a user-overridden path, so it must not delete it.
if (-not (Test-Path (Join-Path $AppData "gmm.db"))) {
    throw "uninstall deleted %APPDATA%\GMM\gmm.db — user data must survive"
}
$dbAfter = (Get-Item (Join-Path $AppData "gmm.db")).Length
if ($dbAfter -lt $dbBefore) {
    throw "gmm.db shrank across the uninstall ($dbBefore -> $dbAfter)"
}
if (-not (Test-Path (Join-Path $AppData "library"))) {
    throw "uninstall deleted the Library — it is user data, not program files"
}

# 3. Junctions stay. This is the one with teeth: the game keeps loading
#    those Mods after GMM is gone. Leaving them is deliberate — the
#    alternative is an uninstaller reaching into game directories — and
#    README.md tells the user how to clear them properly *before*
#    uninstalling.
Invoke-Fixture "verify"
Write-Host "install directory gone; %APPDATA%\GMM, Library and Junctions all intact"

Write-Section "INSTALLER LIFECYCLE PASSED"
