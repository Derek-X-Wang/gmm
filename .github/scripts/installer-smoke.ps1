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
$ManifestPeerReadBuffer = $null
$ManifestPeerReadTask = $null
$ManifestAcceptCheckpoints = [System.Collections.Generic.List[string]]::new()
$ManifestPeerCheckpoints = [System.Collections.Generic.List[string]]::new()
$StartupAttemptCount = 0
$ManifestPeerClosedMessage =
    "manifest refresh client closed its held request before the fixture released its response"
$FailureClass = "PRODUCT"
$FixtureMode = $env:GMM_INSTALLER_SMOKE_FIXTURE_MODE

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

function Publish-SmokeFailure($failureClass, $message) {
    $title = "Installer smoke $failureClass failure"
    $annotationMessage = $message.Replace("%", "%25").Replace("`r", "%0D").Replace("`n", "%0A")
    Write-Host "::error title=${title}::$annotationMessage"
    Write-Host "$($title.ToUpperInvariant()): $message" -ForegroundColor Red

    if (Test-Path Env:GITHUB_OUTPUT) {
        "failure_class=$($failureClass.ToLowerInvariant())" |
            Add-Content -Path $env:GITHUB_OUTPUT -Encoding utf8
    }
    if (Test-Path Env:GITHUB_STEP_SUMMARY) {
        @(
            "### $title",
            "",
            $message
        ) | Add-Content -Path $env:GITHUB_STEP_SUMMARY -Encoding utf8
    }
}

function Stop-StartupAttempt {
    if ($null -ne $script:AppProc) {
        Stop-Process -Id $script:AppProc.Id -Force -ErrorAction SilentlyContinue
        $script:AppProc = $null
    }
    if ($null -ne $script:HeldManifestConnection) {
        $script:HeldManifestConnection.Dispose()
        $script:HeldManifestConnection = $null
    }
    $script:ManifestPeerReadBuffer = $null
    $script:ManifestPeerReadTask = $null
    if ($null -ne $script:ManifestListener) {
        $script:ManifestListener.Stop()
        $script:ManifestListener = $null
    }
}

function Confirm-ManifestFixtureListening($listener, $port) {
    $probeAccept = $listener.AcceptTcpClientAsync()
    $probeClient = [System.Net.Sockets.TcpClient]::new()
    $probeConnection = $null
    try {
        $probeClient.Connect([System.Net.IPAddress]::Loopback, $port)
        if (-not $probeAccept.Wait(5000)) {
            throw "manifest fixture did not accept its readiness probe on port $port within 5s"
        }
        $probeConnection = $probeAccept.GetAwaiter().GetResult()
    } finally {
        if ($null -ne $probeConnection) { $probeConnection.Dispose() }
        $probeClient.Dispose()
    }
    Write-Host "manifest fixture confirmed listening on 127.0.0.1:$port"
}

function Assert-ManifestFixtureAcceptHealthy($acceptTask, $checkpoint) {
    if ($acceptTask.IsFaulted) {
        $script:FailureClass = "INFRASTRUCTURE"
        $reason = $acceptTask.Exception.GetBaseException().Message
        throw "manifest fixture accept faulted after GMM launch: $reason"
    }
    if ($acceptTask.IsCanceled) {
        $script:FailureClass = "INFRASTRUCTURE"
        throw "manifest fixture accept was canceled after GMM launch"
    }
    $script:ManifestAcceptCheckpoints.Add($checkpoint)
}

function Start-ManifestFixturePeerMonitor {
    $script:ManifestPeerReadBuffer = [byte[]]::new(8192)
    $stream = $script:HeldManifestConnection.GetStream()
    $script:ManifestPeerReadTask = $stream.ReadAsync(
        $script:ManifestPeerReadBuffer,
        0,
        $script:ManifestPeerReadBuffer.Length
    )
}

