use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use jcode::cli::provider_init::ProviderChoice;
use jcode::id::new_id;
use jcode::message::{Message, Role, StreamEvent, ToolDefinition};
use jcode::provider::{EventStream, Provider};
use jcode::tool::{Registry, ToolContext, ToolExecutionMode};
use jcode::tui::session_picker::{ResumeTarget, SessionInfo, SessionSource};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "jcode-harness")]
#[command(about = "JCode Harness: local AI engineering loop and TUI.")]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize a project for the full jcode-harness experience
    Init(InitArgs),
    /// Preview and serve the Agent Client Protocol integration surface
    Acp(AcpArgs),
    /// Create an isolated safe-evaluation profile for first-time local testing
    #[command(name = "safe-eval")]
    SafeEval(SafeEvalArgs),
    /// Run offline onboarding diagnostics without contacting providers
    Doctor(DoctorArgs),
    /// Print reproducible offline mock demos for README/product claims
    Demo(DemoArgs),
    /// Inspect local/headless session metadata without starting the TUI
    Session(SessionArgs),
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
struct AcpArgs {
    #[command(subcommand)]
    command: AcpCommand,
}

#[derive(Subcommand)]
enum AcpCommand {
    /// Print the preview ACP manifest and registry readiness checklist
    Manifest {
        /// Emit JSON report
        #[arg(long)]
        json: bool,
    },
    /// Serve the preview ACP JSON-RPC protocol over newline-delimited stdio
    Serve {
        /// Read JSON-RPC requests from stdin and write responses to stdout
        #[arg(long)]
        stdio: bool,
    },
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

#[derive(Parser)]
struct DemoArgs {
    #[command(subcommand)]
    command: Option<DemoCommand>,
    /// Project directory used in generated copy-paste commands
    #[arg(long)]
    cwd: Option<String>,
    /// Emit JSON manifest
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum DemoCommand {
    /// Execute one offline demo, or all non-writing demos by default
    Run(DemoRunArgs),
}

#[derive(Parser)]
struct DemoRunArgs {
    /// Demo id from `jcode-harness demo --json`, or `all`
    id: String,
    /// Project directory used for commands that accept --cwd
    #[arg(long)]
    cwd: Option<String>,
    /// Allow demos whose manifest declares project_writes=true
    #[arg(long)]
    allow_writes: bool,
    /// Execute project-writing demos in a temporary sandbox instead of the requested cwd
    #[arg(long)]
    sandbox: bool,
    /// Keep the sandbox directory after the run for manual inspection
    #[arg(long, requires = "sandbox")]
    keep_sandbox: bool,
    /// Emit JSON run report
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
struct SessionArgs {
    #[command(subcommand)]
    command: SessionCommand,
}

#[derive(Subcommand)]
enum SessionCommand {
    /// List local and imported sessions discovered by the session picker
    List(SessionListArgs),
    /// Validate and print a safe new-session spawn plan without starting a provider
    Spawn(SessionSpawnArgs),
    /// Validate and print a safe attach plan without starting the TUI
    Attach(SessionAttachArgs),
    /// Show one local jcode session without starting the TUI
    Show(SessionShowArgs),
    /// Validate and print a safe resume plan without starting the TUI
    Resume(SessionResumeArgs),
}

#[derive(Parser)]
struct SessionListArgs {
    /// Emit JSON report
    #[arg(long)]
    json: bool,
    /// Restrict results by source
    #[arg(long, value_enum, default_value = "all")]
    source: HarnessSessionSourceFilter,
    /// Include debug/test and canary sessions in the output
    #[arg(long)]
    include_test: bool,
    /// Maximum number of sessions to print after filtering
    #[arg(long)]
    limit: Option<usize>,
}

#[derive(Parser)]
struct SessionSpawnArgs {
    /// Goal/message for the new jcode run envelope
    goal: String,
    /// Working directory for the spawned run envelope (defaults to current directory)
    #[arg(long)]
    cwd: Option<String>,
    /// Provider to request when the returned envelope is executed
    #[arg(long, conflicts_with = "provider_profile")]
    provider: Option<String>,
    /// Named OpenAI-compatible provider profile for the returned envelope
    #[arg(long, conflicts_with = "provider")]
    provider_profile: Option<String>,
    /// Model to request when the returned envelope is executed
    #[arg(long)]
    model: Option<String>,
    /// Validate and print the spawn plan without executing it
    #[arg(long)]
    dry_run: bool,
    /// Emit JSON report
    #[arg(long, conflicts_with = "ndjson")]
    json: bool,
    /// Emit newline-delimited JSON events for the dry-run envelope
    #[arg(long, conflicts_with = "json")]
    ndjson: bool,
}

#[derive(Parser)]
struct SessionAttachArgs {
    /// Local jcode session id from `jcode-harness session list --source jcode --json`
    id: String,
    /// Validate and print the attach plan without executing it
    #[arg(long)]
    dry_run: bool,
    /// Emit JSON report
    #[arg(long, conflicts_with = "ndjson")]
    json: bool,
    /// Emit newline-delimited JSON events for the dry-run envelope
    #[arg(long, conflicts_with = "json")]
    ndjson: bool,
}

#[derive(Parser)]
struct SessionShowArgs {
    /// Local jcode session id from `jcode-harness session list --source jcode --json`
    id: String,
    /// Emit JSON report
    #[arg(long)]
    json: bool,
    /// Include the last N rendered visible messages as a bounded preview
    #[arg(long, default_value_t = 0)]
    preview: usize,
}

#[derive(Parser)]
struct SessionResumeArgs {
    /// Local jcode session id from `jcode-harness session list --source jcode --json`
    id: String,
    /// Validate and print the resume plan without executing it
    #[arg(long)]
    dry_run: bool,
    /// Emit JSON report
    #[arg(long, conflicts_with = "ndjson")]
    json: bool,
    /// Emit newline-delimited JSON events for the dry-run envelope
    #[arg(long, conflicts_with = "json")]
    ndjson: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum HarnessSessionSourceFilter {
    All,
    Jcode,
    ClaudeCode,
    Codex,
    Pi,
    Opencode,
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
    /// Manage project-local skill scope policy states
    Scope {
        #[command(subcommand)]
        command: SkillsScopeCommand,
    },
    /// Preview or apply imports from other local skill ecosystems into jcode skills
    Import {
        /// Project directory for resolving default sources and project target
        #[arg(long)]
        cwd: Option<String>,
        /// Source skills directory. Repeat to import from multiple dirs. Defaults to .agents/.claude/.codex/.jcode skills.
        #[arg(long = "from", value_name = "DIR")]
        from: Vec<PathBuf>,
        /// Destination skill scope
        #[arg(long, value_enum, default_value = "project")]
        scope: HarnessSkillImportScope,
        /// Preview only. This is also the default unless --apply is passed.
        #[arg(long, conflicts_with = "apply")]
        dry_run: bool,
        /// Actually copy planned skills into the destination scope
        #[arg(long, conflicts_with = "dry_run")]
        apply: bool,
        /// Allow apply mode to overwrite files for existing target skills
        #[arg(long)]
        force: bool,
        /// Emit JSON report
        #[arg(long)]
        json: bool,
    },
    /// Validate skill files, precedence, and risky prompt/tool patterns without invoking providers
    Validate {
        /// Project directory for resolving repo-local skills
        #[arg(long)]
        cwd: Option<String>,
        /// Emit JSON report
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

#[derive(Subcommand)]
enum SkillsScopeCommand {
    /// Create `.jcode/skills.scope.json` if it does not exist
    Init {
        /// Project directory for the policy file
        #[arg(long)]
        cwd: Option<String>,
        /// Overwrite an existing policy file with an empty default policy
        #[arg(long)]
        force: bool,
        /// Emit JSON report
        #[arg(long)]
        json: bool,
    },
    /// Print the current project-local skill scope policy
    List {
        /// Project directory for the policy file
        #[arg(long)]
        cwd: Option<String>,
        /// Emit JSON report
        #[arg(long)]
        json: bool,
    },
    /// Set one skill to visible, discoverable, or blocked
    Set {
        name: String,
        /// Skill state in this repository
        #[arg(long, value_enum)]
        state: HarnessSkillScopeState,
        /// Human-readable policy reason
        #[arg(long)]
        reason: Option<String>,
        /// Project directory for the policy file
        #[arg(long)]
        cwd: Option<String>,
        /// Emit JSON report
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

#[derive(Clone, ValueEnum)]
enum HarnessSkillScopeState {
    Visible,
    Discoverable,
    Blocked,
}

impl From<HarnessSkillScopeState> for jcode::skill_scope::SkillScopeState {
    fn from(value: HarnessSkillScopeState) -> Self {
        match value {
            HarnessSkillScopeState::Visible => Self::Visible,
            HarnessSkillScopeState::Discoverable => Self::Discoverable,
            HarnessSkillScopeState::Blocked => Self::Blocked,
        }
    }
}

#[derive(Clone, ValueEnum)]
enum HarnessSkillImportScope {
    Project,
    Global,
}

impl From<HarnessSkillImportScope> for jcode::skill_import::SkillImportScope {
    fn from(value: HarnessSkillImportScope) -> Self {
        match value {
            HarnessSkillImportScope::Project => Self::Project,
            HarnessSkillImportScope::Global => Self::Global,
        }
    }
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
        Some(Command::Acp(args)) => run_acp(args),
        Some(Command::SafeEval(args)) => run_safe_eval(args),
        Some(Command::Doctor(args)) => run_doctor(args),
        Some(Command::Demo(args)) => run_demo(args),
        Some(Command::Session(args)) => run_session(args),
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

fn run_acp(args: AcpArgs) -> Result<()> {
    match args.command {
        AcpCommand::Manifest { json } => run_acp_manifest(json),
        AcpCommand::Serve { stdio } => run_acp_serve(stdio),
    }
}

fn acp_manifest_json() -> serde_json::Value {
    json!({
        "status": "ok",
        "command": "acp manifest",
        "offline": true,
        "read_only": true,
        "protocol": {
            "id": "acp",
            "name": "Agent Client Protocol",
            "jsonrpc": "2.0",
            "transport": ["stdio"],
            "framing": "newline-delimited-json",
            "status": "preview"
        },
        "implementation": {
            "name": "jcode-harness",
            "version": env!("CARGO_PKG_VERSION"),
            "repository": "https://github.com/chapzin/jcode-harness"
        },
        "capabilities": acp_capabilities_json(),
        "methods": [
            {"name": "initialize", "kind": "request", "status": "implemented"},
            {"name": "shutdown", "kind": "request", "status": "implemented"},
            {"name": "initialized", "kind": "notification", "status": "implemented_noop"},
            {"name": "jcode/session.list", "kind": "request", "status": "implemented_offline"},
            {"name": "jcode/session.show", "kind": "request", "status": "implemented_offline_metadata_preview_opt_in"},
            {"name": "jcode/session.spawn", "kind": "request", "status": "implemented_offline_dry_run_envelope"},
            {"name": "jcode/session.attach", "kind": "request", "status": "implemented_offline_dry_run_envelope"},
            {"name": "jcode/session.resume", "kind": "request", "status": "implemented_offline_dry_run_envelope"},
            {"name": "$/cancelRequest", "kind": "notification", "status": "planned"}
        ],
        "registry": {
            "ready": false,
            "status": "preview_not_registry_ready",
            "missing": [
                "live session execution/streaming handlers",
                "tool event streaming contract",
                "cancellation semantics",
                "versioned ACP conformance fixture"
            ]
        },
        "safety": {
            "starts_tui": false,
            "starts_provider": false,
            "starts_tools": false,
            "network_required": false,
            "credentials_required": false
        }
    })
}

fn acp_capabilities_json() -> serde_json::Value {
    json!({
        "initialize": true,
        "shutdown": true,
        "session": {
            "list": {"status": "implemented_offline", "method": "jcode/session.list", "command": "jcode-harness session list --json"},
            "spawn": {"status": "implemented_offline_dry_run", "method": "jcode/session.spawn", "command": "jcode-harness session spawn <goal> --dry-run --json|--ndjson"},
            "attach": {"status": "implemented_offline_dry_run", "method": "jcode/session.attach", "command": "jcode-harness session attach <id> --dry-run --json|--ndjson"},
            "show": {"status": "implemented_offline", "method": "jcode/session.show", "command": "jcode-harness session show <id> --json"},
            "resume": {"status": "implemented_offline_dry_run", "method": "jcode/session.resume", "command": "jcode-harness session resume <id> --dry-run --json|--ndjson"}
        },
        "events": {
            "session_envelopes_ndjson": true,
            "session_updates": false,
            "tool_events": false
        },
        "cancellation": {"supported": false},
        "registry_submission": {"ready": false}
    })
}

fn run_acp_manifest(json_output: bool) -> Result<()> {
    let manifest = acp_manifest_json();
    if json_output {
        println!("{}", serde_json::to_string_pretty(&manifest)?);
    } else {
        println!("jcode-harness ACP manifest: preview");
        println!("Transport: stdio JSON-RPC 2.0");
        println!("Implemented methods: initialize, initialized, shutdown, jcode/session.* offline");
        println!("Registry ready: false");
    }
    Ok(())
}

fn run_acp_serve(stdio: bool) -> Result<()> {
    if !stdio {
        anyhow::bail!(
            "ACP serve currently supports only --stdio newline-delimited JSON-RPC preview mode"
        );
    }

    use std::io::{self, BufRead};

    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<serde_json::Value>(trimmed) {
            Ok(request) => acp_handle_jsonrpc_request(&request),
            Err(error) => Some(acp_jsonrpc_error(
                serde_json::Value::Null,
                -32700,
                "parse error",
                Some(json!({"detail": error.to_string()})),
            )),
        };
        if let Some(response) = response {
            println!("{}", serde_json::to_string(&response)?);
            if response
                .get("result")
                .and_then(|result| result.get("shutdown"))
                .and_then(serde_json::Value::as_bool)
                == Some(true)
            {
                break;
            }
        }
    }
    Ok(())
}

fn acp_handle_jsonrpc_request(request: &serde_json::Value) -> Option<serde_json::Value> {
    let id = request.get("id").cloned();
    let method = request
        .get("method")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();

    if method.is_empty()
        || request.get("jsonrpc").and_then(serde_json::Value::as_str) != Some("2.0")
    {
        return Some(acp_jsonrpc_error(
            id.unwrap_or(serde_json::Value::Null),
            -32600,
            "invalid request",
            None,
        ));
    }

    if id.is_none() {
        return None;
    }
    let id = id.unwrap_or(serde_json::Value::Null);

    match method {
        "initialize" => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocol": "acp",
                "serverInfo": {
                    "name": "jcode-harness",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": acp_capabilities_json(),
                "manifest": acp_manifest_json(),
            }
        })),
        "shutdown" => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {"shutdown": true}
        })),
        "jcode/session.list" => Some(acp_jsonrpc_result(id, acp_session_list_result(request))),
        "jcode/session.show" => Some(acp_jsonrpc_result(id, acp_session_show_result(request))),
        "jcode/session.spawn" => Some(acp_jsonrpc_result(id, acp_session_spawn_result(request))),
        "jcode/session.attach" => Some(acp_jsonrpc_result(id, acp_session_attach_result(request))),
        "jcode/session.resume" => Some(acp_jsonrpc_result(id, acp_session_resume_result(request))),
        "initialized" => None,
        _ => Some(acp_jsonrpc_error(id, -32601, "method not found", None)),
    }
}

fn acp_jsonrpc_result(
    id: serde_json::Value,
    result: Result<serde_json::Value>,
) -> serde_json::Value {
    match result {
        Ok(result) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }),
        Err(error) => acp_jsonrpc_error(
            id,
            -32602,
            "invalid params",
            Some(json!({"detail": error.to_string()})),
        ),
    }
}

