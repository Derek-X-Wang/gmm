$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$Linter = Join-Path $PSScriptRoot "lint-ci-scripts.ps1"
$FixtureDirectory = Join-Path ([System.IO.Path]::GetTempPath()) "gmm-ci-lint-$([guid]::NewGuid())"
$Fixture = Join-Path $FixtureDirectory "known-violation.ps1"

try {
    New-Item -ItemType Directory -Path $FixtureDirectory | Out-Null
    @'
Invoke-Expression 'Write-Host fixture'
'@ | Set-Content -Path $Fixture -NoNewline

    $Output = @(& (Join-Path $PSHOME "pwsh") -NoProfile -File $Linter -Path $FixtureDirectory 2>&1)
    $ExitCode = $LASTEXITCODE

    if ($ExitCode -eq 0) {
        throw "lint-ci-scripts.ps1 accepted a known PSScriptAnalyzer violation"
    }
    if ($Output -notmatch 'PSAvoidUsingInvokeExpression') {
        throw "lint-ci-scripts.ps1 failed without reporting the expected PSAvoidUsingInvokeExpression rule: $($Output -join [Environment]::NewLine)"
    }

    Write-Host "PowerShell analyzer self-test rejected the known violation"
}
finally {
    Remove-Item -Path $FixtureDirectory -Recurse -Force -ErrorAction SilentlyContinue
}
