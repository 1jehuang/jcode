mod agent;
mod ambient;
mod ambient_runner;
mod ambient_scheduler;
mod auth;
mod auto_debug;
mod background;
mod build;
mod bus;
mod cache_tracker;
mod channel;
mod compaction;
mod config;
mod embedding;
mod id;
mod logging;
mod mcp;
mod memory;
mod memory_agent;
mod memory_graph;
mod message;
mod notifications;
mod plan;
mod prompt;
mod protocol;
mod provider;
mod registry;
mod safety;
mod server;
mod session;
mod sidecar;
mod skill;
mod storage;
mod telegram;
mod todo;
mod tool;
mod tui;
mod usage;
mod util;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use provider::Provider;
use std::io::{self, IsTerminal, Write};
use std::panic;
use std::process::Command as ProcessCommand;
use std::sync::{Arc, Mutex};

/// Global session ID for panic recovery
static CURRENT_SESSION_ID: Mutex<Option<String>> = Mutex::new(None);

/// Set the current session ID for panic recovery
pub fn set_current_session(session_id: &str) {
    if let Ok(mut guard) = CURRENT_SESSION_ID.lock() {
        *guard = Some(session_id.to_string());
    }
}

/// Install panic hook that prints session recovery command
fn install_panic_hook() {
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        // Call default hook first (prints backtrace, etc.)
        default_hook(info);

        // Print recovery command if we have a session
        if let Ok(guard) = CURRENT_SESSION_ID.lock() {
            if let Some(session_id) = guard.as_ref() {
                let session_name =
                    id::extract_session_name(session_id).unwrap_or(session_id.as_str());
                eprintln!();
                eprintln!(
                    "\x1b[33mSession \x1b[1m{}\x1b[0m\x1b[33m - to resume:\x1b[0m",
                    session_name
                );
                eprintln!("  jcode --resume {}", session_id);
                eprintln!();

                if let Ok(mut session) = session::Session::load(session_id) {
                    session.mark_crashed(Some(format!("Panic: {}", info)));
                    let _ = session.save();
                }
            }
        }
    }));
}

fn mark_current_session_crashed(message: String) {
    if let Ok(guard) = CURRENT_SESSION_ID.lock() {
        if let Some(session_id) = guard.as_ref() {
            if let Ok(mut session) = session::Session::load(session_id) {
                // Don't overwrite an explicit clean shutdown status.
                if matches!(session.status, session::SessionStatus::Active) {
                    session.mark_crashed(Some(message));
                    let _ = session.save();
                }
            }
        }
    }
}

fn panic_payload_to_string(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

fn init_tui_terminal() -> Result<ratatui::DefaultTerminal> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        anyhow::bail!("jcode TUI requires an interactive terminal (stdin/stdout must be a TTY)");
    }
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(ratatui::init)).map_err(|payload| {
        anyhow::anyhow!(
            "failed to initialize terminal: {}",
            panic_payload_to_string(payload.as_ref())
        )
    })
}

#[cfg(unix)]
fn signal_name(sig: i32) -> &'static str {
    match sig {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        6 => "SIGABRT",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        _ => "unknown",
    }
}

#[cfg(not(unix))]
fn signal_name(_sig: i32) -> &'static str {
    "unknown"
}

#[cfg(unix)]
fn signal_crash_reason(sig: i32) -> String {
    match sig {
        libc::SIGHUP => "Terminal or window closed (SIGHUP)".to_string(),
        libc::SIGTERM => "Terminated (SIGTERM)".to_string(),
        libc::SIGINT => "Interrupted (SIGINT)".to_string(),
        libc::SIGQUIT => "Quit signal (SIGQUIT)".to_string(),
        _ => format!("Terminated by signal {} ({})", signal_name(sig), sig),
    }
}

#[cfg(unix)]
fn handle_termination_signal(sig: i32) -> ! {
    mark_current_session_crashed(signal_crash_reason(sig));
    std::process::exit(128 + sig);
}

#[cfg(unix)]
fn spawn_session_signal_watchers() {
    use tokio::signal::unix::{signal, SignalKind};

    fn spawn_one(sig: i32, kind: SignalKind) {
        tokio::spawn(async move {
            let mut stream = match signal(kind) {
                Ok(s) => s,
                Err(e) => {
                    crate::logging::error(&format!(
                        "Failed to install {} handler: {}",
                        signal_name(sig),
                        e
                    ));
                    return;
                }
            };
            if stream.recv().await.is_some() {
                crate::logging::info(&format!("Received {} in TUI process", signal_name(sig)));
                handle_termination_signal(sig);
            }
        });
    }

    spawn_one(libc::SIGHUP, SignalKind::hangup());
    spawn_one(libc::SIGTERM, SignalKind::terminate());
    spawn_one(libc::SIGINT, SignalKind::interrupt());
    spawn_one(libc::SIGQUIT, SignalKind::quit());
}

#[cfg(not(unix))]
fn spawn_session_signal_watchers() {}

#[derive(Debug, Clone, PartialEq, Eq, ValueEnum)]
enum ProviderChoice {
    Claude,
    /// Deprecated: legacy transport that shells out to Claude CLI.
    /// Use `--provider claude` for the default HTTP API path.
    ClaudeSubprocess,
    Openai,
    Cursor,
    Copilot,
    Antigravity,
    Auto,
}

impl ProviderChoice {
    fn as_arg_value(&self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::ClaudeSubprocess => "claude-subprocess",
            Self::Openai => "openai",
            Self::Cursor => "cursor",
            Self::Copilot => "copilot",
            Self::Antigravity => "antigravity",
            Self::Auto => "auto",
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "jcode")]
#[command(version = env!("JCODE_VERSION"))]
#[command(about = "J-Code: A coding agent using Claude Max or ChatGPT Pro subscriptions")]
struct Args {
    /// Provider to use (claude, claude-subprocess, openai, cursor, copilot, antigravity, or auto-detect)
    #[arg(short, long, default_value = "auto", global = true)]
    provider: ProviderChoice,

    /// Working directory
    #[arg(short = 'C', long, global = true)]
    cwd: Option<String>,

    /// Skip the automatic update check
    #[arg(long, global = true)]
    no_update: bool,

    /// Auto-update when new version is available (default: false, just notify)
    #[arg(long, global = true, default_value = "false")]
    auto_update: bool,

    /// Log tool inputs/outputs and token usage to stderr
    #[arg(long, global = true)]
    trace: bool,

    /// Resume a session by ID, or list sessions if no ID provided
    #[arg(long, global = true, num_args = 0..=1, default_missing_value = "")]
    resume: Option<String>,

    /// Run standalone TUI without connecting to server (DEPRECATED: use server mode)
    #[arg(long, global = true, hide = true)]
    standalone: bool,

    /// Disable auto-detection of jcode repository and self-dev mode
    #[arg(long, global = true)]
    no_selfdev: bool,

    /// Custom socket path for server/client communication
    #[arg(long, global = true)]
    socket: Option<String>,

    /// Enable debug socket (broadcasts all TUI state changes)
    #[arg(long, global = true)]
    debug_socket: bool,

    /// Model to use (e.g., claude-opus-4-5-20251101, gpt-5.3-codex-spark)
    #[arg(short, long, global = true)]
    model: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
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
    Login,

    /// Run in simple REPL mode (no TUI)
    Repl,

    /// Update jcode to the latest version
    Update,

    /// Self-development mode: run as canary with auto-rollback on crash
    SelfDev {
        /// Build and test a new canary version before launching
        #[arg(long)]
        build: bool,
    },

    /// Promote current canary build to stable (other sessions will auto-migrate)
    Promote,

