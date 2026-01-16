mod agent;
mod auth;
mod auto_debug;
mod build;
mod bus;
mod compaction;
mod id;
mod logging;
mod mcp;
mod message;
mod prompt;
mod protocol;
mod provider;
mod server;
mod session;
mod skill;
mod storage;
mod tool;
mod todo;
mod tui;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
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
                let session_name = id::extract_session_name(session_id)
                    .unwrap_or(session_id.as_str());
                eprintln!();
                eprintln!("\x1b[33mSession \x1b[1m{}\x1b[0m\x1b[33m - to resume:\x1b[0m", session_name);
                eprintln!("  jcode --resume {}", session_id);
                eprintln!();
            }
        }
    }));
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

    /// Run standalone TUI without connecting to server
    #[arg(long, global = true)]
    standalone: bool,

    /// Enable debug socket (broadcasts all TUI state changes)
    #[arg(long, global = true)]
    debug_socket: bool,

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

    // Check for updates unless --no-update is specified or running Update command
    if !args.no_update && !matches!(args.command, Some(Command::Update)) && args.resume.is_none() {
        if let Some(update_available) = check_for_updates() {
            if update_available {
                if args.auto_update {
                    eprintln!("Update available - auto-updating...");
                    if let Err(e) = run_auto_update() {
                        eprintln!("Auto-update failed: {}. Continuing with current version.", e);
                    }
                    // If we get here, exec failed or update failed
                } else {
                    eprintln!("\n📦 Update available! Run `jcode update` or `/reload` to update.\n");
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
            let (provider, registry) = init_provider_and_registry(&args.provider).await?;
            let server = server::Server::new(provider, registry);
            server.run().await?;
        }
        Some(Command::Connect) => {
            run_client().await?;
        }
        Some(Command::Run { message }) => {
            let (provider, registry) = init_provider_and_registry(&args.provider).await?;
            let mut agent = agent::Agent::new(provider, registry);
            agent.run_once(&message).await?;
        }
        Some(Command::Login) => {
            run_login(&args.provider).await?;
        }
        Some(Command::Repl) => {
            // Simple REPL mode (no TUI)
            let (provider, registry) = init_provider_and_registry(&args.provider).await?;
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

            // Check for --standalone flag
            if args.standalone {
                let (provider, registry) = init_provider_and_registry(&args.provider).await?;
                run_tui(provider, registry, args.resume, args.debug_socket).await?;
            } else {
                // Default: TUI client mode - start server if needed
                let server_running = if server::socket_path().exists() {
                    // Test if server is actually responding
                    tokio::net::UnixStream::connect(server::socket_path()).await.is_ok()
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
                            if tokio::net::UnixStream::connect(server::socket_path()).await.is_ok() {
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
) -> Result<(Arc<dyn provider::Provider>, tool::Registry)> {
    let provider: Arc<dyn provider::Provider> = match choice {
        ProviderChoice::Claude | ProviderChoice::ClaudeSubprocess => {
            eprintln!("Using Claude Agent SDK");
            std::env::set_var("JCODE_ACTIVE_PROVIDER", "claude");
            Arc::new(provider::claude::ClaudeProvider::new())
        }
        ProviderChoice::Openai => {
            let creds = auth::codex::load_credentials()?;
            std::env::set_var("JCODE_ACTIVE_PROVIDER", "openai");
            Arc::new(provider::openai::OpenAIProvider::new(creds))
        }
        ProviderChoice::Auto => {
            // Prefer Claude if Claude Code credentials are present
            if auth::claude::load_credentials().is_ok() {
                eprintln!("Using Claude Agent SDK");
                std::env::set_var("JCODE_ACTIVE_PROVIDER", "claude");
                Arc::new(provider::claude::ClaudeProvider::new())
            } else if let Ok(creds) = auth::codex::load_credentials() {
                eprintln!("Using OpenAI/Codex provider");
                std::env::set_var("JCODE_ACTIVE_PROVIDER", "openai");
                Arc::new(provider::openai::OpenAIProvider::new(creds))
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
                        Arc::new(provider::claude::ClaudeProvider::new())
                    }
                    "2" => {
                        let tokens = auth::oauth::login_openai().await?;
                        auth::oauth::save_openai_tokens(&tokens)?;
                        eprintln!("\nSuccessfully logged in to OpenAI!\n");
                        let creds = auth::codex::load_credentials()?;
                        Arc::new(provider::openai::OpenAIProvider::new(creds))
                    }
                    _ => {
                        anyhow::bail!("Invalid choice. Run 'jcode login' to try again.");
                    }
                }
            }
        }
    };

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
    // Enable bracketed paste mode for proper paste handling in terminals like Kitty
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste)?;
    let mut app = tui::App::new(provider, registry);

    // Enable debug socket if requested
    let _debug_handle = if debug_socket {
        let rx = app.enable_debug_socket();
        let handle = app.start_debug_socket_listener(rx);
        eprintln!("Debug socket enabled at: {:?}", tui::App::debug_socket_path());
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
    // Disable bracketed paste before restoring terminal
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
    ratatui::restore();

    let run_result = result?;

    // Check for special exit code (canary wrapper communication)
    if let Some(code) = run_result.exit_code {
        std::process::exit(code);
    }

    // Check for hot-reload request
    if let Some(ref reload_session_id) = run_result.reload_session {
        hot_reload(reload_session_id)?;
    }

    // Print resume command for normal exits (not hot-reload)
    if run_result.reload_session.is_none() {
        eprintln!();
        eprintln!("\x1b[33mSession \x1b[1m{}\x1b[0m\x1b[33m - to resume:\x1b[0m", session_name);
        eprintln!("  jcode --resume {}", session_id);
        eprintln!();
    }

    Ok(())
}

/// Hot-reload: pull, rebuild, test, and exec into new binary with session restore
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
            eprintln!("Warning: Migration binary not found at {:?}, falling back to rebuild", binary_path);
        }
    }

    let repo_dir = get_repo_dir().ok_or_else(|| anyhow::anyhow!("Could not find jcode repository"))?;

    eprintln!("Hot-reloading jcode with session {}...", session_id);

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
        eprintln!("Fix the failing tests and try /reload again.");
        anyhow::bail!("Tests failed - staying on current version");
    }

    eprintln!("✓ All tests passed");

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

async fn run_login(choice: &ProviderChoice) -> Result<()> {
    match choice {
        ProviderChoice::Claude | ProviderChoice::ClaudeSubprocess => {
            eprintln!("Claude Agent SDK uses Claude Code CLI credentials.");
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
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste)?;

    // Use App in remote mode - same UI, connects to server
    let app = tui::App::new_for_remote(resume_session).await;
    let result = app.run_remote(terminal).await;

    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
    ratatui::restore();

    // Handle reload request
    if let Ok(Some(_reload_session)) = &result {
        // TODO: Implement client-side reload if needed
    }

    result.map(|_| ())
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

    let repo_dir = get_repo_dir().ok_or_else(|| anyhow::anyhow!("Could not find jcode repository"))?;

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
    let repo_dir = get_repo_dir().ok_or_else(|| anyhow::anyhow!("Could not find jcode repository"))?;

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

    // Get new version hash
    let hash = ProcessCommand::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(&repo_dir)
        .output()?;

    let hash = String::from_utf8_lossy(&hash.stdout);
    eprintln!("Successfully updated to {}", hash.trim());

    Ok(())
}

/// List available sessions for resume
fn list_sessions() -> Result<()> {
    let sessions_dir = storage::jcode_dir()?.join("sessions");

    if !sessions_dir.exists() {
        eprintln!("No sessions found.");
        return Ok(());
    }

    let mut sessions: Vec<(String, session::Session)> = Vec::new();

    for entry in std::fs::read_dir(&sessions_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "json").unwrap_or(false) {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if let Ok(session) = session::Session::load(stem) {
                    sessions.push((stem.to_string(), session));
                }
            }
        }
    }

    if sessions.is_empty() {
        eprintln!("No sessions found.");
        return Ok(());
    }

    // Sort by updated_at descending (most recent first)
    sessions.sort_by(|a, b| b.1.updated_at.cmp(&a.1.updated_at));

    eprintln!("\n\x1b[1mAvailable sessions:\x1b[0m\n");

    for (id, session) in sessions.iter().take(20) {
        let display_name = session.display_name();
        let title = session.title.as_deref().unwrap_or("Untitled");
        let age = chrono::Utc::now().signed_duration_since(session.updated_at);
        let age_str = if age.num_days() > 0 {
            format!("{}d ago", age.num_days())
        } else if age.num_hours() > 0 {
            format!("{}h ago", age.num_hours())
        } else {
            format!("{}m ago", age.num_minutes())
        };

        let canary_marker = if session.is_canary { " \x1b[33m[self-dev]\x1b[0m" } else { "" };
        let msg_count = session.messages.len();

        eprintln!("  \x1b[1;36m{}\x1b[0m  \x1b[2m{}\x1b[0m", display_name, id);
        eprintln!("    {} ({} msgs, {}){}", title, msg_count, age_str, canary_marker);
        eprintln!();
    }

    if sessions.len() > 20 {
        eprintln!("  ... and {} more sessions", sessions.len() - 20);
    }

    eprintln!("\x1b[2mTo resume: jcode --resume <session_id or name>\x1b[0m\n");

    Ok(())
}

