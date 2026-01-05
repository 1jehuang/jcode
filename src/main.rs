mod agent;
mod auth;
mod message;
mod provider;
mod server;
mod skill;
mod tool;
mod tui;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use std::io::{self, Write};

#[derive(Debug, Clone, ValueEnum)]
enum ProviderChoice {
    Claude,
    ClaudeSubprocess,
    Openai,
    Auto,
}

#[derive(Parser, Debug)]
#[command(name = "jcode")]
#[command(about = "J-Code: A coding agent using Claude Max or ChatGPT Pro subscriptions")]
struct Args {
    /// Provider to use (claude, openai, or auto-detect)
    #[arg(short, long, default_value = "auto", global = true)]
    provider: ProviderChoice,

    /// Working directory
    #[arg(short = 'C', long, global = true)]
    cwd: Option<String>,

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
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Change working directory if specified
    if let Some(cwd) = &args.cwd {
        std::env::set_current_dir(cwd)?;
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
) -> Result<(Box<dyn provider::Provider>, tool::Registry)> {
    let provider: Box<dyn provider::Provider> = match choice {
        ProviderChoice::Claude => {
            // Use jcode's own OAuth tokens
            let tokens = auth::oauth::load_claude_tokens()?;
            eprintln!("Using Claude with jcode OAuth");
            Box::new(provider::claude::ClaudeProvider::new(tokens))
        }
        ProviderChoice::ClaudeSubprocess => {
            // Fallback: Use Claude Code CLI as subprocess
            eprintln!("Using Claude Code subprocess provider");
            Box::new(provider::claude_subprocess::ClaudeSubprocessProvider::new(
                "claude-sonnet-4-20250514",
                true, // bypass permissions
            ))
        }
        ProviderChoice::Openai => {
            let creds = auth::codex::load_credentials()?;
            Box::new(provider::openai::OpenAIProvider::new(creds))
        }
        ProviderChoice::Auto => {
            // Try jcode's own Claude OAuth first
            if let Ok(tokens) = auth::oauth::load_claude_tokens() {
                eprintln!("Using Claude with jcode OAuth");
                Box::new(provider::claude::ClaudeProvider::new(tokens))
            } else if let Ok(creds) = auth::codex::load_credentials() {
                eprintln!("Using OpenAI/Codex provider");
                Box::new(provider::openai::OpenAIProvider::new(creds))
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
                        let tokens = auth::oauth::login_claude().await?;
                        auth::oauth::save_claude_tokens(&tokens)?;
                        eprintln!("\nSuccessfully logged in to Claude!\n");
                        Box::new(provider::claude::ClaudeProvider::new(tokens))
                    }
                    "2" => {
                        let tokens = auth::oauth::login_openai().await?;
                        auth::oauth::save_openai_tokens(&tokens)?;
                        eprintln!("\nSuccessfully logged in to OpenAI!\n");
                        let creds = auth::codex::load_credentials()?;
                        Box::new(provider::openai::OpenAIProvider::new(creds))
                    }
                    _ => {
                        anyhow::bail!("Invalid choice. Run 'jcode login' to try again.");
                    }
                }
            }
        }
    };

    let registry = tool::Registry::new().await;
    Ok((provider, registry))
}

async fn run_tui(provider: Box<dyn provider::Provider>, registry: tool::Registry) -> Result<()> {
    let terminal = ratatui::init();
    let app = tui::App::new(provider, registry);
    let result = app.run(terminal).await;
    ratatui::restore();
    result
}

async fn run_login(choice: &ProviderChoice) -> Result<()> {
    match choice {
        ProviderChoice::Claude | ProviderChoice::ClaudeSubprocess => {
            eprintln!("Logging in to Claude...");
            let tokens = auth::oauth::login_claude().await?;
            auth::oauth::save_claude_tokens(&tokens)?;
            eprintln!("Successfully logged in to Claude!");
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
