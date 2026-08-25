$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Get-UpdateServerOutput {
    [CmdletBinding()]
    param(
        [string]$StandardOutputPath,
        [string]$StandardErrorPath
    )

    $output = @()
    foreach ($path in @($StandardOutputPath, $StandardErrorPath)) {
        if ($path -and (Test-Path $path)) {
            $text = Get-Content -Path $path -Raw -ErrorAction SilentlyContinue
            if ($null -ne $text -and $text.Trim()) {
                $output += $text.Trim()
            }
        }
    }

    if ($output.Count -eq 0) {
        return "<no server output>"
    }
    return $output -join [Environment]::NewLine
}

function Wait-ForUpdateServerBind {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [System.Diagnostics.Process]$ServerProcess,

        [Parameter(Mandatory)]
        [string]$StandardOutputPath,

        [string]$StandardErrorPath,

        [ValidateRange(1, 300)]
        [int]$TimeoutSeconds = 13
    )

    $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()

    while ($stopwatch.Elapsed.TotalSeconds -lt $TimeoutSeconds) {
        $serverOutput = Get-UpdateServerOutput $StandardOutputPath $StandardErrorPath
        $bindFailure = [Regex]::Match(
            $serverOutput,
            "(?m)^local update server bind failed: .+$"
        )
        if ($bindFailure.Success) {
            $elapsed = [Math]::Round($stopwatch.Elapsed.TotalSeconds, 1)
            throw "$($bindFailure.Value) after ${elapsed}s"
        }

        $bindSuccess = [Regex]::Match(
            $serverOutput,
            "(?m)^local update server bound to 127\.0\.0\.1:(?<Port>\d+)\r?$"
        )
        if ($bindSuccess.Success) {
            $port = [int]$bindSuccess.Groups["Port"].Value
            $elapsed = [Math]::Round($stopwatch.Elapsed.TotalSeconds, 1)
            Write-Host "$($bindSuccess.Value) after ${elapsed}s"
            return $port
        }

        $ServerProcess.Refresh()
        if ($ServerProcess.HasExited) {
            $serverOutput = Get-UpdateServerOutput $StandardOutputPath $StandardErrorPath
            $elapsed = [Math]::Round($stopwatch.Elapsed.TotalSeconds, 1)
            throw "local update server process exited before reporting a successful " +
                  "bind with code $($ServerProcess.ExitCode) after ${elapsed}s. " +
                  "Server output: $serverOutput"
        }

        Start-Sleep -Milliseconds 50
    }

    $ServerProcess.Refresh()
    $serverOutput = Get-UpdateServerOutput $StandardOutputPath $StandardErrorPath
    $elapsed = [Math]::Round($stopwatch.Elapsed.TotalSeconds, 1)
    if ($ServerProcess.HasExited) {
        throw "local update server process exited before reporting a successful " +
              "bind with code $($ServerProcess.ExitCode) after ${elapsed}s. " +
              "Server output: $serverOutput"
    }
    throw "timed out after ${elapsed}s waiting for local update server to report " +
          "a successful bind. Server output: $serverOutput"
}

function Wait-ForUpdateServer {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [ValidateRange(1, 65535)]
        [int]$Port,

        [ValidateRange(1, 300)]
        [int]$TimeoutSeconds = 15,

        [System.Diagnostics.Process]$ServerProcess,
        [string]$StandardOutputPath,
        [string]$StandardErrorPath
    )

    $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()

    while ($stopwatch.Elapsed.TotalSeconds -lt $TimeoutSeconds) {
        if ($null -ne $ServerProcess) {
            $ServerProcess.Refresh()
            if ($ServerProcess.HasExited) {
                $serverOutput = Get-UpdateServerOutput $StandardOutputPath $StandardErrorPath
                $elapsed = [Math]::Round($stopwatch.Elapsed.TotalSeconds, 1)
                throw "local update server process exited during startup with code " +
                      "$($ServerProcess.ExitCode) while waiting for port $Port after " +
                      "${elapsed}s. Server output: $serverOutput"
            }
        }

        $remainingMilliseconds = [Math]::Ceiling(
            ($TimeoutSeconds - $stopwatch.Elapsed.TotalSeconds) * 1000
        )
        $attemptMilliseconds = [Math]::Max(1, [Math]::Min(250, $remainingMilliseconds))
        $client = [System.Net.Sockets.TcpClient]::new()
        try {
            $connection = $client.ConnectAsync([System.Net.IPAddress]::Loopback, $Port)
            try {
                if ($connection.Wait($attemptMilliseconds) -and $client.Connected) {
                    $elapsed = [Math]::Round($stopwatch.Elapsed.TotalSeconds, 1)
                    Write-Host "local update server accepted a connection on port $Port after ${elapsed}s"
                    return
                }
            } catch [System.AggregateException] {
                Write-Debug "listener probe failed: $($_.Exception.GetBaseException().Message)"
            }
        } finally {
            $client.Dispose()
        }

        Start-Sleep -Milliseconds 100
    }

    if ($null -ne $ServerProcess) {
        $ServerProcess.Refresh()
        if ($ServerProcess.HasExited) {
            $serverOutput = Get-UpdateServerOutput $StandardOutputPath $StandardErrorPath
            $elapsed = [Math]::Round($stopwatch.Elapsed.TotalSeconds, 1)
            throw "local update server process exited during startup with code " +
                  "$($ServerProcess.ExitCode) while waiting for port $Port after " +
                  "${elapsed}s. Server output: $serverOutput"
        }
    }

    $serverOutput = Get-UpdateServerOutput $StandardOutputPath $StandardErrorPath
    $elapsed = [Math]::Round($stopwatch.Elapsed.TotalSeconds, 1)
    throw "timed out after ${elapsed}s waiting for local update server to accept " +
          "connections on port $Port. Server output: $serverOutput"
}
