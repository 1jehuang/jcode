//! Shell-specific hook generation for prompt marking, CWD reporting,
//! and command timing.

use crate::detect::Shell;

/// Feature flags that control which hooks are generated.
#[derive(Debug, Clone)]
pub struct FeatureFlags {
    /// Generate OSC 133 prompt markers (A/B/C/D).
    pub prompt_marking: bool,
    /// Generate OSC 7 CWD reporting.
    pub cwd_reporting: bool,
    /// Generate command execution timing.
    pub command_timing: bool,
    /// Report exit codes in OSC 133;D.
    pub exit_code: bool,
    /// Wrap `ssh` for remote environment preservation.
    pub ssh_wrapping: bool,
    /// Wrap `sudo` for terminfo preservation.
    pub sudo_wrapping: bool,
}

impl Default for FeatureFlags {
    fn default() -> Self {
        Self {
            prompt_marking: true,
            cwd_reporting: true,
            command_timing: true,
            exit_code: true,
            ssh_wrapping: false,
            sudo_wrapping: false,
        }
    }
}

/// Configuration for hook generation.
#[derive(Debug, Clone)]
pub struct HookConfig {
    pub shell: Shell,
    pub features: FeatureFlags,
}

impl HookConfig {
    pub fn new(shell: Shell) -> Self {
        Self {
            shell,
            features: FeatureFlags::default(),
        }
    }
}

/// Generate the complete shell integration script for the given config.
pub fn generate_hooks(config: &HookConfig) -> String {
    match config.shell {
        Shell::Fish => fish_hooks(&config.features),
        Shell::Zsh => zsh_hooks(&config.features),
        Shell::Bash => bash_hooks(&config.features),
        Shell::PowerShell => pwsh_hooks(&config.features),
        Shell::Nushell => nushell_hooks(&config.features),
        Shell::Elvish => elvish_hooks(&config.features),
        Shell::Unknown(_) => String::new(),
    }
}

fn fish_hooks(f: &FeatureFlags) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str("# jcode shell integration for fish\n");
    out.push_str("# Source this file in your config.fish or let jcode auto-inject it.\n\n");

    if f.prompt_marking || f.cwd_reporting || f.exit_code {
        out.push_str("function __jcode_preexec --on-event fish_preexec\n");
        if f.prompt_marking {
            out.push_str("    printf '\\033]133;B\\007'\n");
        }
        if f.command_timing {
            out.push_str("    set -g __jcode_cmd_start (date +%s%N)\n");
        }
        out.push_str("end\n\n");
    }

    if f.prompt_marking || f.exit_code || f.cwd_reporting {
        out.push_str("function __jcode_precmd --on-event fish_prompt\n");
        if f.exit_code {
            out.push_str("    printf '\\033]133;D;%d\\007' $status\n");
        }
        if f.prompt_marking {
            out.push_str("    printf '\\033]133;A\\007'\n");
        }
        if f.cwd_reporting {
            out.push_str("    printf '\\033]7;file://%s%s\\007' (hostname) $PWD\n");
        }
        if f.command_timing {
            out.push_str("    if set -q __jcode_cmd_start\n");
            out.push_str("        set -l elapsed (math (date +%s%N) - $__jcode_cmd_start)\n");
            out.push_str("        set -g __jcode_cmd_duration $elapsed\n");
            out.push_str("        set -e __jcode_cmd_start\n");
            out.push_str("    end\n");
        }
        out.push_str("end\n\n");
    }

    out
}

