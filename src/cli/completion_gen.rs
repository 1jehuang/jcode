//! Shell completion generator — multi-shell deep tree completion
//!
//! Extracted from commands.rs for better modularity.

// Shell completion — multi-shell, deep tree, flags+descriptions+dynamic
// ════════════════════════════════════════════════════════════════════

/// Metadata for a single command/flag — mirrors clap structure
#[derive(Debug, Clone)]
struct CmdMeta {
    name: &'static str,
    desc: &'static str,
    subcommands: &'static [CmdMeta],
    flags: &'static [FlagMeta],
}

#[derive(Debug, Clone)]
struct FlagMeta {
    short: Option<char>,
    long: &'static str,
    desc: &'static str,
    /// None = bool flag; Some("file") / "string" / "number" / "enum:a,b"
    value_hint: Option<&'static str>,
}

/// Full CarpAI command tree — mirrors `src/cli/args.rs`.
const COMMAND_TREE: &[CmdMeta] = &[
    CmdMeta { name: "serve", desc: "Start the agent server (background daemon)", subcommands: &[], flags: &[
        FlagMeta { short: None, long: "temporary-server", desc: "Mark as temporary server", value_hint: None },
        FlagMeta { short: None, long: "owner-pid", desc: "Owning process PID", value_hint: Some("number") },
        FlagMeta { short: None, long: "temp-idle-timeout-secs", desc: "Idle shutdown timeout", value_hint: Some("number") },
    ]},
    CmdMeta { name: "connect", desc: "Connect to a running server", subcommands: &[], flags: &[] },
    CmdMeta { name: "run", desc: "Run a single message and exit", subcommands: &[], flags: &[
        FlagMeta { short: None, long: "json", desc: "Emit JSON result", value_hint: None },
        FlagMeta { short: None, long: "ndjson", desc: "Emit NDJSON events", value_hint: None },
    ]},
    CmdMeta { name: "login", desc: "Login to a provider via OAuth / API key", subcommands: &[], flags: &[
        FlagMeta { short: Some('a'), long: "account", desc: "Account label for multi-account", value_hint: Some("string") },
        FlagMeta { short: None, long: "no-browser", desc: "Do not open browser (headless)", value_hint: None },
        FlagMeta { short: None, long: "print-auth-url", desc: "Print auth URL for scripts", value_hint: None },
        FlagMeta { short: None, long: "callback-url", desc: "Complete auth via callback URL", value_hint: Some("string") },
        FlagMeta { short: None, long: "auth-code", desc: "Complete auth via auth code", value_hint: Some("string") },
        FlagMeta { short: None, long: "json", desc: "Emit JSON output", value_hint: None },
        FlagMeta { short: None, long: "complete", desc: "Resume pending login flow", value_hint: None },
        FlagMeta { short: None, long: "google-access-tier", desc: "Gmail access tier", value_hint: Some("enum:full,readonly") },
        FlagMeta { short: None, long: "api-base", desc: "OpenAI-compatible API base URL", value_hint: Some("string") },
        FlagMeta { short: None, long: "api-key", desc: "OpenAI-compatible API key", value_hint: Some("string") },
        FlagMeta { short: None, long: "api-key-env", desc: "Env var name for API key", value_hint: Some("string") },
    ]},
    CmdMeta { name: "repl", desc: "Run in simple REPL mode (no TUI)", subcommands: &[], flags: &[] },
    CmdMeta { name: "update", desc: "Update CarpAI to the latest version", subcommands: &[], flags: &[] },
    CmdMeta { name: "version", desc: "Show build/version information", subcommands: &[], flags: &[
        FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
    ]},
    CmdMeta { name: "usage", desc: "Show usage limits for connected providers", subcommands: &[], flags: &[
        FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
    ]},
    CmdMeta { name: "selfdev", desc: "Self-development / canary mode", subcommands: &[], flags: &[
        FlagMeta { short: None, long: "build", desc: "Build new canary version", value_hint: None },
    ]},
    CmdMeta { name: "debug", desc: "Debug socket CLI — interact with server", subcommands: &[], flags: &[
        FlagMeta { short: Some('S'), long: "session", desc: "Target session ID", value_hint: Some("string") },
        FlagMeta { short: Some('s'), long: "socket", desc: "Server socket path", value_hint: Some("file") },
        FlagMeta { short: Some('w'), long: "wait", desc: "Wait for response to complete", value_hint: None },
    ]},
    CmdMeta { name: "auth", desc: "Authentication status & validation", subcommands: &[
        CmdMeta { name: "status", desc: "Show configured auth status", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
        ]},
        CmdMeta { name: "doctor", desc: "Diagnose provider auth issues", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "validate", desc: "Run live validation", value_hint: None },
            FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
        ]},
    ], flags: &[]},
    CmdMeta { name: "provider", desc: "Provider discovery & selection", subcommands: &[
        CmdMeta { name: "list", desc: "List provider IDs", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
        ]},
        CmdMeta { name: "current", desc: "Show current provider/model", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
        ]},
        CmdMeta { name: "add", desc: "Add OpenAI-compatible provider profile", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "base-url", desc: "API base URL", value_hint: Some("string") },
            FlagMeta { short: Some('m'), long: "model", desc: "Default model ID", value_hint: Some("string") },
            FlagMeta { short: None, long: "api-key", desc: "API key value", value_hint: Some("string") },
            FlagMeta { short: None, long: "auth", desc: "Auth style (bearer/api-key/none)", value_hint: Some("enum:bearer,api-key,none") },
            FlagMeta { short: None, long: "set-default", desc: "Make this the startup default", value_hint: None },
            FlagMeta { short: None, long: "overwrite", desc: "Replace existing profile", value_hint: None },
            FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
        ]},
    ], flags: &[]},
    CmdMeta { name: "session", desc: "Session operations (rename)", subcommands: &[
        CmdMeta { name: "rename", desc: "Rename a session's display name", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "clear", desc: "Clear custom name", value_hint: None },
            FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
        ]},
    ], flags: &[]},
    CmdMeta { name: "memory", desc: "Memory management commands", subcommands: &[
        CmdMeta { name: "list", desc: "List stored memories", subcommands: &[], flags: &[
            FlagMeta { short: Some('s'), long: "scope", desc: "Scope (project/global/all)", value_hint: Some("enum:project,global,all") },
            FlagMeta { short: Some('t'), long: "tag", desc: "Filter by tag", value_hint: Some("string") },
        ]},
        CmdMeta { name: "search", desc: "Search memories by query", subcommands: &[], flags: &[
            FlagMeta { short: Some('s'), long: "semantic", desc: "Use embedding-based search", value_hint: None },
        ]},
        CmdMeta { name: "export", desc: "Export memories to JSON file", subcommands: &[], flags: &[
            FlagMeta { short: Some('o'), long: "output", desc: "Output file path", value_hint: Some("file") },
            FlagMeta { short: Some('s'), long: "scope", desc: "Export scope", value_hint: Some("enum:project,global,all") },
        ]},
        CmdMeta { name: "import", desc: "Import memories from JSON file", subcommands: &[], flags: &[
            FlagMeta { short: Some('i'), long: "input", desc: "Input file path", value_hint: Some("file") },
            FlagMeta { short: Some('s'), long: "scope", desc: "Import scope", value_hint: Some("enum:project,global") },
            FlagMeta { short: None, long: "overwrite", desc: "Overwrite existing by ID", value_hint: None },
        ]},
        CmdMeta { name: "stats", desc: "Show memory statistics", subcommands: &[], flags: &[] },
        CmdMeta { name: "clear-test", desc: "Clear test memory storage", subcommands: &[], flags: &[] },
    ], flags: &[]},
    CmdMeta { name: "ambient", desc: "Ambient mode management", subcommands: &[
        CmdMeta { name: "status", desc: "Show ambient mode status", subcommands: &[], flags: &[] },
        CmdMeta { name: "log", desc: "Show ambient activity log", subcommands: &[], flags: &[] },
        CmdMeta { name: "trigger", desc: "Manually trigger ambient cycle", subcommands: &[], flags: &[] },
        CmdMeta { name: "stop", desc: "Stop ambient mode", subcommands: &[], flags: &[] },
    ], flags: &[]},
    CmdMeta { name: "pair", desc: "Generate pairing code for iOS/web", subcommands: &[], flags: &[
        FlagMeta { short: None, long: "list", desc: "List paired devices", value_hint: None },
        FlagMeta { short: None, long: "revoke", desc: "Revoke a device by name/ID", value_hint: Some("string") },
    ]},
    CmdMeta { name: "permissions", desc: "Review pending ambient permission requests", subcommands: &[], flags: &[] },
    CmdMeta { name: "transcript", desc: "Inject transcribed text into active TUI", subcommands: &[], flags: &[
        FlagMeta { short: None, long: "mode", desc: "Apply mode", value_hint: Some("enum:send,insert,append,replace") },
        FlagMeta { short: Some('S'), long: "session", desc: "Target session ID", value_hint: Some("string") },
    ]},
    CmdMeta { name: "dictate", desc: "Run dictation from configured mic", subcommands: &[], flags: &[
        FlagMeta { short: None, long: "type", desc: "Type into focused app (not jcode)", value_hint: None },
    ]},
    CmdMeta { name: "setup-hotkey", desc: "Install global hotkey (Alt+;) for carpai", subcommands: &[], flags: &[] },
    CmdMeta { name: "setup-launcher", desc: "Install carpai in your app launcher", subcommands: &[], flags: &[] },
    CmdMeta { name: "browser", desc: "Browser automation setup/status", subcommands: &[], flags: &[] },
    CmdMeta { name: "replay", desc: "Replay a saved session in the TUI", subcommands: &[], flags: &[
        FlagMeta { short: None, long: "swarm", desc: "Multi-pane swarm view", value_hint: None },
        FlagMeta { short: None, long: "export", desc: "Export as JSON instead of playing", value_hint: None },
        FlagMeta { short: None, long: "speed", desc: "Playback speed multiplier", value_hint: Some("number") },
        FlagMeta { short: None, long: "video", desc: "Export as video file", value_hint: Some("file") },
        FlagMeta { short: None, long: "cols", desc: "Video width in columns", value_hint: Some("number") },
        FlagMeta { short: None, long: "rows", desc: "Video height in rows", value_hint: Some("number") },
        FlagMeta { short: None, long: "fps", desc: "Video frames per second", value_hint: Some("number") },
        FlagMeta { short: None, long: "auto-edit", desc: "Compress tool-call wait times", value_hint: None },
        FlagMeta { short: None, long: "timeline", desc: "Path to edited timeline JSON", value_hint: Some("file") },
    ]},
    CmdMeta { name: "model", desc: "Model management (list available models)", subcommands: &[
        CmdMeta { name: "list", desc: "List model names for --model", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
            FlagMeta { short: None, long: "verbose", desc: "Show selection summary", value_hint: None },
        ]},
    ], flags: &[]},
    CmdMeta { name: "auth-test", desc: "End-to-end auth test", subcommands: &[], flags: &[
        FlagMeta { short: None, long: "login", desc: "Run login flow first", value_hint: None },
        FlagMeta { short: None, long: "all-configured", desc: "Test all configured providers", value_hint: None },
        FlagMeta { short: None, long: "no-smoke", desc: "Skip provider smoke prompt", value_hint: None },
        FlagMeta { short: None, long: "json", desc: "Emit JSON report", value_hint: None },
        FlagMeta { short: None, long: "output", desc: "Write JSON report to file", value_hint: Some("file") },
    ]},
    CmdMeta { name: "build", desc: "Build mode: plan -> execute -> verify", subcommands: &[], flags: &[
        FlagMeta { short: None, long: "manual", desc: "Ask for each step", value_hint: None },
        FlagMeta { short: None, long: "no-verify", desc: "Skip micro-ci verification", value_hint: None },
        FlagMeta { short: None, long: "max-retries", desc: "Max retries per step", value_hint: Some("number") },
        FlagMeta { short: None, long: "release", desc: "Build in release mode", value_hint: None },
        FlagMeta { short: None, long: "clean", desc: "Clean build artifacts", value_hint: None },
        FlagMeta { short: None, long: "target", desc: "Build target", value_hint: Some("string") },
        FlagMeta { short: None, long: "all-projects", desc: "Build all workspace projects", value_hint: None },
        FlagMeta { short: None, long: "test", desc: "Run tests after build", value_hint: None },
        FlagMeta { short: None, long: "parallel", desc: "Parallel workspace build", value_hint: None },
        FlagMeta { short: None, long: "jobs", desc: "Number of parallel jobs", value_hint: Some("number") },
    ]},
    CmdMeta { name: "mcp", desc: "Manage MCP servers (add / remove / serve / bridge)", subcommands: &[
        CmdMeta { name: "serve", desc: "Start MCP server on stdio", subcommands: &[], flags: &[
            FlagMeta { short: Some('d'), long: "debug", desc: "Enable debug output", value_hint: None },
            FlagMeta { short: None, long: "verbose", desc: "Verbose mode", value_hint: None },
        ]},
        CmdMeta { name: "add", desc: "Add an MCP server config", subcommands: &[], flags: &[
            FlagMeta { short: Some('s'), long: "scope", desc: "Config scope", value_hint: Some("enum:local,user,project") },
            FlagMeta { short: Some('t'), long: "transport", desc: "Transport type", value_hint: Some("enum:stdio,sse,streamable-http") },
            FlagMeta { short: Some('e'), long: "env", desc: "Env vars (KEY=VALUE)", value_hint: Some("string") },
        ]},
        CmdMeta { name: "add-json", desc: "Add MCP server from JSON config", subcommands: &[], flags: &[
            FlagMeta { short: Some('s'), long: "scope", desc: "Config scope", value_hint: Some("enum:local,user,project") },
        ]},
        CmdMeta { name: "remove", desc: "Remove an MCP server", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "scope", desc: "Config scope", value_hint: Some("enum:local,user,project") },
        ]},
        CmdMeta { name: "list", desc: "List configured MCP servers", subcommands: &[], flags: &[] },
        CmdMeta { name: "get", desc: "Get MCP server details", subcommands: &[], flags: &[] },
        CmdMeta { name: "import-desktop", desc: "Import from Claude Desktop config", subcommands: &[], flags: &[
            FlagMeta { short: Some('s'), long: "scope", desc: "Config scope", value_hint: Some("enum:local,user,project") },
        ]},
        CmdMeta { name: "bridge", desc: "Bidirectional MCP bridge (server+client)", subcommands: &[], flags: &[
            FlagMeta { short: Some('d'), long: "debug", desc: "Enable debug output", value_hint: None },
            FlagMeta { short: None, long: "expose-resources", desc: "Expose workspace resources", value_hint: None },
            FlagMeta { short: None, long: "auto-connect", desc: "Auto-connect configured servers", value_hint: None },
            FlagMeta { short: None, long: "status", desc: "Print bridge status only", value_hint: None },
        ]},
    ], flags: &[]},
    CmdMeta { name: "doctor", desc: "Run system diagnostics and health checks", subcommands: &[], flags: &[
        FlagMeta { short: None, long: "json", desc: "Emit JSON report", value_hint: None },
    ]},
    CmdMeta { name: "init", desc: "Initialize a project in the current dir", subcommands: &[], flags: &[
        FlagMeta { short: None, long: "project-type", desc: "Project type", value_hint: Some("enum:rust,node,typescript,react,vue,python,go,c,cpp,java,csharp,ruby") },
        FlagMeta { short: None, long: "scaffold", desc: "Create project files", value_hint: None },
    ]},
    CmdMeta { name: "restart", desc: "Save / restore jcode windows across reboot", subcommands: &[
        CmdMeta { name: "save", desc: "Save reboot snapshot", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "auto-restore", desc: "Restore on next startup", value_hint: None },
        ]},
        CmdMeta { name: "restore", desc: "Restore saved snapshot", subcommands: &[], flags: &[] },
        CmdMeta { name: "status", desc: "Show saved snapshot info", subcommands: &[], flags: &[] },
        CmdMeta { name: "clear", desc: "Clear saved snapshot", subcommands: &[], flags: &[] },
    ], flags: &[]},
    CmdMeta { name: "code-value", desc: "Analyze code quality (6-dimension)", subcommands: &[], flags: &[
        FlagMeta { short: Some('i'), long: "input", desc: "Cargo check JSON path", value_hint: Some("file") },
        FlagMeta { short: None, long: "manifest-path", desc: "Path to Cargo.toml", value_hint: Some("file") },
        FlagMeta { short: None, long: "json", desc: "Emit JSON report", value_hint: None },
        FlagMeta { short: Some('o'), long: "output", desc: "Write report to file", value_hint: Some("file") },
    ]},
    CmdMeta { name: "skills", desc: "Skill management (list, search, info)", subcommands: &[
        CmdMeta { name: "list", desc: "List available skills", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
        ]},
        CmdMeta { name: "search", desc: "Search skills by keyword", subcommands: &[], flags: &[] },
        CmdMeta { name: "info", desc: "Show skill details", subcommands: &[], flags: &[] },
    ], flags: &[]},
    CmdMeta { name: "workflows", desc: "Workflow management (list, run)", subcommands: &[
        CmdMeta { name: "list", desc: "List workflow templates", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
        ]},
        CmdMeta { name: "templates", desc: "Show template info", subcommands: &[], flags: &[] },
        CmdMeta { name: "run", desc: "Run a workflow", subcommands: &[], flags: &[] },
    ], flags: &[]},
    CmdMeta { name: "tasks", desc: "Task management (list, create, plan)", subcommands: &[
        CmdMeta { name: "list", desc: "List tasks", subcommands: &[], flags: &[
            FlagMeta { short: Some('s'), long: "status", desc: "Filter by status", value_hint: Some("string") },
            FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
        ]},
        CmdMeta { name: "create", desc: "Create a new task", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "priority", desc: "Priority", value_hint: Some("enum:low,medium,high") },
        ]},
        CmdMeta { name: "get", desc: "Get task details by ID", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
        ]},
        CmdMeta { name: "plan", desc: "Plan a task's execution", subcommands: &[], flags: &[] },
    ], flags: &[]},
    CmdMeta { name: "git", desc: "Git operations (branch, diff, status)", subcommands: &[
        CmdMeta { name: "branch", desc: "Show current branch", subcommands: &[], flags: &[] },
        CmdMeta { name: "diff", desc: "Show git diff", subcommands: &[], flags: &[] },
        CmdMeta { name: "context", desc: "Show full git context", subcommands: &[], flags: &[] },
        CmdMeta { name: "status", desc: "Show git status", subcommands: &[], flags: &[] },
    ], flags: &[]},
    CmdMeta { name: "config", desc: "Configuration management (get, set, list)", subcommands: &[
        CmdMeta { name: "get", desc: "Get a config value by key", subcommands: &[], flags: &[] },
        CmdMeta { name: "set", desc: "Set a config value", subcommands: &[], flags: &[] },
        CmdMeta { name: "list", desc: "List all config variables", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
        ]},
    ], flags: &[]},
    CmdMeta { name: "commit", desc: "Commit code with AI assistance", subcommands: &[], flags: &[
        FlagMeta { short: Some('m'), long: "message", desc: "Commit message", value_hint: Some("string") },
        FlagMeta { short: Some('f'), long: "files", desc: "Files to stage", value_hint: Some("file") },
        FlagMeta { short: None, long: "no-ai", desc: "Skip AI message generation", value_hint: None },
    ]},
    CmdMeta { name: "session-mgmt", desc: "Session management (info/export/resume)", subcommands: &[
        CmdMeta { name: "info", desc: "Show current session info", subcommands: &[], flags: &[] },
        CmdMeta { name: "export", desc: "Export session context to file", subcommands: &[], flags: &[
            FlagMeta { short: Some('o'), long: "output", desc: "Output file path", value_hint: Some("file") },
            FlagMeta { short: None, long: "full", desc: "Include full context", value_hint: None },
        ]},
        CmdMeta { name: "resume", desc: "Resume a previous session", subcommands: &[], flags: &[
            FlagMeta { short: Some('i'), long: "id", desc: "Session ID", value_hint: Some("string") },
            FlagMeta { short: Some('l'), long: "list", desc: "List available sessions", value_hint: None },
        ]},
    ], flags: &[]},
    CmdMeta { name: "rethink", desc: "Re-analyze context", subcommands: &[], flags: &[
        FlagMeta { short: Some('m'), long: "mode", desc: "Mode (quick/deep/thinkback)", value_hint: Some("enum:quick,deep,thinkback") },
        FlagMeta { short: Some('d'), long: "depth", desc: "Analysis depth 1-5", value_hint: Some("number") },
    ]},
    CmdMeta { name: "compact", desc: "Compact context to reduce tokens", subcommands: &[], flags: &[
        FlagMeta { short: Some('m'), long: "mode", desc: "Mode (summary/compress/auto)", value_hint: Some("enum:summary,compress,auto") },
        FlagMeta { short: Some('t'), long: "target", desc: "Target token count", value_hint: Some("number") },
        FlagMeta { short: None, long: "json", desc: "Output as JSON", value_hint: None },
    ]},
    CmdMeta { name: "fork", desc: "Fork current session into a new branch", subcommands: &[], flags: &[
        FlagMeta { short: Some('n'), long: "name", desc: "Fork name", value_hint: Some("string") },
        FlagMeta { short: Some('c'), long: "checkpoint", desc: "Start checkpoint", value_hint: Some("string") },
    ]},
    CmdMeta { name: "completion", desc: "Generate shell completion scripts", subcommands: &[], flags: &[
        FlagMeta { short: Some('o'), long: "output", desc: "Write to file instead of stdout", value_hint: Some("file") },
    ]},
];

