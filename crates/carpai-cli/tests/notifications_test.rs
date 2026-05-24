//! 通知模块集成测试
//!
//! 测试 BrowserOpener, TelegramNotifier, GmailNotifier 的基本功能

#[test]
fn test_browser_opener_invalid_url() {
    let result = carpai_cli::BrowserOpener::open("not-a-url");
    assert!(result.is_err());

    let err = result.unwrap_err().to_string();
    assert!(err.contains("URL must start with"));
}

#[test]
fn test_browser_opener_valid_url_does_not_panic() {
    // try_open should never panic
    carpai_cli::BrowserOpener::try_open("https://example.com");
    // No panic = test passes
}

#[test]
fn test_telegram_notifier_needs_env() {
    // Clear env to ensure clean state
    std::env::remove_var("CARPAI_TELEGRAM_BOT_TOKEN");
    std::env::remove_var("CARPAI_TELEGRAM_CHAT_ID");

    let result = carpai_cli::TelegramNotifier::from_env();
    assert!(result.is_err());

    let err = result.unwrap_err();
    match err {
        carpai_cli::notifications::telegram::TelegramError::NotConfigured => {} // expected
        _ => panic!("Expected NotConfigured error, got: {:?}", err),
    }
}

#[test]
fn test_telegram_notifier_manual_creation() {
    let notifier = carpai_cli::TelegramNotifier::new("test-token".into(), "test-chat".into());
    // Verify it was created without panicking; api_base is private
    // The default api_base should be "https://api.telegram.org"
}

#[test]
fn test_gmail_notifier_manual_creation() {
    let notifier = carpai_cli::GmailNotifier::new("from@test.com".into(), "to@test.com".into());

    let summary = notifier.format_session_summary(
        "Test Session",
        42,
        15000,
        std::time::Duration::from_secs(120),
    );

    assert!(summary.contains("Test Session"), "Summary should contain session title");
    assert!(summary.contains("42"), "Summary should contain message count");
    assert!(summary.contains("15000"), "Summary should contain token count");
    assert!(summary.contains("120"), "Summary should contain duration");
}

#[test]
fn test_gmail_notifier_needs_env() {
    std::env::remove_var("CARPAI_GMAIL_FROM");
    std::env::remove_var("CARPAI_GMAIL_TO");

    let result = carpai_cli::GmailNotifier::from_env();
    assert!(result.is_err());
}
