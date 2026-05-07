use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use jcode::cli::provider_init::ProviderChoice;
use jcode::id::new_id;
use jcode::message::{Message, StreamEvent, ToolDefinition};
use jcode::provider::{EventStream, Provider};
use jcode::tool::{Registry, ToolContext, ToolExecutionMode};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "jcode-harness")]
#[command(about = "Standalone jcode harness utilities. With no command, starts interactive jcode.")]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize a project for the full jcode-harness experience
    Init(InitArgs),
    /// Create an isolated safe-evaluation profile for first-time local testing
    #[command(name = "safe-eval")]
    SafeEval(SafeEvalArgs),
    /// Run offline onboarding diagnostics without contacting providers
    Doctor(DoctorArgs),
    /// Run the deterministic tool harness smoke test
    Smoke(SmokeArgs),
    /// Run a single goal by delegating to the jcode run path with skill routing
    Run(RunArgs),
    /// Manage embedded and local skills
    Skills(SkillsArgs),
    /// Run Clean Code Guardian quality checks
    #[command(name = "clean-code")]
    CleanCode(CleanCodeArgs),
}

#[derive(Parser)]
struct InitArgs {
    /// Project directory to initialize (defaults to current directory)
    #[arg(long)]
    cwd: Option<String>,
    /// Overwrite existing generated files
    #[arg(long)]
    force: bool,
    /// Non-interactive defaults, leave questions in .jcode/INIT_QUESTIONS.md
    #[arg(long)]
    yes: bool,
    /// Skip creating .jcode/memory_wiki
    #[arg(long)]
    no_memory_wiki: bool,
    /// Emit JSON report
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
struct SafeEvalArgs {
    /// Project directory to configure (defaults to current directory)
    #[arg(long)]
    cwd: Option<String>,
    /// Isolated JCODE_HOME to create (defaults to <cwd>/.jcode/safe-eval/home)
    #[arg(long)]
    home: Option<PathBuf>,
    /// Overwrite existing generated profile files
    #[arg(long)]
    force: bool,
    /// Emit JSON report
    #[arg(long)]
    json: bool,
    /// Print only the shell command needed to activate the profile
    #[arg(long)]
    print_env: bool,
}

#[derive(Parser)]
struct DoctorArgs {
    /// Project directory to inspect (defaults to current directory)
    #[arg(long)]
    cwd: Option<String>,
    /// Emit JSON report
    #[arg(long)]
    json: bool,
}

#[derive(Parser, Clone)]
struct SmokeArgs {
    /// Use an explicit working directory (defaults to a temp folder).
    #[arg(long)]
    cwd: Option<String>,

    /// Include network-backed tools (webfetch/websearch/codesearch).
    #[arg(long)]
    include_network: bool,
}

#[derive(Parser)]
struct SkillsArgs {
    #[command(subcommand)]
    command: SkillsCommand,
}

#[derive(Subcommand)]
enum SkillsCommand {
    List {
        #[arg(long)]
        json: bool,
    },
    Show {
        name: String,
        #[arg(long)]
        json: bool,
    },
    Sync {
        #[arg(long)]
        force: bool,
    },
    Doctor {
        #[arg(long)]
        json: bool,
    },
    /// Preview task-scoped skill selection for a goal without invoking a model
    Match {
        goal: String,
        /// Project directory for resolving repo-local skills
        #[arg(long)]
        cwd: Option<String>,
        /// Automatic skill routing mode
        #[arg(long, default_value = "auto")]
        skills: HarnessSkillMode,
        /// Explicit task-level skill to include before automatic matches
        #[arg(long = "skill")]
        skill: Vec<String>,
        /// Emit JSON report
        #[arg(long)]
        json: bool,
    },
    /// Print the permission-reviewed local LLM wiki MCP bridge contract
    LlmwikiBridge {
        /// Emit JSON contract for automation
        #[arg(long)]
        json: bool,
    },
}

#[derive(Parser)]
struct CleanCodeArgs {
    #[command(subcommand)]
    command: CleanCodeCommand,
}

#[derive(Subcommand)]
enum CleanCodeCommand {
    /// Run the offline Clean Code Guardian quality gate
    Check {
        /// Project directory to use as root
        #[arg(long)]
        cwd: Option<String>,
        /// Files or directories to scan, defaults to cwd
        paths: Vec<PathBuf>,
        /// Emit JSON report
        #[arg(long)]
        json: bool,
        /// Exit non-zero when findings at this severity or higher are present
        #[arg(long, value_enum, default_value = "error")]
        fail_on: HarnessFailOn,
    },
    /// Print the built-in clean-code rule pack YAML
    Rules,
}

#[derive(Clone, ValueEnum)]
enum HarnessFailOn {
    Info,
    Warning,
    Error,
}

impl From<HarnessFailOn> for jcode::clean_code::Severity {
    fn from(value: HarnessFailOn) -> Self {
        match value {
            HarnessFailOn::Info => Self::Info,
            HarnessFailOn::Warning => Self::Warning,
            HarnessFailOn::Error => Self::Error,
        }
    }
}

#[derive(Parser)]
struct RunArgs {
    goal: String,
    #[arg(long)]
    cwd: Option<String>,
    #[arg(long)]
    provider: Option<String>,
    #[arg(long)]
    provider_profile: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long, default_value = "auto")]
    skills: HarnessSkillMode,
    #[arg(long = "skill")]
    skill: Vec<String>,
    #[arg(long)]
    max_turns: Option<usize>,
    #[arg(long, conflicts_with = "ndjson")]
    json: bool,
    #[arg(long, conflicts_with = "json")]
    ndjson: bool,
    #[arg(long)]
    dry_run: bool,
    /// Use a deterministic local provider response instead of network/provider auth.
    /// Intended for CI smoke tests and harness contract validation.
    #[arg(long)]
    mock_response: Option<String>,
}