fn acp_request_params(
    request: &serde_json::Value,
) -> Result<Option<&serde_json::Map<String, serde_json::Value>>> {
    match request.get("params") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Object(params)) => Ok(Some(params)),
        Some(_) => anyhow::bail!("params must be a JSON object when present"),
    }
}

fn acp_param_value<'a>(
    params: Option<&'a serde_json::Map<String, serde_json::Value>>,
    names: &[&str],
) -> Option<&'a serde_json::Value> {
    let params = params?;
    names.iter().find_map(|name| params.get(*name))
}

fn acp_optional_str<'a>(
    params: Option<&'a serde_json::Map<String, serde_json::Value>>,
    names: &[&str],
) -> Result<Option<&'a str>> {
    match acp_param_value(params, names) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(Some)
            .ok_or_else(|| anyhow::anyhow!("{} must be a string", names[0])),
    }
}

fn acp_required_str<'a>(
    params: Option<&'a serde_json::Map<String, serde_json::Value>>,
    name: &str,
) -> Result<&'a str> {
    acp_optional_str(params, &[name])?
        .ok_or_else(|| anyhow::anyhow!("missing required param {name}"))
}

fn acp_optional_bool(
    params: Option<&serde_json::Map<String, serde_json::Value>>,
    names: &[&str],
) -> Result<Option<bool>> {
    match acp_param_value(params, names) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(value) => value
            .as_bool()
            .map(Some)
            .ok_or_else(|| anyhow::anyhow!("{} must be a boolean", names[0])),
    }
}

