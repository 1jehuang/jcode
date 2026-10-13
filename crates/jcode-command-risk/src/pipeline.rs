//! Security pipeline: orchestrates all checks in sequence.
//!
//! Chains preprocessing (quote extraction, redirection stripping, heredoc
//! detection), the core blast-radius assessment, and additional pattern checks
//! derived from Claude Code's `bashSecurity.ts` validation pipeline.
//!
//! Short-circuits on [`PipelineResult::Block`] (absolute deny) since no later
//! check can override a catastrophic finding.

use crate::heredoc;
use crate::quote_parser;
use crate::substitution;
use crate::tokenize;
use crate::zsh_dangerous;
use crate::{RiskContext, RiskLevel, assess};

/// Outcome of the full security pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelineResult {
    /// Safe. Caller may proceed.
    Continue,
    /// Hard block (catastrophic). No justification unlocks it.
    Block { reason: String },
    /// Suspicious. Model should justify before proceeding.
    Ask { reason: String },
}

/// Run the full security pipeline with default context.
pub fn run_security_pipeline(command: &str) -> PipelineResult {
    run_pipeline(command, &RiskContext::default())
}

/// Run the pipeline with caller-supplied context.
pub fn run_pipeline(command: &str, ctx: &RiskContext) -> PipelineResult {
    let extraction = quote_parser::extract_quotes(command);
    let fully = &extraction.fully_unquoted;
    let unquoted_kq = &extraction.unquoted_keep_quotes;

    // --- Pre-processing checks (cheap, no tokenisation needed) --------------

    let checks: Vec<Option<PipelineResult>> = vec![
        check_empty(command),
        check_incomplete(fully),
        check_control_chars(command),
        check_carriage_return(command),
        check_newline_destructive(command, fully),
        check_unicode_whitespace(command),
        check_obfuscated_flags(unquoted_kq, fully),
        check_backslash_ws(command),
        check_backslash_ops(command),
        check_brace_expansion(fully),
        check_substitution(command),
        check_zsh_dangerous(command),
        check_heredoc(command),
        check_ifs(fully),
        check_dangerous_vars(fully),
    ];

    if let Some(result) = checks.into_iter().flatten().next() {
        return result;
    }

    // --- Core blast-radius assessment (tokenisation + path validation) ------
    let assessment = assess(command, ctx);
    match assessment.level {
        RiskLevel::Safe | RiskLevel::Low => PipelineResult::Continue,
        RiskLevel::Confirm => PipelineResult::Ask {
            reason: assessment.explanation(),
        },
        RiskLevel::Catastrophic => PipelineResult::Block {
            reason: assessment.explanation(),
        },
    }
}

// ---------------------------------------------------------------------------
// Individual checks — each returns None (continue) or Some(PipelineResult).
// ---------------------------------------------------------------------------

fn check_empty(cmd: &str) -> Option<PipelineResult> {
    cmd.trim().is_empty().then_some(PipelineResult::Ask {
        reason: "Empty command".into(),
    })
}

fn check_incomplete(fully: &str) -> Option<PipelineResult> {
    let t = fully.trim_end();
    (t.ends_with("&&") || t.ends_with("||") || t.ends_with('|') || t.ends_with(';')).then_some(
        PipelineResult::Ask {
            reason: "Incomplete command (trailing operator)".into(),
        },
    )
}

fn check_control_chars(cmd: &str) -> Option<PipelineResult> {
    cmd.bytes()
        .any(|b| b < 0x20 && b != b'\n' && b != b'\t' && b != b'\r')
        .then_some(PipelineResult::Ask {
            reason: "Contains non-printable control characters".into(),
        })
}

fn check_carriage_return(cmd: &str) -> Option<PipelineResult> {
    cmd.contains('\r').then_some(PipelineResult::Ask {
        reason: "Contains carriage return that could alter parsing".into(),
    })
}

fn check_newline_destructive(cmd: &str, fully: &str) -> Option<PipelineResult> {
    if !cmd.contains('\n') || !fully.contains('\n') {
        return None;
    }
    let destructive = ["rm", "rmdir", "shred", "unlink", "truncate", "dd", "mkfs"];
    for line in fully.lines().skip(1) {
        let word = line.split_whitespace().next().unwrap_or("");
        let base = word.rsplit('/').next().unwrap_or(word);
        if destructive.contains(&base) {
            return Some(PipelineResult::Ask {
                reason: "Multiline command contains destructive command on subsequent line".into(),
            });
        }
    }
    None
}

fn check_unicode_whitespace(cmd: &str) -> Option<PipelineResult> {
    cmd.chars()
        .any(|c| c != ' ' && c != '\t' && c != '\n' && c != '\r' && c.is_whitespace())
        .then_some(PipelineResult::Ask {
            reason: "Contains non-ASCII whitespace that could alter parsing".into(),
        })
}