    /// Internal: wrapper for canary process (handles crash recovery)
    #[command(hide = true)]
    CanaryWrapper {
        /// Session ID to run
        session_id: String,
        /// Binary path to run
        binary: String,
        /// Git hash of the current build
        git_hash: String,
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

    /// Review and respond to pending ambient permission requests
    Permissions,
}

#[derive(Subcommand, Debug)]
enum AmbientCommand {
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
enum MemoryCommand {
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

#[tokio::main]
async fn main() -> Result<()> {
    // Install panic hook for session recovery
    install_panic_hook();

    // Initialize logging
    logging::init();
    logging::cleanup_old_logs();
    logging::info("jcode starting");

    let args = Args::parse();

    // Change working directory if specified
    if let Some(cwd) = &args.cwd {
        std::env::set_current_dir(cwd)?;
        logging::info(&format!("Changed working directory to: {}", cwd));
    }

    if args.trace {
        std::env::set_var("JCODE_TRACE", "1");
    }

    // Set custom socket path if provided
    if let Some(ref socket) = args.socket {
        server::set_socket_path(socket);
    }

    // Check for updates in background unless --no-update is specified or running Update command
    let check_updates =
        !args.no_update && !matches!(args.command, Some(Command::Update)) && args.resume.is_none();
    let auto_update = args.auto_update;

    if check_updates {
        // Spawn update check in background to avoid blocking startup
        std::thread::spawn(move || {
            if let Some(update_available) = check_for_updates() {
                if update_available {
                    if auto_update {
                        eprintln!("Update available - auto-updating...");
                        if let Err(e) = run_auto_update() {
                            eprintln!(
                                "Auto-update failed: {}. Continuing with current version.",
                                e
                            );
                        }
                    } else {
                        eprintln!(
                            "\n📦 Update available! Run `jcode update` or `/reload` to update.\n"
                        );
                    }
                }
            }
        });
    }

    // Run main logic with error handling for auto-debug
    if let Err(e) = run_main(args).await {
        let error_str = format!("{:?}", e);
        logging::error(&error_str);

        // Trigger auto-debug if enabled
        if auto_debug::is_enabled() {
            auto_debug::analyze_error(&error_str, "main execution");
        }

        // Print session recovery command if we have a session
        if let Ok(guard) = CURRENT_SESSION_ID.lock() {
            if let Some(session_id) = guard.as_ref() {
                eprintln!();
                eprintln!("\x1b[33mTo restore this session, run:\x1b[0m");
                eprintln!("  jcode --resume {}", session_id);
                eprintln!();
            }
        }

        return Err(e);
    }

    Ok(())
}

async fn run_main(mut args: Args) -> Result<()> {
    // Handle --resume without session ID: list available sessions
    if let Some(ref resume_id) = args.resume {
        if resume_id.is_empty() {
            return list_sessions();
        }
        // Resolve memorable name to full session ID
        match session::find_session_by_name_or_id(resume_id) {
            Ok(full_id) => {
                args.resume = Some(full_id);
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                eprintln!("\nUse `jcode --resume` to list available sessions.");
                std::process::exit(1);
            }
        }
    }

    match args.command {
        Some(Command::Serve) => {
            let (provider, _registry) =
                init_provider_and_registry(&args.provider, args.model.as_deref()).await?;
            let server = server::Server::new(provider);
            server.run().await?;
        }
        Some(Command::Connect) => {
            run_client().await?;
        }
        Some(Command::Run { message }) => {
            let (provider, registry) =
                init_provider_and_registry(&args.provider, args.model.as_deref()).await?;
            let mut agent = agent::Agent::new(provider, registry);
            agent.run_once(&message).await?;
        }
        Some(Command::Login) => {
            run_login(&args.provider).await?;
        }
        Some(Command::Repl) => {
            // Simple REPL mode (no TUI)
            let (provider, registry) =
                init_provider_and_registry(&args.provider, args.model.as_deref()).await?;
            let mut agent = agent::Agent::new(provider, registry);
            agent.repl().await?;
        }
        Some(Command::Update) => {
            run_update()?;
        }
        Some(Command::SelfDev { build }) => {
            run_self_dev(build, args.resume).await?;
        }
        Some(Command::Promote) => {
            run_promote()?;
        }
        Some(Command::CanaryWrapper {
            session_id,
            binary,
            git_hash,
        }) => {
            run_canary_wrapper(&session_id, &binary, &git_hash).await?;
        }
        Some(Command::Debug {
            command,
            arg,
            session,
            socket,
            wait,
        }) => {
            run_debug_command(&command, &arg, session, socket, wait).await?;
        }
        Some(Command::Memory(subcmd)) => {
            run_memory_command(subcmd)?;
        }
        Some(Command::Ambient(subcmd)) => {
            run_ambient_command(subcmd).await?;
        }
        Some(Command::Permissions) => {
            tui::permissions::run_permissions()?;
        }
        None => {
            // Auto-detect jcode repo and enable self-dev mode
            let cwd = std::env::current_dir()?;
            let in_jcode_repo = build::is_jcode_repo(&cwd);
            let already_in_selfdev = std::env::var("JCODE_SELFDEV_MODE").is_ok();

            if in_jcode_repo && !already_in_selfdev && !args.standalone && !args.no_selfdev {
                // Auto-start self-dev mode with wrapper
                eprintln!("📍 Detected jcode repository - enabling self-dev mode");
                eprintln!("   (use --no-selfdev to disable auto-detection)\n");

                // Set env var to prevent infinite loop
                std::env::set_var("JCODE_SELFDEV_MODE", "1");

                // Re-exec into self-dev mode
                return run_self_dev(false, args.resume).await;
            }

            // Check for --standalone flag (DEPRECATED)
            if args.standalone {
                eprintln!("\x1b[33m⚠️  Warning: --standalone is deprecated and will be removed in a future version.\x1b[0m");
                eprintln!("\x1b[33m   The default server/client mode now handles all use cases including self-dev.\x1b[0m\n");
                let (provider, registry) =
                    init_provider_and_registry(&args.provider, args.model.as_deref()).await?;
                run_tui(provider, registry, args.resume, args.debug_socket).await?;
            } else {
                // Default: TUI client mode - start server if needed
                let server_running = if server::socket_path().exists() {
                    // Test if server is actually responding
                    tokio::net::UnixStream::connect(server::socket_path())
                        .await
                        .is_ok()
                } else {
                    false
                };

                if server_running && (args.provider != ProviderChoice::Auto || args.model.is_some())
                {
                    eprintln!(
                        "Server already running; provider/model flags only apply when starting a new server."
                    );
                    eprintln!(
                        "Current server settings control `/model`. Restart server to apply: --provider {}{}",
                        args.provider.as_arg_value(),
                        args.model
                            .as_ref()
                            .map(|m| format!(" --model {}", m))
                            .unwrap_or_default()
                    );
                }

                if !server_running {
                    // Clean up any stale sockets
                    let _ = std::fs::remove_file(server::socket_path());
                    let _ = std::fs::remove_file(server::debug_socket_path());

                    // Start server in background
                    eprintln!("Starting server...");
                    let exe = std::env::current_exe()?;
                    let mut cmd = std::process::Command::new(&exe);
                    cmd.arg("--provider").arg(args.provider.as_arg_value());
                    if let Some(model) = args.model.as_deref() {
                        cmd.arg("--model").arg(model);
                    }
                    let mut child = cmd
                        .arg("serve")
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn()?;

                    // Wait for server to be ready (up to 10 seconds)
                    let start = std::time::Instant::now();
                    loop {
                        if start.elapsed() > std::time::Duration::from_secs(10) {
                            let _ = child.kill();
                            anyhow::bail!("Server failed to start within 10 seconds");
                        }
                        if server::socket_path().exists() {
                            if tokio::net::UnixStream::connect(server::socket_path())
                                .await
                                .is_ok()
                            {
                                break;
                            }
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                }

                eprintln!("Connecting to server...");
                run_tui_client(args.resume).await?;
            }
        }
    }

    Ok(())
}

async fn init_provider_and_registry(
    choice: &ProviderChoice,
    model: Option<&str>,
) -> Result<(Arc<dyn provider::Provider>, tool::Registry)> {
    let provider: Arc<dyn provider::Provider> = match choice {
        ProviderChoice::Claude => {
            // Explicit Claude - use MultiProvider but prefer Claude
            eprintln!("Using Claude (with multi-provider support)");
            std::env::set_var("JCODE_ACTIVE_PROVIDER", "claude");
            Arc::new(provider::MultiProvider::with_preference(false))
        }
        ProviderChoice::ClaudeSubprocess => {
            crate::logging::warn(
                "Using --provider claude-subprocess is deprecated. Prefer `--provider claude`.",
            );
            std::env::set_var("JCODE_USE_CLAUDE_CLI", "1");
            eprintln!("Using deprecated Claude subprocess transport (legacy mode)");
            std::env::set_var("JCODE_ACTIVE_PROVIDER", "claude");
            Arc::new(provider::MultiProvider::with_preference(false))
        }
        ProviderChoice::Openai => {
            // Explicit OpenAI - use MultiProvider but prefer OpenAI
            eprintln!("Using OpenAI (with multi-provider support)");
            std::env::set_var("JCODE_ACTIVE_PROVIDER", "openai");
            Arc::new(provider::MultiProvider::with_preference(true))
        }
        ProviderChoice::Cursor => {
            eprintln!("Using Cursor CLI provider (experimental)");
            std::env::set_var("JCODE_ACTIVE_PROVIDER", "cursor");
            Arc::new(provider::cursor::CursorCliProvider::new())
        }
        ProviderChoice::Copilot => {
            eprintln!("Using GitHub Copilot CLI provider (experimental)");
            std::env::set_var("JCODE_ACTIVE_PROVIDER", "copilot");
            Arc::new(provider::copilot::CopilotCliProvider::new())
        }
        ProviderChoice::Antigravity => {
            eprintln!("Using Antigravity CLI provider (experimental)");
            std::env::set_var("JCODE_ACTIVE_PROVIDER", "antigravity");
            Arc::new(provider::antigravity::AntigravityCliProvider::new())
        }
        ProviderChoice::Auto => {
            // Check if we have any credentials (in parallel)
            let (has_claude, has_openai) = tokio::join!(
                tokio::task::spawn_blocking(|| auth::claude::load_credentials().is_ok()),
                tokio::task::spawn_blocking(|| auth::codex::load_credentials().is_ok()),
            );
            let has_claude = has_claude.unwrap_or(false);
            let has_openai = has_openai.unwrap_or(false);

            if has_claude || has_openai {
                // Use MultiProvider - it will auto-detect and allow switching
                let multi = provider::MultiProvider::new();
                eprintln!("Using {} (use /model to switch models)", multi.name());
                std::env::set_var("JCODE_ACTIVE_PROVIDER", multi.name().to_lowercase());
                Arc::new(multi)
            } else {
                // No credentials - prompt for login
                eprintln!("No credentials found. Let's log in!\n");
                eprintln!("Choose a provider:");
                eprintln!("  1. Claude (Claude Max subscription)");
                eprintln!("  2. OpenAI (ChatGPT Pro subscription)");
                eprintln!("  3. Cursor");
                eprintln!("  4. GitHub Copilot");
                eprintln!("  5. Antigravity");
                eprint!("\nEnter 1-5: ");
                io::stdout().flush()?;

                let mut input = String::new();
                io::stdin().read_line(&mut input)?;

                match input.trim() {
                    "1" => {
                        login_claude_flow().await?;
                        eprintln!();
                        Arc::new(provider::MultiProvider::new())
                    }
                    "2" => {
                        login_openai_flow().await?;
                        eprintln!();
                        Arc::new(provider::MultiProvider::with_preference(true))
                    }
                    "3" => {
                        login_cursor_flow()?;
                        eprintln!();
                        Arc::new(provider::cursor::CursorCliProvider::new())
                    }
                    "4" => {
                        login_copilot_flow()?;
                        eprintln!();
                        Arc::new(provider::copilot::CopilotCliProvider::new())
                    }
                    "5" => {
                        login_antigravity_flow()?;
                        eprintln!();
                        Arc::new(provider::antigravity::AntigravityCliProvider::new())
                    }
                    _ => {
                        anyhow::bail!("Invalid choice. Run 'jcode login' to try again.");
                    }
                }
            }
        }
    };

    // Apply model selection if specified
    if let Some(model_name) = model {
        if let Err(e) = provider.set_model(model_name) {
            eprintln!("Warning: failed to set model '{}': {}", model_name, e);
        } else {
            eprintln!("Using model: {}", model_name);
        }
    }

    let registry = tool::Registry::new(provider.clone()).await;
    Ok((provider, registry))
}

async fn run_tui(
    provider: Arc<dyn provider::Provider>,
    registry: tool::Registry,
    resume_session: Option<String>,
    debug_socket: bool,
) -> Result<()> {
    let terminal = init_tui_terminal()?;
    // Initialize mermaid image picker (queries terminal for graphics protocol support)
    crate::tui::mermaid::init_picker();
    let mouse_capture = crate::config::config().display.mouse_capture;
    // Enable Kitty keyboard protocol for unambiguous key reporting (Ctrl+J != Enter, etc.)
    let keyboard_enhanced = tui::enable_keyboard_enhancement();
    // Enable bracketed paste mode for proper paste handling in terminals like Kitty
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste)?;
    if mouse_capture {
        crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;
    }
    let mut app = tui::App::new(provider, registry);

    // Enable debug socket if requested
    let _debug_handle = if debug_socket {
        let rx = app.enable_debug_socket();
        let handle = app.start_debug_socket_listener(rx);
        logging::info(&format!(
            "Debug socket enabled at: {:?}",
            tui::App::debug_socket_path()
        ));
        Some(handle)
    } else {
        None
    };

    // Restore session if resuming
    if let Some(ref session_id) = resume_session {
        app.restore_session(session_id);
    }

    // Set current session for panic recovery
    set_current_session(app.session_id());
    spawn_session_signal_watchers();

    // Save session info before running (for resume message)
    let session_id = app.session_id().to_string();
    let session_name = id::extract_session_name(&session_id)
        .map(|s| s.to_string())
        .unwrap_or_else(|| session_id.clone());

    // Set terminal window title with session icon and name
    let icon = id::session_icon(&session_name);
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::SetTitle(format!("{} jcode {}", icon, session_name))
    );

    app.init_mcp().await;
    let result = app.run(terminal).await;
    // Disable bracketed paste and mouse capture before restoring terminal
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
    if mouse_capture {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    }
    if keyboard_enhanced {
        tui::disable_keyboard_enhancement();
    }
    ratatui::restore();
    crate::tui::mermaid::clear_image_state();

    let run_result = result?;

    // Check for special exit code (canary wrapper communication)
    if let Some(code) = run_result.exit_code {
        std::process::exit(code);
    }

    // Check for hot-reload request (no rebuild)
    if let Some(ref reload_session_id) = run_result.reload_session {
        hot_reload(reload_session_id)?;
    }

    // Check for hot-rebuild request (full git pull + cargo build + tests)
    if let Some(ref rebuild_session_id) = run_result.rebuild_session {
        hot_rebuild(rebuild_session_id)?;
    }

    // Print resume command for normal exits (not hot-reload/rebuild)
    if run_result.reload_session.is_none() && run_result.rebuild_session.is_none() {
        eprintln!();
        eprintln!(
            "\x1b[33mSession \x1b[1m{}\x1b[0m\x1b[33m - to resume:\x1b[0m",
            session_name
        );
        eprintln!("  jcode --resume {}", session_id);
        eprintln!();
    }

    Ok(())
}

/// Hot-reload: exec into existing binary with session restore (no rebuild)
fn hot_reload(session_id: &str) -> Result<()> {
    use std::os::unix::process::CommandExt;

    let cwd = std::env::current_dir()?;
    let in_selfdev = std::env::var("JCODE_SELFDEV_MODE").is_ok()
        || std::env::var("JCODE_SOCKET")
            .ok()
            .as_deref()
            .map(|p| p == SELFDEV_SOCKET)
            .unwrap_or(false);

    // Check if this is a migration to a specific binary (auto-migration to stable)
    if let Ok(migrate_binary) = std::env::var("JCODE_MIGRATE_BINARY") {
        let binary_path = std::path::PathBuf::from(&migrate_binary);
        if binary_path.exists() {
            eprintln!("Migrating to stable binary...");
            let err = ProcessCommand::new(&binary_path)
                .arg("--resume")
                .arg(session_id)
                .arg("--no-update")
                .current_dir(cwd)
                .exec();
            return Err(anyhow::anyhow!("Failed to exec {:?}: {}", binary_path, err));
        } else {
            eprintln!(
                "Warning: Migration binary not found at {:?}, falling back to local binary",
                binary_path
            );
        }
    }

    // Pick binary based on mode:
    // - self-dev: prefer canary symlink so /reload converges to tested canary.
    // - normal: prefer repo release binary, then PATH/current.
    let exe = if in_selfdev {
        crate::build::canary_binary_path()
            .ok()
            .filter(|p| p.exists())
            .or_else(|| {
                get_repo_dir().and_then(|repo_dir| {
                    let candidate = repo_dir.join("target/release/jcode");
                    candidate.exists().then_some(candidate)
                })
            })
            .or_else(crate::build::jcode_path_in_path)
            .or_else(|| std::env::current_exe().ok())
            .ok_or_else(|| anyhow::anyhow!("No reloadable binary found for self-dev mode"))?
    } else if let Some(repo_dir) = get_repo_dir() {
        let candidate = repo_dir.join("target/release/jcode");
        if candidate.exists() {
            candidate
        } else {
            crate::build::jcode_path_in_path()
                .or_else(|| std::env::current_exe().ok())
                .ok_or_else(|| anyhow::anyhow!("No reloadable binary found on PATH"))?
        }
    } else {
        crate::build::jcode_path_in_path()
            .or_else(|| std::env::current_exe().ok())
            .ok_or_else(|| anyhow::anyhow!("No reloadable binary found on PATH"))?
    };

    // Show binary info
    if let Ok(metadata) = std::fs::metadata(&exe) {
        let age = metadata
            .modified()
            .ok()
            .and_then(|m| m.elapsed().ok())
            .map(|d| {
                let secs = d.as_secs();
                if secs < 60 {
                    format!("{} seconds ago", secs)
                } else if secs < 3600 {
                    format!("{} minutes ago", secs / 60)
                } else {
                    format!("{} hours ago", secs / 3600)
                }
            })
            .unwrap_or_else(|| "unknown".to_string());
        eprintln!("Reloading with binary built {}...", age);
    }

    // Build command with --resume flag
    // Retry on ENOENT in case binary is being replaced by a concurrent cargo build
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            if !exe.exists() {
                continue;
            }
        }
        let err = ProcessCommand::new(&exe)
            .arg("--resume")
            .arg(session_id)
            .current_dir(&cwd)
            .exec();

