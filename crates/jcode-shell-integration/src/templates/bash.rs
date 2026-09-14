//! Bash shell templates.

/// Preexec hook (via DEBUG trap).
pub const BASH_PREEXEC: &str = r#"__jcode_preexec() {
    printf '\033]133;B\007'
    __JCODE_CMD_START=$(date +%s%N)
}
"#;

/// Precmd hook (PROMPT_COMMAND).
pub const BASH_PRECMD: &str = r#"__jcode_precmd() {
    local exit_code=$?
    printf '\033]133;D;%d\007' $exit_code
    printf '\033]133;A\007'
}
"#;

/// Full init script for bash.
pub const BASH_INIT: &str = r#"# jcode shell integration for bash
# Auto-generated — do not edit manually.

__jcode_preexec() {
    printf '\033]133;B\007'
    __JCODE_CMD_START=$(date +%s%N)
}

__jcode_precmd() {
    local exit_code=$?
    printf '\033]133;D;%d\007' $exit_code
    printf '\033]133;A\007'
}

# For bash >= 5.1, use DEBUG trap; older versions need PROMPT_COMMAND trick
if [[ ${BASH_VERSINFO[0]} -ge 5 ]] || [[ ${BASH_VERSINFO[0]} -eq 5 && ${BASH_VERSINFO[1]} -ge 1 ]]; then
    trap '__jcode_preexec' DEBUG
else
    # Bash < 5.1: use DEBUG trap for preexec
    trap '__jcode_preexec' DEBUG
fi

PROMPT_COMMAND=__jcode_precmd
"#;

/// Completion header.
pub const BASH_COMPLETION_HEADER: &str = "# jcode bash completions\n# Auto-generated — do not edit manually.\n\n";