#[derive(Clone, ValueEnum)]
enum HarnessSkillMode {
    Auto,
    Off,
    Always,
}

impl From<HarnessSkillMode> for jcode::skill_router::SkillMode {
    fn from(value: HarnessSkillMode) -> Self {
        match value {
            HarnessSkillMode::Auto => Self::Auto,
            HarnessSkillMode::Off => Self::Off,
            HarnessSkillMode::Always => Self::Always,
        }
    }
}

struct NoopProvider;

struct MockRunProvider {
    response: String,
}

#[async_trait::async_trait]
impl Provider for NoopProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        anyhow::bail!("Noop provider - tool harness does not invoke models.")
    }

    fn name(&self) -> &str {
        "noop"
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(NoopProvider)
    }
    fn available_models_display(&self) -> Vec<String> {
        vec![]
    }
    async fn prefetch_models(&self) -> Result<()> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl Provider for MockRunProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let events = vec![
            Ok(StreamEvent::TextDelta(self.response.clone())),
            Ok(StreamEvent::TokenUsage {
                input_tokens: Some(1),
                output_tokens: Some(1),
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            }),
            Ok(StreamEvent::MessageEnd {
                stop_reason: Some("stop".to_string()),
            }),
        ];
        Ok(Box::pin(futures::stream::iter(events)))
    }

    fn name(&self) -> &str {
        "harness-mock"
    }

    fn model(&self) -> String {
        "harness-mock-model".to_string()
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            response: self.response.clone(),
        })
    }

    fn available_models_display(&self) -> Vec<String> {
        vec![self.model()]
    }

    async fn prefetch_models(&self) -> Result<()> {
        Ok(())
    }
}

struct ToolCase {
    name: &'static str,
    input: serde_json::Value,
    label: &'static str,
}

#[tokio::main]
async fn main() -> Result<()> {
    jcode::cli::terminal::install_panic_hook();

    let args = Args::parse();
    match args.command {
        None => jcode::run().await,
        Some(Command::Init(args)) => run_init(args),
        Some(Command::SafeEval(args)) => run_safe_eval(args),
        Some(Command::Doctor(args)) => run_doctor(args),
        Some(Command::Smoke(args)) => run_smoke(args).await,
        Some(Command::Run(args)) => run_goal(args).await,
        Some(Command::Skills(args)) => run_skills(args),
        Some(Command::CleanCode(args)) => run_clean_code(args),
    }
}

fn run_init(args: InitArgs) -> Result<()> {
    let root = args
        .cwd
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let report = jcode::project_init::run_project_init(
        &root,
        jcode::project_init::ProjectInitOptions {
            force: args.force,
            yes: args.yes,
            include_memory_wiki: !args.no_memory_wiki,
        },
    )?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "Initialized jcode-harness project at {}",
            report.root.display()
        );
        println!(
            "Detected stack: {}",
            if report.detected_stack.is_empty() {
                "none".into()
            } else {
                report.detected_stack.join(", ")
            }
        );
        println!("Files written: {}", report.files_written.len());
        for path in &report.files_written {
            println!("  wrote {}", path.display());
        }
        if !report.files_skipped.is_empty() {
            println!(
                "Files skipped: {} (use --force to overwrite)",
                report.files_skipped.len()
            );
            for path in &report.files_skipped {
                println!("  skipped {}", path.display());
            }
        }
        println!("Next steps:");
        for step in &report.next_steps {
            println!("  - {}", step);
        }
    }
    Ok(())
}