// --- Public entry point ----------------------------------------

/// Generate shell completion scripts.  The only public function — delegated to by dispatch.
pub fn run_completion_command(shell: &str, output_path: Option<&str>) -> Result<()> {
    let resolved = if shell == "auto" || shell.is_empty() {
        detect_current_shell()
    } else {
        shell.to_string()
    };
    let script = match resolved.as_str() {
        "bash"      => generate_bash(),
        "zsh"       => generate_zsh(),
        "fish"      => generate_fish(),
        "powershell" => generate_powershell(),
        _ => return Err(anyhow::anyhow!(
            "Unknown shell: {shell}. Supported: bash, zsh, fish, powershell")),
    };
    match output_path {
        Some(p) => std::fs::write(p, &script)?,
        None    => print!("{script}"),
    }
    Ok(())
}

/// Auto-detect shell and install completion scripts to the correct system path.
/// Uses `$SHELL` env var and known paths for each platform.
pub fn run_completion_install_command(shell: &str) -> Result<()> {
    let shell = shell.to_lowercase();
    let shell_detected = if shell == "auto" || shell.is_empty() {
        detect_current_shell()
    } else {
        shell
    };

    let script = match shell_detected.as_str() {
        "bash"      => generate_bash(),
        "zsh"       => generate_zsh(),
        "fish"      => generate_fish(),
        "powershell" => generate_powershell(),
        other => return Err(anyhow::anyhow!(
            "Cannot auto-install for '{other}'. Try `carpai completion {other} -o <path>` instead")),
    };

    let install_path = detect_install_path(&shell_detected)?;

    // Ensure parent directory exists
    if let Some(parent) = std::path::Path::new(&install_path).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("Cannot create dir '{}': {}", parent.display(), e))?;
    }

    std::fs::write(&install_path, &script)
        .map_err(|e| anyhow::anyhow!("Cannot write to '{}': {}", install_path, e))?;

    eprintln!("✅ Completion script installed for {shell_detected}");
    eprintln!("   Path: {install_path}");
    if shell_detected == "bash" {
        eprintln!("   Activate: source {install_path}");
        eprintln!("   Or add to ~/.bashrc: [[ -f {install_path} ]] && source {install_path}");
    } else if shell_detected == "zsh" {
        eprintln!("   Activate: compinit && source {install_path}");
    } else if shell_detected == "fish" {
        eprintln!("   Activate: fish will auto-source from $fish_complete_path");
    } else if shell_detected == "powershell" {
        eprintln!("   Activate: Add-Content -Path $PROFILE -Value '. \"{install_path}\"'");
    }
    Ok(())
}