fn check_obfuscated_flags(unquoted_kq: &str, fully: &str) -> Option<PipelineResult> {
    // Check for flag with embedded quote: -"v", -'v'
    let bytes = unquoted_kq.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'-' && i + 1 < bytes.len() && bytes[i + 1] != b'-' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != b' ' && bytes[j] != b'\t' {
                if matches!(bytes[j], b'"' | b'\'' | b'`') {
                    return Some(PipelineResult::Ask {
                        reason: "Quoted characters in flag names".into(),
                    });
                }
                j += 1;
            }
            i = j;
        } else {
            i += 1;
        }
    }
    // Check for quoted flag prefix: ' `--output` or " `--output`
    let chars: Vec<char> = fully.chars().collect();
    for w in chars.windows(3) {
        if (w[0] == ' ' || w[0] == '\t') && matches!(w[1], '\'' | '"' | '`') && w[2] == '-' {
            return Some(PipelineResult::Ask {
                reason: "Quoted characters in flag names".into(),
            });
        }
    }
    None
}

fn check_backslash_ws(cmd: &str) -> Option<PipelineResult> {
    scan_quotes(cmd, |cmd, i, in_sq, in_dq| {
        let bytes = cmd.as_bytes();
        if bytes[i] == b'\\'
            && !in_sq
            && !in_dq
            && i + 1 < bytes.len()
            && matches!(bytes[i + 1], b' ' | b'\t')
        {
            return true;
        }
        false
    })
    .then_some(PipelineResult::Ask {
        reason: "Backslash-escaped whitespace could alter command parsing".into(),
    })
}

fn check_backslash_ops(cmd: &str) -> Option<PipelineResult> {
    scan_quotes(cmd, |cmd, i, in_sq, in_dq| {
        let bytes = cmd.as_bytes();
        if bytes[i] == b'\\'
            && !in_sq
            && !in_dq
            && i + 1 < bytes.len()
            && matches!(bytes[i + 1], b';' | b'|' | b'&' | b'>' | b'<')
        {
            return true;
        }
        false
    })
    .then_some(PipelineResult::Ask {
        reason: "Backslash-escaped shell operator could alter parsing".into(),
    })
}

/// Walk `cmd` tracking single/double quotes. Call `pred(cmd, byte_index, in_sq, in_dq)`
/// for every byte position so the caller can decide based on per-position quote state.
fn scan_quotes<F>(cmd: &str, pred: F) -> bool
where
    F: Fn(&str, usize, bool, bool) -> bool,
{
    let bytes = cmd.as_bytes();
    let mut sq = false;
    let mut dq = false;
    let mut i = 0;
    while i < bytes.len() {
        // Backslash outside SQ escapes the next byte (skip both).
        if bytes[i] == b'\\' && !sq {
            // Check at this position with current quote state BEFORE skipping.
            if pred(cmd, i, sq, dq) {
                return true;
            }
            i += 2;
            continue;
        }
        if bytes[i] == b'\'' && !dq {
            sq = !sq;
        } else if bytes[i] == b'"' && !sq {
            dq = !dq;
        }
        if pred(cmd, i, sq, dq) {
            return true;
        }
        i += 1;
    }
    false
}

fn check_brace_expansion(fully: &str) -> Option<PipelineResult> {
    if let (Some(start), Some(end)) = (fully.find('{'), fully[start_after(fully, '{')..].find('}'))
    {
        let inner = &fully[start + 1..start + 1 + end];
        if inner.contains(',') || inner.contains("..") {
            return Some(PipelineResult::Ask {
                reason: "Brace expansion detected".into(),
            });
        }
    }
    None
}

fn start_after(s: &str, ch: char) -> usize {
    s.find(ch).map_or(0, |i| i + 1)
}

fn check_substitution(cmd: &str) -> Option<PipelineResult> {
    let sub = substitution::detect_substitution(cmd);
    sub.has_substitution.then_some(PipelineResult::Ask {
        reason: format!(
            "Shell substitution: {}",
            sub.pattern_name.unwrap_or_default()
        ),
    })
}

fn check_zsh_dangerous(cmd: &str) -> Option<PipelineResult> {
    let tokens = tokenize::tokenize(cmd);
    let first = tokens.first()?;
    let base = first.basename();
    if zsh_dangerous::is_zsh_dangerous(&base) {
        let reason =
            zsh_dangerous::get_zsh_dangerous_reason(&base).unwrap_or("dangerous zsh built-in");
        Some(PipelineResult::Ask {
            reason: format!("Zsh `{base}`: {reason}"),
        })
    } else {
        None
    }
}

fn check_heredoc(cmd: &str) -> Option<PipelineResult> {
    if heredoc::has_heredoc_substitution(cmd) && !heredoc::is_safe_heredoc(cmd) {
        Some(PipelineResult::Ask {
            reason: "Unvalidated heredoc in command substitution".into(),
        })
    } else {
        None
    }
}

fn check_ifs(fully: &str) -> Option<PipelineResult> {
    fully.starts_with("IFS=").then_some(PipelineResult::Ask {
        reason: "Sets IFS, altering shell word splitting".into(),
    })
}

fn check_dangerous_vars(fully: &str) -> Option<PipelineResult> {
    for var in [
        "PATH=",
        "LD_PRELOAD=",
        "LD_LIBRARY_PATH=",
        "ENV=",
        "BASH_ENV=",
        "HISTFILE=",
    ] {
        if fully.contains(var) {
            return Some(PipelineResult::Ask {
                reason: format!("Sets dangerous variable `{var}`"),
            });
        }
    }
    None
}

#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod pipeline_tests;
