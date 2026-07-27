//! Path classification: the safety-critical core.
//!
//! Correctness here matters more than anywhere else in the crate, because the
//! catastrophic tier is the one protection that cannot be talked around. It is
//! therefore written to be **absolute and simple**: a small set of paths that
//! may never be recursively destroyed, compared after normalization, with no
//! dependence on correctly understanding the surrounding command.

use super::{RiskContext, RiskFinding, RiskLevel};
use std::path::{Component, Path, PathBuf};

/// Credential stores. Protected *recursively*: destroying a single private key
/// inside `~/.ssh` is as damaging as destroying the directory, so containment
/// matters here and exact-match alone would leave an obvious hole.
const PROTECTED_CREDENTIAL_SUBPATHS: &[&str] = &[".ssh", ".gnupg", ".aws", ".kube", ".docker"];

/// Directories whose *wholesale* destruction is unacceptable, but whose
/// individual files are legitimately edited and removed all the time. Matched
/// exactly, so `~/.config` is protected while `~/.config/app/stale.toml` is not.
const PROTECTED_HOME_SUBPATHS: &[&str] = &[
    ".config",
    ".jcode",
    ".claude",
    ".local",
    ".local/share",
    "Documents",
    "Desktop",
];

/// Absolute system paths that must never be recursively destroyed.
const PROTECTED_SYSTEM_PATHS: &[&str] = &[
    "/",
    "/bin",
    "/boot",
    "/dev",
    "/etc",
    "/lib",
    "/lib64",
    "/opt",
    "/proc",
    "/root",
    "/sbin",
    "/srv",
    "/sys",
    "/usr",
    "/var",
    "/Applications",
    "/System",
    "/Library",
    "/Users",
    "/home",
];

/// System paths where the *contents* are as critical as the directory itself,
/// so deleting a single file inside them is also unacceptable.
const SYSTEM_PATHS_PROTECTED_RECURSIVELY: &[&str] = &[
    "/bin", "/boot", "/dev", "/etc", "/lib", "/lib64", "/proc", "/sbin", "/sys", "/usr",
    "/var/lib", "/System", "/Library",
];

/// The set of paths this policy protects, exposed for testing and docs.
pub struct ProtectedPaths;

impl ProtectedPaths {
    pub fn home_subpaths() -> &'static [&'static str] {
        PROTECTED_HOME_SUBPATHS
    }
    pub fn credential_subpaths() -> &'static [&'static str] {
        PROTECTED_CREDENTIAL_SUBPATHS
    }
    pub fn system_paths() -> &'static [&'static str] {
        PROTECTED_SYSTEM_PATHS
    }
    pub fn recursive_system_paths() -> &'static [&'static str] {
        SYSTEM_PATHS_PROTECTED_RECURSIVELY
    }
}

/// Resolve `~`, `$HOME`, and relative paths into something comparable.
///
/// This is lexical only: it never touches the filesystem, so it is safe to call
/// on paths that do not exist and cannot be slowed down by a hostile argument.
pub fn expand(raw: &str, ctx: &RiskContext) -> PathBuf {
    let mut text = raw.to_string();

    if let Some(home) = &ctx.home_dir {
        let home_str = home.to_string_lossy().to_string();
        // Order matters: replace the longer forms first.
        for var in ["${HOME}", "$HOME"] {
            text = text.replace(var, &home_str);
        }
        if text == "~" {
            text = home_str.clone();
        } else if let Some(rest) = text.strip_prefix("~/") {
            text = format!("{home_str}/{rest}");
        }
    }

    let path = PathBuf::from(&text);
    if path.is_absolute() {
        return normalize(&path);
    }
    match &ctx.working_dir {
        Some(cwd) => normalize(&cwd.join(path)),
        None => path,
    }
}

/// Lexically remove `.` and `..` so `/home/u/../..` is seen as `/`.
///
/// Without this, `rm -rf ~/../..` would walk straight past the protected-path
/// check. Note this is intentionally *not* symlink-aware; see the crate docs on
/// defense in depth.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        return PathBuf::from("/");
    }
    out
}

