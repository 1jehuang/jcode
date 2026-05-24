//! Telegram bot notification channel
//!
//! Sends notifications via a Telegram bot using the Bot API.
//! Configured via environment variables:
//! - `CARPAI_TELEGRAM_BOT_TOKEN` — Bot token from @BotFather
//! - `CARPAI_TELEGRAM_CHAT_ID` — Target chat/group ID

use std::env;
use tracing::{info, warn};

/// Error type for Telegram notification operations
#[derive(Debug, thiserror::Error)]
pub enum TelegramError {
    #[error("Not configured: missing CARPAI_TELEGRAM_BOT_TOKEN or CARPAI_TELEGRAM_CHAT_ID")]
    NotConfigured,
    #[error("HTTP request failed: {0}")]
    HttpError(String),
    #[error("Telegram API error: {0}")]
    ApiError(String),
}

/// Telegram bot notifier
pub struct TelegramNotifier {
    bot_token: String,
    chat_id: String,
    client: reqwest::Client,
    #[allow(dead_code)]
    api_base: String,
}

impl TelegramNotifier {
    /// Create a new Telegram notifier from environment variables
    pub fn from_env() -> Result<Self, TelegramError> {
        let bot_token = env::var("CARPAI_TELEGRAM_BOT_TOKEN")
            .map_err(|_| TelegramError::NotConfigured)?;
        let chat_id = env::var("CARPAI_TELEGRAM_CHAT_ID")
            .map_err(|_| TelegramError::NotConfigured)?;

        Ok(Self {
            bot_token,
            chat_id,
            client: reqwest::Client::new(),
            api_base: "https://api.telegram.org".to_string(),
        })
    }

    /// Create a new Telegram notifier with explicit credentials
    pub fn new(bot_token: String, chat_id: String) -> Self {
        Self {
            bot_token,
            chat_id,
            client: reqwest::Client::new(),
            api_base: "https://api.telegram.org".to_string(),
        }
    }

    /// Send a text message to the configured chat
    pub async fn send_message(&self, text: &str) -> Result<serde_json::Value, TelegramError> {
        let url = format!(
            "{}/bot{}/sendMessage",
            self.api_base, self.bot_token
        );

        let body = serde_json::json!({
            "chat_id": self.chat_id,
            "text": text,
            "parse_mode": "Markdown",
            "disable_web_page_preview": false,
        });

        info!(chat_id = %self.chat_id, text_len = text.len(), "Sending Telegram message");

        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| TelegramError::HttpError(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            warn!(status = %status, "Telegram API error");
            return Err(TelegramError::ApiError(format!("HTTP {}", status)));
        }

        let result: serde_json::Value = response
            .json()
            .await
            .map_err(|e| TelegramError::HttpError(e.to_string()))?;

        info!("Telegram message sent successfully");
        Ok(result)
    }

    /// Send a formatted notification message
    pub async fn notify(&self, title: &str, body: &str) -> Result<serde_json::Value, TelegramError> {
        let message = format!("*{}*\n\n{}", title, body);
        self.send_message(&message).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_env_without_vars() {
        let result = TelegramNotifier::from_env();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TelegramError::NotConfigured));
    }
}
