//! PowerShell templates.

/// Precmd hook function.
pub const PWSH_PRECMD: &str = r#"function Invoke-JcodePreCmd {
    $exitCode = $global:LASTEXITCODE
    Write-Host -NoNewline "`e]133;D;${exitCode}`a"
    Write-Host -NoNewline "`e]133;A`a"
}
"#;

/// Preexec hook function.
pub const PWSH_PREEXEC: &str = r#"function Invoke-JcodePreExec {
    Write-Host -NoNewline "`e]133;B`a"
}
"#;

/// Full init script for PowerShell.
pub const PWSH_INIT: &str = r#"# jcode shell integration for PowerShell
# Auto-generated — do not edit manually.

function Invoke-JcodePreCmd {
    $exitCode = $global:LASTEXITCODE
    Write-Host -NoNewline "`e]133;D;${exitCode}`a"
    Write-Host -NoNewline "`e]133;A`a"
}

function Invoke-JcodePreExec {
    Write-Host -NoNewline "`e]133;B`a"
}

# Register hooks by wrapping the existing Prompt function
if (-not (Test-Path Variable:\JcodeOriginalPrompt)) {
    $global:JcodeOriginalPrompt = $function:Prompt
    $global:Prompt = {
        Invoke-JcodePreCmd
        & $global:JcodeOriginalPrompt
    }
}

# Report exit code on shell exit
Register-EngineEvent -SourceIdentifier PowerShell.Exiting -Action {
    Write-Host -NoNewline "`e]133;D;0`a"
}
"#;

/// Completion header.
pub const PWSH_COMPLETION_HEADER: &str = "# jcode PowerShell completions\n# Auto-generated — do not edit manually.\n\n";
