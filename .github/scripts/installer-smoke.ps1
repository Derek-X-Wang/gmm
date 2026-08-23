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
      5. that log carries the IPC readiness marker, i.e. the WebView
         actually invoked a command and the Rust side answered
      6. the process is still alive after startup (no crash loop)
      7. msiexec uninstalls cleanly

    Criterion 5 is the one that distinguishes a working app from one
    whose UI is entirely broken: the DB and the log both appear on a
    build where every command is denied or unregistered, because the
    Rust side comes up regardless of whether the frontend can reach it.
    See issue #54.

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
$AppProc = $null
$ManifestListener = $null
$HeldManifestConnection = $null

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
    if ($null -ne $AppProc) {
        Stop-Process -Id $AppProc.Id -Force -ErrorAction SilentlyContinue
    }
    if ($null -ne $HeldManifestConnection) { $HeldManifestConnection.Dispose() }
    if ($null -ne $ManifestListener) { $ManifestListener.Stop() }
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

$dbPath = Join-Path $AppData "gmm.db"
$logDir = Join-Path $AppData "logs"

# Must match `IPC_READY_MARKER` in src-tauri/src/core/diagnostics.rs.
# tests/ipc_contract.rs fails if the two drift apart.
$IpcReadyMarker = "gmm-ipc-ready"
# Must match `MANIFEST_REFRESH_STARTED_MARKER`. This is separate from
# readiness: seeing both while the HTTP response is held open proves the
# refresh ran and did not block the usable application behind the network.
$ManifestRefreshStartedMarker = "gmm-manifest-refresh-started"
# This is the terminal event emitted by the startup refresh thread after the
# held-open request reaches the production client's own timeout.
$ManifestRefreshFinishedMessage = "recommended-importers refresh finished"
# Must match `MANIFEST_URL_OVERRIDE_ENV` in recommended_importers.rs.
$ManifestUrlOverrideEnv = "GMM_RECOMMENDED_IMPORTERS_URL"

function Get-DiagnosticMarkerCount($marker) {
    if (-not (Test-Path $logDir)) { return 0 }
    @(Get-ChildItem $logDir -Filter *.log -ErrorAction SilentlyContinue |
        Select-String -SimpleMatch $marker).Count
}

function Get-DiagnosticEventTimestamp($needle, $previousCount) {
    $matchingLines = @(Get-ChildItem $logDir -Filter *.log -ErrorAction SilentlyContinue |
        Sort-Object FullName |
        ForEach-Object { Get-Content $_.FullName } |
        Where-Object { $_.Contains($needle) })
    if ($matchingLines.Count -le $previousCount) { return $null }

    try {
        $event = $matchingLines[$previousCount] | ConvertFrom-Json
        if ($null -eq $event.timestamp) {
            throw "event has no timestamp"
        }
        return [System.DateTimeOffset]::Parse([string]$event.timestamp)
    } catch {
        throw "could not parse timestamp for diagnostic event '$needle': $_"
    }
}

# Accept the refresh request but never answer it. The startup guard is event
# ordering, not the ordinary 90-second liveness deadline: a blocking startup
# logs refresh completion before IPC readiness, while a background refresh
# lets IPC become ready before the client's own 20-second timeout completes.
$ManifestListener = [System.Net.Sockets.TcpListener]::new(
    [System.Net.IPAddress]::Loopback,
    0
)
$ManifestListener.Start()
$manifestPort = ([System.Net.IPEndPoint]$ManifestListener.LocalEndpoint).Port
$manifestAccept = $ManifestListener.AcceptTcpClientAsync()
$manifestUrl = "http://127.0.0.1:$manifestPort/recommended-importers.json"

$ipcBefore = Get-DiagnosticMarkerCount $IpcReadyMarker
$manifestRefreshBefore = Get-DiagnosticMarkerCount $ManifestRefreshStartedMarker
$manifestRefreshFinishedBefore = Get-DiagnosticMarkerCount $ManifestRefreshFinishedMessage
$previousManifestUrl = [System.Environment]::GetEnvironmentVariable(
    $ManifestUrlOverrideEnv,
    [System.EnvironmentVariableTarget]::Process
)
[System.Environment]::SetEnvironmentVariable(
    $ManifestUrlOverrideEnv,
    $manifestUrl,
    [System.EnvironmentVariableTarget]::Process
)
try {
    $AppProc = Start-Process $exe -PassThru
} finally {
    [System.Environment]::SetEnvironmentVariable(
        $ManifestUrlOverrideEnv,
        $previousManifestUrl,
        [System.EnvironmentVariableTarget]::Process
    )
}
Write-Host "launched pid $($AppProc.Id) with a held-open manifest endpoint"

