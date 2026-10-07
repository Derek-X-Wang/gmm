# Native console interruption and Loader/watcher regressions require Windows.
# Keep both mutation evidence and restored green tests in the hosted build log.
$ErrorActionPreference = 'Stop'
$repo = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$cliPath = Join-Path $repo 'src-tauri/crates/cli/src/lib.rs'
$launchPath = Join-Path $repo 'src-tauri/src/runtime/launch.rs'
$cliSource = [IO.File]::ReadAllText($cliPath)
$launchSource = [IO.File]::ReadAllText($launchPath)

function Assert-MutationRejected {
    param([string]$TestName, [string]$Assertion)
    $log = & cargo test -p gmm-cli --test commands $TestName -- --nocapture 2>&1
    $code = $LASTEXITCODE
    $text = $log -join "`n"
    Write-Output $text
    if ($code -eq 0 -or -not $text.Contains($Assertion) -or -not $text.Contains("running 1 test")) {
        throw "Mutation did not fail the named assertion: $TestName / $Assertion"
    }
    Write-Output "MUTATION PROVEN: $TestName => $Assertion"
}

Push-Location (Join-Path $repo 'src-tauri')
try {
    & cargo test -p gmm-cli --test commands
    if ($LASTEXITCODE -ne 0) { throw 'CLI session suite failed before mutations' }

    $mutated = $cliSource.Replace('core.clean_stale_session().await?;', '// mutation: omit dead-session cleanup')
    if ($mutated -eq $cliSource) { throw 'Dead-session mutation target missing' }
    [IO.File]::WriteAllText($cliPath, $mutated)
    & cargo build -p gmm-cli
    if ($LASTEXITCODE -ne 0) { throw 'Mutated CLI build failed' }
    Assert-MutationRejected 'windows::ctrl_c_interrupt_leaves_cli_able_to_launch_enable_and_disable' 'after native Ctrl+C, CLI must enable Mods'
    [IO.File]::WriteAllText($cliPath, $cliSource)

    $mutated = $launchSource.Replace('if let Err(e) = core.end_session().await {', 'if let Err(e) = core.session_info().await.map(|_| ()) {')
    if ($mutated -eq $launchSource) { throw 'Watcher mutation target missing' }
    [IO.File]::WriteAllText($launchPath, $mutated)
    Assert-MutationRejected 'windows::headless_success_injects_and_watcher_releases_session' 'headless watcher must clear the persisted session'
} finally {
    [IO.File]::WriteAllText($cliPath, $cliSource)
    [IO.File]::WriteAllText($launchPath, $launchSource)
    Pop-Location
}

Push-Location (Join-Path $repo 'src-tauri')
try {
    & cargo build --workspace
    if ($LASTEXITCODE -ne 0) { throw 'Restored workspace build failed' }
    & cargo test -p gmm-cli --test commands
    if ($LASTEXITCODE -ne 0) { throw 'Restored CLI session suite failed' }
} finally {
    Pop-Location
}
