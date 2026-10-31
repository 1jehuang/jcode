//! Project-scoped hooks: `[hooks]` in `<project>/.jcode/config.toml`.
//!
//! A project can add hook commands but never remove or replace global ones.
//! Per event, global commands run first, then the project's. Project commands
//! only run when the `hooks` scope of the shared project trust store holds the
//! exact digest of the project's `[hooks]` table, including its timeouts, so
//! any edit needs approval again. Everything else in the project file is
//! ignored.

use crate::project_trust::{self, Scope};
use jcode_config_types::HooksConfig;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

/// How long a resolved project is reused before its file and trust are read
/// again. Edits are never trusted without a new grant, because the digest
/// changes with the file.
const CACHE_TTL: Duration = if cfg!(test) {
    Duration::ZERO
} else {
    Duration::from_secs(2)
};

/// Where a command came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandScope {
    /// The project's own directory (git top level), set for project commands.
    pub project_dir: Option<PathBuf>,
}

impl CommandScope {
    pub fn global() -> Self {
        Self { project_dir: None }
    }

    pub fn is_project(&self) -> bool {
        self.project_dir.is_some()
    }
}

/// A hook command with its provenance and timeout override.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedCommand {
    pub command: String,
    pub scope: CommandScope,
    /// The project's own timeout for this event, when it set one.
    pub timeout_ms: Option<u64>,
}

impl ScopedCommand {
    /// The timeout to run with: global commands use `global_ms`. Project
    /// commands use their own value when set, else `global_ms`, and never
    /// exceed `project_timeout_cap_ms`.
    pub fn effective_timeout_ms(&self, global_ms: u64) -> u64 {
        if !self.scope.is_project() {
            return global_ms;
        }
        let cap = crate::config::config().hooks.project_timeout_cap_ms.max(1);
        self.timeout_ms.unwrap_or(global_ms).clamp(1, cap)
    }
}

/// A trusted project's hooks, ready to merge after the global ones.
#[derive(Debug)]
pub(super) struct ActiveProject {
    pub dir: PathBuf,
    hooks: HooksConfig,
    timeouts: HashMap<String, u64>,
}

type CacheValue = (Instant, Option<Arc<ActiveProject>>);

static CACHE: LazyLock<Mutex<HashMap<String, CacheValue>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static WARNED: LazyLock<Mutex<std::collections::HashSet<String>>> =
    LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));

fn warn_once(key: String, message: &str) {
    let first = WARNED.lock().unwrap_or_else(|e| e.into_inner()).insert(key);
    if first {
        crate::logging::warn(message);
    }
}

fn project_hooks_enabled() -> bool {
    if let Ok(value) = std::env::var("JCODE_PROJECT_HOOKS")
        && matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off"
        )
    {
        return false;
    }
    crate::config::config().hooks.project_hooks
}

/// The `[hooks]` table of a project config file, and its digest. Returns
/// `Ok(None)` when there is no file or no `[hooks]`.
fn read_hooks_table(file: &Path) -> Result<Option<(toml::Table, String)>, String> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read {}: {error}", file.display())),
    };
    let mut table: toml::Table = text
        .parse()
        .map_err(|error| format!("{} is not valid TOML: {error}", file.display()))?;
    let ignored: Vec<String> = table
        .keys()
        .filter(|key| *key != "hooks")
        .cloned()
        .collect();
    if !ignored.is_empty() {
        warn_once(
            format!("ignored:{}:{}", file.display(), ignored.join(",")),
            &format!(
                "{}: only [hooks] is read from a project config; ignoring {}",
                file.display(),
                ignored.join(", ")
            ),
        );
    }
    let Some(toml::Value::Table(hooks)) = table.remove("hooks") else {
        return Ok(None);
    };
    let json: Value = serde_json::to_value(&hooks).map_err(|error| error.to_string())?;
    Ok(Some((hooks, project_trust::digest_json(&json))))
}

/// The digest of the `hooks` scope for the project containing `path`, with
/// the number of hook commands, for `jcode trust`.
pub fn project_hooks_digest(path: &Path) -> Result<Option<(String, Vec<String>)>, String> {
    let dir = project_trust::config_dir(path).map_err(|e| e.to_string())?;
    let file = dir.join(".jcode").join("config.toml");
    let Some((table, digest)) = read_hooks_table(&file)? else {
        return Ok(None);
    };
    let hooks: HooksConfig = toml::Value::Table(table.clone())
        .try_into()
        .map_err(|error| format!("invalid [hooks] in {}: {error}", file.display()))?;
    let mut lines = Vec::new();
    for event in EVENTS {
        for command in super::config_commands(&hooks, event) {
            lines.push(format!(
                "  - {event}: {}",
                project_trust::safe_terminal_text(&command)
            ));
        }
        if let Some(timeout) = table
            .get(&format!("{event}_timeout_ms"))
            .and_then(toml::Value::as_integer)
        {
            lines.push(format!("    {event}_timeout_ms = {timeout}"));
        }
    }
    Ok(Some((digest, lines)))
}

