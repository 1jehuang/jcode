//! E2E Test Configuration
//!
//! Provides test-specific configuration that uses temporary directories
//! to avoid polluting user data.

use std::path::PathBuf;

/// Default E2E test configuration (TOML format)
///
/// This configuration is designed for isolated testing with:
/// - Temporary working directory
/// - Mock/test model
/// - Minimal context window
/// - Session persistence enabled
pub const TEST_CONFIG_TOML: &str = r#"
# E2E Test Configuration (auto-generated)
# Uses temporary directories to isolate from user data

mode = "cli"
working_dir = "/tmp/carpai-e2e-test"
default_model = "test-model"
max_context_tokens = 1000
tools_enabled = false
default_tool_mode = "auto"

[core]
data_dir = "/tmp/carpai-e2e-data"
session_subdir = "test-sessions"
memory_subdir = "test-memory"

[theme]
syntax_theme = "base16-dark"

[provider]
type = "mock"
endpoint = ""
api_key = ""

[logging]
level = "debug"
file_logging = true
"#;

/// Server-specific test configuration
pub const SERVER_TEST_CONFIG_TOML: &str = r#"
# Server E2E Test Configuration

mode = "server"
bind_addr = "127.0.0.1"
port = 0  # Will be dynamically assigned

[grpc]
enabled = true
port = 0  # Dynamically assigned

[rest]
enabled = true
port = 0  # Dynamically assigned

[auth]
enabled = false  # Disable auth for testing
jwt_secret = "test-secret-key-for-e2e-testing-only"

[enterprise]
enabled = false

[observability]
metrics_enabled = false
tracing_enabled = true
log_level = "debug"

[data]
storage_type = "memory"  # Use in-memory storage for tests
session_ttl_seconds = 3600
"#;

/// Generate a test config file path in temporary directory
pub fn generate_test_config() -> anyhow::Result<PathBuf> {
    let temp_dir = std::env::temp_dir();
    let config_path = temp_dir.join("carpai-e2e-test-config.toml");

    std::fs::write(&config_path, TEST_CONFIG_TOML)
        .with_context(|| format!("Failed to write test config to {}", config_path.display()))?;

    Ok(config_path)
}

/// Generate server test config file
pub fn generate_server_test_config() -> anyhow::Result<PathBuf> {
    let temp_dir = std::env::temp_dir();
    let config_path = temp_dir.join("carpai-server-e2e-test-config.toml");

    std::fs::write(&config_path, SERVER_TEST_CONFIG_TOML)
        .with_context(|| format!("Failed to write server test config to {}", config_path.display()))?;

    Ok(config_path)
}

/// Create a complete test environment with all necessary directories and configs
pub struct TestEnvironment {
    pub temp_dir: PathBuf,
    pub config_path: PathBuf,
    pub data_dir: PathBuf,
    pub session_dir: PathBuf,
}

impl TestEnvironment {
    /// Create a new isolated test environment
    pub fn new(prefix: &str) -> anyhow::Result<Self> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();

        let temp_dir = std::env::temp_dir().join(format!("{}-{}", prefix, timestamp));
        let data_dir = temp_dir.join("data");
        let session_dir = data_dir.join("sessions");
        let memory_dir = data_dir.join("memory");

        // Create directory structure
        std::fs::create_dir_all(&temp_dir)?;
        std::fs::create_dir_all(&data_dir)?;
        std::fs::create_dir_all(&session_dir)?;
        std::fs::create_dir_all(&memory_dir)?;

        // Write customized config with actual paths
        let config_content = format!(
            r#"
mode = "cli"
working_dir = "{}"
default_model = "test-model"
max_context_tokens = 1000
tools_enabled = false

[core]
data_dir = "{}"
session_subdir = "test-sessions"
memory_subdir = "test-memory"

[theme]
syntax_theme = "base16-dark"

[provider]
type = "mock"

[logging]
level = "debug"
"#,
            temp_dir.display(),
            data_dir.display()
        );

        let config_path = temp_dir.join("config.toml");
        std::fs::write(&config_path, config_content)?;

        Ok(Self {
            temp_dir,
            config_path,
            data_dir,
            session_dir,
        })
    }

    /// Get path where sessions will be stored
    pub fn session_storage_path(&self) -> PathBuf {
        self.session_dir.clone()
    }

    /// Clean up all test artifacts
    pub fn cleanup(&self) -> anyhow::Result<()> {
        if self.temp_dir.exists() {
            std::fs::remove_dir_all(&self.temp_dir)?;
        }
        Ok(())
    }
}

impl Drop for TestEnvironment {
    fn drop(&mut self) {
        // Best-effort cleanup
        let _ = self.cleanup();
    }
}
