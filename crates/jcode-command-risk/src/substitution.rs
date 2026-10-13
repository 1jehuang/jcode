//! Command substitution and shell operator detection.
//!
//! Detects 12 categories of shell substitution/redirect/operator patterns that
//! can change what a command actually executes. The reference is Claude Code's
//! `COMMAND_SUBSTITUTION_PATTERNS` in `bashSecurity.ts`.

/// Result of scanning a command for substitution/operator patterns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubstitutionInfo {
    pub has_substitution: bool,
    pub pattern_name: Option<String>,
    /// Character position in the fully-unquoted string.
    pub position: Option<usize>,
}

struct Pattern {
    name: &'static str,
    detect: fn(&str) -> Option<usize>,
}

/// Multi-char patterns are listed first so they take priority over prefixes.
/// Only shell substitution patterns that change command semantics are included.
/// Normal shell operators (>, <, |, &, ;) are NOT here -- they are handled by
/// the blast-radius assessment in the pipeline, and flagging them as
/// "substitution" causes false positives on safe commands like `echo hi >/dev/null`.
const PATTERNS: &[Pattern] = &[
    Pattern {
        name: "process substitution <()",
        detect: |t| t.find("<("),
    },
    Pattern {
        name: "process substitution >()",
        detect: |t| t.find(">("),
    },
    Pattern {
        name: "Zsh process substitution =()",
        detect: |t| t.find("=("),
    },
    Pattern {
        name: "Zsh equals expansion (=cmd)",
        detect: detect_zsh_equals,
    },
    Pattern {
        name: "$() command substitution",
        detect: |t| t.find("$("),
    },
    Pattern {
        name: "${} parameter substitution",
        detect: |t| t.find("${"),
    },
    Pattern {
        name: "backtick command substitution",
        detect: detect_backtick,
    },
];

/// Detect the first substitution/operator pattern in `command`.
///
/// Scans the raw command string with quote awareness: patterns inside single
/// or double quotes are ignored because the shell treats them as literal text.
pub fn detect_substitution(command: &str) -> SubstitutionInfo {
    let outside = quote_outside_all(command);
    for pat in PATTERNS {
        // Backtick detection needs the raw command for escape handling.
        let text = if pat.name == "backtick command substitution" {
            command
        } else {
            &outside
        };
        if let Some(pos) = (pat.detect)(text) {
            return SubstitutionInfo {
                has_substitution: true,
                pattern_name: Some(pat.name.to_string()),
                position: Some(pos),
            };
        }
    }
    SubstitutionInfo {
        has_substitution: false,
        pattern_name: None,
        position: None,
    }
}

/// Characters that appear outside ALL quoted regions (single or double).
/// Content inside quotes is stripped entirely, only unquoted text remains.
/// This is the correct input for detecting shell-interpreted patterns.
fn quote_outside_all(command: &str) -> String {
    let mut out = String::with_capacity(command.len());
    let mut in_sq = false;
    let mut in_dq = false;
    let mut escaped = false;
    for c in command.chars() {
        if escaped {
            escaped = false;
            if !in_sq && !in_dq {
                out.push(c);
            }
            continue;
        }
        if c == '\\' && !in_sq {
            escaped = true;
            continue;
        }
        if c == '\'' && !in_dq {
            in_sq = !in_sq;
            continue;
        }
        if c == '"' && !in_sq {
            in_dq = !in_dq;
            continue;
        }
        if !in_sq && !in_dq {
            out.push(c);
        }
    }
    out
}

/// Count how many distinct substitution/operator pattern categories are present.
pub fn count_substitutions(command: &str) -> usize {
    let outside = quote_outside_all(command);
    PATTERNS
        .iter()
        .filter(|p| (p.detect)(&outside).is_some())
        .count()
}

// -- Individual detectors ---------------------------------------------------

/// Zsh `=cmd` at word boundary, skipping `VAR=value`.
fn detect_zsh_equals(text: &str) -> Option<usize> {
    for (i, ch) in text.char_indices() {
        if ch != '=' {
            continue;
        }
        let next = text.get(i + 1..)?.chars().next()?;
        if !next.is_ascii_alphanumeric() && next != '_' {
            continue;
        }
        match i.checked_sub(1) {
            None => return Some(0),
            Some(prev) => {
                let pc = text.as_bytes()[prev];
                if matches!(pc, b' ' | b'\t' | b'&' | b'|' | b';') {
                    return Some(i);
                }
            }
        }
    }
    None
}