fn zsh_hooks(f: &FeatureFlags) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str("# jcode shell integration for zsh\n");
    out.push_str("# Source this file at the top of your .zshrc.\n\n");

    if f.prompt_marking || f.exit_code {
        out.push_str("__jcode_precmd() {\n");
        out.push_str("    local exit_code=$?\n");
        if f.exit_code {
            out.push_str("    printf '\\033]133;D;%d\\007' $exit_code\n");
        }
        if f.prompt_marking {
            out.push_str("    printf '\\033]133;A\\007'\n");
        }
        out.push_str("}\n\n");
    }

    if f.prompt_marking || f.command_timing {
        out.push_str("__jcode_preexec() {\n");
        if f.prompt_marking {
            out.push_str("    printf '\\033]133;B\\007'\n");
        }
        if f.command_timing {
            out.push_str("    __JCODE_CMD_START=$(date +%s%N)\n");
        }
        out.push_str("}\n\n");
    }

    if f.cwd_reporting {
        out.push_str("__jcode_chpwd() {\n");
        out.push_str("    printf '\\033]7;file://%s%s\\007' ${HOSTNAME:-$(hostname)} $PWD\n");
        out.push_str("}\n\n");
    }

    // Register hooks
    out.push_str("autoload -Uz add-zsh-hook\n");
    if f.prompt_marking || f.exit_code {
        out.push_str("add-zsh-hook precmd __jcode_precmd\n");
    }
    if f.prompt_marking || f.command_timing {
        out.push_str("add-zsh-hook preexec __jcode_preexec\n");
    }
    if f.cwd_reporting {
        out.push_str("add-zsh-hook chpwd __jcode_chpwd\n");
    }
    out.push('\n');

    out
}

fn bash_hooks(f: &FeatureFlags) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str("# jcode shell integration for bash\n");
    out.push_str("# Source this file at the top of your .bashrc.\n\n");

    if f.prompt_marking || f.exit_code {
        out.push_str("__jcode_precmd() {\n");
        out.push_str("    local exit_code=$?\n");
        if f.exit_code {
            out.push_str("    printf '\\033]133;D;%d\\007' $exit_code\n");
        }
        if f.prompt_marking {
            out.push_str("    printf '\\033]133;A\\007'\n");
        }
        out.push_str("}\n\n");
    }

    if f.prompt_marking {
        out.push_str("__jcode_preexec() {\n");
        out.push_str("    printf '\\033]133;B\\007'\n");
        out.push_str("}\n\n");
    }

    // Bash 5.1+ supports PROMPT_COMMAND as an array
    if f.prompt_marking || f.exit_code {
        out.push_str("PROMPT_COMMAND=__jcode_precmd\n");
    }

    // DEBUG trap for preexec (bash 5.0+)
    if f.prompt_marking {
        out.push_str("if [[ ${BASH_VERSINFO[0]:-0} -ge 5 ]]; then\n");
        out.push_str("    trap '__jcode_preexec' DEBUG\n");
        out.push_str("fi\n");
    }

    if f.cwd_reporting {
        out.push_str("__jcode_report_cwd() {\n");
        out.push_str("    printf '\\033]7;file://%s%s\\007' ${HOSTNAME:-$(hostname)} \"$PWD\"\n");
        out.push_str("}\n");
        out.push_str("PROMPT_COMMAND+=\"; __jcode_report_cwd\"\n");
    }

    out.push('\n');
    out
}

fn pwsh_hooks(f: &FeatureFlags) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str("# jcode shell integration for PowerShell\n");
    out.push_str("# Add this to your $PROFILE.\n\n");

    if f.prompt_marking || f.exit_code {
        out.push_str("function Invoke-JcodePreCmd {\n");
        out.push_str("    $exitCode = $global:LASTEXITCODE\n");
        if f.exit_code {
            out.push_str("    Write-Host -NoNewline \"`e]133;D;${exitCode}`a\"\n");
        }
        if f.prompt_marking {
            out.push_str("    Write-Host -NoNewline \"`e]133;A`a\"\n");
        }
        out.push_str("}\n\n");
    }

    if f.prompt_marking {
        out.push_str("function Invoke-JcodePreExec {\n");
        out.push_str("    Write-Host -NoNewline \"`e]133;B`a\"\n");
        out.push_str("}\n\n");
    }

    // Wrap prompt function
    if f.prompt_marking || f.exit_code {
        out.push_str("if (-not (Test-Path Variable:\\JcodeOriginalPrompt)) {\n");
        out.push_str("    $global:JcodeOriginalPrompt = $function:Prompt\n");
        out.push_str("    $global:Prompt = {\n");
        out.push_str("        Invoke-JcodePreCmd\n");
        out.push_str("        & $global:JcodeOriginalPrompt\n");
        out.push_str("    }\n");
        out.push_str("}\n\n");
    }

    if f.cwd_reporting {
        out.push_str("function __jcodeReportCwd {\n");
        out.push_str("    $uri = [System.Uri]::new((Get-Location).Path)\n");
        out.push_str("    Write-Host -NoNewline \"`e]7;$uri`a\"\n");
        out.push_str("}\n");
        out.push_str("# Call after each command\n");
    }

    out
}

