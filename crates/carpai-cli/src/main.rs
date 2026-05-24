//! CarpAI CLI Entry Point
//!
//! ```bash
//! $ carpai chat          # Interactive TUI mode (default)
//! $ carpai ask "question" # One-shot question
//! $ carpai serve         # Launch server (delegates to carpai-server)
//! $ carpai complete <file> <line> <col>  # Code completion
//! ```

use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "carpai", version, about = "CarpAI — AI Programming Assistant", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand, Debug)]
enum Commands {
    /// Start interactive TUI chat session
    Chat {
        /// Working directory (default: current directory)
        #[arg(short, long)]
        dir: Option<String>,
    },
    /// Ask a single question and exit
    Ask {
        /// The question to ask
        question: String,
        /// Working directory
        #[arg(short, long)]
        dir: Option<String>,
    },
    /// Get code completion for a file location
    Complete {
        /// File path
        file: String,
        /// Line number (1-based)
        line: usize,
        /// Column number (1-based)
        column: usize,
    },
    /// Launch the CarpAI server (requires server feature)
    Serve,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Chat { dir } => cli::chat::run(dir).await,
        Commands::Ask { question, dir } => cli::ask::run(question, dir).await,
        Commands::Complete { file, line, column } => cli::completion::run(file, line, column).await,
        Commands::Serve => cli::serve::run().await,
    }
}
