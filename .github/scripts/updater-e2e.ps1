<#
.SYNOPSIS
    End-to-end signed-update round trip against a throwaway key.

.DESCRIPTION
    `tests/updater_config.rs` checks that tauri.conf.json *says* the right
    things. This checks that the pipeline *does* them: that a build
    actually emits updater artifacts, that the signature over the real
    MSI zip verifies, that a tampered one does not, and that installing
    the newer build over the older one leaves the user's data alone.

    The very first release tag shipped without `createUpdaterArtifacts`,
    so installers went out with no update path at all. Step 3 is the one
    that would have caught it.

    Sequence:
      1. generate a throwaway signing keypair — never the release key,
         which stays a repository secret and is not read here
      2. build version OLD and version NEW with that key, both pointed at
         a local update endpoint
      3. assert NEW produced a signed updater artifact (the bundler's
         `*.sig` plus the file it signs — a raw `.msi` on the Tauri
         version this repo pins, a `.msi.zip` on older ones)
      4. serve NEW's `latest.json` + artifact over 127.0.0.1
      5. fetch both back through that endpoint and verify the signature
         the way tauri-plugin-updater does; assert a tampered artifact is
         refused
      6. install OLD, run it, leave user data under %APPDATA%\GMM
      7. install NEW over it, assert the installed version moved, the app
         still starts, and the user data survived
      8. uninstall

    See issue #56 and ADR 0004.
#>

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$BundleDir = Join-Path $RepoRoot "src-tauri\target\release\bundle\msi"
$AppData = Join-Path $env:APPDATA "GMM"
$Work = Join-Path $RepoRoot "ci-updater"
$LogDir = Join-Path $RepoRoot "ci-diagnostics"
New-Item -ItemType Directory -Force -Path $Work, $LogDir | Out-Null

# Versions are deliberately far above anything real so the MSI upgrade
# ordering is unambiguous no matter what the repo's version is.
$OldVersion = "9.9.0"
$NewVersion = "9.9.1"
$Port = 18317
$Canary = "updater-e2e-canary.txt"
$CanaryText = "user data must survive the update"
# Must match `IPC_READY_MARKER` in src-tauri/src/core/diagnostics.rs.
$IpcReadyMarker = "gmm-ipc-ready"

function Write-Section($msg) {
    Write-Host ""
    Write-Host "=== $msg ===" -ForegroundColor Cyan
}

function Get-InstalledExe {
    $candidates = @(
        (Join-Path $env:LOCALAPPDATA "Programs\GMM\GMM.exe"),
        (Join-Path ${env:ProgramFiles} "GMM\GMM.exe"),
        (Join-Path ${env:ProgramFiles(x86)} "GMM\GMM.exe")
    )
    $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
}