fn detect_current_shell() -> String {
    // Prefer SHELL env var
    if let Ok(shell) = std::env::var("SHELL") {
        if shell.ends_with("bash") { return "bash".into(); }
        if shell.ends_with("zsh")  { return "zsh".into(); }
        if shell.ends_with("fish") { return "fish".into(); }
    }
    // Windows: prefer PowerShell
    if cfg!(windows) {
        return "powershell".into();
    }
    // Fallback: check /proc/self/exe or default
    "bash".into()
}

fn detect_install_path(shell: &str) -> Result<String> {
    match shell {
        "bash" => {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            Ok(format!("{home}/.local/share/bash-completion/completions/carpai"))
        }
        "zsh" => {
            // Prefer site-functions (requires root), fallback to user dir
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            let user_path = format!("{home}/.zsh/completions/_carpai");
            if std::path::Path::new(&user_path).parent().map_or(false, |p| p.exists()) {
                return Ok(user_path);
            }
            Ok(format!("{home}/.zsh/completions/_carpai"))
        }
        "fish" => {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            Ok(format!("{home}/.config/fish/completions/carpai.fish"))
        }
        "powershell" => {
            // Try to find the PowerShell profile
            if let Ok(profile) = std::env::var("PROFILE") {
                let dir = std::path::Path::new(&profile).parent()
                    .map(|p| p.join("carpai_completion.psm1"))
                    .unwrap_or_else(|| std::path::PathBuf::from("carpai_completion.psm1"));
                return Ok(dir.to_string_lossy().into());
            }
            let local = std::env::var("LOCALAPPDATA")
                .unwrap_or_else(|_| r"C:\Users\Default".into());
            Ok(format!("{local}\\carpai\\carpai_completion.psm1"))
        }
        other => Err(anyhow::anyhow!("Unsupported shell: {other}")),
    }
}

