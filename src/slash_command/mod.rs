//! Slash command system — Registry + 30+ built-in commands.
//!
//! Architecture:
//! - `mod.rs` — Registry, types, public API
//! - `help.rs` — /help
//! - `build.rs` — /build, /plan, /review
//! - `session.rs` — /session, /resume, /export, /fork
//! - `config.rs` — /config, /model, /env
//! - `vcs.rs` — /commit, /rethink, /diff, /status
//! - `tasks.rs` — /tasks, /workflows, /skills
//! - `utils.rs` — /clear, /compact, /cost, /learn, /doctor, /search, /memory, /mcp, /undo

mod help;
mod build;
mod session;
mod config;
mod vcs;
mod tasks;
mod utils;

use std::collections::HashMap;

// ════════════════════════════════════════════════════════════════════
// Types
// ════════════════════════════════════════════════════════════════════

/// Metadata about a registered slash command.
#[derive(Clone)]
pub struct SlashCommandInfo {
    pub name: &'static str,
    pub description: &'static str,
    pub usage: &'static str,
}

/// Handler signature: receives trimmed arguments after the command.
pub type SlashHandler = std::sync::Arc<dyn Fn(&str) -> SlashResult + Send + Sync>;

/// Result of executing a slash command.
pub enum SlashResult {
    Ok(String),
    Err(String),
    Unavailable,
}

// ════════════════════════════════════════════════════════════════════
// Registry
// ════════════════════════════════════════════════════════════════════

static REGISTRY: std::sync::LazyLock<tokio::sync::RwLock<Registry>> =
    std::sync::LazyLock::new(|| tokio::sync::RwLock::new(Registry::new()));

struct Registry {
    commands: HashMap<&'static str, SlashCommandInfo>,
    handlers: HashMap<&'static str, SlashHandler>,
    aliases: HashMap<&'static str, &'static str>,
}

impl Registry {
    fn new() -> Self {
        Self {
            commands: HashMap::new(),
            handlers: HashMap::new(),
            aliases: HashMap::new(),
        }
    }
}

// ════════════════════════════════════════════════════════════════════
// Registration API
// ════════════════════════════════════════════════════════════════════

pub async fn register(name: &'static str, description: &'static str, usage: &'static str, handler: SlashHandler) {
    let mut reg = REGISTRY.write().await;
    reg.commands.insert(name, SlashCommandInfo { name, description, usage });
    reg.handlers.insert(name, handler);
}

pub async fn register_alias(alias: &'static str, target: &'static str) {
    let mut reg = REGISTRY.write().await;
    reg.aliases.insert(alias, target);
}

// ════════════════════════════════════════════════════════════════════
// Lookup & execution
// ════════════════════════════════════════════════════════════════════

/// Parse a full input line into (command_name, args).
pub fn parse(input: &str) -> Option<(&str, &str)> {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') { return None; }
    let after_slash = &trimmed[1..];
    let end = after_slash.find(|c: char| c.is_whitespace()).unwrap_or(after_slash.len());
    let cmd = &after_slash[..end];
    let args = after_slash[end..].trim();
    Some((cmd, args))
}

/// Look up a command's info.
pub async fn lookup(name: &str) -> Option<SlashCommandInfo> {
    let reg = REGISTRY.read().await;
    let canonical = reg.aliases.get(name).unwrap_or(&name);
    reg.commands.get(canonical).cloned()
}

/// Execute a parsed slash command.
pub async fn execute(cmd: &str, args: &str) -> SlashResult {
    let reg = REGISTRY.read().await;
    let canonical = reg.aliases.get(cmd).copied().unwrap_or(cmd);
    match reg.handlers.get(canonical) {
        Some(handler) => handler(args),
        None => SlashResult::Err(format!("Unknown: /{}. Type /help.", cmd)),
    }
}

/// List all registered commands.
pub async fn list() -> Vec<SlashCommandInfo> {
    let reg = REGISTRY.read().await;
    let mut cmds: Vec<SlashCommandInfo> = reg.commands.values().cloned().collect();
    cmds.sort_by_key(|c| c.name);
    cmds
}

pub async fn names() -> Vec<&'static str> {
    let reg = REGISTRY.read().await;
    reg.commands.keys().copied().collect()
}

pub async fn is_registered(name: &str) -> bool {
    let reg = REGISTRY.read().await;
    reg.commands.contains_key(name) || reg.aliases.contains_key(name)
}

// ════════════════════════════════════════════════════════════════════
// Initialize all built-in slash commands (30+)
// ════════════════════════════════════════════════════════════════════

pub async fn init() {
    help::register().await;
    build::register_build().await;
    build::register_plan().await;
    build::register_review().await;
    config::register_model().await;
    config::register_config().await;
    config::register_env().await;
    utils::register_clear().await;
    utils::register_compact().await;
    utils::register_cost().await;
    utils::register_learn().await;
    utils::register_doctor().await;
    utils::register_search().await;
    utils::register_memory().await;
    utils::register_mcp().await;
    utils::register_undo().await;
    session::register_export().await;
    session::register_resume().await;
    session::register_session().await;
    session::register_fork().await;
    tasks::register_tasks().await;
    tasks::register_skills().await;
    tasks::register_workflows().await;
    vcs::register_commit().await;
    vcs::register_rethink().await;
    vcs::register_diff().await;
    vcs::register_status().await;
    vcs::register_push().await;
    vcs::register_pull().await;
    vcs::register_branch().await;
    vcs::register_merge().await;
    vcs::register_log().await;
    utils::register_redo().await;

    // 30+ aliases
    let aliases = [
        ("b", "build"), ("p", "plan"), ("r", "review"),
        ("h", "help"), ("m", "model"), ("cl", "clear"),
        ("cp", "compact"), ("c", "cost"), ("e", "export"),
        ("res", "resume"), ("l", "learn"), ("t", "tasks"),
        ("sk", "skills"), ("wf", "workflows"), ("cfg", "config"),
        ("ss", "session"), ("cm", "commit"), ("rt", "rethink"),
        ("tb", "thinkback"), ("fk", "fork"), ("d", "diff"),
        ("st", "status"), ("dt", "doctor"), ("s", "search"),
        ("mem", "memory"), ("u", "undo"), ("dc", "dictate"),
        ("env", "env"),         ("dbg", "debug"),
        ("ps", "push"), ("pl", "pull"), ("br", "branch"),
        ("mg", "merge"), ("lg", "log"), ("rd", "redo"),
    ];
    for (alias, target) in aliases {
        register_alias(alias, target).await;
    }
}