fn acp_optional_usize(
    params: Option<&serde_json::Map<String, serde_json::Value>>,
    names: &[&str],
) -> Result<Option<usize>> {
    match acp_param_value(params, names) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(value) => {
            let number = value
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("{} must be a non-negative integer", names[0]))?;
            usize::try_from(number)
                .map(Some)
                .map_err(|_| anyhow::anyhow!("{} is too large", names[0]))
        }
    }
}

fn acp_session_source_filter(value: Option<&str>) -> Result<HarnessSessionSourceFilter> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(HarnessSessionSourceFilter::All);
    };
    match value.to_ascii_lowercase().replace('-', "_").as_str() {
        "all" => Ok(HarnessSessionSourceFilter::All),
        "jcode" => Ok(HarnessSessionSourceFilter::Jcode),
        "claude" | "claude_code" | "claudecode" => Ok(HarnessSessionSourceFilter::ClaudeCode),
        "codex" => Ok(HarnessSessionSourceFilter::Codex),
        "pi" => Ok(HarnessSessionSourceFilter::Pi),
        "opencode" | "open_code" => Ok(HarnessSessionSourceFilter::Opencode),
        _ => anyhow::bail!("unsupported session source '{value}'"),
    }
}

fn acp_output_mode(value: Option<&str>) -> Result<SessionEnvelopeOutputMode> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(SessionEnvelopeOutputMode::Json);
    };
    match value.to_ascii_lowercase().replace('-', "_").as_str() {
        "text" => Ok(SessionEnvelopeOutputMode::Text),
        "json" => Ok(SessionEnvelopeOutputMode::Json),
        "ndjson" | "jsonl" => Ok(SessionEnvelopeOutputMode::Ndjson),
        _ => anyhow::bail!("unsupported outputMode '{value}'"),
    }
}

fn acp_session_list_result(request: &serde_json::Value) -> Result<serde_json::Value> {
    let params = acp_request_params(request)?;
    let source = acp_session_source_filter(acp_optional_str(params, &["source"])?)?;
    let include_test =
        acp_optional_bool(params, &["includeTest", "include_test"])?.unwrap_or(false);
    let limit = acp_optional_usize(params, &["limit"])?;
    session_list_report(source, include_test, limit)
}

fn acp_session_show_result(request: &serde_json::Value) -> Result<serde_json::Value> {
    let params = acp_request_params(request)?;
    let id = acp_required_str(params, "id")?;
    let preview = acp_optional_usize(params, &["preview"])?.unwrap_or(0);
    session_show_report(id, preview)
}

fn acp_session_spawn_result(request: &serde_json::Value) -> Result<serde_json::Value> {
    let params = acp_request_params(request)?;
    let goal = acp_required_str(params, "goal")?;
    let output_mode = acp_output_mode(acp_optional_str(params, &["outputMode", "output_mode"])?)?;
    session_spawn_report(
        goal,
        acp_optional_str(params, &["cwd"])?,
        acp_optional_str(params, &["provider"])?,
        acp_optional_str(params, &["providerProfile", "provider_profile"])?,
        acp_optional_str(params, &["model"])?,
        output_mode,
    )
}

fn acp_session_attach_result(request: &serde_json::Value) -> Result<serde_json::Value> {
    let params = acp_request_params(request)?;
    let id = acp_required_str(params, "id")?;
    session_attach_report(id)
}

fn acp_session_resume_result(request: &serde_json::Value) -> Result<serde_json::Value> {
    let params = acp_request_params(request)?;
    let id = acp_required_str(params, "id")?;
    session_resume_report(id)
}

fn acp_jsonrpc_error(
    id: serde_json::Value,
    code: i64,
    message: &str,
    data: Option<serde_json::Value>,
) -> serde_json::Value {
    let mut error = serde_json::Map::new();
    error.insert("code".to_string(), json!(code));
    error.insert("message".to_string(), json!(message));
    if let Some(data) = data {
        error.insert("data".to_string(), data);
    }
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": error,
    })
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

fn run_session(args: SessionArgs) -> Result<()> {
    match args.command {
        SessionCommand::List(args) => run_session_list(args),
        SessionCommand::Spawn(args) => run_session_spawn(args),
        SessionCommand::Attach(args) => run_session_attach(args),
        SessionCommand::Show(args) => run_session_show(args),
        SessionCommand::Resume(args) => run_session_resume(args),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionEnvelopeOutputMode {
    Text,
    Json,
    Ndjson,
}

impl SessionEnvelopeOutputMode {
    fn label(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
            Self::Ndjson => "ndjson",
        }
    }

    fn argv_flag(self) -> Option<&'static str> {
        match self {
            Self::Text => None,
            Self::Json => Some("--json"),
            Self::Ndjson => Some("--ndjson"),
        }
    }
}

fn print_session_dry_run_report(
    report: &serde_json::Value,
    json_output: bool,
    ndjson_output: bool,
) -> Result<bool> {
    if ndjson_output {
        let command = report
            .get("command")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("session");
        for event in [
            json!({
                "type": "start",
                "command": command,
                "offline": true,
                "read_only": true,
                "dry_run": true,
            }),
            json!({
                "type": "envelope",
                "command": command,
                "envelope": report,
            }),
            json!({
                "type": "done",
                "command": command,
                "status": "ok",
                "executed": false,
            }),
        ] {
            println!("{}", serde_json::to_string(&event)?);
        }
        return Ok(true);
    }

    if json_output {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(true);
    }

    Ok(false)
}

fn run_session_spawn(args: SessionSpawnArgs) -> Result<()> {
    if !args.dry_run {
        anyhow::bail!(
            "session spawn execution is not supported by jcode-harness yet; rerun with --dry-run to inspect the safe spawn envelope"
        );
    }
    let output_mode = if args.json {
        SessionEnvelopeOutputMode::Json
    } else if args.ndjson {
        SessionEnvelopeOutputMode::Ndjson
    } else {
        SessionEnvelopeOutputMode::Text
    };
    let report = session_spawn_report(
        &args.goal,
        args.cwd.as_deref(),
        args.provider.as_deref(),
        args.provider_profile.as_deref(),
        args.model.as_deref(),
        output_mode,
    )?;

    if print_session_dry_run_report(&report, args.json, args.ndjson)? {
        return Ok(());
    }

    let command_display = report["spawn"]["command_display"]
        .as_str()
        .unwrap_or("jcode run <goal>");
    let working_dir = report["spawn"]["cwd"].as_str().unwrap_or("<unknown>");

    println!("jcode-harness session spawn dry-run");
    println!("Offline: true");
    println!("Read only: true");
    println!("Executed: false");
    println!("Command: {command_display}");
    println!("Working directory: {working_dir}");

    Ok(())
}

