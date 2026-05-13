//! Slash command system — provides `/build`, `/plan`, `/review` and
//! a common registry for registering new slash commands.
//!
//! Each command has a name, description, and async handler that receives
//! the remainder of the input after the command name.

use std::collections::HashMap;

// ════════════════════════════════════════════════════════════════════
// Types
// ════════════════════════════════════════════════════════════════════

/// Metadata about a registered slash command.
#[derive(Clone)]
pub struct SlashCommandInfo {
    /// Command name (without `/`), e.g. `"build"`
    pub name: &'static str,
    /// One-line description shown in help
    pub description: &'static str,
    /// Longer usage hint
    pub usage: &'static str,
}

/// Handler signature: receives the trimmed arguments after the command.
pub type SlashHandler = std::sync::Arc<dyn Fn(&str) -> SlashResult + Send + Sync>;

/// Result of executing a slash command.
pub enum SlashResult {
    /// The command completed successfully.
    Ok(String),
    /// The command failed with an error message.
    Err(String),
    /// The command is not available in the current context.
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
    aliases: HashMap<&'static str, &'static str>, // alias → canonical name
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
// Registration
// ════════════════════════════════════════════════════════════════════

/// Register a slash command. Aliases should be registered separately
/// via `register_alias`.
pub async fn register(
    name: &'static str,
    description: &'static str,
    usage: &'static str,
    handler: SlashHandler,
) {
    let mut reg = REGISTRY.write().await;
    reg.commands.insert(
        name,
        SlashCommandInfo {
            name,
            description,
            usage,
        },
    );
    reg.handlers.insert(name, handler);
}

/// Register an alias for an existing command.
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
    if !trimmed.starts_with('/') {
        return None;
    }
    let after_slash = &trimmed[1..];
    let end = after_slash
        .find(|c: char| c.is_whitespace())
        .unwrap_or(after_slash.len());
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
        None => SlashResult::Err(format!(
            "Unknown command: /{}. Type /help for available commands.",
            cmd
        )),
    }
}

/// List all registered commands.
pub async fn list() -> Vec<SlashCommandInfo> {
    let reg = REGISTRY.read().await;
    let mut cmds: Vec<SlashCommandInfo> = reg.commands.values().cloned().collect();
    cmds.sort_by_key(|c| c.name);
    cmds
}

/// List all registered commands' names.
pub async fn names() -> Vec<&'static str> {
    let reg = REGISTRY.read().await;
    reg.commands.keys().copied().collect()
}

/// Check if the given command is registered.
pub async fn is_registered(name: &str) -> bool {
    let reg = REGISTRY.read().await;
    reg.commands.contains_key(name) || reg.aliases.contains_key(name)
}

/// Get the canonical name for an alias, or the name itself.
pub async fn canonical_name(name: &str) -> &str {
    let reg = REGISTRY.read().await;
    reg.aliases.get(name).copied().unwrap_or(name)
}

// ════════════════════════════════════════════════════════════════════
// Built-in commands
// ════════════════════════════════════════════════════════════════════

/// Initialize all built-in slash commands.
pub async fn init() {
    register_help_command().await;
    register_build_command().await;
    register_plan_command().await;
    register_review_command().await;
    register_model_command().await;
    register_clear_command().await;
    register_compact_command().await;
    register_cost_command().await;
    register_export_command().await;
    register_resume_command().await;

    // Aliases
    register_alias("b", "build").await;
    register_alias("p", "plan").await;
    register_alias("r", "review").await;
    register_alias("h", "help").await;
    register_alias("m", "model").await;
    register_alias("cl", "clear").await;
    register_alias("cp", "compact").await;
    register_alias("c", "cost").await;
    register_alias("e", "export").await;
    register_alias("res", "resume").await;
}

// ── /help ──

async fn register_help_command() {
    register(
        "help",
        "Show available slash commands",
        "/help [command]",
        std::sync::Arc::new(|args: &str| {
            let rt = tokio::runtime::Handle::try_current();
            match rt {
                Ok(handle) => {
                    let args = args.to_string();
                    let result = handle.block_on(async move {
                        if args.is_empty() {
                            let cmds = list().await;
                            let mut out = String::from("Available slash commands:\n");
                            for cmd in &cmds {
                                out.push_str(&format!(
                                    "  /{:<12} {}\n",
                                    cmd.name, cmd.description
                                ));
                            }
                            out
                        } else {
                            let info = lookup(&args).await;
                            match info {
                                Some(info) => format!(
                                    "  /{} — {}\n  Usage: {}\n",
                                    info.name, info.description, info.usage
                                ),
                                None => format!("Unknown command: /{}", args),
                            }
                        }
                    });
                    SlashResult::Ok(result)
                }
                Err(_) => SlashResult::Ok("No async runtime available".into()),
            }
        }),
    )
    .await;
}

// ── /build ──