function Install-Msi($msi) {
    $log = Join-Path $LogDir ("msi-" + [System.IO.Path]::GetFileNameWithoutExtension($msi) + ".log")
    $p = Start-Process msiexec.exe `
        -ArgumentList "/i", "`"$msi`"", "/quiet", "/norestart", "/l*v", "`"$log`"" `
        -Wait -PassThru
    if ($p.ExitCode -ne 0) {
        if (Test-Path $log) { Get-Content $log -Tail 60 }
        throw "msiexec install of $msi exited $($p.ExitCode)"
    }
}

# Launch the installed app and wait until it proves the frontend reached
# the backend (issue #54's marker). Returns once it has, kills the app.
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

trap {
    Write-Host "UPDATER E2E FAILED: $_" -ForegroundColor Red
    if (Test-Path (Join-Path $AppData "logs")) {
        Get-ChildItem (Join-Path $AppData "logs") -Filter *.log | ForEach-Object {
            Write-Host "--- $($_.Name) ---"
            Get-Content $_.FullName -Tail 60
        }
    }
    exit 1
}

# ---------------------------------------------------------------------
Write-Section "Throwaway signing key"

# The release key lives in repository secrets and is never used by CI on
# a pull request. This one exists for the length of this job.
$KeyPath = Join-Path $Work "e2e-key"
Remove-Item "$KeyPath*" -Force -ErrorAction SilentlyContinue

# The key gets a real password rather than an empty one, and that is not
# cosmetic. PowerShell's default native-argument passing on Windows
# *drops* an empty-string argument entirely, so `-p ""` reached the
# Tauri CLI as no `-p` at all:
#
#   error: a value is required for '--password <PASSWORD>' but none was supplied
#
# (It works from a POSIX shell, which is why it survived review — the
# script was written on macOS. See $PSNativeCommandArgumentPassing.)
# A non-empty password also matches how the real release key is used,
# so this exercises the same code path releases do.
$KeyPassword = "updater-e2e"

pnpm tauri signer generate --ci -p $KeyPassword -w $KeyPath | Out-Null
if ($LASTEXITCODE -ne 0) { throw "tauri signer generate exited $LASTEXITCODE" }
if (-not (Test-Path "$KeyPath.pub")) { throw "signer generate produced no public key" }

$PubKey = (Get-Content "$KeyPath.pub" -Raw).Trim()
$env:TAURI_SIGNING_PRIVATE_KEY = (Get-Content $KeyPath -Raw).Trim()
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = $KeyPassword
Write-Host "throwaway pubkey: $($PubKey.Substring(0, 24))…"

# ---------------------------------------------------------------------
function Build-Version($version, $destDir) {
    Write-Section "Build $version"

    # Overrides go through a file: pwsh strips the quotes out of inline
    # --config JSON and tauri then rejects it.
    $conf = [ordered]@{
        version = $version
        bundle  = [ordered]@{ createUpdaterArtifacts = $true }
        plugins = [ordered]@{
            updater = [ordered]@{
                pubkey    = $PubKey
                endpoints = @("http://127.0.0.1:$Port/latest.json")
            }
        }
    }
    $confPath = Join-Path $Work "tauri.$version.conf.json"
    $conf | ConvertTo-Json -Depth 8 | Set-Content -Path $confPath -Encoding utf8

    if (Test-Path $BundleDir) { Remove-Item $BundleDir -Recurse -Force }
    pnpm tauri build --config $confPath
    if ($LASTEXITCODE -ne 0) { throw "tauri build ($version) exited $LASTEXITCODE" }

    New-Item -ItemType Directory -Force -Path $destDir | Out-Null
    Copy-Item (Join-Path $BundleDir "*") $destDir -Recurse -Force
    Get-ChildItem $destDir | ForEach-Object { Write-Host "  $($_.Name)" }
}

$OldDir = Join-Path $Work "old"
$NewDir = Join-Path $Work "new"
Build-Version $OldVersion $OldDir
Build-Version $NewVersion $NewDir

# ---------------------------------------------------------------------
Write-Section "Updater artifacts exist"

# What "the updater artifact" *is* has changed shape across Tauri
# versions: v1 and early v2 signed a zipped installer (`.msi.zip`), the
# 2.11 line this repo pins signs the raw `.msi`. This script was written
# against the old shape and asserted `.msi.zip`, so it would have failed
# on every correct build — the first execution of it said
# "createUpdaterArtifacts produced nothing" about a build that had just
# printed "Finished 2 updater signatures at: ...msi.sig".
#
# So derive the artifact from the signature rather than hardcoding
# either container. What matters is that *something* got signed and that
# the signature verifies over exactly those bytes; the extension is
# Tauri's business and is allowed to change again.
$sig = Get-ChildItem $NewDir -File |
    Where-Object { $_.Name.EndsWith(".sig") } |
    Select-Object -First 1
if (-not $sig) {
    $present = (Get-ChildItem $NewDir -File | ForEach-Object Name) -join ", "
    throw "no updater signature (*.sig) in $NewDir — createUpdaterArtifacts " +
          "produced nothing, which is exactly how the first release shipped " +
          "with no update path. Bundle contained: $present"
}
$artifactPath = $sig.FullName -replace '\.sig$', ''
if (-not (Test-Path $artifactPath)) {
    throw "signature $($sig.Name) has no artifact beside it at $artifactPath — " +
          "the update would have a signature over a file nobody ships"
}
$artifact = Get-Item $artifactPath
Write-Host "updater artifacts: $($artifact.Name) + $($sig.Name)"

$oldMsi = (Get-ChildItem $OldDir -Filter *.msi | Select-Object -First 1).FullName
$newMsi = (Get-ChildItem $NewDir -Filter *.msi | Select-Object -First 1).FullName
if (-not $oldMsi -or -not $newMsi) { throw "expected one MSI per build" }

# ---------------------------------------------------------------------
Write-Section "Serve the update over 127.0.0.1"

$Serve = Join-Path $Work "serve"
New-Item -ItemType Directory -Force -Path $Serve | Out-Null
Copy-Item $artifact.FullName $Serve -Force
Copy-Item $sig.FullName $Serve -Force

$latest = [ordered]@{
    version   = $NewVersion
    notes     = "updater e2e"
    pub_date  = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    platforms = [ordered]@{
        "windows-x86_64" = [ordered]@{
            signature = (Get-Content $sig.FullName -Raw).Trim()
            url       = "http://127.0.0.1:$Port/$($artifact.Name)"
        }
    }
}
$latest | ConvertTo-Json -Depth 8 | Set-Content -Path (Join-Path $Serve "latest.json") -Encoding utf8

$server = Start-Process python -ArgumentList "-m", "http.server", "$Port", "--bind", "127.0.0.1" `
    -WorkingDirectory $Serve -PassThru -WindowStyle Hidden
Start-Sleep -Seconds 3

try {
    $manifest = Invoke-RestMethod "http://127.0.0.1:$Port/latest.json"
    if ($manifest.version -ne $NewVersion) {
        throw "served manifest advertises $($manifest.version), expected $NewVersion"
    }
    $downloaded = Join-Path $Work ("downloaded" + [System.IO.Path]::GetExtension($artifact.Name))
    Invoke-WebRequest $manifest.platforms."windows-x86_64".url -OutFile $downloaded
    Write-Host "fetched $((Get-Item $downloaded).Length) bytes through the endpoint"

    # ------------------------------------------------------------------
    Write-Section "Verify the signature the way the app would"

    # The verification itself is `tauri-plugin-updater`'s own code path,
    # driven from the Rust test so the assertion and the shipped
    # implementation can't drift. It also tampers with a copy and
    # requires the rejection.
    $env:GMM_UPDATER_ARTIFACT = $downloaded
    $env:GMM_UPDATER_SIGNATURE = $manifest.platforms."windows-x86_64".signature
    $env:GMM_UPDATER_PUBKEY = $PubKey
    Push-Location (Join-Path $RepoRoot "src-tauri")
    try {
        cargo test --test updater_signature -- --ignored --exact `
            the_bundled_artifact_verifies_against_the_key_that_signed_it --nocapture
        if ($LASTEXITCODE -ne 0) { throw "signature verification of the real artifact failed" }
    } finally {
        Pop-Location
    }
} finally {
    Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue
}

# ---------------------------------------------------------------------
Write-Section "Install the old version and give it user data"

if (Test-Path $AppData) { Remove-Item $AppData -Recurse -Force }
Install-Msi $oldMsi
$exe = Get-InstalledExe
if (-not $exe) { throw "GMM.exe not found after installing $OldVersion" }
$installedOld = (Get-Item $exe).VersionInfo.ProductVersion
Write-Host "installed version: $installedOld"
if (-not $installedOld.StartsWith($OldVersion)) {
    throw "expected the installed exe to report $OldVersion, got $installedOld"
}

Assert-AppStarts $exe

# Stand-in for a user's Library settings and mod state: whatever else
# the update does, it must not touch %APPDATA%\GMM.
Set-Content -Path (Join-Path $AppData $Canary) -Value $CanaryText -Encoding utf8
$dbBefore = (Get-Item (Join-Path $AppData "gmm.db")).Length

# ---------------------------------------------------------------------
Write-Section "Install the update over it"

# What tauri-plugin-updater does on Windows once verification passes:
# run the downloaded installer silently, then relaunch.
Install-Msi $newMsi

$exe = Get-InstalledExe
if (-not $exe) { throw "GMM.exe missing after the update" }
$installedNew = (Get-Item $exe).VersionInfo.ProductVersion
Write-Host "installed version after update: $installedNew"
if (-not $installedNew.StartsWith($NewVersion)) {
    throw "the update did not take: exe still reports $installedNew, expected $NewVersion"
}

Write-Section "User data survived"

$canaryPath = Join-Path $AppData $Canary
if (-not (Test-Path $canaryPath)) { throw "the update deleted user data ($Canary is gone)" }
if ((Get-Content $canaryPath -Raw).Trim() -ne $CanaryText) {
    throw "the update rewrote user data ($Canary changed)"
}
$dbPath = Join-Path $AppData "gmm.db"
if (-not (Test-Path $dbPath)) { throw "the update deleted gmm.db" }
if ((Get-Item $dbPath).Length -lt $dbBefore) {
    throw "gmm.db shrank across the update ($dbBefore -> $((Get-Item $dbPath).Length))"
}
Write-Host "settings, database and canary all intact"

Write-Section "The updated app runs"
Assert-AppStarts $exe

# ---------------------------------------------------------------------
Write-Section "Uninstall"

$p = Start-Process msiexec.exe `
    -ArgumentList "/x", "`"$newMsi`"", "/quiet", "/norestart" -Wait -PassThru
if ($p.ExitCode -ne 0) { throw "msiexec uninstall exited $($p.ExitCode)" }

Write-Section "UPDATER E2E PASSED"