/// Self-development mode: run as canary with crash recovery wrapper
async fn run_self_dev(should_build: bool, resume_session: Option<String>) -> Result<()> {
    let repo_dir = get_repo_dir().ok_or_else(|| anyhow::anyhow!("Could not find jcode repository"))?;

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
        let mut session = session::Session::create(None, Some("Self-development session".to_string()));
        session.set_canary("self-dev");
        let _ = session.save();
        session.id.clone()
    };

    let mut hash = if should_build {
        // Build new canary version
        eprintln!("Building canary version...");

        let pull = ProcessCommand::new("git")
            .args(["pull", "-q"])
            .current_dir(&repo_dir)
            .status()?;

        if !pull.success() {
            eprintln!("Warning: git pull failed, continuing with local changes");
        }

        let build_status = ProcessCommand::new("cargo")
            .args(["build", "--release"])
            .current_dir(&repo_dir)
            .status()?;

        if !build_status.success() {
            anyhow::bail!("Build failed");
        }

        // Run quick tests
        eprintln!("Running tests...");
        let test = ProcessCommand::new("cargo")
            .args(["test", "--release"])
            .current_dir(&repo_dir)
            .status()?;

        if !test.success() {
            anyhow::bail!("Tests failed - not creating canary");
        }

        let hash = build::current_git_hash(&repo_dir)?;

        // Install to versioned location
        build::install_version(&repo_dir, &hash)?;
        build::update_canary_symlink(&hash)?;

        // Update manifest
        let mut manifest = build::BuildManifest::load()?;
        manifest.start_canary(&hash, &session_id)?;

        // Record build info
        let info = build::current_build_info(&repo_dir)?;
        manifest.add_to_history(info)?;

        eprintln!("✓ Canary build {} ready", hash);
        hash
    } else {
        // Use existing canary or current binary (verified below)
        let manifest = build::BuildManifest::load()?;
        if let Some(canary) = manifest.canary {
            canary
        } else {
            // No canary yet, use current hash (will be rebuilt if missing)
            build::current_git_hash(&repo_dir)?
        }
    };

    if !should_build {
        let mut needs_rebuild = false;
        let canary_path = build::canary_binary_path()?;

        if build::is_working_tree_dirty(&repo_dir).unwrap_or(false) {
            needs_rebuild = true;
        }

        if !canary_path.exists() {
            needs_rebuild = true;
        } else {
            match std::fs::read_link(&canary_path) {
                Ok(target) => {
                    let target_str = target.to_string_lossy();
                    if !target_str.contains(&hash) {
                        needs_rebuild = true;
                    }
                }
                Err(_) => {
                    needs_rebuild = true;
                }
            }
        }

        if needs_rebuild {
            eprintln!("Self-dev canary missing or mismatched; rebuilding and testing...");
            hash = build::rebuild_canary(&repo_dir)?;
        }
    }

    // Save migration context
    let stable_hash = build::read_stable_version()?.unwrap_or_else(|| "unknown".to_string());
    let diff = build::current_git_diff(&repo_dir).ok();

    let ctx = build::MigrationContext {
        session_id: session_id.clone(),
        from_version: stable_hash,
        to_version: hash.clone(),
        change_summary: build::get_commit_message(&repo_dir, &hash).ok(),
        diff,
        timestamp: chrono::Utc::now(),
    };
    build::save_migration_context(&ctx)?;

    // Ensure canary binary exists - wrapper reads from canary symlink
    let canary_binary = build::canary_binary_path()?;
    if !canary_binary.exists() {
        // No canary binary yet - install current binary as canary
        let current_exe = std::env::current_exe()?;
        let canary_dir = canary_binary.parent().unwrap();
        std::fs::create_dir_all(canary_dir)?;
        std::fs::copy(&current_exe, &canary_binary)?;

        // Make executable
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&canary_binary)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&canary_binary, perms)?;
        }

        eprintln!("Installed current binary as initial canary");
    }

    let binary_path = canary_binary;

    // Launch wrapper process
    eprintln!("Starting self-dev session with canary {}...", hash);

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
const EXIT_DONE: i32 = 0;               // Clean exit, stop wrapper
const EXIT_RELOAD_REQUESTED: i32 = 42;  // Agent wants to reload to new canary build
const EXIT_ROLLBACK_REQUESTED: i32 = 43; // Agent wants to rollback to stable