fn session_spawn_report(
    goal: &str,
    cwd: Option<&str>,
    provider: Option<&str>,
    provider_profile: Option<&str>,
    model: Option<&str>,
    output_mode: SessionEnvelopeOutputMode,
) -> Result<serde_json::Value> {
    if goal.trim().is_empty() {
        anyhow::bail!("session spawn goal must not be empty");
    }

    let provider = provider.map(str::trim).filter(|value| !value.is_empty());
    let provider_profile = provider_profile
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if provider.is_some() && provider_profile.is_some() {
        anyhow::bail!("session spawn provider and provider_profile are mutually exclusive");
    }
    if let Some(provider) = provider {
        ProviderChoice::from_str(provider, true)
            .map_err(|err| anyhow::anyhow!("invalid provider '{}': {}", provider, err))?;
    }

    let cwd = cwd.map(str::trim).filter(|value| !value.is_empty());
    let root = resolve_existing_root(cwd, "session spawn")?;
    let working_dir = root.to_string_lossy().to_string();
    let working_dir_source = if cwd.is_some() {
        "argument"
    } else {
        "current_dir"
    };
    let provider = provider.unwrap_or("auto");
    let model = model.map(str::trim).filter(|value| !value.is_empty());

    let mut argv = vec!["jcode".to_string(), "-C".to_string(), working_dir.clone()];
    if let Some(profile) = provider_profile {
        argv.push("--provider-profile".to_string());
        argv.push(profile.to_string());
    } else if provider != "auto" {
        argv.push("-p".to_string());
        argv.push(provider.to_string());
    }
    if let Some(model) = model {
        argv.push("-m".to_string());
        argv.push(model.to_string());
    }
    argv.push("run".to_string());
    if let Some(flag) = output_mode.argv_flag() {
        argv.push(flag.to_string());
    }
    argv.push(goal.to_string());
    let command_display = argv
        .iter()
        .map(|arg| shell_quote(arg))
        .collect::<Vec<_>>()
        .join(" ");

    Ok(json!({
        "status": "ok",
        "command": "session spawn",
        "offline": true,
        "read_only": true,
        "dry_run": true,
        "executed": false,
        "source": "jcode",
        "goal": goal,
        "spawn": {
            "supported_by": "jcode-cli-run",
            "execution_supported_by_harness": false,
            "creates_new_session": true,
            "requires_terminal": false,
            "starts_tui": false,
            "starts_provider": "on_execution",
            "program": "jcode",
            "argv": argv,
            "command_display": command_display,
            "cwd": &working_dir,
            "cwd_source": working_dir_source,
            "output_mode": output_mode.label(),
            "provider": provider,
            "provider_profile": provider_profile,
            "model": model,
        },
        "safety": {
            "executed": false,
            "writes": false,
            "network_required_for_dry_run": false,
            "credentials_required_for_dry_run": false,
            "note": "Use the returned argv/cwd outside dry-run only after choosing an execution surface."
        },
    }))
}

fn run_session_attach(args: SessionAttachArgs) -> Result<()> {
    if !args.dry_run {
        anyhow::bail!(
            "session attach execution is not supported by jcode-harness yet; rerun with --dry-run to inspect the safe attach envelope"
        );
    }
    let report = session_attach_report(&args.id)?;

    if print_session_dry_run_report(&report, args.json, args.ndjson)? {
        return Ok(());
    }

    let session_id = report["id"].as_str().unwrap_or(&args.id);
    let command_display = report["attach"]["command_display"]
        .as_str()
        .unwrap_or("jcode --resume <id>");
    let working_dir = report["attach"]["cwd"].as_str().unwrap_or("<unknown>");

    println!("jcode-harness session attach dry-run: {session_id}");
    println!("Offline: true");
    println!("Read only: true");
    println!("Executed: false");
    println!("Command: {command_display}");
    println!("Working directory: {working_dir}");

    Ok(())
}

fn session_attach_report(id: &str) -> Result<serde_json::Value> {
    if id.contains(':') {
        anyhow::bail!(
            "session attach currently supports local jcode session ids only; use `session list --source jcode --json`"
        );
    }

    let session_path = jcode::session::session_path(id)?;
    let journal_path = jcode::session::session_journal_path(id)?;
    let session = jcode::session::Session::load(id)?;
    let fallback_cwd = std::env::current_dir()?.to_string_lossy().to_string();
    let working_dir = session
        .working_dir
        .as_deref()
        .filter(|dir| !dir.trim().is_empty())
        .unwrap_or(&fallback_cwd);
    let working_dir_source = if session
        .working_dir
        .as_deref()
        .filter(|dir| !dir.trim().is_empty())
        .is_some()
    {
        "session"
    } else {
        "current_dir"
    };
    let argv = vec![
        "jcode".to_string(),
        "--resume".to_string(),
        session.id.clone(),
    ];
    let command_display = argv
        .iter()
        .map(|arg| shell_quote(arg))
        .collect::<Vec<_>>()
        .join(" ");

    Ok(json!({
        "status": "ok",
        "command": "session attach",
        "offline": true,
        "read_only": true,
        "dry_run": true,
        "executed": false,
        "source": "jcode",
        "id": &session.id,
        "session_path": &session_path,
        "session_exists": session_path.exists(),
        "journal_path": &journal_path,
        "journal_exists": journal_path.exists(),
        "metadata": session_metadata_json(&session),
        "attach": {
            "supported_by": "jcode-cli-resume",
            "execution_supported_by_harness": false,
            "attach_mode": "local_session_resume_surface",
            "requires_terminal": true,
            "starts_tui": true,
            "starts_provider": "on_interaction_or_resume_flow",
            "program": "jcode",
            "argv": argv,
            "command_display": command_display,
            "cwd": working_dir,
            "cwd_source": working_dir_source,
            "live_session_detection": "not_attempted_offline_dry_run",
        },
        "safety": {
            "executed": false,
            "writes": false,
            "network_required_for_dry_run": false,
            "credentials_required_for_dry_run": false,
            "note": "Use the returned argv/cwd outside dry-run only after choosing an execution surface."
        },
    }))
}

fn run_session_resume(args: SessionResumeArgs) -> Result<()> {
    if !args.dry_run {
        anyhow::bail!(
            "session resume execution is not supported by jcode-harness yet; rerun with --dry-run to inspect the safe resume envelope"
        );
    }
    let report = session_resume_report(&args.id)?;

    if print_session_dry_run_report(&report, args.json, args.ndjson)? {
        return Ok(());
    }

    let session_id = report["id"].as_str().unwrap_or(&args.id);
    let command_display = report["resume"]["command_display"]
        .as_str()
        .unwrap_or("jcode --resume <id>");
    let working_dir = report["resume"]["cwd"].as_str().unwrap_or("<unknown>");

    println!("jcode-harness session resume dry-run: {session_id}");
    println!("Offline: true");
    println!("Read only: true");
    println!("Executed: false");
    println!("Command: {command_display}");
    println!("Working directory: {working_dir}");

    Ok(())
}

fn session_resume_report(id: &str) -> Result<serde_json::Value> {
    if id.contains(':') {
        anyhow::bail!(
            "session resume currently supports local jcode session ids only; use `session list --source jcode --json`"
        );
    }

    let session_path = jcode::session::session_path(id)?;
    let journal_path = jcode::session::session_journal_path(id)?;
    let session = jcode::session::Session::load(id)?;
    let fallback_cwd = std::env::current_dir()?.to_string_lossy().to_string();
    let working_dir = session
        .working_dir
        .as_deref()
        .filter(|dir| !dir.trim().is_empty())
        .unwrap_or(&fallback_cwd);
    let working_dir_source = if session
        .working_dir
        .as_deref()
        .filter(|dir| !dir.trim().is_empty())
        .is_some()
    {
        "session"
    } else {
        "current_dir"
    };
    let argv = vec![
        "jcode".to_string(),
        "--resume".to_string(),
        session.id.clone(),
    ];
    let command_display = argv
        .iter()
        .map(|arg| shell_quote(arg))
        .collect::<Vec<_>>()
        .join(" ");

    Ok(json!({
        "status": "ok",
        "command": "session resume",
        "offline": true,
        "read_only": true,
        "dry_run": true,
        "executed": false,
        "source": "jcode",
        "id": &session.id,
        "session_path": &session_path,
        "session_exists": session_path.exists(),
        "journal_path": &journal_path,
        "journal_exists": journal_path.exists(),
        "metadata": session_metadata_json(&session),
        "resume": {
            "supported_by": "jcode-cli",
            "execution_supported_by_harness": false,
            "requires_terminal": true,
            "starts_tui": true,
            "starts_provider": "on_interaction_or_resume_flow",
            "program": "jcode",
            "argv": argv,
            "command_display": command_display,
            "cwd": working_dir,
            "cwd_source": working_dir_source,
        },
        "safety": {
            "executed": false,
            "writes": false,
            "network_required_for_dry_run": false,
            "credentials_required_for_dry_run": false,
            "note": "Use the returned argv/cwd outside dry-run only after choosing an execution surface."
        },
    }))
}

