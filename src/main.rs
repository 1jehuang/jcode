mod agent;
mod auth;
mod message;
mod provider;
mod tool;

use anyhow::Result;
use clap::{Parser, ValueEnum};

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
    #[arg(short, long, default_value = "auto")]
    provider: ProviderChoice,

    /// Initial prompt (if not provided, starts REPL)
    #[arg(short = 'm', long)]
    message: Option<String>,

    /// Working directory
    #[arg(short = 'C', long)]
    cwd: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Change working directory if specified
    if let Some(cwd) = &args.cwd {
        std::env::set_current_dir(cwd)?;
    }

    // Initialize provider based on args
    let provider: Box<dyn provider::Provider> = match args.provider {
        ProviderChoice::Claude => {
            let creds = auth::claude::load_credentials()?;
            Box::new(provider::claude::ClaudeProvider::new(creds))
        }
        ProviderChoice::Openai => {
            let creds = auth::codex::load_credentials()?;
            Box::new(provider::openai::OpenAIProvider::new(creds))
        }
        ProviderChoice::Auto => {
            // Try Claude first, then OpenAI
            if let Ok(creds) = auth::claude::load_credentials() {
                eprintln!("Using Claude Max provider");
                Box::new(provider::claude::ClaudeProvider::new(creds))
            } else if let Ok(creds) = auth::codex::load_credentials() {
                eprintln!("Using OpenAI/Codex provider");
                Box::new(provider::openai::OpenAIProvider::new(creds))
            } else {
                anyhow::bail!(
                    "No credentials found. Run 'claude' or 'codex login' first."
                );
            }
        }
    };

    // Initialize tools
    let registry = tool::Registry::new();

    // Create agent
    let mut agent = agent::Agent::new(provider, registry);

    // Run with initial message or start REPL
    if let Some(message) = args.message {
        agent.run_once(&message).await?;
    } else {
        agent.repl().await?;
    }

    Ok(())
}
