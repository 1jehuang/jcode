//! Fish shell templates.

/// Preexec hook (runs before each command).
pub const FISH_PREEXEC: &str = r#"function __jcode_preexec --on-event fish_preexec
    # OSC 133 command start
    printf '\033]133;B\007'
    # Record command start time for timing
    set -g __jcode_cmd_start (date +%s%N)
end
"#;

/// Precmd hook (runs before each prompt).
pub const FISH_PRECMD: &str = r#"function __jcode_precmd --on-event fish_prompt
    # OSC 133 prompt start
    printf '\033]133;A\007'
    # OSC 133 command finished with exit code
    printf '\033]133;D;%d\007' $status
end
"#;

/// Full init script for fish.
pub const FISH_INIT: &str = r#"# jcode shell integration for fish
# Auto-generated — do not edit manually.

# Enable universal hooks if not already set
functions -c fish_preexec __jcode_original_preexec 2>/dev/null || true

function __jcode_preexec --on-event fish_preexec
    printf '\033]133;B\007'
    set -g __jcode_cmd_start (date +%s%N)
end

function __jcode_precmd --on-event fish_prompt
    printf '\033]133;A\007'
    printf '\033]133;D;%d\007' $status
    # OSC 7: report CWD
    printf '\033]7;file://%s%s\007' (hostname) $PWD
end
"#;

/// Completion template prefix.
pub const FISH_COMPLETION_HEADER: &str = "# jcode fish completions\n# Auto-generated — do not edit manually.\n\n";
