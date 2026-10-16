$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$prepare = Join-Path $PSScriptRoot "prepare-release-notes.ps1"
$workflow = Get-Content (Join-Path $PSScriptRoot "../workflows/release.yml") -Raw
$temp = Join-Path ([System.IO.Path]::GetTempPath()) "gmm-release-notes-$([guid]::NewGuid().ToString('N'))"
$utf8 = [System.Text.UTF8Encoding]::new($false)
$script:Checks = 0

function Assert-Equal($Name, $Actual, $Expected) {
    if ($Actual -cne $Expected) {
        throw "${Name}: expected '$Expected', got '$Actual'"
    }
    $script:Checks++
    Write-Host "PASS $Name"
}

function Add-TestTag($Tag, $Annotation) {
    [System.IO.File]::WriteAllText((Join-Path $temp "annotation.txt"), $Annotation, $utf8)
    git -c user.name=release-test -c user.email=release-test@example.invalid -c tag.gpgsign=false tag -a $Tag --cleanup=verbatim -F annotation.txt
    if ($LASTEXITCODE -ne 0) { throw "Could not create fixture tag '$Tag'" }
}

function Read-Notes($Tag) {
    $outputPath = Join-Path $temp "outputs-$Tag.txt"
    & $prepare -Tag $Tag -OutputPath $outputPath
    $output = [System.IO.File]::ReadAllText($outputPath)
    $fields = @{}
    foreach ($match in [regex]::Matches($output, '(?ms)^(summary|annotation)<<([^\n]+)\n(.*?)\n\2\n')) {
        $fields[$match.Groups[1].Value] = $match.Groups[3].Value
    }
    if ($fields.Count -ne 2) { throw "Missing release note outputs for '$Tag'" }
    return $fields
}

function Assert-Rejected($Name, $Tag, $Diagnostic) {
    $outputPath = Join-Path $temp "rejected-$Tag.txt"
    $failure = $null
    try { & $prepare -Tag $Tag -OutputPath $outputPath } catch { $failure = $_.Exception.Message }
    if ($null -eq $failure -or $failure -notlike "*$Diagnostic*") {
        throw "${Name}: expected '$Diagnostic', got '$failure'"
    }
    if (Test-Path -LiteralPath $outputPath) { throw "${Name}: failure wrote updater notes" }
    $script:Checks++
    Write-Host "PASS ${Name}: $failure"
}

