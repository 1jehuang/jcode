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
    /// Destructive target cannot be determined statically. Requires the model
    /// to re-justify against the user's actual request before running.
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
    /// Dedicated scratch directory supplied to the command.
    pub scratch_dir: Option<std::path::PathBuf>,
}

impl RiskContext {
    pub fn from_env(working_dir: Option<std::path::PathBuf>) -> Self {
        Self {
            working_dir,
            home_dir: dirs_home(),
            scratch_dir: std::env::var_os("JCODE_SCRATCH_DIR")
                .filter(|value| !value.is_empty())
                .map(std::path::PathBuf::from),
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

/// Shell grammar words that may prefix the actual command in a segment.
const SHELL_CONTROL_PREFIXES: &[&str] = &[
    "then", "do", "else", "elif", "if", "while", "until", "case", "in", "select",
];

/// Whether a wrapper option consumes the following word. Option spelling is
/// wrapper-specific: `nice -n 10` takes a value, while `sudo -n ls` does not.
fn wrapper_flag_takes_value(wrapper: &str, flag: &str) -> bool {
    match wrapper {
        "sudo" | "doas" => matches!(flag, "-u" | "--user" | "-g" | "--group" | "-C"),
        "env" => matches!(flag, "-u" | "--unset" | "-C" | "--chdir"),
        "nice" => matches!(flag, "-n" | "--adjustment"),
        "ionice" => matches!(
            flag,
            "-c" | "--class" | "-n" | "--classdata" | "-p" | "--pid"
        ),
        "timeout" => matches!(flag, "-s" | "--signal" | "-k" | "--kill-after"),
        "xargs" => matches!(
            flag,
            "-n" | "--max-args" | "-P" | "--max-procs" | "-s" | "--max-chars"
        ),
        "chroot" => matches!(flag, "--userspec" | "--groups"),
        _ => false,
    }
}

/// Shells, which take their program from a string argument we cannot parse
/// reliably. Treated as opaque rather than assumed safe.
const SHELL_COMMANDS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "fish"];

/// Commands that are destructive only with specific flags.
const CONDITIONALLY_DESTRUCTIVE: &[(&str, &[&str])] =
    &[("git", &["clean"]), ("chmod", &["-R"]), ("chown", &["-R"])];

/// Assess a single shell command string.
///
/// This is the crate's entry point and is intentionally total: any input,
/// including garbage, produces an assessment rather than an error.
pub fn assess(command: &str, ctx: &RiskContext) -> RiskAssessment {
    let mut findings = Vec::new();
    // Keep the trusted context for literal protected paths, but never approve
    // commands whose own assignments can invalidate known-variable expansion.
    if command.contains("HOME=") || command.contains("JCODE_SCRATCH_DIR=") {
        findings.push(RiskFinding {
            level: RiskLevel::Confirm,
            reason: "command reassigns a path variable used by risk assessment".into(),
            target: None,
        });
    }

    for segment in tokenize::split_segments(command) {
        assess_segment(&segment, ctx, &mut findings);
    }

    if findings.is_empty() {
        return RiskAssessment::safe();
    }
    RiskAssessment::from_findings(findings)
}

fn assess_segment(tokens: &[Token], ctx: &RiskContext, findings: &mut Vec<RiskFinding>) {
    // Shell redirects happen even when a wrapper only prints information.
    for target in tokens.iter().filter(|t| t.is_truncating_redirect_target) {
        if !is_safe_redirect_sink(&target.text) {
            let expanded = paths::expand(&target.text, ctx);
            if let Some(finding) = paths::classify_target(&expanded, &target.text, false, ctx) {
                findings.push(finding);
            }
        }
    }
    // Strip wrapper programs (`sudo`, `env`, `xargs`, ...) so the destructive
    // verb underneath is the one we classify. Without this, any common prefix
    // is a complete bypass.
    let mut tokens = tokens;
    while tokens.len() > 1
        && tokens
            .first()
            .is_some_and(|token| SHELL_CONTROL_PREFIXES.contains(&token.text.as_str()))
    {
        tokens = &tokens[1..];
    }
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
        // `command -v/-V` describes names, including names of wrappers. Only
        // inspect the option prefix, never options belonging to its payload.
        if name == "command"
            && tokens
                .iter()
                .skip(1)
                .take_while(|t| t.is_flag() && t.text != "--")
                .any(|t| {
                    !t.text.starts_with("--") && t.text[1..].chars().any(|c| matches!(c, 'v' | 'V'))
                })
        {
            return;
        }
        wrapped_by = Some(name.clone());
        // Skip the wrapper plus its own options and `VAR=value` assignments,
        // landing on the wrapped program. Options that take a separate value
        // (`nice -n 10`, `timeout 5`) must consume that value too.
        let rest = &tokens[1..];
        let mut idx = 0;
        while idx < rest.len() {
            let token = &rest[idx];
            if name == "env"
                && (token.text.starts_with("--split-string")
                    || (token.text.starts_with('-')
                        && !token.text.starts_with("--")
                        && token.text.contains('S')))
            {
                findings.push(RiskFinding {
                    level: RiskLevel::Confirm,
                    reason: "`env` split-string payload cannot be identified statically".into(),
                    target: None,
                });
                return;
            }
            if token.is_operator || token.text.contains('=') {
                idx += 1;
                continue;
            }
            if token.is_flag() {
                idx += 1;
                // A short flag known to take an argument consumes the next word.
                if wrapper_flag_takes_value(&name, &token.text) && idx < rest.len() {
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
        if name == "env" && idx == rest.len() {
            return;
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

    if program_name == "find" {
        assess_find(tokens, ctx, findings);
        return;
    }

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

    if !triggered {
        return;
    }

    let targets: Vec<&Token> = tokens
        .iter()
        .skip(1)
        .filter(|t| !t.is_flag() && !t.is_operator && !t.is_truncating_redirect_target)
        .collect();

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

    // Flags belonging to a read-only command are not deletion flags. In
    // particular, the `r` in `find -printf` must not turn a redirect into a
    // recursive deletion.
    let recursive = triggered && tokens.iter().any(|t| t.is_recursive_flag());

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

/// Find actions run a separate argv. Search roots are deletion targets only
/// for -delete or a destructive action, not for read-only inspection.
fn assess_find(tokens: &[Token], ctx: &RiskContext, findings: &mut Vec<RiskFinding>) {
    let roots: Vec<_> = tokens
        .iter()
        .skip(1)
        .skip_while(|t| matches!(t.text.as_str(), "-H" | "-L" | "-P" | "--"))
        .take_while(|t| !t.is_flag() && !t.is_operator && !matches!(t.text.as_str(), "!" | "("))
        .filter(|t| !t.is_truncating_redirect_target)
        .collect();
    let classify_roots = |findings: &mut Vec<RiskFinding>| {
        for raw in roots
            .iter()
            .map(|t| t.text.as_str())
            .chain(roots.is_empty().then_some("."))
        {
            let expanded = paths::expand(raw, ctx);
            if let Some(finding) = paths::classify_target(&expanded, raw, true, ctx) {
                findings.push(finding);
            }
        }
    };
    let mut index = 1;
    while index < tokens.len() {
        match tokens[index].text.as_str() {
            "-delete" => classify_roots(findings),
            "-fprint" | "-fprint0" | "-fprintf" | "-fls" => {
                if let Some(target) = tokens.get(index + 1) {
                    if !is_safe_redirect_sink(&target.text) {
                        let expanded = paths::expand(&target.text, ctx);
                        if let Some(finding) =
                            paths::classify_target(&expanded, &target.text, false, ctx)
                        {
                            findings.push(finding);
                        }
                    }
                } else {
                    findings.push(RiskFinding {
                        level: RiskLevel::Confirm,
                        reason: "`find` output target cannot be identified statically".into(),
                        target: None,
                    });
                }
                index += if tokens[index].text == "-fprintf" {
                    2
                } else {
                    1
                };
            }
            // These predicates consume literal data, not another action.
            "-name" | "-iname" | "-path" | "-ipath" | "-wholename" | "-iwholename" | "-regex"
            | "-iregex" | "-lname" | "-ilname" | "-printf" => index += 1,
            "-exec" | "-execdir" | "-ok" | "-okdir" => {
                let start = index + 1;
                let end = tokens[start..]
                    .iter()
                    .position(|t| matches!(t.text.as_str(), ";" | "+"))
                    .map(|offset| start + offset)
                    .unwrap_or(tokens.len());
                let payload = &tokens[start..end];
                if !read_only_find_payload(payload) || end == tokens.len() {
                    let mut nested = Vec::new();
                    assess_segment(payload, ctx, &mut nested);
                    if payload.iter().any(|t| t.text.contains("{}")) {
                        for root in roots
                            .iter()
                            .map(|t| t.text.as_str())
                            .chain(roots.is_empty().then_some("."))
                        {
                            let substituted: Vec<_> = payload
                                .iter()
                                .map(|token| {
                                    let mut token = token.clone();
                                    token.text = token.text.replace("{}", root);
                                    token
                                })
                                .collect();
                            assess_segment(&substituted, ctx, &mut nested);
                        }
                    }
                    findings.extend(nested);
                    findings.push(RiskFinding {
                        level: RiskLevel::Confirm,
                        reason: "`find` action may modify files and its full effects cannot be determined statically".into(),
                        target: None,
                    });
                }
                index = end;
            }
            _ => {}
        }
        index += 1;
    }
}

fn read_only_find_payload(tokens: &[Token]) -> bool {
    let Some(program) = tokens.first() else {
        return false;
    };
    match program.basename().as_str() {
        "cat" | "readlink" => true,
        // Sed can execute commands or write files even without -i. Recognize
        // only simple print scripts rather than guessing about arbitrary code.
        "sed" => {
            let args: Vec<_> = tokens.iter().skip(1).map(|t| t.text.as_str()).collect();
            let script_index = if args.first() == Some(&"-n") { 1 } else { 0 };
            let Some(script) = args.get(script_index) else {
                return false;
            };
            script.ends_with('p')
                && script[..script.len() - 1]
                    .chars()
                    .all(|c| c.is_ascii_digit() || matches!(c, ',' | '$'))
                && args[script_index + 1..]
                    .iter()
                    .all(|arg| !arg.starts_with('-'))
        }
        _ => false,
    }
}

/// Conventional bit buckets are safe *redirect* destinations. They remain
/// protected when explicitly passed to a destructive command such as `rm`.
fn is_safe_redirect_sink(raw: &str) -> bool {
    matches!(raw, "/dev/null" | "/dev/stdout" | "/dev/stderr" | "NUL")
}

#[cfg(test)]
#[path = "assess_tests.rs"]
mod assess_tests;