/// What the `hooks` scope would let the project containing `path` run, for
/// `jcode trust`. `Ok(None)` when the project has no `[hooks]`.
pub fn hooks_scope_review(path: &Path) -> anyhow::Result<Option<project_trust::ScopeReview>> {
    let Some((digest, lines)) = project_hooks_digest(path).map_err(anyhow::Error::msg)? else {
        return Ok(None);
    };
    let item_count = lines
        .iter()
        .filter(|line| !line.starts_with("    "))
        .count();
    Ok(Some(project_trust::ScopeReview {
        scope: Scope::Hooks,
        digest,
        lines,
        item_count,
        redacted: false,
    }))
}

/// Every event key a project may register commands for.
pub(super) const EVENTS: &[&str] = &[
    "turn_start",
    "turn_end",
    "session_start",
    "session_end",
    "pre_tool",
    "pre_tool_transform",
    "post_tool",
    "post_tool_feedback",
    "swarm_member_start",
    "swarm_member_end",
    "user_prompt_submit",
    "stop",
];

fn load(cwd: &Path) -> Option<Arc<ActiveProject>> {
    if !project_hooks_enabled() {
        return None;
    }
    let Ok(dir) = project_trust::config_dir(cwd) else {
        return None;
    };
    let file = dir.join(".jcode").join("config.toml");
    let (table, digest) = match read_hooks_table(&file) {
        Ok(Some(found)) => found,
        Ok(None) => return None,
        Err(message) => {
            warn_once(
                format!("bad:{}", file.display()),
                &format!("Project hooks are off for this session: {message}"),
            );
            return None;
        }
    };
    if !project_trust::is_trusted(&dir, Scope::Hooks, &digest) {
        warn_once(
            format!("untrusted:{}:{digest}", dir.display()),
            &format!(
                "Project hooks in {} are not trusted and will not run. Review them with `jcode trust show {}` and approve with `jcode trust grant --scope hooks`.",
                file.display(),
                dir.display()
            ),
        );
        return None;
    }
    let hooks: HooksConfig = match toml::Value::Table(table.clone()).try_into() {
        Ok(hooks) => hooks,
        Err(error) => {
            warn_once(
                format!("invalid:{}:{digest}", file.display()),
                &format!(
                    "Project hooks are off for this session: invalid [hooks] in {}: {error}",
                    file.display()
                ),
            );
            return None;
        }
    };
    let timeouts = EVENTS
        .iter()
        .filter_map(|event| {
            let key = format!("{event}_timeout_ms");
            let value = table.get(&key)?.as_integer()?;
            (value > 0).then(|| ((*event).to_string(), value as u64))
        })
        .collect();
    Some(Arc::new(ActiveProject {
        dir,
        hooks,
        timeouts,
    }))
}

/// The trusted project hooks for sessions running in `cwd`, if any.
pub(super) fn active_project(cwd: Option<&str>) -> Option<Arc<ActiveProject>> {
    let cwd = cwd.filter(|cwd| !cwd.is_empty())?;
    let now = Instant::now();
    {
        let cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, value)) = cache.get(cwd)
            && now.duration_since(*at) < CACHE_TTL
        {
            return value.clone();
        }
    }
    let loaded = load(Path::new(cwd));
    CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(cwd.to_string(), (now, loaded.clone()));
    loaded
}

/// The project's own directory for sessions in `cwd`, trusted or not. Used to
/// export `JCODE_PROJECT_DIR` to every hook.
pub(super) fn project_dir_for(cwd: Option<&str>) -> Option<PathBuf> {
    let cwd = cwd.filter(|cwd| !cwd.is_empty())?;
    match project_trust::config_dir(Path::new(cwd)) {
        Ok(dir) => Some(dir),
        Err(error) => {
            crate::logging::debug(&format!("No project dir for hooks: {error}"));
            None
        }
    }
}

impl ActiveProject {
    /// The project's commands for `event`.
    pub(super) fn commands(&self, event: &str) -> Vec<ScopedCommand> {
        let timeout = self.timeouts.get(event).copied();
        super::config_commands(&self.hooks, event)
            .into_iter()
            .map(|command| ScopedCommand {
                command,
                scope: CommandScope {
                    project_dir: Some(self.dir.clone()),
                },
                timeout_ms: timeout,
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "project_tests.rs"]
mod tests;