        if err.kind() == std::io::ErrorKind::NotFound && attempt < 2 {
            crate::logging::warn(&format!(
                "exec attempt {} failed (ENOENT) for {:?}, retrying...",
                attempt + 1,
                exe
            ));
            continue;
        }
        return Err(anyhow::anyhow!("Failed to exec {:?}: {}", exe, err));
    }
    Err(anyhow::anyhow!(
        "Failed to exec {:?}: binary not found after retries",
        exe
    ))
}

/// Hot-rebuild: pull, rebuild, test, and exec into new binary with session restore
fn hot_rebuild(session_id: &str) -> Result<()> {
    use std::os::unix::process::CommandExt;

    let cwd = std::env::current_dir()?;
    let repo_dir =
        get_repo_dir().ok_or_else(|| anyhow::anyhow!("Could not find jcode repository"))?;

    eprintln!("Rebuilding jcode with session {}...", session_id);

    // Pull latest changes (quiet)
    eprintln!("Pulling latest changes...");
    let pull = ProcessCommand::new("git")
        .args(["pull", "-q"])
        .current_dir(&repo_dir)
        .status()?;

    if !pull.success() {
        eprintln!("Warning: git pull failed, continuing with current version");
    }

    // Rebuild (show progress)
    eprintln!("Building...");
    let build = ProcessCommand::new("cargo")
        .args(["build", "--release"])
        .current_dir(&repo_dir)
        .status()?;

    if !build.success() {
        anyhow::bail!("Build failed - staying on current version");
    }

    // Run tests to check for breaking changes
    eprintln!("Running tests...");
    let test = ProcessCommand::new("cargo")
        .args(["test", "--release", "--", "--test-threads=1"])
        .current_dir(&repo_dir)
        .status()?;

    if !test.success() {
        eprintln!("\n⚠️  Tests failed! Aborting reload to protect your session.");
        eprintln!("Fix the failing tests and try /rebuild again.");
        anyhow::bail!("Tests failed - staying on current version");
    }

    eprintln!("✓ All tests passed");

    if let Err(e) = build::install_local_release(&repo_dir) {
        eprintln!("Warning: install failed: {}", e);
    }

    // Get the binary path - use the known location in the repo
    let exe = repo_dir.join("target/release/jcode");
    if !exe.exists() {
        anyhow::bail!("Binary not found at {:?}", exe);
    }

    eprintln!("Restarting with session {}...", session_id);

    // Build command with --resume flag
    let err = ProcessCommand::new(&exe)
        .arg("--resume")
        .arg(session_id)
        .current_dir(cwd)
        .exec();

    // exec() only returns on error
    Err(anyhow::anyhow!("Failed to exec {:?}: {}", exe, err))
}

/// Run a debug socket command
async fn run_debug_command(
    command: &str,
    arg: &str,
    session_id: Option<String>,
    socket_path: Option<String>,
    _wait: bool,
) -> Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    // Handle special commands that don't need a server connection
    match command {
        "list" => return debug_list_servers().await,
        "start" => return debug_start_server(arg, socket_path).await,
        _ => {}
    }

    // Determine which debug socket to connect to
    let debug_socket = if let Some(ref path) = socket_path {
        // User specified a main socket path, derive debug socket from it
        let main_path = std::path::PathBuf::from(path);
        let filename = main_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("jcode.sock");
        let debug_filename = filename.replace(".sock", "-debug.sock");
        main_path.with_file_name(debug_filename)
    } else {
        server::debug_socket_path()
    };

    if !debug_socket.exists() {
        eprintln!("Debug socket not found at {:?}", debug_socket);
        eprintln!("\nMake sure:");
        eprintln!("  1. A jcode server is running (jcode or jcode serve)");
        eprintln!("  2. debug_socket is enabled in ~/.jcode/config.toml");
        eprintln!("     [display]");
        eprintln!("     debug_socket = true");
        eprintln!("\nOr use 'jcode debug start' to start a server.");
        eprintln!("Use 'jcode debug list' to see running servers.");
        anyhow::bail!("Debug socket not available");
    }

