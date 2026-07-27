//! Deterministic risk classification for shell commands.
//!
//! # Why this exists
//!
//! jcode executes `bash` tool calls with no gate of its own: the only check in
//! `ToolRegistry::execute` is an opt-in external `pre_tool` hook, which is off
//! by default. A model that decides to run `rm -rf ~` is obeyed immediately.
//! That is issue #604, where a user lost their home directory.
//!
//! # Design
//!
//! This crate is **stage 1** of a two-stage cascade: a cheap, deterministic,
//! high-recall filter. It never calls a model and never touches the network, so
//! it costs nothing on the overwhelmingly common safe path. Stage 2 (the
//! reflection gate) only runs when this returns something other than
//! [`RiskLevel::Safe`].
//!
//! Two deliberate choices:
//!
//! 1. **Classify by blast radius, not by command name.** A denylist of
//!    `rm -rf` misses `find -delete`, `shred`, `truncate`, `dd`, and `>file`.
//!    We ask "what would this destroy, and can it be undone" instead.
//! 2. **Bias hard toward recall.** A false positive costs one reflection turn.
//!    A false negative costs a home directory. When parsing is ambiguous we
//!    escalate rather than allow.
//!
//! # Honest limitations
//!
//! This is defense in depth, not a sandbox. A determined or unlucky
//! `sh -c "$(printf ...)"` can defeat any static parser, which is exactly why
//! [`RiskLevel::Confirm`] is a reflection prompt rather than a hard block, and
//! why the catastrophic tier is a small, absolute, path-based deny that does
//! not depend on parsing the command correctly.

mod gate;
mod paths;
mod tokenize;

pub use gate::{GateOutcome, Justification, gate};
pub use paths::{ProtectedPaths, is_catastrophic_target};
pub use tokenize::{Token, tokenize};

/// How dangerous a command looks, and therefore how much scrutiny it earns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskLevel {
    /// No destructive potential detected. Run immediately, no overhead.
    Safe,
    /// Destructive but bounded (inside the working directory, recoverable via
    /// git, or under a temp dir). Run, but record it.
    Low,
    /// Irreversible and reaches outside the working directory. Requires the
    /// model to re-justify against the user's actual request before running.
    Confirm,
    /// Would destroy the user's home, root, or credentials. Never runs, and no
    /// amount of model justification can unlock it.
    Catastrophic,
}

impl RiskLevel {
    /// Whether execution may proceed without a reflection turn.
    pub fn runs_immediately(self) -> bool {
        matches!(self, RiskLevel::Safe | RiskLevel::Low)
    }

    /// Whether any confirmation could ever unlock this.
    pub fn is_absolute_deny(self) -> bool {
        matches!(self, RiskLevel::Catastrophic)
    }
}

/// A specific reason a command was flagged, used to explain the refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskFinding {
    pub level: RiskLevel,
    /// Human-readable explanation, shown to the model verbatim.
    pub reason: String,
    /// The concrete path or argument that triggered this, when there is one.
    pub target: Option<String>,
}

/// The full verdict for one command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskAssessment {
    pub level: RiskLevel,
    pub findings: Vec<RiskFinding>,
}

impl RiskAssessment {
    fn safe() -> Self {
        Self {
            level: RiskLevel::Safe,
            findings: Vec::new(),
        }
    }

    fn from_findings(findings: Vec<RiskFinding>) -> Self {
        let level = findings
            .iter()
            .map(|f| f.level)
            .max()
            .unwrap_or(RiskLevel::Safe);
        Self { level, findings }
    }

    /// The refusal text shown to the model, phrased to force a comparison
    /// against what the user actually asked for rather than a yes/no reflex.
    pub fn explanation(&self) -> String {
        let mut out = String::new();
        for finding in &self.findings {
            out.push_str("- ");
            out.push_str(&finding.reason);
            if let Some(target) = &finding.target {
                out.push_str(&format!(" (target: {target})"));
            }
            out.push('\n');
        }
        out
    }
}