fn run_safe_eval(args: SafeEvalArgs) -> Result<()> {
    let root = args
        .cwd
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    if !root.is_dir() {
        anyhow::bail!(
            "safe-eval cwd does not exist or is not a directory: {}",
            root.display()
        );
    }

    let profile_dir = root.join(".jcode").join("safe-eval");
    let safe_home = args
        .home
        .clone()
        .unwrap_or_else(|| profile_dir.join("home"));
    let runtime_dir = safe_home.join("runtime");
    let env_file = profile_dir.join("safe-eval.env");
    let ps1_file = profile_dir.join("safe-eval.ps1");
    let guide_file = profile_dir.join("README.md");

    std::fs::create_dir_all(&profile_dir)?;
    std::fs::create_dir_all(&safe_home)?;
    std::fs::create_dir_all(&runtime_dir)?;
    harden_private_dir(&safe_home)?;

    let env_vars = safe_eval_env_vars(&safe_home, &runtime_dir);
    let disabled = safe_eval_disabled_surfaces();
    let env_content = render_posix_safe_eval_env(&env_vars);
    let ps1_content = render_powershell_safe_eval_env(&env_vars);
    let guide_content = render_safe_eval_guide(&root, &safe_home, &env_file, &ps1_file, &disabled);

    let mut files_written = Vec::new();
    let mut files_skipped = Vec::new();
    write_profile_file(
        &env_file,
        &env_content,
        args.force,
        &mut files_written,
        &mut files_skipped,
    )?;
    write_profile_file(
        &ps1_file,
        &ps1_content,
        args.force,
        &mut files_written,
        &mut files_skipped,
    )?;
    write_profile_file(
        &guide_file,
        &guide_content,
        args.force,
        &mut files_written,
        &mut files_skipped,
    )?;

    let source_command = format!("source {}", shell_quote(&env_file.display().to_string()));
    let powershell_command = format!(". {}", powershell_quote(&ps1_file.display().to_string()));

    if args.print_env {
        println!("{source_command}");
        return Ok(());
    }

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "profile": "safe-eval",
                "root": root,
                "jcode_home": safe_home,
                "runtime_dir": runtime_dir,
                "env_file": env_file,
                "powershell_env_file": ps1_file,
                "guide_file": guide_file,
                "source_command": source_command,
                "powershell_command": powershell_command,
                "env": env_vars
                    .iter()
                    .map(|(name, value)| json!({ "name": name, "value": value }))
                    .collect::<Vec<_>>(),
                "disabled_surfaces": disabled,
                "files_written": files_written,
                "files_skipped": files_skipped,
            }))?
        );
        return Ok(());
    }

    println!(
        "Created jcode-harness safe-eval profile at {}",
        profile_dir.display()
    );
    println!("Isolated JCODE_HOME: {}", safe_home.display());
    println!("Files written: {}", files_written.len());
    for path in &files_written {
        println!("  wrote {}", path.display());
    }
    if !files_skipped.is_empty() {
        println!(
            "Files skipped: {} (use --force to overwrite)",
            files_skipped.len()
        );
        for path in &files_skipped {
            println!("  skipped {}", path.display());
        }
    }
    println!("\nActivate on POSIX shells:");
    println!("  {source_command}");
    println!("Activate on PowerShell:");
    println!("  {powershell_command}");
    println!("\nSmoke test without provider credentials:");
    println!("  jcode-harness run \"say hello\" --json --mock-response \"safe eval ok\"");
    println!(
        "\nSee {} for the trust boundary checklist.",
        guide_file.display()
    );
    Ok(())
}

fn safe_eval_env_vars(
    home: &std::path::Path,
    runtime_dir: &std::path::Path,
) -> Vec<(&'static str, String)> {
    vec![
        ("JCODE_HOME", home.display().to_string()),
        ("JCODE_RUNTIME_DIR", runtime_dir.display().to_string()),
        ("JCODE_SAFE_EVAL", "1".to_string()),
        ("JCODE_NO_TELEMETRY", "1".to_string()),
        ("DO_NOT_TRACK", "1".to_string()),
        ("JCODE_AMBIENT_ENABLED", "false".to_string()),
        ("JCODE_AMBIENT_PROACTIVE", "false".to_string()),
        ("JCODE_SWARM_ENABLED", "false".to_string()),
        ("JCODE_MEMORY_ENABLED", "false".to_string()),
        ("JCODE_MEMORY_BACKEND", "off".to_string()),
        ("JCODE_AUTOREVIEW_ENABLED", "false".to_string()),
        ("JCODE_AUTOJUDGE_ENABLED", "false".to_string()),
        ("JCODE_GATEWAY_ENABLED", "false".to_string()),
        ("JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES", String::new()),
    ]
}

fn safe_eval_disabled_surfaces() -> Vec<&'static str> {
    vec![
        "telemetry",
        "ambient autonomous cycles",
        "proactive ambient work",
        "swarm auto-coordination",
        "persistent semantic memory",
        "autoreview",
        "autojudge",
        "web/iOS gateway",
        "external credential auto-trust",
    ]
}