// --- Bash ------------------------------------------------------

fn generate_bash() -> String {
    let top_names: Vec<&str> = COMMAND_TREE.iter().map(|c| c.name).collect();
    let top = top_names.join(" ");

    let provider_vals  = "auto claude openai openai-api openrouter azure gemini groq mistral deepseek ollama lmstudio copilot perplexity togetherai deepinfra xai";
    let model_vals     = "claude-sonnet-4-5 claude-opus-4-5 gpt-4o gpt-5.5 gemini-2.5-pro deepseek-chat";

    let mut sub_cases = String::new();
    for cmd in COMMAND_TREE {
        if !cmd.subcommands.is_empty() {
            let sub_names: Vec<&str> = cmd.subcommands.iter().map(|s| s.name).collect();
            sub_cases.push_str(&format!("        {}) COMPREPLY=($(compgen -W \"{}\" -- \"$cur\")) ;;\n",
                cmd.name, sub_names.join(" ")));
        }
    }

    let mut flag_cases = String::new();
    for cmd in COMMAND_TREE {
        for f in cmd.flags {
            if let Some(h) = f.value_hint {
                let pat = match f.short {
                    Some(s) => format!("-{}|--{}", s, f.long),
                    None    => format!("--{}", f.long),
                };
                let val = match h {
                    "file"   => r#"$(_filedir)"#.to_string(),
                    "number" | "string" => String::new(),
                    v if v.starts_with("enum:") => {
                        let items = v.trim_start_matches("enum:");
                        format!(r#"$(compgen -W "{items}" -- "$cur")"#)
                    }
                    _ => String::new(),
                };
                if !val.is_empty() {
                    flag_cases.push_str(&format!("        {}) COMPREPLY=({val}) ;;\n", pat));
                }
            }
        }
    }

    format!(r#"# CarpAI bash completion — generated by `carpai completion bash`
# Source:   source /dev/stdin <<< "$(carpai completion bash)"
# Install:  carpai completion bash -o /etc/bash_completion.d/carpai

_carpai() {{
    local cur="${{COMP_WORDS[COMP_CWORD]}}"
    local prev="${{COMP_WORDS[COMP_CWORD-1]}}"
    local words=("${{COMP_WORDS[@]}}")

    # -- global‑flag value completions ----------------------------
    case "$prev" in
        -C|--cwd) COMPREPLY=($(compgen -d -- "$cur")); return 0 ;;
        -p|--provider) COMPREPLY=($(compgen -W "{provider_vals}" -- "$cur")); return 0 ;;
        -m|--model) COMPREPLY=($(compgen -W "{model_vals}" -- "$cur")); return 0 ;;
        --socket) COMPREPLY=($(compgen -f -- "$cur")); return 0 ;;
        --resume) COMPREPLY=(); return 0 ;;
{flag_cases}    esac

    # -- first level: top‑level commands -------------------------
    if (( COMP_CWORD == 1 )); then
        COMPREPLY=($(compgen -W "{top}" -- "$cur"))
        return 0
    fi

    # -- second level: sub‑subcommands ---------------------------
    if (( COMP_CWORD == 2 )); then
        case "${{words[1]}}" in
{sub_cases}            *) COMPREPLY=($(compgen -f -- "$cur")) ;;
        esac
        return 0
    fi

    # -- fallback: files -----------------------------------------
    COMPREPLY=($(compgen -f -- "$cur"))
}}

