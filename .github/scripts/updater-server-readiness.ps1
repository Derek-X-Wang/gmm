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