fn write_profile_file(
    path: &std::path::Path,
    content: &str,
    force: bool,
    files_written: &mut Vec<PathBuf>,
    files_skipped: &mut Vec<PathBuf>,
) -> Result<()> {
    if path.exists() && !force {
        files_skipped.push(path.to_path_buf());
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    files_written.push(path.to_path_buf());
    Ok(())
}

fn render_posix_safe_eval_env(env_vars: &[(&'static str, String)]) -> String {
    let mut out =
        String::from("# Source this file to activate the jcode-harness safe-eval profile.\n");
    out.push_str("# It intentionally avoids importing existing credentials or long-lived state.\n");
    for (name, value) in env_vars {
        out.push_str(&format!("export {name}={}\n", shell_quote(value)));
    }
    out
}

fn render_powershell_safe_eval_env(env_vars: &[(&'static str, String)]) -> String {
    let mut out =
        String::from("# Dot-source this file to activate the jcode-harness safe-eval profile.\n");
    out.push_str("# It intentionally avoids importing existing credentials or long-lived state.\n");
    for (name, value) in env_vars {
        out.push_str(&format!("$env:{name} = {}\n", powershell_quote(value)));
    }
    out
}

fn render_safe_eval_guide(
    root: &std::path::Path,
    safe_home: &std::path::Path,
    env_file: &std::path::Path,
    ps1_file: &std::path::Path,
    disabled: &[&str],
) -> String {
    let disabled_list = disabled
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        r#"# jcode-harness Safe Evaluation Profile

This profile is for first-time evaluation in a disposable or low-risk project checkout.

## Scope

- Project root: `{}`
- Isolated `JCODE_HOME`: `{}`
- POSIX activation file: `{}`
- PowerShell activation file: `{}`

## Activate

POSIX shells:

```bash
source {}
```

PowerShell:

```powershell
. {}
```

## What this disables or isolates

{}

The profile also points runtime files at the isolated home, so provider credentials, sessions, logs, memory, and transient sockets do not mix with the user's normal `~/.jcode` state.

## Suggested smoke tests

```bash
jcode-harness run "say hello" --json --mock-response "safe eval ok"
jcode-harness skills doctor --json
jcode-harness smoke
```

## Trust checklist before leaving safe-eval

1. Review any project-local `.jcode/mcp.json` or `.claude/mcp.json` before starting MCP servers.
2. Avoid importing credentials from Claude, Codex, Gemini, Copilot, browsers, Gmail, or other tools until you understand the trust boundary.
3. Prefer disposable repos, worktrees, containers, or VMs for unknown projects.
4. Do not enable ambient/autonomous/self-dev workflows until the basic smoke tests pass.
5. Keep secrets out of prompts, transcripts, wiki pages, side panels, and generated skills.
"#,
        root.display(),
        safe_home.display(),
        env_file.display(),
        ps1_file.display(),
        env_file.display(),
        ps1_file.display(),
        disabled_list
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(unix)]
fn harden_private_dir(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn harden_private_dir(_path: &std::path::Path) -> Result<()> {
    Ok(())
}

fn run_doctor(args: DoctorArgs) -> Result<()> {
    let root = args
        .cwd
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    if !root.is_dir() {
        anyhow::bail!(
            "doctor cwd does not exist or is not a directory: {}",
            root.display()
        );
    }

    let jcode_home = jcode::storage::jcode_dir()?;
    let jcode_home_source = if std::env::var("JCODE_HOME").is_ok() {
        "env"
    } else {
        "default"
    };
    let safe_eval = build_safe_eval_doctor(&root, &jcode_home);
    let privacy = build_privacy_doctor();
    let features = build_feature_env_doctor();
    let skills = build_skill_doctor_summary(&root);
    let mcp_configs = build_mcp_doctor_configs(&root, &jcode_home);
    let mut recommendations = Vec::new();

    if !safe_eval["active"].as_bool().unwrap_or(false) {
        recommendations.push(
            "Run `jcode-harness safe-eval` before first evaluation or unknown repos.".to_string(),
        );
    }
    if !privacy["telemetry_opted_out"].as_bool().unwrap_or(false) {
        recommendations.push(
            "Set `JCODE_NO_TELEMETRY=1` or `DO_NOT_TRACK=1` for sensitive evaluations.".to_string(),
        );
    }
    if mcp_configs
        .iter()
        .any(|config| config["exists"].as_bool().unwrap_or(false))
    {
        recommendations.push(
            "Review project-local MCP configs before allowing any server command to start."
                .to_string(),
        );
    }
    if std::env::consts::OS == "windows" {
        recommendations.push("Windows support is still a high-risk path; prefer WSL2 or run `jcode-harness safe-eval` first.".to_string());
    }
    if skills["status"] != "ok" {
        recommendations.push(
            "Run `jcode-harness skills doctor` and fix malformed local skill frontmatter."
                .to_string(),
        );
    }

    let status = if recommendations.is_empty() {
        "ok"
    } else {
        "warn"
    };
    let report = json!({
        "status": status,
        "offline": true,
        "root": root,
        "platform": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        },
        "jcode_home": {
            "path": jcode_home,
            "source": jcode_home_source,
            "exists": jcode_home.exists(),
        },
        "safe_eval": safe_eval,
        "privacy": privacy,
        "features": features,
        "skills": skills,
        "mcp": {
            "configs": mcp_configs,
        },
        "recommendations": recommendations,
    });

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    println!("jcode-harness doctor: {status}");
    println!("Offline diagnostics: true");
    println!("Root: {}", report["root"].as_str().unwrap_or("<unknown>"));
    println!(
        "Platform: {}/{}",
        report["platform"]["os"].as_str().unwrap_or("unknown"),
        report["platform"]["arch"].as_str().unwrap_or("unknown")
    );
    println!(
        "JCODE_HOME: {} ({})",
        report["jcode_home"]["path"].as_str().unwrap_or("<unknown>"),
        report["jcode_home"]["source"].as_str().unwrap_or("unknown")
    );
    println!(
        "Safe eval active: {}",
        report["safe_eval"]["active"].as_bool().unwrap_or(false)
    );
    println!(
        "Telemetry opted out: {}",
        report["privacy"]["telemetry_opted_out"]
            .as_bool()
            .unwrap_or(false)
    );
    println!(
        "Skills: {} loaded, built-ins {} ({})",
        report["skills"]["loaded"].as_u64().unwrap_or(0),
        report["skills"]["builtins"].as_u64().unwrap_or(0),
        report["skills"]["status"].as_str().unwrap_or("unknown")
    );
    let mcp_count = report["mcp"]["configs"]
        .as_array()
        .map(|configs| {
            configs
                .iter()
                .filter(|config| config["exists"].as_bool().unwrap_or(false))
                .count()
        })
        .unwrap_or(0);
    println!("MCP configs found: {mcp_count}");
    if let Some(items) = report["recommendations"].as_array()
        && !items.is_empty()
    {
        println!("Recommendations:");
        for item in items {
            println!("  - {}", item.as_str().unwrap_or(""));
        }
    }
    Ok(())
}

fn build_safe_eval_doctor(
    root: &std::path::Path,
    active_home: &std::path::Path,
) -> serde_json::Value {
    let profile_dir = root.join(".jcode").join("safe-eval");
    let expected_home = profile_dir.join("home");
    let env_file = profile_dir.join("safe-eval.env");
    let ps1_file = profile_dir.join("safe-eval.ps1");
    let guide_file = profile_dir.join("README.md");
    let active_marker = std::env::var("JCODE_SAFE_EVAL").ok().as_deref() == Some("1");
    let active_home_matches = paths_equal(active_home, &expected_home);
    json!({
        "active": active_marker || active_home_matches,
        "active_marker": active_marker,
        "active_home_matches_profile": active_home_matches,
        "profile_dir": profile_dir,
        "expected_home": expected_home,
        "files": [
            { "name": "posix_env", "path": env_file, "exists": env_file.exists() },
            { "name": "powershell_env", "path": ps1_file, "exists": ps1_file.exists() },
            { "name": "guide", "path": guide_file, "exists": guide_file.exists() }
        ]
    })
}

fn build_privacy_doctor() -> serde_json::Value {
    let jcode_no_telemetry = std::env::var("JCODE_NO_TELEMETRY").is_ok();
    let do_not_track = std::env::var("DO_NOT_TRACK").is_ok();
    json!({
        "jcode_no_telemetry": jcode_no_telemetry,
        "do_not_track": do_not_track,
        "telemetry_opted_out": jcode_no_telemetry || do_not_track,
    })
}

fn build_feature_env_doctor() -> serde_json::Value {
    json!({
        "ambient_enabled_env": std::env::var("JCODE_AMBIENT_ENABLED").ok(),
        "ambient_proactive_env": std::env::var("JCODE_AMBIENT_PROACTIVE").ok(),
        "swarm_enabled_env": std::env::var("JCODE_SWARM_ENABLED").ok(),
        "memory_enabled_env": std::env::var("JCODE_MEMORY_ENABLED").ok(),
        "memory_backend_env": std::env::var("JCODE_MEMORY_BACKEND").ok(),
        "autoreview_enabled_env": std::env::var("JCODE_AUTOREVIEW_ENABLED").ok(),
        "autojudge_enabled_env": std::env::var("JCODE_AUTOJUDGE_ENABLED").ok(),
        "gateway_enabled_env": std::env::var("JCODE_GATEWAY_ENABLED").ok(),
    })
}

fn build_skill_doctor_summary(root: &std::path::Path) -> serde_json::Value {
    match jcode::skill::SkillRegistry::load_for_working_dir(Some(root)) {
        Ok(registry) => json!({
            "status": "ok",
            "builtins": jcode::skill_pack::builtin_skills().len(),
            "loaded": registry.list().len(),
        }),
        Err(err) => json!({
            "status": "error",
            "builtins": jcode::skill_pack::builtin_skills().len(),
            "loaded": 0,
            "error": err.to_string(),
        }),
    }
}

fn build_mcp_doctor_configs(
    root: &std::path::Path,
    jcode_home: &std::path::Path,
) -> Vec<serde_json::Value> {
    [
        ("project-jcode", root.join(".jcode").join("mcp.json")),
        ("project-claude", root.join(".claude").join("mcp.json")),
        ("global-jcode", jcode_home.join("mcp.json")),
    ]
    .into_iter()
    .map(|(scope, path)| {
        let project_local = scope.starts_with("project-");
        let exists = path.exists();
        json!({
            "scope": scope,
            "path": path,
            "exists": exists,
            "requires_review": exists && project_local,
        })
    })
    .collect()
}

fn paths_equal(left: &std::path::Path, right: &std::path::Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

async fn run_goal(args: RunArgs) -> Result<()> {
    if let Some(cwd) = &args.cwd {
        std::env::set_current_dir(cwd)?;
    }
    if let Some(profile_name) = args
        .provider_profile
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        jcode::provider_catalog::apply_named_provider_profile_env(profile_name)?;
        jcode::env::set_var("JCODE_PROVIDER_PROFILE_NAME", profile_name);
        jcode::env::set_var("JCODE_PROVIDER_PROFILE_ACTIVE", "1");
    }

    let provider_choice = if args.provider_profile.is_some() {
        ProviderChoice::OpenaiCompatible
    } else if let Some(provider) = args.provider.as_deref() {
        ProviderChoice::from_str(provider, true)
            .map_err(|err| anyhow::anyhow!("invalid provider '{}': {}", provider, err))?
    } else {
        ProviderChoice::Auto
    };
    let message =
        jcode::cli::commands::with_auto_skill_preface(&args.goal, &args.skill, args.skills.into());
    if args.dry_run {
        println!("{}", message);
        return Ok(());
    }

    let provider: Arc<dyn Provider> = if let Some(response) = args.mock_response.clone() {
        Arc::new(MockRunProvider { response })
    } else if args.json || args.ndjson {
        jcode::cli::provider_init::init_provider_quiet(&provider_choice, args.model.as_deref())
            .await?
    } else {
        jcode::cli::provider_init::init_provider_for_validation(
            &provider_choice,
            args.model.as_deref(),
        )
        .await?
    };
    let registry = Registry::new(provider.clone()).await;
    let mut agent = jcode::agent::Agent::new(provider.clone(), registry);

    if args.ndjson {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "type": "start",
                "session_id": agent.session_id(),
                "provider": provider.name(),
                "model": provider.model(),
            }))?
        );
    }

    let max_turns = args.max_turns.unwrap_or(1).max(1);
    let mut text = String::new();
    for turn in 0..max_turns {
        let prompt = if turn == 0 {
            message.as_str()
        } else {
            "Continue if needed, otherwise summarize completion."
        };
        if args.json || args.ndjson {
            let output = agent.run_once_capture(prompt).await?;
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(&output);
        } else {
            agent.run_once(prompt).await?;
        }
    }

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "session_id": agent.session_id(),
                "provider": provider.name(),
                "model": provider.model(),
                "text": text,
                "usage": agent.last_usage(),
            }))?
        );
    } else if args.ndjson {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "type": "done",
                "session_id": agent.session_id(),
                "text": text,
                "usage": agent.last_usage(),
            }))?
        );
    }
    Ok(())
}