    let stream = server::connect_socket(&debug_socket).await?;
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    // Build the debug command
    let debug_cmd = if arg.is_empty() {
        command.to_string()
    } else {
        format!("{}:{}", command, arg)
    };

    // Build the request
    let request = serde_json::json!({
        "type": "debug_command",
        "id": 1,
        "command": debug_cmd,
        "session_id": session_id,
    });

    // Send request
    let mut json = serde_json::to_string(&request)?;
    json.push('\n');
    writer.write_all(json.as_bytes()).await?;

    // Read response
    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        anyhow::bail!("Server disconnected before sending response");
    }

    // Parse and display response
    let response: serde_json::Value = serde_json::from_str(&line)?;

    match response.get("type").and_then(|v| v.as_str()) {
        Some("debug_response") => {
            let ok = response
                .get("ok")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let output = response
                .get("output")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            if ok {
                println!("{}", output);
            } else {
                eprintln!("Error: {}", output);
                std::process::exit(1);
            }
        }
        Some("error") => {
            let message = response
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown error");
            eprintln!("Error: {}", message);
            std::process::exit(1);
        }
        _ => {
            // Print raw response
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
    }

    Ok(())
}

/// Run ambient mode CLI commands via the debug socket
async fn run_ambient_command(cmd: AmbientCommand) -> Result<()> {
    match cmd {
        AmbientCommand::RunVisible => {
            return run_ambient_visible().await;
        }
        _ => {}
    }

    let debug_cmd = match cmd {
        AmbientCommand::Status => "ambient:status",
        AmbientCommand::Log => "ambient:log",
        AmbientCommand::Trigger => "ambient:trigger",
        AmbientCommand::Stop => "ambient:stop",
        AmbientCommand::RunVisible => unreachable!(),
    };

    // Send command via debug socket
    run_debug_command(debug_cmd, "", None, None, false).await
}

/// Run a visible ambient cycle in a standalone TUI.
/// Reads context from `~/.jcode/ambient/visible_cycle.json`, starts a TUI with
/// ambient system prompt and tools, and auto-sends the initial message.
async fn run_ambient_visible() -> Result<()> {
    use crate::ambient::VisibleCycleContext;

    // Load the cycle context saved by the ambient runner
    let context = VisibleCycleContext::load().map_err(|e| {
        anyhow::anyhow!(
            "Failed to load visible cycle context: {}\nIs the ambient runner running?",
            e
        )
    })?;

    // Initialize provider (uses same auth as normal jcode)
    let (provider, registry) = init_provider_and_registry(&ProviderChoice::Auto, None).await?;

    // Register ambient tools (in addition to the normal tools)
    registry.register_ambient_tools().await;

    // Initialize safety system for ambient tools
    let safety = std::sync::Arc::new(crate::safety::SafetySystem::new());
    crate::tool::ambient::init_safety_system(safety);

    // Start TUI with ambient mode
    let terminal = init_tui_terminal()?;
    crate::tui::mermaid::init_picker();
    let mouse_capture = crate::config::config().display.mouse_capture;
    let keyboard_enhanced = tui::enable_keyboard_enhancement();
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste)?;
    if mouse_capture {
        crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;
    }

    let mut app = tui::App::new(provider, registry);
    app.set_ambient_mode(context.system_prompt, context.initial_message);

    // Set terminal title
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::SetTitle("🤖 jcode ambient cycle")
    );

    let result = app.run(terminal).await;

    // Cleanup terminal
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
    if mouse_capture {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    }
    if keyboard_enhanced {
        tui::disable_keyboard_enhancement();
    }
    ratatui::restore();
    crate::tui::mermaid::clear_image_state();

    // Save cycle result to file if end_ambient_cycle was called
    if let Some(cycle_result) = crate::tool::ambient::take_cycle_result() {
        let result_path = VisibleCycleContext::result_path()?;
        crate::storage::write_json(&result_path, &cycle_result)?;
        eprintln!("Ambient cycle result saved.");
    }

    result?;
    Ok(())
}

/// Run memory management commands
fn run_memory_command(cmd: MemoryCommand) -> Result<()> {
    use memory::{MemoryEntry, MemoryManager};

    let manager = MemoryManager::new();

    match cmd {
        MemoryCommand::List { scope, tag } => {
            let mut all_memories: Vec<MemoryEntry> = Vec::new();

            // Load based on scope
            if scope == "all" || scope == "project" {
                if let Ok(graph) = manager.load_project_graph() {
                    all_memories.extend(graph.all_memories().cloned());
                }
            }
            if scope == "all" || scope == "global" {
                if let Ok(graph) = manager.load_global_graph() {
                    all_memories.extend(graph.all_memories().cloned());
                }
            }

            // Filter by tag if specified
            if let Some(tag_filter) = tag {
                all_memories.retain(|m| m.tags.contains(&tag_filter));
            }

            // Sort by updated_at descending
            all_memories.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

            if all_memories.is_empty() {
                println!("No memories found.");
            } else {
                println!("Found {} memories:\n", all_memories.len());
                for entry in &all_memories {
                    let tags_str = if entry.tags.is_empty() {
                        String::new()
                    } else {
                        format!(" [{}]", entry.tags.join(", "))
                    };
                    let conf = entry.effective_confidence();
                    println!(
                        "- [{}] {}{}\n  id: {} (conf: {:.0}%, accessed: {}x)",
                        entry.category,
                        entry.content,
                        tags_str,
                        entry.id,
                        conf * 100.0,
                        entry.access_count
                    );
                    println!();
                }
            }
        }

        MemoryCommand::Search { query, semantic } => {
            if semantic {
                // Semantic search using embeddings
                match manager.find_similar(&query, 0.3, 20) {
                    Ok(results) => {
                        if results.is_empty() {
                            println!("No memories found matching '{}'", query);
                        } else {
                            println!(
                                "Found {} memories matching '{}' (semantic):\n",
                                results.len(),
                                query
                            );
                            for (entry, score) in results {
                                let tags_str = if entry.tags.is_empty() {
                                    String::new()
                                } else {
                                    format!(" [{}]", entry.tags.join(", "))
                                };
                                println!(
                                    "- [{}] {}{}\n  id: {} (score: {:.0}%)",
                                    entry.category,
                                    entry.content,
                                    tags_str,
                                    entry.id,
                                    score * 100.0
                                );
                                println!();
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Search failed: {}", e);
                    }
                }
            } else {
                // Keyword search
                match manager.search(&query) {
                    Ok(results) => {
                        if results.is_empty() {
                            println!("No memories found matching '{}'", query);
                        } else {
                            println!(
                                "Found {} memories matching '{}' (keyword):\n",
                                results.len(),
                                query
                            );
                            for entry in results {
                                let tags_str = if entry.tags.is_empty() {
                                    String::new()
                                } else {
                                    format!(" [{}]", entry.tags.join(", "))
                                };
                                println!(
                                    "- [{}] {}{}\n  id: {}",
                                    entry.category, entry.content, tags_str, entry.id
                                );
                                println!();
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Search failed: {}", e);
                    }
                }
            }
        }

        MemoryCommand::Export { output, scope } => {
            let mut all_memories: Vec<MemoryEntry> = Vec::new();

            if scope == "all" || scope == "project" {
                if let Ok(graph) = manager.load_project_graph() {
                    all_memories.extend(graph.all_memories().cloned());
                }
            }
            if scope == "all" || scope == "global" {
                if let Ok(graph) = manager.load_global_graph() {
                    all_memories.extend(graph.all_memories().cloned());
                }
            }

            let json = serde_json::to_string_pretty(&all_memories)?;
            std::fs::write(&output, json)?;
            println!("Exported {} memories to {}", all_memories.len(), output);
        }

        MemoryCommand::Import {
            input,
            scope,
            overwrite,
        } => {
            let content = std::fs::read_to_string(&input)?;
            let memories: Vec<MemoryEntry> = serde_json::from_str(&content)?;

            let mut imported = 0;
            let mut skipped = 0;

            for entry in memories {
                let result = if scope == "global" {
                    if !overwrite {
                        // Check if exists
                        if let Ok(graph) = manager.load_global_graph() {
                            if graph.get_memory(&entry.id).is_some() {
                                skipped += 1;
                                continue;
                            }
                        }
                    }
                    manager.remember_global(entry)
                } else {
                    if !overwrite {
                        if let Ok(graph) = manager.load_project_graph() {
                            if graph.get_memory(&entry.id).is_some() {
                                skipped += 1;
                                continue;
                            }
                        }
                    }
                    manager.remember_project(entry)
                };

                if result.is_ok() {
                    imported += 1;
                }
            }

            println!("Imported {} memories ({} skipped)", imported, skipped);
        }

        MemoryCommand::Stats => {
            let mut project_count = 0;
            let mut global_count = 0;
            let mut total_tags = std::collections::HashSet::new();
            let mut categories: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();

            if let Ok(graph) = manager.load_project_graph() {
                project_count = graph.memory_count();
                for entry in graph.all_memories() {
                    for tag in &entry.tags {
                        total_tags.insert(tag.clone());
                    }
                    *categories.entry(entry.category.to_string()).or_default() += 1;
                }
            }

            if let Ok(graph) = manager.load_global_graph() {
                global_count = graph.memory_count();
                for entry in graph.all_memories() {
                    for tag in &entry.tags {
                        total_tags.insert(tag.clone());
                    }
                    *categories.entry(entry.category.to_string()).or_default() += 1;
                }
            }

            println!("Memory Statistics:");
            println!("  Project memories: {}", project_count);
            println!("  Global memories:  {}", global_count);
            println!("  Total:            {}", project_count + global_count);
            println!("  Unique tags:      {}", total_tags.len());
            println!("\nBy category:");
            for (cat, count) in &categories {
                println!("  {}: {}", cat, count);
            }
        }

        MemoryCommand::ClearTest => {
            let test_dir = storage::jcode_dir()?.join("memory").join("test");
            if test_dir.exists() {
                let count = std::fs::read_dir(&test_dir)?.count();
                std::fs::remove_dir_all(&test_dir)?;
                println!("Cleared test memory storage ({} files)", count);
            } else {
                println!("Test memory storage is already empty");
            }
        }
    }

    Ok(())
}

/// Scan for running jcode servers
async fn debug_list_servers() -> Result<()> {
    let mut servers = Vec::new();

    // Scan XDG_RUNTIME_DIR
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));

    // Scan /tmp as well
    let scan_dirs = vec![runtime_dir, std::path::PathBuf::from("/tmp")];

    for dir in scan_dirs {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    // Look for jcode socket files (but not debug sockets)
                    if name.starts_with("jcode")
                        && name.ends_with(".sock")
                        && !name.contains("-debug")
                    {
                        servers.push(path);
                    }
                }
            }
        }
    }

    if servers.is_empty() {
        println!("No running jcode servers found.");
        println!("\nStart one with: jcode debug start");
        return Ok(());
    }

    println!("Running jcode servers:\n");

    for socket_path in servers {
        let debug_socket = {
            let filename = socket_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("jcode.sock");
            let debug_filename = filename.replace(".sock", "-debug.sock");
            socket_path.with_file_name(debug_filename)
        };

        // Check if server is alive and clean stale sockets when detected.
        let mut stale_main_removed = false;
        let alive = match tokio::net::UnixStream::connect(&socket_path).await {
            Ok(_) => true,
            Err(err)
                if err.kind() == std::io::ErrorKind::ConnectionRefused && socket_path.exists() =>
            {
                server::cleanup_socket_pair(&socket_path);
                stale_main_removed = true;
                false
            }
            Err(_) => false,
        };

        let mut stale_debug_removed = false;
        let debug_enabled = if debug_socket.exists() {
            match tokio::net::UnixStream::connect(&debug_socket).await {
                Ok(_) => true,
                Err(err)
                    if err.kind() == std::io::ErrorKind::ConnectionRefused
                        && debug_socket.exists() =>
                {
                    server::cleanup_socket_pair(&debug_socket);
                    stale_debug_removed = true;
                    false
                }
                Err(_) => false,
            }
        } else {
            false
        };

        // Try to get session count if debug is enabled
        let session_info = if debug_enabled {
            get_server_info(&debug_socket).await.unwrap_or_default()
        } else {
            String::new()
        };

        let status = if alive {
            if debug_enabled {
                format!("✓ running, debug: enabled{}", session_info)
            } else if stale_debug_removed {
                "✓ running, debug: disabled (removed stale debug socket)".to_string()
            } else {
                "✓ running, debug: disabled".to_string()
            }
        } else if stale_main_removed {
            "✗ stale socket removed".to_string()
        } else {
            "✗ not responding (stale socket?)".to_string()
        };

        println!("  {} ({})", socket_path.display(), status);
    }

    println!("\nUse -s/--socket to target a specific server:");
    println!("  jcode debug -s /path/to/socket.sock sessions");

    Ok(())
}

