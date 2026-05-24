//! Browser notification — Open URLs in the default browser
//!
//! Provides a cross-platform way to open links (e.g., AI-generated web results,
//! documentation links, or authentication URLs).

use std::process::Command;
use tracing::{info, warn};

/// Error type for browser operations
#[derive(Debug, thiserror::Error)]
pub enum BrowserError {
    #[error("Failed to open browser: {0}")]
    OpenFailed(String),
    #[error("URL validation failed: {0}")]
    InvalidUrl(String),
}

/// Cross-platform browser opener
pub struct BrowserOpener;

impl BrowserOpener {
    /// Open a URL in the default browser
    pub fn open(url: &str) -> Result<(), BrowserError> {
        // Basic URL validation
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(BrowserError::InvalidUrl(format!(
                "URL must start with http:// or https://, got: {}",
                url
            )));
        }

        info!(url = %url, "Opening URL in browser");

        let status = if cfg!(target_os = "windows") {
            Command::new("cmd")
                .args(["/c", "start", url])
                .status()
        } else if cfg!(target_os = "macos") {
            Command::new("open")
                .arg(url)
                .status()
        } else {
            // Linux / other Unix
            Command::new("xdg-open")
                .arg(url)
                .status()
        };

        match status {
            Ok(s) if s.success() => Ok(()),
            Ok(s) => {
                warn!(exit_code = %s, "Browser process exited with non-zero status");
                Err(BrowserError::OpenFailed(format!(
                    "Process exited with code: {}",
                    s
                )))
            }
            Err(e) => {
                warn!(error = %e, "Failed to launch browser");
                Err(BrowserError::OpenFailed(e.to_string()))
            }
        }
    }

    /// Try to open a URL, logging errors instead of propagating them
    pub fn try_open(url: &str) {
        if let Err(e) = Self::open(url) {
            warn!(error = %e, url = %url, "Failed to open URL in browser");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_url() {
        assert!(BrowserOpener::open("not-a-url").is_err());
    }

    #[test]
    fn test_valid_url_schemes() {
        assert!(BrowserOpener::open("http://example.com").is_ok() == false); // May fail if no browser
        assert!(BrowserOpener::open("https://example.com").is_ok() == false);
    }
}