function Assert-ManifestFixturePeerConnected($checkpoint) {
    if ($null -eq $script:ManifestPeerReadTask) { return }

    # Drain the request bytes without blocking, then leave another read pending.
    # A completed zero-byte read or a read fault means the client abandoned its
    # own in-flight refresh. That is PRODUCT behavior even though the fixture is
    # the side that observes it.
    while ($script:ManifestPeerReadTask.IsCompleted) {
        if ($script:ManifestPeerReadTask.IsFaulted) {
            $script:FailureClass = "PRODUCT"
            $reason = $script:ManifestPeerReadTask.Exception.GetBaseException().Message
            throw "$ManifestPeerClosedMessage`: $reason"
        }
        if ($script:ManifestPeerReadTask.IsCanceled) {
            $script:FailureClass = "PRODUCT"
            throw $ManifestPeerClosedMessage
        }

        $bytesRead = $script:ManifestPeerReadTask.GetAwaiter().GetResult()
        if ($bytesRead -eq 0) {
            $script:FailureClass = "PRODUCT"
            throw $ManifestPeerClosedMessage
        }

        $stream = $script:HeldManifestConnection.GetStream()
        $script:ManifestPeerReadTask = $stream.ReadAsync(
            $script:ManifestPeerReadBuffer,
            0,
            $script:ManifestPeerReadBuffer.Length
        )
    }
    $script:ManifestPeerCheckpoints.Add($checkpoint)
}

function Assert-ManifestFixtureCheckpointOrder($guard, $observed, $required) {
    $previous = -1
    foreach ($checkpoint in $required) {
        $current = $observed.IndexOf($checkpoint)
        if ($current -le $previous) {
            $script:FailureClass = "INFRASTRUCTURE"
            throw ("installer smoke did not execute required manifest fixture " +
                   "$guard check '$checkpoint' in order; observed: " +
                   "$($observed -join ', ')")
        }
        $previous = $current
    }
}

function Assert-ManifestFixtureGuardCoverage {
    # Source scans cannot prove that a PowerShell command executes. This
    # assertion checks the stages observed by this Windows run instead, while
    # allowing extra guards and an arbitrary number of polling iterations.
    Assert-ManifestFixtureCheckpointOrder `
        "accept" `
        $script:ManifestAcceptCheckpoints `
        @("startup-poll", "startup-post-loop")
    Assert-ManifestFixtureCheckpointOrder `
        "peer" `
        $script:ManifestPeerCheckpoints `
        @(
            "startup-poll",
            "startup-post-loop",
            "release-pre-prefix",
            "release-pre-final-byte"
        )
}

function Complete-ManifestFixtureRequest {
    # Keep the final body byte separate: the HTTP response is not released to
    # GMM until that byte is written, so a peer close after the prefix cannot be
    # mistaken for a normal close after receiving a complete response.
    $responsePrefix = [System.Text.Encoding]::ASCII.GetBytes(
        "HTTP/1.1 200 OK`r`n" +
        "Content-Length: 30`r`n" +
        "Connection: close`r`n`r`n" +
        '{"schemaVersion":1,"games":{}'
    )
    $responseFinalByte = [System.Text.Encoding]::ASCII.GetBytes("}")
    $stream = $script:HeldManifestConnection.GetStream()
    # Keep the last pre-release check immediately beside the write. A FIN can
    # otherwise arrive after the startup loop's final check while the response
    # bytes and stream are being prepared.
    Assert-ManifestFixturePeerConnected "release-pre-prefix"
    # Deliberate mutation-proof seam: when CI selects this mode with a
    # temporarily shortened client timeout, GMM closes after the pre-write
    # check, during the response-work window that issue #219 exposed.
    if ($FixtureMode -eq "pause-after-prewrite-peer-check") {
        Write-Host "pausing after pre-write peer check for close-window mutation proof"
        Start-Sleep -Seconds 105
    }
    try {
        $stream.Write($responsePrefix, 0, $responsePrefix.Length)
        $stream.Flush()
    } catch {
        $script:FailureClass = "PRODUCT"
        throw "$ManifestPeerClosedMessage`: $($_.Exception.Message)"
    }
    # A graceful FIN may let the prefix write succeed. Recheck after that flush
    # and immediately before the final body byte releases the response.
    if ($false) {
        Assert-ManifestFixturePeerConnected "release-pre-final-byte"
    }
    try {
        $stream.Write($responseFinalByte, 0, $responseFinalByte.Length)
        $stream.Flush()
    } catch {
        $script:FailureClass = "PRODUCT"
        throw "$ManifestPeerClosedMessage`: $($_.Exception.Message)"
    }
    Assert-ManifestFixtureGuardCoverage
    $script:HeldManifestConnection.Dispose()
    $script:HeldManifestConnection = $null
    $script:ManifestPeerReadBuffer = $null
    $script:ManifestPeerReadTask = $null
    Write-Host "manifest fixture released the request after IPC readiness"
}