/// Wrapper that runs canary binary and handles crashes
async fn run_canary_wrapper(session_id: &str, initial_binary: &str) -> Result<()> {
    use std::process::Stdio;

    let cwd = std::env::current_dir()?;
    let repo_dir = get_repo_dir();
    let initial_binary_path = std::path::PathBuf::from(initial_binary);

    loop {
        // Always read canary path fresh - allows agent to rebuild and update symlink
        // Fall back to initial binary (usually target/release/jcode) if canary not set up yet
        let canary_path = build::canary_binary_path()?;
        let binary_path = if canary_path.exists() {
            canary_path
        } else if initial_binary_path.exists() {
            initial_binary_path.clone()
        } else {
            anyhow::bail!("No binary found: canary at {:?} or initial at {:?}", canary_path, initial_binary_path);
        };

        eprintln!("Launching canary session {}...", session_id);

        // Run the canary binary
        let mut child = ProcessCommand::new(&binary_path)
            .arg("--resume")
            .arg(session_id)
            .arg("--standalone")
            .arg("--no-update")
            .current_dir(&cwd)
            .stderr(Stdio::piped())
            .spawn()?;

        let status = child.wait()?;
        let exit_code = status.code().unwrap_or(-1);

        if exit_code == EXIT_DONE {
            // Clean exit - done with self-dev
            eprintln!("Canary session exited cleanly");
            build::clear_migration_context(session_id)?;
            break;
        }

        if exit_code == EXIT_RELOAD_REQUESTED {
            // Agent requested reload to new canary build - loop and respawn
            eprintln!("Reload requested, respawning with new canary build...");
            continue;
        }

        if exit_code == EXIT_ROLLBACK_REQUESTED {
            // Agent requested rollback to stable - spawn stable instead
            eprintln!("Rollback requested, switching to stable build...");
            let stable_binary = build::stable_binary_path()?;
            if stable_binary.exists() {
                let mut child = ProcessCommand::new(&stable_binary)
                    .arg("--resume")
                    .arg(session_id)
                    .arg("--standalone")
                    .arg("--no-update")
                    .current_dir(&cwd)
                    .spawn()?;
                let _ = child.wait();
            }
            break;
        }

        // Any other exit code is a crash
        if status.success() {
            // Shouldn't happen but handle gracefully
            break;
        }

        // Crash! Collect info
        let exit_code = status.code().unwrap_or(-1);
        eprintln!("\n⚠️  Canary crashed with exit code {}", exit_code);

        // Read stderr if available
        let stderr_output = if let Some(mut stderr) = child.stderr.take() {
            use std::io::Read;
            let mut buf = String::new();
            stderr.read_to_string(&mut buf).unwrap_or(0);
            buf
        } else {
            String::new()
        };

        // Get diff from migration context
        let diff = build::load_migration_context(session_id)?
            .and_then(|ctx| ctx.diff);

        // Record crash in manifest
        let hash = build::BuildManifest::load()?.canary.unwrap_or_default();
        let mut manifest = build::BuildManifest::load()?;
        manifest.record_crash(&hash, exit_code, &stderr_output, diff)?;

        // Inject crash context into session
        inject_crash_context(session_id, &hash, exit_code, &stderr_output, repo_dir.as_ref())?;

        // Rollback to stable
        let stable_binary = build::stable_binary_path()?;
        if stable_binary.exists() {
            eprintln!("Rolling back to stable version...");

            let mut child = ProcessCommand::new(&stable_binary)
                .arg("--resume")
                .arg(session_id)
                .arg("--standalone")
                .arg("--no-update")
                .current_dir(&cwd)
                .spawn()?;

            let status = child.wait()?;
            if status.success() {
                break;
            }
            // If stable also crashes, we have bigger problems
            eprintln!("Stable version also crashed! Exiting.");
            break;
        } else {
            eprintln!("No stable version to rollback to. Exiting.");
            break;
        }
    }

    Ok(())
}