fn run_session_show(args: SessionShowArgs) -> Result<()> {
    let report = session_show_report(&args.id, args.preview)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "jcode-harness session show: {}",
            report["id"].as_str().unwrap_or(&args.id)
        );
        println!("Offline: true");
        println!("Read only: true");
        println!(
            "Title: {}",
            report["metadata"]["title"].as_str().unwrap_or("Untitled")
        );
        println!(
            "Status: {}",
            report["metadata"]["status"].as_str().unwrap_or("unknown")
        );
        println!(
            "Messages stored: {}",
            report["metadata"]["stored_message_count"]
                .as_u64()
                .unwrap_or(0)
        );
        if args.preview == 0 {
            println!("Preview: disabled (use --preview N)");
        } else {
            println!("Preview: last {} rendered messages", args.preview);
            for message in report["preview"]["messages"]
                .as_array()
                .into_iter()
                .flatten()
            {
                println!(
                    "- [{}] {}",
                    message["role"].as_str().unwrap_or("unknown"),
                    message["content"].as_str().unwrap_or_default()
                );
            }
        }
    }

    Ok(())
}

fn session_show_report(id: &str, preview: usize) -> Result<serde_json::Value> {
    if id.contains(':') {
        anyhow::bail!(
            "session show currently supports local jcode session ids only; use `session list --source jcode --json`"
        );
    }

    let session_path = jcode::session::session_path(id)?;
    let journal_path = jcode::session::session_journal_path(id)?;
    let session = jcode::session::Session::load(id)?;
    let preview_messages = session_preview_json(&session, preview);
    Ok(json!({
        "status": "ok",
        "command": "session show",
        "offline": true,
        "read_only": true,
        "source": "jcode",
        "id": &session.id,
        "session_path": &session_path,
        "session_exists": session_path.exists(),
        "journal_path": &journal_path,
        "journal_exists": journal_path.exists(),
        "metadata": session_metadata_json(&session),
        "preview": {
            "requested": preview,
            "returned": preview_messages.len(),
            "content_truncated_to_chars": SESSION_SHOW_PREVIEW_CONTENT_CHARS,
            "messages": preview_messages,
        },
    }))
}

fn run_session_list(args: SessionListArgs) -> Result<()> {
    let report = session_list_report(args.source, args.include_test, args.limit)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let sessions = report["sessions"].as_array().cloned().unwrap_or_default();
        let hidden_test_count = report["hidden_test_count"].as_u64().unwrap_or(0);
        println!("jcode-harness session list: {} sessions", sessions.len());
        println!("Offline: true");
        println!("Read only: true");
        if !args.include_test && hidden_test_count > 0 {
            println!("Hidden test/canary sessions: {hidden_test_count} (use --include-test)");
        }
        for session in &sessions {
            println!(
                "- {} [{}] {} ({}, {} messages)",
                session["short_name"].as_str().unwrap_or("unknown"),
                session["source"].as_str().unwrap_or("unknown"),
                session["title"].as_str().unwrap_or("Untitled"),
                session["status"].as_str().unwrap_or("unknown"),
                session["message_count"].as_u64().unwrap_or(0)
            );
        }
    }

    Ok(())
}

fn session_list_report(
    source: HarnessSessionSourceFilter,
    include_test: bool,
    limit: Option<usize>,
) -> Result<serde_json::Value> {
    let mut sessions = jcode::tui::session_picker::load_sessions()?;
    let loaded_count = sessions.len();

    if source != HarnessSessionSourceFilter::All {
        sessions.retain(|session| session_matches_source_filter(session, source));
    }

    let discovered_count = sessions.len();
    let hidden_test_count = sessions
        .iter()
        .filter(|session| session.is_debug || session.is_canary)
        .count();

    if !include_test {
        sessions.retain(|session| !session.is_debug && !session.is_canary);
    }

    if let Some(limit) = limit {
        sessions.truncate(limit);
    }

    let sessions_dir = jcode::storage::jcode_dir()?.join("sessions");
    Ok(json!({
        "status": "ok",
        "command": "session list",
        "offline": true,
        "read_only": true,
        "sessions_dir": sessions_dir,
        "loaded_count": loaded_count,
        "session_count": sessions.len(),
        "discovered_count": discovered_count,
        "hidden_test_count": if include_test { 0 } else { hidden_test_count },
        "include_test": include_test,
        "source": session_source_filter_label(source),
        "limit": limit,
        "sessions": sessions.iter().map(session_info_json).collect::<Vec<_>>(),
    }))
}

const SESSION_SHOW_PREVIEW_CONTENT_CHARS: usize = 4000;

fn session_metadata_json(session: &jcode::session::Session) -> serde_json::Value {
    let user_message_count = session
        .messages
        .iter()
        .filter(|message| message.display_role.is_none() && message.role == Role::User)
        .count();
    let assistant_message_count = session
        .messages
        .iter()
        .filter(|message| message.display_role.is_none() && message.role == Role::Assistant)
        .count();
    let total_tokens: u64 = session
        .messages
        .iter()
        .filter_map(|message| message.token_usage.as_ref())
        .map(|usage| {
            usage.input_tokens
                + usage.output_tokens
                + usage.cache_read_input_tokens.unwrap_or(0)
                + usage.cache_creation_input_tokens.unwrap_or(0)
        })
        .sum();

    json!({
        "id": &session.id,
        "parent_id": &session.parent_id,
        "short_name": &session.short_name,
        "display_name": session.display_name(),
        "title": &session.title,
        "created_at": session.created_at.to_rfc3339(),
        "updated_at": session.updated_at.to_rfc3339(),
        "last_active_at": session.last_active_at.map(|time| time.to_rfc3339()),
        "working_dir": &session.working_dir,
        "model": &session.model,
        "provider_key": &session.provider_key,
        "provider_session_id": &session.provider_session_id,
        "reasoning_effort": &session.reasoning_effort,
        "status": session.status.display(),
        "status_detail": session.status.detail(),
        "stored_message_count": session.messages.len(),
        "user_message_count": user_message_count,
        "assistant_message_count": assistant_message_count,
        "env_snapshot_count": session.env_snapshots.len(),
        "memory_injection_count": session.memory_injections.len(),
        "replay_event_count": session.replay_events.len(),
        "estimated_total_tokens": total_tokens,
        "is_canary": session.is_canary,
        "testing_build": &session.testing_build,
        "is_debug": session.is_debug,
        "saved": session.saved,
        "save_label": &session.save_label,
        "last_pid": session.last_pid,
        "has_compaction": session.compaction.is_some(),
        "compaction": session.compaction.as_ref().map(|state| json!({
            "covers_up_to_turn": state.covers_up_to_turn,
            "original_turn_count": state.original_turn_count,
            "compacted_count": state.compacted_count,
            "has_summary": !state.summary_text.trim().is_empty(),
            "has_openai_encrypted_content": state.openai_encrypted_content.is_some(),
        })),
    })
}

fn session_preview_json(
    session: &jcode::session::Session,
    preview_limit: usize,
) -> Vec<serde_json::Value> {
    if preview_limit == 0 {
        return Vec::new();
    }

    let rendered = jcode::session::render_messages(session);
    let start_index = rendered.len().saturating_sub(preview_limit);
    rendered
        .into_iter()
        .enumerate()
        .skip(start_index)
        .map(|(index, message)| {
            let truncated =
                jcode::util::truncate_str(&message.content, SESSION_SHOW_PREVIEW_CONTENT_CHARS);
            let content = truncated.to_string();
            let is_truncated = content.len() < message.content.len();
            json!({
                "index": index,
                "role": message.role,
                "content": content,
                "truncated": is_truncated,
                "tool_calls": message.tool_calls,
                "tool": message.tool_data.as_ref().map(|tool| json!({
                    "id": &tool.id,
                    "name": &tool.name,
                    "intent": &tool.intent,
                })),
            })
        })
        .collect()
}