# Execute the shipped digest step, substituting only its network boundary.
$digestMatch = [regex]::Match($workflow, '(?s)      - name: Add SHA-256 digests to the release body.*?        run: \|\r?\n(.*)\z')
if (-not $digestMatch.Success) { throw "Could not find the release digest step" }
$digestCode = ($digestMatch.Groups[1].Value -split '\r?\n' | ForEach-Object { $_ -replace '^          ', '' }) -join "`n"
$digestStep = [scriptblock]::Create($digestCode)
$script:ExistingBody = ""
$script:PatchedBody = $null
function Invoke-RestMethod {
    [Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidOverwritingBuiltInCmdlets', '', Justification = 'Intercept the shipped digest step HTTP boundary; no network calls are allowed in this test.')]
    param($Uri, $Headers, $Method, $ContentType, $Body)
    if ($Method -eq "Patch") {
        $script:PatchedBody = ($Body | ConvertFrom-Json).body
    } else {
        return @{ body = $script:ExistingBody }
    }
}

$envNames = @("ARTIFACT_PATHS", "GITHUB_TOKEN", "RELEASE_ID", "RELEASE_SUMMARY", "RELEASE_ANNOTATION", "GITHUB_API_URL", "GITHUB_REPOSITORY")
$savedEnv = @{}
foreach ($name in $envNames) { $savedEnv[$name] = [Environment]::GetEnvironmentVariable($name) }

New-Item -ItemType Directory -Path $temp | Out-Null
Push-Location $temp
try {
    git init --quiet
    git -c user.name=release-test -c user.email=release-test@example.invalid -c commit.gpgsign=false commit --quiet --allow-empty -m fixture
    if ($LASTEXITCODE -ne 0) { throw "Could not create fixture repository" }

    $annotation = "GMM v1.0.0`n`nA useful release`nwith wrapped lines.`n `nDetails stay on GitHub."
    Add-TestTag "v1.0.0" $annotation
    $notes = Read-Notes "v1.0.0"
    Assert-Equal "non-empty updater notes use only the first prose paragraph" $notes.summary "A useful release with wrapped lines."
    Assert-Equal "full annotation retained for the published body" $notes.annotation $annotation

    Add-TestTag "v1.0.1" "No title is required.`nSecond summary line.`n`nOther details."
    Assert-Equal "untitled summary keeps its first line" (Read-Notes "v1.0.1").summary "No title is required. Second summary line."
    Add-TestTag "v1.0.2" "v1.0.2`n`nA bare tag title.`n`nDetails."
    Assert-Equal "bare tag title is removed" (Read-Notes "v1.0.2").summary "A bare tag title."
    Add-TestTag "v1.0.3" "GMM v1.0.3`r`n`r`nUnicode: 界 and `$1.`r`nContinued.`r`n`r`nDetails."
    Assert-Equal "CRLF and Unicode summary" (Read-Notes "v1.0.3").summary 'Unicode: 界 and $1. Continued.'

    Add-TestTag "v2.0.0" "GMM v2.0.0"
    Assert-Rejected "title-only annotation fails" "v2.0.0" "no summary paragraph"
    Add-TestTag "v2.0.1" ""
    Assert-Rejected "empty annotation fails" "v2.0.1" "empty annotation"
    git -c tag.gpgsign=false tag v2.0.2
    Assert-Rejected "lightweight tag fails" "v2.0.2" "must exist and have an annotation"
    Assert-Rejected "missing tag fails" "v2.0.3" "must exist and have an annotation"

    Assert-Equal "tauri-action receives summary before manifest creation" ($workflow -match 'releaseBody: \$\{\{ steps\.release-notes\.outputs\.summary \}\}') $true
    Assert-Equal "release workflow runs the guarded extractor" ($workflow -match 'run: \.github/scripts/prepare-release-notes\.ps1 -Tag \$env:RELEASE_TAG -OutputPath \$env:GITHUB_OUTPUT') $true

    $artifact = Join-Path $temp "fixture.msi"
    [System.IO.File]::WriteAllText($artifact, "fixture", $utf8)
    $env:ARTIFACT_PATHS = ConvertTo-Json @($artifact) -Compress
    $env:GITHUB_TOKEN = "fixture"
    $env:RELEASE_ID = "1"
    $env:GITHUB_API_URL = "https://example.invalid"
    $env:GITHUB_REPOSITORY = "fixture/release"
    $env:RELEASE_SUMMARY = $notes.summary
    $env:RELEASE_ANNOTATION = $notes.annotation

    $script:ExistingBody = $notes.summary
    & $digestStep
    $publishedBody = $script:PatchedBody
    Assert-Equal "published body starts with the complete annotation" ($publishedBody.StartsWith($annotation + [char]10 + [char]10 + '<!-- gmm-release-sha256:start -->')) $true
    $hash = (Get-FileHash $artifact -Algorithm SHA256).Hash.ToLowerInvariant()
    Assert-Equal "digest table contains the measured artifact hash" ($publishedBody.Contains($hash)) $true

    $script:ExistingBody = $publishedBody.Replace($hash, "obsolete")
    & $digestStep
    Assert-Equal "rerun replaces the digest section without losing annotation" $script:PatchedBody $publishedBody

    $script:ExistingBody = "Maintainer description.`n`n<!-- gmm-release-sha256:start -->old<!-- gmm-release-sha256:end -->"
    & $digestStep
    Assert-Equal "maintainer body survives marker replacement" $script:PatchedBody ($publishedBody.Replace($annotation, "Maintainer description."))

    $script:ExistingBody = $notes.summary + $publishedBody.Substring($annotation.Length)
    & $digestStep
    Assert-Equal "summary plus existing digests expands on rerun" $script:PatchedBody $publishedBody

    $script:ExistingBody = ""
    & $digestStep
    Assert-Equal "empty draft body recovers the full annotation" $script:PatchedBody $publishedBody

    # Replacement text must be literal: release prose can contain dollar signs.
    $env:RELEASE_ANNOTATION = $annotation + [char]10 + 'Literal $& and $1.'
    $script:ExistingBody = $notes.summary
    & $digestStep
    Assert-Equal "annotation is literal regex replacement text" ($script:PatchedBody.StartsWith($env:RELEASE_ANNOTATION)) $true
} finally {
    Pop-Location
    foreach ($name in $envNames) { [Environment]::SetEnvironmentVariable($name, $savedEnv[$name]) }
    Remove-Item -LiteralPath $temp -Recurse -Force
}

Write-Host "Release notes checks passed: $script:Checks"