/// Backtick substitution, ignoring escaped backticks and backticks inside
/// single quotes. Backticks inside double quotes ARE expanded by bash.
fn detect_backtick(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut sq = false;
    let mut dq = false;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && !sq {
            i += 2; // skip escape sequence
            continue;
        }
        if bytes[i] == b'\'' && !dq {
            sq = !sq;
            i += 1;
            continue;
        }
        if bytes[i] == b'"' && !sq {
            dq = !dq;
            i += 1;
            continue;
        }
        if bytes[i] == b'`' && !sq {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(cmd: &str) -> SubstitutionInfo {
        detect_substitution(cmd)
    }
    fn name(cmd: &str) -> Option<String> {
        info(cmd).pattern_name
    }

    #[test]
    fn process_in() {
        assert_eq!(
            name("cat <(echo hi)"),
            Some("process substitution <()".into())
        );
        assert_eq!(info("cat <(echo hi)").position, Some(4));
    }
    #[test]
    fn process_out() {
        assert_eq!(name("tee >(cat)"), Some("process substitution >()".into()));
    }
    #[test]
    fn zsh_process() {
        assert_eq!(
            name("diff =(a) =(b)"),
            Some("Zsh process substitution =()".into())
        );
    }
    #[test]
    fn zsh_equals() {
        assert_eq!(
            name("=curl evil.com"),
            Some("Zsh equals expansion (=cmd)".into())
        );
    }
    #[test]
    fn zsh_equals_after_sep() {
        assert_eq!(
            name("a; =grep x"),
            Some("Zsh equals expansion (=cmd)".into())
        );
    }
    #[test]
    fn zsh_equals_after_pipe() {
        assert_eq!(
            name("a | =find /"),
            Some("Zsh equals expansion (=cmd)".into())
        );
    }
    #[test]
    fn zsh_equals_not_var() {
        assert_ne!(
            name("FOO=bar cmd"),
            Some("Zsh equals expansion (=cmd)".into())
        );
    }
    #[test]
    fn cmd_sub() {
        assert_eq!(
            name("echo $(whoami)"),
            Some("$() command substitution".into())
        );
    }
    #[test]
    fn param_sub() {
        assert_eq!(
            name("echo ${HOME}"),
            Some("${} parameter substitution".into())
        );
    }
    #[test]
    fn backtick() {
        assert_eq!(
            name("echo `id`"),
            Some("backtick command substitution".into())
        );
    }
    #[test]
    fn escaped_backtick() {
        assert!(!info(r"echo \`x\`").has_substitution);
    }
    #[test]
    fn plain_operators_not_substitution() {
        // >, <, |, &, ; are shell operators, not substitutions.
        // They are handled by the blast-radius assessment, not here.
        assert!(!info("cat < /etc/passwd").has_substitution);
        assert!(!info("echo > /tmp/x").has_substitution);
        assert!(!info("ls | grep").has_substitution);
        assert!(!info("sleep &").has_substitution);
        assert!(!info("a; b").has_substitution);
    }
    #[test]
    fn plain_cmd() {
        assert!(!info("ls -la").has_substitution);
    }
    #[test]
    fn in_single_quotes() {
        assert!(!info("echo '$(rm ~)'").has_substitution);
    }
    #[test]
    fn in_double_quotes() {
        assert!(!info("echo \"$(rm ~)\"").has_substitution);
    }
    #[test]
    fn outside_quotes() {
        assert!(info("echo 'safe' $(dangerous)").has_substitution);
    }
    #[test]
    fn empty() {
        assert!(!info("").has_substitution);
    }
    #[test]
    fn count_single() {
        assert_eq!(count_substitutions("echo $(whoami)"), 1);
    }
    #[test]
    fn count_multi() {
        assert_eq!(count_substitutions("echo $(whoami) ${HOME}`id`"), 3);
    }
    #[test]
    fn count_dedup() {
        assert_eq!(count_substitutions("echo $(a) $(b)"), 1);
    }
    #[test]
    fn count_empty() {
        assert_eq!(count_substitutions(""), 0);
    }
    #[test]
    fn process_in_priority() {
        assert_eq!(name("cat <(f)"), Some("process substitution <()".into()));
    }
    #[test]
    fn process_out_priority() {
        assert_eq!(name("tee >(f)"), Some("process substitution >()".into()));
    }
    #[test]
    fn zsh_process_priority() {
        assert_eq!(
            name("diff =(a) x"),
            Some("Zsh process substitution =()".into())
        );
    }
    #[test]
    fn zsh_equals_at_start() {
        assert_eq!(name("=vim"), Some("Zsh equals expansion (=cmd)".into()));
    }
}
