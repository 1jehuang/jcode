use clap::{Parser, Subcommand, ValueEnum};

use super::provider_init::ProviderChoice;

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum TranscriptModeArg {
    Insert,
    Append,
    Replace,
    Send,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum GoogleAccessTierArg {
    Full,
    Readonly,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum ProviderAuthArg {
    /// Send the API key as Authorization: Bearer <key> (OpenAI-compatible default)
    Bearer,
    /// Send the API key in an API-key header (defaults to api-key)
    ApiKey,
    /// Do not send authentication, useful for localhost model servers
    None,
}

#[derive(Parser, Debug)]
#[command(name = "carpai")]
#[command(version = env!("JCODE_VERSION"))]
#[command(about = "CarpAI: A coding agent powered by AI")]
pub(crate) struct Args {
    /// Provider to use (jcode, claude, openai, openai-api, openrouter, azure, opencode, opencode-go, zai, 302ai, baseten, cortecs, comtegra, deepseek, firmware, huggingface, moonshotai, nebius, scaleway, stackit, groq, mistral, perplexity, togetherai, deepinfra, xai, lmstudio, ollama, chutes, cerebras, alibaba-coding-plan, openai-compatible, cursor, copilot, gemini, antigravity, google, or auto-detect)
    #[arg(short, long, default_value = "auto", global = true)]
    pub(crate) provider: ProviderChoice,

    /// Working directory
    #[arg(short = 'C', long, global = true)]
    pub(crate) cwd: Option<String>,

    /// Skip the automatic update check
    #[arg(long, global = true)]
    pub(crate) no_update: bool,

    /// Auto-update when new version is available (default: true for release builds)
    #[arg(long, global = true, default_value = "true")]
    pub(crate) auto_update: bool,

    /// Log tool inputs/outputs and token usage to stderr
    #[arg(long, global = true)]
    pub(crate) trace: bool,

    /// Suppress non-error CLI/status output for scripting and wrappers
    #[arg(long, global = true)]
    pub(crate) quiet: bool,

    /// Resume a session by ID, or list sessions if no ID provided
    #[arg(long, global = true, num_args = 0..=1, default_missing_value = "")]
    pub(crate) resume: Option<String>,

    /// Internal: launched as a freshly spawned window, so skip heavy local resume bootstrap.
    #[arg(long, global = true, hide = true)]
    pub(crate) fresh_spawn: bool,

    /// Disable auto-detection of jcode repository and self-dev mode
    #[arg(long, global = true)]
    pub(crate) no_selfdev: bool,

    /// Custom socket path for server/client communication
    #[arg(long, global = true)]
    pub(crate) socket: Option<String>,

    /// Enable debug socket (broadcasts all TUI state changes)
    #[arg(long, global = true)]
    pub(crate) debug_socket: bool,

    /// Model to use (e.g., claude-opus-4-6, gpt-5.5)
    #[arg(short, long, global = true)]
    pub(crate) model: Option<String>,

    /// Named provider profile from [providers.<name>] in config.toml.
    /// Implies --provider openai-compatible for OpenAI-compatible profiles.
    #[arg(long, global = true)]
    pub(crate) provider_profile: Option<String>,

    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Command {
    /// Start the agent server (background daemon)
    Serve {
        /// Internal: mark this server as temporary so it can self-clean when its owner exits.
        #[arg(long, hide = true)]
        temporary_server: bool,

        /// Internal: owning process pid for a temporary server.
        #[arg(long, hide = true)]
        owner_pid: Option<u32>,

        /// Internal: idle shutdown timeout in seconds for a temporary server.
        #[arg(long, hide = true)]
        temp_idle_timeout_secs: Option<u64>,
    },

    /// Connect to a running server
    Connect,

    /// Run a single message and exit
    Run {
        /// Emit a machine-readable JSON result instead of streaming text
        #[arg(long, conflicts_with = "ndjson")]
        json: bool,

        /// Emit newline-delimited JSON events while the response streams
        #[arg(long, conflicts_with = "json")]
        ndjson: bool,

        /// The message to send
        message: String,
    },

    /// Login to a provider via OAuth, API key, or local credentials
    Login {
        /// Account label for multi-account support (stored labels are auto-numbered)
        #[arg(long, short = 'a')]
        account: Option<String>,

        /// Do not try to open a browser locally. Useful over SSH or on headless machines.
        #[arg(long, alias = "headless")]
        no_browser: bool,

        /// Print a script-friendly auth URL and persist temporary login state for later completion.
        #[arg(long, conflicts_with_all = ["callback_url", "auth_code"])]
        print_auth_url: bool,

        /// Complete a previously printed auth flow using a full callback URL or query string.
        #[arg(long, conflicts_with = "auth_code")]
        callback_url: Option<String>,

        /// Complete a previously printed auth flow using a provider-issued authorization code.
        #[arg(long, conflicts_with = "callback_url")]
        auth_code: Option<String>,

        /// Emit machine-readable JSON for script-friendly login flows.
        #[arg(long)]
        json: bool,

        /// Resume a pending scriptable login flow that does not require callback/code input.
        #[arg(long, conflicts_with_all = ["print_auth_url", "callback_url", "auth_code"])]
        complete: bool,

        /// Gmail/Google access tier for non-interactive flows. Defaults to full.
        #[arg(long, value_enum)]
        google_access_tier: Option<GoogleAccessTierArg>,

        /// OpenAI-compatible API base URL. Used with --provider openai-compatible/custom profiles.
        #[arg(long)]
        api_base: Option<String>,

        /// OpenAI-compatible API key. If omitted, jcode prompts securely when needed.
        #[arg(long)]
        api_key: Option<String>,

        /// Environment variable name to store/use for an OpenAI-compatible API key.
        #[arg(long)]
        api_key_env: Option<String>,
    },

    /// Run in simple REPL mode (no TUI)
    Repl,

    /// Update jcode to the latest version
    Update,

    /// Show build/version information in human or JSON form
    Version {
        /// Emit JSON instead of plain text
        #[arg(long)]
        json: bool,
    },

    /// Show usage limits for connected providers
    Usage {
        /// Emit JSON instead of plain text
        #[arg(long)]
        json: bool,
    },

    /// Self-development mode: run as a canary session on the shared server
    #[command(alias = "selfdev")]
    SelfDev {
        /// Build and test a new canary version before launching
        #[arg(long)]
        build: bool,
    },

    /// Debug socket CLI - interact with running jcode server
    Debug {
        /// Debug command to run (list, start, sessions, create_session, message, tool, state, history, etc.)
        #[arg(default_value = "help")]
        command: String,

        /// Optional argument for the command
        #[arg(default_value = "")]
        arg: String,

        /// Target a specific session by ID
        #[arg(short = 'S', long)]
        session: Option<String>,

        /// Connect to specific server socket path
        #[arg(short = 's', long)]
        socket: Option<String>,

        /// Wait for response to complete (for message command)
        #[arg(short, long)]
        wait: bool,
    },

    /// Authentication status and validation helpers
    #[command(subcommand)]
    Auth(AuthCommand),

    /// Provider discovery and selection helpers
    #[command(subcommand)]
    Provider(ProviderCommand),

    /// Memory management commands
    #[command(subcommand)]
    Memory(MemoryCommand),

    /// Session management commands
    #[command(subcommand)]
    Session(SessionCommand),

    /// Ambient mode management
    #[command(subcommand)]
    Ambient(AmbientCommand),

    /// Generate a pairing code for iOS/web client
    Pair {
        /// List paired devices instead of generating a code
        #[arg(long)]
        list: bool,

        /// Revoke a paired device by name or ID
        #[arg(long)]
        revoke: Option<String>,
    },

    /// Review and respond to pending ambient permission requests
    Permissions,

    /// Inject externally transcribed text into the active Jcode TUI
    Transcript {
        /// Transcript text. If omitted, reads from stdin.
        text: Option<String>,

        /// How to apply the transcript inside Jcode
        #[arg(long, value_enum, default_value = "send")]
        mode: TranscriptModeArg,

        /// Target a specific live session instead of the active TUI
        #[arg(short = 'S', long)]
        session: Option<String>,
    },

    /// Run configured dictation: send to last-focused jcode client or type raw text
    Dictate {
        /// Type the transcript into the focused app instead of sending to jcode
        #[arg(long)]
        r#type: bool,
    },

    /// Set up a global hotkey (Alt+;) to launch jcode
    SetupHotkey {
        /// Internal: run as the macOS hotkey listener process.
        #[arg(long, hide = true)]
        listen_macos_hotkey: bool,
    },

    /// Install a launcher so jcode appears in your app launcher
    SetupLauncher,

    /// Browser automation setup and status
    Browser {
        /// Action (setup, status)
        #[arg(default_value = "setup")]
        action: String,
    },

    /// Replay a saved session in the TUI
    Replay {
        /// Session ID, name, or path to session JSON file
        session: String,

        /// Replay related swarm sessions together in a synchronized multi-pane view
        #[arg(long)]
        swarm: bool,

        /// Export timeline as JSON instead of playing
        #[arg(long)]
        export: bool,

        /// Playback speed multiplier (default: 1.0)
        #[arg(long, default_value = "1.0")]
        speed: f64,

        /// Path to an edited timeline JSON file (overrides session timing)
        #[arg(long)]
        timeline: Option<String>,

        /// Auto-edit timeline: compress tool call wait times and gaps between prompts
        #[arg(long)]
        auto_edit: bool,

        /// Export as video file (auto-generates name if no path given)
        #[arg(long, default_missing_value = "auto", num_args = 0..=1)]
        video: Option<String>,

        /// Video width in columns (default: 120)
        #[arg(long, default_value = "120")]
        cols: u16,

        /// Video height in rows (default: 40)
        #[arg(long, default_value = "40")]
        rows: u16,

        /// Video frames per second (default: 60)
        #[arg(long, default_value = "60")]
        fps: u32,

        /// Force centered layout (overrides config)
        #[arg(long, conflicts_with = "no_centered")]
        centered: bool,

        /// Force left-aligned (non-centered) layout (overrides config)
        #[arg(long, conflicts_with = "centered")]
        no_centered: bool,
    },

    /// Model management commands
    #[command(subcommand)]
    Model(ModelCommand),

    /// Test authentication end-to-end: login (optional), credential probe, refresh, and provider smoke
    AuthTest {
        /// Run the provider login flow before validation (interactive/browser-based)
        #[arg(long)]
        login: bool,

        /// Test all currently configured supported auth providers instead of just --provider
        #[arg(long)]
        all_configured: bool,

        /// Skip the provider runtime smoke prompt
        #[arg(long)]
        no_smoke: bool,

        /// Skip the tool-enabled runtime smoke prompt (the same request path used during normal chat)
        #[arg(long)]
        no_tool_smoke: bool,

        /// Custom smoke prompt (default asks for AUTH_TEST_OK)
        #[arg(long)]
        prompt: Option<String>,

        /// Emit JSON report instead of human-readable output
        #[arg(long)]
        json: bool,

        /// Write the full auth-test report JSON to a file
        #[arg(long)]
        output: Option<String>,
    },

    /// Build mode: plan + execute + verify pipeline
    Build {
        /// The build request / goal description
        message: Option<String>,

        /// Disable auto-approve (ask for each step)
        #[arg(long)]
        manual: bool,

        /// Skip post-build micro-ci verification
        #[arg(long)]
        no_verify: bool,

        /// Max retries per failed step
        #[arg(long, default_value = "3")]
        max_retries: u32,

        /// Build in release/optimized mode
        #[arg(long)]
        release: bool,

        /// Clean build artifacts before building
        #[arg(long)]
        clean: bool,

        /// Build only the specified target (e.g. binary name, package)
        #[arg(long)]
        target: Option<String>,

        /// Build all projects in the workspace
        #[arg(long)]
        all_projects: bool,

        /// Run tests after building
        #[arg(long)]
        test: bool,

        /// Build projects in parallel (only with --all-projects)
        #[arg(long)]
        parallel: bool,

        /// Number of parallel jobs (for supported build systems)
        #[arg(long)]
        jobs: Option<usize>,
    },

    /// Manage MCP servers (add, remove, list, serve, etc.)
    #[command(subcommand)]
    Mcp(McpCommand),

    /// Run system diagnostics and health checks
    Doctor {
        /// Emit JSON report
        #[arg(long)]
        json: bool,
    },

    /// Initialize a project in the current directory
    Init {
        /// Project type (auto-detect if omitted)
        #[arg(long)]
        project_type: Option<String>,

        /// Create a minimal project structure
        #[arg(long)]
        scaffold: bool,
    },

    /// Save or restore the current set of open jcode windows across a system reboot
    Restart {
        #[command(subcommand)]
        action: RestartCommand,
    },

    /// Analyze code value using six-dimension classification
    /// (预留/遗留/缺失功能/无效/重复/冗余)
    CodeValue {
        /// Path to cargo check JSON output file.
        /// If omitted, runs `cargo check` in the current directory.
        #[arg(short, long)]
        input: Option<String>,

        /// Cargo manifest path (Cargo.toml) for running cargo check.
        #[arg(long, default_value = "Cargo.toml")]
        manifest_path: String,

        /// Emit JSON report instead of human-readable output.
        #[arg(long)]
        json: bool,

        /// Output file path to write the report JSON.
        #[arg(short, long)]
        output: Option<String>,
    },

    /// Skill management: list, search, info
    #[command(subcommand)]
    Skills(SkillsCommand),

    /// Workflow management: list, templates, run
    #[command(subcommand)]
    Workflows(WorkflowsCommand),

    /// Task management: list, create, plan, status
    #[command(subcommand)]
    Tasks(TasksCommand),

    /// Git operations: branch, diff, context, status
    #[command(subcommand)]
    Git(GitCommand),

    /// Configuration management: get, set, list
    #[command(subcommand)]
    Config(ConfigCommand),

    /// Commit code with AI assistance
    Commit {
        /// Commit message (auto-generated if omitted)
        #[arg(short, long)]
        message: Option<String>,

        /// Files to stage (defaults to all tracked changes)
        #[arg(short, long)]
        files: Vec<String>,

        /// Skip AI message generation, use the provided message directly
        #[arg(long)]
        no_ai: bool,
    },

    /// Session management: info, export, resume
    #[command(subcommand)]
    SessionMgmt(SessionSubCommand),

    /// Re-analyze and rethink the current context
    Rethink {
        /// Rethink mode: quick, deep, or thinkback
        #[arg(short, long)]
        mode: Option<String>,

        /// Analysis depth (1-5)
        #[arg(short, long, default_value_t = 3)]
        depth: u32,
    },

    /// Compact context to reduce token usage
    Compact {
        /// Compact mode: summary, compress, or auto
        #[arg(short, long)]
        mode: Option<String>,

        /// Target token count
        #[arg(short, long)]
        target: Option<usize>,

        /// Output compacted result as JSON
        #[arg(long)]
        json: bool,
    },

    /// Fork current session into a new branch
    Fork {
        /// Name for the forked session
        #[arg(short, long)]
        name: Option<String>,

        /// Start from a specific checkpoint
        #[arg(short, long)]
        checkpoint: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum RestartCommand {
    /// Save a reboot snapshot of currently active jcode windows
    Save {
        /// Restore this reboot snapshot automatically the next time plain `jcode` starts
        #[arg(long)]
        auto_restore: bool,
    },
    /// Restore the most recently saved reboot snapshot
    Restore,
    /// Show the currently saved reboot snapshot
    Status,
    /// Remove the currently saved reboot snapshot
    Clear,
}

#[derive(Subcommand, Debug)]
pub(crate) enum SessionSubCommand {
    /// Show current session info
    Info,

    /// Export session context to a file
    Export {
        /// Output file path
        #[arg(short, long, default_value = "session_export.md")]
        output: String,

        /// Include full context (conversation, files, state)
        #[arg(long)]
        full: bool,
    },

    /// Resume a previous session
    Resume {
        /// Session ID to resume
        #[arg(short, long)]
        id: Option<String>,

        /// List available sessions
        #[arg(short, long)]
        list: bool,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum ModelCommand {
    /// List model names you can pass to -m/--model
    List {
        /// Emit JSON instead of plain text
        #[arg(long)]
        json: bool,

        /// Show provider/selection summary before the list
        #[arg(long)]
        verbose: bool,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum SessionCommand {
    /// Rename a saved session's human-readable name/title
    Rename {
        /// Session ID or memorable short name, e.g. fox
        session: String,

        /// New session name/title
        #[arg(required_unless_present = "clear")]
        name: Option<String>,

        /// Clear the custom session name/title
        #[arg(long, conflicts_with = "name")]
        clear: bool,

        /// Emit JSON instead of human-readable output
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum ProviderCommand {
    /// List provider IDs you can pass to -p/--provider
    List {
        /// Emit JSON instead of plain text
        #[arg(long)]
        json: bool,
    },

    /// Show the currently requested and resolved provider selection
    Current {
        /// Emit JSON instead of plain text
        #[arg(long)]
        json: bool,
    },

    /// Add a named OpenAI-compatible API provider profile
    Add {
        /// Profile name used with --provider-profile and config defaults, e.g. my-gateway
        name: String,

        /// OpenAI-compatible API base URL, e.g. https://llm.example.com/v1
        #[arg(long, alias = "api-base")]
        base_url: String,

        /// Default model id for this provider profile
        #[arg(short, long)]
        model: String,

        /// Optional model context window in tokens
        #[arg(long)]
        context_window: Option<usize>,

        /// Environment variable name that contains the API key
        #[arg(long, conflicts_with = "no_api_key")]
        api_key_env: Option<String>,

        /// API key value to store in jcode's private provider env file. Prefer --api-key-stdin for shell history safety.
        #[arg(long, conflicts_with_all = ["api_key_stdin", "no_api_key"])]
        api_key: Option<String>,

        /// Read the API key from stdin and store it in jcode's private provider env file
        #[arg(long, conflicts_with = "no_api_key")]
        api_key_stdin: bool,

        /// Configure the provider with no API key/authentication
        #[arg(long, conflicts_with_all = ["api_key", "api_key_stdin", "api_key_env"])]
        no_api_key: bool,

        /// Authentication style for the API key
        #[arg(long, value_enum)]
        auth: Option<ProviderAuthArg>,

        /// Header name when --auth api-key is used (default: api-key)
        #[arg(long)]
        auth_header: Option<String>,

        /// Private env file name under jcode's app config directory for stored API keys
        #[arg(long)]
        env_file: Option<String>,

        /// Make this profile the startup default provider/model
        #[arg(long, alias = "default")]
        set_default: bool,

        /// Replace an existing profile with the same name
        #[arg(long)]
        overwrite: bool,

        /// Allow provider-routing features for OpenRouter-style gateways
        #[arg(long)]
        provider_routing: bool,

        /// Fetch/list models from the provider's /models endpoint
        #[arg(long)]
        model_catalog: bool,

        /// Emit JSON instead of human-readable setup output
        #[arg(long)]
        json: bool,
    },
}

/// MCP server management commands.
#[derive(Subcommand, Debug)]
pub(crate) enum McpCommand {
    /// Start CarpAI as an MCP server (for IDE integration)
    Serve {
        /// Enable debug output
        #[arg(short, long)]
        debug: bool,

        /// Override verbose mode setting
        #[arg(long)]
        verbose: bool,
    },

    /// Add an MCP server configuration
    Add {
        /// Server name
        name: String,

        /// Server command (for stdio) or URL (for SSE/HTTP)
        command_or_url: String,

        /// Additional arguments to the command
        args: Vec<String>,

        /// Configuration scope (local, user, or project)
        #[arg(short, long, default_value = "local")]
        scope: String,

        /// Transport type: stdio, sse, streamable-http
        #[arg(short, long, default_value = "stdio")]
        transport: String,

        /// Environment variables (KEY=VALUE)
        #[arg(short = 'e', long)]
        env: Vec<String>,
    },

    /// Add an MCP server from a JSON config string
    AddJson {
        /// Server name
        name: String,

        /// JSON configuration string
        json: String,

        /// Configuration scope (local, user, or project)
        #[arg(short, long, default_value = "local")]
        scope: String,
    },

    /// Remove an MCP server
    Remove {
        /// Server name to remove
        name: String,

        /// Configuration scope (local, user, or project)
        #[arg(short, long)]
        scope: Option<String>,
    },

    /// List configured MCP servers
    List,

    /// Get details about an MCP server
    Get {
        /// Server name
        name: String,
    },

    /// Import MCP servers from Claude Desktop config
    ImportDesktop {
        /// Configuration scope (local, user, or project)
        #[arg(short, long, default_value = "local")]
        scope: String,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum AuthCommand {
    /// Show configured authentication status for model/tool providers
    Status {
        /// Emit JSON instead of plain text
        #[arg(long)]
        json: bool,
    },
    /// Diagnose provider auth issues and suggest next steps
    Doctor {
        /// Optional provider id or alias to focus diagnosis on one provider
        #[arg(id = "auth_provider", value_name = "PROVIDER")]
        provider: Option<String>,

        /// Run live post-login validation for configured providers during diagnosis
        #[arg(long)]
        validate: bool,

        /// Emit JSON instead of plain text
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum AmbientCommand {
    /// Show ambient mode status
    Status,
    /// Show recent ambient activity log
    Log,
    /// Manually trigger an ambient cycle
    Trigger,
    /// Stop ambient mode
    Stop,
    /// Run an ambient cycle in a visible TUI (internal, spawned by the ambient runner)
    #[command(hide = true)]
    RunVisible,
}

#[derive(Subcommand, Debug)]
pub(crate) enum MemoryCommand {
    /// List all stored memories
    List {
        /// Filter by scope (project, global, all)
        #[arg(short, long, default_value = "all")]
        scope: String,

        /// Filter by tag
        #[arg(short, long)]
        tag: Option<String>,
    },

    /// Search memories by query
    Search {
        /// Search query
        query: String,

        /// Use semantic search (embedding-based) instead of keyword
        #[arg(short, long)]
        semantic: bool,
    },

    /// Export memories to a JSON file
    Export {
        /// Output file path
        output: String,

        /// Export scope (project, global, all)
        #[arg(short, long, default_value = "all")]
        scope: String,
    },

    /// Import memories from a JSON file
    Import {
        /// Input file path
        input: String,

        /// Import scope (project, global)
        #[arg(short, long, default_value = "project")]
        scope: String,

        /// Overwrite existing memories with same ID
        #[arg(long)]
        overwrite: bool,
    },

    /// Show memory statistics
    Stats,

    /// Clear test memory storage (used by debug sessions)
    ClearTest,
}

/// Skills management commands.
#[derive(Subcommand, Debug)]
pub(crate) enum SkillsCommand {
    /// List available skills
    List {
        /// Emit JSON instead of plain text
        #[arg(long)]
        json: bool,
    },
    /// Search skills by query
    Search {
        query: String,
    },
    /// Show detailed info about a skill
    Info {
        skill: String,
    },
}

/// Workflow management commands.
#[derive(Subcommand, Debug)]
pub(crate) enum WorkflowsCommand {
    /// List available workflow templates
    List {
        /// Emit JSON instead of plain text
        #[arg(long)]
        json: bool,
    },
    /// Show workflow template info
    Templates {
        /// Template name
        name: Option<String>,
    },
    /// Run a workflow
    Run {
        /// Workflow name or template
        workflow: String,
    },
}

/// Task management commands.
#[derive(Subcommand, Debug)]
pub(crate) enum TasksCommand {
    /// List tasks
    List {
        /// Filter by status
        #[arg(short, long)]
        status: Option<String>,
    },
    /// Create a new task
    Create {
        /// Task description
        description: String,
    },
    /// Plan task execution
    Plan {
        /// Task ID
        id: String,
    },
    /// Show task status
    Status {
        /// Task ID
        id: String,
    },
}

/// Git operations commands.
#[derive(Subcommand, Debug)]
pub(crate) enum GitCommand {
    /// Show current branch info
    Branch,
    /// Show git diff
    Diff {
        /// Path to show diff for
        path: Option<String>,
    },
    /// Show git context (branch, status, recent commits)
    Context,
    /// Show git status
    Status,
}

/// Configuration management commands.
#[derive(Subcommand, Debug)]
pub(crate) enum ConfigCommand {
    /// Get a configuration value
    Get {
        /// Config key
        key: String,
    },
    /// Set a configuration value
    Set {
        /// Config key
        key: String,
        /// Config value
        value: String,
    },
    /// List all configuration
    List {
        /// Emit JSON instead of plain text
        #[arg(long)]
        json: bool,
    },
}

#[cfg(test)]
mod tests;
