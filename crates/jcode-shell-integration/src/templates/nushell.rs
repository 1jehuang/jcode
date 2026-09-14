//! Nushell templates.

/// Nushell hooks use config values rather than trap-based hooks.
pub const NUSHELL_INIT: &str = r#"# jcode shell integration for Nushell
# Auto-generated — do not edit manually.
# Add `$env.config.hooks.pre_command` and `$env.config.hooks.post_command` entries.

def __jcode_preexec [] {
    print -n (char --os-escape "]133;B" + (char bel))
}

def __jcode_precmd [--exit-code: int = 0] {
    print -n (char --os-escape "]133;D;($exit-code)" + (char bel))
    print -n (char --os-escape "]133;A" + (char bel))
}

$env.config.hooks.pre_command = ($env.config.hooks.pre_command | append {|| __jcode_preexec })
$env.config.hooks.post_command = ($env.config.hooks.post_command | append {|_| __jcode_precmd --exit-code $env.LAST_EXIT_CODE })
"#;
