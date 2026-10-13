$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

. (Join-Path $PSScriptRoot "updater-server-readiness.ps1")

# Observing both old ceilings expire would cost at least 13s + 15s before
# other tests run. Check the shipped defaults structurally to keep this suite
# under 30s; exercise the real polling and expiry behavior separately below.
foreach ($functionName in @("Wait-ForUpdateServerBind", "Wait-ForUpdateServer")) {
    $command = Get-Command $functionName
    $body = $command.ScriptBlock.Ast.Body
    $parameters = $body.ParamBlock.Parameters
    $timeoutParameter = @($parameters | Where-Object {
        $_.Name.VariablePath.UserPath -eq "TimeoutSeconds"
    })
    if ($timeoutParameter.Count -ne 1 -or $timeoutParameter[0].DefaultValue.SafeGetValue() -ne 120) {
        throw "$functionName must ship a 120s default readiness ceiling"
    }

    # A correct default is useless if validation rejects the CI ceiling or
    # the loop silently clamps it. Check the budget without waiting 120s.
    $timeoutRanges = @($command.Parameters["TimeoutSeconds"].Attributes | Where-Object {
        $_ -is [System.Management.Automation.ValidateRangeAttribute]
    })
    if ($timeoutRanges.Count -ne 1 -or $timeoutRanges[0].MinRange -gt 1 -or
        $timeoutRanges[0].MaxRange -lt 120) {
        throw "$functionName TimeoutSeconds validation must accept 1s tests and the 120s CI ceiling"
    }
    $loops = @($body.FindAll({
        param($node)
        $node -is [System.Management.Automation.Language.WhileStatementAst]
    }, $true))
    if ($loops.Count -ne 1) {
        throw "$functionName must have one readiness polling loop"
    }
    $condition = $loops[0].Condition.PipelineElements[0].Expression
    if ($condition -isnot [System.Management.Automation.Language.BinaryExpressionAst] -or
        $condition.Operator -ne [System.Management.Automation.Language.TokenKind]::Ilt -or
        $condition.Right -isnot [System.Management.Automation.Language.VariableExpressionAst] -or
        $condition.Right.VariablePath.UserPath -ne "TimeoutSeconds") {
        throw "$functionName polling loop must compare elapsed time directly against the unclamped TimeoutSeconds budget"
    }
    $timeoutAssignments = @($body.FindAll({
        param($node)
        $node -is [System.Management.Automation.Language.AssignmentStatementAst] -and
            $node.Left -is [System.Management.Automation.Language.VariableExpressionAst] -and
            $node.Left.VariablePath.UserPath -eq "TimeoutSeconds"
    }, $true))
    if ($timeoutAssignments.Count -ne 0) {
        throw "$functionName must not reassign the TimeoutSeconds budget"
    }
}

# Parse rather than execute the installer script: this is the ceiling that
# CI passes explicitly, independently of either helper's default.
$tokens = $null
$parseErrors = $null
$updaterAst = [System.Management.Automation.Language.Parser]::ParseFile(
    (Join-Path $PSScriptRoot "updater-e2e.ps1"), [ref]$tokens, [ref]$parseErrors
)
if ($parseErrors.Count -ne 0) {
    throw "updater-e2e.ps1 could not be parsed"
}
$startupAssignments = @($updaterAst.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.AssignmentStatementAst] -and
        $node.Left -is [System.Management.Automation.Language.VariableExpressionAst] -and
        $node.Left.VariablePath.UserPath -eq "ServerStartupTimeoutSeconds"
}, $true))
if ($startupAssignments.Count -ne 1 -or $startupAssignments[0].Right.Expression.SafeGetValue() -ne 120) {
    throw "updater-e2e.ps1 must pass a 120s server startup ceiling"
}
$readinessCalls = @($updaterAst.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.CommandAst] -and
        $node.GetCommandName() -like "Wait-ForUpdateServer*"
}, $true))
if ($readinessCalls.Count -eq 0) {
    throw "updater-e2e.ps1 must contain server readiness calls"
}
foreach ($call in $readinessCalls) {
    $timeoutArguments = @(
        for ($index = 0; $index -lt $call.CommandElements.Count; $index++) {
            $element = $call.CommandElements[$index]
            if ($element -is [System.Management.Automation.Language.CommandParameterAst] -and
                $element.ParameterName -eq "TimeoutSeconds") {
                if ($null -ne $element.Argument) {
                    $element.Argument
                } elseif ($index + 1 -lt $call.CommandElements.Count) {
                    $call.CommandElements[$index + 1]
                }
            }
        }
    )
    if ($timeoutArguments.Count -ne 1 -or
        $timeoutArguments[0] -isnot [System.Management.Automation.Language.VariableExpressionAst] -or
        $timeoutArguments[0].VariablePath.UserPath -ne "ServerStartupTimeoutSeconds") {
        $site = "updater-e2e.ps1:$($call.Extent.StartLineNumber):$($call.Extent.StartColumnNumber)"
        throw "$site $($call.GetCommandName()) must bind -TimeoutSeconds to `$ServerStartupTimeoutSeconds"
    }
}
Write-Host "both helper defaults and all $($readinessCalls.Count) CI readiness calls use the unclamped 120s ceiling"