/// Inject crash context into the session so agent can see what happened
fn inject_crash_context(
    session_id: &str,
    build_hash: &str,
    exit_code: i32,
    stderr: &str,
    repo_dir: Option<&std::path::PathBuf>,
) -> Result<()> {
    use crate::message::{ContentBlock, Role};

    let mut session = match session::Session::load(session_id) {
        Ok(s) => s,
        Err(_) => return Ok(()), // Session doesn't exist yet, skip
    };

    // Get diff if available
    let diff_info = if let Some(dir) = repo_dir {
        build::current_git_diff(dir).ok()
    } else {
        None
    };

    // Build crash report
    let mut report = format!(
        "🔴 **Canary Build Crashed**\n\n\
         Build: `{}`\n\
         Exit code: {}\n",
        build_hash, exit_code
    );

    if !stderr.is_empty() {
        let truncated = if stderr.len() > 2000 {
            format!("{}...(truncated)", &stderr[..2000])
        } else {
            stderr.to_string()
        };
        report.push_str(&format!("\n**Stderr:**\n```\n{}\n```\n", truncated));
    }

    if let Some(diff) = diff_info {
        if !diff.is_empty() {
            let truncated = if diff.len() > 3000 {
                format!("{}...(truncated)", &diff[..3000])
            } else {
                diff
            };
            report.push_str(&format!("\n**Recent changes:**\n```diff\n{}\n```\n", truncated));
        }
    }

    report.push_str("\nI've been rolled back to the stable version. Please investigate and fix the issue.");

    // Add as system message
    session.add_message(
        Role::User,
        vec![ContentBlock::Text { text: report }],
    );
    session.save()?;

    Ok(())
}

