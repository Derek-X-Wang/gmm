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
} finally {
    if ($null -ne $server -and -not $server.HasExited) {
        Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue
    }
    Remove-Item $testRoot -Recurse -Force -ErrorAction SilentlyContinue
}
