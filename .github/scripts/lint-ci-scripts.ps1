param(
    [string]$Path = $PSScriptRoot
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$Settings = Join-Path (Split-Path -Parent $PSScriptRoot) "PSScriptAnalyzerSettings.psd1"
$Files = @(Get-ChildItem -Path $Path -Filter *.ps1 -File -Recurse)
if ($Files.Count -eq 0) {
    throw "no PowerShell scripts found under '$Path'"
}

$Failed = $false

function Write-LintError($File, $Line, $Message) {
    $script:Failed = $true
    [Console]::Error.WriteLine("${File}:${Line}: $Message")
}

foreach ($File in $Files) {
    $Tokens = $null
    $ParseErrors = $null
    $Ast = [System.Management.Automation.Language.Parser]::ParseFile(
        $File.FullName,
        [ref]$Tokens,
        [ref]$ParseErrors
    )

    foreach ($ParseError in $ParseErrors) {
        Write-LintError $File.FullName $ParseError.Extent.StartLineNumber "PowerShell parse error: $($ParseError.Message)"
    }

    # A backtick followed by `t`, `n`, and similar letters inside an
    # expandable string silently becomes a control character. These scripts'
    # diagnostics are the only evidence available to a Windows-less
    # maintainer, so accidental control characters are correctness failures.
    # An intentional one must carry `ci-lint: allow-control-character-string`
    # on the same or immediately preceding line, with its reason at the site.
    # Multi-line strings are deliberately excluded: their intended line breaks
    # are control characters too, and the AST value does not distinguish those
    # source newlines from backtick escapes. Parsing and PSScriptAnalyzer still
    # cover them, but this additional diagnostic check does not.
    $Lines = @(Get-Content -Path $File.FullName)
    $ExpandableStrings = $Ast.FindAll({
        param($Node)
        $Node -is [System.Management.Automation.Language.ExpandableStringExpressionAst] -and
            $Node.Extent.StartLineNumber -eq $Node.Extent.EndLineNumber -and
            $Node.Value -match '[\x00-\x1f]'
    }, $true)

    foreach ($String in $ExpandableStrings) {
        $LineNumber = $String.Extent.StartLineNumber
        $Line = $Lines[$LineNumber - 1]
        $PreviousLine = if ($LineNumber -gt 1) { $Lines[$LineNumber - 2] } else { "" }
        if ($Line -notmatch 'ci-lint: allow-control-character-string' -and
            $PreviousLine -notmatch 'ci-lint: allow-control-character-string') {
            Write-LintError $File.FullName $LineNumber "control character in an expandable string; escape a literal backtick twice or justify an intentional control character inline"
        }
    }

    $Findings = @(Invoke-ScriptAnalyzer -Path $File.FullName -Settings $Settings)
    foreach ($Finding in $Findings) {
        Write-LintError $File.FullName $Finding.Line "$($Finding.RuleName) [$($Finding.Severity)]: $($Finding.Message)"
    }
}

if ($Failed) {
    exit 1
}

Write-Host "PowerShell parse and analysis passed for $($Files.Count) scripts"
