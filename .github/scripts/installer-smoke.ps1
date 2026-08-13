<#
.SYNOPSIS
    Installs the built MSI, launches GMM headlessly, proves it reached a
    working state, then uninstalls.

.DESCRIPTION
    This is the automated replacement for "download the release on a
    Windows box and see if it opens". It exercises the artefacts a user
    actually receives — the MSI, the installed exe, WebView2 startup,
    SQLite migrations, and the logging subsystem — none of which
    `cargo test` touches.

    Success criteria, in order:
      1. msiexec installs silently with exit code 0
      2. the installed gmm.exe exists on disk
      3. launching it creates %APPDATA%\GMM\gmm.db (migrations ran)
      4. it creates %APPDATA%\GMM\logs\*.log (tracing subscriber up)
      5. the process is still alive after startup (no crash loop)
      6. msiexec uninstalls cleanly

    Any failure dumps the MSI verbose log and GMM's own logs to stdout so
    a CI reader can diagnose without a Windows machine.
#>

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$BundleDir = Join-Path $RepoRoot "src-tauri\target\release\bundle\msi"
$AppData = Join-Path $env:APPDATA "GMM"
# Logs live inside the workspace: actions/upload-artifact requires every
# uploaded path to share one root, and RUNNER_TEMP sits outside it.
$LogDir = Join-Path $RepoRoot "ci-diagnostics"
New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
$InstallLog = Join-Path $LogDir "msi-install.log"
$UninstallLog = Join-Path $LogDir "msi-uninstall.log"

function Write-Section($msg) {
    Write-Host ""
    Write-Host "=== $msg ===" -ForegroundColor Cyan
}

function Dump-Diagnostics {
    Write-Section "DIAGNOSTICS"

    if (Test-Path $InstallLog) {
        Write-Host "--- msiexec install log (last 80 lines) ---"
        Get-Content $InstallLog -Tail 80
    }

    $logDir = Join-Path $AppData "logs"
    if (Test-Path $logDir) {
        Get-ChildItem $logDir -Filter *.log | ForEach-Object {
            Write-Host "--- $($_.Name) ---"
            Get-Content $_.FullName -Tail 100
        }
    } else {
        Write-Host "(no GMM log directory at $logDir)"
    }

    Write-Host "--- recent Application event-log errors ---"
    Get-WinEvent -LogName Application -MaxEvents 40 -ErrorAction SilentlyContinue |
        Where-Object { $_.LevelDisplayName -eq "Error" } |
        Select-Object TimeCreated, ProviderName, Message |
        Format-List | Out-String | Write-Host
}

trap {
    Write-Host "SMOKE FAILED: $_" -ForegroundColor Red
    Dump-Diagnostics
    exit 1
}

# ---------------------------------------------------------------------
Write-Section "Locate MSI"

$msi = Get-ChildItem $BundleDir -Filter *.msi -ErrorAction SilentlyContinue |
    Select-Object -First 1
if (-not $msi) {
    throw "no .msi found under $BundleDir — did `tauri build` run?"
}
Write-Host "MSI: $($msi.FullName) ($([math]::Round($msi.Length / 1MB, 2)) MB)"

# Start from a clean slate so "the DB appeared" is unambiguous.
if (Test-Path $AppData) {
    Write-Host "removing pre-existing $AppData"
    Remove-Item $AppData -Recurse -Force
}

# ---------------------------------------------------------------------
Write-Section "Install (silent)"

$p = Start-Process msiexec.exe `
    -ArgumentList "/i", "`"$($msi.FullName)`"", "/quiet", "/norestart", "/l*v", "`"$InstallLog`"" `
    -Wait -PassThru
if ($p.ExitCode -ne 0) {
    throw "msiexec install exited $($p.ExitCode)"
}
Write-Host "install OK"

# ---------------------------------------------------------------------
Write-Section "Locate installed executable"

$candidates = @(
    (Join-Path $env:LOCALAPPDATA "Programs\GMM\GMM.exe"),
    (Join-Path ${env:ProgramFiles} "GMM\GMM.exe"),
    (Join-Path ${env:ProgramFiles(x86)} "GMM\GMM.exe")
)
$exe = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $exe) {
    # Fall back to a search so a bundler layout change doesn't silently
    # skip the launch half of the smoke.
    $exe = Get-ChildItem -Path $env:LOCALAPPDATA, ${env:ProgramFiles} `
        -Filter "GMM.exe" -Recurse -ErrorAction SilentlyContinue |
        Select-Object -First 1 -ExpandProperty FullName
}
if (-not $exe) {
    throw "GMM.exe not found after install; searched: $($candidates -join ', ')"
}
Write-Host "exe: $exe"

# ---------------------------------------------------------------------
Write-Section "Launch and verify startup"

$proc = Start-Process $exe -PassThru
Write-Host "launched pid $($proc.Id)"

$dbPath = Join-Path $AppData "gmm.db"
$logDir = Join-Path $AppData "logs"
$deadline = (Get-Date).AddSeconds(90)
$dbSeen = $false
$logSeen = $false

while ((Get-Date) -lt $deadline) {
    if (-not $dbSeen -and (Test-Path $dbPath)) {
        $dbSeen = $true
        Write-Host "gmm.db created (SQLite migrations ran)"
    }
    if (-not $logSeen -and (Test-Path $logDir) -and
        (Get-ChildItem $logDir -Filter *.log -ErrorAction SilentlyContinue)) {
        $logSeen = $true
        Write-Host "log file created (tracing subscriber up)"
    }
    if ($dbSeen -and $logSeen) { break }

    if ($proc.HasExited) {
        throw "GMM exited early with code $($proc.ExitCode) before finishing startup"
    }
    Start-Sleep -Milliseconds 500
}

if (-not $dbSeen) { throw "timed out waiting for $dbPath" }
if (-not $logSeen) { throw "timed out waiting for a log file in $logDir" }

# A crash-on-idle would show up here.
Start-Sleep -Seconds 5
if ($proc.HasExited) {
    throw "GMM exited with code $($proc.ExitCode) shortly after startup"
}
Write-Host "process still alive after startup — no crash loop"

# ---------------------------------------------------------------------
Write-Section "Shut down"

# Must happen before reading gmm.db: the running app holds the SQLite
# file open, and a read while it's locked fails with a sharing violation.
Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 3

# ---------------------------------------------------------------------
# The six supported games must be seeded by the initial migration.
Write-Section "Verify seeded schema"

$dbBytes = [System.IO.File]::ReadAllBytes($dbPath)
$dbText = [System.Text.Encoding]::ASCII.GetString($dbBytes)
foreach ($code in @("gimi", "srmi", "zzmi", "wwmi", "himi", "efmi")) {
    if ($dbText -notmatch $code) {
        throw "game code '$code' not present in gmm.db — migration seed incomplete"
    }
}
Write-Host "all six game codes present in gmm.db"

# ---------------------------------------------------------------------
Write-Section "Uninstall"

$p = Start-Process msiexec.exe `
    -ArgumentList "/x", "`"$($msi.FullName)`"", "/quiet", "/norestart", "/l*v", "`"$UninstallLog`"" `
    -Wait -PassThru
if ($p.ExitCode -ne 0) {
    throw "msiexec uninstall exited $($p.ExitCode)"
}
if (Test-Path $exe) {
    throw "uninstall left $exe behind"
}
Write-Host "uninstall OK"

Write-Section "SMOKE PASSED"
