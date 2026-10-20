[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidOverwritingBuiltInCmdlets', '',
    Justification = 'Test-local clock and process commands exercise unchanged production functions deterministically.')]
param([string]$Case = '*')

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
# The shared helpers and command mocks live in an isolated module scope.
# Consumer launch calls are exercised without running installer top-level code.
$contract = {
    param($scriptName, $RepoRoot, $Case)
    $ErrorActionPreference = 'Stop'
    Set-StrictMode -Version Latest
    $script:Checks = 0
    function Assert-That($condition, $message) { if (-not $condition) { throw $message } }
    function Check($name, [scriptblock]$test) {
        if ($name -notlike "*/$Case") { return }
        try { & $test } catch { throw "${name}: $_" }
        $script:Checks++
        Write-Host "PASS $name"
    }
    function Get-Date { $script:Now }
    function Start-Sleep {
        param($Milliseconds, $Seconds)
        $script:Sleeps++
        $script:Now = $script:Now.AddSeconds(10)
        if ($null -ne $script:OnSleep) { & $script:OnSleep }
        if ($script:Sleeps -gt 20) { throw 'poll was not bounded' }
    }
    function Start-Process { param($FilePath, [switch]$PassThru) return $script:Proc }
    function Stop-Process { param($Id, [switch]$Force, $ErrorAction) $script:Stopped = $true }
    function Stop-StartupAttempt {
        $script:Stopped = $true
        $script:AppProc.HasExited = $true
        $script:AppProc = $null
    }
    function Write-Warning { param($Message) $script:Warnings += $Message }
    function Reset-Clock {
        $script:Now = [datetime]'2026-10-16T00:00:00Z'
        $script:Sleeps = 0
        $script:OnSleep = $null
        $script:Stopped = $false
        $script:Warnings = @()
        $script:BusyPaths = @{}
        $script:ProbedPaths = @()
        $script:Probe = { param($path) $script:ProbedPaths += $path; return -not $script:BusyPaths.ContainsKey($path) }
        $script:Proc = [pscustomobject]@{ Id = 12345; HasExited = $true; ExitCode = 17; Disposed = $false }
        $script:Proc | Add-Member -MemberType ScriptMethod -Name Dispose -Value { $this.Disposed = $true }
    }
    function Assert-Failure([scriptblock]$operation, $pattern) {
        $failure = $null
        try { & $operation } catch { $failure = $_.ToString() }
        Assert-That ($null -ne $failure -and $failure -match $pattern) "expected '$pattern', got '$failure'"
    }
    # Mock clock/process commands and inject the file-release predicate. These
    # tests prove control flow, not Windows mapped-image sharing or MSI behavior.
    $root = Join-Path ([IO.Path]::GetTempPath()) ('gmm-app-starts-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $root | Out-Null
    try {
        $file = Join-Path $RepoRoot ".github/scripts/$scriptName.ps1"
        $tokens = $null; $parseErrors = $null
        $ast = [Management.Automation.Language.Parser]::ParseFile($file, [ref]$tokens, [ref]$parseErrors)
        Assert-That ($parseErrors.Count -eq 0) 'parse errors'
        $helperNames = @('Get-IpcMarkerCount', 'Test-AppFileReleased', 'Wait-AppLiveInstanceLock', 'Wait-AppResourcesReleased', 'Assert-AppStarts')
        Check "$scriptName/SharedDefinitionsOnly" {
            $statements = @($ast.EndBlock.Statements)
            Assert-That ($statements.Count -eq $helperNames.Count) 'shared file must contain only the expected helper definitions'
            foreach ($name in $helperNames) {
                $definitions = @($statements | Where-Object {
                    $_ -is [Management.Automation.Language.FunctionDefinitionAst] -and $_.Name -eq $name
                })
                Assert-That ($definitions.Count -eq 1) "shared file must define $name exactly once"
            }
        }
        . $file
        $fn = $ast.Find({ param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Assert-AppStarts' }, $true)
        $consumerCalls = @{}
        foreach ($consumer in @('installer-lifecycle', 'updater-e2e', 'installer-smoke')) {
            $consumerFile = Join-Path $RepoRoot ".github/scripts/$consumer.ps1"
            $consumerAst = [Management.Automation.Language.Parser]::ParseFile($consumerFile, [ref]$tokens, [ref]$parseErrors)
            Assert-That ($parseErrors.Count -eq 0) "$consumer parse errors"
            Check "$consumer/NoLocalLaunchHelpers" {
                $localHelpers = @($consumerAst.FindAll({
                    param($node)
                    $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -in $helperNames
                }, $true))
                Assert-That ($localHelpers.Count -eq 0) "$consumer must not redefine shared app-launch helpers locally"
            }
            Check "$consumer/SharedHelpersDotSourced" {
                # Require the established literal top-level dot-source form;
                # comments and unused nested imports cannot satisfy this guard.
                $imports = @($consumerAst.EndBlock.Statements | Where-Object {
                    $_ -is [Management.Automation.Language.PipelineAst] -and
                        $_.PipelineElements.Count -eq 1 -and
                        $_.PipelineElements[0] -is [Management.Automation.Language.CommandAst] -and
                        $_.PipelineElements[0].InvocationOperator -eq [Management.Automation.Language.TokenKind]::Dot -and
                        ($_.Extent.Text -replace '\s+', ' ') -eq '. (Join-Path $PSScriptRoot "app-starts.ps1")'
                })
                Assert-That ($imports.Count -eq 1) "$consumer must dot-source app-starts.ps1 exactly once at top level"
            }
            if ($consumer -eq 'installer-smoke') { $smokeAst = $consumerAst; continue }
            $consumerCalls[$consumer] = @($consumerAst.FindAll({
                param($node)
                $node -is [Management.Automation.Language.CommandAst] -and $node.GetCommandName() -eq 'Assert-AppStarts'
            }, $true))
        }
        # AST guards cover ordinary literal budget edits and direct sleep calls.
        # They do not prove arbitrary program semantics (aliases, computed command
        # names or dynamic invocation). Runtime tests below cover actual polling,
        # count increases, expiry, cleanup, and the consumer database arguments.
        Check "$scriptName/MeasuredLaunchBudget" {
            $writes = @($fn.Body.FindAll({
                param($node)
                $node -is [Management.Automation.Language.AssignmentStatementAst] -and
                    $node.Left -is [Management.Automation.Language.VariableExpressionAst] -and
                    ($node.Left.VariablePath.UserPath -replace '^[^:]+:', '') -eq 'deadline'
            }, $true))
            Assert-That ($writes.Count -eq 1) 'launch deadline must be assigned once'
            $budget = $writes[0].Right.Find({
                param($node)
                $node -is [Management.Automation.Language.InvokeMemberExpressionAst] -and
                    $node.Member.Value -eq 'AddSeconds'
            }, $true)
            Assert-That ($null -ne $budget -and $budget.Arguments.Count -eq 1 -and
                $budget.Arguments[0].SafeGetValue() -eq 90) 'measured launch ceiling must stay 90 seconds'
        }
        Check "$scriptName/NoFinallySleep" {
            $cleanup = @($fn.Body.FindAll({
                param($node)
                $node -is [Management.Automation.Language.TryStatementAst] -and $null -ne $node.Finally
            }, $true))
            Assert-That ($cleanup.Count -gt 0) 'missing launch cleanup'
            foreach ($block in $cleanup) {
                $sleeps = @($block.Finally.FindAll({
                    param($node)
                    $node -is [Management.Automation.Language.CommandAst] -and
                        $node.GetCommandName() -eq 'Start-Sleep'
                }, $true))
                Assert-That ($sleeps.Count -eq 0) 'finally must wait on resources, never a fixed sleep'
            }
        }
        $AppData = Join-Path $root $scriptName
        $logDir = Join-Path $AppData 'logs'
        New-Item -ItemType Directory -Path $logDir -Force | Out-Null
        $IpcReadyMarker = 'gmm-ipc-ready'
        $log = Join-Path $logDir 'current.log'
        [IO.File]::WriteAllText($log, "old $IpcReadyMarker" + [Environment]::NewLine)
        Check "$scriptName/IncrementalMarkerCount" {
            [IO.File]::WriteAllText($log, "old GMM-IPC-READY twice GMM-IPC-READY`nnoise`ngmm-ipc-")
            $state = @{}
            Assert-That ((Get-IpcMarkerCount $state) -eq 1) 'count lines rather than occurrences'
            $offset = $state[$log].Offset
            Assert-That ((Get-IpcMarkerCount $state) -eq 1 -and $state[$log].Offset -eq $offset) 'unchanged poll'
            # A prior line can no longer be read: only appended bytes may be scanned.
            [IO.File]::WriteAllText($log, ('x' * $offset))
            [IO.File]::AppendAllText($log, "ready`ngmm-ipc-ready")
            Assert-That ((Get-IpcMarkerCount $state) -eq 3) 'append-only scan and split marker'
            [IO.File]::AppendAllText($log, " more`n")
            Assert-That ((Get-IpcMarkerCount $state) -eq 3) 'partial line counted twice'
            [IO.File]::WriteAllText($log, "gmm-ipc-ready`n")
            Assert-That ((Get-IpcMarkerCount $state) -eq 1) 'truncation not reset'
            Move-Item $log (Join-Path $logDir 'rotated.log')
            [IO.File]::WriteAllText($log, "gmm-ipc-ready`n")
            Assert-That ((Get-IpcMarkerCount $state) -eq 2) 'rolling files not counted'
            Remove-Item (Join-Path $logDir 'rotated.log')
            Assert-That ((Get-IpcMarkerCount $state) -eq 1) 'deleted file still counted'
        }
        $exe = Join-Path $AppData 'GMM.exe'
        $dll = Join-Path $AppData 'WebView2Loader.dll'
        $db = Join-Path $AppData 'gmm.db'
        foreach ($path in @($exe, $dll, $db)) { [IO.File]::WriteAllText($path, 'fixture') }
        $lock = Join-Path $AppData 'instance.lock'
        [IO.File]::WriteAllText($lock, 'stale files are not evidence')
        Check "$scriptName/LivenessImmediate" {
            Reset-Clock
            $script:Proc.HasExited = $false
            $script:BusyPaths[$lock] = $true
            Wait-AppLiveInstanceLock $script:Proc $lock -deadline $script:Now.AddSeconds(30) -FileReleased $script:Probe
            Assert-That ($script:Sleeps -eq 0) 'healthy liveness added a fixed delay'
        }
        Check "$scriptName/LivenessWaitsForHeldLock" {
            Reset-Clock
            $script:Proc.HasExited = $false
            $script:OnSleep = { if ($script:Sleeps -eq 2) { $script:BusyPaths[$lock] = $true } }
            Wait-AppLiveInstanceLock $script:Proc $lock -deadline $script:Now.AddSeconds(30) -FileReleased $script:Probe
            Assert-That ($script:Sleeps -eq 2) 'must observe the held lock'
        }
        Check "$scriptName/LivenessExitedBeforeCheck" {
            Reset-Clock
            $script:BusyPaths[$lock] = $true
            Assert-Failure {
                Wait-AppLiveInstanceLock $script:Proc $lock -deadline $script:Now.AddSeconds(30) -FileReleased $script:Probe
            } '^GMM exited with code 17 before post-startup liveness:'
            Assert-That ($script:Sleeps -eq 0) 'dead process must fail immediately'
        }
        Check "$scriptName/LivenessExitDuringProbe" {
            Reset-Clock
            $script:Proc.HasExited = $false
            Assert-Failure {
                Wait-AppLiveInstanceLock $script:Proc $lock -deadline $script:Now.AddSeconds(30) -FileReleased {
                    param($path)
                    $script:Proc.HasExited = $true
                    return $false
                }
            } '^GMM exited with code 17 during post-startup liveness:'
        }
        foreach ($signal in @('released', 'missing')) {
            Check "$scriptName/LivenessExpiry-$signal" {
                Reset-Clock
                $script:Proc.HasExited = $false
                $lockPath = if ($signal -eq 'missing') { Join-Path $AppData 'missing.lock' } else { $lock }
                Assert-Failure {
                    Wait-AppLiveInstanceLock $script:Proc $lockPath -deadline $script:Now.AddSeconds(30) -FileReleased $script:Probe
                } (('^timed out waiting for post-startup liveness: running GMM must hold ') + [regex]::Escape($lockPath))
                Assert-That ($script:Sleeps -eq 3) 'liveness ceiling not enforced'
            }
        }
        # Run the shipped smoke completion statements, including the handle
        # saved before Stop-StartupAttempt clears AppProc. Inject only the
        # portable file-release predicate, as for the other consumers below.
        $smokeFn = $smokeAst.Find({ param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Invoke-StartupSmoke' }, $true)
        $completion = @($smokeFn.Body.EndBlock.Statements | Select-Object -Last 3)
        $completionText = ($completion | ForEach-Object { $_.Extent.Text }) -join [Environment]::NewLine
        Check 'installer-smoke/MeasuredCompletionOnly' {
            Assert-That ($completion[0].Extent.Text -eq 'Wait-AppLiveInstanceLock $script:AppProc (Join-Path $AppData "instance.lock") -deadline $refreshDeadline') 'smoke must assert live lock after refresh'
            Assert-That ($completionText.Contains('Wait-AppResourcesReleased $appToStop $exe -database $dbPath')) 'smoke must wait for its database'
            $sleeps = @($smokeFn.Body.FindAll({ param($node) $node -is [Management.Automation.Language.CommandAst] -and $node.GetCommandName() -eq 'Start-Sleep' }, $true))
            foreach ($sleep in $sleeps) {
                Assert-That ($sleep.Extent.Text -eq 'Start-Sleep -Milliseconds 500') 'startup may only sleep inside observable polls'
            }
        }
        foreach ($heldDatabase in @($false, $true)) {
            $caseName = if ($heldDatabase) { 'CompletionDatabaseHeld' } else { 'CompletionImmediate' }
            Check "installer-smoke/$caseName" {
                Reset-Clock
                $script:Proc.HasExited = $false
                $script:AppProc = $script:Proc
                $script:BusyPaths[$lock] = $true
                $dbPath = $db
                $refreshDeadline = $script:Now.AddSeconds(30)
                if ($heldDatabase) { $script:BusyPaths[$db] = $true }
                $operation = [scriptblock]::Create('param($dbPath, $refreshDeadline)' + [Environment]::NewLine + $completionText.Replace('-deadline $refreshDeadline', '-deadline $refreshDeadline -FileReleased $script:Probe').Replace('-database $dbPath', '-database $dbPath -FileReleased $script:Probe'))
                if ($heldDatabase) {
                    Assert-Failure { & $operation $dbPath $refreshDeadline } (('timed out waiting for GMM teardown:.*still held: ') + [regex]::Escape($db))
                    Assert-That ($script:Sleeps -eq 3) 'database hold must expire'
                } else {
                    & $operation $dbPath $refreshDeadline
                    Assert-That ($script:Sleeps -eq 0) 'healthy completion added a fixed delay'
                }
                Assert-That ($script:Stopped -and $script:Proc.Disposed -and $null -eq $script:AppProc) 'smoke must stop, wait and dispose the saved handle'
                Assert-That ($db -in $script:ProbedPaths) 'smoke did not probe its database'
            }
        }
        Check "$scriptName/TeardownImmediate" {
            Reset-Clock
            Wait-AppResourcesReleased $script:Proc $exe -database $db -FileReleased $script:Probe
            Assert-That ($script:Sleeps -eq 0) 'healthy teardown added a fixed delay'
        }
        foreach ($resource in @('exe', 'dll', 'database', 'process')) {
            $lockedPath = switch ($resource) { exe { $exe }; dll { $dll }; database { $db }; process { $null } }
            Check "$scriptName/TeardownWaitsForProbe-$resource" {
                Reset-Clock
                    if ($lockedPath) { $script:BusyPaths[$lockedPath] = $true }
                else { $script:Proc.HasExited = $false }
                try {
                    $script:OnSleep = { if ($script:Sleeps -eq 2) { $script:BusyPaths.Clear(); $script:Proc.HasExited = $true } }
                    Wait-AppResourcesReleased $script:Proc $exe -database $db -FileReleased $script:Probe
                    Assert-That ($script:Sleeps -eq 2) 'did not poll until actual resource release'
                } finally { $script:BusyPaths.Clear() }
            }
            Check "$scriptName/TeardownProbeExpiry-$resource" {
                Reset-Clock
                    if ($lockedPath) { $script:BusyPaths[$lockedPath] = $true }
                else { $script:Proc.HasExited = $false }
                try {
                    Assert-Failure { Wait-AppResourcesReleased $script:Proc $exe -database $db -FileReleased $script:Probe } 'timed out waiting for GMM teardown.*still held:'
                    Assert-That ($script:Sleeps -eq 3) 'teardown ceiling not enforced'
                } finally { $script:BusyPaths.Clear() }
            }
        }
        Check "$scriptName/TeardownProbeFailureIsNotExpiry" {
            Reset-Clock
            Assert-Failure {
                Wait-AppResourcesReleased $script:Proc $exe -FileReleased { param($path) throw "probe failure: $path" }
            } '^probe failure:'
            Assert-That ($script:Sleeps -eq 0) 'unexpected probe error retried as expiry'
        }
        Check "$scriptName/StaleMarkerDoesNotPassExitedLaunch" {
            Reset-Clock
            Assert-Failure { Assert-AppStarts $exe -FileReleased $script:Probe } 'GMM exited with code 17 before reaching a working state'
            Assert-That ($script:Stopped -and $script:Proc.Disposed) 'cleanup did not complete'
        }
        Check "$scriptName/StartupExpiryPreserved" {
            Reset-Clock
            $script:Proc.HasExited = $false
            # Parent exits only once cleanup calls Stop-Process.
            $script:OnSleep = { if ($script:Stopped) { $script:Proc.HasExited = $true } }
            Assert-Failure { Assert-AppStarts $exe -FileReleased $script:Probe } '^timed out waiting for GMM to reach a working state$'
            Assert-That ($script:Stopped -and $script:Proc.Disposed) 'expiry skipped cleanup'
        }
        Check "$scriptName/StartupAndCleanupFailurePreserved" {
            Reset-Clock
            $script:BusyPaths[$exe] = $true
            try {
                Assert-Failure { Assert-AppStarts $exe -FileReleased $script:Probe } 'GMM exited with code 17 before reaching a working state'
                Assert-That ($script:Warnings.Count -eq 1 -and $script:Warnings[0] -match 'timed out waiting for GMM teardown') 'secondary failure not visible'
                Assert-That $script:Proc.Disposed 'secondary failure skipped dispose'
            } finally { $script:BusyPaths.Clear() }
        }
        Check "$scriptName/StartupNewMarkerAndCleanup" {
            Reset-Clock
            $script:Proc.HasExited = $false
            $script:OnSleep = {
                if ($script:Stopped) { $script:Proc.HasExited = $true }
                else { [IO.File]::AppendAllText($log, "gmm-ipc-ready`n") }
            }
            Assert-AppStarts $exe -FileReleased $script:Probe
            Assert-That ($script:Stopped -and $script:Proc.Disposed -and $script:Sleeps -eq 2) 'stale marker passed or cleanup not polled'
        }
        Check "$scriptName/StartupCleanupExpiryFails" {
            Reset-Clock
            $script:Proc.HasExited = $false
            $script:OnSleep = { [IO.File]::AppendAllText($log, "gmm-ipc-ready`n") }
            Assert-Failure { Assert-AppStarts $exe -FileReleased $script:Probe } 'timed out waiting for GMM teardown'
            Assert-That $script:Proc.Disposed 'cleanup expiry skipped dispose'
        }
        foreach ($consumer in $consumerCalls.Keys) {
            Check "$consumer/DatabaseTeardownArgument" {
                Assert-That ($consumerCalls[$consumer].Count -gt 0) 'missing consumer launch calls'
                foreach ($call in $consumerCalls[$consumer]) {
                    Reset-Clock
                    $script:Proc.HasExited = $false
                    $script:BusyPaths[$db] = $true
                    $script:OnSleep = {
                        if ($script:Stopped) { $script:Proc.HasExited = $true }
                        else { [IO.File]::AppendAllText($log, "gmm-ipc-ready`n") }
                        if ($script:Sleeps -eq 3) { $script:BusyPaths.Clear() }
                    }
                    # Execute each shipped call's actual arguments against the
                    # shared helper, injecting only the portable release probe.
                    & ([scriptblock]::Create($call.Extent.Text + ' -FileReleased $script:Probe'))
                    $expectsDatabase = $consumer -eq 'installer-lifecycle'
                    Assert-That (($db -in $script:ProbedPaths) -eq $expectsDatabase) "$consumer database probe must match its teardown policy"
                    $expectedSleeps = if ($expectsDatabase) { 3 } else { 2 }
                    Assert-That ($script:Sleeps -eq $expectedSleeps) "$consumer must wait only for its own resources"
                    Assert-That ($script:Stopped -and $script:Proc.Disposed) "$consumer cleanup did not complete"
                }
            }
        }
        Assert-That ($script:Checks -gt 0) "no tests matched '$Case'"
        Write-Host "$scriptName TOTAL $script:Checks passed"
    } finally { Remove-Item -LiteralPath $root -Force -Recurse }
    Export-ModuleMember -Function @()
}
$module = New-Module -Name 'app-starts-contract' -ScriptBlock $contract -ArgumentList 'app-starts', $RepoRoot, $Case
Remove-Module $module -ErrorAction SilentlyContinue
Write-Host "NOT-RUN: Windows mapped-image sharing, WebView2 lifetime and MSI behavior require the native installer/updater CI jobs."
