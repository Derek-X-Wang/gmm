# Launch the installed app and wait until it proves the frontend reached
# the backend (#54's marker), then release installation files for msiexec
# and the optional database for callers that use fixtures.
# How many times the IPC readiness marker appears across every log file.
#
# Counted rather than merely searched for, because the logs are NOT
# cleared between launches. A plain "does the marker appear anywhere"
# check is satisfied instantly by the *previous* launch's line, so it
# would return before the process under test had done anything — and an
# app that crashed on startup would pass. Requiring the count to go up
# is what makes "the app started" an assertion about this launch.
function Get-IpcMarkerCount([hashtable]$state) {
    $logs = Join-Path $AppData "logs"
    if (-not (Test-Path $logs)) { return 0 }
    $count = 0
    foreach ($file in Get-ChildItem $logs -Filter *.log -File) {
        $key = $file.FullName
        # New paths (including rotated logs) and shrinkage below the saved offset
        # reset the scan. CreationTimeUtc can change on ordinary Unix writes.
        # Known limit: a same-path replacement at least as long as the saved
        # offset is not detected; this reader assumes append-only logs otherwise.
        if (-not $state.ContainsKey($key) -or $file.Length -lt $state[$key].Offset) {
            $state[$key] = @{ Offset = 0L; Tail = ""; Count = 0 }
        }
        $entry = $state[$key]
        # Scan historical bytes once per launch, then only appended bytes. Keep
        # the unfinished line so a marker split across polls still counts once.
        $stream = [IO.File]::Open($key, [IO.FileMode]::Open, [IO.FileAccess]::Read,
            [IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete)
        $reader = $null
        try {
            $stream.Position = $entry.Offset
            $reader = [IO.StreamReader]::new($stream)
            $text = $entry.Tail + $reader.ReadToEnd()
            $entry.Offset = $stream.Position
            $lines = $text -split "\r\n|\n|\r"
            for ($index = 0; $index -lt $lines.Length - 1; $index++) {
                if ($lines[$index].IndexOf($IpcReadyMarker, [StringComparison]::OrdinalIgnoreCase) -ge 0) { $entry.Count++ }
            }
            $entry.Tail = $lines[-1]
        } finally {
            if ($null -ne $reader) { $reader.Dispose() }
            $stream.Dispose()
        }
        $count += $entry.Count
        if ($entry.Tail.IndexOf($IpcReadyMarker, [StringComparison]::OrdinalIgnoreCase) -ge 0) { $count++ }
    }
    # Sum only current files, preserving count-increase semantics on rotation.
    return $count
}

# Platform predicate kept separate so non-Windows tests can inject release
# observations without claiming to exercise Windows mapped-image sharing rules.
function Test-AppFileReleased($path) {
    $handle = $null
    try {
        $handle = [IO.File]::Open($path, [IO.FileMode]::Open,
            [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
        return $true
    } catch [IO.IOException] {
        return $false
    } finally {
        if ($null -ne $handle) { $handle.Dispose() }
    }
}

function Wait-AppResourcesReleased($proc, $exe, [string]$database = "",
    [scriptblock]$FileReleased = ${function:Test-AppFileReleased}) {
    # msiexec needs the installed exe and DLLs released. WebView2 children can
    # outlive GMM and keep installation DLLs mapped, so parent exit alone (or
    # instance.lock release) is insufficient. Probe those consumer files too.
    $paths = @($exe) + @(Get-ChildItem (Split-Path -Parent $exe) -Filter *.dll -File -Recurse |
        Select-Object -ExpandProperty FullName)
    if ($database -and (Test-Path -LiteralPath $database)) { $paths += $database }
    # A failure ceiling, not a delay: return on the first released observation.
    $deadline = (Get-Date).AddSeconds(30)
    do {
        $pending = @()
        if (-not $proc.HasExited) { $pending += "GMM process $($proc.Id)" }
        foreach ($path in $paths) {
            if (-not (& $FileReleased $path)) { $pending += $path }
        }
        if ($pending.Count -eq 0) { return }
        if ((Get-Date) -ge $deadline) {
            throw "timed out waiting for GMM teardown: process exit and exclusive consumer-file access; still held: $($pending -join ', ')"
        }
        Start-Sleep -Milliseconds 100
    } while ($true)
}

function Assert-AppStarts($exe, [scriptblock]$FileReleased = ${function:Test-AppFileReleased},
    [string]$database = "") {
    $markerState = @{}
    $before = Get-IpcMarkerCount $markerState
    $proc = Start-Process $exe -PassThru
    $startupFailure = $null
    try {
        # 30 successful ci.yml runs (2026-10-15..16), log timestamps via gh api:
        # cold n=60 = 30 exact smoke launches + 30 first-launch brackets from
        # the preceding log line (milliseconds of overhead, not exact timings).
        # Seconds: min 3.59, p25 4.20, median 4.92, p75 8.68, p90 11.81,
        # p95 16.38, max 26.47, mean 7.27, sd 4.64; 90s gives 3.4x max headroom.
        # Warm 0.51/1.03s samples reflect 500ms polling and are excluded.
        # A new marker comes from a Tauri command body: WebView boot, IPC,
        # command registration and ACL permission all worked for this launch.
        $deadline = (Get-Date).AddSeconds(90)
        while ((Get-Date) -lt $deadline) {
            if ((Get-IpcMarkerCount $markerState) -gt $before) {
                Write-Host "app reached a working state (new IPC marker seen)"
                return
            }
            if ($proc.HasExited) {
                throw "GMM exited with code $($proc.ExitCode) before reaching a working state"
            }
            Start-Sleep -Milliseconds 500
        }
        throw "timed out waiting for GMM to reach a working state"
    } catch {
        $startupFailure = $_
        throw
    } finally {
        try {
            Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
            Wait-AppResourcesReleased $proc $exe -database $database -FileReleased $FileReleased
        } catch {
            # Preserve the startup exit/expiry diagnostic if teardown also fails.
            if ($null -ne $startupFailure) { Write-Warning "GMM cleanup also failed: $_" }
            else { throw }
        } finally {
            $proc.Dispose()
        }
    }
}
