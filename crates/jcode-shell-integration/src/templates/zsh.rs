//! Zsh shell templates.

/// Preexec hook (runs before each command).
pub const ZSH_PREEXEC: &str = r#"__jcode_preexec() {
    printf '\033]133;B\007'
    __JCODE_CMD_START=$(date +%s%N)
}
"#;

/// Precmd hook (runs before each prompt).
pub const ZSH_PRECMD: &str = r#"__jcode_precmd() {
    local exit_code=$?
    printf '\033]133;D;%d\007' $exit_code
    printf '\033]133;A\007'
    # Report CWD via OSC 7
    printf '\033]7;file://%s%s\007' ${(%)_:%%M} $PWD
}
"#;

/// Hook registration.
pub const ZSH_HOOK_REGISTER: &str = r#"autoload -Uz add-zsh-hook
add-zsh-hook preexec __jcode_preexec
add-zsh-hook precmd __jcode_precmd
"#;

/// Full init script for zsh.
pub const ZSH_INIT: &str = r#"# jcode shell integration for zsh
# Auto-generated — do not edit manually.

__jcode_preexec() {
    printf '\033]133;B\007'
    __JCODE_CMD_START=$(date +%s%N)
}

__jcode_precmd() {
    local exit_code=$?
    printf '\033]133;D;%d\007' $exit_code
    printf '\033]133;A\007'
    # Report CWD via OSC 7
    printf '\033]7;file://%s%s\007' ${(%)_:%%M} $PWD
}

autoload -Uz add-zsh-hook
add-zsh-hook preexec __jcode_preexec
add-zsh-hook precmd __jcode_precmd
"#;

/// Completion header.
pub const ZSH_COMPLETION_HEADER: &str = "# jcode zsh completions\n#compdef jcode\n# Auto-generated — do not edit manually.\n\n";