fn run_skills(args: SkillsArgs) -> Result<()> {
    match args.command {
        SkillsCommand::List { json } => jcode::cli::commands::run_skills_list_command(json),
        SkillsCommand::Show { name, json } => {
            jcode::cli::commands::run_skills_show_command(&name, json)
        }
        SkillsCommand::Sync { force } => jcode::cli::commands::run_skills_sync_command(force),
        SkillsCommand::Doctor { json } => jcode::cli::commands::run_skills_doctor_command(json),
        SkillsCommand::Match {
            goal,
            cwd,
            skills,
            skill,
            json,
        } => run_skills_match(&goal, cwd, skills.into(), &skill, json),
        SkillsCommand::LlmwikiBridge { json } => run_llmwiki_bridge(json),
    }
}

fn llmwiki_bridge_contract() -> serde_json::Value {
    json!({
        "skill": "llmwiki-memory",
        "kind": "local-mcp-bridge-preview",
        "offline": true,
        "network_required": false,
        "permission_boundary": {
            "default": "read-only preview; this command never invokes MCP tools",
            "writes": "wiki_sync may write local raw/session pages only when the operator explicitly invokes it outside this preview",
            "secrets": "do not record credentials, tokens, private keys, or unredacted personal data in wiki pages"
        },
        "commands": [
            {
                "name": "wiki_query",
                "purpose": "Retrieve synthesized project memory, decisions, and prior context by question.",
                "when_to_use": "Before planning or coding when prior decisions may exist.",
                "mcp_tool": "mcp__llmwiki__wiki_query",
                "example": { "question": "what did we decide about embedded skills?", "max_pages": 5 }
            },
            {
                "name": "wiki_search",
                "purpose": "Find literal text across wiki pages and optionally raw session transcripts.",
                "when_to_use": "When exact wording, issue numbers, or command output matters.",
                "mcp_tool": "mcp__llmwiki__wiki_search",
                "example": { "term": "llmwiki-memory", "include_raw": false }
            },
            {
                "name": "wiki_read_page",
                "purpose": "Read one known wiki or raw page by path for provenance.",
                "when_to_use": "After query/search returns a source path that needs verification.",
                "mcp_tool": "mcp__llmwiki__wiki_read_page",
                "example": { "path": "wiki/index.md" }
            },
            {
                "name": "wiki_sync",
                "purpose": "Import new local agent session transcripts into raw/sessions for future wiki use.",
                "when_to_use": "At explicit memory-capture checkpoints after reviewing local write/secret boundaries.",
                "mcp_tool": "mcp__llmwiki__wiki_sync",
                "example": { "dry_run": true },
                "write_risk": "local-files"
            },
            {
                "name": "wiki_export",
                "purpose": "Export a machine-readable wiki index or flattened dump for handoff/context packaging.",
                "when_to_use": "When producing durable handoff context or release evidence.",
                "mcp_tool": "mcp__llmwiki__wiki_export",
                "example": { "format": "llms-txt" }
            },
            {
                "name": "wiki_lint",
                "purpose": "Check wiki graph health, broken wikilinks, stale summaries, and contradictions.",
                "when_to_use": "Before trusting wiki context in a release or long-running agent loop.",
                "mcp_tool": "mcp__llmwiki__wiki_lint",
                "example": {}
            }
        ],
        "recommended_flow": [
            "Run wiki_query with the task question.",
            "Use wiki_search for exact issue numbers or command names.",
            "Read cited pages with wiki_read_page before treating them as evidence.",
            "Use wiki_sync --dry-run first when capturing new local transcripts.",
            "Run wiki_lint before release or handoff if wiki-derived context is relied on."
        ]
    })
}

