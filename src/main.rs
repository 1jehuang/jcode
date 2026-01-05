mod agent;
mod auth;
mod message;
mod provider;
mod server;
mod tool;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use std::io::{self, Write};

#[derive(Debug, Clone, ValueEnum)]
enum ProviderChoice {
    Claude,
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
            let (provider, registry) = init_provider_and_registry(&args.provider)?;
            let server = server::Server::new(provider, registry);
            server.run().await?;
        }
        Some(Command::Connect) => {
            run_client().await?;
        }
        Some(Command::Run { message }) => {
            let (provider, registry) = init_provider_and_registry(&args.provider)?;
            let mut agent = agent::Agent::new(provider, registry);
            agent.run_once(&message).await?;
        }
        None => {
            // Default: interactive REPL
            let (provider, registry) = init_provider_and_registry(&args.provider)?;
            let mut agent = agent::Agent::new(provider, registry);
            agent.repl().await?;
        }
    }

    Ok(())
}

fn init_provider_and_registry(
    choice: &ProviderChoice,
) -> Result<(Box<dyn provider::Provider>, tool::Registry)> {
    let provider: Box<dyn provider::Provider> = match choice {
        ProviderChoice::Claude => {
            let creds = auth::claude::load_credentials()?;
            Box::new(provider::claude::ClaudeProvider::new(creds))
        }
        ProviderChoice::Openai => {
            let creds = auth::codex::load_credentials()?;
            Box::new(provider::openai::OpenAIProvider::new(creds))
        }
        ProviderChoice::Auto => {
            if let Ok(creds) = auth::claude::load_credentials() {
                eprintln!("Using Claude Max provider");
                Box::new(provider::claude::ClaudeProvider::new(creds))
            } else if let Ok(creds) = auth::codex::load_credentials() {
                eprintln!("Using OpenAI/Codex provider");
                Box::new(provider::openai::OpenAIProvider::new(creds))
            } else {
                anyhow::bail!("No credentials found. Run 'claude' or 'codex login' first.");
            }
        }
    };

    let registry = tool::Registry::new();
    Ok((provider, registry))
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