complete -F _carpai carpai
"#)
}

// --- Zsh -------------------------------------------------------

fn generate_zsh() -> String {
    let top: String = COMMAND_TREE.iter()
        .map(|c| format!("        \"{}:{}\"", c.name, c.desc))
        .collect::<Vec<_>>()
        .join("\n");
    let flags_global = zsh_flag_block("",
        &[FlagMeta { short: Some('C'), long: "cwd", desc: "Working directory", value_hint: Some("file") },
          FlagMeta { short: Some('p'), long: "provider", desc: "AI provider (auto/claude/openai/...)", value_hint: Some("enum:auto,claude,openai,openai-api,openrouter,azure,gemini") },
          FlagMeta { short: Some('m'), long: "model", desc: "Model name", value_hint: Some("string") },
          FlagMeta { short: None, long: "no-update", desc: "Skip update check", value_hint: None },
          FlagMeta { short: None, long: "trace", desc: "Log tool I/O to stderr", value_hint: None },
          FlagMeta { short: None, long: "quiet", desc: "Suppress non-error output", value_hint: None },
          FlagMeta { short: None, long: "help", desc: "Show help", value_hint: None },
        ]);

    let mut sub_body = String::new();
    for cmd in COMMAND_TREE {
        if !cmd.subcommands.is_empty() {
            let subs: String = cmd.subcommands.iter()
                .map(|s| format!("                    \"{}:{}\"", s.name, s.desc))
                .collect::<Vec<_>>()
                .join("\n");
            sub_body.push_str(&format!(r#"
                {name})
                    local -a _carpai_sub
                    _carpai_sub=(
{miss}
                    )
                    _describe -t '{name}-cmd' '{name} subcommand' _carpai_sub && ret=0
                    ;;"#, name = cmd.name, miss = subs));
        }
        // standalone commands with flags
        if cmd.subcommands.is_empty() && !cmd.flags.is_empty() {
            let fb = zsh_flag_block(cmd.name, cmd.flags);
            sub_body.push_str(&format!(r#"
                {name})
                    _arguments -s -S \
{fb}                    && ret=0
                    ;;"#, name = cmd.name));
        }
    }

    format!(r#"# CarpAI zsh completion — generated by `carpai completion zsh`
#compdef carpai
# Install: carpai completion zsh -o /usr/local/share/zsh/site-functions/_carpai

_carpai() {{
    typeset -A opt_args
    local curcontext="$curcontext" state line ret=1

    _arguments -C -S \
        '(-): :->command' \
        '(-)*: :->args' \
    && ret=0

    case $state in
        command)
            local -a _carpai_cmds
            _carpai_cmds=(
{top}
            )
            _describe -t carpai-cmd 'carpai command' _carpai_cmds && ret=0
            ;;
        args)
            case $words[1] in
{sub_body}                *)
                    _arguments -s -S \
{flags_global}                    && ret=0
                    _files && ret=0
                    ;;
            esac
            ;;
    esac
    return ret
}}

_carpai "$@"
"#)
}