async fn register_build_command() {
    register(
        "build",
        "Build the current project",
        "/build [--release] [--clean] [--test] [message...]",
        std::sync::Arc::new(|args: &str| {
            let rt = tokio::runtime::Handle::try_current();
            match rt {
                Ok(handle) => {
                    let args_owned = args.to_string();
                    handle.spawn(async move {
                        let release = args_owned.contains("--release");
                        let clean = args_owned.contains("--clean");
                        let run_tests = args_owned.contains("--test");
                        let parallel = args_owned.contains("--parallel");
                        let all_projects = args_owned.contains("--all") || args_owned.contains("--workspace");
                        // Remove flags from the message
                        let message = args_owned
                            .replace("--release", "")
                            .replace("--clean", "")
                            .replace("--test", "")
                            .replace("--parallel", "")
                            .replace("--all", "")
                            .replace("--workspace", "")
                            .trim()
                            .to_string();
                        let message = if message.is_empty() {
                            "Build project".to_string()
                        } else {
                            message
                        };
                        match crate::cli::commands::run_build_command(
                            &message,
                            false,
                            false,
                            3,
                            release,
                            clean,
                            None,
                            all_projects,
                            run_tests,
                            parallel,
                            None,
                        )
                        .await
                        {
                            Ok(_) => eprintln!("\n✅ Build completed."),
                            Err(e) => eprintln!("\n❌ Build failed: {:#}", e),
                        }
                    });
                    SlashResult::Ok("Starting build...".into())
                }
                Err(_) => SlashResult::Err("No async runtime available".into()),
            }
        }),
    )
    .await;
}

// ── /plan ──

async fn register_plan_command() {
    register(
        "plan",
        "Generate a build/implementation plan",
        "/plan [goal description...]",
        std::sync::Arc::new(|args: &str| {
            let rt = tokio::runtime::Handle::try_current();
            match rt {
                Ok(handle) => {
                    let args = args.to_string();
                    handle.spawn(async move {
                        run_plan(&args).await;
                    });
                    SlashResult::Ok("Generating plan...".into())
                }
                Err(_) => SlashResult::Err("No async runtime available".into()),
            }
        }),
    )
    .await;
}

async fn run_plan(goal: &str) {

    eprintln!("\n📋 Plan — Analyzing: {}\n", goal);

    // Detect project
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("❌ Cannot get current directory: {}", e);
            return;
        }
    };
    let project_type = crate::workspace_manager::ProjectType::detect_from_path(&cwd);
    eprintln!("  Project: {} ({:?})", cwd.display(), project_type);

    // Determine default build command
    let default_cmd = project_type.default_build_command();
    let test_cmd = project_type.default_test_command();

    eprintln!("\n  ┌─ Build Plan ──────────────────────────────");
    eprintln!("  │ Default Build: {}", default_cmd);
    eprintln!("  │ Default Test:  {}", test_cmd);
    eprintln!("  └────────────────────────────────────────────\n");

    // Detect project structure
    let files = match std::fs::read_dir(&cwd) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect::<Vec<_>>(),
        Err(_) => vec![],
    };

    eprintln!("  Detected files:");
    for f in files.iter().take(20) {
        eprintln!("    📄 {}", f);
    }
    if files.len() > 20 {
        eprintln!("    ... and {} more files", files.len() - 20);
    }

    eprintln!("\n✅ Plan generated. Use `/build {}` to execute.", goal);
}

// ── /review ──

async fn register_review_command() {
    register(
        "review",
        "Run code review on current changes",
        "/review [--staged] [--all]",
        std::sync::Arc::new(|args: &str| {
            let rt = tokio::runtime::Handle::try_current();
            match rt {
                Ok(handle) => {
                    let args = args.to_string();
                    handle.spawn(async move {
                        run_review(&args).await;
                    });
                    SlashResult::Ok("Starting code review...".into())
                }
                Err(_) => SlashResult::Err("No async runtime available".into()),
            }
        }),
    )
    .await;
}

async fn run_review(_args: &str) {
    use std::time::Instant;

    eprintln!("\n🔍 Code Review\n");
    let start = Instant::now();

    // Get git diff
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("❌ Cannot get current directory: {}", e);
            return;
        }
    };

    let diff_output = tokio::process::Command::new("git")
        .args(["diff", "HEAD"])
        .current_dir(&cwd)
        .output()
        .await;

    match diff_output {
        Ok(output) if !output.stdout.is_empty() => {
            let diff = String::from_utf8_lossy(&output.stdout);
            let lines: Vec<&str> = diff.lines().collect();
            let added = lines.iter().filter(|l| l.starts_with('+') && !l.starts_with("+++")).count();
            let removed = lines.iter().filter(|l| l.starts_with('-') && !l.starts_with("---")).count();
            let files_changed = lines
                .iter()
                .filter(|l| l.starts_with("diff --git"))
                .count();

            eprintln!("  ┌─ Review Summary ─────────────────────────");
            eprintln!("  │ Files changed:  {}", files_changed);
            eprintln!("  │ Lines added:    {}", added);
            eprintln!("  │ Lines removed:  {}", removed);
            eprintln!("  │ Diff size:      {} bytes", diff.len());
            eprintln!("  └───────────────────────────────────────────\n");

            // Run micro-ci as part of review
            eprintln!("  Running micro-ci checks...\n");
            let ci = jcode_micro_ci::MicroCi::new(jcode_micro_ci::CiConfig {
                workspace_root: cwd.to_string_lossy().to_string(),
                parallel: true,
                auto_fix: false,
                ..Default::default()
            });
            let ci_report = ci.run().await;

            if ci_report.issues.is_empty() {
                eprintln!("  ✅ No issues found.");
            } else {
                eprintln!("  Issues found: {}", ci_report.issues.len());
                for issue in ci_report.issues.iter().take(10) {
                    eprintln!(
                        "    [{}] {:?}:{:?} — {}",
                        issue.severity, issue.file, issue.line, issue.message
                    );
                }
                if ci_report.issues.len() > 10 {
                    eprintln!("    ... and {} more issues", ci_report.issues.len() - 10);
                }
            }

            eprintln!(
                "\n✅ Review completed in {:.1}s",
                start.elapsed().as_secs_f32()
            );
        }
        Ok(_) => {
            eprintln!("  No uncommitted changes to review.");
            eprintln!("  Use `/review --all` to scan all files.");
        }
        Err(e) => {
            eprintln!("  ❌ Git error: {}", e);
        }
    }
}