/// Whether destroying this path is categorically unacceptable.
///
/// Exposed separately because this is the single most important predicate in
/// the crate and deserves to be testable in isolation.
pub fn is_catastrophic_target(path: &Path, ctx: &RiskContext) -> bool {
    let path = normalize(path);

    // Exact system roots, plus anything inside the ones whose contents are as
    // unrecoverable as the directory itself (`/etc/passwd`). `/home` and
    // `/Users` are deliberately not recursive: a user's own project lives
    // under them, and the home directory itself is handled below.
    if PROTECTED_SYSTEM_PATHS.iter().any(|p| path == Path::new(p)) {
        return true;
    }
    if SYSTEM_PATHS_PROTECTED_RECURSIVELY
        .iter()
        .any(|p| path.starts_with(p))
    {
        return true;
    }

    let Some(home) = &ctx.home_dir else {
        return false;
    };
    let home = normalize(home);

    // The home directory itself.
    if path == home {
        return true;
    }
    // Credential stores, including anything inside them.
    if PROTECTED_CREDENTIAL_SUBPATHS
        .iter()
        .any(|sub| path.starts_with(home.join(sub)))
    {
        return true;
    }
    // Config and document roots, but not their individual files.
    PROTECTED_HOME_SUBPATHS
        .iter()
        .any(|sub| path == home.join(sub))
}

/// Classify one resolved target, returning a finding when it is notable.
pub fn classify_target(
    expanded: &Path,
    raw: &str,
    recursive: bool,
    ctx: &RiskContext,
) -> Option<RiskFinding> {
    // Glob and variable expansion we did not perform: we cannot know the
    // footprint, so escalate rather than guess.
    if raw.contains('*') || raw.contains('?') {
        // A bare `/*` or `~/*` is catastrophic in effect even though no single
        // resolved path is protected.
        if let Some(parent_of_glob) = expanded.parent()
            && is_catastrophic_target(parent_of_glob, ctx)
            && expanded.file_name().is_some_and(|n| n == "*")
        {
            return Some(RiskFinding {
                level: RiskLevel::Catastrophic,
                reason: "would destroy the entire contents of a protected directory".to_string(),
                target: Some(raw.to_string()),
            });
        }
        return Some(RiskFinding {
            level: RiskLevel::Confirm,
            reason: "target contains a glob, so the exact set of affected files \
                     is not known before execution"
                .to_string(),
            target: Some(raw.to_string()),
        });
    }

    // Check the resolved path first: `$HOME` expands to a protected path and
    // must be denied outright, not merely queried. Only genuinely unresolvable
    // substitutions fall through to the Confirm tier below.
    if is_catastrophic_target(expanded, ctx) {
        return Some(RiskFinding {
            level: RiskLevel::Catastrophic,
            reason: "targets a protected system or home path that must never be \
                     destroyed"
                .to_string(),
            target: Some(expanded.display().to_string()),
        });
    }

    if raw.contains('$') || raw.contains('`') {
        return Some(RiskFinding {
            level: RiskLevel::Confirm,
            reason: "target is computed at runtime (variable or command \
                     substitution), so its value cannot be checked in advance"
                .to_string(),
            target: Some(raw.to_string()),
        });
    }

    // Raw device nodes are never a safe write target.
    if expanded.starts_with("/dev")
        && !expanded.starts_with("/dev/null")
        && !expanded.starts_with("/dev/stdout")
        && !expanded.starts_with("/dev/stderr")
    {
        return Some(RiskFinding {
            level: RiskLevel::Catastrophic,
            reason: "writes directly to a device node, which can destroy a \
                     filesystem or disk"
                .to_string(),
            target: Some(expanded.display().to_string()),
        });
    }

    let inside_cwd = ctx
        .working_dir
        .as_ref()
        .is_some_and(|cwd| expanded.starts_with(normalize(cwd)));

    if inside_cwd {
        // The routine case: an agent tidying up its own workspace.
        return recursive.then(|| RiskFinding {
            level: RiskLevel::Low,
            reason: "recursive delete inside the working directory".to_string(),
            target: Some(expanded.display().to_string()),
        });
    }

    if is_temp_path(expanded) {
        return None;
    }

    Some(RiskFinding {
        level: RiskLevel::Confirm,
        reason: "targets a path outside the working directory".to_string(),
        target: Some(expanded.display().to_string()),
    })
}

/// Temp directories are conventionally disposable, so deleting inside them is
/// not worth a reflection turn.
fn is_temp_path(path: &Path) -> bool {
    ["/tmp", "/var/tmp", "/private/tmp"]
        .iter()
        .any(|prefix| path.starts_with(prefix))
}

#[cfg(test)]
#[path = "paths_tests.rs"]
mod paths_tests;