fn run_llmwiki_bridge(json_output: bool) -> Result<()> {
    let contract = llmwiki_bridge_contract();
    if json_output {
        println!("{}", serde_json::to_string_pretty(&contract)?);
        return Ok(());
    }

    println!(
        "LLM wiki bridge for skill: {}",
        contract["skill"].as_str().unwrap_or("llmwiki-memory")
    );
    println!("Offline preview: true");
    println!("Network required: false");
    println!(
        "Permission boundary: this command only prints the bridge contract; it does not invoke MCP tools.\n"
    );
    println!("Concrete MCP commands:");
    if let Some(commands) = contract["commands"].as_array() {
        for command in commands {
            println!(
                "- {} -> {}: {}",
                command["name"].as_str().unwrap_or("unknown"),
                command["mcp_tool"].as_str().unwrap_or("unknown"),
                command["purpose"].as_str().unwrap_or("")
            );
        }
    }
    println!("\nRecommended flow:");
    if let Some(flow) = contract["recommended_flow"].as_array() {
        for (idx, step) in flow.iter().enumerate() {
            println!("{}. {}", idx + 1, step.as_str().unwrap_or(""));
        }
    }
    Ok(())
}

fn run_skills_match(
    goal: &str,
    cwd: Option<String>,
    mode: jcode::skill_router::SkillMode,
    explicit: &[String],
    json_output: bool,
) -> Result<()> {
    let working_dir = cwd.map(PathBuf::from);
    let registry = jcode::skill::SkillRegistry::load_for_working_dir(working_dir.as_deref())?;
    let selected = jcode::skill_router::select_skills(goal, explicit, mode);

    if json_output {
        let entries = selected
            .iter()
            .map(|name| {
                if let Some(skill) = registry.get(name) {
                    json!({
                        "name": skill.name,
                        "description": skill.description,
                        "origin": skill.origin.label(),
                        "path": skill.path.display().to_string(),
                        "allowed_tools": skill.allowed_tools,
                    })
                } else {
                    json!({
                        "name": name,
                        "missing": true,
                    })
                }
            })
            .collect::<Vec<_>>();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "goal": goal,
                "mode": format!("{:?}", mode).to_ascii_lowercase(),
                "selected": entries,
            }))?
        );
        return Ok(());
    }

    if selected.is_empty() {
        println!("No skills selected for this task.");
        return Ok(());
    }

    println!("Selected skills for task:");
    for name in selected {
        if let Some(skill) = registry.get(&name) {
            println!(
                "- {}\t{}\t{}",
                skill.name,
                skill.origin.label(),
                skill.description
            );
        } else {
            println!("- {name}\tmissing");
        }
    }
    Ok(())
}