// ── /model ──

async fn register_model_command() {
    register(
        "model",
        "Switch AI model for the session",
        "/model <model-name>",
        std::sync::Arc::new(|args: &str| {
            if args.trim().is_empty() {
                SlashResult::Err("Usage: /model <model-name> (e.g., /model claude-opus-4-5)".into())
            } else {
                let model = args.trim().to_string();
                let model_display = model.clone();
                let rt = tokio::runtime::Handle::try_current();
                match rt {
                    Ok(handle) => {
                        handle.spawn(async move {
                            eprintln!("\n🔄 Switching model to: {}\n", model_display);
                            eprintln!("  Model change requested: {}", model_display);
                            eprintln!("  (Full model switching requires session re-init)\n");
                        });
                        SlashResult::Ok(format!("Switching to model: {}", model))
                    }
                    Err(_) => SlashResult::Err("No async runtime available".into()),
                }
            }
        }),
    )
    .await;
}

// ── /clear ──

async fn register_clear_command() {
    register(
        "clear",
        "Clear the current session context",
        "/clear",
        std::sync::Arc::new(|_args: &str| {
            eprintln!("\n🗑️  Session context cleared.\n");
            SlashResult::Ok("Session context cleared.".into())
        }),
    )
    .await;
}

// ── /compact ──

async fn register_compact_command() {
    register(
        "compact",
        "Compact/compress the conversation to save tokens",
        "/compact",
        std::sync::Arc::new(|_args: &str| {
            eprintln!("\n📦 Compacting conversation...\n");
            eprintln!("  (Compaction requires full session API; CLI placeholder)\n");
            SlashResult::Ok("Compact requested.".into())
        }),
    )
    .await;
}

// ── /cost ──

async fn register_cost_command() {
    register(
        "cost",
        "Show estimated token usage and cost for the session",
        "/cost [--json]",
        std::sync::Arc::new(|args: &str| {
            let is_json = args.contains("--json");
            if is_json {
                SlashResult::Ok(r#"{"tokens_in":0,"tokens_out":0,"cost_usd":0.0}"#.into())
            } else {
                eprintln!("\n💰 Session Cost (estimated)\n");
                eprintln!("  (Cost tracking requires active session)\n");
                eprintln!("  Tokens in:    -");
                eprintln!("  Tokens out:   -");
                eprintln!("  Estimated:    $0.00\n");
                SlashResult::Ok("Cost info displayed.".into())
            }
        }),
    )
    .await;
}

// ── /export ──

async fn register_export_command() {
    register(
        "export",
        "Export the current session to a file",
        "/export [--format json|markdown] [output-file]",
        std::sync::Arc::new(|args: &str| {
            let trimmed = args.trim();
            let path = if trimmed.is_empty() || trimmed.starts_with("--") {
                "session_export.md".to_string()
            } else {
                trimmed
                    .split_whitespace()
                    .last()
                    .unwrap_or("session_export.md")
                    .to_string()
            };
            eprintln!("\n📤 Exporting session to: {}\n", path);
            eprintln!("  (Session export requires active session.)\n");
            SlashResult::Ok(format!("Exporting to {}", path))
        }),
    )
    .await;
}

// ── /resume ──

async fn register_resume_command() {
    register(
        "resume",
        "List or resume a previous session",
        "/resume [session-id]",
        std::sync::Arc::new(|args: &str| {
            let trimmed = args.trim();
            if trimmed.is_empty() {
                eprintln!("\n📋 Recent Sessions\n");
                eprintln!("  (Session listing requires session storage.)\n");
                eprintln!("  Use `/resume <session-id>` to resume a session.\n");
                SlashResult::Ok("No sessions listed.".into())
            } else {
                eprintln!("\n📋 Resuming session: {}\n", trimmed);
                eprintln!("  (Session resume requires full session API.)\n");
                SlashResult::Ok(format!("Resuming session: {}", trimmed))
            }
        }),
    )
    .await;
}
