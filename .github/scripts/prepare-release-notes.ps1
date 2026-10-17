param(
    [Parameter(Mandatory)]
    [string]$Tag,
    [Parameter(Mandatory)]
    [string]$OutputPath
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
# Inspect Git exit codes ourselves so a failed recovery gets our diagnostic.
$PSNativeCommandUseErrorActionPreference = $false

git check-ref-format "refs/tags/$Tag"
if ($LASTEXITCODE -ne 0) {
    throw "Invalid release tag: $Tag"
}

$objectType = git cat-file -t "refs/tags/$Tag" 2>$null
if ($LASTEXITCODE -ne 0) {
    throw "Release tag ref 'refs/tags/$Tag' does not exist"
}
if ($objectType -ne "tag") {
    # Checkout can overwrite an annotated tag ref with the commit it targets.
    git fetch --force --no-recurse-submodules origin "refs/tags/${Tag}:refs/tags/$Tag" 2>$null | Out-Null
    $objectType = git cat-file -t "refs/tags/$Tag" 2>$null
    if ($LASTEXITCODE -ne 0 -or $objectType -ne "tag") {
        throw "Release tag ref 'refs/tags/$Tag' resolved to '$objectType'; recovery from origin did not produce an annotated tag object"
    }
}

$annotationLines = @(git tag -l --format='%(contents)' -- $Tag)
if ($LASTEXITCODE -ne 0) {
    throw "Could not read the annotation for release tag '$Tag'"
}
$annotation = ($annotationLines -join "`n").Trim()
if ([string]::IsNullOrWhiteSpace($annotation)) {
    throw "Release tag '$Tag' has an empty annotation"
}

$lines = @($annotation -split '\r?\n')
# Existing release annotations start with "GMM v<version>" on its own line.
# Accept a bare tag title too, but never discard an ordinary summary line.
if ($lines[0].Trim() -in @("GMM $Tag", $Tag)) {
    $lines = @($lines | Select-Object -Skip 1)
}
$paragraph = [System.Collections.Generic.List[string]]::new()
foreach ($line in $lines) {
    if ([string]::IsNullOrWhiteSpace($line)) {
        if ($paragraph.Count -gt 0) { break }
        continue
    }
    $paragraph.Add($line.Trim())
}
$summary = $paragraph -join " "
if ([string]::IsNullOrWhiteSpace($summary)) {
    throw "Release tag '$Tag' has no summary paragraph after its title; updater notes must not be empty"
}

# A random delimiter keeps annotation text from terminating a multiline output.
$delimiter = "gmm_release_$([guid]::NewGuid().ToString('N'))"
$output = @(
    "summary<<$delimiter"
    $summary
    $delimiter
    "annotation<<$delimiter"
    $annotation
    $delimiter
) -join "`n"
[System.IO.File]::AppendAllText($OutputPath, $output + [char]10, [System.Text.UTF8Encoding]::new($false))