/// Context needed to judge blast radius. Supplied by the caller because this
/// crate deliberately does no I/O of its own beyond path inspection.
#[derive(Debug, Clone, Default)]
pub struct RiskContext {
    /// The tool call's working directory, if any.
    pub working_dir: Option<std::path::PathBuf>,
    /// The user's home directory.
    pub home_dir: Option<std::path::PathBuf>,
}

impl RiskContext {
    pub fn from_env(working_dir: Option<std::path::PathBuf>) -> Self {
        Self {
            working_dir,
            home_dir: dirs_home(),
        }
    }
}

fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(std::path::PathBuf::from)
}

/// Commands that destroy data as their primary purpose.
///
/// Presence here does not by itself mean danger: `rm` inside the working
/// directory is routine. It means "inspect the targets".
const DESTRUCTIVE_COMMANDS: &[&str] = &[
    "rm", "rmdir", "shred", "unlink", "truncate", "dd", "mkfs", "fdisk", "parted", "wipefs", "srm",
];

/// Commands that run another command. The real program is one of their
/// arguments, so `sudo rm -rf ~` must be unwrapped before classification or the
/// destructive verb is never seen at all.
const WRAPPER_COMMANDS: &[&str] = &[
    "sudo", "doas", "env", "nice", "ionice", "time", "timeout", "nohup", "xargs", "command",
    "builtin", "exec", "setsid", "stdbuf", "chroot", "su", "watch", "eval",
];

/// Wrapper options that consume the following word as their value.
const WRAPPER_FLAGS_WITH_VALUES: &[&str] = &[
    "-n",
    "-u",
    "-s",
    "-c",
    "-k",
    "--signal",
    "--adjustment",
    "--user",
];

/// Shells, which take their program from a string argument we cannot parse
/// reliably. Treated as opaque rather than assumed safe.
const SHELL_COMMANDS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "fish"];

/// Commands that are destructive only with specific flags.
const CONDITIONALLY_DESTRUCTIVE: &[(&str, &[&str])] = &[
    ("find", &["-delete", "-exec"]),
    ("git", &["clean"]),
    ("chmod", &["-R"]),
    ("chown", &["-R"]),
];

/// Assess a single shell command string.
///
/// This is the crate's entry point and is intentionally total: any input,
/// including garbage, produces an assessment rather than an error.
pub fn assess(command: &str, ctx: &RiskContext) -> RiskAssessment {
    let mut findings = Vec::new();

    for segment in tokenize::split_segments(command) {
        assess_segment(&segment, ctx, &mut findings);
    }

    if findings.is_empty() {
        return RiskAssessment::safe();
    }
    RiskAssessment::from_findings(findings)
}

