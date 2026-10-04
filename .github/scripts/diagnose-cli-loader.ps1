# Loader evidence and standalone startup verification on Windows. Compare the
# CLI with the probe, then launch only the CLI from an isolated directory.
param(
    [ValidateSet("debug", "release")]
    [string]$BuildProfile = "debug"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$Repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$BinaryDir = Join-Path $Repo "src-tauri/target/$BuildProfile"
$VsWhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio/Installer/vswhere.exe"
$VsRoot = & $VsWhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
$DumpBin = Get-ChildItem -Path "$VsRoot/VC/Tools/MSVC/*/bin/Hostx64/x64/dumpbin.exe" |
    Sort-Object FullName -Descending | Select-Object -First 1
$ManifestTool = Get-ChildItem -Path "${env:ProgramFiles(x86)}/Windows Kits/10/bin/*/x64/mt.exe" |
    Sort-Object FullName -Descending | Select-Object -First 1

foreach ($Name in @("gmm-cli.exe", "concurrency-probe.exe", "gmm_lib.dll")) {
    $Binary = Join-Path $BinaryDir $Name
    if (-not (Test-Path -LiteralPath $Binary)) {
        Write-Host "missing binary: $Binary"
        continue
    }
    $File = Get-Item -LiteralPath $Binary
    Write-Host "executable: $($File.FullName); exists: True; size: $($File.Length)"
    Write-Host "=== imports: $Name ==="
    & $DumpBin.FullName /imports $Binary
    Write-Host "dumpbin exit code: $LASTEXITCODE"

    # EXEs use manifest resource 1; DLLs conventionally use resource 2.
    # Try both, preserving mt's raw error when a resource is absent.
    foreach ($Resource in @(1, 2)) {
        $Manifest = Join-Path $env:RUNNER_TEMP "$Name.$Resource.manifest.xml"
        Write-Host "=== manifest: $Name resource $Resource ==="
        & $ManifestTool.FullName "-inputresource:$Binary;#$Resource" "-out:$Manifest"
        Write-Host "mt exit code: $LASTEXITCODE"
        if (Test-Path -LiteralPath $Manifest) {
            Get-Content -LiteralPath $Manifest
        }
    }
}

Write-Host "=== app-local DLL candidates ==="
Get-ChildItem -Path $BinaryDir, (Join-Path $BinaryDir "deps") -Filter *.dll |
    Select-Object FullName, Length | Format-Table -AutoSize

# No Cargo/probe environment or app-local DLL may make this load check pass.
# Keep only Windows system directories on the child's PATH; native runtime
# prerequisites (including VCRUNTIME140.dll) must resolve from the system.
$IsolatedDir = Join-Path $env:RUNNER_TEMP ("gmm-cli-standalone-" + [guid]::NewGuid())
$null = New-Item -Path $IsolatedDir -ItemType Directory
$Process = [System.Diagnostics.Process]::new()
try {
    $IsolatedCli = Join-Path $IsolatedDir "gmm-cli.exe"
    Copy-Item -LiteralPath (Join-Path $BinaryDir "gmm-cli.exe") -Destination $IsolatedCli
    $Process.StartInfo.FileName = $IsolatedCli
    $Process.StartInfo.WorkingDirectory = $IsolatedDir
    $Process.StartInfo.UseShellExecute = $false
    $Process.StartInfo.CreateNoWindow = $true
    $Process.StartInfo.RedirectStandardOutput = $true
    $Process.StartInfo.RedirectStandardError = $true
    $Process.StartInfo.Environment["PATH"] = [Environment]::SystemDirectory + ";" + $env:SystemRoot
    Write-Host "=== standalone CLI (only executable copied; system-only PATH) ==="
    $null = $Process.Start()
    $StdoutTask = $Process.StandardOutput.ReadToEndAsync()
    $StderrTask = $Process.StandardError.ReadToEndAsync()
    if (-not $Process.WaitForExit(30000)) {
        $Process.Kill($true)
        if (-not $Process.WaitForExit(5000)) {
            throw "standalone CLI could not be reaped after termination"
        }
        throw "standalone CLI did not exit within 30 seconds"
    }
    $Stdout = $StdoutTask.GetAwaiter().GetResult()
    $Stderr = $StderrTask.GetAwaiter().GetResult()
    Write-Host "standalone CLI exit: $($Process.ExitCode); stdout: $Stdout; stderr: $Stderr"
    if ($Process.ExitCode -ne 1) {
        throw "standalone CLI must reach its usage-error exit (1), got $($Process.ExitCode)"
    }
    $Lines = @($Stdout.Trim() -split '\r?\n')
    if ($Lines.Count -ne 1) {
        throw "standalone CLI must print exactly one JSON outcome"
    }
    $Outcome = $Lines[0] | ConvertFrom-Json
    if ($Outcome.ok -ne $false -or $Outcome.error.message -ne "a command is required; see docs/cli.md") {
        throw "standalone CLI did not reach the expected argument parser"
    }
    Write-Host "standalone CLI startup verified without GMM DLLs or Cargo PATH entries"
}
finally {
    $Process.Dispose()
    Remove-Item -LiteralPath $IsolatedDir -Recurse -Force
}
exit 0