fn session_matches_source_filter(
    session: &SessionInfo,
    filter: HarnessSessionSourceFilter,
) -> bool {
    match filter {
        HarnessSessionSourceFilter::All => true,
        HarnessSessionSourceFilter::Jcode => session.source == SessionSource::Jcode,
        HarnessSessionSourceFilter::ClaudeCode => session.source == SessionSource::ClaudeCode,
        HarnessSessionSourceFilter::Codex => session.source == SessionSource::Codex,
        HarnessSessionSourceFilter::Pi => session.source == SessionSource::Pi,
        HarnessSessionSourceFilter::Opencode => session.source == SessionSource::OpenCode,
    }
}

fn session_source_filter_label(filter: HarnessSessionSourceFilter) -> &'static str {
    match filter {
        HarnessSessionSourceFilter::All => "all",
        HarnessSessionSourceFilter::Jcode => "jcode",
        HarnessSessionSourceFilter::ClaudeCode => "claude_code",
        HarnessSessionSourceFilter::Codex => "codex",
        HarnessSessionSourceFilter::Pi => "pi",
        HarnessSessionSourceFilter::Opencode => "opencode",
    }
}

fn session_info_json(session: &SessionInfo) -> serde_json::Value {
    json!({
        "id": &session.id,
        "parent_id": &session.parent_id,
        "source": session_source_label(session.source),
        "short_name": &session.short_name,
        "icon": &session.icon,
        "title": &session.title,
        "message_count": session.message_count,
        "user_message_count": session.user_message_count,
        "assistant_message_count": session.assistant_message_count,
        "created_at": session.created_at.to_rfc3339(),
        "last_message_time": session.last_message_time.to_rfc3339(),
        "last_active_at": session.last_active_at.map(|time| time.to_rfc3339()),
        "working_dir": &session.working_dir,
        "model": &session.model,
        "provider_key": &session.provider_key,
        "status": session.status.display(),
        "status_detail": session.status.detail(),
        "needs_catchup": session.needs_catchup,
        "estimated_tokens": session.estimated_tokens,
        "is_canary": session.is_canary,
        "is_debug": session.is_debug,
        "saved": session.saved,
        "save_label": &session.save_label,
        "server_name": &session.server_name,
        "server_icon": &session.server_icon,
        "resume_target": resume_target_json(&session.resume_target),
        "external_path": &session.external_path,
    })
}

fn session_source_label(source: SessionSource) -> &'static str {
    match source {
        SessionSource::Jcode => "jcode",
        SessionSource::ClaudeCode => "claude_code",
        SessionSource::Codex => "codex",
        SessionSource::Pi => "pi",
        SessionSource::OpenCode => "opencode",
    }
}

fn resume_target_json(target: &ResumeTarget) -> serde_json::Value {
    match target {
        ResumeTarget::JcodeSession { session_id } => {
            json!({ "kind": "jcode_session", "id": session_id })
        }
        ResumeTarget::ClaudeCodeSession {
            session_id,
            session_path,
        } => json!({
            "kind": "claude_code_session",
            "id": session_id,
            "path": session_path,
        }),
        ResumeTarget::CodexSession {
            session_id,
            session_path,
        } => json!({
            "kind": "codex_session",
            "id": session_id,
            "path": session_path,
        }),
        ResumeTarget::PiSession { session_path } => json!({
            "kind": "pi_session",
            "id": session_path,
            "path": session_path,
        }),
        ResumeTarget::OpenCodeSession {
            session_id,
            session_path,
        } => json!({
            "kind": "opencode_session",
            "id": session_id,
            "path": session_path,
        }),
    }
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

fn run_demo(args: DemoArgs) -> Result<()> {
    if let Some(command) = args.command {
        return match command {
            DemoCommand::Run(run_args) => run_demo_run(run_args),
        };
    }

    let root = resolve_existing_root(args.cwd.as_deref(), "demo")?;
    let root = root.canonicalize().unwrap_or(root);
    let manifest = build_demo_manifest(&root);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&manifest)?);
        return Ok(());
    }

    println!(
        "jcode-harness demo: {}",
        manifest["status"].as_str().unwrap_or("ok")
    );
    println!("Offline: true");
    println!("Network required: false");
    println!("Credentials required: false");
    println!("Root: {}", manifest["root"].as_str().unwrap_or("<unknown>"));
    println!("\nReproducible demos:");
    if let Some(demos) = manifest["demos"].as_array() {
        for demo in demos {
            println!(
                "- [{}] {}: {}",
                demo["surface"].as_str().unwrap_or("unknown"),
                demo["id"].as_str().unwrap_or("unknown"),
                demo["title"].as_str().unwrap_or("")
            );
            println!("  $ {}", demo["command"].as_str().unwrap_or(""));
        }
    }
    println!("\nRecommended flow:");
    if let Some(flow) = manifest["recommended_flow"].as_array() {
        for (idx, step) in flow.iter().enumerate() {
            println!("{}. {}", idx + 1, step.as_str().unwrap_or(""));
        }
    }
    Ok(())
}

fn run_demo_run(args: DemoRunArgs) -> Result<()> {
    let root = resolve_existing_root(args.cwd.as_deref(), "demo run")?;
    let root = root.canonicalize().unwrap_or(root);
    let sandbox_root = if args.sandbox {
        Some(create_demo_sandbox_root()?)
    } else {
        None
    };
    let execution_root = sandbox_root.as_deref().unwrap_or(&root);
    let manifest = build_demo_manifest(execution_root);
    let demos = manifest["demos"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("demo manifest missing demos array"))?;

    let requested = args.id.trim();
    let mut results = Vec::new();
    if requested == "all" {
        for demo in demos {
            if demo["project_writes"].as_bool().unwrap_or(false)
                && !args.allow_writes
                && !args.sandbox
            {
                results.push(blocked_demo_result(
                    execution_root,
                    demo,
                    "project_writes=true; pass --allow-writes or --sandbox to execute this demo",
                ));
                continue;
            }
            results.push(execute_demo_entry(execution_root, demo)?);
        }
    } else if let Some(demo) = demos
        .iter()
        .find(|demo| demo["id"].as_str() == Some(requested))
    {
        if demo["project_writes"].as_bool().unwrap_or(false) && !args.allow_writes && !args.sandbox
        {
            results.push(blocked_demo_result(
                execution_root,
                demo,
                "project_writes=true; pass --allow-writes or --sandbox to execute this demo",
            ));
        } else {
            results.push(execute_demo_entry(execution_root, demo)?);
        }
    } else {
        let known = demos
            .iter()
            .filter_map(|demo| demo["id"].as_str())
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!("unknown demo id '{requested}'. Known demos: {known}");
    }

    let has_fail = results.iter().any(|result| result["status"] == "fail");
    let has_blocked = results.iter().any(|result| result["status"] == "blocked");
    let status = if has_fail {
        "error"
    } else if has_blocked && requested != "all" {
        "blocked"
    } else if has_blocked {
        "warn"
    } else {
        "ok"
    };
    let sandbox = sandbox_root
        .as_ref()
        .map(|path| {
            json!({
                "enabled": true,
                "path": path,
                "retained": args.keep_sandbox,
                "cleanup": if args.keep_sandbox { "kept" } else { "removed_after_run" },
            })
        })
        .unwrap_or_else(|| {
            json!({
                "enabled": false,
                "path": null,
                "retained": false,
                "cleanup": "none",
            })
        });
    let report = json!({
        "status": status,
        "offline": true,
        "network_required": false,
        "credentials_required": false,
        "root": root,
        "execution_root": execution_root,
        "sandbox": sandbox,
        "requested": requested,
        "allow_writes": args.allow_writes,
        "results": results,
    });

    let rendered_json = if args.json {
        Some(serde_json::to_string_pretty(&report)?)
    } else {
        None
    };
    if let Some(path) = &sandbox_root
        && !args.keep_sandbox
    {
        std::fs::remove_dir_all(path)?;
    }

    if args.json {
        println!("{}", rendered_json.unwrap());
    } else {
        print_demo_run_report(&report);
    }

    if has_fail {
        anyhow::bail!("one or more demo runs failed");
    }
    if status == "blocked" {
        anyhow::bail!("demo run blocked by project write safety policy");
    }
    Ok(())
}

