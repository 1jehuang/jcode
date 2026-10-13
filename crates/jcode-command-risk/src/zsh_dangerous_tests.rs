use super::*;

#[test]
fn set_contains_all_17_commands() {
    assert_eq!(ZSH_DANGEROUS_COMMANDS.len(), 17);
}

#[test]
fn is_zsh_dangerous_true_for_each_command() {
    let expected = [
        "zmodload",
        "emulate",
        "sysopen",
        "ztcp",
        "exec",
        "eval",
        "source",
        "builtin",
        "setopt",
        "unsetopt",
        "alias",
        "unalias",
        "functions",
        "whence",
        "disable",
        "enable",
        "sched",
    ];
    for cmd in expected {
        assert!(is_zsh_dangerous(cmd), "expected {cmd} to be dangerous");
    }
}

#[test]
fn is_zsh_dangerous_false_for_safe_commands() {
    for cmd in ["ls", "echo", "cd", "pwd", "grep", "cat", "rm"] {
        assert!(!is_zsh_dangerous(cmd), "expected {cmd} to be safe");
    }
}

#[test]
fn get_zsh_dangerous_reason_returns_reason_for_each_command() {
    let expected = [
        ("zmodload", "Gateway to module loading attacks"),
        ("emulate", "eval-equivalent with -c flag"),
        ("sysopen", "File I/O bypassing shell safety"),
        ("ztcp", "TCP connections enable exfiltration"),
        ("exec", "Replaces shell process"),
        ("eval", "Arbitrary code execution"),
        ("source", "Execute arbitrary file"),
        ("builtin", "Bypass function redefinitions"),
        ("setopt", "Change shell behavior unsafely"),
        ("unsetopt", "Change shell behavior unsafely"),
        ("alias", "Redefine commands"),
        ("unalias", "Redefine commands"),
        ("functions", "Define arbitrary functions"),
        ("whence", "Command path disclosure"),
        ("disable", "Modify command availability"),
        ("enable", "Modify command availability"),
        ("sched", "Schedule arbitrary commands"),
    ];
    for (cmd, reason) in expected {
        assert_eq!(get_zsh_dangerous_reason(cmd), Some(reason));
    }
}

#[test]
fn get_zsh_dangerous_reason_none_for_safe_commands() {
    assert_eq!(get_zsh_dangerous_reason("ls"), None);
    assert_eq!(get_zsh_dangerous_reason("cat"), None);
    assert_eq!(get_zsh_dangerous_reason(""), None);
}