/// Get server info via debug socket
async fn get_server_info(debug_socket: &std::path::Path) -> Result<String> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    let stream = UnixStream::connect(debug_socket).await?;
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    // Send sessions command
    let request = serde_json::json!({
        "type": "debug_command",
        "id": 1,
        "command": "sessions",
    });
    let mut json = serde_json::to_string(&request)?;
    json.push('\n');
    writer.write_all(json.as_bytes()).await?;

    // Read response
    let mut line = String::new();
    let n = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        reader.read_line(&mut line),
    )
    .await??;
    if n == 0 {
        return Ok(String::new()); // Server disconnected
    }

    let response: serde_json::Value = serde_json::from_str(&line)?;
    if let Some(output) = response.get("output").and_then(|v| v.as_str()) {
        if let Ok(sessions) = serde_json::from_str::<Vec<String>>(output) {
            return Ok(format!(", sessions: {}", sessions.len()));
        }
    }

    Ok(String::new())
}

/// Start a new jcode server
async fn debug_start_server(arg: &str, socket_path: Option<String>) -> Result<()> {
    let socket = socket_path.unwrap_or_else(|| {
        if !arg.is_empty() {
            arg.to_string()
        } else {
            server::socket_path().to_string_lossy().to_string()
        }
    });

    let socket_pathbuf = std::path::PathBuf::from(&socket);

    // Check if server already running
    if socket_pathbuf.exists() {
        if tokio::net::UnixStream::connect(&socket_pathbuf)
            .await
            .is_ok()
        {
            eprintln!("Server already running at {}", socket);
            eprintln!("Use 'jcode debug list' to see all servers.");
            return Ok(());
        }
        // Stale socket, remove it
        server::cleanup_socket_pair(&socket_pathbuf);
    }

    // Also clean up debug socket
    let debug_socket = {
        let filename = socket_pathbuf
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("jcode.sock");
        let debug_filename = filename.replace(".sock", "-debug.sock");
        socket_pathbuf.with_file_name(debug_filename)
    };
    let _ = std::fs::remove_file(&debug_socket);

    eprintln!("Starting jcode server...");

    // Start server in background
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("serve");

    if socket != server::socket_path().to_string_lossy() {
        cmd.arg("--socket").arg(&socket);
    }

    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;

    // Wait for server to be ready
    let start = std::time::Instant::now();
    loop {
        if start.elapsed() > std::time::Duration::from_secs(10) {
            anyhow::bail!("Server failed to start within 10 seconds");
        }
        if socket_pathbuf.exists() {
            if tokio::net::UnixStream::connect(&socket_pathbuf)
                .await
                .is_ok()
            {
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    eprintln!("✓ Server started at {}", socket);

    // Check if debug socket is available
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    if debug_socket.exists() {
        eprintln!("✓ Debug socket at {}", debug_socket.display());
    } else {
        eprintln!("⚠ Debug socket not enabled. Add to ~/.jcode/config.toml:");
        eprintln!("  [display]");
        eprintln!("  debug_socket = true");
    }

    Ok(())
}

async fn run_login(choice: &ProviderChoice) -> Result<()> {
    match choice {
        ProviderChoice::Claude | ProviderChoice::ClaudeSubprocess => {
            if matches!(choice, ProviderChoice::ClaudeSubprocess) {
                eprintln!("Warning: Claude subprocess transport is deprecated. Direct Claude API mode is preferred.");
            }
            login_claude_flow().await?;
        }
        ProviderChoice::Openai => {
            login_openai_flow().await?;
        }
        ProviderChoice::Cursor => {
            login_cursor_flow()?;
        }
        ProviderChoice::Copilot => {
            login_copilot_flow()?;
        }
        ProviderChoice::Antigravity => {
            login_antigravity_flow()?;
        }
        ProviderChoice::Auto => {
            eprintln!("Choose a provider to log in:");
            eprintln!("  1. Claude (Claude Max)");
            eprintln!("  2. OpenAI (ChatGPT Pro)");
            eprintln!("  3. Cursor");
            eprintln!("  4. GitHub Copilot");
            eprintln!("  5. Antigravity");
            eprint!("\nEnter 1-5: ");
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            match input.trim() {
                "1" => login_claude_flow().await?,
                "2" => login_openai_flow().await?,
                "3" => login_cursor_flow()?,
                "4" => login_copilot_flow()?,
                "5" => login_antigravity_flow()?,
                _ => anyhow::bail!(
                    "Invalid choice. Use --provider claude|claude-subprocess|openai|cursor|copilot|antigravity"
                ),
            }
        }
    }
    Ok(())
}

async fn login_claude_flow() -> Result<()> {
    eprintln!("Logging in to Claude...");
    let tokens = auth::oauth::login_claude().await?;
    auth::oauth::save_claude_tokens(&tokens)?;
    eprintln!("Successfully logged in to Claude!");
    eprintln!("Stored at ~/.jcode/auth.json");
    Ok(())
}

async fn login_openai_flow() -> Result<()> {
    eprintln!("Logging in to OpenAI/Codex...");
    let tokens = auth::oauth::login_openai().await?;
    auth::oauth::save_openai_tokens(&tokens)?;
    eprintln!("Successfully logged in to OpenAI!");
    Ok(())
}

fn login_cursor_flow() -> Result<()> {
    eprintln!("Starting Cursor login...");
    let binary =
        std::env::var("JCODE_CURSOR_CLI_PATH").unwrap_or_else(|_| "cursor-agent".to_string());
    run_external_login_command(&binary, &["login"]).with_context(|| {
        format!(
            "Cursor login failed. Install Cursor Agent and run `{} login`.",
            binary
        )
    })?;
    eprintln!("Cursor login command completed.");
    Ok(())
}

fn login_copilot_flow() -> Result<()> {
    eprintln!("Starting GitHub Copilot login...");
    let (program, args, rendered) = provider::copilot::copilot_login_command();
    run_external_login_command_owned(&program, &args).with_context(|| {
        format!(
            "Copilot login failed. Install Copilot CLI (https://gh.io/copilot-cli) and run `{}`.",
            rendered
        )
    })?;
    eprintln!("Copilot login command completed.");
    Ok(())
}

fn login_antigravity_flow() -> Result<()> {
    eprintln!("Starting Antigravity login...");
    let binary =
        std::env::var("JCODE_ANTIGRAVITY_CLI_PATH").unwrap_or_else(|_| "antigravity".to_string());
    run_external_login_command(&binary, &["login"]).with_context(|| {
        format!(
            "Antigravity login failed. Check `{}` is installed and run `{} login`.",
            binary, binary
        )
    })?;
    eprintln!("Antigravity login command completed.");
    Ok(())
}

fn run_external_login_command(program: &str, args: &[&str]) -> Result<()> {
    let status = ProcessCommand::new(program)
        .args(args)
        .status()
        .with_context(|| format!("Failed to start command: {} {}", program, args.join(" ")))?;
    if !status.success() {
        anyhow::bail!(
            "Command exited with non-zero status: {} {} ({})",
            program,
            args.join(" "),
            status
        );
    }
    Ok(())
}

fn run_external_login_command_owned(program: &str, args: &[String]) -> Result<()> {
    let status = ProcessCommand::new(program)
        .args(args)
        .status()
        .with_context(|| format!("Failed to start command: {} {}", program, args.join(" ")))?;
    if !status.success() {
        anyhow::bail!(
            "Command exited with non-zero status: {} {} ({})",
            program,
            args.join(" "),
            status
        );
    }
    Ok(())
}

async fn run_client() -> Result<()> {
    let mut client = server::Client::connect().await?;

    // Check connection
    if !client.ping().await? {
        anyhow::bail!("Failed to ping server");
    }

    println!("Connected to J-Code server");
    println!("Type your message, or 'quit' to exit.\n");

    loop {
        print!("> ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        let input = input.trim();
        if input.is_empty() {
            continue;
        }

        if input == "quit" || input == "exit" {
            break;
        }

        match client.send_message(input).await {
            Ok(msg_id) => {
                // Read events until Done
                loop {
                    match client.read_event().await {
                        Ok(event) => {
                            use crate::protocol::ServerEvent;
                            match event {
                                ServerEvent::TextDelta { text } => {
                                    print!("{}", text);
                                    std::io::stdout().flush()?;
                                }
                                ServerEvent::Done { id } if id == msg_id => {
                                    break;
                                }
                                ServerEvent::Error { message, .. } => {
                                    eprintln!("Error: {}", message);
                                    break;
                                }
                                _ => {}
                            }
                        }
                        Err(e) => {
                            eprintln!("Event error: {}", e);
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("Error: {}", e);
            }
        }

        println!();
    }

    Ok(())
}

/// Run TUI client connected to server
async fn run_tui_client(resume_session: Option<String>) -> Result<()> {
    let terminal = init_tui_terminal()?;
    // Initialize mermaid image picker (queries terminal for graphics protocol support)
    crate::tui::mermaid::init_picker();
    let mouse_capture = crate::config::config().display.mouse_capture;
    let keyboard_enhanced = tui::enable_keyboard_enhancement();
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste)?;
    if mouse_capture {
        crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;
    }

    if let Some(ref session_id) = resume_session {
        set_current_session(session_id);
    }
    spawn_session_signal_watchers();

    // Use App in remote mode - same UI, connects to server
    let app = tui::App::new_for_remote(resume_session).await;
    let result = app.run_remote(terminal).await;

    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
    if mouse_capture {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    }
    if keyboard_enhanced {
        tui::disable_keyboard_enhancement();
    }
    ratatui::restore();
    crate::tui::mermaid::clear_image_state();

    let run_result = result?;

    // Check for special exit code (canary wrapper communication)
    if let Some(code) = run_result.exit_code {
        std::process::exit(code);
    }

    // Check for hot-reload request (no rebuild) - reload CLIENT binary
    if let Some(ref reload_session_id) = run_result.reload_session {
        hot_reload(reload_session_id)?;
    }

    // Check for hot-rebuild request (full git pull + cargo build + tests)
    if let Some(ref rebuild_session_id) = run_result.rebuild_session {
        hot_rebuild(rebuild_session_id)?;
    }

    Ok(())
}

#[cfg(unix)]
fn spawn_resume_in_new_terminal(
    exe: &std::path::Path,
    session_id: &str,
    cwd: &std::path::Path,
) -> Result<bool> {
    use std::process::{Command, Stdio};

    let mut candidates: Vec<String> = Vec::new();
    if let Ok(term) = std::env::var("JCODE_TERMINAL") {
        if !term.trim().is_empty() {
            candidates.push(term);
        }
    }
    candidates.extend(
        [
            "kitty",
            "wezterm",
            "alacritty",
            "gnome-terminal",
            "konsole",
            "xterm",
            "foot",
        ]
        .iter()
        .map(|s| s.to_string()),
    );

    for term in candidates {
        let mut cmd = Command::new(&term);
        cmd.current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        match term.as_str() {
            "kitty" => {
                cmd.args(["--title", "jcode resume", "-e"])
                    .arg(exe)
                    .arg("--resume")
                    .arg(session_id);
            }
            "wezterm" => {
                cmd.args([
                    "start",
                    "--always-new-process",
                    "--",
                    exe.to_string_lossy().as_ref(),
                    "--resume",
                    session_id,
                ]);
            }
            "alacritty" => {
                cmd.args(["-e"]).arg(exe).arg("--resume").arg(session_id);
            }
            "gnome-terminal" => {
                cmd.args(["--", exe.to_string_lossy().as_ref(), "--resume", session_id]);
            }
            "konsole" => {
                cmd.args(["-e"]).arg(exe).arg("--resume").arg(session_id);
            }
            "xterm" => {
                cmd.args(["-e"]).arg(exe).arg("--resume").arg(session_id);
            }
            "foot" => {
                cmd.args(["-e"]).arg(exe).arg("--resume").arg(session_id);
            }
            _ => continue,
        }

        if cmd.spawn().is_ok() {
            return Ok(true);
        }
    }

    Ok(false)
}

#[cfg(not(unix))]
fn spawn_resume_in_new_terminal(
    _exe: &std::path::Path,
    _session_id: &str,
    _cwd: &std::path::Path,
) -> Result<bool> {
    Ok(false)
}

/// Get the jcode repository directory (where the source code lives)
fn get_repo_dir() -> Option<std::path::PathBuf> {
    build::get_repo_dir()
}

/// Public accessor for repo dir (used by TUI)
pub fn main_get_repo_dir() -> Option<std::path::PathBuf> {
    build::get_repo_dir()
}

/// Check if updates are available (returns None if unable to check)
/// Only returns true if remote is AHEAD of local (not if local is ahead)
fn check_for_updates() -> Option<bool> {
    let repo_dir = get_repo_dir()?;

    // Fetch quietly
    let fetch = ProcessCommand::new("git")
        .args(["fetch", "-q"])
        .current_dir(&repo_dir)
        .output()
        .ok()?;

    if !fetch.status.success() {
        return None;
    }

    // Count commits that remote has but local doesn't
    // This returns 0 if local is equal to or ahead of remote
    let behind = ProcessCommand::new("git")
        .args(["rev-list", "--count", "HEAD..@{u}"])
        .current_dir(&repo_dir)
        .output()
        .ok()?;

    if behind.status.success() {
        let count: u32 = String::from_utf8_lossy(&behind.stdout)
            .trim()
            .parse()
            .unwrap_or(0);
        Some(count > 0)
    } else {
        None
    }
}

/// Auto-update: pull, build, and exec into new binary
fn run_auto_update() -> Result<()> {
    use std::os::unix::process::CommandExt;

    let repo_dir =
        get_repo_dir().ok_or_else(|| anyhow::anyhow!("Could not find jcode repository"))?;

    // Git pull (quiet)
    let pull = ProcessCommand::new("git")
        .args(["pull", "-q"])
        .current_dir(&repo_dir)
        .status()?;

    if !pull.success() {
        anyhow::bail!("git pull failed");
    }

    // Cargo build --release (show output for progress)
    eprintln!("Building new version...");
    let build = ProcessCommand::new("cargo")
        .args(["build", "--release"])
        .current_dir(&repo_dir)
        .status()?;

    if !build.success() {
        anyhow::bail!("cargo build failed");
    }

    if let Err(e) = build::install_local_release(&repo_dir) {
        eprintln!("Warning: install failed: {}", e);
    }

    // Get new version
    let hash = ProcessCommand::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(&repo_dir)
        .output()?;
    let hash = String::from_utf8_lossy(&hash.stdout);
    eprintln!("Updated to {}. Restarting...", hash.trim());

    // Exec into new binary with same args
    let exe = std::env::current_exe()?;
    let args: Vec<String> = std::env::args().skip(1).collect();

    let err = ProcessCommand::new(&exe)
        .args(&args)
        .arg("--no-update") // Prevent infinite update loop
        .exec();

    Err(anyhow::anyhow!("Failed to exec new binary {:?}: {}", exe, err))
}

/// Run the update process (manual)
fn run_update() -> Result<()> {
    let repo_dir =
        get_repo_dir().ok_or_else(|| anyhow::anyhow!("Could not find jcode repository"))?;

    eprintln!("Updating jcode from {}...", repo_dir.display());

    // Git pull
    eprintln!("Pulling latest changes...");
    let pull = ProcessCommand::new("git")
        .args(["pull"])
        .current_dir(&repo_dir)
        .status()?;

    if !pull.success() {
        anyhow::bail!("git pull failed");
    }

    // Cargo build --release
    eprintln!("Building...");
    let build = ProcessCommand::new("cargo")
        .args(["build", "--release"])
        .current_dir(&repo_dir)
        .status()?;

    if !build.success() {
        anyhow::bail!("cargo build failed");
    }

    if let Err(e) = build::install_local_release(&repo_dir) {
        eprintln!("Warning: install failed: {}", e);
    }

    // Get new version hash
    let hash = ProcessCommand::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(&repo_dir)
        .output()?;

    let hash = String::from_utf8_lossy(&hash.stdout);
    eprintln!("Successfully updated to {}", hash.trim());

    Ok(())
}

/// List available sessions for resume - interactive picker
fn list_sessions() -> Result<()> {
    match tui::session_picker::pick_session()? {
        Some(tui::session_picker::PickerResult::Selected(session_id)) => {
            // User selected a session - exec into jcode with that session
            use std::os::unix::process::CommandExt;
            let exe = std::env::current_exe()?;
            let cwd = std::env::current_dir()?;

            let err = ProcessCommand::new(&exe)
                .arg("--resume")
                .arg(&session_id)
                .current_dir(cwd)
                .exec();

            // exec() only returns on error
            Err(anyhow::anyhow!("Failed to exec {:?}: {}", exe, err))
        }
        Some(tui::session_picker::PickerResult::RestoreAllCrashed) => {
            let recovered = session::recover_crashed_sessions()?;
            if recovered.is_empty() {
                eprintln!("No crashed sessions found.");
                return Ok(());
            }

            eprintln!(
                "Recovered {} crashed session(s) from the last crash window.",
                recovered.len()
            );

            let exe = std::env::current_exe()?;
            let cwd = std::env::current_dir()?;
            let mut spawned = 0usize;
            let mut warned_no_terminal = false;

            for session_id in recovered {
                let mut session_cwd = cwd.clone();
                if let Ok(session) = session::Session::load(&session_id) {
                    if let Some(dir) = session.working_dir.as_deref() {
                        if std::path::Path::new(dir).is_dir() {
                            session_cwd = std::path::PathBuf::from(dir);
                        }
                    }
                }

                match spawn_resume_in_new_terminal(&exe, &session_id, &session_cwd) {
                    Ok(true) => {
                        spawned += 1;
                    }
                    Ok(false) => {
                        if !warned_no_terminal {
                            eprintln!("No supported terminal emulator found. Run these commands manually:");
                            warned_no_terminal = true;
                        }
                        eprintln!("  jcode --resume {}", session_id);
                    }
                    Err(e) => {
                        eprintln!("Failed to spawn session {}: {}", session_id, e);
                    }
                }
            }

            if spawned == 0 && warned_no_terminal {
                return Ok(());
            }

            if spawned == 0 {
                anyhow::bail!("Failed to spawn any recovered sessions");
            }

            Ok(())
        }
        None => {
            // User cancelled
            eprintln!("No session selected.");
            Ok(())
        }
    }
}

/// Self-development mode: run as canary with crash recovery wrapper
async fn run_self_dev(should_build: bool, resume_session: Option<String>) -> Result<()> {
    // Ensure self-dev env is set for subprocesses (server, agent, tools)
    std::env::set_var("JCODE_SELFDEV_MODE", "1");

    let repo_dir =
        get_repo_dir().ok_or_else(|| anyhow::anyhow!("Could not find jcode repository"))?;

    // Track if this is a fresh start (not resuming) before we move resume_session
    let is_fresh_start = resume_session.is_none();

    // Get or create session and mark as canary
    let session_id = if let Some(id) = resume_session {
        // Load existing session and ensure it's marked as canary
        if let Ok(mut session) = session::Session::load(&id) {
            if !session.is_canary {
                session.set_canary("self-dev");
                let _ = session.save();
            }
        }
        id
    } else {
        let mut session =
            session::Session::create(None, Some("Self-development session".to_string()));
        session.set_canary("self-dev");
        let _ = session.save();
        session.id.clone()
    };

    // Use target/release/jcode as the binary
    let target_binary = repo_dir.join("target/release/jcode");

    // Only build if explicitly requested with --build flag
    if should_build {
        eprintln!("Building release version...");

        let build_status = ProcessCommand::new("cargo")
            .args(["build", "--release"])
            .current_dir(&repo_dir)
            .status()?;

        if !build_status.success() {
            anyhow::bail!("Build failed");
        }

        eprintln!("✓ Build complete");
    }

    // Require binary to exist - developer builds manually otherwise
    if !target_binary.exists() {
        anyhow::bail!(
            "No binary found at {:?}\n\
             Run 'cargo build --release' first, or use 'jcode self-dev --build'.",
            target_binary
        );
    }

    let hash = build::current_git_hash(&repo_dir)?;
    let binary_path = target_binary.clone();

    // On fresh start (not resume), set current build as rollback safety net —
    // but only if no self-dev server is already running (to avoid clobbering
    // manifest state that other sessions depend on).
    if is_fresh_start {
        let selfdev_socket = std::path::Path::new(SELFDEV_SOCKET);
        let server_already_running = selfdev_socket.exists()
            && tokio::net::UnixStream::connect(SELFDEV_SOCKET)
                .await
                .is_ok();

        if !server_already_running {
            eprintln!("Setting {} as rollback safety net...", hash);

            build::install_version(&repo_dir, &hash)?;
            build::update_rollback_symlink(&hash)?;

            let mut manifest = build::BuildManifest::load()?;
            manifest.stable = Some(hash.clone());
            manifest.canary = None;
            manifest.canary_session = None;
            manifest.canary_status = None;
            manifest.save()?;
        }
    }

    // Launch wrapper process
    eprintln!("Starting self-dev session with {}...", hash);

    let exe = std::env::current_exe()?;
    let cwd = std::env::current_dir()?;

    // Use wrapper to handle crashes
    use std::os::unix::process::CommandExt;
    let err = ProcessCommand::new(&exe)
        .arg("canary-wrapper")
        .arg(&session_id)
        .arg(binary_path.to_string_lossy().as_ref())
        .arg(&hash)
        .current_dir(cwd)
        .exec();

    Err(anyhow::anyhow!("Failed to exec wrapper {:?}: {}", exe, err))
}

// Exit codes for canary wrapper communication
// Note: Rust panic exits with 101, so we avoid that for our signals
const EXIT_DONE: i32 = 0; // Clean exit, stop wrapper
const EXIT_RELOAD_REQUESTED: i32 = 42; // Agent wants to reload to new canary build
const EXIT_ROLLBACK_REQUESTED: i32 = 43; // Agent wants to rollback to stable

/// Path for self-dev shared server socket
const SELFDEV_SOCKET: &str = "/tmp/jcode-selfdev.sock";

/// Check if a server is actually responding (not just socket exists)
async fn is_server_alive(socket_path: &str) -> bool {
    if !std::path::Path::new(socket_path).exists() {
        return false;
    }
    tokio::net::UnixStream::connect(socket_path).await.is_ok()
}

/// Wrapper that runs client, spawning server as detached daemon if needed
async fn run_canary_wrapper(
    session_id: &str,
    initial_binary: &str,
    current_hash: &str,
) -> Result<()> {
    let initial_binary_path = std::path::PathBuf::from(initial_binary);
    let socket_path = SELFDEV_SOCKET.to_string();

    server::set_socket_path(&socket_path);

    // Check if server is already running
    let server_alive = is_server_alive(&socket_path).await;

    if !server_alive {
        // Server not running - spawn it as a detached daemon
        eprintln!("Starting self-dev server...");

        // Cleanup stale socket and hash file
        let _ = std::fs::remove_file(&socket_path);
        let _ = std::fs::remove_file(format!("{}.hash", socket_path));
        let _ = std::fs::remove_file(server::debug_socket_path());

        // Select binary to use - prefer the initial binary (target/release/jcode)
        // since it's guaranteed to be the most up-to-date when starting fresh
        let binary_path = if initial_binary_path.exists() {
            initial_binary_path.clone()
        } else {
            let canary_path = build::canary_binary_path().ok();
            let stable_path = build::stable_binary_path().ok();
            if canary_path.as_ref().map(|p| p.exists()).unwrap_or(false) {
                canary_path.unwrap()
            } else if stable_path.as_ref().map(|p| p.exists()).unwrap_or(false) {
                stable_path.unwrap()
            } else {
                anyhow::bail!("No binary found for server!");
            }
        };

        // Spawn server as detached daemon (not tied to this client's lifecycle)
        let cwd = std::env::current_dir().unwrap_or_default();
        std::process::Command::new(&binary_path)
            .arg("serve")
            .current_dir(&cwd)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .stdin(std::process::Stdio::null())
            .spawn()?;

        // Wait for server to be ready
        let start = std::time::Instant::now();
        loop {
            if start.elapsed() > std::time::Duration::from_secs(30) {
                anyhow::bail!("Server failed to start within 30 seconds");
            }
            if is_server_alive(&socket_path).await {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        eprintln!("Self-dev server ready on {}", socket_path);
    } else {
        // Server is already running - just connect to it.
        // Don't force a server restart on version mismatch: that would kill
        // all other connected sessions. The client/server protocol is
        // compatible across versions; explicit `/reload` can be used when a
        // server restart is actually desired.
        let hash_path = format!("{}.hash", socket_path);
        let server_hash = std::fs::read_to_string(&hash_path).unwrap_or_default();

        let server_ver = if server_hash.is_empty() {
            "unknown version"
        } else {
            server_hash.trim()
        };

        if !server_hash.is_empty() && server_hash.trim() != current_hash {
            eprintln!(
                "Connecting to existing self-dev server ({}) on {} (client built from {})",
                server_ver, socket_path, current_hash
            );
        } else {
            eprintln!(
                "Connecting to existing self-dev server ({}) on {}...",
                server_ver, socket_path
            );
        }
    }

    let session_name = id::extract_session_name(session_id)
        .map(|s| s.to_string())
        .unwrap_or_else(|| session_id.to_string());

    eprintln!("Starting TUI client...");
    set_current_session(session_id);
    spawn_session_signal_watchers();

    // Run client TUI
    let terminal = init_tui_terminal()?;
    // Initialize mermaid image picker (queries terminal for graphics protocol support)
    crate::tui::mermaid::init_picker();
    let mouse_capture = crate::config::config().display.mouse_capture;
    let keyboard_enhanced = tui::enable_keyboard_enhancement();
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste)?;
    if mouse_capture {
        crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;
    }

    let app = tui::App::new_for_remote(Some(session_id.to_string())).await;

    // Set terminal title
    let icon = id::session_icon(&session_name);
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::SetTitle(format!("{} jcode {} [self-dev]", icon, session_name))
    );

    let result = app.run_remote(terminal).await;

    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
    if mouse_capture {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    }
    if keyboard_enhanced {
        tui::disable_keyboard_enhancement();
    }
    ratatui::restore();
    crate::tui::mermaid::clear_image_state();

    let run_result = result?;

    // Check for hot-reload request (no rebuild)
    if let Some(ref reload_session_id) = run_result.reload_session {
        hot_reload(reload_session_id)?;
    }

    // Check for hot-rebuild request (full git pull + cargo build + tests)
    if let Some(ref rebuild_session_id) = run_result.rebuild_session {
        hot_rebuild(rebuild_session_id)?;
    }

    // Check if reload/rollback was requested - exec into new binary
    if let Some(code) = run_result.exit_code {
        if code == EXIT_RELOAD_REQUESTED || code == EXIT_ROLLBACK_REQUESTED {
            use std::os::unix::process::CommandExt;

            let action = if code == EXIT_RELOAD_REQUESTED {
                "reload"
            } else {
                "rollback"
            };
            eprintln!(
                "\n🔄 Client {} requested, restarting with new binary...",
                action
            );

            // Small delay for filesystem sync
            std::thread::sleep(std::time::Duration::from_millis(200));

            // Get the appropriate binary (canary for reload, rollback for rollback)
            let binary_path = if code == EXIT_RELOAD_REQUESTED {
                build::canary_binary_path().ok()
            } else {
                build::rollback_binary_path().ok()
            };

            let binary = binary_path
                .filter(|p| p.exists())
                .or_else(|| {
                    initial_binary_path
                        .exists()
                        .then(|| initial_binary_path.clone())
                })
                .ok_or_else(|| anyhow::anyhow!("No binary found for reload"))?;

            let cwd = std::env::current_dir()?;

            // Exec into the new binary with self-dev mode and session resume
            let err = ProcessCommand::new(&binary)
                .arg("self-dev")
                .arg("--resume")
                .arg(session_id)
                .current_dir(cwd)
                .exec();

            return Err(anyhow::anyhow!("Failed to exec {:?}: {}", binary, err));
        }
    }

    // Print resume info for normal exit
    eprintln!();
    eprintln!(
        "\x1b[33mSession \x1b[1m{}\x1b[0m\x1b[33m - to resume:\x1b[0m",
        session_name
    );
    eprintln!("  jcode --resume {}", session_id);
    eprintln!();

    Ok(())
}

/// Promote current canary to stable
fn run_promote() -> Result<()> {
    let mut manifest = build::BuildManifest::load()?;

    let canary_hash = manifest
        .canary
        .clone()
        .ok_or_else(|| anyhow::anyhow!("No canary build to promote"))?;

    eprintln!("Promoting canary {} to stable...", canary_hash);

    // Update symlink
    build::update_stable_symlink(&canary_hash)?;

    // Update manifest
    manifest.promote_to_stable(&canary_hash)?;

    eprintln!("✓ Build {} is now stable", canary_hash);
    eprintln!("Other sessions will auto-migrate to this version.");

    Ok(())
}

#[cfg(test)]
mod test_env {
    use std::ffi::OsString;
    use std::sync::{Mutex, OnceLock};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        let mutex = ENV_LOCK.get_or_init(|| Mutex::new(()));
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub struct TestEnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        prev_home: Option<OsString>,
        prev_test_session: Option<OsString>,
        _temp_home: tempfile::TempDir,
    }

    impl TestEnvGuard {
        fn new() -> anyhow::Result<Self> {
            let lock = lock_env();
            let temp_home = tempfile::Builder::new()
                .prefix("jcode-main-test-home-")
                .tempdir()?;
            let prev_home = std::env::var_os("JCODE_HOME");
            let prev_test_session = std::env::var_os("JCODE_TEST_SESSION");

            std::env::set_var("JCODE_HOME", temp_home.path());
            std::env::set_var("JCODE_TEST_SESSION", "1");

            Ok(Self {
                _lock: lock,
                prev_home,
                prev_test_session,
                _temp_home: temp_home,
            })
        }
    }

    impl Drop for TestEnvGuard {
        fn drop(&mut self) {
            if let Some(prev_home) = &self.prev_home {
                std::env::set_var("JCODE_HOME", prev_home);
            } else {
                std::env::remove_var("JCODE_HOME");
            }

            if let Some(prev_test_session) = &self.prev_test_session {
                std::env::set_var("JCODE_TEST_SESSION", prev_test_session);
            } else {
                std::env::remove_var("JCODE_TEST_SESSION");
            }
        }
    }

    pub fn setup() -> TestEnvGuard {
        TestEnvGuard::new().expect("failed to setup isolated test environment")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_SESSION_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_session_recovery_tracking() {
        let _guard = TEST_SESSION_LOCK.lock().unwrap();
        // Set a session ID
        set_current_session("test_session_123");

        // Verify it's stored correctly
        let guard = CURRENT_SESSION_ID.lock().unwrap();
        assert_eq!(guard.as_ref().unwrap(), "test_session_123");
    }

    #[test]
    fn test_session_recovery_message_format() {
        let _guard = TEST_SESSION_LOCK.lock().unwrap();
        // Set a unique session ID for this test
        let test_session = "session_format_test_12345";
        set_current_session(test_session);

        // Verify the session ID is accessible and forms a valid recovery command
        if let Ok(guard) = CURRENT_SESSION_ID.lock() {
            if let Some(session_id) = guard.as_ref() {
                // Verify the recovery command format is correct
                let expected_cmd = format!("jcode --resume {}", session_id);
                assert!(expected_cmd.starts_with("jcode --resume "));
                // Session ID should be non-empty
                assert!(!session_id.is_empty());
            } else {
                panic!("Session ID should be set");
            }
        }
    }

    #[test]
    fn test_provider_choice_arg_values() {
        assert_eq!(ProviderChoice::Claude.as_arg_value(), "claude");
        assert_eq!(
            ProviderChoice::ClaudeSubprocess.as_arg_value(),
            "claude-subprocess"
        );
        assert_eq!(ProviderChoice::Openai.as_arg_value(), "openai");
        assert_eq!(ProviderChoice::Cursor.as_arg_value(), "cursor");
        assert_eq!(ProviderChoice::Copilot.as_arg_value(), "copilot");
        assert_eq!(ProviderChoice::Antigravity.as_arg_value(), "antigravity");
        assert_eq!(ProviderChoice::Auto.as_arg_value(), "auto");
    }
}

#[cfg(test)]
mod selfdev_integration_tests {
    use super::*;

    // Simple null provider for testing
    struct TestProvider;

    #[async_trait::async_trait]
    impl provider::Provider for TestProvider {
        fn name(&self) -> &str {
            "test"
        }
        fn model(&self) -> String {
            "test".to_string()
        }
        fn available_models(&self) -> Vec<&'static str> {
            vec![]
        }

        fn available_models_display(&self) -> Vec<String> {
            vec![]
        }

        async fn prefetch_models(&self) -> anyhow::Result<()> {
            Ok(())
        }
        fn set_model(&self, _model: &str) -> anyhow::Result<()> {
            Ok(())
        }
        fn handles_tools_internally(&self) -> bool {
            false
        }
        async fn complete(
            &self,
            _messages: &[crate::message::Message],
            _tools: &[crate::message::ToolDefinition],
            _system: &str,
            _session_id: Option<&str>,
        ) -> anyhow::Result<crate::provider::EventStream> {
            unimplemented!()
        }

        fn fork(&self) -> Arc<dyn provider::Provider> {
            Arc::new(TestProvider)
        }
    }

    #[tokio::test]
    async fn test_selfdev_tool_registration() {
        let _env = super::test_env::setup();

        // Create a canary session
        let mut session = session::Session::create(None, Some("Test".to_string()));
        session.set_canary("test");

        // Verify session is canary
        assert!(session.is_canary, "Session should be marked as canary");

        // Create registry
        let provider = Arc::new(TestProvider) as Arc<dyn provider::Provider>;
        let registry = tool::Registry::new(provider).await;

        // Get tool names before
        let tools_before: Vec<String> = registry.tool_names().await;
        let has_selfdev_before = tools_before.contains(&"selfdev".to_string());

        // Register selfdev tools
        registry.register_selfdev_tools().await;

        // Get tool names after
        let tools_after: Vec<String> = registry.tool_names().await;
        let has_selfdev_after = tools_after.contains(&"selfdev".to_string());

        println!(
            "Before: selfdev={}, tools={:?}",
            has_selfdev_before,
            tools_before.len()
        );
        println!(
            "After: selfdev={}, tools={:?}",
            has_selfdev_after,
            tools_after.len()
        );

        assert!(has_selfdev_after, "selfdev should be registered");
    }
}

#[cfg(test)]
mod selfdev_e2e_tests {
    use super::*;

    #[tokio::test]
    async fn test_selfdev_session_and_registry() {
        let _env = super::test_env::setup();

        // 1. Create a canary session
        let mut session = session::Session::create(None, Some("Test E2E".to_string()));
        session.set_canary("test-build");
        let session_id = session.id.clone();
        session.save().expect("Failed to save session");

        // Verify session was saved correctly
        let loaded = session::Session::load(&session_id).expect("Failed to load session");
        assert!(loaded.is_canary, "Loaded session should be canary");

        // 2. Create registry
        struct TestProvider;
        #[async_trait::async_trait]
        impl provider::Provider for TestProvider {
            fn name(&self) -> &str {
                "test"
            }
            fn model(&self) -> String {
                "test".to_string()
            }
            fn available_models(&self) -> Vec<&'static str> {
                vec![]
            }

            fn available_models_display(&self) -> Vec<String> {
                vec![]
            }

            async fn prefetch_models(&self) -> anyhow::Result<()> {
                Ok(())
            }
            fn set_model(&self, _model: &str) -> anyhow::Result<()> {
                Ok(())
            }
            fn handles_tools_internally(&self) -> bool {
                false
            }
            async fn complete(
                &self,
                _messages: &[crate::message::Message],
                _tools: &[crate::message::ToolDefinition],
                _system: &str,
                _session_id: Option<&str>,
            ) -> anyhow::Result<crate::provider::EventStream> {
                unimplemented!()
            }

            fn fork(&self) -> Arc<dyn provider::Provider> {
                Arc::new(TestProvider)
            }
        }

        let provider = Arc::new(TestProvider) as Arc<dyn provider::Provider>;
        let registry = tool::Registry::new(provider.clone()).await;

        // 3. Check tools before selfdev registration
        let tools_before = registry.tool_names().await;
        assert!(
            !tools_before.contains(&"selfdev".to_string()),
            "selfdev should NOT be registered initially"
        );

        // 4. Register selfdev (simulating what init_mcp does when session.is_canary=true)
        registry.register_selfdev_tools().await;

        // 5. Check tools after
        let tools_after = registry.tool_names().await;
        assert!(
            tools_after.contains(&"selfdev".to_string()),
            "selfdev SHOULD be registered after register_selfdev_tools"
        );

        // 6. Test that the tool is executable
        let ctx = tool::ToolContext {
            session_id: session_id.clone(),
            message_id: "test".to_string(),
            tool_call_id: "test".to_string(),
            working_dir: None,
        };
        let result = registry
            .execute("selfdev", serde_json::json!({"action": "status"}), ctx)
            .await;

        println!("selfdev status result: {:?}", result);
        assert!(result.is_ok(), "selfdev tool should execute successfully");

        // 7. Cleanup
        let _ = std::fs::remove_file(
            crate::storage::jcode_dir()
                .unwrap()
                .join("sessions")
                .join(format!("{}.json", session_id)),
        );
    }
}