fn run_clean_code(args: CleanCodeArgs) -> Result<()> {
    match args.command {
        CleanCodeCommand::Rules => jcode::cli::commands::run_clean_code_rules_command(),
        CleanCodeCommand::Check {
            cwd,
            paths,
            json,
            fail_on,
        } => {
            if let Some(cwd) = cwd {
                std::env::set_current_dir(cwd)?;
            }
            jcode::cli::commands::run_clean_code_check_command(paths, json, fail_on.into())
        }
    }
}

async fn run_smoke(args: SmokeArgs) -> Result<()> {
    let workspace = if let Some(cwd) = args.cwd {
        PathBuf::from(cwd)
    } else {
        create_temp_workspace()?
    };

    std::fs::create_dir_all(&workspace)?;
    std::env::set_current_dir(&workspace)?;
    eprintln!("Harness workspace: {}", workspace.display());

    let provider: Arc<dyn Provider> = Arc::new(NoopProvider);
    let registry = Registry::new(provider).await;

    let session_id = new_id("harness");
    let base_ctx = ToolContext {
        session_id: session_id.clone(),
        message_id: session_id.clone(),
        tool_call_id: String::new(),
        working_dir: Some(workspace.clone()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    };

    let mut cases = vec![
        ToolCase {
            name: "write",
            label: "write sample.txt",
            input: json!({"file_path": "sample.txt", "content": "alpha\nbeta\n"}),
        },
        ToolCase {
            name: "read",
            label: "read sample.txt",
            input: json!({"file_path": "sample.txt"}),
        },
        ToolCase {
            name: "edit",
            label: "edit sample.txt (alpha -> alpha1)",
            input: json!({"file_path": "sample.txt", "old_string": "alpha", "new_string": "alpha1"}),
        },
        ToolCase {
            name: "multiedit",
            label: "multiedit sample.txt",
            input: json!({"file_path": "sample.txt", "edits": [{"old_string": "alpha1", "new_string": "alpha2"}, {"old_string": "beta", "new_string": "beta1"}]}),
        },
        ToolCase {
            name: "patch",
            label: "patch sample.txt",
            input: json!({"patch_text": "--- a/sample.txt\n+++ b/sample.txt\n@@ -1,2 +1,3 @@\n alpha2\n beta1\n+gamma\n"}),
        },
        ToolCase {
            name: "apply_patch",
            label: "apply_patch add file",
            input: json!({"patch_text": "*** Begin Patch\n*** Add File: added.txt\n+added\n*** End Patch\n"}),
        },
        ToolCase {
            name: "ls",
            label: "ls .",
            input: json!({"path": "."}),
        },
        ToolCase {
            name: "glob",
            label: "glob *.txt",
            input: json!({"pattern": "*.txt"}),
        },
        ToolCase {
            name: "grep",
            label: "grep gamma",
            input: json!({"pattern": "gamma", "path": "."}),
        },
        ToolCase {
            name: "bash",
            label: "bash pwd",
            input: json!({"command": "pwd"}),
        },
        ToolCase {
            name: "invalid",
            label: "invalid tool call",
            input: json!({"tool": "unknown", "error": "missing required field"}),
        },
        ToolCase {
            name: "todo",
            label: "todo write",
            input: json!({"todos": [{"content": "harness task", "status": "pending", "priority": "low", "id": "1"}]}),
        },
        ToolCase {
            name: "todo",
            label: "todo read",
            input: json!({}),
        },
        ToolCase {
            name: "batch",
            label: "batch ls + read",
            input: json!({"tool_calls": [{"tool": "ls", "parameters": {"path": "."}}, {"tool": "read", "parameters": {"file_path": "sample.txt"}}]}),
        },
    ];

    if args.include_network {
        cases.push(ToolCase {
            name: "webfetch",
            label: "webfetch example.com",
            input: json!({"url": "https://example.com", "format": "text"}),
        });
        cases.push(ToolCase {
            name: "websearch",
            label: "websearch rust async",
            input: json!({"query": "rust async await"}),
        });
        cases.push(ToolCase {
            name: "codesearch",
            label: "codesearch tokio spawn",
            input: json!({"query": "tokio::spawn"}),
        });
    }

    for (idx, case) in cases.iter().enumerate() {
        let ctx = ToolContext {
            tool_call_id: format!("harness-{}", idx + 1),
            ..base_ctx.clone()
        };
        println!("\n== {} ({}) ==", case.name, case.label);
        match registry.execute(case.name, case.input.clone(), ctx).await {
            Ok(output) => {
                if let Some(title) = output.title {
                    println!("[title] {}", title);
                }
                println!("{}", output.output);
            }
            Err(err) => println!("[error] {}", err),
        }
    }

    Ok(())
}

fn create_temp_workspace() -> Result<PathBuf> {
    let mut path = std::env::temp_dir();
    path.push(format!("jcode-harness-{}", new_id("run")));
    Ok(path)
}