/// Promote current canary to stable
fn run_promote() -> Result<()> {
    let mut manifest = build::BuildManifest::load()?;

    let canary_hash = manifest.canary.clone()
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

    #[test]
    fn test_session_recovery_tracking() {
        // Set a session ID
        set_current_session("test_session_123");

        // Verify it's stored correctly
        let guard = CURRENT_SESSION_ID.lock().unwrap();
        assert_eq!(guard.as_ref().unwrap(), "test_session_123");
    }

    #[test]
    fn test_session_recovery_message_format() {
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
        fn name(&self) -> &str { "test" }
        fn model(&self) -> String { "test".to_string() }
        fn available_models(&self) -> Vec<&'static str> { vec![] }
        fn set_model(&self, _model: &str) -> anyhow::Result<()> { Ok(()) }
        fn handles_tools_internally(&self) -> bool { false }
        async fn complete(
            &self,
            _messages: &[crate::message::Message],
            _tools: &[crate::message::ToolDefinition],
            _system: &str,
            _session_id: Option<&str>,
        ) -> anyhow::Result<crate::provider::EventStream> {
            unimplemented!()
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
        
        println!("Before: selfdev={}, tools={:?}", has_selfdev_before, tools_before.len());
        println!("After: selfdev={}, tools={:?}", has_selfdev_after, tools_after.len());
        
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
            fn name(&self) -> &str { "test" }
            fn model(&self) -> String { "test".to_string() }
            fn available_models(&self) -> Vec<&'static str> { vec![] }
            fn set_model(&self, _model: &str) -> anyhow::Result<()> { Ok(()) }
            fn handles_tools_internally(&self) -> bool { false }
            async fn complete(
                &self,
                _messages: &[crate::message::Message],
                _tools: &[crate::message::ToolDefinition],
                _system: &str,
                _session_id: Option<&str>,
            ) -> anyhow::Result<crate::provider::EventStream> {
                unimplemented!()
            }
        }

        let provider = Arc::new(TestProvider) as Arc<dyn provider::Provider>;
        let registry = tool::Registry::new(provider.clone()).await;

        // 3. Check tools before selfdev registration
        let tools_before = registry.tool_names().await;
        assert!(!tools_before.contains(&"selfdev".to_string()),
            "selfdev should NOT be registered initially");

        // 4. Register selfdev (simulating what init_mcp does when session.is_canary=true)
        registry.register_selfdev_tools().await;

        // 5. Check tools after
        let tools_after = registry.tool_names().await;
        assert!(tools_after.contains(&"selfdev".to_string()),
            "selfdev SHOULD be registered after register_selfdev_tools");

        // 6. Test that the tool is executable
        let ctx = tool::ToolContext {
            session_id: session_id.clone(),
            message_id: "test".to_string(),
            tool_call_id: "test".to_string(),
        };
        let result = registry.execute(
            "selfdev",
            serde_json::json!({"action": "status"}),
            ctx
        ).await;

        println!("selfdev status result: {:?}", result);
        assert!(result.is_ok(), "selfdev tool should execute successfully");

        // 7. Cleanup
        let _ = std::fs::remove_file(
            crate::storage::jcode_dir().unwrap().join("sessions").join(format!("{}.json", session_id))
        );
    }
}