trap {
    Publish-SmokeFailure $FailureClass $_.Exception.Message
    Stop-StartupAttempt
    Dump-Diagnostics
    exit 1
}

# ---------------------------------------------------------------------
Write-Section "Locate MSI"

$msi = Get-ChildItem $BundleDir -Filter *.msi -ErrorAction SilentlyContinue |
    Select-Object -First 1
if (-not $msi) {
    throw "no .msi found under $BundleDir — did ``tauri build`` run?"
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
# fixture releases its response, which happens only after IPC is ready.
$ManifestRefreshFinishedMessage = "recommended-importers refresh finished"
# Must match `MANIFEST_URL_OVERRIDE_ENV` in recommended_importers.rs.
$ManifestUrlOverrideEnv = "GMM_RECOMMENDED_IMPORTERS_URL"

function Get-DiagnosticMarkerCount($marker) {
    if (-not (Test-Path $logDir)) { return 0 }
    @(Get-ChildItem $logDir -Filter *.log -ErrorAction SilentlyContinue |
        Select-String -SimpleMatch $marker).Count
}

function Invoke-StartupSmoke {
    $script:FailureClass = "INFRASTRUCTURE"
    if ($script:StartupAttemptCount -ne 0) {
        throw "installer smoke must not retry a failed product startup assertion"
    }
    $script:StartupAttemptCount++
    $script:ManifestAcceptCheckpoints.Clear()
    $script:ManifestPeerCheckpoints.Clear()
    if (Test-Path $AppData) {
        Write-Host "removing startup data before launch"
        Remove-Item $AppData -Recurse -Force
    }

    # Port zero delegates collision avoidance to Windows. Start is synchronous,
    # then a real accept proves the listener before the chosen port is passed to
    # the installed app. This is the installer-smoke equivalent of #203.
    $script:ManifestListener = [System.Net.Sockets.TcpListener]::new(
        [System.Net.IPAddress]::Loopback,
        0
    )
    $script:ManifestListener.Start()
    $manifestPort = ([System.Net.IPEndPoint]$script:ManifestListener.LocalEndpoint).Port
    Confirm-ManifestFixtureListening $script:ManifestListener $manifestPort

    if ($FixtureMode -and
        $FixtureMode -notin @(
            "unavailable",
            "unavailable-after-launch",
            "pause-after-prewrite-peer-check"
        )) {
        throw ("unknown GMM_INSTALLER_SMOKE_FIXTURE_MODE " +
               "'$FixtureMode'")
    }
    if ($FixtureMode -eq "unavailable") {
        $script:ManifestListener.Stop()
        $script:ManifestListener = $null
        throw "manifest fixture deliberately made unavailable after readiness confirmation"
    }

    # Accept the refresh request but do not answer until IPC is ready. The
    # loopback override's 120-second client timeout is deliberately longer than
    # this smoke's 90-second startup deadline. A blocking startup therefore
    # cannot escape the assertion through a client timeout, while a slow but
    # non-blocking startup cannot invert two independently scheduled timestamps.
    $manifestAccept = $script:ManifestListener.AcceptTcpClientAsync()
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
    $script:FailureClass = "PRODUCT"
    try {
        $script:AppProc = Start-Process $exe -PassThru
    } finally {
        [System.Environment]::SetEnvironmentVariable(
            $ManifestUrlOverrideEnv,
            $previousManifestUrl,
            [System.EnvironmentVariableTarget]::Process
        )
    }
    Write-Host "launched pid $($script:AppProc.Id) with held-open manifest endpoint"

    if ($FixtureMode -eq "unavailable-after-launch") {
        $script:ManifestListener.Stop()
        Write-Host "manifest fixture deliberately stopped after GMM launch"
    }

    $deadline = (Get-Date).AddSeconds(90)
    $dbSeen = $false
    $logSeen = $false
    $ipcSeen = $false
    $manifestRefreshSeen = $false
    $manifestRefreshFinishedSeen = $false
    $manifestRequestSeen = $false

    while ((Get-Date) -lt $deadline) {
        Assert-ManifestFixtureAcceptHealthy $manifestAccept "startup-poll"

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
                Write-Host "new IPC readiness marker seen (frontend reached the backend)"
            }
            if (-not $manifestRefreshSeen -and
                (Get-DiagnosticMarkerCount $ManifestRefreshStartedMarker) -gt
                    $manifestRefreshBefore) {
                $manifestRefreshSeen = $true
                Write-Host "new manifest-refresh marker seen"
            }
            if (-not $manifestRefreshFinishedSeen -and
                (Get-DiagnosticMarkerCount $ManifestRefreshFinishedMessage) -gt
                    $manifestRefreshFinishedBefore) {
                $manifestRefreshFinishedSeen = $true
                Write-Host "manifest refresh reached its terminal event"
            }
        }
        if (-not $manifestRequestSeen -and $manifestAccept.IsCompletedSuccessfully) {
            $script:HeldManifestConnection = $manifestAccept.Result
            Start-ManifestFixturePeerMonitor
            $manifestRequestSeen = $true
            Write-Host "manifest request accepted and deliberately left unanswered"
        }
        Assert-ManifestFixturePeerConnected "startup-poll"
        if ($manifestRefreshFinishedSeen) {
            throw "manifest refresh finished before the fixture released its held response"
        }
        if ($dbSeen -and $logSeen -and $ipcSeen -and
            $manifestRefreshSeen -and $manifestRequestSeen) { break }

        if ($script:AppProc.HasExited) {
            throw "GMM exited early with code $($script:AppProc.ExitCode) before finishing startup"
        }
        Start-Sleep -Milliseconds 500
    }

    if (-not $dbSeen) { throw "timed out waiting for $dbPath" }
    if (-not $logSeen) { throw "timed out waiting for a log file in $logDir" }
    Assert-ManifestFixtureAcceptHealthy $manifestAccept "startup-post-loop"
    if (-not $ipcSeen -and $manifestRefreshSeen -and $manifestRequestSeen) {
        throw "IPC readiness did not occur while the manifest request remained held open " +
              "and unanswered — startup did not prove independence from the network"
    }
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
    Assert-ManifestFixturePeerConnected "startup-post-loop"
    Write-Host "IPC readiness observed while manifest request was held open and unanswered"

    Complete-ManifestFixtureRequest
    $script:FailureClass = "PRODUCT"

    $refreshDeadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $refreshDeadline) {
        if ((Get-DiagnosticMarkerCount $ManifestRefreshFinishedMessage) -gt
            $manifestRefreshFinishedBefore) {
            $manifestRefreshFinishedSeen = $true
            Write-Host "manifest refresh reached its terminal event"
            break
        }
        if ($script:AppProc.HasExited) {
            throw "GMM exited early with code $($script:AppProc.ExitCode) before refresh completion"
        }
        Start-Sleep -Milliseconds 500
    }
    if (-not $manifestRefreshFinishedSeen) {
        throw "timed out waiting for manifest refresh completion after fixture response"
    }

    # A crash-on-idle would show up here.
    Start-Sleep -Seconds 5
    if ($script:AppProc.HasExited) {
        throw "GMM exited with code $($script:AppProc.ExitCode) shortly after startup"
    }
    Write-Host "process still alive after startup — no crash loop"

    # Must happen before reading gmm.db: the running app holds the SQLite
    # file open, and a read while it is locked fails with a sharing violation.
    Stop-StartupAttempt
    Start-Sleep -Seconds 3
}

Write-Section "Startup"
Invoke-StartupSmoke

# ---------------------------------------------------------------------
Write-Section "Shut down"

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
