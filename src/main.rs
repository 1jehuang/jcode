mod agent;
mod auth;
mod auto_debug;
mod background;
mod build;
mod bus;
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
mod prompt;
mod protocol;
mod provider;
mod registry;
mod server;
mod session;
mod sidecar;
mod skill;
mod storage;
mod todo;
mod tool;
mod tui;
mod usage;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use provider::Provider;
use std::io::{self, Write};
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

#[derive(Debug, Clone, ValueEnum)]
enum ProviderChoice {
    Claude,
    ClaudeSubprocess,
    Openai,
    Auto,
}

#[derive(Parser, Debug)]
#[command(name = "jcode")]
#[command(version = env!("JCODE_VERSION"))]
#[command(about = "J-Code: A coding agent using Claude Max or ChatGPT Pro subscriptions")]
struct Args {
    /// Provider to use (claude, openai, or auto-detect)
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

    /// Custom socket path for server/client communication
    #[arg(long, global = true)]
    socket: Option<String>,

    /// Enable debug socket (broadcasts all TUI state changes)
    #[arg(long, global = true)]
    debug_socket: bool,

    /// Model to use (e.g., claude-opus-4-5-20251101, gpt-5.2-codex)
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

    // Check for updates unless --no-update is specified or running Update command
    if !args.no_update && !matches!(args.command, Some(Command::Update)) && args.resume.is_none() {
        if let Some(update_available) = check_for_updates() {
            if update_available {
                if args.auto_update {
                    eprintln!("Update available - auto-updating...");
                    if let Err(e) = run_auto_update() {
                        eprintln!(
                            "Auto-update failed: {}. Continuing with current version.",
                            e
                        );
                    }
                    // If we get here, exec failed or update failed
                } else {
                    eprintln!(
                        "\n📦 Update available! Run `jcode update` or `/reload` to update.\n"
                    );
                }
            }
        }
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
        Some(Command::CanaryWrapper { session_id, binary }) => {
            run_canary_wrapper(&session_id, &binary).await?;
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
        None => {
            // Auto-detect jcode repo and enable self-dev mode
            let cwd = std::env::current_dir()?;
            let in_jcode_repo = build::is_jcode_repo(&cwd);
            let already_in_selfdev = std::env::var("JCODE_SELFDEV_MODE").is_ok();

            if in_jcode_repo && !already_in_selfdev && !args.standalone {
                // Auto-start self-dev mode with wrapper
                eprintln!("📍 Detected jcode repository - enabling self-dev mode");
                eprintln!("   (use --standalone to disable auto-detection)\n");

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

                if !server_running {
                    // Clean up any stale sockets
                    let _ = std::fs::remove_file(server::socket_path());
                    let _ = std::fs::remove_file(server::debug_socket_path());

                    // Start server in background
                    eprintln!("Starting server...");
                    let exe = std::env::current_exe()?;
                    let mut child = std::process::Command::new(&exe)
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
        ProviderChoice::Claude | ProviderChoice::ClaudeSubprocess => {
            // Explicit Claude - use MultiProvider but prefer Claude
            eprintln!("Using Claude (with multi-provider support)");
            std::env::set_var("JCODE_ACTIVE_PROVIDER", "claude");
            Arc::new(provider::MultiProvider::with_preference(false))
        }
        ProviderChoice::Openai => {
            // Explicit OpenAI - use MultiProvider but prefer OpenAI
            eprintln!("Using OpenAI (with multi-provider support)");
            std::env::set_var("JCODE_ACTIVE_PROVIDER", "openai");
            Arc::new(provider::MultiProvider::with_preference(true))
        }
        ProviderChoice::Auto => {
            // Check if we have any credentials
            let has_claude = auth::claude::load_credentials().is_ok();
            let has_openai = auth::codex::load_credentials().is_ok();

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
                eprint!("\nEnter 1 or 2: ");
                io::stdout().flush()?;

                let mut input = String::new();
                io::stdin().read_line(&mut input)?;

                match input.trim() {
                    "1" => {
                        eprintln!("\nRun `claude` and complete the login flow, then retry.\n");
                        Arc::new(provider::MultiProvider::new())
                    }
                    "2" => {
                        let tokens = auth::oauth::login_openai().await?;
                        auth::oauth::save_openai_tokens(&tokens)?;
                        eprintln!("\nSuccessfully logged in to OpenAI!\n");
                        Arc::new(provider::MultiProvider::with_preference(true))
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
    let terminal = ratatui::init();
    // Initialize mermaid image picker (queries terminal for graphics protocol support)
    crate::tui::mermaid::init_picker();
    let mouse_capture = crate::config::config().display.mouse_capture;
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
    ratatui::restore();

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
            return Err(anyhow::anyhow!("Failed to exec: {}", err));
        } else {
            eprintln!(
                "Warning: Migration binary not found at {:?}, falling back to local binary",
                binary_path
            );
        }
    }

    // Get the binary path - prefer repo release if available
    let exe = if let Some(repo_dir) = get_repo_dir() {
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
    let err = ProcessCommand::new(&exe)
        .arg("--resume")
        .arg(session_id)
        .current_dir(cwd)
        .exec();

    // exec() only returns on error
    Err(anyhow::anyhow!("Failed to exec: {}", err))
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
    Err(anyhow::anyhow!("Failed to exec: {}", err))
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
    use tokio::net::UnixStream;

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

    let stream = UnixStream::connect(&debug_socket).await?;
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
    reader.read_line(&mut line).await?;

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
                        entry.category, entry.content, tags_str, entry.id,
                        conf * 100.0, entry.access_count
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
                            println!("Found {} memories matching '{}' (semantic):\n", results.len(), query);
                            for (entry, score) in results {
                                let tags_str = if entry.tags.is_empty() {
                                    String::new()
                                } else {
                                    format!(" [{}]", entry.tags.join(", "))
                                };
                                println!(
                                    "- [{}] {}{}\n  id: {} (score: {:.0}%)",
                                    entry.category, entry.content, tags_str, entry.id,
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
                            println!("Found {} memories matching '{}' (keyword):\n", results.len(), query);
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

        MemoryCommand::Import { input, scope, overwrite } => {
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
            let mut categories: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

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

        // Check if server is alive
        let alive = tokio::net::UnixStream::connect(&socket_path).await.is_ok();
        let debug_enabled =
            debug_socket.exists() && tokio::net::UnixStream::connect(&debug_socket).await.is_ok();

        // Try to get session count if debug is enabled
        let session_info = if debug_enabled {
            get_server_info(&debug_socket).await.unwrap_or_default()
        } else {
            String::new()
        };

        let status = if alive {
            if debug_enabled {
                format!("✓ running, debug: enabled{}", session_info)
            } else {
                "✓ running, debug: disabled".to_string()
            }
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
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        reader.read_line(&mut line),
    )
    .await??;

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
        let _ = std::fs::remove_file(&socket_pathbuf);
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
            eprintln!("Claude Code CLI provider uses Claude Code CLI credentials.");
            eprintln!("Run `claude` or `claude setup-token` to authenticate.");
        }
        ProviderChoice::Openai => {
            eprintln!("Logging in to OpenAI/Codex...");
            let tokens = auth::oauth::login_openai().await?;
            auth::oauth::save_openai_tokens(&tokens)?;
            eprintln!("Successfully logged in to OpenAI!");
        }
        ProviderChoice::Auto => {
            eprintln!("Please specify a provider: --provider claude or --provider openai");
        }
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
    let terminal = ratatui::init();
    // Initialize mermaid image picker (queries terminal for graphics protocol support)
    crate::tui::mermaid::init_picker();
    let mouse_capture = crate::config::config().display.mouse_capture;
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste)?;
    if mouse_capture {
        crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;
    }

    // Use App in remote mode - same UI, connects to server
    let app = tui::App::new_for_remote(resume_session).await;
    let result = app.run_remote(terminal).await;

    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
    if mouse_capture {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    }
    ratatui::restore();

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

    Err(anyhow::anyhow!("Failed to exec new binary: {}", err))
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
            Err(anyhow::anyhow!("Failed to exec: {}", err))
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
            if recovered.len() > 1 {
                eprintln!("Other recovered sessions:");
                for id in recovered.iter().skip(1) {
                    eprintln!("  jcode --resume {}", id);
                }
            }

            // Exec into the most recent recovered session
            use std::os::unix::process::CommandExt;
            let exe = std::env::current_exe()?;
            let cwd = std::env::current_dir()?;
            let first = &recovered[0];
            let err = ProcessCommand::new(&exe)
                .arg("--resume")
                .arg(first)
                .current_dir(cwd)
                .exec();
            Err(anyhow::anyhow!("Failed to exec: {}", err))
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

    // Only rebuild if explicitly requested with --build flag
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

    // On fresh start (not resume), set current build as stable
    // This gives us a safety net to rollback to if canary crashes
    if is_fresh_start {
        eprintln!("Setting {} as stable (safety net)...", hash);

        // Install this version and set as stable
        build::install_version(&repo_dir, &hash)?;
        build::update_stable_symlink(&hash)?;

        // Update manifest - clear any old canary, set stable
        let mut manifest = build::BuildManifest::load()?;
        manifest.stable = Some(hash.clone());
        manifest.canary = None;
        manifest.canary_session = None;
        manifest.canary_status = None;
        manifest.save()?;
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
        .current_dir(cwd)
        .exec();

    Err(anyhow::anyhow!("Failed to exec wrapper: {}", err))
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
async fn run_canary_wrapper(session_id: &str, initial_binary: &str) -> Result<()> {
    let initial_binary_path = std::path::PathBuf::from(initial_binary);
    let socket_path = SELFDEV_SOCKET.to_string();

    server::set_socket_path(&socket_path);

    // Check if server is already running
    let server_alive = is_server_alive(&socket_path).await;

    if !server_alive {
        // Server not running - spawn it as a detached daemon
        eprintln!("Starting self-dev server...");

        // Cleanup stale socket
        let _ = std::fs::remove_file(&socket_path);
        let _ = std::fs::remove_file(server::debug_socket_path());

        // Select binary to use
        let canary_path = build::canary_binary_path().ok();
        let stable_path = build::stable_binary_path().ok();

        let binary_path = if canary_path.as_ref().map(|p| p.exists()).unwrap_or(false) {
            canary_path.unwrap()
        } else if stable_path.as_ref().map(|p| p.exists()).unwrap_or(false) {
            stable_path.unwrap()
        } else if initial_binary_path.exists() {
            initial_binary_path.clone()
        } else {
            anyhow::bail!("No binary found for server!");
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
        eprintln!(
            "Connecting to existing self-dev server on {}...",
            socket_path
        );
    }

    let session_name = id::extract_session_name(session_id)
        .map(|s| s.to_string())
        .unwrap_or_else(|| session_id.to_string());

    eprintln!("Starting TUI client...");

    // Run client TUI
    let terminal = ratatui::init();
    // Initialize mermaid image picker (queries terminal for graphics protocol support)
    crate::tui::mermaid::init_picker();
    let mouse_capture = crate::config::config().display.mouse_capture;
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
    ratatui::restore();

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

            // Get the appropriate binary (canary for reload, stable for rollback)
            let binary_path = if code == EXIT_RELOAD_REQUESTED {
                build::canary_binary_path().ok()
            } else {
                build::stable_binary_path().ok()
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

            return Err(anyhow::anyhow!("Failed to exec: {}", err));
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