function Test-DelayedReadiness {
    param(
        [ValidateSet("Bind", "Accept")]
        [string]$Kind,
        [System.Diagnostics.Process]$ServerProcess,
        [string]$StandardOutputPath
    )

    $port = Get-UnusedLoopbackPort
    $listener = [System.Net.Sockets.TcpListener]::new(
        [System.Net.IPAddress]::Loopback, $port
    )
    $prepared = [System.Threading.ManualResetEventSlim]::new($false)
    $start = [System.Threading.ManualResetEventSlim]::new($false)
    $state = [hashtable]::Synchronized(@{ Stopwatch = [System.Diagnostics.Stopwatch]::new(); ReadyAt = $null })
    $fixture = [PowerShell]::Create()
    try {
        # Warm up the independent runspace before timing the wait. The fixture
        # publishes readiness only after the wait has had time to poll and sleep.
        $null = $fixture.AddScript({
            param($kind, $path, $listener, $prepared, $start, $state)
            $ErrorActionPreference = "Stop"
            $prepared.Set()
            $start.Wait()
            Start-Sleep -Milliseconds 500
            $state.ReadyAt = $state.Stopwatch.Elapsed.TotalSeconds
            if ($kind -eq "Bind") {
                Set-Content $path "local update server bound to 127.0.0.1:12345"
            } else {
                $listener.Start()
            }
        }).AddArgument($Kind).AddArgument($StandardOutputPath).AddArgument($listener).
            AddArgument($prepared).AddArgument($start).AddArgument($state)
        $pending = $fixture.BeginInvoke()
        if (-not $prepared.Wait(5000)) { throw "$Kind fixture did not prepare in 5s" }
        $state.Stopwatch.Start()
        $start.Set()
        if ($Kind -eq "Bind") {
            $boundPort = Wait-ForUpdateServerBind -ServerProcess $ServerProcess `
                -StandardOutputPath $StandardOutputPath -TimeoutSeconds 3
            if ($boundPort -ne 12345) { throw "delayed bind returned port $boundPort" }
        } else {
            Wait-ForUpdateServer -Port $port -ServerProcess $ServerProcess -TimeoutSeconds 3
        }
        $elapsed = $state.Stopwatch.Elapsed.TotalSeconds
        $null = $fixture.EndInvoke($pending)
        if ($fixture.HadErrors) { throw "$Kind fixture failed: $($fixture.Streams.Error)" }
        if ($null -eq $state.ReadyAt -or $elapsed -lt 0.5 -or $elapsed -ge 2 -or
            ($elapsed - $state.ReadyAt) -ge 1) {
            throw "delayed $Kind took ${elapsed}s (ready at $($state.ReadyAt)s); expected 0.5s to under 2s and under 1s after readiness"
        }
        Write-Host "delayed $Kind returned in ${elapsed}s, under 1s after readiness"
    } finally {
        $start.Set()
        $fixture.Stop()
        $fixture.Dispose()
        $listener.Stop()
        $prepared.Dispose()
        $start.Dispose()
    }
}

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
if ($stopwatch.Elapsed.TotalSeconds -lt 1 -or $stopwatch.Elapsed.TotalSeconds -ge 2) {
    throw "unused-port readiness check took $($stopwatch.Elapsed.TotalSeconds)s; expected 1s to under 2s"
}
if ($failure -notmatch [Regex]::Escape($unusedPort.ToString())) {
    throw "unused-port failure did not name port ${unusedPort}: $failure"
}
if ($failure -notmatch "timed out") {
    throw "unused-port failure was not the readiness deadline: $failure"
}
Write-Host "unused port $unusedPort failed with the readiness diagnostic in $($stopwatch.Elapsed.TotalSeconds)s (expected 1s to under 2s)"

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
    if ($stopwatch.Elapsed.TotalSeconds -lt 1 -or $stopwatch.Elapsed.TotalSeconds -ge 2) {
        throw "bind expiry took $($stopwatch.Elapsed.TotalSeconds)s; expected 1s to under 2s"
    }
    Write-Host "live server with no output failed as bind expiry in $($stopwatch.Elapsed.TotalSeconds)s (expected 1s to under 2s)"

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
    Write-Host "bind success returned on the published line in $($stopwatch.Elapsed.TotalSeconds)s with the default ceiling (expected under 5s)"

    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    try {
        $listener.Start()
        $listeningPort = ([System.Net.IPEndPoint]$listener.LocalEndpoint).Port
        $stopwatch.Restart()
        Wait-ForUpdateServer -Port $listeningPort -ServerProcess $liveProcess
        if ($stopwatch.Elapsed.TotalSeconds -ge 5) {
            throw "connection acceptance took $($stopwatch.Elapsed.TotalSeconds)s; expected under 5s"
        }
        Write-Host "connection acceptance returned in $($stopwatch.Elapsed.TotalSeconds)s with the default ceiling (expected under 5s)"
    } finally {
        $listener.Stop()
    }

    Remove-Item $standardOutput
    Test-DelayedReadiness -Kind Bind -ServerProcess $liveProcess -StandardOutputPath $standardOutput
    Test-DelayedReadiness -Kind Accept -ServerProcess $liveProcess -StandardOutputPath $standardOutput

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
