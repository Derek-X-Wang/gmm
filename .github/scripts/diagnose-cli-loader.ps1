# Read-only loader evidence after a Windows Rust-test failure. Compare the
# shipped CLI with the probe that successfully starts in the same directory.
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$Repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$DebugDir = Join-Path $Repo "src-tauri/target/debug"
$VsWhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio/Installer/vswhere.exe"
$VsRoot = & $VsWhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
$DumpBin = Get-ChildItem -Path "$VsRoot/VC/Tools/MSVC/*/bin/Hostx64/x64/dumpbin.exe" |
    Sort-Object FullName -Descending | Select-Object -First 1
$ManifestTool = Get-ChildItem -Path "${env:ProgramFiles(x86)}/Windows Kits/10/bin/*/x64/mt.exe" |
    Sort-Object FullName -Descending | Select-Object -First 1

foreach ($Name in @("gmm-cli.exe", "concurrency-probe.exe", "gmm_lib.dll")) {
    $Binary = Join-Path $DebugDir $Name
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
Get-ChildItem -Path $DebugDir, (Join-Path $DebugDir "deps") -Filter *.dll |
    Select-Object FullName, Length | Format-Table -AutoSize
# Missing manifest resources are diagnostic evidence, not a second CI failure.
exit 0