fn assess_segment(tokens: &[Token], ctx: &RiskContext, findings: &mut Vec<RiskFinding>) {
    // Strip wrapper programs (`sudo`, `env`, `xargs`, ...) so the destructive
    // verb underneath is the one we classify. Without this, any common prefix
    // is a complete bypass.
    let mut tokens = tokens;
    let mut wrapped_by: Option<String> = None;
    loop {
        let Some(first) = tokens.first() else {
            // Ran off the end while unwrapping: the payload is invisible.
            if let Some(wrapper) = wrapped_by {
                findings.push(RiskFinding {
                    level: RiskLevel::Confirm,
                    reason: format!(
                        "`{wrapper}` runs another command that could not be \
                         identified statically"
                    ),
                    target: None,
                });
            }
            return;
        };
        let name = first.basename();
        if !WRAPPER_COMMANDS.contains(&name.as_str()) {
            break;
        }
        wrapped_by = Some(name);
        // Skip the wrapper plus its own options and `VAR=value` assignments,
        // landing on the wrapped program. Options that take a separate value
        // (`nice -n 10`, `timeout 5`) must consume that value too.
        let rest = &tokens[1..];
        let mut idx = 0;
        while idx < rest.len() {
            let token = &rest[idx];
            if token.is_operator || token.text.contains('=') {
                idx += 1;
                continue;
            }
            if token.is_flag() {
                idx += 1;
                // A short flag known to take an argument consumes the next word.
                if WRAPPER_FLAGS_WITH_VALUES.contains(&token.text.as_str()) && idx < rest.len() {
                    idx += 1;
                }
                continue;
            }
            // A bare number is an operand of the wrapper itself (`timeout 5`),
            // not the program to run.
            if token.text.chars().all(|c| c.is_ascii_digit() || c == '.') {
                idx += 1;
                continue;
            }
            break;
        }
        tokens = &rest[idx..];
    }

    let Some(program) = tokens.first() else {
        // A wrapper with nothing recognizable after it hides its payload.
        if let Some(wrapper) = wrapped_by {
            findings.push(RiskFinding {
                level: RiskLevel::Confirm,
                reason: format!(
                    "`{wrapper}` runs another command that could not be \
                     identified statically"
                ),
                target: None,
            });
        }
        return;
    };
    let program_name = program.basename();

    // A shell invoked with an inline script is opaque to this parser. Assess
    // the script text too, so `sh -c "rm -rf ~"` is not a free pass.
    if SHELL_COMMANDS.contains(&program_name.as_str()) {
        for token in tokens.iter().skip(1).filter(|t| !t.is_flag()) {
            for segment in tokenize::split_segments(&token.text) {
                assess_segment(&segment, ctx, findings);
            }
        }
        return;
    }

    let is_destructive = DESTRUCTIVE_COMMANDS.contains(&program_name.as_str());
    let conditional_flags = CONDITIONALLY_DESTRUCTIVE
        .iter()
        .find(|(name, _)| *name == program_name)
        .map(|(_, flags)| *flags);

    let triggered = if is_destructive {
        true
    } else if let Some(flags) = conditional_flags {
        tokens.iter().any(|t| flags.contains(&t.text.as_str()))
    } else {
        false
    };

    // Output redirection truncates a file even with a harmless program.
    let redirect_targets: Vec<&Token> = tokens
        .iter()
        .filter(|t| t.is_truncating_redirect_target)
        .collect();

    if !triggered && redirect_targets.is_empty() {
        return;
    }

    let mut targets: Vec<&Token> = tokens
        .iter()
        .skip(1)
        .filter(|t| !t.is_flag() && !t.is_operator)
        .collect();
    targets.extend(redirect_targets.iter().copied());

    // A destructive command fed by a pipe takes its operands from the previous
    // command's output, which we cannot enumerate. `find ~ -type f | xargs rm`
    // is a real deletion of home contents that neither segment reveals on its
    // own, so escalate rather than trust the visible arguments.
    if triggered && tokens.first().is_some_and(|t| t.receives_pipe) {
        findings.push(RiskFinding {
            level: RiskLevel::Confirm,
            reason: format!(
                "`{program_name}` deletes paths supplied by a pipe, so the set \
                 of affected files cannot be checked before it runs"
            ),
            target: None,
        });
    }

    // A destructive program with no parsable target is more suspicious, not
    // less: we could not see what it would touch.
    if triggered && targets.is_empty() {
        findings.push(RiskFinding {
            level: RiskLevel::Confirm,
            reason: format!(
                "`{program_name}` is destructive but its target could not be \
                 determined statically, so its blast radius is unknown"
            ),
            target: None,
        });
        return;
    }

    let recursive = tokens.iter().any(|t| t.is_recursive_flag());

    for target in targets {
        // `dd`-style `key=value` operands hide the path from a naive scan.
        let raw = target
            .text
            .split_once('=')
            .filter(|(key, _)| matches!(*key, "of" | "if" | "seek" | "conv"))
            .map(|(_, value)| value)
            .unwrap_or(&target.text);
        let expanded = paths::expand(raw, ctx);
        if let Some(finding) = paths::classify_target(&expanded, raw, recursive, ctx) {
            findings.push(finding);
        }
    }
}

#[cfg(test)]
#[path = "assess_tests.rs"]
mod assess_tests;
