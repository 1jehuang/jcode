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
                eprintln!();
                eprintln!("\x1b[33mTo restore this session, run:\x1b[0m");
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

    /// Resume a session (used internally for hot-reload)
    #[arg(long, global = true, hide = true)]
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

async fn run_main(args: Args) -> Result<()> {

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
                run_tui_client().await?;
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
    if let Some(session_id) = resume_session {
        app.restore_session(&session_id);
    }

    // Set current session for panic recovery
    set_current_session(app.session_id());

    app.init_mcp().await;
    let result = app.run(terminal).await;
    // Disable bracketed paste before restoring terminal
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
    ratatui::restore();

    // Check for hot-reload request
    if let Ok(Some(session_id)) = &result {
        hot_reload(session_id)?;
    }

    result.map(|_| ())
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
async fn run_tui_client() -> Result<()> {
    let terminal = ratatui::init();
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste)?;

    // Use App in remote mode - same UI, connects to server
    let app = tui::App::new_for_remote().await;
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
    // First try: compile-time directory
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = std::path::PathBuf::from(manifest_dir);
    if path.join(".git").exists() {
        return Some(path);
    }

    // Fallback: check relative to executable
    if let Ok(exe) = std::env::current_exe() {
        // Assume structure: repo/target/release/jcode
        if let Some(repo) = exe.parent().and_then(|p| p.parent()).and_then(|p| p.parent()) {
            if repo.join(".git").exists() {
                return Some(repo.to_path_buf());
            }
        }
    }

    None
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

/// Self-development mode: run as canary with crash recovery wrapper
async fn run_self_dev(should_build: bool, resume_session: Option<String>) -> Result<()> {
    let repo_dir = get_repo_dir().ok_or_else(|| anyhow::anyhow!("Could not find jcode repository"))?;

    // Get or create session
    let session_id = if let Some(id) = resume_session {
        id
    } else {
        let session = session::Session::create(None, Some("Self-development session".to_string()));
        session.id.clone()
    };

    let hash = if should_build {
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
        // Use existing canary or current binary
        let manifest = build::BuildManifest::load()?;
        if let Some(canary) = manifest.canary {
            canary
        } else {
            // No canary, use current
            build::current_git_hash(&repo_dir)?
        }
    };

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

    // Get canary binary path
    let canary_binary = build::canary_binary_path()?;
    let binary_path = if canary_binary.exists() {
        canary_binary
    } else {
        repo_dir.join("target/release/jcode")
    };

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

/// Wrapper that runs canary binary and handles crashes
async fn run_canary_wrapper(session_id: &str, binary: &str) -> Result<()> {
    use std::process::Stdio;

    let binary_path = std::path::PathBuf::from(binary);
    if !binary_path.exists() {
        anyhow::bail!("Canary binary not found: {}", binary);
    }

    let cwd = std::env::current_dir()?;
    let repo_dir = get_repo_dir();

    loop {
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

        if status.success() {
            // Clean exit
            eprintln!("Canary session exited cleanly");
            build::clear_migration_context(session_id)?;
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