fn create_demo_sandbox_root() -> Result<PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("jcode-harness-demo-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

fn execute_demo_entry(
    root: &std::path::Path,
    demo: &serde_json::Value,
) -> Result<serde_json::Value> {
    let argv = demo["argv"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("demo entry missing argv array"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| anyhow::anyhow!("demo argv contains non-string value"))
        })
        .collect::<Result<Vec<_>>>()?;
    if argv.is_empty() {
        anyhow::bail!("demo entry has empty argv");
    }

    let exe = std::env::current_exe()?;
    let output = std::process::Command::new(exe)
        .args(argv.iter().skip(1))
        .current_dir(root)
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let expects_json = argv.iter().any(|arg| arg == "--json");
    let json_parseable = if expects_json {
        serde_json::from_str::<serde_json::Value>(&stdout).is_ok()
    } else {
        false
    };
    let pass = output.status.success() && (!expects_json || json_parseable);

    Ok(json!({
        "id": demo["id"],
        "surface": demo["surface"],
        "status": if pass { "pass" } else { "fail" },
        "exit_code": output.status.code(),
        "executed_root": root,
        "project_writes": demo["project_writes"].as_bool().unwrap_or(false),
        "command": demo["command"],
        "json_parseable": json_parseable,
        "stdout": stdout,
        "stderr": stderr,
    }))
}

fn blocked_demo_result(
    root: &std::path::Path,
    demo: &serde_json::Value,
    reason: &str,
) -> serde_json::Value {
    json!({
        "id": demo["id"],
        "surface": demo["surface"],
        "status": "blocked",
        "exit_code": null,
        "executed_root": root,
        "project_writes": demo["project_writes"].as_bool().unwrap_or(false),
        "command": demo["command"],
        "json_parseable": false,
        "stdout": "",
        "stderr": "",
        "reason": reason,
    })
}

fn print_demo_run_report(report: &serde_json::Value) {
    println!(
        "jcode-harness demo run: {}",
        report["status"].as_str().unwrap_or("unknown")
    );
    println!("Offline: true");
    println!("Requested: {}", report["requested"].as_str().unwrap_or(""));
    if report["sandbox"]["enabled"].as_bool().unwrap_or(false) {
        println!(
            "Sandbox: {} ({})",
            report["sandbox"]["path"].as_str().unwrap_or("<unknown>"),
            report["sandbox"]["cleanup"].as_str().unwrap_or("unknown")
        );
    }
    if let Some(results) = report["results"].as_array() {
        for result in results {
            println!(
                "- {}: {}",
                result["id"].as_str().unwrap_or("unknown"),
                result["status"].as_str().unwrap_or("unknown")
            );
            if let Some(reason) = result["reason"].as_str() {
                println!("  reason: {reason}");
            }
            if result["json_parseable"].as_bool().unwrap_or(false) {
                println!("  json: parseable");
            }
        }
    }
}

fn build_demo_manifest(root: &std::path::Path) -> serde_json::Value {
    let root_arg = root.display().to_string();
    let demo_workspace = root.join(".jcode").join("demo").join("smoke");
    json!({
        "status": "ok",
        "offline": true,
        "network_required": false,
        "credentials_required": false,
        "root": root,
        "demos": [
            demo_manifest_entry(
                "safe-eval-profile",
                "safe-eval",
                "Create an isolated trust-center profile for first evaluation.",
                "Safe evaluation can be reproduced without importing existing credentials or long-lived state.",
                vec!["jcode-harness", "safe-eval", "--cwd", &root_arg, "--json"],
                true,
                vec!["profile is safe-eval", "disabled_surfaces includes telemetry and external credential auto-trust", "activation files are written or skipped deterministically"],
                "Writes only under .jcode/safe-eval unless --home points elsewhere."
            ),
            demo_manifest_entry(
                "mock-provider-run-json",
                "mock-provider",
                "Exercise the real Agent runtime with a deterministic provider response.",
                "The run JSON contract can be parsed without network, model credentials, or quota.",
                vec!["jcode-harness", "run", "review this diff", "--json", "--mock-response", "mocked harness response"],
                false,
                vec!["provider is harness-mock", "model is harness-mock-model", "usage token counts are deterministic"],
                "May write normal session metadata under JCODE_HOME, but does not write project files."
            ),
            demo_manifest_entry(
                "memory-llmwiki-bridge",
                "memory",
                "Preview the local llmwiki memory bridge contract.",
                "Memory integration has explicit read/write boundaries before any MCP tool is invoked.",
                vec!["jcode-harness", "skills", "llmwiki-bridge", "--json"],
                false,
                vec!["offline is true", "network_required is false", "commands include wiki_query, wiki_search, wiki_sync, wiki_lint"],
                "This command only prints a contract; it does not call MCP tools."
            ),
            demo_manifest_entry(
                "plan-init-scaffold",
                "plan",
                "Generate deterministic project planning scaffolds.",
                "Plan-first onboarding can be inspected as local files before any model/provider turn.",
                vec!["jcode-harness", "init", "--cwd", &root_arg, "--yes", "--no-memory-wiki", "--json"],
                true,
                vec!["files_written/files_skipped report scaffold changes", "detected_stack is derived from local files", "next_steps are machine-readable"],
                "Use a temporary checkout or safe-eval workspace if you do not want scaffold files in the repo."
            ),
            demo_manifest_entry(
                "swarm-analysis-plan-scaffold",
                "swarm",
                "Create the local init swarm analysis plan artifact.",
                "The swarm bootstrap claim has a reviewable local plan artifact before interactive execution.",
                vec!["jcode-harness", "init", "--cwd", &root_arg, "--yes", "--json"],
                true,
                vec![".jcode/init/SWARM_ANALYSIS_PLAN.md is produced", "no provider is initialized by the CLI scaffold", "operator review happens before execution"],
                "This is the deterministic scaffold side of the interactive /init swarm flow."
            ),
            demo_manifest_entry(
                "browser-safety-doctor",
                "browser",
                "Inspect browser-adjacent safety without opening a browser.",
                "Onboarding diagnostics can report platform/config risk without launching auth or browser integrations.",
                vec!["jcode-harness", "doctor", "--cwd", &root_arg, "--json"],
                false,
                vec!["offline is true", "platform os/arch are reported", "mcp configs are review-only findings"],
                "No browser window is opened; future browser demos should remain opt-in."
            ),
            demo_manifest_entry(
                "skills-router-match",
                "skills",
                "Preview skill routing and scope-policy decisions.",
                "Skill selection is explainable before a model prompt is sent.",
                vec!["jcode-harness", "skills", "match", "fix this Rust bug", "--cwd", &root_arg, "--json"],
                false,
                vec!["selected preserves router order", "policy records selected/skipped decisions", "entries include origin and allowed_tools"],
                "The command only reads skill metadata from built-in and local skill origins."
            ),
            demo_manifest_entry(
                "release-gate-smoke",
                "release-gates",
                "Run the deterministic offline harness smoke gate.",
                "Release claims can include a local tool-execution smoke without providers or network.",
                vec!["jcode-harness", "smoke", "--cwd", &demo_workspace.display().to_string()],
                true,
                vec!["write/read/edit/patch/todo/batch cases pass", "network-backed cases are skipped by default", "deterministic artifacts are created under the demo workspace"],
                "Use a disposable --cwd path because the smoke gate intentionally writes sample artifacts."
            )
        ],
        "recommended_flow": [
            "Run safe-eval first in unfamiliar repositories.",
            "Use mock-provider run JSON/NDJSON demos for README screenshots and CI parser checks.",
            "Use init/smoke demos in a disposable workspace when you need file-system evidence.",
            "Treat browser, memory, and swarm demos as preview contracts until explicit opt-in execution is added."
        ]
    })
}

