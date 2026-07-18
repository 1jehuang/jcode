param(
    [string[]]$Paths = @("scripts")
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$scriptFiles = @()
foreach ($path in $Paths) {
    if (-not (Test-Path -LiteralPath $path)) {
        continue
    }

    $scriptFiles += Get-ChildItem -LiteralPath $path -Recurse -File -Filter '*.ps1'
}

if (-not $scriptFiles -or $scriptFiles.Count -eq 0) {
    Write-Host 'No PowerShell scripts found.' -ForegroundColor Yellow
    exit 0
}

$hadErrors = $false

foreach ($file in $scriptFiles | Sort-Object FullName -Unique) {
    # Reject a UTF-8 BOM. `irm <url> | iex` on Windows PowerShell 5.1 decodes the
    # body without honoring the BOM, so the stray bytes corrupt the leading `<#`
    # help comment and the whole script fails to parse (issue #503).
    $prefix = [System.IO.File]::ReadAllBytes($file.FullName) | Select-Object -First 3
    if ($prefix.Count -ge 3 -and $prefix[0] -eq 0xEF -and $prefix[1] -eq 0xBB -and $prefix[2] -eq 0xBF) {
        $hadErrors = $true
        Write-Host "UTF-8 BOM found in $($file.FullName); save without BOM (breaks 'irm | iex')" -ForegroundColor Red
        continue
    }

    $tokens = $null
    $errors = $null
    [System.Management.Automation.Language.Parser]::ParseFile($file.FullName, [ref]$tokens, [ref]$errors) | Out-Null

    if ($errors -and $errors.Count -gt 0) {
        $hadErrors = $true
        Write-Host "Parse errors in $($file.FullName):" -ForegroundColor Red
        foreach ($error in $errors) {
            $line = $error.Extent.StartLineNumber
            $column = $error.Extent.StartColumnNumber
            Write-Host "  Line ${line}, Col ${column}: $($error.Message)" -ForegroundColor Red
        }
    } else {
        Write-Host "OK: $($file.FullName)" -ForegroundColor Green
    }
}

if ($hadErrors) {
    exit 1
}