fn zsh_flag_block(_cmd: &str, flags: &[FlagMeta]) -> String {
    let mut out = String::new();
    for f in flags {
        let mut spec = match f.short {
            Some(s) => format!("'{{-{s},--{}}}'", f.long),
            None    => format!("'--{}'", f.long),
        };
        spec.push_str(&format!("[{}]", f.desc));
        match f.value_hint {
            None => {} // boolean
            Some("file") => spec.push_str(":file:_files' \\"),
            Some("number") => spec.push_str(":number:' \\"),
            Some(h) if h.starts_with("enum:") => {
                let vals = h.trim_start_matches("enum:");
                spec.push_str(&format!(":value:({vals})' \\"));
            }
            Some(_) => spec.push_str(":value:' \\"),
        }
        if !spec.ends_with("' \\") && !spec.ends_with("\\") {
            spec.push_str("' \\");
        }
        out.push_str(&format!("                        {spec}\n"));
    }
    out
}

// --- Fish ------------------------------------------------------

fn generate_fish() -> String {
    let mut lines = vec![
        "# CarpAI fish completion — generated by `carpai completion fish`".into(),
        "# Install: carpai completion fish -o ~/.config/fish/completions/carpai.fish".into(),
        String::new(),
    ];
    // top-level commands
    for cmd in COMMAND_TREE {
        let d = cmd.desc.replace('\'', "\\'");
        lines.push(format!("complete -c carpai -f -n '__fish_use_subcommand' -a '{}' -d '{d}'", cmd.name));
    }
    // subcommand flags & sub-subcommands
    for cmd in COMMAND_TREE {
        let parent = format!("__fish_seen_subcommand_from '{}'", cmd.name);
        for sub in cmd.subcommands {
            let cond = format!("{parent}; and __fish_use_subcommand");
            let d = sub.desc.replace('\'', "\\'");
            lines.push(format!("complete -c carpai -f -n '{cond}' -a '{}' -d '{d}'", sub.name));
            for f in sub.flags {
                lines.push(fish_flag(&format!("{parent}; and __fish_seen_subcommand_from '{}'", sub.name), f));
            }
        }
        for f in cmd.flags {
            lines.push(fish_flag(&parent, f));
        }
    }
    // global flags (available at root)
    let globals = [
        ("C","cwd","Working directory","file"),
        ("p","provider","AI provider","string"),
        ("m","model","Model name","string"),
        ("","no-update","Skip update check",""),
        ("","trace","Log tool I/O",""),
        ("","quiet","Suppress output",""),
        ("","help","Show help",""),
        ("","version","Show version",""),
    ];
    for (s,l,d,h) in &globals {
        let mut base = format!("complete -c carpai -n '__fish_no_subcommand' -l {l}"); // use format arg
        if !s.is_empty() { base.push_str(&format!(" -s {s}")); }
        if h.is_empty() { base.push_str(&format!(" -d '{d}'")); }
        else { base.push_str(&format!(" -r -d '{d}'")); }
        lines.push(base);
    }
    // provider/model/help
    lines.push("complete -c carpai -n '__fish_no_subcommand' -l 'provider' -s 'p' -r -d 'AI provider' -xa 'auto claude openai openai-api openrouter azure gemini groq mistral deepseek ollama lmstudio copilot'".into());
    lines.push("complete -c carpai -n '__fish_no_subcommand' -l 'model'    -s 'm' -r -d 'Model name'".into());
    lines.push("complete -c carpai -n '__fish_no_subcommand' -l 'help'              -d  'Show help'".into());
    lines.push("complete -c carpai -n '__fish_no_subcommand' -l 'version'           -d  'Show version'".into());
    lines.join("\n") + "\n"
}