fn nushell_hooks(f: &FeatureFlags) -> String {
    let mut out = String::with_capacity(512);
    out.push_str("# jcode shell integration for nushell\n\n");

    if f.prompt_marking {
        out.push_str("$env.config.hooks.pre_prompt = [{|| print '\\e]133;A\\x07'}]\n");
        out.push_str("$env.config.hooks.command_started = [{|| print '\\e]133;B\\x07'}]\n");
    }

    out
}

fn elvish_hooks(f: &FeatureFlags) -> String {
    let mut out = String::with_capacity(512);
    out.push_str("# jcode shell integration for elvish\n\n");

    if f.prompt_marking {
        out.push_str("set edit:prompt = {\n");
        out.push_str("    print '\\e]133;A\\x07'\n");
        out.push_str("    # your existing prompt\n");
        out.push_str("}\n\n");
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fish_hooks_include_markers() {
        let config = HookConfig::new(Shell::Fish);
        let hooks = generate_hooks(&config);
        assert!(hooks.contains("133;A"));
        assert!(hooks.contains("133;B"));
        assert!(hooks.contains("133;D"));
        assert!(hooks.contains("fish_preexec"));
        assert!(hooks.contains("fish_prompt"));
    }

    #[test]
    fn zsh_hooks_include_markers() {
        let config = HookConfig::new(Shell::Zsh);
        let hooks = generate_hooks(&config);
        assert!(hooks.contains("__jcode_precmd"));
        assert!(hooks.contains("__jcode_preexec"));
        assert!(hooks.contains("add-zsh-hook"));
    }

    #[test]
    fn bash_hooks_include_prompt_command() {
        let config = HookConfig::new(Shell::Bash);
        let hooks = generate_hooks(&config);
        assert!(hooks.contains("PROMPT_COMMAND"));
        assert!(hooks.contains("trap"));
        assert!(hooks.contains("__jcode_precmd"));
    }

    #[test]
    fn pwsh_hooks_include_prompt_wrapper() {
        let config = HookConfig::new(Shell::PowerShell);
        let hooks = generate_hooks(&config);
        assert!(hooks.contains("Invoke-JcodePreCmd"));
        assert!(hooks.contains("JcodeOriginalPrompt"));
    }

    #[test]
    fn no_hooks_for_unknown_shell() {
        let config = HookConfig::new(Shell::Unknown("dash".into()));
        let hooks = generate_hooks(&config);
        assert!(hooks.is_empty());
    }

    #[test]
    fn cwd_reporting_adds_osc7() {
        let config = HookConfig {
            shell: Shell::Zsh,
            features: FeatureFlags {
                cwd_reporting: true,
                ..Default::default()
            },
        };
        let hooks = generate_hooks(&config);
        assert!(hooks.contains("file://"));
    }

    #[test]
    fn minimal_features_no_timing() {
        let config = HookConfig {
            shell: Shell::Zsh,
            features: FeatureFlags {
                prompt_marking: true,
                cwd_reporting: false,
                command_timing: false,
                exit_code: false,
                ssh_wrapping: false,
                sudo_wrapping: false,
            },
        };
        let hooks = generate_hooks(&config);
        assert!(hooks.contains("133;A"));
        assert!(!hooks.contains("date +%s%N"));
    }
}
