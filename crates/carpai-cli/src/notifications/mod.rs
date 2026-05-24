//! # Notifications
//!
//! Notification channels for CarpAI CLI. Supports:
//! - **Telegram**: Bot API notifications
//! - **Gmail**: Email summaries
//! - **Browser**: Open URLs in default browser

pub mod browser;
pub mod gmail;
pub mod telegram;

pub use browser::BrowserOpener;
pub use gmail::GmailNotifier;
pub use telegram::TelegramNotifier;
