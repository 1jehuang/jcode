//! CarpAI Server binary entry point

use carpai_server::{Application, ServerConfig};
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Load configuration
    let config_path = std::env::var("CARPAI_CONFIG")
        .unwrap_or_else(|_| "/etc/carpai/server.toml".to_string());
    let config = ServerConfig::load(std::path::Path::new(&config_path))?;

    info!("CarpAI Server v{}", env!("CARGO_PKG_VERSION"));
    info!("Configuration loaded from {}", config_path);

    // Create and run application (async initialization now required)
    let app = Application::new(config).await?;
    app.run().await?;

    Ok(())
}