fn fish_flag(condition: &str, f: &FlagMeta) -> String {
    let d = f.desc.replace('\'', "\\'");
    let mut parts = vec![format!("complete -c carpai -f -n '{condition}'")];
    if let Some(s) = f.short { parts.push(format!("-s {s}")); }
    parts.push(format!("-l {}", f.long));
    match f.value_hint {
        None => parts.push(format!("-d '{d}'")),
        Some("file") => { parts.push("-r".into()); parts.push(format!("-d '{d}'")); parts.push("-F".into()); }
        Some("number") => { parts.push("-r".into()); parts.push(format!("-d '{d}'")); }
        Some(v) if v.starts_with("enum:") => {
            let vals = v.trim_start_matches("enum:");
            parts.push("-r".into()); parts.push(format!("-d '{d}'")); parts.push(format!("-xa '{vals}'"));
        }
        Some(_) => { parts.push("-r".into()); parts.push(format!("-d '{d}'")); }
    }
    parts.join(" ")
}

// --- PowerShell ------------------------------------------------

fn generate_powershell() -> String {
    let tree = ps_json(COMMAND_TREE);
    format!(r#"# CarpAI PowerShell completion — generated by `carpai completion powershell`
# Install: Add-Content -Path $PROFILE -Value (carpai completion powershell)

$script:CarpaiCompletionTree = '{tree}'

Register-ArgumentCompleter -Native -CommandName carpai -ScriptBlock {{
    param($wordToComplete, $commandAst, $cursorPosition)

    $commands = [System.Collections.Generic.List[System.Management.Automation.CompletionResult]]::new()
    $tree = $script:CarpaiCompletionTree | ConvertFrom-Json

    function Complete-Node {{
        param($Node, $Depth, $Words, $CurrentWord, [ref]$Results)

        if ($Depth -ge $Words.Count - 1) {{
            foreach ($child in $Node.subcommands) {{
                if ($child.name -like "$CurrentWord*") {{
                    $Results.Value.Add([System.Management.Automation.CompletionResult]::new(
                        $child.name, $child.name, 'ParameterValue', $child.desc
                    ))
                }}
            }}
            foreach ($flag in $Node.flags) {{
                $flagName = if ($flag.short) {{ "-$($flag.short)" }} else {{ "--$($flag.long)" }}
                if ($flagName -like "*$CurrentWord*") {{
                    $Results.Value.Add([System.Management.Automation.CompletionResult]::new(
                        $flagName, $flagName, 'Parameter', $flag.desc
                    ))
                }}
            }}
            return
        }}
        $word = $Words[$Depth + 1]
        foreach ($child in $Node.subcommands) {{
            if ($child.name -eq $word) {{
                Complete-Node -Node $child -Depth ($Depth + 1) -Words $Words `
                    -CurrentWord $CurrentWord -Results $Results
                return
            }}
        }}
    }}

    $words = $commandAst.CommandElements | ForEach-Object {{ $_.Extent.Text }}
    Complete-Node -Node $tree -Depth 1 -Words $words -CurrentWord $wordToComplete `
        -Results ([ref]$commands)

    if ($commands.Count -eq 0) {{
        $commands.Add([System.Management.Automation.CompletionResult]::new(
            $wordToComplete, $wordToComplete, 'ParameterValue', ' ')
        )
    }}
    $commands.ToArray()
}}
"#)
}

fn ps_json(cmds: &[CmdMeta]) -> String {
    use serde_json::json;
    let items: Vec<serde_json::Value> = cmds.iter().map(|c| {
        let flags: Vec<serde_json::Value> = c.flags.iter().map(|f| {
            let mut m = json!({ "long": f.long, "desc": f.desc });
            if let Some(s) = f.short { m["short"] = json!(s.to_string()); }
            if let Some(v) = f.value_hint { m["value_hint"] = json!(v); }
            m
        }).collect();
        json!({ "name": c.name, "desc": c.desc, "subcommands": ps_json(c.subcommands), "flags": flags })
    }).collect();
    serde_json::to_string(&items).unwrap_or_default()
}

// ════════════════════════════════════════════════════════════════════
// ════════════════════════════════════════════════════════════════════