$deadline = (Get-Date).AddSeconds(90)
$dbSeen = $false
$logSeen = $false
$ipcSeen = $false
$manifestRefreshSeen = $false
$manifestRefreshFinishedSeen = $false
$manifestRequestSeen = $false
$ipcReadyAt = $null
$manifestRefreshFinishedAt = $null

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
    if ($logSeen) {
        if (-not $ipcSeen -and
            (Get-DiagnosticMarkerCount $IpcReadyMarker) -gt $ipcBefore) {
            $ipcSeen = $true
            $ipcReadyAt = Get-DiagnosticEventTimestamp $IpcReadyMarker $ipcBefore
            Write-Host "new IPC readiness marker seen (frontend reached the backend)"
        }
        if (-not $manifestRefreshSeen -and
            (Get-DiagnosticMarkerCount $ManifestRefreshStartedMarker) -gt $manifestRefreshBefore) {
            $manifestRefreshSeen = $true
            Write-Host "new manifest-refresh marker seen"
        }
        if (-not $manifestRefreshFinishedSeen -and
            (Get-DiagnosticMarkerCount $ManifestRefreshFinishedMessage) -gt
                $manifestRefreshFinishedBefore) {
            $manifestRefreshFinishedSeen = $true
            $manifestRefreshFinishedAt = Get-DiagnosticEventTimestamp `
                $ManifestRefreshFinishedMessage `
                $manifestRefreshFinishedBefore
            Write-Host "manifest refresh reached its terminal event"
        }
    }
    if (-not $manifestRequestSeen -and $manifestAccept.IsCompletedSuccessfully) {
        $HeldManifestConnection = $manifestAccept.Result
        $manifestRequestSeen = $true
        Write-Host "manifest request accepted and deliberately left unanswered"
    }
    if ($dbSeen -and $logSeen -and $ipcSeen -and
        $manifestRefreshSeen -and $manifestRefreshFinishedSeen -and
        $manifestRequestSeen) { break }

    if ($AppProc.HasExited) {
        throw "GMM exited early with code $($AppProc.ExitCode) before finishing startup"
    }
    Start-Sleep -Milliseconds 500
}

if (-not $dbSeen) { throw "timed out waiting for $dbPath" }
if (-not $logSeen) { throw "timed out waiting for a log file in $logDir" }
if (-not $ipcSeen) {
    throw "timed out waiting for the IPC readiness marker '$IpcReadyMarker' in $logDir — " +
          "the backend started but the frontend never completed a command round-trip " +
          "(unregistered command, ACL denial, a WebView that never loaded, or " +
          "startup work blocked Tauri past the deadline)"
}
if (-not $manifestRefreshSeen) {
    throw "timed out waiting for a new manifest-refresh marker " +
          "'$ManifestRefreshStartedMarker' in $logDir"
}
if (-not $manifestRequestSeen) {
    throw "timed out waiting for the manifest refresh to reach $manifestUrl"
}
if (-not $manifestRefreshFinishedSeen) {
    throw "timed out waiting for the held-open manifest refresh to reach its terminal event"
}
if ($ipcReadyAt -ge $manifestRefreshFinishedAt) {
    throw "IPC readiness at $ipcReadyAt did not precede manifest refresh completion at " +
          "$manifestRefreshFinishedAt — startup appears to be waiting on the network"
}
Write-Host "IPC readiness preceded manifest refresh completion"

# A crash-on-idle would show up here.
Start-Sleep -Seconds 5
if ($AppProc.HasExited) {
    throw "GMM exited with code $($AppProc.ExitCode) shortly after startup"
}
Write-Host "process still alive after startup — no crash loop"

# ---------------------------------------------------------------------
Write-Section "Shut down"

# Must happen before reading gmm.db: the running app holds the SQLite
# file open, and a read while it's locked fails with a sharing violation.
Stop-Process -Id $AppProc.Id -Force -ErrorAction SilentlyContinue
$AppProc = $null
if ($null -ne $HeldManifestConnection) {
    $HeldManifestConnection.Dispose()
    $HeldManifestConnection = $null
}
$ManifestListener.Stop()
$ManifestListener = $null
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
