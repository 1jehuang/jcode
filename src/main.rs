mod agent;
mod auth;
mod auto_debug;
mod bus;
mod id;
mod logging;
mod message;
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
use std::process::Command as ProcessCommand;
use std::sync::Arc;

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

    /// Log tool inputs/outputs and token usage to stderr
    #[arg(long, global = true)]
    trace: bool,

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
}

#[tokio::main]
async fn main() -> Result<()> {
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
    if !args.no_update && !matches!(args.command, Some(Command::Update)) {
        if let Some(update_available) = check_for_updates() {
            if update_available {
                eprintln!("Update available! Run `jcode update` to update.");
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
        None => {
            // Default: TUI mode
            let (provider, registry) = init_provider_and_registry(&args.provider).await?;
            run_tui(provider, registry).await?;
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

async fn run_tui(provider: Arc<dyn provider::Provider>, registry: tool::Registry) -> Result<()> {
    let terminal = ratatui::init();
    let app = tui::App::new(provider, registry);
    let result = app.run(terminal).await;
    ratatui::restore();
    result
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
            Ok(result) => {
                if let Some(status) = result.get("status") {
                    if status == "ok" {
                        // Output was already printed by server
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

    // Get local HEAD
    let local = ProcessCommand::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&repo_dir)
        .output()
        .ok()?;

    // Get remote HEAD
    let remote = ProcessCommand::new("git")
        .args(["rev-parse", "@{u}"])
        .current_dir(&repo_dir)
        .output()
        .ok()?;

    if local.status.success() && remote.status.success() {
        let local_hash = String::from_utf8_lossy(&local.stdout);
        let remote_hash = String::from_utf8_lossy(&remote.stdout);
        Some(local_hash.trim() != remote_hash.trim())
    } else {
        None
    }
}

/// Run the update process
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