fn demo_manifest_entry(
    id: &str,
    surface: &str,
    title: &str,
    claim: &str,
    argv: Vec<&str>,
    project_writes: bool,
    expected_evidence: Vec<&str>,
    notes: &str,
) -> serde_json::Value {
    let argv = argv.into_iter().map(ToOwned::to_owned).collect::<Vec<_>>();
    json!({
        "id": id,
        "surface": surface,
        "title": title,
        "claim": claim,
        "command": shell_command(&argv),
        "argv": argv,
        "offline": true,
        "network_required": false,
        "credentials_required": false,
        "project_writes": project_writes,
        "expected_evidence": expected_evidence,
        "notes": notes,
    })
}

fn shell_command(argv: &[String]) -> String {
    argv.iter()
        .map(|part| shell_word(part))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_word(value: &str) -> String {
    if !value.is_empty()
        && value.chars().all(|ch| {
            ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/' | ':' | '=' | '@')
        })
    {
        value.to_string()
    } else {
        shell_quote(value)
    }
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
    let working_dir = std::env::current_dir()?;
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
    let message = match jcode::skill_router::build_skill_preface_for_working_dir(
        &args.goal,
        &args.skill,
        args.skills.into(),
        Some(&working_dir),
    ) {
        Some(preface) => format!("{preface}\n---\n\nTask:\n{}", args.goal),
        None => args.goal.clone(),
    };
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
        SkillsCommand::Scope { command } => run_skills_scope(command),
        SkillsCommand::Import {
            cwd,
            from,
            scope,
            dry_run,
            apply,
            force,
            json,
        } => run_skills_import(cwd, from, scope, dry_run, apply, force, json),
        SkillsCommand::Validate { cwd, json } => run_skills_validate(cwd, json),
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

fn run_skills_scope(command: SkillsScopeCommand) -> Result<()> {
    match command {
        SkillsScopeCommand::Init { cwd, force, json } => {
            let root = resolve_existing_root(cwd.as_deref(), "skills scope init")?;
            let report = jcode::skill_scope::init_policy(&root, force)?;
            print_skill_scope_report(&report, json)
        }
        SkillsScopeCommand::List { cwd, json } => {
            let root = resolve_existing_root(cwd.as_deref(), "skills scope list")?;
            let report = jcode::skill_scope::list_policy(&root)?;
            print_skill_scope_report(&report, json)
        }
        SkillsScopeCommand::Set {
            name,
            state,
            reason,
            cwd,
            json,
        } => {
            let root = resolve_existing_root(cwd.as_deref(), "skills scope set")?;
            let report = jcode::skill_scope::set_skill_state(&root, &name, state.into(), reason)?;
            print_skill_scope_report(&report, json)
        }
    }
}

fn resolve_existing_root(cwd: Option<&str>, label: &str) -> Result<PathBuf> {
    let root = cwd.map(PathBuf::from).unwrap_or(std::env::current_dir()?);
    if !root.is_dir() {
        anyhow::bail!(
            "{label} cwd does not exist or is not a directory: {}",
            root.display()
        );
    }
    Ok(root)
}

fn print_skill_scope_report(
    report: &jcode::skill_scope::SkillScopeReport,
    json: bool,
) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }

    println!("jcode-harness skills scope: {}", report.policy_path);
    println!("Exists: {}", report.exists);
    println!("Created: {}", report.created);
    println!("Updated: {}", report.updated);
    println!("Default state: {}", report.policy.default_state.label());
    if report.policy.skills.is_empty() {
        println!("No explicit skill scope entries.");
    } else {
        println!("Skill scope entries:");
        for entry in &report.policy.skills {
            let reason = entry
                .reason
                .as_deref()
                .map(|reason| format!(" ({reason})"))
                .unwrap_or_default();
            println!("  - {}: {}{}", entry.name, entry.state.label(), reason);
        }
    }
    Ok(())
}

fn run_skills_import(
    cwd: Option<String>,
    from: Vec<PathBuf>,
    scope: HarnessSkillImportScope,
    _dry_run: bool,
    apply: bool,
    force: bool,
    json_output: bool,
) -> Result<()> {
    let root = cwd
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    if !root.is_dir() {
        anyhow::bail!(
            "skills import cwd does not exist or is not a directory: {}",
            root.display()
        );
    }

    let sources = from
        .into_iter()
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                root.join(path)
            }
        })
        .collect();
    let report = jcode::skill_import::run_import(jcode::skill_import::SkillImportOptions {
        root,
        sources,
        scope: scope.into(),
        apply,
        force,
    })?;

    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_skill_import_report(&report);
    }

    if report.should_fail() {
        anyhow::bail!("skill import failed with {} error(s)", report.errors);
    }
    Ok(())
}

fn print_skill_import_report(report: &jcode::skill_import::SkillImportReport) {
    println!("jcode-harness skills import: {}", report.status.label());
    println!("Offline diagnostics: true");
    println!("Dry run: {}", report.dry_run);
    println!(
        "Target: {} ({})",
        report.target.scope.label(),
        report.target.path
    );
    println!(
        "Planned: {} write(s), copied {}, skipped {}",
        report.planned, report.copied, report.skipped
    );
    println!(
        "Findings: {} error(s), {} warning(s)",
        report.errors, report.warnings
    );
    println!("Sources:");
    for source in &report.sources {
        println!(
            "  - {}: {} checked, exists={} ({})",
            source.origin, source.checked, source.exists, source.path
        );
    }

    if report.actions.is_empty() {
        println!("No skill import actions planned.");
        return;
    }

    println!("Actions:");
    for action in &report.actions {
        let name = action.name.as_deref().unwrap_or("<invalid>");
        let reason = action
            .reason
            .as_ref()
            .map(|reason| format!(" ({reason})"))
            .unwrap_or_default();
        println!(
            "  - {} {} -> {} [{} applied={}]{}",
            action.action.label(),
            action.source_path,
            action.target_path,
            name,
            action.applied,
            reason
        );
    }
}

fn run_skills_validate(cwd: Option<String>, json_output: bool) -> Result<()> {
    let root = cwd
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    if !root.is_dir() {
        anyhow::bail!(
            "skills validate cwd does not exist or is not a directory: {}",
            root.display()
        );
    }

    let report = jcode::skill_validation::validate_for_working_dir(&root)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_skill_validation_report(&report);
    }

    if report.should_fail() {
        anyhow::bail!("skill validation failed with {} error(s)", report.errors);
    }
    Ok(())
}

fn print_skill_validation_report(report: &jcode::skill_validation::SkillValidationReport) {
    println!("jcode-harness skills validate: {}", report.status.label());
    println!("Offline diagnostics: true");
    println!("Root: {}", report.root);
    println!(
        "Skills checked: {} (valid {}, invalid {})",
        report.checked, report.valid, report.invalid
    );
    println!(
        "Findings: {} error(s), {} warning(s)",
        report.errors, report.warnings
    );

    println!("Origins:");
    for origin in &report.origins {
        println!(
            "  - {}: {} checked, exists={} ({})",
            origin.origin, origin.checked, origin.exists, origin.path
        );
    }

    if report.findings.is_empty() {
        println!("No findings.");
        return;
    }

    println!("Findings detail:");
    for finding in &report.findings {
        println!(
            "  - [{}] {} {}: {}",
            finding.severity.label(),
            finding.code,
            finding.path,
            finding.message
        );
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
    let root = working_dir.clone().unwrap_or(std::env::current_dir()?);
    let registry = jcode::skill::SkillRegistry::load_for_working_dir(Some(&root))?;
    let raw_selected = jcode::skill_router::select_skills(goal, explicit, mode);
    let scope_selection =
        jcode::skill_scope::apply_policy_for_selection(&root, raw_selected, explicit)?;
    let selected = scope_selection.selected_names();

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
                "policy": scope_selection,
            }))?
        );
        return Ok(());
    }

    if selected.is_empty() {
        println!("No skills selected for this task.");
        if !scope_selection.skipped.is_empty() {
            println!("Skipped by scope policy:");
            for decision in scope_selection.skipped {
                println!(
                    "- {}\t{}\t{}",
                    decision.name,
                    decision.state.label(),
                    decision.reason.unwrap_or_default()
                );
            }
        }
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
    if !scope_selection.skipped.is_empty() {
        println!("Skipped by scope policy:");
        for decision in scope_selection.skipped {
            println!(
                "- {}\t{}\t{}",
                decision.name,
                decision.state.label(),
                decision.reason.unwrap_or_default()
            );
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
