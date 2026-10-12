$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

. (Join-Path $PSScriptRoot "updater-server-readiness.ps1")

function Get-UnusedLoopbackPort {
    $listener = [System.Net.Sockets.TcpListener]::new(
        [System.Net.IPAddress]::Loopback,
        0
    )
    try {
        $listener.Start()
        return ([System.Net.IPEndPoint]$listener.LocalEndpoint).Port
    } finally {
        $listener.Stop()
    }
}

$unusedPort = Get-UnusedLoopbackPort
$stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
$failure = $null
try {
    Wait-ForUpdateServer -Port $unusedPort -TimeoutSeconds 1
} catch {
    $failure = $_.Exception.Message
}

if ($null -eq $failure) {
    throw "readiness check unexpectedly accepted unused port $unusedPort"
}
if ($stopwatch.Elapsed.TotalSeconds -ge 5) {
    throw "unused-port readiness check took $($stopwatch.Elapsed.TotalSeconds)s; expected under 5s"
}
if ($failure -notmatch [Regex]::Escape($unusedPort.ToString())) {
    throw "unused-port failure did not name port ${unusedPort}: $failure"
}
if ($failure -notmatch "timed out") {
    throw "unused-port failure was not the readiness deadline: $failure"
}
Write-Host "unused port $unusedPort failed with the readiness diagnostic in under 5s"

$testRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("gmm-readiness-" + [Guid]::NewGuid())
$standardOutput = Join-Path $testRoot "server.stdout.log"
$standardError = Join-Path $testRoot "server.stderr.log"
$deadServerFixture = Join-Path $PSScriptRoot "test-fixtures/dead-update-server.ps1"
New-Item -ItemType Directory -Path $testRoot | Out-Null

$server = $null
try {
    # A live process with empty output models an interpreter that has not yet
    # reached its first print. It must expire, not claim a conflict or exit.
    $liveProcess = Get-Process -Id $PID
    $stopwatch.Restart()
    $failure = $null
    try {
        Wait-ForUpdateServerBind `
            -ServerProcess $liveProcess `
            -StandardOutputPath $standardOutput `
            -StandardErrorPath $standardError `
            -TimeoutSeconds 1
    } catch {
        $failure = $_.Exception.Message
    }

    if ($null -eq $failure -or
        $failure -notmatch '^timed out after [\d.]+s waiting for local update server to report a successful bind\. Server output: <no server output>$') {
        throw "bind expiry was not reported as expiry: $failure"
    }
    if ($stopwatch.Elapsed.TotalSeconds -lt 1 -or $stopwatch.Elapsed.TotalSeconds -ge 5) {
        throw "bind expiry took $($stopwatch.Elapsed.TotalSeconds)s; expected 1s to under 5s"
    }
    Write-Host "live server with no output failed as bind expiry in under 5s"

    Set-Content $standardError "local update server bind failed: address already in use"
    $failure = $null
    try {
        Wait-ForUpdateServerBind `
            -ServerProcess $liveProcess `
            -StandardOutputPath $standardOutput `
            -StandardErrorPath $standardError `
            -TimeoutSeconds 1
    } catch {
        $failure = $_.Exception.Message
    }
    if ($null -eq $failure -or
        $failure -notmatch '^local update server bind failed: address already in use after [\d.]+s$') {
        throw "bind conflict was not reported as a conflict: $failure"
    }
    Write-Host "bind conflict was distinguished from expiry and process exit"
    Remove-Item $standardError

    # Once the bind line is present, the generous default must not become
    # a fixed startup delay. Use the live test process to avoid timing another
    # interpreter's cold start as part of this early-return assertion.
    Set-Content $standardOutput "local update server bound to 127.0.0.1:12345"
    $stopwatch.Restart()
    $boundPort = Wait-ForUpdateServerBind `
        -ServerProcess $liveProcess `
        -StandardOutputPath $standardOutput `
        -StandardErrorPath $standardError
    if ($boundPort -ne 12345 -or $stopwatch.Elapsed.TotalSeconds -ge 5) {
        throw "bind success returned port $boundPort in $($stopwatch.Elapsed.TotalSeconds)s; expected port 12345 in under 5s"
    }
    Write-Host "bind success returned on the published line in under 5s with the default ceiling"

    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    try {
        $listener.Start()
        $listeningPort = ([System.Net.IPEndPoint]$listener.LocalEndpoint).Port
        $stopwatch.Restart()
        Wait-ForUpdateServer -Port $listeningPort -ServerProcess $liveProcess
        if ($stopwatch.Elapsed.TotalSeconds -ge 5) {
            throw "connection acceptance took $($stopwatch.Elapsed.TotalSeconds)s; expected under 5s"
        }
        Write-Host "connection acceptance returned in under 5s with the default ceiling"
    } finally {
        $listener.Stop()
    }

    $pwsh = (Get-Process -Id $PID).Path
    $server = Start-Process $pwsh `
        -ArgumentList "-NoProfile", "-File", "`"$deadServerFixture`"" `
        -RedirectStandardOutput $standardOutput `
        -RedirectStandardError $standardError `
        -PassThru

    $deadPort = Get-UnusedLoopbackPort
    $failure = $null
    try {
        Wait-ForUpdateServer `
            -Port $deadPort `
            -TimeoutSeconds 5 `
            -ServerProcess $server `
            -StandardOutputPath $standardOutput `
            -StandardErrorPath $standardError
    } catch {
        $failure = $_.Exception.Message
    }

    if ($null -eq $failure) {
        throw "readiness check accepted dead server process on port $deadPort"
    }
    foreach ($expected in @("exited during startup", "code 23", "$deadPort", "startup exploded")) {
        if ($failure -notmatch [Regex]::Escape($expected)) {
            throw "dead-server failure did not contain '$expected': $failure"
        }
    }
    Write-Host "dead server was distinguished from a refused connection and included its output"

    $failure = $null
    try {
        Wait-ForUpdateServerBind `
            -ServerProcess $server `
            -StandardOutputPath $standardOutput `
            -StandardErrorPath $standardError `
            -TimeoutSeconds 1
    } catch {
        $failure = $_.Exception.Message
    }
    if ($null -eq $failure) {
        throw "bind check accepted a dead server process"
    }
    foreach ($expected in @("process exited before reporting a successful bind", "code 23", "startup exploded")) {
        if ($failure -notmatch [Regex]::Escape($expected)) {
            throw "dead-server bind failure did not contain '$expected': $failure"
        }
    }
    if ($failure -match "timed out|bind failed:") {
        throw "dead-server bind failure was misreported as expiry or conflict: $failure"
    }
    Write-Host "dead server bind failure was distinguished from expiry and conflict and included its output"
} finally {
    if ($null -ne $server -and -not $server.HasExited) {
        Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue
    }
    Remove-Item $testRoot -Recurse -Force -ErrorAction SilentlyContinue
}
