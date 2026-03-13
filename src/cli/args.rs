use clap::{Parser, Subcommand};

use super::provider_init::ProviderChoice;

#[derive(Parser, Debug)]
#[command(name = "jcode")]
#[command(version = env!("JCODE_VERSION"))]
#[command(about = "J-Code: A coding agent using Claude Max or ChatGPT Pro subscriptions")]
pub(crate) struct Args {
    /// Provider to use (jcode, claude, openai, openrouter, azure, opencode, opencode-go, zai, chutes, cerebras, openai-compatible, cursor, copilot, gemini, antigravity, or auto-detect)
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

    /// Resume a session by ID, or list sessions if no ID provided
    #[arg(long, global = true, num_args = 0..=1, default_missing_value = "")]
    pub(crate) resume: Option<String>,

    /// DEPRECATED: Run standalone TUI without connecting to server.
    /// The default mode is now always client/server (even for self-dev).
    /// Standalone mode is missing features like graceful cancel with partial
    /// content preservation on the server side. Will be removed in a future version.
    #[arg(long, global = true, hide = true)]
    #[deprecated = "Use default client/server mode instead"]
    pub(crate) standalone: bool,

    /// Disable auto-detection of jcode repository and self-dev mode
    #[arg(long, global = true)]
    pub(crate) no_selfdev: bool,

    /// Custom socket path for server/client communication
    #[arg(long, global = true)]
    pub(crate) socket: Option<String>,

    /// Enable debug socket (broadcasts all TUI state changes)
    #[arg(long, global = true)]
    pub(crate) debug_socket: bool,

    /// Model to use (e.g., claude-opus-4-6, gpt-5.4)
    #[arg(short, long, global = true)]
    pub(crate) model: Option<String>,

    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Command {
    /// Start the agent server (background daemon)
    Serve,

    /// Connect to a running server
    Connect,

    /// Run a single message and exit
    Run {
        /// The message to send
        message: String,
    },

    /// Login to a provider via OAuth
    Login {
        /// Account label for multi-account support (default: "default")
        #[arg(long, short = 'a')]
        account: Option<String>,
    },

    /// Run in simple REPL mode (no TUI)
    Repl,

    /// Update jcode to the latest version
    Update,

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

    /// Memory management commands
    #[command(subcommand)]
    Memory(MemoryCommand),

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

    /// Set up a global hotkey (Alt+;) to launch jcode
    SetupHotkey {
        /// Internal: run as the macOS hotkey listener process.
        #[arg(long, hide = true)]
        listen_macos_hotkey: bool,
    },

    /// Firefox Agent Bridge - browser automation setup and management
    Browser {
        /// Subcommand (setup, status)
        #[arg(default_value = "setup")]
        action: String,
    },

    /// Replay a saved session in the TUI
    Replay {
        /// Session ID, name, or path to session JSON file
        session: String,

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::provider_init::ProviderChoice;

    #[test]
    fn test_provider_choice_aliases_parse() {
        let args = Args::try_parse_from(["jcode", "--provider", "z.ai", "run", "smoke"]).unwrap();
        assert_eq!(args.provider, ProviderChoice::Zai);

        let args =
            Args::try_parse_from(["jcode", "--provider", "cerebrascode", "run", "smoke"]).unwrap();
        assert_eq!(args.provider, ProviderChoice::Cerebras);

        let args = Args::try_parse_from(["jcode", "--provider", "compat", "run", "smoke"]).unwrap();
        assert_eq!(args.provider, ProviderChoice::OpenaiCompatible);
    }
}
